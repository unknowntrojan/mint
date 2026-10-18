//! One shared mod list across all profiles.
//!
//! Every profile lists the same mods in the same order; a profile only differs in which mods are
//! enabled (and required). The save format still stores a full list per profile, so this module
//! keeps them in lockstep: changes are applied at the same position in every profile, and
//! [`ModData::normalize_shared_list`] reconciles profiles that disagree (e.g. files written by older
//! versions).

use std::collections::{BTreeMap, HashMap};

use super::{ModConfig, ModData_v0_1_0 as ModData, ModOrGroup, ModProfile_v0_1_0 as ModProfile};
use crate::providers::ModSpecification;

/// Identifies an entry across profiles. mod.io mods are identified by mod ID (falling back to the
/// slug), so the same mod pinned to different versions is still one entry.
fn entry_key(entry: &ModOrGroup) -> String {
    match entry {
        ModOrGroup::Group { group_name, .. } => format!("group:{group_name}"),
        ModOrGroup::Individual(mc) => {
            let url = mc.spec.url.as_str();
            match url.split_once("mod.io/g/drg/m/") {
                Some((_, rest)) => {
                    let (slug, fragment) = rest.split_once('#').unwrap_or((rest, ""));
                    match fragment.split('/').next().filter(|id| !id.is_empty()) {
                        Some(id) => format!("modio:{id}"),
                        None => format!("modio:{}", slug.trim_end_matches('/')),
                    }
                }
                None => url.to_string(),
            }
        }
    }
}

/// The same entry, switched off: how a mod shows up in profiles it wasn't enabled in.
fn disabled(entry: &ModOrGroup) -> ModOrGroup {
    match entry {
        ModOrGroup::Group { group_name, .. } => ModOrGroup::Group {
            group_name: group_name.clone(),
            enabled: false,
        },
        ModOrGroup::Individual(mc) => ModOrGroup::Individual(ModConfig {
            enabled: false,
            required: false,
            ..mc.clone()
        }),
    }
}

/// Moves the item at `from` so it lands before the item originally at `to` (egui_dnd semantics).
fn shift<T>(from: usize, to: usize, items: &mut [T]) {
    if from < to {
        if let Some(slice) = items.get_mut(from..to) {
            slice.rotate_left(1.min(slice.len()));
        }
    } else if let Some(slice) = items.get_mut(to..=from) {
        slice.rotate_right(1.min(slice.len()));
    }
}

/// A mod removed with [`ModData::remove_mod`], with what each profile had for it.
#[derive(Debug, Clone)]
pub struct DeletedMod {
    index: usize,
    entries: BTreeMap<String, ModOrGroup>,
}

impl DeletedMod {
    /// The deleted mod (`None` for a mod group).
    pub fn spec(&self) -> Option<&ModSpecification> {
        match self.entries.values().next()? {
            ModOrGroup::Individual(mc) => Some(&mc.spec),
            ModOrGroup::Group { .. } => None,
        }
    }
}

impl ModData {
    /// Makes every profile list the same entries in the same order. The active profile's order
    /// wins; entries only other profiles have are inserted after the entry they follow there.
    /// Missing entries are added disabled, duplicates are dropped, and version + load priority
    /// come from the first profile listing the mod. Returns whether anything changed.
    pub fn normalize_shared_list(&mut self) -> bool {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        if let Some(pos) = names.iter().position(|n| *n == self.active_profile) {
            let active = names.remove(pos);
            names.insert(0, active);
        }

        let mut order: Vec<String> = Vec::new();
        let mut canonical: HashMap<String, ModOrGroup> = HashMap::new();
        for name in &names {
            let mut prev: Option<String> = None;
            for entry in &self.profiles[name].mods {
                let key = entry_key(entry);
                if !canonical.contains_key(&key) {
                    let at = prev
                        .as_ref()
                        .and_then(|p| order.iter().position(|k| k == p))
                        .map_or(0, |i| i + 1);
                    order.insert(at, key.clone());
                    canonical.insert(key.clone(), entry.clone());
                }
                prev = Some(key);
            }
        }

        let mut changed = false;
        for profile in self.profiles.values_mut() {
            let mut own: HashMap<String, ModOrGroup> = HashMap::new();
            for entry in &profile.mods {
                own.entry(entry_key(entry)).or_insert_with(|| entry.clone());
            }
            let mods = order
                .iter()
                .map(|key| {
                    let shared = &canonical[key];
                    match (own.remove(key), shared) {
                        (Some(ModOrGroup::Individual(mine)), ModOrGroup::Individual(shared)) => {
                            ModOrGroup::Individual(ModConfig {
                                spec: shared.spec.clone(),
                                priority: shared.priority,
                                ..mine
                            })
                        }
                        (Some(mine), _) => mine,
                        (None, shared) => disabled(shared),
                    }
                })
                .collect::<Vec<_>>();
            if mods != profile.mods {
                profile.mods = mods;
                changed = true;
            }
        }
        changed
    }

    /// Moves an entry in every profile (egui_dnd `DragUpdate` semantics).
    pub fn move_mod(&mut self, from: usize, to: usize) {
        for profile in self.profiles.values_mut() {
            shift(from, to, &mut profile.mods);
        }
    }

    /// Adds a mod at the top of every profile: as given in the active profile, disabled in the
    /// others. If exactly this mod (same version) is already listed, it is enabled in the active
    /// profile instead.
    pub fn add_mod(&mut self, mc: ModConfig) {
        let active = self.active_profile.clone();
        if let Some(existing) = self.profiles[&active]
            .mods
            .iter()
            .position(|m| matches!(m, ModOrGroup::Individual(existing) if existing.spec == mc.spec))
        {
            if let ModOrGroup::Individual(existing) =
                &mut self.profiles.get_mut(&active).unwrap().mods[existing]
            {
                existing.enabled = true;
            }
            return;
        }
        let entry = ModOrGroup::Individual(mc);
        for (name, profile) in self.profiles.iter_mut() {
            let e = if *name == active {
                entry.clone()
            } else {
                disabled(&entry)
            };
            profile.mods.insert(0, e);
        }
    }

    /// Removes an entry from every profile. Restore it with [`ModData::restore_mod`].
    pub fn remove_mod(&mut self, index: usize) -> Option<DeletedMod> {
        let mut entries = BTreeMap::new();
        for (name, profile) in self.profiles.iter_mut() {
            if index < profile.mods.len() {
                entries.insert(name.clone(), profile.mods.remove(index));
            }
        }
        (!entries.is_empty()).then_some(DeletedMod { index, entries })
    }

    /// Puts a removed entry back where it was, with each profile's state from before. Profiles
    /// created since get it disabled; if the mod was added again in the meantime, that entry is
    /// replaced.
    pub fn restore_mod(&mut self, deleted: DeletedMod) {
        let Some(template) = deleted.entries.values().next().cloned() else {
            return;
        };
        let key = entry_key(&template);
        if let Some(index) = self
            .get_active_profile()
            .mods
            .iter()
            .position(|m| entry_key(m) == key)
        {
            self.remove_mod(index);
        }
        for (name, profile) in self.profiles.iter_mut() {
            let entry = deleted
                .entries
                .get(name)
                .cloned()
                .unwrap_or_else(|| disabled(&template));
            let at = deleted.index.min(profile.mods.len());
            profile.mods.insert(at, entry);
        }
    }

    /// Adds a profile listing every mod, all disabled, and makes it active.
    pub fn add_profile(&mut self, name: &str) {
        let mods = self
            .profiles
            .get(&self.active_profile)
            .or_else(|| self.profiles.values().next())
            .map(|p| p.mods.iter().map(disabled).collect())
            .unwrap_or_default();
        self.profiles.insert(name.to_owned(), ModProfile { mods });
        self.active_profile = name.to_string();
    }

    /// Copies version and load priority of every entry from `profile` to all other profiles.
    pub fn sync_shared_fields(&mut self, profile: &str) {
        let Some(source) = self.profiles.get(profile).map(|p| p.mods.clone()) else {
            return;
        };
        for (name, other) in self.profiles.iter_mut() {
            if name == profile {
                continue;
            }
            for (theirs, ours) in other.mods.iter_mut().zip(&source) {
                if let (ModOrGroup::Individual(theirs), ModOrGroup::Individual(ours)) =
                    (theirs, ours)
                {
                    theirs.spec = ours.spec.clone();
                    theirs.priority = ours.priority;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(url: &str, enabled: bool) -> ModOrGroup {
        ModOrGroup::Individual(ModConfig {
            spec: ModSpecification::new(url.to_string()),
            required: false,
            enabled,
            priority: 0,
        })
    }

    fn data(active: &str, profiles: &[(&str, Vec<ModOrGroup>)]) -> ModData {
        ModData {
            active_profile: active.to_string(),
            profiles: profiles
                .iter()
                .map(|(n, mods)| (n.to_string(), ModProfile { mods: mods.clone() }))
                .collect(),
            groups: BTreeMap::new(),
        }
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

    fn enabled(d: &ModData, profile: &str) -> Vec<bool> {
        d.profiles[profile]
            .mods
            .iter()
            .map(|e| match e {
                ModOrGroup::Individual(mc) => mc.enabled,
                ModOrGroup::Group { enabled, .. } => *enabled,
            })
            .collect()
    }

    fn assert_shared(d: &ModData) {
        let first = urls(d, d.profiles.keys().next().unwrap());
        for name in d.profiles.keys() {
            assert_eq!(urls(d, name), first, "profile {name} out of order");
        }
    }

    #[test]
    fn identical_profiles_are_left_alone() {
        let mut d = data(
            "a",
            &[
                ("a", vec![m("x", true), m("y", false)]),
                ("b", vec![m("x", false), m("y", true)]),
            ],
        );
        assert!(!d.normalize_shared_list());
        assert_eq!(enabled(&d, "a"), [true, false]);
        assert_eq!(enabled(&d, "b"), [false, true]);
    }

    #[test]
    fn disagreeing_profiles_are_merged() {
        // active order first; "q" only exists in b, after "y" -> lands right after "y"
        let mut d = data(
            "a",
            &[
                ("a", vec![m("x", true), m("y", true), m("z", true)]),
                ("b", vec![m("y", true), m("q", true), m("x", false)]),
            ],
        );
        assert!(d.normalize_shared_list());
        assert_shared(&d);
        assert_eq!(urls(&d, "a"), ["x", "y", "q", "z"]);
        assert_eq!(enabled(&d, "a"), [true, true, false, true]);
        assert_eq!(enabled(&d, "b"), [false, true, true, false]);
    }

    #[test]
    fn same_mod_different_version_or_slug_is_one_entry() {
        let mut d = data(
            "a",
            &[
                ("a", vec![m("https://mod.io/g/drg/m/new-slug#42", true)]),
                (
                    "b",
                    vec![
                        m("https://mod.io/g/drg/m/old-slug#42/999", false),
                        m("https://mod.io/g/drg/m/new-slug#42", true),
                    ],
                ),
            ],
        );
        d.normalize_shared_list();
        assert_shared(&d);
        assert_eq!(urls(&d, "b"), ["https://mod.io/g/drg/m/new-slug#42"]);
        // first entry of the mod in b decides b's state
        assert_eq!(enabled(&d, "b"), [false]);
    }

    #[test]
    fn add_move_remove_restore_keep_profiles_in_lockstep() {
        let mut d = data(
            "a",
            &[
                ("a", vec![m("x", true), m("y", false)]),
                ("b", vec![m("x", false), m("y", true)]),
            ],
        );
        d.add_mod(ModConfig {
            spec: ModSpecification::new("new".into()),
            required: false,
            enabled: true,
            priority: 0,
        });
        assert_shared(&d);
        assert_eq!(urls(&d, "a"), ["new", "x", "y"]);
        assert_eq!(enabled(&d, "a"), [true, true, false]);
        assert_eq!(enabled(&d, "b"), [false, false, true]);

        d.move_mod(0, 3); // to the bottom
        assert_shared(&d);
        assert_eq!(urls(&d, "b"), ["x", "y", "new"]);

        let deleted = d.remove_mod(1).unwrap();
        assert_shared(&d);
        assert_eq!(urls(&d, "a"), ["x", "new"]);

        d.add_profile("c");
        assert_eq!(urls(&d, "c"), ["x", "new"]);
        assert_eq!(enabled(&d, "c"), [false, false]);

        d.restore_mod(deleted);
        assert_shared(&d);
        assert_eq!(urls(&d, "a"), ["x", "y", "new"]);
        assert_eq!(enabled(&d, "a"), [true, false, true]);
        assert_eq!(enabled(&d, "b"), [false, true, false]);
        assert_eq!(enabled(&d, "c"), [false, false, false]);
    }

    #[test]
    fn re_adding_an_existing_mod_enables_it_instead_of_duplicating() {
        let mut d = data(
            "a",
            &[("a", vec![m("x", false)]), ("b", vec![m("x", true)])],
        );
        d.add_mod(ModConfig {
            spec: ModSpecification::new("x".into()),
            required: false,
            enabled: true,
            priority: 0,
        });
        assert_eq!(urls(&d, "a"), ["x"]);
        assert_eq!(enabled(&d, "a"), [true]);
        assert_eq!(enabled(&d, "b"), [true]);
    }

    #[test]
    fn restore_after_re_adding_replaces_the_new_entry() {
        let mut d = data("a", &[("a", vec![m("x", false), m("y", true)])]);
        let deleted = d.remove_mod(1).unwrap();
        d.add_mod(ModConfig {
            spec: ModSpecification::new("y".into()),
            required: false,
            enabled: false,
            priority: 0,
        });
        d.restore_mod(deleted);
        assert_eq!(urls(&d, "a"), ["x", "y"]);
        assert_eq!(enabled(&d, "a"), [false, true]);
    }

    #[test]
    fn version_and_priority_are_shared() {
        let mut d = data(
            "a",
            &[("a", vec![m("x", true)]), ("b", vec![m("x", false)])],
        );
        if let ModOrGroup::Individual(mc) = &mut d.profiles.get_mut("a").unwrap().mods[0] {
            mc.spec = ModSpecification::new("x#v2".into());
            mc.priority = 5;
        }
        d.sync_shared_fields("a");
        let ModOrGroup::Individual(b) = &d.profiles["b"].mods[0] else {
            unreachable!()
        };
        assert_eq!(
            (b.spec.url.as_str(), b.priority, b.enabled),
            ("x#v2", 5, false)
        );
    }
}
