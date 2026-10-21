//! Started on its own, mint checks that Steam still starts DRG through it. Setup put it into DRG's
//! launch options; without it there, the mods aren't updated before the game starts any more. If
//! it's gone, mint offers to put it back.

use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use tracing::{info, warn};

use super::App;
use crate::setup::{self, Paths};

pub(super) struct SteamPrompt {
    paths: Paths,
    step: Step,
}

enum Step {
    Ask,
    /// "Yes" while Steam runs: it would undo the change when it exits.
    WaitingForSteam,
    Done(Result<(), String>),
}

enum Answer {
    Yes,
    No,
    StopAsking,
    Close,
}

impl SteamPrompt {
    /// The prompt, if this is the mint setup installed and DRG doesn't start through it.
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

    fn restore(&mut self) {
        let result = setup::set_launch_options(&self.paths);
        self.step = Step::Done(if result.through_mint {
            info!("put mint back into DRG's launch options");
            Ok(())
        } else {
            Err("Steam's launch options couldn't be changed. mint's log has the details.".into())
        });
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
        if let Step::WaitingForSteam = prompt.step {
            if setup::steam_running() {
                ctx.request_repaint_after(Duration::from_secs(1));
            } else {
                prompt.restore();
            }
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
                            "DRG does not appear to be set up to launch through mint. Would you \
                             like me to restore that?",
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
                    Step::WaitingForSteam => {
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
                    Step::Done(result) => {
                        match result {
                            Ok(()) => {
                                ui.label("Done. Deep Rock Galactic starts through mint again.")
                            }
                            Err(e) => {
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
                info!("putting mint back into DRG's launch options");
                if setup::steam_running() {
                    prompt.step = Step::WaitingForSteam;
                } else {
                    prompt.restore();
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
