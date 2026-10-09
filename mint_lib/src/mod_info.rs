use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Tags from mod.io.
#[derive(Debug, Clone)]
pub struct ModioTags {
    pub qol: bool,
    pub gameplay: bool,
    pub audio: bool,
    pub visual: bool,
    pub framework: bool,
    pub versions: BTreeSet<String>,
    pub required_status: RequiredStatus,
    pub approval_status: ApprovalStatus,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequiredStatus {
    RequiredByAll,
    Optional,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ApprovalStatus {
    Verified,
    Approved,
    Sandbox,
}

/// Whether a mod can be resolved by clients or not
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum ResolvableStatus {
    Unresolvable(String),
    Resolvable,
}

/// Returned from ModStore
#[derive(Debug, Clone)]
pub struct ModInfo {
    pub provider: &'static str,
    pub name: String,
    pub spec: ModSpecification,          // unpinned version
    pub versions: Vec<ModSpecification>, // pinned versions TODO make this a different type
    pub resolution: ModResolution,
    pub suggested_require: bool,
    pub suggested_dependencies: Vec<ModSpecification>, // ModResponse
    pub modio_tags: Option<ModioTags>,                 // only available for mods from mod.io
    pub modio_id: Option<u32>,                         // only available for mods from mod.io
}

/// Returned from ModProvider
#[derive(Debug, Clone)]
pub enum ModResponse {
    Redirect(ModSpecification),
    Resolve(ModInfo),
}

/// Points to a mod, optionally a specific version
#[derive(
    Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ModSpecification {
    pub url: String,
}

impl ModSpecification {
    pub fn new(url: String) -> Self {
        Self { url }
    }
    pub fn satisfies_dependency(&self, other: &ModSpecification) -> bool {
        // TODO this hack works surprisingly well but is still a complete hack and should be replaced
        self.url.starts_with(&other.url) || other.url.starts_with(&self.url)
    }
}

/// Points to a specific version of a specific mod
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ModResolution {
    pub url: ModIdentifier,
    pub status: ResolvableStatus,
}

impl ModResolution {
    pub fn resolvable(url: ModIdentifier) -> Self {
        Self {
            url,
            status: ResolvableStatus::Resolvable,
        }
    }
    pub fn unresolvable(url: ModIdentifier, name: String) -> Self {
        Self {
            url,
            status: ResolvableStatus::Unresolvable(name),
        }
    }
    /// Used to get the URL if resolvable or just return the mod name if not
    pub fn get_resolvable_url_or_name(&self) -> &str {
        match &self.status {
            ResolvableStatus::Resolvable => &self.url.0,
            ResolvableStatus::Unresolvable(name) => name,
        }
    }
}

/// Mod identifier used for tracking gameplay affecting status.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModIdentifier(pub String);

impl ModIdentifier {
    pub fn new(s: String) -> Self {
        Self(s)
    }
}
impl From<String> for ModIdentifier {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}
impl From<&str> for ModIdentifier {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

/// Stripped down mod info stored in the mod pak to be used in game
#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub version: String,
    pub mods: Vec<MetaMod>,
    pub config: MetaConfig,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct MetaConfig {
    /// Zlib-compress files written to mods_P.pak. Off by default: the game decompresses
    /// into memory on load anyway, so compression only trades integration time for disk.
    pub compress_pak: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct MetaMod {
    pub name: String,
    pub version: String,
    pub url: String,
    pub author: String,
    pub approval: ApprovalStatus,
    pub required: bool,
}
/// Budget for [`Meta::to_server_list_string`]. The string ends up JSON-wrapped in a Steam lobby
/// value, which is capped at `k_cubChatMetadataMax` = 8192 bytes; past that `SetLobbyData` fails
/// and hosting/invites break. The slack covers the JSON wrapper and escaping.
const SERVER_LIST_MAX_BYTES: usize = 7 * 1024;

impl Meta {
    /// `mint;<version>;<code>;<name>;...`, Sandbox mods first. Truncated to
    /// [`SERVER_LIST_MAX_BYTES`], with a trailing `V;(+N more)` entry if anything was dropped.
    pub fn to_server_list_string(&self) -> String {
        use itertools::Itertools;

        let mut out = format!("mint;{}", self.version);
        let mods = self
            .mods
            .iter()
            .sorted_by_key(|m| (std::cmp::Reverse(m.approval), &m.name))
            .collect::<Vec<_>>();

        for (i, m) in mods.iter().enumerate() {
            let code = match m.approval {
                ApprovalStatus::Verified => 'V',
                ApprovalStatus::Approved => 'A',
                ApprovalStatus::Sandbox => 'S',
            };
            let entry = format!(";{code};{}", m.name.replace(';', ""));
            let remaining = mods.len() - i;
            // reserve room for the marker unless this is the last entry
            let reserve = if remaining > 1 {
                format!(";V;(+{remaining} more)").len()
            } else {
                0
            };
            if out.len() + entry.len() + reserve > SERVER_LIST_MAX_BYTES {
                out.push_str(&format!(";V;(+{remaining} more)"));
                break;
            }
            out.push_str(&entry);
        }
        out
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn meta(n: usize, approval: ApprovalStatus) -> Meta {
        Meta {
            version: "0.0.0".into(),
            mods: (0..n)
                .map(|i| MetaMod {
                    name: format!("Some Reasonably Long Mod Name {i:04}"),
                    version: String::new(),
                    url: String::new(),
                    author: String::new(),
                    approval,
                    required: false,
                })
                .collect(),
            config: MetaConfig {
                compress_pak: false,
            },
        }
    }

    #[test]
    fn server_list_small_is_untruncated() {
        let s = meta(2, ApprovalStatus::Sandbox).to_server_list_string();
        assert_eq!(
            s,
            "mint;0.0.0;S;Some Reasonably Long Mod Name 0000;S;Some Reasonably Long Mod Name 0001"
        );
    }

    #[test]
    fn server_list_large_is_truncated_with_marker() {
        let m = meta(1000, ApprovalStatus::Approved);
        let s = m.to_server_list_string();
        assert!(s.len() <= SERVER_LIST_MAX_BYTES, "{}", s.len());
        let kept = s.matches(";A;").count();
        assert!(s.ends_with(&format!(";V;(+{} more)", 1000 - kept)), "{s}");
    }

    #[test]
    fn server_list_sandbox_kept_first() {
        let mut m = meta(1000, ApprovalStatus::Verified);
        m.mods.push(MetaMod {
            approval: ApprovalStatus::Sandbox,
            ..meta(1, ApprovalStatus::Sandbox).mods.remove(0)
        });
        assert!(m.to_server_list_string().starts_with("mint;0.0.0;S;"));
    }
}
