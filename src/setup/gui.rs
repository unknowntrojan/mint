//! The setup window: shows what it found, waits for Steam to close, then sets mint up.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, RichText};
use tracing::{error, info};

use super::{Paths, Payload};

pub fn run(payload: Payload, paths: Paths) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mint pint setup")
            .with_inner_size([620.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "mint-setup",
        options,
        Box::new(|_| Ok(Box::new(Setup::new(payload, paths)))),
    )
    .map_err(|e| anyhow!("{e}"))
}

struct Done {
    lines: Vec<String>,
    through_steam: bool,
}

enum Stage {
    Ready,
    Working(mpsc::Receiver<Result<Done, String>>),
    Done(Done),
    Failed(String),
}

struct Setup {
    payload: Arc<Payload>,
    paths: Arc<Paths>,
    pak: Option<PathBuf>,
    pak_err: Option<String>,
    xbox: bool,
    previous: bool,
    steam_running: bool,
    /// An open mint would save its old config over the imported one when it exits.
    mint_running: bool,
    steam_checked: Instant,
    /// Set up without touching Steam's launch options.
    skip_steam: bool,
    stage: Stage,
}

impl Setup {
    fn new(payload: Payload, paths: Paths) -> Self {
        let pak = super::find_drg();
        info!(
            "DRG: {pak:?}, Steam configs: {:?}, Flatpak/Snap: {:?}",
            paths.steam_configs, paths.sandboxed_steam_configs
        );
        Self {
            payload: Arc::new(payload),
            previous: paths.previous_setup(),
            paths: Arc::new(paths),
            xbox: false,
            pak,
            pak_err: None,
            steam_running: super::steam_running(),
            mint_running: super::mint_running(),
            steam_checked: Instant::now(),
            skip_steam: false,
            stage: Stage::Ready,
        }
    }

    fn steam_found(&self) -> bool {
        !self.paths.steam_configs.is_empty() || !self.paths.sandboxed_steam_configs.is_empty()
    }

    fn steam_setup(&self) -> bool {
        !self.xbox && !self.skip_steam && self.steam_found()
    }

    fn choose_pak(&mut self) {
        let Some(pak) = rfd::FileDialog::new()
            .set_title(
                "Select FSD-WindowsNoEditor.pak (Steam) or FSD-WinGDK.pak (Xbox / Game Pass)",
            )
            .add_filter("Deep Rock Galactic", &["pak"])
            .pick_file()
        else {
            return;
        };
        match super::check_pak(&pak) {
            Ok(xbox) => {
                self.xbox = xbox;
                self.pak = Some(pak);
                self.pak_err = None;
            }
            Err(e) => self.pak_err = Some(format!("{e:#}")),
        }
    }

    fn start(&mut self) {
        let (tx, rx) = mpsc::channel();
        let (payload, paths, pak) = (self.payload.clone(), self.paths.clone(), self.pak.clone());
        let steam = self.steam_setup();
        std::thread::spawn(move || {
            let result = (|| -> std::io::Result<Done> {
                if super::mint_running() {
                    return Err(std::io::Error::other(
                        "mint is open. Close it and try again.",
                    ));
                }
                let mut lines = Vec::new();
                lines.extend(super::install_exe(&payload, &paths)?);
                super::install_support(&payload, &paths)?;
                lines.extend(super::import(&payload, &paths, pak.as_deref(), steam)?);
                lines.extend(super::add_shortcuts(&paths));
                let mut through_steam = false;
                if steam && super::steam_running() {
                    // started since the window last looked; it would undo the change on exit
                    lines.push(
                        "Steam was started meanwhile, so it still starts DRG without mint. Close \
                         Steam completely and run mint-setup again."
                            .into(),
                    );
                } else if steam {
                    let opts = super::set_launch_options(&paths);
                    lines.extend(opts.done);
                    through_steam = opts.through_mint;
                }
                Ok(Done {
                    lines,
                    through_steam,
                })
            })();
            if let Err(e) = &result {
                error!("setup failed: {e}");
            }
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
        self.stage = Stage::Working(rx);
    }
}

fn row(ui: &mut egui::Ui, ok: Option<bool>, what: &str, add: impl FnOnce(&mut egui::Ui)) {
    let (icon, color) = match ok {
        Some(true) => ("✔", Color32::from_rgb(80, 200, 120)),
        Some(false) => ("✖", Color32::from_rgb(230, 90, 80)),
        None => ("ℹ", Color32::GRAY),
    };
    ui.label(RichText::new(icon).color(color).size(18.0));
    ui.label(RichText::new(what).strong());
    ui.horizontal_wrapped(add);
    ui.end_row();
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Stage::Working(rx) = &self.stage {
            match rx.try_recv() {
                Ok(Ok(done)) => self.stage = Stage::Done(done),
                Ok(Err(e)) => self.stage = Stage::Failed(e),
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.stage = Stage::Failed(
                        "Setup stopped unexpectedly. Details are in setup.log next to mint.".into(),
                    )
                }
            }
        }
        if matches!(self.stage, Stage::Ready) {
            if self.steam_checked.elapsed() > Duration::from_secs(1) {
                self.steam_running = super::steam_running();
                self.mint_running = super::mint_running();
                self.steam_checked = Instant::now();
            }
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("Set up mint");
            ui.add_space(8.0);
            match &self.stage {
                Stage::Ready | Stage::Working(_) => self.show_plan(ui),
                Stage::Done(done) => {
                    let (lines, through_steam) = (done.lines.clone(), done.through_steam);
                    self.show_done(ui, &lines, through_steam);
                }
                Stage::Failed(e) => {
                    let e = e.clone();
                    ui.label(
                        RichText::new("Setup didn't work:")
                            .color(Color32::from_rgb(230, 90, 80))
                            .strong(),
                    );
                    ui.label(e);
                    ui.add_space(8.0);
                    if ui.button("Try again").clicked() {
                        self.stage = Stage::Ready;
                    }
                }
            }
        });
    }
}

impl Setup {
    fn show_plan(&mut self, ui: &mut egui::Ui) {
        let working = matches!(self.stage, Stage::Working(_));
        egui::Grid::new("plan")
            .num_columns(3)
            .spacing([10.0, 12.0])
            .show(ui, |ui| {
                let pak = self.pak.clone();
                let mut choose = false;
                row(ui, Some(pak.is_some()), "Deep Rock Galactic", |ui| {
                    match &pak {
                        Some(p) => ui.label(p.display().to_string()),
                        None => ui.label("not found"),
                    };
                    choose = ui
                        .add_enabled(!working, egui::Button::new("Choose…"))
                        .clicked();
                    if let Some(e) = &self.pak_err {
                        ui.label(RichText::new(e).color(Color32::from_rgb(230, 90, 80)));
                    }
                });
                if choose {
                    self.choose_pak();
                }

                if self.xbox {
                    row(ui, None, "Steam", |ui| {
                        ui.label("Xbox / Game Pass version: start mint before the game and click Install mods.");
                    });
                } else if !self.steam_found() {
                    row(ui, None, "Steam", |ui| {
                        ui.label("not found, so you'll start mint yourself before playing.");
                    });
                } else if self.skip_steam {
                    row(ui, None, "Steam", |ui| {
                        ui.label("left alone: you'll start mint yourself before playing.");
                        if ui.add_enabled(!working, egui::Button::new("Undo")).clicked() {
                            self.skip_steam = false;
                        }
                    });
                } else if self.steam_running {
                    row(ui, Some(false), "Steam", |ui| {
                        ui.label(
                            "is running. Close it completely: right-click its icon next to the \
                             clock, then Exit.",
                        );
                        if ui
                            .add_enabled(!working, egui::Button::new("Skip"))
                            .on_hover_text("Set up without changing Steam; you'll start mint yourself before playing.")
                            .clicked()
                        {
                            self.skip_steam = true;
                        }
                    });
                } else if self.paths.steam_configs.is_empty() {
                    row(ui, None, "Steam", |ui| {
                        ui.label(
                            "is the Flatpak or Snap version, which can't start mint, so you'll \
                             start mint yourself before playing.",
                        );
                    });
                } else {
                    row(ui, Some(true), "Steam", |ui| {
                        ui.label("closed. Pressing Play will get your mods ready first.");
                    });
                }

                if self.mint_running {
                    row(ui, Some(false), "mint", |ui| {
                        ui.label("is open. Close it first.");
                    });
                }

                if self.previous {
                    row(ui, None, "Earlier mint", |ui| {
                        ui.label("found; it's kept as a backup next to the new one.");
                    });
                }
            });

        ui.add_space(16.0);
        let ready =
            self.pak.is_some() && !self.mint_running && !(self.steam_setup() && self.steam_running);
        ui.horizontal(|ui| {
            let button = egui::Button::new(RichText::new("  Set up  ").size(18.0));
            if ui.add_enabled(ready && !working, button).clicked() {
                self.start();
            }
            if working {
                ui.spinner();
            } else if self.pak.is_none() {
                ui.label("Choose Deep Rock Galactic first.");
            } else if self.mint_running {
                ui.label("Waiting for mint to close…");
            } else if !ready {
                ui.label("Waiting for Steam to close…");
            }
        });
    }

    fn show_done(&mut self, ui: &mut egui::Ui, lines: &[String], through_steam: bool) {
        ui.label(
            RichText::new("✔ Done!")
                .color(Color32::from_rgb(80, 200, 120))
                .size(18.0),
        );
        ui.add_space(6.0);
        for line in lines {
            ui.label(format!("• {line}"));
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new(if through_steam {
                "From now on just start Deep Rock Galactic in Steam. mint gets your mods ready \
             first and then starts the game."
            } else if cfg!(windows) {
                "Before playing, open mint (Start Menu or Desktop) and click Install mods."
            } else {
                "Before playing, open mint from your app menu and click Install mods."
            })
            .strong(),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Open mint").clicked() {
                if let Err(e) = super::start(&self.paths) {
                    error!("starting mint: {e}");
                }
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if ui.button("Close").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}
