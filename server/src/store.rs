//! Presets on disk: `<data>/presets/<slug>/<revision>.json`, one immutable file per published
//! revision. Unpublished presets are moved to `<data>/deleted/<slug>-<unix time>/`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use remote_preset::{Preset, PresetSummary, PresetUpload, slug};

pub struct Store {
    presets: PathBuf,
    deleted: PathBuf,
    /// Latest revision of every preset.
    latest: RwLock<BTreeMap<String, Preset>>,
    /// Serializes publishes and deletes.
    write: Mutex<()>,
}

/// Slugs in URLs come from outside; only these can name a directory.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Store {
    pub fn open(data: &Path) -> io::Result<Self> {
        let presets = data.join("presets");
        let deleted = data.join("deleted");
        fs::create_dir_all(&presets)?;
        fs::create_dir_all(&deleted)?;

        let mut latest = BTreeMap::new();
        for dir in fs::read_dir(&presets)? {
            let dir = dir?;
            let name = dir.file_name().to_string_lossy().into_owned();
            if !valid_slug(&name) || !dir.file_type()?.is_dir() {
                continue;
            }
            if let Some(rev) = revisions(&dir.path())?.into_iter().max() {
                latest.insert(name, read(&dir.path().join(format!("{rev}.json")))?);
            }
        }
        Ok(Self {
            presets,
            deleted,
            latest: RwLock::new(latest),
            write: Mutex::new(()),
        })
    }

    /// All presets, sorted by name.
    pub fn list(&self) -> Vec<PresetSummary> {
        let mut list: Vec<_> = self
            .latest
            .read()
            .unwrap()
            .values()
            .map(PresetSummary::from)
            .collect();
        list.sort_by_key(|p| p.name.to_lowercase());
        list
    }

    pub fn latest(&self, slug: &str) -> Option<Preset> {
        self.latest.read().unwrap().get(slug).cloned()
    }

    pub fn revision(&self, slug: &str, revision: u32) -> io::Result<Option<Preset>> {
        if !valid_slug(slug) {
            return Ok(None);
        }
        match read(&self.presets.join(slug).join(format!("{revision}.json"))) {
            Ok(p) => Ok(Some(p)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Stores a new revision. Publishing exactly what is already the latest revision returns that
    /// revision instead of creating a new one. The upload must be validated already.
    pub fn publish(&self, upload: PresetUpload, publisher: &str, now: u64) -> io::Result<Preset> {
        let _write = self.write.lock().unwrap();
        let name = upload.name.trim().to_string();
        let slug = slug(&name);

        let previous = self.latest(&slug);
        if let Some(prev) = &previous
            && prev.name == name
            && prev.mods == upload.mods
        {
            return Ok(prev.clone());
        }

        let preset = Preset {
            name,
            revision: previous.map_or(1, |p| p.revision + 1),
            published: now,
            publisher: publisher.to_string(),
            mods: upload.mods,
            slug: slug.clone(),
        };
        let dir = self.presets.join(&slug);
        fs::create_dir_all(&dir)?;
        write_atomic(
            &dir,
            &format!("{}.json", preset.revision),
            &serde_json::to_vec_pretty(&preset)?,
        )?;

        self.latest.write().unwrap().insert(slug, preset.clone());
        Ok(preset)
    }

    /// Unpublishes a preset. Its revisions are kept under `deleted/`. Returns whether it existed.
    pub fn delete(&self, slug: &str, now: u64) -> io::Result<bool> {
        let _write = self.write.lock().unwrap();
        if !valid_slug(slug) || self.latest(slug).is_none() {
            return Ok(false);
        }
        fs::rename(
            self.presets.join(slug),
            self.deleted.join(format!("{slug}-{now}")),
        )?;
        self.latest.write().unwrap().remove(slug);
        Ok(true)
    }
}

fn revisions(dir: &Path) -> io::Result<Vec<u32>> {
    let mut revs = Vec::new();
    for f in fs::read_dir(dir)? {
        let name = f?.file_name();
        if let Some(rev) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.parse().ok())
        {
            revs.push(rev);
        }
    }
    Ok(revs)
}

fn read(path: &Path) -> io::Result<Preset> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// Writes via a temporary file and rename, so readers never see a partial file.
fn write_atomic(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = fs::File::create(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    fs::rename(&tmp, dir.join(name))?;
    fs::File::open(dir)?.sync_all()
}
