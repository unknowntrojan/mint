pub mod config;
pub mod mod_list;

use std::{
    collections::{BTreeMap, HashMap},
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
};

use fs_err as fs;
use serde::{Deserialize, Serialize};
use snafu::prelude::*;

use self::config::ConfigWrapper;
use crate::{
    Dirs,
    gui::GuiTheme,
    providers::{ModSpecification, ModStore},
};
use crate::{gui::SortBy, providers::ProviderError};
use mint_lib::{DRGInstallation, mod_info::MetaConfig};

/// Mod configuration, holds ModSpecification as well as other metadata
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModConfig {
    pub spec: ModSpecification,
    pub required: bool,

    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: i32,
}

fn default_true() -> bool {
    true
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

#[obake::versioned]
#[obake(version("0.0.0"))]
#[obake(version("0.1.0"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModProfile {
    #[obake(cfg("0.0.0"))]
    pub mods: Vec<ModConfig>,

    #[obake(cfg("0.1.0"))]
    #[serde(deserialize_with = "without_groups")]
    pub mods: Vec<ModConfig>,
}

/// Version 0.1.0 profiles could also list mod groups, which mint never got a UI for. Those
/// entries are dropped.
fn without_groups<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ModConfig>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Mod(ModConfig),
        Group {
            #[allow(dead_code)]
            group_name: String,
        },
    }
    let entries = Vec::<Entry>::deserialize(d)?;
    let dropped = entries
        .iter()
        .filter(|e| matches!(e, Entry::Group { .. }))
        .count();
    if dropped > 0 {
        tracing::warn!("dropped {dropped} mod group entries from a profile");
    }
    Ok(entries
        .into_iter()
        .filter_map(|e| match e {
            Entry::Mod(mc) => Some(mc),
            Entry::Group { .. } => None,
        })
        .collect())
}

impl From<ModProfile!["0.0.0"]> for ModProfile!["0.1.0"] {
    fn from(_legacy: ModProfile!["0.0.0"]) -> Self {
        // The migration requires `ModData` to handle instead.
        unimplemented!("migration requires handling from `ModData`")
    }
}

#[obake::versioned]
#[obake(version("0.0.0"))]
#[obake(version("0.1.0"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModData {
    pub active_profile: String,
    #[obake(cfg("0.0.0"))]
    pub profiles: BTreeMap<String, ModProfile!["0.0.0"]>,
    #[obake(cfg("0.1.0"))]
    pub profiles: BTreeMap<String, ModProfile!["0.1.0"]>,
    /// Unused (mod groups were never finished). Older mint builds refuse files without it, so
    /// whatever is there is kept.
    #[obake(cfg("0.1.0"))]
    #[serde(default, rename = "groups")]
    pub legacy_groups: BTreeMap<String, serde_json::Value>,
    /// Section headers in the mod list, top to bottom; the same in every profile.
    #[obake(cfg("0.1.0"))]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
}

/// A named header in the mod list. It sits directly above the entry `before` names (see
/// `mod_list::entry_key`), or at the end of the list if that is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
}

impl ModData!["0.1.0"] {
    pub fn for_each_mod<F: FnMut(&ModConfig)>(&self, profile: &str, f: F) {
        self.profiles[profile].mods.iter().for_each(f)
    }

    pub fn for_each_enabled_mod<F: FnMut(&ModConfig)>(&self, profile: &str, f: F) {
        self.profiles[profile]
            .mods
            .iter()
            .filter(|mc| mc.enabled)
            .for_each(f)
    }

    pub fn any_mod<F: FnMut(&ModConfig) -> bool>(&self, profile: &str, f: F) -> bool {
        self.profiles[profile].mods.iter().any(f)
    }

    pub fn any_mod_mut<F: FnMut(&mut ModConfig) -> bool>(&mut self, profile: &str, f: F) -> bool {
        self.profiles
            .get_mut(profile)
            .unwrap()
            .mods
            .iter_mut()
            .any(f)
    }
}

impl Default for ModData!["0.1.0"] {
    fn default() -> Self {
        Self {
            active_profile: "default".to_string(),
            profiles: [("default".to_string(), Default::default())]
                .into_iter()
                .collect(),
            legacy_groups: BTreeMap::new(),
            sections: Vec::new(),
        }
    }
}

impl From<ModData!["0.0.0"]> for ModData!["0.1.0"] {
    fn from(legacy: ModData!["0.0.0"]) -> Self {
        let mut new_profiles = Vec::new();
        for (name, profile) in legacy.profiles {
            let new_profile = ModProfile_v0_1_0 { mods: profile.mods };
            new_profiles.push((name, new_profile));
        }

        Self {
            active_profile: legacy.active_profile,
            profiles: new_profiles.into_iter().collect(),
            legacy_groups: BTreeMap::new(),
            sections: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "version")]
pub enum VersionAnnotatedModData {
    #[serde(rename = "0.0.0")]
    V0_0_0(ModData!["0.0.0"]),
    #[serde(rename = "0.1.0")]
    V0_1_0(ModData!["0.1.0"]),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MaybeVersionedModData {
    Versioned(VersionAnnotatedModData),
    Legacy(ModData!["0.0.0"]),
}

impl Default for ModData!["0.0.0"] {
    fn default() -> Self {
        Self {
            active_profile: "default".to_string(),
            profiles: [("default".to_string(), Default::default())]
                .into_iter()
                .collect(),
        }
    }
}

impl Default for MaybeVersionedModData {
    fn default() -> Self {
        MaybeVersionedModData::Versioned(Default::default())
    }
}

impl Default for VersionAnnotatedModData {
    fn default() -> Self {
        VersionAnnotatedModData::V0_1_0(Default::default())
    }
}

impl Deref for VersionAnnotatedModData {
    type Target = ModData!["0.1.0"];

    fn deref(&self) -> &Self::Target {
        match self {
            VersionAnnotatedModData::V0_0_0(_) => unreachable!(),
            VersionAnnotatedModData::V0_1_0(md) => md,
        }
    }
}

impl DerefMut for VersionAnnotatedModData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            VersionAnnotatedModData::V0_0_0(_) => unreachable!(),
            VersionAnnotatedModData::V0_1_0(md) => md,
        }
    }
}

impl ModData!["0.1.0"] {
    pub fn get_active_profile(&self) -> &ModProfile!["0.1.0"] {
        &self.profiles[&self.active_profile]
    }

    pub fn get_active_profile_mut(&mut self) -> &mut ModProfile!["0.1.0"] {
        self.profiles.get_mut(&self.active_profile).unwrap()
    }

    pub fn remove_active_profile(&mut self) {
        self.profiles.remove(&self.active_profile);
        self.active_profile = self.profiles.keys().next().unwrap().to_string();
    }
}

#[obake::versioned]
#[obake(version("0.0.0"))]
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub provider_parameters: HashMap<String, HashMap<String, String>>,
    pub drg_pak_path: Option<PathBuf>,
    pub gui_theme: Option<GuiTheme>,
    pub sorting_config: Option<SortingConfig>,
    #[serde(default)]
    pub compress_pak: bool,
    /// Overrides [`DEFAULT_PRESET_SERVER`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_server: Option<String>,
    /// Lets this install publish presets. friendpack leaves it out of the mods pack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_key: Option<String>,
    /// Slug of the selected remote preset; `None` when a local preset is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_preset: Option<String>,
}

/// Where `[Remote]` presets come from unless the config says otherwise.
pub const DEFAULT_PRESET_SERVER: &str = "https://mint.kohlroulade.lgbt";

impl Config!["0.0.0"] {
    pub fn preset_server(&self) -> &str {
        self.preset_server
            .as_deref()
            .unwrap_or(DEFAULT_PRESET_SERVER)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SortingConfig {
    pub sort_category: SortBy,
    pub is_ascending: bool,
}

impl Default for SortingConfig {
    fn default() -> Self {
        Self {
            sort_category: SortBy::Enabled,
            is_ascending: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "version")]
pub enum VersionAnnotatedConfig {
    #[serde(rename = "0.0.0")]
    V0_0_0(Config!["0.0.0"]),
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MaybeVersionedConfig {
    Versioned(VersionAnnotatedConfig),
    Legacy(Config!["0.0.0"]),
}

impl Default for MaybeVersionedConfig {
    fn default() -> Self {
        MaybeVersionedConfig::Versioned(Default::default())
    }
}

impl Default for VersionAnnotatedConfig {
    fn default() -> Self {
        VersionAnnotatedConfig::V0_0_0(Default::default())
    }
}

impl Deref for VersionAnnotatedConfig {
    type Target = Config!["0.0.0"];

    fn deref(&self) -> &Self::Target {
        match self {
            VersionAnnotatedConfig::V0_0_0(cfg) => cfg,
            VersionAnnotatedConfig::Unsupported => unreachable!(),
        }
    }
}

impl DerefMut for VersionAnnotatedConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            VersionAnnotatedConfig::V0_0_0(cfg) => cfg,
            VersionAnnotatedConfig::Unsupported => unreachable!(),
        }
    }
}

impl Default for Config!["0.0.0"] {
    fn default() -> Self {
        Self {
            provider_parameters: Default::default(),
            drg_pak_path: DRGInstallation::find()
                .as_ref()
                .map(DRGInstallation::main_pak),
            gui_theme: None,
            sorting_config: None,
            compress_pak: false,
            preset_server: None,
            publish_key: None,
            remote_preset: None,
        }
    }
}

impl From<&VersionAnnotatedConfig> for MetaConfig {
    fn from(value: &VersionAnnotatedConfig) -> Self {
        MetaConfig {
            compress_pak: value.compress_pak,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum StateError {
    #[snafu(display("failed to deserialize user config"))]
    CfgDeserializationFailed { source: serde_json::Error },
    #[snafu(display("unsupported config version"))]
    UnsupportedCfgVersion,
    #[snafu(display("failed to read config.json"))]
    CfgReadFailed { source: std::io::Error },
    #[snafu(display("failed to save config"))]
    CfgSaveFailed { source: std::io::Error },
    #[snafu(display("failed to serialize user config"))]
    CfgSerializationFailed { source: serde_json::Error },
    #[snafu(transparent)]
    IoError { source: std::io::Error },
    #[snafu(transparent)]
    PersistError { source: tempfile::PersistError },
    #[snafu(transparent)]
    ProviderError { source: ProviderError },
    #[snafu(display("failed to deserialize mod data"))]
    ModDataDeserializationFailed { source: serde_json::Error },
    #[snafu(display("failed to deserialize legacy profiles"))]
    LegacyProfilesDeserializationFailed { source: serde_json::Error },
}

pub struct State {
    pub dirs: Dirs,
    pub config: ConfigWrapper<VersionAnnotatedConfig>,
    pub mod_data: ConfigWrapper<VersionAnnotatedModData>,
    pub store: Arc<ModStore>,
}

impl State {
    pub fn init(dirs: Dirs) -> Result<Self, StateError> {
        let config_path = dirs.config_dir.join("config.json");

        let config = read_config_or_default(&config_path)?;
        let config = ConfigWrapper::<VersionAnnotatedConfig>::new(&config_path, config);
        config.save().unwrap();

        let legacy_mod_profiles_path = dirs.config_dir.join("profiles.json");
        let mod_data_path = dirs.config_dir.join("mod_data.json");
        let mut mod_data = read_mod_data_or_default(&mod_data_path, legacy_mod_profiles_path)?;
        mod_data.normalize_shared_list();
        let mod_data = ConfigWrapper::<VersionAnnotatedModData>::new(mod_data_path, mod_data);
        mod_data.save().unwrap();

        let store: Arc<ModStore> =
            ModStore::new(&dirs.cache_dir, &config.provider_parameters)?.into();
        store.set_mirror(Some(config.preset_server()));

        Ok(Self {
            dirs,
            config,
            mod_data,
            store,
        })
    }
}

fn read_config_or_default(config_path: &PathBuf) -> Result<VersionAnnotatedConfig, StateError> {
    Ok(match fs::read(config_path) {
        Ok(buf) => {
            let config = serde_json::from_slice::<MaybeVersionedConfig>(&buf)
                .context(CfgDeserializationFailedSnafu)?;
            match config {
                MaybeVersionedConfig::Versioned(v) => match v {
                    VersionAnnotatedConfig::V0_0_0(v) => VersionAnnotatedConfig::V0_0_0(v),
                    VersionAnnotatedConfig::Unsupported => UnsupportedCfgVersionSnafu.fail()?,
                },
                MaybeVersionedConfig::Legacy(legacy) => {
                    VersionAnnotatedConfig::V0_0_0(Config_v0_0_0 {
                        provider_parameters: legacy.provider_parameters,
                        drg_pak_path: legacy.drg_pak_path,
                        ..Default::default()
                    })
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => VersionAnnotatedConfig::default(),
        Err(e) => Err(e)?,
    })
}

fn read_mod_data_or_default(
    mod_data_path: &PathBuf,
    legacy_mod_profiles_path: PathBuf,
) -> Result<VersionAnnotatedModData, StateError> {
    let mod_data = match fs::read(mod_data_path) {
        Ok(buf) => serde_json::from_slice::<MaybeVersionedModData>(&buf)
            .context(ModDataDeserializationFailedSnafu)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match fs::read(&legacy_mod_profiles_path) {
                Ok(buf) => {
                    let mod_data = serde_json::from_slice::<MaybeVersionedModData>(&buf)
                        .context(LegacyProfilesDeserializationFailedSnafu)?;
                    fs::remove_file(&legacy_mod_profiles_path)?;
                    mod_data
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    MaybeVersionedModData::default()
                }
                Err(e) => Err(e)?,
            }
        }
        Err(e) => Err(e)?,
    };

    let mod_data = match mod_data {
        MaybeVersionedModData::Legacy(legacy) => VersionAnnotatedModData::V0_1_0(legacy.into()),
        MaybeVersionedModData::Versioned(v) => match v {
            VersionAnnotatedModData::V0_0_0(md) => VersionAnnotatedModData::V0_1_0(md.into()),
            VersionAnnotatedModData::V0_1_0(md) => VersionAnnotatedModData::V0_1_0(md),
        },
    };

    Ok(mod_data)
}

#[cfg(test)]
mod mod_data_tests {
    use super::{ModData_v0_1_0 as ModData, VersionAnnotatedModData};

    #[test]
    fn group_entries_are_dropped_and_the_groups_key_is_kept() {
        let json = r#"{"version":"0.1.0","active_profile":"a","profiles":{"a":{"mods":[
            {"spec":{"url":"https://mod.io/g/drg/m/x#1"},"required":false,"enabled":true},
            {"group_name":"g","enabled":true}
        ]}},"groups":{"g":{"mods":[]}}}"#;
        let data: VersionAnnotatedModData = serde_json::from_str(json).unwrap();
        let VersionAnnotatedModData::V0_1_0(data) = data else {
            panic!("wrong version");
        };
        let data: ModData = data;
        assert_eq!(data.profiles["a"].mods.len(), 1);
        let out = serde_json::to_value(&data).unwrap();
        assert_eq!(out["groups"], serde_json::json!({"g": {"mods": []}}));
    }

    #[test]
    fn config_without_compress_pak_deserializes() {
        use super::{MaybeVersionedConfig, VersionAnnotatedConfig};

        let cfg: MaybeVersionedConfig = serde_json::from_str(
            r#"{"version":"0.0.0","provider_parameters":{},"drg_pak_path":null,"gui_theme":null,"sorting_config":null}"#,
        )
        .unwrap();
        let MaybeVersionedConfig::Versioned(cfg @ VersionAnnotatedConfig::V0_0_0(_)) = cfg else {
            panic!("expected versioned 0.0.0 config");
        };
        assert!(!cfg.compress_pak);
    }
}

#[cfg(test)]
mod config_tests {
    use super::{DEFAULT_PRESET_SERVER, VersionAnnotatedConfig};

    #[test]
    fn remote_fields_are_optional_and_omitted_when_unset() {
        let old = r#"{"version":"0.0.0","provider_parameters":{},"drg_pak_path":null,"gui_theme":null,"sorting_config":null}"#;
        let mut cfg: VersionAnnotatedConfig = serde_json::from_str(old).unwrap();
        assert_eq!(cfg.preset_server(), DEFAULT_PRESET_SERVER);
        assert_eq!(cfg.publish_key, None);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("publish_key") && !json.contains("preset_server"));

        cfg.preset_server = Some("http://localhost:8080".into());
        cfg.publish_key = Some("k".into());
        let back: VersionAnnotatedConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.preset_server(), "http://localhost:8080");
        assert_eq!(back.publish_key.as_deref(), Some("k"));
    }
}
