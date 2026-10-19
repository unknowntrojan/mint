//! Remote presets: mod presets published to a mint server (`server/`; the format is in
//! `remote_preset`).
//!
//! The server's presets are fetched on start and cached, so remote presets keep working offline.
//! They are read-only: installing one uses its list as is, copying one makes a local preset.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use remote_preset::{Preset, PresetMod, PresetSection, PresetSummary, PresetUpload};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::providers::{ModSpecification, ModStore};
use crate::state::mod_list::Row;
use crate::state::{
    ModConfig, ModData_v0_1_0 as ModData, ModOrGroup, ModProfile_v0_1_0 as ModProfile, Section,
};

/// Shown in front of remote presets in the preset list.
pub const PREFIX: &str = "[Remote] ";

/// The presets a server offered the last time it was reachable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemotePresets {
    pub server: String,
    pub presets: Vec<Preset>,
}

impl RemotePresets {
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes)
            .inspect_err(|e| warn!("ignoring {}: {e}", path.display()))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// The cached presets if they came from `server`.
    pub fn for_server(&self, server: &str) -> &[Preset] {
        if self.server == server {
            &self.presets
        } else {
            &[]
        }
    }

    /// Adds or replaces a preset fetched from (or just published to) `server`.
    pub fn upsert(&mut self, server: &str, preset: Preset) {
        if self.server != server {
            *self = Self {
                server: server.to_string(),
                presets: Vec::new(),
            };
        }
        self.presets.retain(|p| p.slug != preset.slug);
        self.presets.push(preset);
        sort(&mut self.presets);
    }
}

fn sort(presets: &mut [Preset]) {
    presets.sort_by_cached_key(|p| p.name.to_lowercase());
}

/// The mod.io URL mint uses for a preset mod.
pub fn spec(m: &PresetMod) -> ModSpecification {
    ModSpecification::new(match m.modfile_id {
        Some(file) => format!("https://mod.io/g/drg/m/{}#{}/{file}", m.name_id, m.mod_id),
        None => format!("https://mod.io/g/drg/m/{}#{}", m.name_id, m.mod_id),
    })
}

/// The mod's page on mod.io.
pub fn page_url(m: &PresetMod) -> String {
    format!("https://mod.io/g/drg/m/{}", m.name_id)
}

/// `(name_id, mod_id, modfile_id)` of a mod.io URL that includes the mod ID.
fn parse_modio(url: &str) -> Option<(&str, u32, Option<u32>)> {
    let rest = url.strip_prefix("https://mod.io/g/drg/m/")?;
    let (name_id, ids) = rest.split_once('#')?;
    let (mod_id, file) = match ids.split_once('/') {
        Some((mod_id, file)) => (mod_id, Some(file.parse().ok()?)),
        None => (ids, None),
    };
    Some((name_id.trim_end_matches('/'), mod_id.parse().ok()?, file))
}

/// What publishing `profile` uploads, and how many of its entries are left out (not on mod.io,
/// or not resolved yet).
pub fn upload(data: &ModData, profile: &str, store: &ModStore) -> (PresetUpload, usize) {
    let entries = data
        .profiles
        .get(profile)
        .map_or(&[][..], |p| p.mods.as_slice());
    let mut mods = Vec::new();
    let mut skipped = 0;
    // mod ID of each entry, for the section anchors
    let mut ids: Vec<Option<u32>> = Vec::with_capacity(entries.len());
    let mut seen = HashSet::new();
    for entry in entries {
        let ModOrGroup::Individual(mc) = entry else {
            skipped += 1;
            ids.push(None);
            continue;
        };
        let info = store.get_mod_info(&mc.spec);
        let parsed = parse_modio(&mc.spec.url)
            .or_else(|| info.as_ref().and_then(|i| parse_modio(&i.spec.url)))
            .map(|(name_id, id, file)| (name_id.to_string(), id, file))
            .filter(|(_, id, _)| seen.insert(*id));
        let Some((name_id, mod_id, modfile_id)) = parsed else {
            skipped += 1;
            ids.push(None);
            continue;
        };
        // "latest" goes out as the file it resolves to here, so everyone gets the same files
        // and a republish after "Update cache" hands out the new ones
        let modfile_id = modfile_id.or_else(|| store.modio_latest_modfile(mod_id));
        let name = info.map(|i| i.name).unwrap_or_default();
        mods.push(PresetMod {
            mod_id,
            name_id,
            name: name.chars().filter(|c| !c.is_control()).take(200).collect(),
            modfile_id,
            enabled: mc.enabled,
            required: mc.required,
            priority: mc.priority.clamp(-999, 999),
        });
        ids.push(Some(mod_id));
    }

    let rows = data.rows();
    let sections = rows
        .iter()
        .enumerate()
        .filter_map(|(pos, row)| {
            let Row::Section(s) = *row else {
                return None;
            };
            let before = rows[pos..].iter().find_map(|r| match r {
                Row::Entry(i) => ids.get(*i).copied().flatten(),
                Row::Section(_) => None,
            });
            Some(PresetSection {
                name: data.sections.get(s)?.name.clone(),
                before,
            })
        })
        .collect();

    (
        PresetUpload {
            name: profile.to_string(),
            mods,
            sections,
        },
        skipped,
    )
}

/// The enabled mods of a remote preset in install order: highest load priority first, then list
/// order (like local presets). Disabled mods are never downloaded.
pub fn install_specs(preset: &Preset) -> Vec<ModSpecification> {
    let mut mods: Vec<&PresetMod> = preset.mods.iter().filter(|m| m.enabled).collect();
    mods.sort_by_key(|m| -m.priority);
    mods.into_iter().map(spec).collect()
}

/// Copies a remote preset into a new local preset `name` and makes it active. Mods the shared
/// list doesn't have yet are added (disabled in the other presets) after the mod they follow in
/// the remote preset; for mods it has, the local order and versions stay. Without local sections
/// the remote ones are adopted.
pub fn copy_to_profile(data: &mut ModData, preset: &Preset, name: &str) {
    let mods = preset
        .mods
        .iter()
        .map(|m| {
            ModOrGroup::Individual(ModConfig {
                spec: spec(m),
                required: m.required,
                enabled: m.enabled,
                priority: m.priority,
            })
        })
        .collect();
    data.profiles.insert(name.to_string(), ModProfile { mods });
    if data.sections.is_empty() {
        data.sections = preset
            .sections
            .iter()
            .map(|s| Section {
                name: s.name.clone(),
                before: s.before.map(|id| format!("modio:{id}")),
                collapsed: false,
            })
            .collect();
    }
    // the active (local) preset decides the order
    data.normalize_shared_list();
    data.active_profile = name.to_string();
}

/// "3 h ago" for a unix time.
pub fn ago(unix: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let s = now.saturating_sub(unix);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("mint/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

/// The server's `{"error": …}` message, or the status.
async fn error_of(res: reqwest::Response) -> String {
    #[derive(Deserialize)]
    struct ApiError {
        error: String,
    }
    let status = res.status();
    match res.json::<ApiError>().await {
        Ok(e) => format!("{status}: {}", e.error),
        Err(_) => status.to_string(),
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: String,
) -> Result<T, String> {
    let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(error_of(res).await);
    }
    res.json().await.map_err(|e| format!("{url}: {e}"))
}

/// Fetches `server`'s presets. Revisions already in `cached` aren't downloaded again (they never
/// change); presets that fail validation are left out.
pub async fn fetch(server: &str, cached: &RemotePresets) -> Result<RemotePresets, String> {
    let client = client()?;
    let list: Vec<PresetSummary> = get_json(&client, format!("{server}/api/v1/presets")).await?;
    let mut presets = Vec::new();
    for summary in list {
        let known = cached
            .for_server(server)
            .iter()
            .find(|p| p.slug == summary.slug && p.revision == summary.revision);
        let preset = match known {
            Some(p) => p.clone(),
            None => {
                let url = format!(
                    "{server}/api/v1/presets/{}/{}",
                    summary.slug, summary.revision
                );
                get_json::<Preset>(&client, url).await?
            }
        };
        let upload = PresetUpload {
            name: preset.name.clone(),
            mods: preset.mods.clone(),
            sections: preset.sections.clone(),
        };
        match upload.validate() {
            Ok(()) if preset.slug == summary.slug => presets.push(preset),
            Ok(()) => warn!("remote preset {}: slug mismatch", summary.slug),
            Err(e) => warn!("remote preset {}: {e}", summary.slug),
        }
    }
    sort(&mut presets);
    Ok(RemotePresets {
        server: server.to_string(),
        presets,
    })
}

pub async fn publish(server: &str, key: &str, upload: &PresetUpload) -> Result<Preset, String> {
    let res = client()?
        .put(format!("{server}/api/v1/presets"))
        .bearer_auth(key)
        .json(upload)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(error_of(res).await);
    }
    res.json().await.map_err(|e| e.to_string())
}

pub async fn unpublish(server: &str, key: &str, slug: &str) -> Result<(), String> {
    let res = client()?
        .delete(format!("{server}/api/v1/presets/{slug}"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(error_of(res).await);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn pm(id: u32, enabled: bool, priority: i32) -> PresetMod {
        PresetMod {
            mod_id: id,
            name_id: format!("mod-{id}"),
            name: String::new(),
            modfile_id: Some(id * 10),
            enabled,
            required: false,
            priority,
        }
    }

    fn preset(mods: Vec<PresetMod>, sections: Vec<PresetSection>) -> Preset {
        Preset {
            name: "Remote".into(),
            slug: "remote".into(),
            revision: 1,
            published: 0,
            publisher: "alex".into(),
            mods,
            sections,
        }
    }

    fn local(id: u32, enabled: bool) -> ModOrGroup {
        ModOrGroup::Individual(ModConfig {
            spec: ModSpecification::new(format!("https://mod.io/g/drg/m/mod-{id}#{id}")),
            required: false,
            enabled,
            priority: 0,
        })
    }

    fn urls(d: &ModData, profile: &str) -> Vec<String> {
        d.profiles[profile]
            .mods
            .iter()
            .map(|e| match e {
                ModOrGroup::Individual(mc) => mc.spec.url.clone(),
                ModOrGroup::Group { group_name, .. } => group_name.clone(),
            })
            .collect()
    }

    #[test]
    fn urls_round_trip() {
        let m = pm(42, true, 0);
        assert_eq!(spec(&m).url, "https://mod.io/g/drg/m/mod-42#42/420");
        assert_eq!(parse_modio(&spec(&m).url), Some(("mod-42", 42, Some(420))));
        assert_eq!(
            parse_modio("https://mod.io/g/drg/m/x#7"),
            Some(("x", 7, None))
        );
        assert_eq!(parse_modio("https://mod.io/g/drg/m/x"), None);
        assert_eq!(parse_modio("https://example.com/x#7"), None);
    }

    #[test]
    fn install_order_is_priority_then_list_and_skips_disabled() {
        let p = preset(
            vec![
                pm(1, true, 0),
                pm(2, false, 5),
                pm(3, true, 5),
                pm(4, true, 0),
            ],
            vec![],
        );
        let urls: Vec<String> = install_specs(&p).into_iter().map(|s| s.url).collect();
        assert_eq!(
            urls,
            [
                "https://mod.io/g/drg/m/mod-3#3/30",
                "https://mod.io/g/drg/m/mod-1#1/10",
                "https://mod.io/g/drg/m/mod-4#4/40"
            ]
        );
    }

    #[test]
    fn copying_merges_into_the_shared_list() {
        let mut d = ModData {
            active_profile: "mine".into(),
            profiles: BTreeMap::from([(
                "mine".to_string(),
                ModProfile {
                    mods: vec![local(2, true), local(1, true)],
                },
            )]),
            groups: BTreeMap::new(),
            sections: Vec::new(),
        };
        // remote: 1, 3 (new, after 1), 2 disabled; sections above 3 and 2
        let p = preset(
            vec![pm(1, true, 0), pm(3, true, 0), pm(2, false, 0)],
            vec![
                PresetSection {
                    name: "A".into(),
                    before: Some(3),
                },
                PresetSection {
                    name: "B".into(),
                    before: Some(2),
                },
            ],
        );
        copy_to_profile(&mut d, &p, "copy");
        assert_eq!(d.active_profile, "copy");
        // local order wins; 3 lands after 1, with the remote's pinned version
        assert_eq!(
            urls(&d, "copy"),
            [
                "https://mod.io/g/drg/m/mod-2#2",
                "https://mod.io/g/drg/m/mod-1#1",
                "https://mod.io/g/drg/m/mod-3#3/30"
            ]
        );
        assert_eq!(urls(&d, "mine"), urls(&d, "copy"));
        let enabled = |profile: &str| -> Vec<bool> {
            d.profiles[profile]
                .mods
                .iter()
                .map(|e| matches!(e, ModOrGroup::Individual(mc) if mc.enabled))
                .collect()
        };
        assert_eq!(enabled("copy"), [false, true, true]);
        assert_eq!(enabled("mine"), [true, true, false]);
        assert_eq!(d.sections.len(), 2);
    }
}
