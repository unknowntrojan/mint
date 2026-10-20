//! Started by Steam (`mint launch %command%`; setup puts that into DRG's launch options): mint
//! refreshes mod versions, installs the mods if anything changed since the last install, starts
//! the game and closes its window. The process stays until the game exits, so Steam keeps
//! counting the game as running. If installing fails, mint stays open with the error, and its
//! Launch game button starts the game anyway.

use std::ops::Deref;
use std::process::Child;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use eframe::egui;
use mint_lib::DRGInstallation;
use mint_lib::mod_info::MetaConfig;
use sha2::{Digest, Sha256};
use tracing::{error, info};

use super::{App, LastAction, LastActionStatus, message};

/// The game mint started; `gui` waits for it after the window closed.
pub(super) static GAME: Mutex<Option<Child>> = Mutex::new(None);

/// In the data directory: what the last successful install was (see [`App::install_fingerprint`]).
const INSTALLED: &str = "installed-mods";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoLaunch {
    /// For the self-update check: a new release restarts mint, which carries on.
    Waiting,
    /// Mod versions are being refreshed from mod.io.
    Refreshing,
    Installing,
}

impl App {
    pub(super) fn drive_auto_launch(&mut self, ctx: &egui::Context) {
        let Some(step) = self.auto_launch else {
            return;
        };
        match step {
            AutoLaunch::Waiting => {
                if self.self_update.is_some() || self.needs_restart {
                    return;
                }
                if self.state.config.drg_pak_path.is_none() {
                    self.auto_launch = None;
                    self.last_action = Some(LastAction::failure(
                        "Deep Rock Galactic not found, set it in the settings".into(),
                    ));
                    return;
                }
                info!("started through Steam: refreshing mod versions");
                message::UpdateCache::send(self);
                self.auto_launch = Some(AutoLaunch::Refreshing);
            }
            AutoLaunch::Refreshing => {
                if self.update_rid.is_some() {
                    return;
                }
                if self.mods_installed() {
                    info!("mods are installed and up to date");
                    self.launch_game(ctx);
                } else {
                    info!("installing mods before the game starts");
                    self.start_install(ctx);
                    self.auto_launch = Some(AutoLaunch::Installing);
                }
            }
            AutoLaunch::Installing => {
                if self.integrate_rid.is_some() {
                    return;
                }
                let installed = matches!(
                    self.last_action,
                    Some(LastAction {
                        status: LastActionStatus::Success(_),
                        ..
                    })
                );
                if installed {
                    self.launch_game(ctx);
                } else {
                    self.auto_launch = None;
                }
            }
        }
    }

    fn launch_game(&mut self, ctx: &egui::Context) {
        self.auto_launch = None;
        let Some((program, args)) = self.args.as_ref().and_then(|a| a.split_first()) else {
            return;
        };
        info!("starting {program:?} {args:?}");
        match std::process::Command::new(program).args(args).spawn() {
            Ok(child) => {
                *GAME.lock().unwrap() = Some(child);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                error!("starting the game: {e}");
                self.last_action = Some(LastAction::failure(format!("starting the game: {e}")));
            }
        }
    }

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

    /// Whether the current mods are installed in the game, unchanged since.
    fn mods_installed(&self) -> bool {
        let Some(pak) = &self.state.config.drg_pak_path else {
            return false;
        };
        let Ok(drg) = DRGInstallation::from_pak_path(pak) else {
            return false;
        };
        let files = [
            drg.paks_path().join("mods_P.pak"),
            drg.binaries_directory()
                .join(drg.installation_type.hook_dll_name()),
        ];
        let recorded = std::fs::read_to_string(self.state.dirs.data_dir.join(INSTALLED)).ok();
        files.iter().all(|f| f.is_file())
            && matches!((recorded, self.install_fingerprint()), (Some(r), Some(f)) if r.trim() == f)
    }
}
