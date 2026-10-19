//! One shared mod list across all profiles.
//!
//! Every profile lists the same mods in the same order; a profile only differs in which mods are
//! enabled (and required). The save format still stores a full list per profile, so this module
//! keeps them in lockstep: changes are applied at the same position in every profile, and
//! [`ModData::normalize_shared_list`] reconciles profiles that disagree (e.g. files written by older
//! versions).
//!
//! Section headers ([`ModData::sections`]) are shared the same way. Each is anchored to the entry
//! below it, so adding mods never moves a header; operations that reorder the list go through
//! [`ModData::arrange`], which re-anchors them.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use super::{
    ModConfig, ModData_v0_1_0 as ModData, ModOrGroup, ModProfile_v0_1_0 as ModProfile, Section,
};
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
    /// sections whose header sat directly above it
    sections: Vec<String>,
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
        self.normalize_sections() || changed
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
        let list = &self.get_active_profile().mods;
        let key = list.get(index).map(entry_key);
        let below = list.get(index + 1).map(entry_key);
        let mut sections = Vec::new();
        for section in &mut self.sections {
            if key.is_some() && section.before == key {
                section.before = below.clone();
                sections.push(section.name.clone());
            }
        }

        let mut entries = BTreeMap::new();
        for (name, profile) in self.profiles.iter_mut() {
            if index < profile.mods.len() {
                entries.insert(name.clone(), profile.mods.remove(index));
            }
        }
        (!entries.is_empty()).then_some(DeletedMod {
            index,
            entries,
            sections,
        })
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

        let list = &self.get_active_profile().mods;
        let at = deleted.index.min(list.len() - 1);
        let below = list.get(at + 1).map(entry_key);
        for section in &mut self.sections {
            if deleted.sections.contains(&section.name) && section.before == below {
                section.before = Some(key.clone());
            }
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

    /// Enables or disables an entry in every profile.
    pub fn set_enabled_everywhere(&mut self, index: usize, enabled: bool) {
        for profile in self.profiles.values_mut() {
            match profile.mods.get_mut(index) {
                Some(ModOrGroup::Individual(mc)) => mc.enabled = enabled,
                Some(ModOrGroup::Group { enabled: e, .. }) => *e = enabled,
                None => {}
            }
        }
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

/// A row of the mod list as shown: a section header (index into [`ModData::sections`]) or an
/// entry (index into every profile's list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Row {
    Section(usize),
    Entry(usize),
}

/// Where a section header sits: right above the first entry after `pos` in `rows`.
fn anchor_after(rows: &[Row], pos: usize) -> usize {
    rows[pos..]
        .iter()
        .position(|r| matches!(r, Row::Section(_)))
        .map_or(rows.len(), |p| pos + p)
}

/// `rows` after dragging visible row `from` before visible row `to` (egui_dnd semantics). A row
/// carries the hidden rows below it, so a collapsed section moves with its entries.
pub fn drag_rows(rows: &[Row], hidden: &[bool], from: usize, to: usize) -> Vec<Row> {
    let mut chunks: Vec<Vec<Row>> = Vec::new();
    for (row, &hidden) in rows.iter().zip(hidden) {
        match chunks.last_mut() {
            Some(chunk) if hidden => chunk.push(*row),
            _ => chunks.push(vec![*row]),
        }
    }
    shift(from, to, &mut chunks);
    chunks.into_iter().flatten().collect()
}

impl ModData {
    /// The index of the first entry below each section header. A header whose entry is gone
    /// (e.g. removed by an older mint) moves down to the next header.
    fn section_starts(&self) -> Vec<usize> {
        let list = &self.get_active_profile().mods;
        let index: HashMap<String, usize> = list
            .iter()
            .enumerate()
            .map(|(i, e)| (entry_key(e), i))
            .collect();
        let mut starts = vec![list.len(); self.sections.len()];
        let mut next = list.len();
        for (start, section) in starts.iter_mut().zip(&self.sections).rev() {
            if let Some(i) = section.before.as_ref().and_then(|k| index.get(k)) {
                next = (*i).min(next);
            }
            *start = next;
        }
        starts
    }

    /// The entries of each section.
    pub fn section_ranges(&self) -> Vec<Range<usize>> {
        let starts = self.section_starts();
        let len = self.get_active_profile().mods.len();
        (0..starts.len())
            .map(|s| starts[s]..starts.get(s + 1).copied().unwrap_or(len))
            .collect()
    }

    /// The list with section headers in place; every entry and section appears once.
    pub fn rows(&self) -> Vec<Row> {
        let starts = self.section_starts();
        let len = self.get_active_profile().mods.len();
        let mut rows = Vec::with_capacity(len + starts.len());
        let mut s = 0;
        for i in 0..=len {
            while s < starts.len() && starts[s] == i {
                rows.push(Row::Section(s));
                s += 1;
            }
            if i < len {
                rows.push(Row::Entry(i));
            }
        }
        rows
    }

    /// Reorders entries (in every profile) and sections to match `rows`, a permutation of
    /// [`ModData::rows`].
    pub fn arrange(&mut self, rows: &[Row]) {
        let keys: Vec<String> = self
            .get_active_profile()
            .mods
            .iter()
            .map(entry_key)
            .collect();
        for profile in self.profiles.values_mut() {
            let mut old: Vec<Option<ModOrGroup>> = std::mem::take(&mut profile.mods)
                .into_iter()
                .map(Some)
                .collect();
            profile.mods = rows
                .iter()
                .filter_map(|r| match r {
                    Row::Entry(i) => old.get_mut(*i).and_then(Option::take),
                    Row::Section(_) => None,
                })
                .collect();
        }

        let mut old: Vec<Option<Section>> = std::mem::take(&mut self.sections)
            .into_iter()
            .map(Some)
            .collect();
        let mut below = None;
        for row in rows.iter().rev() {
            match *row {
                Row::Entry(i) => below = keys.get(i).cloned(),
                Row::Section(s) => {
                    if let Some(section) = old.get_mut(s).and_then(Option::take) {
                        self.sections.push(Section {
                            before: below.clone(),
                            ..section
                        });
                    }
                }
            }
        }
        self.sections.reverse();
    }

    /// Rewrites section anchors that point at missing entries. Returns whether anything changed.
    pub fn normalize_sections(&mut self) -> bool {
        let before = self.sections.clone();
        let rows = self.rows();
        self.arrange(&rows);
        self.sections != before
    }

    /// Adds a section at `row` (a position in [`ModData::rows`]) and returns its index.
    pub fn insert_section(&mut self, name: String, row: usize) -> usize {
        let mut rows = self.rows();
        let row = row.min(rows.len());
        rows.insert(row, Row::Section(self.sections.len()));
        self.sections.push(Section {
            name,
            before: None,
            collapsed: false,
        });
        self.arrange(&rows);
        rows[..row]
            .iter()
            .filter(|r| matches!(r, Row::Section(_)))
            .count()
    }

    /// Adds a section directly below section `s` and all its entries.
    pub fn insert_section_after(&mut self, name: String, s: usize) -> usize {
        let rows = self.rows();
        let header = rows.iter().position(|r| *r == Row::Section(s));
        let at = header.map_or(rows.len(), |h| anchor_after(&rows, h + 1));
        self.insert_section(name, at)
    }

    /// Adds a section directly above entry `i`.
    pub fn insert_section_above(&mut self, name: String, i: usize) -> usize {
        let at = self
            .rows()
            .iter()
            .position(|r| *r == Row::Entry(i))
            .unwrap_or(usize::MAX);
        self.insert_section(name, at)
    }

    /// Removes a section header; its entries join the section above.
    pub fn remove_section(&mut self, s: usize) {
        let rows: Vec<Row> = self
            .rows()
            .into_iter()
            .filter(|r| *r != Row::Section(s))
            .collect();
        self.arrange(&rows);
    }

    /// Swaps section `s` and its entries with the section above or below. Returns its new index.
    pub fn move_section(&mut self, s: usize, up: bool) -> usize {
        let target = if up { s.checked_sub(1) } else { Some(s + 1) };
        let Some(target) = target.filter(|t| *t < self.sections.len()) else {
            return s;
        };
        // blocks[0]: entries above the first header; blocks[k + 1]: section k
        let mut blocks: Vec<Vec<Row>> = vec![Vec::new()];
        for row in self.rows() {
            if matches!(row, Row::Section(_)) {
                blocks.push(Vec::new());
            }
            blocks.last_mut().unwrap().push(row);
        }
        blocks.swap(s + 1, target + 1);
        self.arrange(&blocks.concat());
        target
    }

    /// Moves entry `i` to the end of section `s`, or above all sections if `None`.
    pub fn move_to_section(&mut self, i: usize, s: Option<usize>) {
        let mut rows: Vec<Row> = self
            .rows()
            .into_iter()
            .filter(|r| *r != Row::Entry(i))
            .collect();
        let at = match s {
            None => anchor_after(&rows, 0),
            Some(s) => match rows.iter().position(|r| *r == Row::Section(s)) {
                Some(h) => anchor_after(&rows, h + 1),
                None => return,
            },
        };
        rows.insert(at, Row::Entry(i));
        self.arrange(&rows);
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
            sections: Vec::new(),
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

        let rows = drag_rows(&d.rows(), &[false; 3], 0, 3); // to the bottom
        d.arrange(&rows);
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
    fn set_enabled_everywhere_touches_only_that_entry() {
        let mut d = data(
            "a",
            &[
                ("a", vec![m("x", true), m("y", false)]),
                ("b", vec![m("x", false), m("y", true)]),
            ],
        );
        d.set_enabled_everywhere(0, true);
        assert_eq!(enabled(&d, "a"), [true, false]);
        assert_eq!(enabled(&d, "b"), [true, true]);
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

    fn view(d: &ModData) -> Vec<(String, Vec<String>)> {
        let all = urls(d, &d.active_profile);
        d.section_ranges()
            .into_iter()
            .zip(&d.sections)
            .map(|(r, s)| (s.name.clone(), all[r].to_vec()))
            .collect()
    }

    fn sectioned() -> ModData {
        let mut d = data(
            "a",
            &[
                ("a", vec![m("x", true), m("y", false), m("z", true)]),
                ("b", vec![m("x", false), m("y", true), m("z", false)]),
            ],
        );
        assert_eq!(d.insert_section_above("S1".into(), 0), 0);
        assert_eq!(d.insert_section_above("S2".into(), 2), 1);
        d
    }

    fn sv(v: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        v.iter()
            .map(|(n, e)| (n.to_string(), e.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn sections_survive_adding_and_dragging() {
        let mut d = sectioned();
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &["z"])]));

        // new mods land above every header
        d.add_mod(ModConfig {
            spec: ModSpecification::new("new".into()),
            required: false,
            enabled: true,
            priority: 0,
        });
        assert_eq!(urls(&d, "a"), ["new", "x", "y", "z"]);
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &["z"])]));

        // rows: new S1 x y S2 z; drag "new" between S2 and z -> first entry of S2
        let rows = d.rows();
        assert_eq!(rows[1], Row::Section(0));
        let moved = drag_rows(&rows, &[false; 6], 0, 5);
        d.arrange(&moved);
        assert_shared(&d);
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &["new", "z"])]));

        // dragging the first entry of S1 out of it re-anchors S1 to the next one
        let rows = d.rows(); // S1 x y S2 new z
        d.arrange(&drag_rows(&rows, &[false; 6], 1, 6));
        assert_eq!(view(&d), sv(&[("S1", &["y"]), ("S2", &["new", "z", "x"])]));
    }

    #[test]
    fn dragging_a_collapsed_section_carries_its_entries() {
        let mut d = sectioned(); // S1 x y S2 z
        let rows = d.rows();
        let hidden = [false, true, true, false, false];
        // visible: S1 S2 z; drop S1 (with x y) at the end
        d.arrange(&drag_rows(&rows, &hidden, 0, 3));
        assert_eq!(urls(&d, "b"), ["z", "x", "y"]);
        assert_eq!(view(&d), sv(&[("S2", &["z"]), ("S1", &["x", "y"])]));
    }

    #[test]
    fn section_operations() {
        let mut d = sectioned();
        assert_eq!(d.move_section(0, false), 1);
        assert_eq!(urls(&d, "a"), ["z", "x", "y"]);
        assert_eq!(view(&d), sv(&[("S2", &["z"]), ("S1", &["x", "y"])]));
        assert_eq!(d.move_section(1, true), 0);
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &["z"])]));
        assert_eq!(d.move_section(0, true), 0);

        d.move_to_section(0, Some(1));
        assert_eq!(view(&d), sv(&[("S1", &["y"]), ("S2", &["z", "x"])]));
        d.move_to_section(2, None);
        assert_eq!(urls(&d, "a"), ["x", "y", "z"]);
        assert_eq!(view(&d), sv(&[("S1", &["y"]), ("S2", &["z"])]));

        // empty section below S1, then above everything
        assert_eq!(d.insert_section_after("E".into(), 0), 1);
        assert_eq!(view(&d), sv(&[("S1", &["y"]), ("E", &[]), ("S2", &["z"])]));
        assert_eq!(d.insert_section("Top".into(), 0), 0);
        assert_eq!(view(&d)[0], sv(&[("Top", &["x"])])[0]);

        d.remove_section(0);
        d.remove_section(1);
        assert_eq!(view(&d), sv(&[("S1", &["y"]), ("S2", &["z"])]));
        assert_shared(&d);
    }

    #[test]
    fn deleting_the_first_entry_of_a_section_keeps_the_header() {
        let mut d = sectioned(); // S1 x y S2 z
        let deleted = d.remove_mod(2).unwrap();
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &[])]));
        d.restore_mod(deleted);
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &["z"])]));

        let deleted = d.remove_mod(0).unwrap();
        assert_eq!(view(&d), sv(&[("S1", &["y"]), ("S2", &["z"])]));
        d.restore_mod(deleted);
        assert_eq!(view(&d), sv(&[("S1", &["x", "y"]), ("S2", &["z"])]));
    }

    #[test]
    fn missing_anchors_are_repaired() {
        let mut d = sectioned();
        d.sections[0].before = Some("gone".into());
        assert!(d.normalize_shared_list());
        assert_eq!(view(&d), sv(&[("S1", &[]), ("S2", &["z"])]));
        assert_eq!(d.sections[0].before.as_deref(), Some("z"));
        assert!(!d.normalize_shared_list());
    }
}
