//! HTTP API for remote presets.
//!
//! - `GET    /api/v1/presets`                   list of all presets (latest revisions)
//! - `GET    /api/v1/presets/{slug}`            latest revision
//! - `GET    /api/v1/presets/{slug}/{revision}` a specific revision (immutable, cacheable)
//! - `PUT    /api/v1/presets`                   publish (`Authorization: Bearer <publish key>`)
//! - `DELETE /api/v1/presets/{slug}`            unpublish (same auth)
//! - `GET    /health`

pub mod keys;
pub mod store;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use remote_preset::PresetUpload;
use tracing::{info, warn};

use store::{Store, valid_slug};

/// Uploads are a few KB; this leaves room for very large presets.
const MAX_BODY: usize = 512 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub keys_file: PathBuf,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/presets", get(list).put(publish))
        .route("/api/v1/presets/{slug}", get(latest).delete(unpublish))
        .route("/api/v1/presets/{slug}/{revision}", get(revision))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    warn!("internal error: {e}");
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "no such preset".into())
}

/// Latest data may change any time; tell caches to revalidate.
fn fresh(body: impl IntoResponse) -> Response {
    let mut r = body.into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Runs blocking store work off the async executor.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)?
}

async fn list(State(s): State<AppState>) -> Response {
    fresh(Json(s.store.list()))
}

async fn latest(State(s): State<AppState>, Path(slug): Path<String>) -> Result<Response, ApiError> {
    let preset = s.store.latest(&slug).ok_or_else(not_found)?;
    Ok(fresh(Json(preset)))
}

async fn revision(
    State(s): State<AppState>,
    Path((slug, revision)): Path<(String, u32)>,
) -> Result<Response, ApiError> {
    let preset = blocking(move || s.store.revision(&slug, revision).map_err(internal))
        .await?
        .ok_or_else(not_found)?;
    let mut r = Json(preset).into_response();
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(r)
}

/// Returns the publisher name of a valid `Authorization: Bearer <key>` header.
async fn authorize(s: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let unauthorized = || {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "invalid or missing publish key".into(),
        )
    };
    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)?
        .to_string();
    let keys_file = s.keys_file.clone();
    // re-read every time, so adding or revoking a key needs no restart
    let publisher = blocking(move || keys::check(&keys_file, &key).map_err(internal)).await?;
    publisher.ok_or_else(|| {
        warn!("rejected publish key");
        unauthorized()
    })
}

async fn publish(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    // authorize before even parsing what was sent
    let publisher = authorize(&s, &headers).await?;
    let upload: PresetUpload = serde_json::from_slice(&body)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("invalid preset: {e}")))?;
    upload
        .validate()
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;

    let store = s.store.clone();
    let who = publisher.clone();
    let preset = blocking(move || store.publish(upload, &who, now()).map_err(internal)).await?;
    info!(
        "{publisher} published {:?} revision {} ({} mods)",
        preset.name,
        preset.revision,
        preset.mods.len()
    );
    Ok(fresh(Json(preset)))
}

async fn unpublish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    let publisher = authorize(&s, &headers).await?;
    if !valid_slug(&slug) {
        return Err(not_found());
    }
    let store = s.store.clone();
    let what = slug.clone();
    if blocking(move || store.delete(&what, now()).map_err(internal)).await? {
        info!("{publisher} unpublished {slug:?}");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}
