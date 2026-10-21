//! The mint launcher: Steam starting DRG through mint, which setup puts into DRG's launch options.
//! Started on its own, mint checks that it's still there; without it, the mods aren't updated
//! before the game starts any more, so mint offers to put it back. The settings install and
//! uninstall it.

use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use tracing::{info, warn};

use super::App;
use crate::setup::{self, Paths};

pub(super) struct SteamPrompt {
    paths: Paths,
    step: Step,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Install,
    Uninstall,
}

enum Step {
    /// The mint launcher is gone: put it back?
    Ask,
    /// Steam runs, and would undo the change when it exits.
    WaitingForSteam(Change),
    Done(Change, Result<(), String>),
}

enum Answer {
    Yes,
    No,
    StopAsking,
    Close,
}

impl SteamPrompt {
    /// The question, if this is the mint setup installed and DRG doesn't start through it.
    pub(super) fn check() -> Option<Self> {
        let paths = Paths::for_user()?;
        let exe = std::env::current_exe().ok()?;
        paths.launch_options_missing(&exe).then(|| {
            info!("DRG's launch options don't start mint");
            Self {
                paths,
                step: Step::Ask,
            }
        })
    }

    /// Whether the mint launcher is installed, or `None` if that isn't up to this mint (not the
    /// mint setup installed, or not DRG's Steam version).
    pub(super) fn launcher_installed() -> Option<bool> {
        let paths = Paths::for_user()?;
        let exe = std::env::current_exe().ok()?;
        paths
            .manages_steam_launch(&exe)
            .then(|| paths.launcher_installed())
    }

    /// Installs or uninstalls the mint launcher, once Steam is closed.
    pub(super) fn change(change: Change) -> Option<Self> {
        let mut prompt = Self {
            paths: Paths::for_user()?,
            step: Step::WaitingForSteam(change),
        };
        if !setup::steam_running() {
            prompt.apply(change);
        }
        Some(prompt)
    }

    fn apply(&mut self, change: Change) {
        info!("mint launcher: {change:?}");
        let ok = match change {
            Change::Install => setup::set_launch_options(&self.paths).through_mint,
            Change::Uninstall => setup::remove_launch_options(&self.paths),
        };
        // setup and the question go by this, whether or not every Steam user could be changed
        if let Err(e) = self
            .paths
            .set_steam_launch_wanted(change == Change::Install)
        {
            warn!("remembering it: {e}");
        }
        self.step = Step::Done(
            change,
            if ok {
                Ok(())
            } else {
                Err(
                    "Steam's launch options couldn't be changed. mint's log has the details."
                        .into(),
                )
            },
        );
    }
}

impl App {
    pub(super) fn show_steam_prompt(&mut self, ctx: &egui::Context) {
        // an update restarts mint, which asks again
        if matches!(self.self_update.as_ref().map(|h| h.state), Some(Some(_))) {
            return;
        }
        let Some(prompt) = &mut self.steam_prompt else {
            return;
        };
        if let Step::WaitingForSteam(change) = prompt.step {
            if setup::steam_running() {
                ctx.request_repaint_after(Duration::from_secs(1));
            } else {
                prompt.apply(change);
            }
        }
        if let (Step::Done(change, Ok(())), Some(settings)) =
            (&prompt.step, &mut self.settings_window)
        {
            settings.mint_launcher = Some(*change == Change::Install);
        }

        let response = egui::Modal::new("steam-prompt".into())
            .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(14.0))
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                let mut answer = None;
                match &prompt.step {
                    Step::Ask => {
                        ui.label(
                            RichText::new(
                                "DRG does not appear to be set up to launch through mint. Would \
                                 you like me to restore that?",
                            )
                            .strong(),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "mint injects itself into how the game starts via Steam's launch \
                                 options. When you remove mint from them, your mods won't update \
                                 automatically anymore.",
                            )
                            .size(11.0)
                            .weak(),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("Yes").clicked() {
                                answer = Some(Answer::Yes);
                            }
                            if ui.button("No").clicked() {
                                answer = Some(Answer::No);
                            }
                            if ui.button("No, and stop asking").clicked() {
                                answer = Some(Answer::StopAsking);
                            }
                        });
                    }
                    Step::WaitingForSteam(_) => {
                        ui.label(
                            "Steam is running and would undo the change when it exits. Close it \
                             completely (Steam menu, then Exit); mint carries on by itself.",
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Waiting for Steam to close…");
                            if ui.button("Cancel").clicked() {
                                answer = Some(Answer::Close);
                            }
                        });
                    }
                    Step::Done(change, result) => {
                        match (change, result) {
                            (Change::Install, Ok(())) => {
                                ui.label("Done. Deep Rock Galactic starts through mint now.")
                            }
                            (Change::Uninstall, Ok(())) => ui.label(
                                "Done. Deep Rock Galactic starts without mint now. To update your \
                                 mods, open mint and click Install mods before playing.",
                            ),
                            (_, Err(e)) => {
                                ui.label(RichText::new(e).color(Color32::from_rgb(230, 90, 80)))
                            }
                        };
                        ui.add_space(10.0);
                        if ui.button("OK").clicked() {
                            answer = Some(Answer::Close);
                        }
                    }
                }
                answer
            });
        // clicking next to it or Escape: not now, or cancel
        let dismissed = response.should_close();
        let answer = response.inner.or_else(|| {
            dismissed.then_some(match prompt.step {
                Step::Ask => Answer::No,
                _ => Answer::Close,
            })
        });
        if answer.is_some() {
            // show what the answer led to without waiting for the next input
            ctx.request_repaint();
        }
        match answer {
            Some(Answer::Yes) => {
                if setup::steam_running() {
                    prompt.step = Step::WaitingForSteam(Change::Install);
                } else {
                    prompt.apply(Change::Install);
                }
            }
            Some(Answer::No) => {
                info!("not putting mint back into DRG's launch options");
                self.steam_prompt = None;
            }
            Some(Answer::StopAsking) => {
                info!("not putting mint back into DRG's launch options, and not asking again");
                if let Err(e) = prompt.paths.set_steam_launch_wanted(false) {
                    warn!("remembering that: {e}");
                }
                self.steam_prompt = None;
            }
            Some(Answer::Close) => self.steam_prompt = None,
            None => {}
        }
    }
}
