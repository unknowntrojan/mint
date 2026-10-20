use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mint_server::mods::{ModFile, ModIo, Mods};
use mint_server::{AppState, keys, releases::Releases, router, store::Store};
use remote_preset::release::{LINUX, Manifest, ReleaseFile, SignedManifest, WINDOWS};
use remote_preset::{Preset, PresetMod, PresetSummary, PresetUpload};
use sha2::{Digest, Sha256};
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

/// Signs releases in tests; the server is set up to trust it.
fn release_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[42; 32])
}

fn app(data: &std::path::Path) -> Router {
    app_with_modio(data, None)
}

fn app_with_modio(data: &std::path::Path, modio: Option<ModIo>) -> Router {
    let public = hex::encode(release_key().verifying_key().to_bytes());
    router(AppState {
        store: Arc::new(Store::open(data).unwrap()),
        releases: Arc::new(Releases::open(data, &public).unwrap()),
        mods: Arc::new(Mods::open(data, modio).unwrap()),
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
                name: format!("Mod {id}"),
                modfile_id: Some(id * 100),
                enabled: id % 2 == 1,
                required: false,
                priority: 0,
            })
            .collect(),
        sections: Vec::new(),
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

#[tokio::test]
async fn sections_are_stored_and_count_as_content() {
    let s = server();
    let mut u = upload("Sorted", &[1, 2, 3]);
    let (_, _, body) = call(&s.app, put(Some(&s.key), json(&u))).await;
    assert_eq!(serde_json::from_slice::<Preset>(&body).unwrap().revision, 1);

    u.sections = vec![remote_preset::PresetSection {
        name: "QoL".into(),
        before: Some(2),
    }];
    let (_, _, body) = call(&s.app, put(Some(&s.key), json(&u))).await;
    let p: Preset = serde_json::from_slice(&body).unwrap();
    assert_eq!((p.revision, p.sections.len()), (2, 1));

    let (_, _, body) = call(&s.app, get("/api/v1/presets/sorted")).await;
    assert_eq!(
        serde_json::from_slice::<Preset>(&body).unwrap().sections,
        u.sections
    );

    u.sections[0].before = Some(9);
    assert_eq!(
        call(&s.app, put(Some(&s.key), json(&u))).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

fn put_file(key: Option<&str>, release: u32, name: &str, data: Vec<u8>) -> Request<Body> {
    let mut r = Request::put(format!("/api/v1/releases/{release}/files/{name}"));
    if let Some(k) = key {
        r = r.header(header::AUTHORIZATION, format!("Bearer {k}"));
    }
    r.body(Body::from(data)).unwrap()
}

fn put_manifest(key: &str, release: u32, signed: &SignedManifest) -> Request<Body> {
    Request::put(format!("/api/v1/releases/{release}"))
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(signed).unwrap()))
        .unwrap()
}

fn manifest(release: u32, files: &[(&str, &str, &[u8])]) -> Manifest {
    Manifest {
        release,
        published: 0,
        notes: String::new(),
        files: files
            .iter()
            .map(|(target, name, data)| {
                let file = ReleaseFile {
                    name: name.to_string(),
                    size: data.len() as u64,
                    sha256: hex::encode(Sha256::digest(data)),
                };
                (target.to_string(), file)
            })
            .collect(),
    }
}

#[tokio::test]
async fn releases_upload_publish_and_download() {
    let s = server();
    let k = s.key.as_str();
    // bigger than the preset body limit: release uploads stream
    let linux = vec![1u8; 700 * 1024];
    let windows = b"MZ windows".to_vec();

    assert_eq!(
        call(&s.app, get("/api/v1/releases/latest")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&s.app, put_file(None, 1, "mint-1", linux.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&s.app, put_file(Some(k), 1, "../keys", linux.clone()))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&s.app, put_file(Some(k), 1, "mint-1", linux.clone()))
            .await
            .0,
        StatusCode::NO_CONTENT
    );

    // windows file still missing: refused
    let m = manifest(
        1,
        &[(LINUX, "mint-1", &linux), (WINDOWS, "mint-1.exe", &windows)],
    );
    let signed = SignedManifest::sign(&m, &release_key());
    assert_eq!(
        call(&s.app, put_manifest(k, 1, &signed)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // unpublished files aren't served
    assert_eq!(
        call(&s.app, get("/api/v1/releases/1/files/mint-1")).await.0,
        StatusCode::NOT_FOUND
    );

    call(&s.app, put_file(Some(k), 1, "mint-1.exe", windows.clone())).await;
    // signed with another key: refused even with a valid publish key
    let forged = SignedManifest::sign(&m, &ed25519_dalek::SigningKey::from_bytes(&[1; 32]));
    assert_eq!(
        call(&s.app, put_manifest(k, 1, &forged)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // manifest for another number
    assert_eq!(
        call(&s.app, put_manifest(k, 2, &signed)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(&s.app, put_manifest(k, 1, &signed)).await.0,
        StatusCode::OK
    );

    let (st, _, body) = call(&s.app, get("/api/v1/releases/latest")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<SignedManifest>(&body).unwrap(),
        signed
    );

    let (st, h, body) = call(&s.app, get("/api/v1/releases/1/files/mint-1")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, linux);
    assert!(
        h[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );

    // published releases are frozen
    assert_eq!(
        call(&s.app, put_file(Some(k), 1, "mint-1", vec![9]))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&s.app, put_manifest(k, 1, &signed)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // a restarted server knows the latest release
    let restarted = app(&s.data);
    let (_, _, body) = call(&restarted, get("/api/v1/releases/latest")).await;
    assert_eq!(
        serde_json::from_slice::<SignedManifest>(&body).unwrap(),
        signed
    );
}

fn put_mod(
    key: Option<&str>,
    mod_id: u32,
    modfile_id: u32,
    sha256: &str,
    data: &[u8],
) -> Request<Body> {
    let mut r = Request::put(format!(
        "/api/v1/mods/{mod_id}/files/{modfile_id}?sha256={sha256}"
    ));
    if let Some(k) = key {
        r = r.header(header::AUTHORIZATION, format!("Bearer {k}"));
    }
    r.body(Body::from(data.to_vec())).unwrap()
}

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

#[tokio::test]
async fn mod_files_can_be_uploaded_by_publishers() {
    let s = server();
    let data = b"cached pak".as_slice();
    let sha = sha256(data);

    assert_eq!(
        call(&s.app, put_mod(None, 5, 500, &sha, data)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&s.app, put_mod(Some(&s.key), 5, 500, "xyz", data))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &s.app,
            put_mod(Some(&s.key), 5, 500, &sha256(b"other"), data)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(&s.app, get("/api/v1/mods/5/files/500")).await.0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        call(&s.app, put_mod(Some(&s.key), 5, 500, &sha, data))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (st, headers, body) = call(&s.app, get("/api/v1/mods/5/files/500")).await;
    assert_eq!((st, body.as_slice()), (StatusCode::OK, data));
    assert_eq!(headers["x-sha256"], sha.as_str());
    assert!(
        headers[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    // a modfile never changes: uploading it again is fine and keeps what's there
    assert_eq!(
        call(&s.app, put_mod(Some(&s.key), 5, 500, &sha, data))
            .await
            .0,
        StatusCode::NO_CONTENT
    );

    let (_, _, body) = call(&s.app, get("/api/v1/mods")).await;
    let list: Vec<ModFile> = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        list,
        [ModFile {
            mod_id: 5,
            modfile_id: 500,
            size: data.len() as u64,
            sha256: sha,
        }]
    );

    // without a mod.io token, missing files stay missing, even for preset mods
    call(&s.app, put(Some(&s.key), json(&upload("Pack", &[9])))).await;
    assert_eq!(
        call(&s.app, get("/api/v1/mods/9/files/900")).await.0,
        StatusCode::NOT_FOUND
    );
}

/// Serves `/v1/games/2475/mods/{mod}/files?id=…` and the downloads like mod.io, for token `tok`.
async fn fake_modio(
    files: Vec<(u32, u32, &'static [u8], String)>,
) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use axum::extract::{Query, State};
    use axum::routing::get;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Files = Arc<Vec<(u32, u32, &'static [u8], String)>>;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let downloads = Arc::new(AtomicUsize::new(0));
    let state = (Arc::new(files), downloads.clone(), base.clone());
    let app = Router::new()
        .route(
            "/v1/games/2475/mods/{mod_id}/files",
            get(
                |State((files, _, base)): State<(Files, Arc<AtomicUsize>, String)>,
                 axum::extract::Path(mod_id): axum::extract::Path<u32>,
                 Query(q): Query<HashMap<String, u32>>,
                 headers: axum::http::HeaderMap| async move {
                    if headers.get(header::AUTHORIZATION).map(|v| v.as_bytes())
                        != Some(b"Bearer tok".as_slice())
                    {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    let data: Vec<_> = files
                        .iter()
                        .filter(|f| f.0 == mod_id && Some(&f.1) == q.get("id"))
                        .map(|(m, id, bytes, md5)| {
                            serde_json::json!({
                                "id": id, "mod_id": m, "filesize": bytes.len(),
                                "filehash": { "md5": md5 },
                                "download": { "binary_url": format!("{base}/dl/{id}") },
                            })
                        })
                        .collect();
                    Ok(axum::Json(serde_json::json!({ "data": data })))
                },
            ),
        )
        .route(
            "/dl/{id}",
            get(
                |State((files, downloads, _)): State<(Files, Arc<AtomicUsize>, String)>,
                 axum::extract::Path(id): axum::extract::Path<u32>| async move {
                    downloads.fetch_add(1, Ordering::SeqCst);
                    files
                        .iter()
                        .find(|f| f.1 == id)
                        .map(|f| f.2.to_vec())
                        .ok_or(StatusCode::NOT_FOUND)
                },
            ),
        )
        .with_state(state);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("{base}/v1"), downloads)
}

fn md5(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    hex::encode(Md5::digest(data))
}

#[tokio::test]
async fn mod_files_of_preset_mods_are_fetched_from_modio_once() {
    use std::sync::atomic::Ordering;

    let seven = b"pak seven".as_slice();
    let (api, downloads) = fake_modio(vec![
        (7, 700, seven, md5(seven)),
        (8, 800, b"tampered", md5(b"original")),
    ])
    .await;
    let s = server();
    let app = app_with_modio(&s.data, Some(ModIo::new(&api, "tok").unwrap()));

    // not in any preset: the server doesn't ask mod.io
    assert_eq!(
        call(&app, get("/api/v1/mods/7/files/700")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(downloads.load(Ordering::SeqCst), 0);

    call(&app, put(Some(&s.key), json(&upload("Pack", &[7, 8])))).await;
    for _ in 0..2 {
        let (st, headers, body) = call(&app, get("/api/v1/mods/7/files/700")).await;
        assert_eq!((st, body.as_slice()), (StatusCode::OK, seven));
        assert_eq!(headers["x-sha256"], sha256(seven).as_str());
    }
    assert_eq!(
        downloads.load(Ordering::SeqCst),
        1,
        "served from disk the second time"
    );

    // doesn't match mod.io's MD5: refused, nothing kept
    assert_eq!(
        call(&app, get("/api/v1/mods/8/files/800")).await.0,
        StatusCode::BAD_GATEWAY
    );
    // mod.io has no such file
    assert_eq!(
        call(&app, get("/api/v1/mods/7/files/999")).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, _, body) = call(&app, get("/api/v1/mods")).await;
    let list: Vec<ModFile> = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        list.iter()
            .map(|f| (f.mod_id, f.modfile_id))
            .collect::<Vec<_>>(),
        [(7, 700)]
    );
    let leftovers: Vec<_> = std::fs::read_dir(s.data.join("mods/8")).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
