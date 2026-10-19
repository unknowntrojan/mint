use remote_preset::Preset;

use super::{colors, custom_popup_below_widget, is_committed};

use crate::remote;
use crate::state::ModData_v0_1_0 as ModData;

#[derive(Debug, Clone)]
struct NamePopup {
    buffer_needs_prefill_and_focus: bool,
    buffer: String,
}

impl Default for NamePopup {
    fn default() -> Self {
        Self {
            buffer_needs_prefill_and_focus: true,
            buffer: String::new(),
        }
    }
}

pub trait NamedEntries {
    fn len(&self) -> usize;
    fn contains(&self, name: &str) -> bool;
    fn select(&mut self, name: String);
    fn selected_name(&self) -> &str;
    fn add_new(&mut self, name: &str);
    fn remove_selected(&mut self);
    fn rename_selected(&mut self, new_name: String);
    fn duplicate_selected(&mut self, new_name: String);
    /// The names to choose from; `None` draws a separator.
    fn names(&self) -> Vec<Option<String>>;
    /// Whether the selected entry can be renamed and deleted.
    fn editable(&self) -> bool {
        true
    }
    /// Suggested name for a duplicate of the selected entry.
    fn duplicate_name(&self) -> String {
        format!("{} - Copy", self.selected_name())
    }
}

/// The preset dropdown: local presets, then the read-only remote ones.
pub struct Presets<'a> {
    pub data: &'a mut ModData,
    /// (shown name, preset)
    pub remote: Vec<(String, &'a Preset)>,
    /// slug of the selected remote preset; `None` when a local one is selected
    pub selected_remote: &'a mut Option<String>,
}

impl<'a> Presets<'a> {
    pub fn remote_selected(&self) -> Option<&(String, &'a Preset)> {
        let slug = self.selected_remote.as_ref()?;
        self.remote.iter().find(|(_, p)| p.slug == *slug)
    }
}

impl NamedEntries for Presets<'_> {
    fn len(&self) -> usize {
        self.data.profiles.len()
    }
    fn contains(&self, name: &str) -> bool {
        // local names can't look like remote ones
        self.data.profiles.contains_key(name) || name.starts_with(remote::PREFIX)
    }
    fn select(&mut self, name: String) {
        match self.remote.iter().find(|(shown, _)| *shown == name) {
            Some((_, preset)) => *self.selected_remote = Some(preset.slug.clone()),
            None => {
                *self.selected_remote = None;
                self.data.active_profile = name;
            }
        }
    }
    fn selected_name(&self) -> &str {
        match self.remote_selected() {
            Some((shown, _)) => shown,
            None => &self.data.active_profile,
        }
    }
    fn add_new(&mut self, name: &str) {
        self.data.add_profile(name);
        *self.selected_remote = None;
    }
    fn remove_selected(&mut self) {
        if self.selected_remote.is_none() {
            self.data.remove_active_profile();
        }
    }
    fn rename_selected(&mut self, new_name: String) {
        if self.selected_remote.is_some() {
            return;
        }
        let tmp = self
            .data
            .profiles
            .remove(&self.data.active_profile)
            .unwrap();
        self.data.profiles.insert(new_name.clone(), tmp);
        self.data.active_profile = new_name;
    }
    fn duplicate_selected(&mut self, new_name: String) {
        if let Some(&(_, preset)) = self.remote_selected() {
            remote::copy_to_profile(self.data, preset, &new_name);
            *self.selected_remote = None;
            return;
        }
        let new = self.data.get_active_profile().clone();
        self.data.profiles.insert(new_name.clone(), new);
        self.data.active_profile = new_name;
    }
    fn names(&self) -> Vec<Option<String>> {
        let mut names: Vec<Option<String>> = self.data.profiles.keys().cloned().map(Some).collect();
        if !self.remote.is_empty() {
            names.push(None);
            names.extend(self.remote.iter().map(|(shown, _)| Some(shown.clone())));
        }
        names
    }
    fn editable(&self) -> bool {
        self.selected_remote.is_none()
    }
    fn duplicate_name(&self) -> String {
        match self.remote_selected() {
            Some((_, preset)) if !self.contains(&preset.name) => preset.name.clone(),
            Some((_, preset)) => format!("{} - Copy", preset.name),
            None => format!("{} - Copy", self.data.active_profile),
        }
    }
}

/// Render and return whether any changes were made
pub(crate) fn ui<N>(
    ui: &mut egui::Ui,
    name: &str,
    entries: &mut N,
    additional_ui: Option<impl FnOnce(&mut egui::Ui, &mut N)>,
) -> bool
where
    N: NamedEntries,
{
    let mut modified = false;
    ui.push_id(name, |ui| {
        ui.horizontal(|ui| {
            mk_add(ui, name, entries, &mut modified);
            mk_delete(ui, name, entries, &mut modified);
            mk_rename(ui, name, entries, &mut modified);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                mk_duplicate(ui, name, entries, &mut modified);

                if let Some(additional_ui) = additional_ui {
                    additional_ui(ui, entries);
                }

                ui.with_layout(ui.layout().with_main_justify(true), |ui| {
                    mk_dropdown(ui, name, entries, &mut modified);
                });
            });
        });
    });
    modified
}

fn mk_delete<N>(ui: &mut egui::Ui, name: &str, entries: &mut N, modified: &mut bool)
where
    N: NamedEntries,
{
    ui.add_enabled_ui(entries.len() > 1 && entries.editable(), |ui| {
        ui.scope(|ui| {
            ui.visuals_mut().widgets.hovered.weak_bg_fill = colors::DARK_RED;
            ui.visuals_mut().widgets.active.weak_bg_fill = colors::DARKER_RED;
            if ui
                .button(" 🗑 ")
                .on_hover_text_at_pointer(format!("Delete {name}"))
                .clicked()
            {
                entries.remove_selected();
                *modified = true;
            }
        });
    });
}

fn mk_add<N>(ui: &mut egui::Ui, name: &str, entries: &mut N, modified: &mut bool)
where
    N: NamedEntries,
{
    ui.add_enabled_ui(true, |ui| {
        let response = ui
            .scope(|ui| {
                ui.visuals_mut().widgets.hovered.weak_bg_fill = colors::DARK_GREEN;
                ui.visuals_mut().widgets.active.weak_bg_fill = colors::DARKER_GREEN;
                ui.button(" ➕ ")
                    .on_hover_text_at_pointer(format!("Add new {name}"))
            })
            .inner;

        let popup_id = ui.make_persistent_id(format!("add-{name}"));
        if response.clicked() {
            egui::Popup::open_id(ui.ctx(), popup_id);
        }
        mk_name_popup(
            entries,
            ui,
            name,
            popup_id,
            response,
            |_state| String::new(),
            |entries, name| {
                entries.add_new(&name);
                *modified = true;
            },
        );
    });
}

fn mk_rename<N>(ui: &mut egui::Ui, name: &str, entries: &mut N, modified: &mut bool)
where
    N: NamedEntries,
{
    ui.add_enabled_ui(entries.editable(), |ui| {
        let response = ui
            .button("Rename")
            .on_hover_text_at_pointer(format!("Rename {name}"));
        let popup_id = ui.make_persistent_id(format!("rename-{name}"));
        if response.clicked() {
            egui::Popup::open_id(ui.ctx(), popup_id);
        }
        mk_name_popup(
            entries,
            ui,
            name,
            popup_id,
            response,
            |entries| entries.selected_name().to_string(),
            |entries, name| {
                entries.rename_selected(name);
                *modified = true;
            },
        );
    });
}

fn mk_duplicate<N>(ui: &mut egui::Ui, name: &str, entries: &mut N, modified: &mut bool)
where
    N: NamedEntries,
{
    let hint = if entries.editable() {
        format!("Duplicate {name}")
    } else {
        "Copy to my presets, to change it".to_string()
    };
    let response = ui.button("🗐").on_hover_text_at_pointer(hint);
    let popup_id = ui.make_persistent_id(format!("duplicate-{name}"));
    if response.clicked() {
        egui::Popup::open_id(ui.ctx(), popup_id);
    }
    mk_name_popup(
        entries,
        ui,
        name,
        popup_id,
        response,
        |state| state.duplicate_name(),
        |state, name| {
            state.duplicate_selected(name);
            *modified = true;
        },
    );
}

fn mk_dropdown<N>(ui: &mut egui::Ui, name: &str, entries: &mut N, modified: &mut bool)
where
    N: NamedEntries,
{
    let mut selected = entries.selected_name().to_owned();

    egui::ComboBox::from_id_salt(format!("dropdown-{name}"))
        .width(ui.available_width())
        .selected_text(selected.clone())
        .show_ui(ui, |ui| {
            for name in entries.names() {
                match name {
                    Some(name) => {
                        ui.selectable_value(&mut selected, name.clone(), name);
                    }
                    None => {
                        ui.separator();
                    }
                }
            }
        });

    if selected != entries.selected_name() {
        entries.select(selected);
        *modified = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_name_popup<N>(
    entries: &mut N,
    ui: &egui::Ui,
    name: &str,
    popup_id: egui::Id,
    response: egui::Response,
    default_name: impl Fn(&mut N) -> String,
    mut accept: impl FnMut(&mut N, String),
) where
    N: NamedEntries,
{
    let data_id = popup_id.with("data");
    let mut popup: NamePopup = ui.data(|data| data.get_temp(data_id)).unwrap_or_default();
    popup.buffer_needs_prefill_and_focus = custom_popup_below_widget(popup_id, &response, |ui| {
        ui.set_min_width(200.0);
        ui.vertical(|ui| {
            if popup.buffer_needs_prefill_and_focus {
                popup.buffer = default_name(entries);
            }

            let res = ui.add(
                egui::TextEdit::singleline(&mut popup.buffer)
                    .hint_text(format!("Enter new {name} name")),
            );
            if popup.buffer_needs_prefill_and_focus {
                res.request_focus();
            }

            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    egui::Popup::close_id(ui.ctx(), popup_id);
                }

                let invalid_name = popup.buffer.is_empty() || entries.contains(&popup.buffer);
                let clicked = ui
                    .add_enabled(!invalid_name, egui::Button::new("OK"))
                    .clicked();
                if !invalid_name && (clicked || is_committed(&res)) {
                    egui::Popup::close_id(ui.ctx(), popup_id);
                    accept(entries, std::mem::take(&mut popup.buffer));
                }
            });
        });
    })
    .is_none();

    ui.data_mut(|data| data.insert_temp(data_id, popup));
}
