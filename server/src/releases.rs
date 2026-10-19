//! mint releases on disk: `<data>/releases/<n>/` holds the binaries of release `n` and, once it
//! is published, `manifest.json` (a [`SignedManifest`]). The highest published `n` is the latest
//! release. Files can only be uploaded for releases that aren't published yet, so a published
//! release never changes.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use remote_preset::release::{Manifest, SignedManifest, valid_file_name};
use sha2::{Digest, Sha256};

const MANIFEST: &str = "manifest.json";

pub struct Releases {
    dir: PathBuf,
    /// Hex ed25519 key manifests must be signed with.
    public_key: String,
    latest: RwLock<Option<(Manifest, SignedManifest)>>,
    /// Serializes publishes.
    write: Mutex<()>,
}

/// Why a publish was refused.
pub enum PublishError {
    Invalid(String),
    Io(io::Error),
}

impl From<io::Error> for PublishError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl Releases {
    pub fn open(data: &Path, public_key: &str) -> io::Result<Self> {
        let dir = data.join("releases");
        fs::create_dir_all(&dir)?;
        let mut latest: Option<(Manifest, SignedManifest)> = None;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let Some(n) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let path = entry.path().join(MANIFEST);
            if !path.exists() || latest.as_ref().is_some_and(|(m, _)| m.release >= n) {
                continue;
            }
            let signed: SignedManifest = serde_json::from_slice(&fs::read(&path)?)?;
            let manifest = signed
                .verify(public_key)
                .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
            latest = Some((manifest, signed));
        }
        Ok(Self {
            dir,
            public_key: public_key.to_string(),
            latest: RwLock::new(latest),
            write: Mutex::new(()),
        })
    }

    pub fn latest(&self) -> Option<SignedManifest> {
        self.latest.read().unwrap().as_ref().map(|(_, s)| s.clone())
    }

    /// 0 if nothing is published yet.
    pub fn latest_number(&self) -> u32 {
        self.latest
            .read()
            .unwrap()
            .as_ref()
            .map_or(0, |(m, _)| m.release)
    }

    /// A file of a published release.
    pub fn file(&self, release: u32, name: &str) -> Option<PathBuf> {
        if !valid_file_name(name) || release == 0 || release > self.latest_number() {
            return None;
        }
        let dir = self.dir.join(release.to_string());
        let path = dir.join(name);
        // only what the release's manifest lists, not leftovers of a failed upload
        let signed: SignedManifest =
            serde_json::from_slice(&fs::read(dir.join(MANIFEST)).ok()?).ok()?;
        let manifest: Manifest = serde_json::from_str(&signed.manifest).ok()?;
        manifest
            .files
            .values()
            .any(|f| f.name == name)
            .then_some(path)
            .filter(|p| p.is_file())
    }

    /// Where an upload of `name` for unpublished release `release` goes, after it's complete.
    pub fn upload_path(&self, release: u32, name: &str) -> io::Result<Option<PathBuf>> {
        if !valid_file_name(name) || release <= self.latest_number() {
            return Ok(None);
        }
        let dir = self.dir.join(release.to_string());
        fs::create_dir_all(&dir)?;
        Ok(Some(dir.join(name)))
    }

    /// Publishes release `release`. Its manifest must verify, carry that number, be newer than
    /// the latest, and list only files that were uploaded with exactly that size and hash.
    pub fn publish(&self, release: u32, signed: SignedManifest) -> Result<Manifest, PublishError> {
        let _write = self.write.lock().unwrap();
        let manifest = signed
            .verify(&self.public_key)
            .map_err(PublishError::Invalid)?;
        if manifest.release != release {
            return Err(PublishError::Invalid(format!(
                "manifest is for release {}",
                manifest.release
            )));
        }
        if release <= self.latest_number() {
            return Err(PublishError::Invalid(format!(
                "release {} is already published",
                self.latest_number()
            )));
        }
        let dir = self.dir.join(release.to_string());
        for file in manifest.files.values() {
            let path = dir.join(&file.name);
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size != file.size || sha256_file(&path)? != file.sha256 {
                return Err(PublishError::Invalid(format!(
                    "{} wasn't uploaded, or differs from the manifest",
                    file.name
                )));
            }
        }
        crate::store::write_atomic(&dir, MANIFEST, &serde_json::to_vec_pretty(&signed).unwrap())?;
        *self.latest.write().unwrap() = Some((manifest.clone(), signed));
        Ok(manifest)
    }
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut f = fs::File::open(path)?;
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
