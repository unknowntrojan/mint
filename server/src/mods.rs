//! Mirror of mod.io mod files: `<data>/mods/<mod_id>/<modfile_id>`, and next to it
//! `<modfile_id>.json` (a [`ModFile`]), written last; a file without one is incomplete. A modfile
//! never changes on mod.io, so its file here never changes either.
//!
//! Files get here two ways: a publisher uploads them from mint's cache (checked against the
//! SHA-256 mint stores them under), or the server downloads them from mod.io the first time
//! someone asks for one (checked against mod.io's MD5 and size), for mods it has a file of
//! already or that a published preset lists.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use md5::Md5;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::info;

/// mod.io's id for Deep Rock Galactic.
const DRG: u32 = 2475;

/// Largest file downloaded from mod.io; the biggest DRG mods are around 120 MB.
pub const MAX_PULL_SIZE: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModFile {
    pub mod_id: u32,
    pub modfile_id: u32,
    pub size: u64,
    pub sha256: String,
}

pub struct Mods {
    dir: PathBuf,
    modio: Option<ModIo>,
    /// One download from mod.io per modfile at a time; later requests wait for it.
    fetching: Mutex<HashMap<u32, Arc<tokio::sync::Mutex<()>>>>,
}

/// Why a file couldn't be downloaded from mod.io.
#[derive(Debug)]
pub enum FetchError {
    /// mod.io has no such file for that mod.
    NotFound,
    /// mod.io failed, or sent something that doesn't match its own metadata.
    Upstream(String),
    Io(io::Error),
}

impl From<io::Error> for FetchError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl Mods {
    pub fn open(data: &Path, modio: Option<ModIo>) -> io::Result<Self> {
        let dir = data.join("mods");
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            modio,
            fetching: Mutex::new(HashMap::new()),
        })
    }

    /// Whether missing files can be downloaded from mod.io.
    pub fn can_fetch(&self) -> bool {
        self.modio.is_some()
    }

    fn file_path(&self, mod_id: u32, modfile_id: u32) -> PathBuf {
        self.dir
            .join(mod_id.to_string())
            .join(modfile_id.to_string())
    }

    /// Whether any file of the mod is here.
    pub fn has_mod(&self, mod_id: u32) -> io::Result<bool> {
        let entries = match fs::read_dir(self.dir.join(mod_id.to_string())) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        for entry in entries {
            if entry?
                .file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(".json"))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The file and its metadata, if it's here.
    pub fn get(&self, mod_id: u32, modfile_id: u32) -> io::Result<Option<(PathBuf, ModFile)>> {
        let path = self.file_path(mod_id, modfile_id);
        let meta = match fs::read(path.with_extension("json")) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let meta: ModFile = serde_json::from_slice(&meta)?;
        Ok(path.is_file().then_some((path, meta)))
    }

    /// Every file here.
    pub fn list(&self) -> io::Result<Vec<ModFile>> {
        let mut files = Vec::new();
        for dir in fs::read_dir(&self.dir)? {
            let dir = dir?;
            let Some(mod_id) = dir.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            for entry in fs::read_dir(dir.path())? {
                let name = entry?.file_name();
                let Some(modfile_id) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                    continue;
                };
                if let Some((_, meta)) = self.get(mod_id, modfile_id)? {
                    files.push(meta);
                }
            }
        }
        files.sort_by_key(|f| (f.mod_id, f.modfile_id));
        Ok(files)
    }

    /// Where to write an incoming file before [`Mods::commit`]. `source` keeps an upload and a
    /// download of the same file apart.
    pub fn tmp_path(&self, mod_id: u32, modfile_id: u32, source: &str) -> io::Result<PathBuf> {
        let dir = self.dir.join(mod_id.to_string());
        fs::create_dir_all(&dir)?;
        Ok(dir.join(format!(".{modfile_id}.{source}.tmp")))
    }

    /// Moves a complete, checked file from [`Mods::tmp_path`] into place.
    pub fn commit(&self, tmp: &Path, meta: &ModFile) -> io::Result<()> {
        let path = self.file_path(meta.mod_id, meta.modfile_id);
        fs::rename(tmp, &path)?;
        crate::store::write_atomic(
            path.parent().unwrap(),
            &format!("{}.json", meta.modfile_id),
            &serde_json::to_vec_pretty(meta).unwrap(),
        )
    }

    /// Downloads a file from mod.io unless it's here already. Needs [`Mods::can_fetch`].
    pub async fn fetch(&self, mod_id: u32, modfile_id: u32) -> Result<ModFile, FetchError> {
        let modio = self
            .modio
            .as_ref()
            .ok_or_else(|| FetchError::Upstream("no mod.io token configured".into()))?;
        // entries stay: one small lock per file ever requested
        let lock = self
            .fetching
            .lock()
            .unwrap()
            .entry(modfile_id)
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        match self.get(mod_id, modfile_id)? {
            Some((_, meta)) => Ok(meta),
            None => modio.download(self, mod_id, modfile_id).await,
        }
    }
}

/// Downloads mod files from mod.io with the server operator's OAuth token.
pub struct ModIo {
    client: reqwest::Client,
    /// `https://api.mod.io/v1`, or a fake one in tests.
    api: String,
    token: String,
}

#[derive(Deserialize)]
struct FileList {
    data: Vec<ModioFile>,
}

#[derive(Deserialize)]
struct ModioFile {
    id: u32,
    mod_id: u32,
    filesize: u64,
    filehash: FileHash,
    download: Download,
}

#[derive(Deserialize)]
struct FileHash {
    md5: String,
}

#[derive(Deserialize)]
struct Download {
    binary_url: String,
}

impl ModIo {
    pub fn new(api: &str, token: &str) -> reqwest::Result<Self> {
        let client = reqwest::Client::builder()
            // mod.io answers requests without one with 403
            .user_agent(concat!("mint_server/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            client,
            api: api.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    async fn download(
        &self,
        mods: &Mods,
        mod_id: u32,
        modfile_id: u32,
    ) -> Result<ModFile, FetchError> {
        let upstream = |e: reqwest::Error| FetchError::Upstream(e.to_string());
        // the list endpoint: single-file GETs intermittently 403 for non-admins (seen 2026-07)
        let list: FileList = self
            .client
            .get(format!("{}/games/{DRG}/mods/{mod_id}/files", self.api))
            .query(&[("id", modfile_id)])
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(upstream)?
            .json()
            .await
            .map_err(upstream)?;
        let file = list
            .data
            .into_iter()
            .find(|f| f.id == modfile_id && f.mod_id == mod_id)
            .ok_or(FetchError::NotFound)?;
        if file.filesize > MAX_PULL_SIZE {
            return Err(FetchError::Upstream(format!(
                "file is {} bytes, more than {MAX_PULL_SIZE}",
                file.filesize
            )));
        }

        info!("downloading mod {mod_id} file {modfile_id} from mod.io");
        // reqwest drops the token if this redirects to another host (the CDN)
        let res = self
            .client
            .get(&file.download.binary_url)
            .bearer_auth(&self.token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(upstream)?;
        let tmp = mods.tmp_path(mod_id, modfile_id, "modio")?;
        let received = async {
            let mut out = tokio::fs::File::create(&tmp).await?;
            let (mut md5, mut sha256, mut size) = (Md5::new(), Sha256::new(), 0u64);
            let mut stream = res.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(upstream)?;
                size += chunk.len() as u64;
                if size > file.filesize {
                    return Err(FetchError::Upstream(
                        "download is larger than announced".into(),
                    ));
                }
                md5.update(&chunk);
                sha256.update(&chunk);
                out.write_all(&chunk).await?;
            }
            out.sync_all().await?;
            if size != file.filesize
                || !hex::encode(md5.finalize()).eq_ignore_ascii_case(&file.filehash.md5)
            {
                return Err(FetchError::Upstream(
                    "download doesn't match mod.io's size and MD5".into(),
                ));
            }
            Ok(ModFile {
                mod_id,
                modfile_id,
                size,
                sha256: hex::encode(sha256.finalize()),
            })
        }
        .await;
        match received {
            Ok(meta) => {
                mods.commit(&tmp, &meta)?;
                info!(
                    "mirrored mod {mod_id} file {modfile_id} ({} bytes)",
                    meta.size
                );
                Ok(meta)
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                Err(e)
            }
        }
    }
}
