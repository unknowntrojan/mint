//! Started by Steam (`mint launch %command%`, through mint-launch.exe on Windows; setup puts that
//! into DRG's launch options), mint shows a small chooser instead of the mod manager: it checks for
//! a newer mint and mod updates, installs the mods if anything changed since the last install, and
//! then counts down to Play. Any click stops the countdown. "Play without mods" takes the mods out
//! of the game first, "Open mod manager" switches to the full window. Steam's Safe Mode entry
//! (`-fsdsafemode`) skips all of it and starts the game without mods.
//!
//! The process stays until the game exits, so Steam keeps counting the game as running.

use std::ops::Deref;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use eframe::egui::{self, Color32, RichText};
use mint_lib::DRGInstallation;
use mint_lib::mod_info::MetaConfig;
use sha2::{Digest, Sha256};
use tracing::{error, info};

use super::{App, LastAction, LastActionStatus, SpecFetchProgress, message};
use crate::remote;

/// The game mint started; `gui` waits for it after the window closed.
pub(super) static GAME: Mutex<Option<Child>> = Mutex::new(None);

/// In the data directory: what the last successful install was (see [`App::install_fingerprint`]).
const INSTALLED: &str = "installed-mods";

const COUNTDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// For the self-update check: a new release restarts mint, which carries on.
    Waiting,
    /// Mod versions are being refreshed from mod.io.
    Refreshing,
    Installing,
    /// Ready to play; plays by itself at the instant, unless someone interacted.
    Ready(Option<Instant>),
    /// Installing failed; Play starts the game with whatever is installed.
    Failed,
    /// The game is starting and the window closing.
    Started,
}

pub(super) struct Chooser {
    step: Step,
    /// Someone clicked or typed: no countdown any more.
    interacted: bool,
    refresh_ok: Option<bool>,
    error: Option<String>,
}

impl Chooser {
    pub(super) fn new() -> Self {
        Self {
            step: Step::Waiting,
            interacted: false,
            refresh_ok: None,
            error: None,
        }
    }
}

impl App {
    fn safe_mode(&self) -> bool {
        self.args
            .as_ref()
            .is_some_and(|a| a.iter().any(|arg| arg.eq_ignore_ascii_case("-fsdsafemode")))
    }

    pub(super) fn drive_chooser(&mut self, ctx: &egui::Context) {
        let Some(step) = self.chooser.as_ref().map(|c| c.step) else {
            return;
        };
        match step {
            Step::Waiting => {
                if self.self_update.is_some() || self.needs_restart {
                    return;
                }
                if self.safe_mode() {
                    info!("Steam's Safe Mode: starting the game without mods");
                    self.play_without_mods(ctx);
                } else if self.state.config.drg_pak_path.is_none() {
                    self.chooser_failed("Deep Rock Galactic isn't set; open the mod manager and set it in the settings.".into());
                } else {
                    info!("started through Steam: refreshing mod versions");
                    message::UpdateCache::send(self);
                    self.set_step(Step::Refreshing);
                }
            }
            Step::Refreshing => {
                if self.update_rid.is_some() {
                    return;
                }
                let ok = self.last_succeeded();
                if let Some(c) = &mut self.chooser {
                    c.refresh_ok = Some(ok);
                }
                self.ensure_installed(ctx);
            }
            Step::Installing => {
                if self.integrate_rid.is_some() {
                    return;
                }
                if self.last_succeeded() {
                    self.ready();
                } else {
                    let error = match &self.last_action {
                        Some(LastAction {
                            status: LastActionStatus::Failure(e),
                            ..
                        }) => e.clone(),
                        _ => "installing the mods failed".into(),
                    };
                    self.chooser_failed(error);
                }
            }
            Step::Ready(Some(at)) => {
                if Instant::now() >= at {
                    self.play(ctx);
                } else {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
            Step::Ready(None) | Step::Failed | Step::Started => {}
        }
    }

    fn set_step(&mut self, step: Step) {
        if let Some(c) = &mut self.chooser {
            c.step = step;
        }
    }

    fn chooser_failed(&mut self, error: String) {
        error!("{error}");
        if let Some(c) = &mut self.chooser {
            c.step = Step::Failed;
            c.error = Some(error);
        }
    }

    fn last_succeeded(&self) -> bool {
        matches!(
            self.last_action,
            Some(LastAction {
                status: LastActionStatus::Success(_),
                ..
            })
        )
    }

    /// Installs the selected mods unless they're installed as they are.
    fn ensure_installed(&mut self, ctx: &egui::Context) {
        if self.mods_installed() {
            info!("mods are installed and up to date");
            self.ready();
        } else {
            info!("installing mods before the game starts");
            if let Some(c) = &mut self.chooser {
                c.error = None;
            }
            self.start_install(ctx);
            self.set_step(Step::Installing);
        }
    }

    fn ready(&mut self) {
        if let Some(c) = &mut self.chooser {
            c.step = Step::Ready((!c.interacted).then(|| Instant::now() + COUNTDOWN));
        }
    }

    fn play(&mut self, ctx: &egui::Context) {
        let Some((program, args)) = self.args.as_ref().and_then(|a| a.split_first()) else {
            return;
        };
        info!("starting {program:?} {args:?}");
        match std::process::Command::new(program).args(args).spawn() {
            Ok(child) => {
                *GAME.lock().unwrap() = Some(child);
                // the chooser stays drawn until the window is gone
                self.set_step(Step::Started);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.chooser_failed(format!("starting the game: {e}")),
        }
    }

    fn play_without_mods(&mut self, ctx: &egui::Context) {
        match self.uninstall_mods() {
            Ok(()) => self.play(ctx),
            Err(e) => self.chooser_failed(format!("taking the mods out failed: {e}")),
        }
    }

    fn open_manager(&mut self, ctx: &egui::Context) {
        self.chooser = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(900.0, 500.0)));
    }

    /// The chooser, drawn instead of the mod manager.
    pub(super) fn ui_chooser(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(chooser) = &mut self.chooser else {
            return;
        };
        // any click or key press stops the countdown
        if ctx.input(|i| {
            i.pointer.any_pressed()
                || i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { .. }))
        }) {
            chooser.interacted = true;
            if let Step::Ready(Some(_)) = chooser.step {
                chooser.step = Step::Ready(None);
            }
        }
        let step = chooser.step;
        let (refresh_ok, error) = (chooser.refresh_ok, chooser.error.clone());
        let busy = matches!(
            step,
            Step::Waiting | Step::Refreshing | Step::Installing | Step::Started
        );
        if step == Step::Started {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading("Starting Deep Rock Galactic…");
            });
            return;
        }

        // the buttons first, at the bottom, so nothing above can push them out of the window
        egui::Panel::bottom("chooser-actions").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = match step {
                    Step::Ready(Some(at)) => {
                        let left = at.saturating_duration_since(Instant::now()).as_secs() + 1;
                        format!("  Play ({left})  ")
                    }
                    Step::Failed => "  Play anyway  ".into(),
                    _ => "  Play  ".into(),
                };
                let play = egui::Button::new(RichText::new(label).size(20.0));
                if ui
                    .add_enabled(matches!(step, Step::Ready(_) | Step::Failed), play)
                    .clicked()
                {
                    self.play(&ctx);
                }
                if ui
                    .add_enabled(
                        !busy || step == Step::Waiting,
                        egui::Button::new("Play without mods"),
                    )
                    .on_hover_text(
                        "Takes the mods out of the game first; the next Play puts them back.",
                    )
                    .clicked()
                {
                    self.play_without_mods(&ctx);
                }
                if ui
                    .add_enabled(
                        step != Step::Installing,
                        egui::Button::new("Open mod manager"),
                    )
                    .clicked()
                {
                    self.open_manager(&ctx);
                }
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("Deep Rock Galactic");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Mods:");
                ui.add_enabled_ui(!busy, |ui| self.ui_chooser_profiles(ui, &ctx));
            });
            ui.add_space(10.0);

            egui::Grid::new("checks")
                .num_columns(3)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    self.ui_checks(ui, step, refresh_ok, error.as_deref())
                });
        });
    }

    fn ui_chooser_profiles(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let server = self.state.config.preset_server().to_string();
        let remote: Vec<(String, String)> = self
            .remote_presets
            .for_server(&server)
            .iter()
            .map(|p| (p.slug.clone(), format!("{}{}", remote::PREFIX, p.name)))
            .collect();
        let profiles: Vec<String> = self.state.mod_data.profiles.keys().cloned().collect();
        let selected = match self.selected_remote() {
            Some(p) => format!("{}{}", remote::PREFIX, p.name),
            None => self.state.mod_data.active_profile.clone(),
        };
        let mut chosen = None;
        egui::ComboBox::from_id_salt("chooser-profile")
            .selected_text(selected)
            .width(260.0)
            .show_ui(ui, |ui| {
                for name in &profiles {
                    if ui.selectable_label(false, name).clicked() {
                        chosen = Some((None, name.clone()));
                    }
                }
                for (slug, name) in &remote {
                    if ui.selectable_label(false, name).clicked() {
                        chosen = Some((Some(slug.clone()), name.clone()));
                    }
                }
            });
        if let Some((slug, name)) = chosen {
            match slug {
                Some(slug) => self.state.config.remote_preset = Some(slug),
                None => {
                    self.state.config.remote_preset = None;
                    self.state.mod_data.active_profile = name;
                    self.state.mod_data.save().unwrap();
                }
            }
            self.state.config.save().unwrap();
            self.ensure_installed(ctx);
        }
    }

    fn ui_checks(
        &self,
        ui: &mut egui::Ui,
        step: Step,
        refresh_ok: Option<bool>,
        error: Option<&str>,
    ) {
        use Check::*;
        match self.self_update.as_ref().map(|h| h.state) {
            Some(None) => row(ui, Busy, "mint", "checking for a newer version…"),
            Some(Some(_)) => row(ui, Busy, "mint", "updating…"),
            None => row(ui, Good, "mint", &crate::update::describe()),
        }
        match self.server_ok {
            None => row(ui, Busy, "Mod server", "contacting…"),
            Some(true) => row(ui, Good, "Mod server", "reachable"),
            Some(false) => row(
                ui,
                Warn,
                "Mod server",
                "not reachable; mods come from mod.io directly",
            ),
        }
        match (step, refresh_ok) {
            (Step::Waiting, _) => row(ui, Busy, "Mod updates", "waiting…"),
            (Step::Refreshing, _) => row(ui, Busy, "Mod updates", "checking…"),
            (_, Some(true)) => row(ui, Good, "Mod updates", "checked"),
            (_, Some(false)) => row(
                ui,
                Warn,
                "Mod updates",
                "couldn't check; using the versions from last time",
            ),
            (_, None) => row(ui, Warn, "Mod updates", "not checked"),
        }
        let total = self.install_specs().len();
        match step {
            Step::Installing => {
                let done = self.integrate_rid.as_ref().map_or(0, |h| {
                    h.state
                        .values()
                        .filter(|p| matches!(p, SpecFetchProgress::Complete))
                        .count()
                });
                row(ui, Busy, "Mods", &format!("installing… {done}/{total}"));
            }
            Step::Ready(_) => {
                row(
                    ui,
                    Good,
                    "Mods",
                    &format!("{total} installed and up to date"),
                );
                if !self.hook_present() {
                    row(
                        ui,
                        Bad,
                        "Mod hook",
                        "missing; an antivirus may have removed it, so some mods won't work",
                    );
                }
            }
            Step::Failed => {
                let full = error.unwrap_or("not installed");
                let short = short_error(full);
                row_with_hover(ui, Bad, "Mods", &short, Some(full));
            }
            Step::Waiting | Step::Refreshing | Step::Started => row(ui, Busy, "Mods", "waiting…"),
        }
    }
}

/// The first line of `error`, cut to fit the chooser; the whole thing goes into a hover text.
/// Install errors can be long: every pak version mint tried.
fn short_error(error: &str) -> String {
    const MAX: usize = 60;
    let first = error
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let more = first.len() < error.trim().len();
    match first.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}…", &first[..i]),
        None if more => format!("{first} …"),
        None => first.to_string(),
    }
}

/// What a status row of the chooser shows.
#[derive(Clone, Copy)]
enum Check {
    Busy,
    Good,
    Warn,
    Bad,
}

fn row(ui: &mut egui::Ui, check: Check, what: &str, text: &str) {
    row_with_hover(ui, check, what, text, None);
}

fn row_with_hover(ui: &mut egui::Ui, check: Check, what: &str, text: &str, hover: Option<&str>) {
    let (icon, color) = match check {
        Check::Busy => {
            ui.spinner();
            ("", Color32::GRAY)
        }
        Check::Good => ("✔", Color32::from_rgb(80, 200, 120)),
        Check::Warn => ("⚠", Color32::from_rgb(230, 170, 60)),
        Check::Bad => ("✖", Color32::from_rgb(230, 90, 80)),
    };
    if !icon.is_empty() {
        ui.label(RichText::new(icon).color(color));
    }
    ui.label(RichText::new(what).strong());
    let text = ui.label(text);
    if let Some(hover) = hover {
        text.on_hover_text(hover);
    }
    ui.end_row();
}

impl App {
    /// Everything an install of the current mods depends on: the mods as they resolve now, mint
    /// itself (it brings the hook), its settings, and the game's own pak (a DRG update needs a
    /// new install). `None` without a DRG pak.
    pub(super) fn install_fingerprint(&self) -> Option<String> {
        let pak = self.state.config.drg_pak_path.as_ref()?;
        let meta = std::fs::metadata(pak).ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs();
        let mut h = Sha256::new();
        h.update(mint_lib::built_info::GIT_VERSION.unwrap_or_default());
        h.update(crate::update::describe());
        h.update(format!("|{}|{}|{modified}|", pak.display(), meta.len()));
        h.update(format!("{:?}", MetaConfig::from(self.state.config.deref())));
        for spec in self.install_specs() {
            let resolved = self
                .state
                .store
                .get_mod_info(&spec)
                .map_or(spec.url, |info| info.resolution.url.0);
            h.update(resolved);
            h.update([0]);
        }
        Some(hex::encode(h.finalize()))
    }

    /// Remembers that the current mods are installed.
    pub(super) fn record_install(&self) {
        let Some(fingerprint) = self.install_fingerprint() else {
            return;
        };
        let path = self.state.dirs.data_dir.join(INSTALLED);
        if let Err(e) = std::fs::write(&path, fingerprint) {
            error!("writing {}: {e}", path.display());
        }
    }

    fn installed_files(&self) -> Option<[std::path::PathBuf; 2]> {
        let drg = DRGInstallation::from_pak_path(self.state.config.drg_pak_path.as_ref()?).ok()?;
        Some([
            drg.paks_path().join("mods_P.pak"),
            drg.binaries_directory()
                .join(drg.installation_type.hook_dll_name()),
        ])
    }

    fn hook_present(&self) -> bool {
        self.installed_files()
            .is_some_and(|[_, hook]| hook.is_file())
    }

    /// Whether the current mods are installed in the game, unchanged since.
    fn mods_installed(&self) -> bool {
        let Some(files) = self.installed_files() else {
            return false;
        };
        let recorded = std::fs::read_to_string(self.state.dirs.data_dir.join(INSTALLED)).ok();
        files.iter().all(|f| f.is_file())
            && matches!((recorded, self.install_fingerprint()), (Some(r), Some(f)) if r.trim() == f)
    }
}

#[cfg(test)]
mod tests {
    use super::short_error;

    #[test]
    fn errors_are_cut_to_their_first_line() {
        assert_eq!(short_error("no network"), "no network");
        assert_eq!(short_error("\n  first\nsecond"), "first …");
        let long = "x".repeat(100);
        assert_eq!(short_error(&long), format!("{}…", "x".repeat(60)));
        // cut on characters, not bytes
        assert_eq!(short_error(&"ä".repeat(61)), format!("{}…", "ä".repeat(60)));
    }
}
