//! Mod presets published to a mint server ("remote presets").
//!
//! A preset is an ordered list of mod.io mods, each pinned to a file version, with the enabled
//! and required flags and load priority the publisher had, plus the publisher's section headers.
//! The server stores every published revision; clients show the latest one.

use serde::{Deserialize, Serialize};

pub mod release;

pub const MAX_NAME_LEN: usize = 64;
pub const MAX_MODS: usize = 2000;
pub const MAX_SECTIONS: usize = 200;
const MAX_NAME_ID_LEN: usize = 128;
const MAX_MOD_NAME_LEN: usize = 200;
const MAX_PRIORITY: i32 = 999;

/// One mod in a preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetMod {
    /// mod.io mod ID.
    pub mod_id: u32,
    /// mod.io URL slug, so clients can build a readable mod URL.
    pub name_id: String,
    /// Display name, for clients that haven't fetched the mod yet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// mod.io file ID the preset is pinned to; `None` means latest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modfile_id: Option<u32>,
    pub enabled: bool,
    #[serde(default)]
    pub required: bool,
    /// Load priority: on asset conflicts the higher one wins.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: i32,
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

/// A header in the mod list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetSection {
    pub name: String,
    /// `mod_id` of the first mod below the header; `None` puts it at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u32>,
}

/// What a publisher uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetUpload {
    pub name: String,
    pub mods: Vec<PresetMod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<PresetSection>,
}

/// A published revision of a preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub slug: String,
    /// Starts at 1, increases with every publish.
    pub revision: u32,
    /// Unix time (seconds) of the publish.
    pub published: u64,
    /// Name of the publish key that published this revision.
    pub publisher: String,
    pub mods: Vec<PresetMod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<PresetSection>,
}

/// Entry in the list of presets a server offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetSummary {
    pub name: String,
    pub slug: String,
    pub revision: u32,
    pub published: u64,
    pub mod_count: usize,
}

impl From<&Preset> for PresetSummary {
    fn from(p: &Preset) -> Self {
        Self {
            name: p.name.clone(),
            slug: p.slug.clone(),
            revision: p.revision,
            published: p.published,
            mod_count: p.mods.len(),
        }
    }
}

/// URL-safe identifier for a preset name: lowercase ASCII letters and digits, everything else
/// collapsed to single dashes. "Weekend Pack!!" -> "weekend-pack". Empty if the name has no
/// letters or digits.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

impl PresetUpload {
    /// Checks everything a server must enforce before storing an upload.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
            return Err(format!("name must be 1-{MAX_NAME_LEN} characters"));
        }
        if name.chars().any(char::is_control) {
            return Err("name contains control characters".into());
        }
        if slug(name).is_empty() {
            return Err("name needs at least one letter or digit".into());
        }
        if self.mods.is_empty() || self.mods.len() > MAX_MODS {
            return Err(format!("a preset needs 1-{MAX_MODS} mods"));
        }
        let mut seen = std::collections::HashSet::new();
        for m in &self.mods {
            if m.mod_id == 0 || m.modfile_id == Some(0) {
                return Err(format!("invalid ID in mod {}", m.mod_id));
            }
            if !seen.insert(m.mod_id) {
                return Err(format!("mod {} is listed twice", m.mod_id));
            }
            let id_ok = !m.name_id.is_empty()
                && m.name_id.len() <= MAX_NAME_ID_LEN
                && m.name_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !id_ok {
                return Err(format!("invalid name_id for mod {}", m.mod_id));
            }
            if m.name.chars().count() > MAX_MOD_NAME_LEN || m.name.chars().any(char::is_control) {
                return Err(format!("invalid name for mod {}", m.mod_id));
            }
            if !(-MAX_PRIORITY..=MAX_PRIORITY).contains(&m.priority) {
                return Err(format!("priority of mod {} is out of range", m.mod_id));
            }
        }
        if self.sections.len() > MAX_SECTIONS {
            return Err(format!("at most {MAX_SECTIONS} sections"));
        }
        for section in &self.sections {
            let name = section.name.trim();
            if name.is_empty()
                || name.chars().count() > MAX_NAME_LEN
                || name.chars().any(char::is_control)
            {
                return Err(format!("section names must be 1-{MAX_NAME_LEN} characters"));
            }
            if section.before.is_some_and(|id| !seen.contains(&id)) {
                return Err(format!("section {name:?} is above a mod that isn't listed"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: u32) -> PresetMod {
        PresetMod {
            mod_id: id,
            name_id: format!("mod-{id}"),
            name: format!("Mod {id}"),
            modfile_id: Some(id * 10),
            enabled: true,
            required: false,
            priority: 0,
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Weekend Pack!!"), "weekend-pack");
        assert_eq!(slug("  QoL only "), "qol-only");
        assert_eq!(
            slug("Alex (with Funny/Meme Mods)"),
            "alex-with-funny-meme-mods"
        );
        assert_eq!(slug("ÄÖÜ"), "");
    }

    #[test]
    fn validation() {
        let ok = PresetUpload {
            name: "Weekend Pack".into(),
            mods: vec![m(1), m(2)],
            sections: vec![PresetSection {
                name: "QoL".into(),
                before: Some(2),
            }],
        };
        assert_eq!(ok.validate(), Ok(()));

        let bad = |f: &dyn Fn(&mut PresetUpload)| {
            let mut u = ok.clone();
            f(&mut u);
            u.validate().unwrap_err()
        };
        bad(&|u| u.name = "".into());
        bad(&|u| u.name = "!!!".into());
        bad(&|u| u.name = "x".repeat(MAX_NAME_LEN + 1));
        bad(&|u| u.name = "a\nb".into());
        bad(&|u| u.mods.clear());
        bad(&|u| u.mods.push(m(1)));
        bad(&|u| u.mods[0].mod_id = 0);
        bad(&|u| u.mods[0].modfile_id = Some(0));
        bad(&|u| u.mods[0].name_id = "../etc".into());
        bad(&|u| u.mods[0].name = "a\u{1b}[31m".into());
        bad(&|u| u.mods[0].priority = 1000);
        bad(&|u| u.sections[0].name = " ".into());
        bad(&|u| u.sections[0].before = Some(3));
    }

    #[test]
    fn latest_version_is_omitted_in_json() {
        let mut x = m(1);
        x.modfile_id = None;
        x.name.clear();
        let json = serde_json::to_string(&x).unwrap();
        assert!(
            !json.contains("modfile_id")
                && !json.contains("priority")
                && !json.contains("\"name\""),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<PresetMod>(&json).unwrap(), x);
    }
}
