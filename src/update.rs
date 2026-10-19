//! Self-update from the preset server.
//!
//! Builds made by `release.sh` carry their release number (`MINT_RELEASE`). On start they ask the
//! server for the latest release, and if it's newer, download the binary for this platform,
//! check it against the signed manifest and replace themselves. Other builds never update.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::TryStreamExt;
use remote_preset::release::{LINUX, Manifest, PUBLIC_KEY, SignedManifest, WINDOWS};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::Sender;
use tracing::info;

/// The release this build is, or `None` for builds not made by `release.sh`.
pub const RELEASE: Option<u32> = match option_env!("MINT_RELEASE") {
    Some(n) => Some(parse_release(n)),
    None => None,
};

const fn parse_release(s: &str) -> u32 {
    let b = s.as_bytes();
    assert!(!b.is_empty(), "MINT_RELEASE is empty");
    let mut n: u32 = 0;
    let mut i = 0;
    while i < b.len() {
        assert!(b[i].is_ascii_digit(), "MINT_RELEASE must be a number");
        n = n * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    n
}

/// Which file of a release this build runs.
const TARGET: &str = if cfg!(windows) { WINDOWS } else { LINUX };

/// Remembers the release last installed, so a broken update can't loop.
const ATTEMPT_FILE: &str = "update-attempt";

pub fn describe() -> String {
    match RELEASE {
        Some(n) => format!("release {n}"),
        None => "development build (doesn't update itself)".into(),
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("mint/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

/// The latest release if it's newer than this build. `Ok(None)` for development builds, when the
/// server has no newer release, or none at all.
pub async fn check(server: &str) -> Result<Option<Manifest>, String> {
    let Some(current) = RELEASE else {
        return Ok(None);
    };
    let res = client()?
        .get(format!("{server}/api/v1/releases/latest"))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(res.status().to_string());
    }
    let signed: SignedManifest = res.json().await.map_err(|e| e.to_string())?;
    let manifest = signed.verify(PUBLIC_KEY)?;
    if !manifest.files.contains_key(TARGET) {
        return Err(format!(
            "release {} has no {TARGET} build",
            manifest.release
        ));
    }
    Ok((manifest.release > current).then_some(manifest))
}

/// Whether installing `release` was already tried without this build becoming it. Then it
/// failed somehow, and trying again would only loop.
pub fn attempted_before(cache_dir: &Path, release: u32) -> bool {
    std::fs::read_to_string(cache_dir.join(ATTEMPT_FILE))
        .is_ok_and(|s| s.trim().parse() == Ok(release))
}

/// Downloads `manifest`'s binary for this platform, verifies it and replaces the running
/// executable with it. Sends `(bytes so far, total)` while downloading. Returns the path to
/// start the new version from.
pub async fn install(
    server: &str,
    manifest: &Manifest,
    cache_dir: &Path,
    progress: Sender<(u64, u64)>,
) -> Result<PathBuf, String> {
    let file = &manifest.files[TARGET];
    let url = format!(
        "{server}/api/v1/releases/{}/files/{}",
        manifest.release, file.name
    );
    info!("downloading {url}");
    let res = client()?
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;

    let mut tmp = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.try_next().await.map_err(|e| e.to_string())? {
        done += chunk.len() as u64;
        if done > file.size {
            return Err("download is larger than the release says".into());
        }
        hasher.update(&chunk);
        tmp.write_all(&chunk).map_err(|e| e.to_string())?;
        let _ = progress.try_send((done, file.size));
    }
    if done != file.size || hex::encode(hasher.finalize()) != file.sha256 {
        return Err("download doesn't match the signed release".into());
    }
    tmp.flush().map_err(|e| e.to_string())?;

    std::fs::write(cache_dir.join(ATTEMPT_FILE), manifest.release.to_string())
        .map_err(|e| e.to_string())?;
    let new = tmp.into_temp_path();
    let exe = tokio::task::spawn_blocking(move || -> std::io::Result<PathBuf> {
        // resolve first: on Linux /proc/self/exe points at the replaced file afterwards
        let exe = std::env::current_exe()?;
        self_replace::self_replace(&new)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(exe)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("replacing mint: {e}"))?;
    info!("installed release {}", manifest.release);
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_numbers_parse() {
        assert_eq!(parse_release("1"), 1);
        assert_eq!(parse_release("120"), 120);
    }

    #[test]
    fn a_failed_attempt_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!attempted_before(dir.path(), 3));
        std::fs::write(dir.path().join(ATTEMPT_FILE), "3").unwrap();
        assert!(attempted_before(dir.path(), 3));
        assert!(!attempted_before(dir.path(), 4));
    }
}
