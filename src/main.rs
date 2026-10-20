// Release builds are GUI programs on Windows, so no console window opens next to mint; see
// attach_parent_console for the CLI.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use tracing::{debug, info};

use mint::mod_lints::{LintId, run_lints};
use mint::providers::ProviderFactory;
use mint::{
    Dirs, MintError, resolve_ordered_with_provider_init,
    resolve_unordered_and_integrate_with_provider_init,
};
use mint::{gui::gui, providers::ModSpecification, state::State};

/// Command line integration tool.
#[derive(Parser, Debug)]
struct ActionIntegrate {
    /// Path to FSD-WindowsNoEditor.pak (FSD-WinGDK.pak for Microsoft Store version) located
    /// inside the "Deep Rock Galactic" installation directory under FSD/Content/Paks. Only
    /// necessary if it cannot be found automatically.
    #[arg(short, long)]
    fsd_pak: Option<PathBuf>,

    /// Update mods. By default all mods and metadata are cached offline so this is necessary to
    /// check for updates.
    #[arg(short, long)]
    update: bool,

    /// Paths of mods to integrate
    ///
    /// Can be a file path or URL to a .pak or .zip file or a URL to a mod on https://mod.io/g/drg
    /// Examples:
    ///     ./local/path/test-mod.pak
    ///     https://mod.io/g/drg/m/custom-difficulty
    ///     https://example.org/some-online-mod-repository/public-mod.zip
    #[arg(short, long, num_args=0.., verbatim_doc_comment)]
    mods: Vec<String>,
}

/// Integrate a profile
#[derive(Parser, Debug)]
struct ActionIntegrateProfile {
    /// Path to FSD-WindowsNoEditor.pak (FSD-WinGDK.pak for Microsoft Store version) located
    /// inside the "Deep Rock Galactic" installation directory under FSD/Content/Paks. Only
    /// necessary if it cannot be found automatically.
    #[arg(short, long)]
    fsd_pak: Option<PathBuf>,

    /// Update mods. By default all mods and metadata are cached offline so this is necessary to
    /// check for updates.
    #[arg(short, long)]
    update: bool,

    /// Profile to integrate.
    profile: String,
}

/// Launch via steam
#[derive(Parser, Debug)]
struct ActionLaunch {
    /// The game's command line, Steam's `%command%` and DRG's options after it
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    args: Vec<String>,
}

/// Lint the mod bundle that would be created for a profile.
#[derive(Parser, Debug)]
struct ActionLint {
    /// Path to FSD-WindowsNoEditor.pak (FSD-WinGDK.pak for Microsoft Store version) located
    /// inside the "Deep Rock Galactic" installation directory under FSD/Content/Paks. Only
    /// necessary if it cannot be found automatically.
    #[arg(short, long)]
    fsd_pak: Option<PathBuf>,

    /// Profile to lint.
    profile: String,
}

#[derive(Subcommand, Debug)]
enum Action {
    Integrate(ActionIntegrate),
    Profile(ActionIntegrateProfile),
    Launch(ActionLaunch),
    Lint(ActionLint),
}

#[derive(Parser, Debug)]
#[command(author, version=mint_lib::built_info::GIT_VERSION.unwrap())]
struct Args {
    #[command(subcommand)]
    action: Option<Action>,

    /// Location to store configs and data
    #[arg(long)]
    appdata: Option<PathBuf>,

    /// Print the release this build is (`none` for others) and exit; setup asks installed mints.
    #[arg(long, hide = true)]
    release_number: bool,
}

/// Run from a terminal with a subcommand, a GUI-subsystem mint prints into that terminal. The
/// shell doesn't wait for it, though.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    if std::env::args_os().len() <= 1 {
        return;
    }
    let usable = |h: HANDLE| !h.is_null() && h != INVALID_HANDLE_VALUE;
    let open = |name: &str| {
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated name, no security attributes or template
        unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        }
    };
    // SAFETY: plain Win32 calls; the console handles stay open for the process's lifetime
    unsafe {
        // redirected into a file or pipe: that works as it is
        if usable(GetStdHandle(STD_OUTPUT_HANDLE)) || AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        // attaching doesn't give a GUI process standard handles
        let out = open("CONOUT$");
        if usable(out) {
            SetStdHandle(STD_OUTPUT_HANDLE, out);
            if !usable(GetStdHandle(STD_ERROR_HANDLE)) {
                SetStdHandle(STD_ERROR_HANDLE, out);
            }
        }
        if !usable(GetStdHandle(STD_INPUT_HANDLE)) {
            let input = open("CONIN$");
            if usable(input) {
                SetStdHandle(STD_INPUT_HANDLE, input);
            }
        }
    }
}

fn main() -> Result<()> {
    #[cfg(all(windows, not(debug_assertions)))]
    attach_parent_console();

    // a setup binary started like a program sets mint up, or starts the installed one
    if std::env::args_os().len() == 1
        && let Some(payload) = mint::setup::Payload::of_current_exe()
    {
        return mint::setup::run(payload).inspect_err(mint::setup::error_dialog);
    }

    #[cfg(target_os = "windows")]
    {
        // Try to enable ANSI code support on Windows 10 for console. If it fails, then whatever
        // *shrugs*.
        let _res = ansi_term::enable_ansi_support();
    }

    let args = Args::parse();
    if args.release_number {
        println!(
            "{}",
            mint::update::RELEASE.map_or("none".into(), |n| n.to_string())
        );
        return Ok(());
    }

    let dirs = args
        .appdata
        .as_ref()
        .map(Dirs::from_path)
        .unwrap_or_else(Dirs::default_xdg)?;

    unsafe { std::env::set_var("RUST_BACKTRACE", "1") };

    let _guard = mint_lib::setup_logging(dirs.data_dir.join("mint.log"), "mint")?;
    debug!("logging setup complete");

    info!("mint {}", mint::update::describe());
    info!("config dir = {}", dirs.config_dir.display());
    info!("cache dir = {}", dirs.cache_dir.display());
    info!("data dir = {}", dirs.data_dir.display());

    let rt = tokio::runtime::Runtime::new().expect("Unable to create Runtime");
    debug!("tokio runtime created");
    let _enter = rt.enter();

    debug!(?args);

    match args.action {
        Some(Action::Integrate(action)) => rt.block_on(async {
            action_integrate(dirs, action).await?;
            Ok(())
        }),
        Some(Action::Profile(action)) => rt.block_on(async {
            action_integrate_profile(dirs, action).await?;
            Ok(())
        }),
        Some(Action::Launch(action)) => {
            std::thread::spawn(move || {
                rt.block_on(std::future::pending::<()>());
            });
            gui(dirs, Some(action.args))?;
            Ok(())
        }
        Some(Action::Lint(action)) => rt.block_on(async {
            action_lint(dirs, action).await?;
            Ok(())
        }),
        None => {
            std::thread::spawn(move || {
                rt.block_on(std::future::pending::<()>());
            });
            gui(dirs, None)?;
            Ok(())
        }
    }
}

#[tracing::instrument(skip(state))]
fn init_provider(
    state: &mut State,
    url: String,
    factory: &ProviderFactory,
) -> Result<(), MintError> {
    info!("initializing provider for {:?}", url);

    let params = state
        .config
        .provider_parameters
        .entry(factory.id.to_owned())
        .or_default();
    for p in factory.parameters {
        if !params.contains_key(p.name) {
            // this blocks but since we're calling it on the main thread it'll be fine
            let value =
                dialoguer::Password::with_theme(&dialoguer::theme::ColorfulTheme::default())
                    .with_prompt(p.description)
                    .interact()
                    .unwrap();
            params.insert(p.id.to_owned(), value);
        }
    }
    Ok(state.store.add_provider(factory, params)?)
}

fn check_profile(state: &State, profile: &str) -> Result<()> {
    if state.mod_data.profiles.contains_key(profile) {
        return Ok(());
    }
    let names: Vec<_> = state
        .mod_data
        .profiles
        .keys()
        .map(|p| format!("{p:?}"))
        .collect();
    Err(anyhow!(
        "no profile named {profile:?}; there are {}",
        names.join(", ")
    ))
}

fn get_pak_path(state: &State, arg: &Option<PathBuf>) -> Result<PathBuf> {
    arg.as_ref()
        .or_else(|| state.config.drg_pak_path.as_ref())
        .cloned()
        .context("Could not find DRG pak file, please specify manually with the --fsd_pak flag")
}

async fn action_integrate(dirs: Dirs, action: ActionIntegrate) -> Result<()> {
    let mut state = State::init(dirs)?;
    let game_pak_path = get_pak_path(&state, &action.fsd_pak)?;
    debug!(?game_pak_path);

    let mod_specs = action
        .mods
        .into_iter()
        .map(ModSpecification::new)
        .collect::<Vec<_>>();

    resolve_unordered_and_integrate_with_provider_init(
        game_pak_path,
        &mut state,
        &mod_specs,
        action.update,
        init_provider,
    )
    .await
    .map_err(|e| anyhow!("{}", e))
}

async fn action_integrate_profile(dirs: Dirs, action: ActionIntegrateProfile) -> Result<()> {
    let mut state = State::init(dirs)?;
    check_profile(&state, &action.profile)?;
    let game_pak_path = get_pak_path(&state, &action.fsd_pak)?;
    debug!(?game_pak_path);

    let mut mods = Vec::new();
    state.mod_data.for_each_enabled_mod(&action.profile, |mc| {
        mods.push(mc.spec.clone());
    });

    resolve_unordered_and_integrate_with_provider_init(
        game_pak_path,
        &mut state,
        &mods,
        action.update,
        init_provider,
    )
    .await
    .map_err(|e| anyhow!("{}", e))
}

async fn action_lint(dirs: Dirs, action: ActionLint) -> Result<()> {
    let mut state = State::init(dirs)?;
    check_profile(&state, &action.profile)?;
    let game_pak_path = get_pak_path(&state, &action.fsd_pak)?;
    debug!(?game_pak_path);

    let mut mods = Vec::new();
    state.mod_data.for_each_mod(&action.profile, |mc| {
        mods.push(mc.spec.clone());
    });

    let mod_paths = resolve_ordered_with_provider_init(&mut state, &mods, init_provider).await?;

    let report = tokio::task::spawn_blocking(move || {
        run_lints(
            &BTreeSet::from([
                LintId::ARCHIVE_WITH_ONLY_NON_PAK_FILES,
                LintId::ASSET_REGISTRY_BIN,
                LintId::CONFLICTING,
                LintId::EMPTY_ARCHIVE,
                LintId::OUTDATED_PAK_VERSION,
                LintId::SHADER_FILES,
                LintId::ARCHIVE_WITH_MULTIPLE_PAKS,
                LintId::NON_ASSET_FILES,
                LintId::SPLIT_ASSET_PAIRS,
            ]),
            mods.into_iter().zip(mod_paths).collect(),
            Some(game_pak_path),
        )
    })
    .await??;
    println!("{report:#?}");
    Ok(())
}
