use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mint_server::{AppState, keys, router, store::Store};
use remote_preset::{Preset, PresetMod, PresetSummary, PresetUpload};
use tower::ServiceExt;

struct Server {
    _dir: tempfile::TempDir,
    data: std::path::PathBuf,
    app: Router,
    key: String,
}

fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    let (key, line) = keys::generate("alex").unwrap();
    std::fs::write(data.join("keys"), format!("# publish keys\n{line}\n")).unwrap();
    let app = app(&data);
    Server {
        _dir: dir,
        data,
        app,
        key,
    }
}

fn app(data: &std::path::Path) -> Router {
    router(AppState {
        store: Arc::new(Store::open(data).unwrap()),
        keys_file: data.join("keys"),
    })
}

fn upload(name: &str, ids: &[u32]) -> PresetUpload {
    PresetUpload {
        name: name.into(),
        mods: ids
            .iter()
            .map(|&id| PresetMod {
                mod_id: id,
                name_id: format!("mod-{id}"),
                modfile_id: Some(id * 100),
                enabled: id % 2 == 1,
                required: false,
            })
            .collect(),
    }
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

fn put(key: Option<&str>, body: impl Into<Body>) -> Request<Body> {
    let mut r = Request::put("/api/v1/presets").header(header::CONTENT_TYPE, "application/json");
    if let Some(k) = key {
        r = r.header(header::AUTHORIZATION, format!("Bearer {k}"));
    }
    r.body(body.into()).unwrap()
}

fn json(u: &PresetUpload) -> String {
    serde_json::to_string(u).unwrap()
}

#[tokio::test]
async fn publish_requires_a_valid_key() {
    let s = server();
    let body = json(&upload("Weekend Pack", &[1, 2]));
    assert_eq!(
        call(&s.app, put(None, body.clone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&s.app, put(Some("nope"), body.clone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    // garbage body without a key: still 401, the body isn't looked at
    assert_eq!(
        call(&s.app, put(None, "{{{")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&s.app, put(Some(&s.key), body)).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn revisions_count_up_and_identical_publishes_are_no_ops() {
    let s = server();
    let (st, _, body) = call(
        &s.app,
        put(Some(&s.key), json(&upload("Weekend Pack", &[1, 2]))),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let p: Preset = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (p.slug.as_str(), p.revision, p.publisher.as_str()),
        ("weekend-pack", 1, "alex")
    );

    let (_, _, body) = call(
        &s.app,
        put(Some(&s.key), json(&upload("Weekend Pack", &[1, 2]))),
    )
    .await;
    assert_eq!(serde_json::from_slice::<Preset>(&body).unwrap().revision, 1);

    let (_, _, body) = call(
        &s.app,
        put(Some(&s.key), json(&upload("Weekend Pack", &[3, 1, 2]))),
    )
    .await;
    assert_eq!(serde_json::from_slice::<Preset>(&body).unwrap().revision, 2);

    let (st, h, body) = call(&s.app, get("/api/v1/presets/weekend-pack")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-cache");
    let latest: Preset = serde_json::from_slice(&body).unwrap();
    assert_eq!(latest.revision, 2);
    assert_eq!(
        latest.mods.iter().map(|m| m.mod_id).collect::<Vec<_>>(),
        [3, 1, 2]
    );

    let (st, h, body) = call(&s.app, get("/api/v1/presets/weekend-pack/1")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        h[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert_eq!(
        serde_json::from_slice::<Preset>(&body).unwrap().mods.len(),
        2
    );

    let (_, _, body) = call(&s.app, get("/api/v1/presets")).await;
    let list: Vec<PresetSummary> = serde_json::from_slice(&body).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].revision, list[0].mod_count), (2, 3));
}

#[tokio::test]
async fn bad_uploads_and_paths_are_rejected() {
    let s = server();
    let k = Some(s.key.as_str());
    assert_eq!(
        call(&s.app, put(k, "not json")).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&s.app, put(k, json(&upload("!!!", &[1])))).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(&s.app, put(k, json(&upload("Dupes", &[1, 1]))))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(&s.app, put(k, "x".repeat(600 * 1024))).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );

    for path in [
        "/api/v1/presets/nope",
        "/api/v1/presets/..%2Fkeys",
        "/api/v1/presets/..%2F..%2Fetc/1",
        "/api/v1/presets/nope/1",
    ] {
        assert_eq!(
            call(&s.app, get(path)).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn unpublish_and_restart() {
    let s = server();
    call(&s.app, put(Some(&s.key), json(&upload("Keep", &[1])))).await;
    call(&s.app, put(Some(&s.key), json(&upload("Gone", &[2])))).await;

    let del = |key: Option<&str>| {
        let mut r = Request::delete("/api/v1/presets/gone");
        if let Some(k) = key {
            r = r.header(header::AUTHORIZATION, format!("Bearer {k}"));
        }
        r.body(Body::empty()).unwrap()
    };
    assert_eq!(call(&s.app, del(None)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&s.app, del(Some(&s.key))).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&s.app, del(Some(&s.key))).await.0,
        StatusCode::NOT_FOUND
    );

    // a fresh server on the same data sees the same presets
    let restarted = app(&s.data);
    let (_, _, body) = call(&restarted, get("/api/v1/presets")).await;
    let list: Vec<PresetSummary> = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        list.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
        ["keep"]
    );
    assert_eq!(
        std::fs::read_dir(s.data.join("deleted")).unwrap().count(),
        1
    );
}

#[tokio::test]
async fn revoked_keys_stop_working_without_restart() {
    let s = server();
    std::fs::write(s.data.join("keys"), "").unwrap();
    let body = json(&upload("Late", &[1]));
    assert_eq!(
        call(&s.app, put(Some(&s.key), body)).await.0,
        StatusCode::UNAUTHORIZED
    );
}
