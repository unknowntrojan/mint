use std::collections::BTreeSet;
use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::mpsc::{self, Sender},
    task::JoinHandle,
};
use tracing::*;

use super::UpdateState;
use super::{
    App, SpecFetchProgress, WindowProviderParameters,
    request_counter::{RequestCounter, RequestID},
};
use crate::gui::LastAction;
use crate::integrate::*;
use crate::mod_lints::{LintId, LintReport};
use crate::remote::{self, RemotePresets};
use crate::update;
use crate::*;
use crate::{
    providers::{FetchProgress, ModInfo, ModStore},
    state::ModConfig,
};
use mint_lib::mod_info::MetaConfig;
use remote_preset::Preset;

#[derive(Debug)]
pub struct MessageHandle<S> {
    pub rid: RequestID,
    pub handle: JoinHandle<()>,
    pub state: S,
}

#[derive(Debug)]
pub enum Message {
    ResolveMods(ResolveMods),
    Integrate(Integrate),
    FetchModProgress(FetchModProgress),
    UpdateCache(UpdateCache),
    LintMods(Box<LintMods>),
    Update(Update),
    UpdateProgress(UpdateProgress),
    FetchRemotePresets(FetchRemotePresets),
    PublishPreset(PublishPreset),
    UnpublishPreset(UnpublishPreset),
}

impl Message {
    pub fn handle(self, app: &mut App) {
        match self {
            Self::ResolveMods(msg) => msg.receive(app),
            Self::Integrate(msg) => msg.receive(app),
            Self::FetchModProgress(msg) => msg.receive(app),
            Self::UpdateCache(msg) => msg.receive(app),
            Self::LintMods(msg) => msg.receive(app),
            Self::Update(msg) => msg.receive(app),
            Self::UpdateProgress(msg) => msg.receive(app),
            Self::FetchRemotePresets(msg) => msg.receive(app),
            Self::PublishPreset(msg) => msg.receive(app),
            Self::UnpublishPreset(msg) => msg.receive(app),
        }
    }
}

#[derive(Debug)]
pub struct ResolveMods {
    rid: RequestID,
    specs: Vec<ModSpecification>,
    result: Result<HashMap<ModSpecification, ModInfo>, ProviderError>,
    is_dependency: bool,
}

impl ResolveMods {
    pub fn send(
        app: &mut App,
        ctx: &egui::Context,
        specs: Vec<ModSpecification>,
        is_dependency: bool,
    ) {
        let rid = app.request_counter.next();
        let store = app.state.store.clone();
        let ctx = ctx.clone();
        let tx = app.tx.clone();
        let handle = tokio::spawn(async move {
            let result = store.resolve_mods(&specs, false).await;
            tx.send(Message::ResolveMods(Self {
                rid,
                specs,
                result,
                is_dependency,
            }))
            .await
            .unwrap();
            ctx.request_repaint();
        });
        app.last_action = None;
        app.resolve_mod_rid = Some(MessageHandle {
            rid,
            handle,
            state: (),
        });
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) == app.resolve_mod_rid.as_ref().map(|r| r.rid) {
            match self.result {
                Ok(resolved_mods) => {
                    let primary_mods = self
                        .specs
                        .into_iter()
                        .collect::<HashSet<ModSpecification>>();
                    for (resolved_spec, info) in resolved_mods {
                        let is_dep = self.is_dependency || !primary_mods.contains(&resolved_spec);
                        let add = if is_dep {
                            // if mod is a dependency then check if there is a disabled
                            // mod that satisfies the dependency and enable it. if it
                            // is not a dependency then assume the user explicitly
                            // wants to add a specific mod version.
                            let active_profile = app.state.mod_data.active_profile.clone();
                            !app.state.mod_data.any_mod_mut(&active_profile, |mc| {
                                if mc.spec.satisfies_dependency(&resolved_spec) {
                                    mc.enabled = true;
                                    true
                                } else {
                                    false
                                }
                            })
                        } else {
                            true
                        };

                        if add {
                            app.state.mod_data.add_mod(ModConfig {
                                spec: info.spec.clone(),
                                required: info.suggested_require,
                                enabled: true,
                                priority: 0,
                            });
                        }
                    }
                    app.resolve_mod.clear();
                    app.state.mod_data.save().unwrap();
                    app.last_action = Some(LastAction::success(
                        "mods successfully resolved".to_string(),
                    ));
                }
                Err(ProviderError::NoProvider { url: _, factory }) => {
                    app.window_provider_parameters =
                        Some(WindowProviderParameters::new(factory, &app.state));
                    app.last_action = Some(LastAction::failure("no provider".to_string()));
                }
                Err(e) => {
                    error!("{}", e);
                    app.problematic_mod_id = e.opt_mod_id();
                    app.last_action = Some(LastAction::failure(e.to_string()));
                }
            }
            app.resolve_mod_rid = None;
        }
    }
}

#[derive(Debug)]
pub struct Integrate {
    rid: RequestID,
    result: Result<(), IntegrationError>,
}

impl Integrate {
    pub fn send(
        rc: &mut RequestCounter,
        store: Arc<ModStore>,
        mods: Vec<ModSpecification>,
        fsd_pak: PathBuf,
        config: MetaConfig,
        tx: Sender<Message>,
        ctx: egui::Context,
    ) -> MessageHandle<HashMap<ModSpecification, SpecFetchProgress>> {
        let rid = rc.next();
        MessageHandle {
            rid,
            handle: tokio::task::spawn(async move {
                let res =
                    integrate_async(store, ctx.clone(), mods, fsd_pak, config, rid, tx.clone())
                        .await;
                tx.send(Message::Integrate(Integrate { rid, result: res }))
                    .await
                    .unwrap();
                ctx.request_repaint();
            }),
            state: Default::default(),
        }
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) == app.integrate_rid.as_ref().map(|r| r.rid) {
            match self.result {
                Ok(()) => {
                    info!("integration complete");
                    app.last_action = Some(LastAction::success("integration complete".to_string()));
                    app.record_install();
                }
                Err(ref e)
                    if let IntegrationError::ProviderError { source } = e
                        && let ProviderError::NoProvider { url: _, factory } = source =>
                {
                    app.window_provider_parameters =
                        Some(WindowProviderParameters::new(factory, &app.state));
                    app.last_action = Some(LastAction::failure("no provider".to_string()));
                }
                Err(e) => {
                    error!("{}", e);
                    app.problematic_mod_id = e.opt_mod_id();
                    app.last_action = Some(LastAction::failure(e.to_string()));
                }
            }
            app.integrate_rid = None;
        }
    }
}

#[derive(Debug)]
pub struct FetchModProgress {
    rid: RequestID,
    spec: ModSpecification,
    progress: SpecFetchProgress,
}

impl FetchModProgress {
    fn receive(self, app: &mut App) {
        if let Some(MessageHandle { rid, state, .. }) = &mut app.integrate_rid
            && *rid == self.rid
        {
            state.insert(self.spec, self.progress);
        }
    }
}

#[derive(Debug)]
pub struct UpdateCache {
    rid: RequestID,
    result: Result<(), ProviderError>,
}

impl UpdateCache {
    pub fn send(app: &mut App) {
        let rid = app.request_counter.next();
        let tx = app.tx.clone();
        let store = app.state.store.clone();
        let handle = tokio::spawn(async move {
            let res = store.update_cache().await;
            tx.send(Message::UpdateCache(UpdateCache { rid, result: res }))
                .await
                .unwrap();
        });
        app.last_action = None;
        app.update_rid = Some(MessageHandle {
            rid,
            handle,
            state: (),
        });
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) == app.update_rid.as_ref().map(|r| r.rid) {
            match self.result {
                Ok(()) => {
                    info!("cache update complete");
                    app.last_action = Some(LastAction::success(
                        "successfully updated cache".to_string(),
                    ));
                }
                Err(ProviderError::NoProvider { url: _, factory }) => {
                    app.window_provider_parameters =
                        Some(WindowProviderParameters::new(factory, &app.state));
                    app.last_action = Some(LastAction::failure("no provider".to_string()));
                }
                Err(e) => {
                    error!("{}", e);
                    app.problematic_mod_id = e.opt_mod_id();
                    app.last_action = Some(LastAction::failure(e.to_string()));
                }
            }
            app.update_rid = None;
        }
    }
}

async fn integrate_async(
    store: Arc<ModStore>,
    ctx: egui::Context,
    mod_specs: Vec<ModSpecification>,
    fsd_pak: PathBuf,
    config: MetaConfig,
    rid: RequestID,
    message_tx: Sender<Message>,
) -> Result<(), IntegrationError> {
    let update = false;

    let mods = store.resolve_mods(&mod_specs, update).await?;

    let to_integrate = mod_specs
        .iter()
        .map(|u| mods[u].clone())
        .collect::<Vec<_>>();
    let res_map: HashMap<ModResolution, ModSpecification> = mods
        .iter()
        .map(|(spec, info)| (info.resolution.clone(), spec.clone()))
        .collect();
    let urls = to_integrate
        .iter()
        .map(|m| &m.resolution)
        .collect::<Vec<_>>();

    let (tx, mut rx) = mpsc::channel::<FetchProgress>(10);

    tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if let Some(spec) = res_map.get(progress.resolution()) {
                message_tx
                    .send(Message::FetchModProgress(FetchModProgress {
                        rid,
                        spec: spec.clone(),
                        progress: progress.into(),
                    }))
                    .await
                    .unwrap();
                ctx.request_repaint();
            }
        }
    });

    let paths = store.fetch_mods_ordered(&urls, update, Some(tx)).await?;

    tokio::task::spawn_blocking(|| {
        crate::integrate::integrate(
            fsd_pak,
            config,
            to_integrate.into_iter().zip(paths).collect(),
        )
    })
    .await??;

    Ok(())
}

#[derive(Debug)]
pub struct LintMods {
    rid: RequestID,
    result: Result<LintReport, IntegrationError>,
}

impl LintMods {
    pub fn send(
        rc: &mut RequestCounter,
        store: Arc<ModStore>,
        mods: Vec<ModSpecification>,
        enabled_lints: BTreeSet<LintId>,
        game_pak_path: Option<PathBuf>,
        tx: Sender<Message>,
        ctx: egui::Context,
    ) -> MessageHandle<()> {
        let rid = rc.next();

        let handle = tokio::task::spawn(async move {
            let paths_res =
                resolve_async_ordered(store, ctx.clone(), mods.clone(), rid, tx.clone()).await;
            let mod_path_pairs_res =
                paths_res.map(|paths| mods.into_iter().zip(paths).collect::<Vec<_>>());

            let report_res = match mod_path_pairs_res {
                Ok(pairs) => tokio::task::spawn_blocking(move || {
                    crate::mod_lints::run_lints(
                        &enabled_lints,
                        pairs.into_iter().collect(),
                        game_pak_path,
                    )
                })
                .await
                .unwrap()
                .map_err(Into::into),
                Err(e) => Err(e),
            };

            tx.send(Message::LintMods(
                LintMods {
                    rid,
                    result: report_res,
                }
                .into(),
            ))
            .await
            .unwrap();
            ctx.request_repaint();
        });

        MessageHandle {
            rid,
            handle,
            state: Default::default(),
        }
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) == app.lint_rid.as_ref().map(|r| r.rid) {
            match self.result {
                Ok(report) => {
                    info!("lint mod report complete");
                    app.lint_report = Some(report);
                    app.last_action =
                        Some(LastAction::success("lint mod report complete".to_string()));
                }
                Err(ref e)
                    if let IntegrationError::ProviderError { source } = e
                        && let ProviderError::NoProvider { url: _, factory } = source =>
                {
                    app.window_provider_parameters =
                        Some(WindowProviderParameters::new(factory, &app.state));
                    app.last_action = Some(LastAction::failure("no provider".to_string()));
                }
                Err(e) => {
                    error!("{}", e);
                    app.problematic_mod_id = e.opt_mod_id();
                    app.last_action = Some(LastAction::failure(e.to_string()));
                }
            }
            app.integrate_rid = None;
        }
    }
}

async fn resolve_async_ordered(
    store: Arc<ModStore>,
    ctx: egui::Context,
    mod_specs: Vec<ModSpecification>,
    rid: RequestID,
    message_tx: Sender<Message>,
) -> Result<Vec<PathBuf>, IntegrationError> {
    let update = false;

    let mods = store.resolve_mods(&mod_specs, update).await?;

    let to_integrate = mod_specs
        .iter()
        .map(|u| mods[u].clone())
        .collect::<Vec<_>>();
    let res_map: HashMap<ModResolution, ModSpecification> = mods
        .iter()
        .map(|(spec, info)| (info.resolution.clone(), spec.clone()))
        .collect();
    let urls = to_integrate
        .iter()
        .map(|m| &m.resolution)
        .collect::<Vec<&ModResolution>>();

    let (tx, mut rx) = mpsc::channel::<FetchProgress>(10);

    tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if let Some(spec) = res_map.get(progress.resolution()) {
                message_tx
                    .send(Message::FetchModProgress(FetchModProgress {
                        rid,
                        spec: spec.clone(),
                        progress: progress.into(),
                    }))
                    .await
                    .unwrap();
                ctx.request_repaint();
            }
        }
    });

    Ok(store.fetch_mods_ordered(&urls, update, Some(tx)).await?)
}

#[derive(Debug)]
pub struct FetchRemotePresets {
    rid: RequestID,
    /// asked for by the user: report the outcome
    user: bool,
    result: Result<RemotePresets, String>,
}

impl FetchRemotePresets {
    pub fn send(app: &mut App, ctx: &egui::Context, user: bool) {
        let rid = app.request_counter.next();
        let tx = app.tx.clone();
        let ctx = ctx.clone();
        let server = app.state.config.preset_server().to_string();
        let cached = app.remote_presets.clone();
        let handle = tokio::spawn(async move {
            let result = remote::fetch(&server, &cached).await;
            tx.send(Message::FetchRemotePresets(Self { rid, user, result }))
                .await
                .unwrap();
            ctx.request_repaint();
        });
        app.remote_rid = Some(MessageHandle {
            rid,
            handle,
            state: (),
        });
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) != app.remote_rid.as_ref().map(|r| r.rid) {
            return;
        }
        app.remote_rid = None;
        app.server_ok = Some(self.result.is_ok());
        match self.result {
            Ok(presets) => {
                if let Err(e) = presets.save(&app.remote_presets_path()) {
                    warn!("saving remote presets: {e}");
                }
                if self.user {
                    app.last_action = Some(LastAction::success(format!(
                        "{} remote presets",
                        presets.presets.len()
                    )));
                }
                app.remote_presets = presets;
                // an unpublished preset can't stay selected
                if app.selected_remote().is_none() && app.state.config.remote_preset.is_some() {
                    app.state.config.remote_preset = None;
                    app.state.config.save().unwrap();
                }
            }
            Err(e) => {
                warn!("fetching remote presets: {e}");
                if self.user {
                    app.last_action = Some(LastAction::failure(format!("remote presets: {e}")));
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct PublishPreset {
    rid: RequestID,
    skipped: usize,
    result: Result<Preset, String>,
}

impl PublishPreset {
    pub fn send(app: &mut App, ctx: &egui::Context, profile: &str) {
        let Some(key) = app.state.config.publish_key.clone() else {
            return;
        };
        let (upload, skipped) = remote::upload(&app.state.mod_data, profile, &app.state.store);
        let rid = app.request_counter.next();
        let tx = app.tx.clone();
        let ctx = ctx.clone();
        let server = app.state.config.preset_server().to_string();
        let handle = tokio::spawn(async move {
            let result = remote::publish(&server, &key, &upload).await;
            tx.send(Message::PublishPreset(Self {
                rid,
                skipped,
                result,
            }))
            .await
            .unwrap();
            ctx.request_repaint();
        });
        app.last_action = None;
        app.publish_rid = Some(MessageHandle {
            rid,
            handle,
            state: (),
        });
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) != app.publish_rid.as_ref().map(|r| r.rid) {
            return;
        }
        app.publish_rid = None;
        match self.result {
            Ok(preset) => {
                let mut msg = format!(
                    "published \"{}\" (revision {})",
                    preset.name, preset.revision
                );
                if self.skipped > 0 {
                    msg += &format!(", left out {} mods not on mod.io", self.skipped);
                }
                info!("{msg}");
                app.last_action = Some(LastAction::success(msg));
                let server = app.state.config.preset_server().to_string();
                app.remote_presets.upsert(&server, preset);
                if let Err(e) = app.remote_presets.save(&app.remote_presets_path()) {
                    warn!("saving remote presets: {e}");
                }
            }
            Err(e) => {
                error!("publishing: {e}");
                app.last_action = Some(LastAction::failure(format!("publishing: {e}")));
            }
        }
    }
}

#[derive(Debug)]
pub struct UnpublishPreset {
    rid: RequestID,
    slug: String,
    result: Result<(), String>,
}

impl UnpublishPreset {
    pub fn send(app: &mut App, ctx: &egui::Context, slug: String) {
        let Some(key) = app.state.config.publish_key.clone() else {
            return;
        };
        let rid = app.request_counter.next();
        let tx = app.tx.clone();
        let ctx = ctx.clone();
        let server = app.state.config.preset_server().to_string();
        let handle = tokio::spawn(async move {
            let result = remote::unpublish(&server, &key, &slug).await;
            tx.send(Message::UnpublishPreset(Self { rid, slug, result }))
                .await
                .unwrap();
            ctx.request_repaint();
        });
        app.last_action = None;
        app.publish_rid = Some(MessageHandle {
            rid,
            handle,
            state: (),
        });
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) != app.publish_rid.as_ref().map(|r| r.rid) {
            return;
        }
        app.publish_rid = None;
        match self.result {
            Ok(()) => {
                app.remote_presets.presets.retain(|p| p.slug != self.slug);
                if let Err(e) = app.remote_presets.save(&app.remote_presets_path()) {
                    warn!("saving remote presets: {e}");
                }
                if app.state.config.remote_preset.as_deref() == Some(self.slug.as_str()) {
                    app.state.config.remote_preset = None;
                    app.state.config.save().unwrap();
                }
                app.last_action = Some(LastAction::success(format!("unpublished {}", self.slug)));
            }
            Err(e) => {
                error!("unpublishing: {e}");
                app.last_action = Some(LastAction::failure(format!("unpublishing: {e}")));
            }
        }
    }
}

/// How a self-update went.
#[derive(Debug)]
enum UpdateOutcome {
    UpToDate,
    /// offline, server down, …: not worth bothering anyone
    CheckFailed(String),
    /// installing this release failed before
    Skipped(u32),
    Installed(u32, PathBuf),
    Failed(u32, String),
}

#[derive(Debug)]
pub struct Update {
    rid: RequestID,
    outcome: UpdateOutcome,
}

impl Update {
    /// Checks for a newer release and installs it. Does nothing in development builds.
    pub fn send(app: &mut App, ctx: &egui::Context) {
        if update::RELEASE.is_none() {
            return;
        }
        let rid = app.request_counter.next();
        let tx = app.tx.clone();
        let ctx = ctx.clone();
        let server = app.state.config.preset_server().to_string();
        let cache_dir = app.state.dirs.cache_dir.clone();
        let handle = tokio::spawn(async move {
            let outcome = match update::check(&server).await {
                Ok(None) => UpdateOutcome::UpToDate,
                Err(e) => UpdateOutcome::CheckFailed(e),
                Ok(Some(m)) if update::attempted_before(&cache_dir, m.release) => {
                    UpdateOutcome::Skipped(m.release)
                }
                Ok(Some(m)) => {
                    let (progress_tx, mut progress_rx) = mpsc::channel(4);
                    let forward = {
                        let (tx, ctx, release) = (tx.clone(), ctx.clone(), m.release);
                        tokio::spawn(async move {
                            while let Some((done, total)) = progress_rx.recv().await {
                                let state = UpdateState {
                                    release,
                                    done,
                                    total,
                                };
                                let msg = UpdateProgress { rid, state };
                                if tx.send(Message::UpdateProgress(msg)).await.is_err() {
                                    break;
                                }
                                ctx.request_repaint();
                            }
                        })
                    };
                    let _ = tx
                        .send(Message::UpdateProgress(UpdateProgress {
                            rid,
                            state: UpdateState {
                                release: m.release,
                                done: 0,
                                total: 0,
                            },
                        }))
                        .await;
                    let result = update::install(&server, &m, &cache_dir, progress_tx).await;
                    let _ = forward.await;
                    match result {
                        Ok(exe) => UpdateOutcome::Installed(m.release, exe),
                        Err(e) => UpdateOutcome::Failed(m.release, e),
                    }
                }
            };
            let _ = tx.send(Message::Update(Update { rid, outcome })).await;
            ctx.request_repaint();
        });
        app.self_update = Some(MessageHandle {
            rid,
            handle,
            state: None,
        });
    }

    fn receive(self, app: &mut App) {
        if Some(self.rid) != app.self_update.as_ref().map(|r| r.rid) {
            return;
        }
        app.self_update = None;
        match self.outcome {
            UpdateOutcome::UpToDate => {}
            UpdateOutcome::CheckFailed(e) => warn!("checking for updates: {e}"),
            UpdateOutcome::Skipped(n) => {
                warn!("not installing release {n} again: the last attempt didn't take");
                app.last_action = Some(LastAction::failure(format!(
                    "updating to release {n} failed earlier; get mint again from the installer"
                )));
            }
            UpdateOutcome::Installed(n, exe) => {
                info!("updated to release {n}, restarting");
                app.original_exe_path = Some(exe);
                app.needs_restart = true;
            }
            UpdateOutcome::Failed(n, e) => {
                error!("updating to release {n}: {e}");
                app.last_action = Some(LastAction::failure(format!(
                    "updating to release {n} failed: {e}"
                )));
            }
        }
    }
}

#[derive(Debug)]
pub struct UpdateProgress {
    rid: RequestID,
    state: UpdateState,
}

impl UpdateProgress {
    fn receive(self, app: &mut App) {
        if let Some(handle) = &mut app.self_update
            && handle.rid == self.rid
        {
            handle.state = Some(self.state);
        }
    }
}
