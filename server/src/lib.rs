//! HTTP API for remote presets and mint releases.
//!
//! - `GET    /api/v1/presets`                   list of all presets (latest revisions)
//! - `GET    /api/v1/presets/{slug}`            latest revision
//! - `GET    /api/v1/presets/{slug}/{revision}` a specific revision (immutable, cacheable)
//! - `PUT    /api/v1/presets`                   publish (`Authorization: Bearer <publish key>`)
//! - `DELETE /api/v1/presets/{slug}`            unpublish (same auth)
//! - `GET    /api/v1/releases/latest`           signed manifest of the latest release
//! - `PUT    /api/v1/releases/{n}/files/{name}` upload a file of unpublished release n (auth)
//! - `PUT    /api/v1/releases/{n}`              publish release n with its signed manifest (auth)
//! - `GET    /api/v1/releases/{n}/files/{name}` a file of a published release (immutable)
//! - `GET    /health`

pub mod keys;
pub mod releases;
pub mod store;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use remote_preset::PresetUpload;
use remote_preset::release::{MAX_FILE_SIZE, SignedManifest};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use releases::{PublishError, Releases};
use store::{Store, valid_slug};

/// Uploads are a few KB; this leaves room for very large presets.
const MAX_BODY: usize = 512 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub releases: Arc<Releases>,
    pub keys_file: PathBuf,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/presets", get(list).put(publish))
        .route("/api/v1/presets/{slug}", get(latest).delete(unpublish))
        .route("/api/v1/presets/{slug}/{revision}", get(revision))
        .route("/api/v1/releases/latest", get(latest_release))
        .route("/api/v1/releases/{release}", put(publish_release))
        .route(
            "/api/v1/releases/{release}/files/{name}",
            get(release_file).put(upload_release_file),
        )
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

async fn latest_release(State(s): State<AppState>) -> Result<Response, ApiError> {
    let signed = s
        .releases
        .latest()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no release yet".into()))?;
    Ok(fresh(Json(signed)))
}

async fn upload_release_file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((release, name)): Path<(u32, String)>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let publisher = authorize(&s, &headers).await?;
    let too_large = || {
        ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("files are limited to {MAX_FILE_SIZE} bytes"),
        )
    };
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if announced.is_some_and(|n| n > MAX_FILE_SIZE) {
        return Err(too_large());
    }
    let releases = s.releases.clone();
    let file_name = name.clone();
    let path = blocking(move || releases.upload_path(release, &file_name).map_err(internal))
        .await?
        .ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                format!(
                    "release {release} is published already, or {name:?} is no valid file name"
                ),
            )
        })?;

    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let received = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(internal)?;
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
            size += chunk.len() as u64;
            if size > MAX_FILE_SIZE {
                return Err(too_large());
            }
            file.write_all(&chunk).await.map_err(internal)?;
        }
        file.sync_all().await.map_err(internal)?;
        Ok(size)
    }
    .await;
    let size = match received {
        Ok(size) => size,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };
    tokio::fs::rename(&tmp, &path).await.map_err(internal)?;
    info!("{publisher} uploaded {name} for release {release} ({size} bytes)");
    Ok(StatusCode::NO_CONTENT)
}

async fn publish_release(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(release): Path<u32>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let publisher = authorize(&s, &headers).await?;
    let signed: SignedManifest = serde_json::from_slice(&body)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("invalid manifest: {e}")))?;
    let releases = s.releases.clone();
    let manifest = blocking(move || {
        releases.publish(release, signed).map_err(|e| match e {
            PublishError::Invalid(e) => ApiError(StatusCode::UNPROCESSABLE_ENTITY, e),
            PublishError::Io(e) => internal(e),
        })
    })
    .await?;
    info!(
        "{publisher} published release {} ({} files)",
        manifest.release,
        manifest.files.len()
    );
    Ok(fresh(Json(manifest)))
}

async fn release_file(
    State(s): State<AppState>,
    Path((release, name)): Path<(u32, String)>,
) -> Result<Response, ApiError> {
    let releases = s.releases.clone();
    let path = blocking(move || Ok(releases.file(release, &name)))
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such file".into()))?;
    let file = tokio::fs::File::open(&path).await.map_err(internal)?;
    let size = file.metadata().await.map_err(internal)?.len();
    let stream = futures_util::stream::try_unfold(file, |mut file| async move {
        let mut buf = vec![0; 1 << 16];
        let n = file.read(&mut buf).await?;
        buf.truncate(n);
        Ok::<_, std::io::Error>((n > 0).then(|| (Bytes::from(buf), file)))
    });
    let mut r = Body::from_stream(stream).into_response();
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(r)
}
