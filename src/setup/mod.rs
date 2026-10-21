//! Friendpack setup. `mint-setup` is a release binary with a zip appended, the payload: a
//! friend's config (with the mod.io token), profiles, mod metadata and Oodle; see
//! friendpack/pack.sh. Started without arguments, it installs mint for the current user, imports
//! the payload and points DRG's Steam launch options at mint.
//!
//! Once that's done (the marker file in mint's config directory), running it again never touches
//! config, profiles, the mod cache or shortcuts: it replaces the installed mint only if it's a
//! newer release itself, and starts the installed mint.

pub mod gui;
pub mod vdf;

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs, UserDirs};
use mint_lib::{DRGInstallation, DRGInstallationType};
use tracing::{info, warn};

/// Ends a setup binary: `[binary][payload zip][zip length, u64 LE][MAGIC]`.
pub const MAGIC: &[u8; 8] = b"MINTPACK";

pub const EXE: &str = if cfg!(windows) { "mint.exe" } else { "mint" };
pub const OODLE: &str = if cfg!(windows) {
    "oo2core_9_win64.dll"
} else {
    "liboo2corelinux64.so.9"
};
/// In the config directory once a friendpack is imported; install.ps1 and install.sh write it too.
const MARKER: &str = "friendpack-installed";

/// What Steam starts on Windows, next to mint: it starts mint, or the game anyway if mint can't
/// (see launcher/). Linux starts mint directly.
pub const LAUNCHER: &str = "mint-launch.exe";

#[cfg(windows)]
const LAUNCHER_BIN: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_MINT_LAUNCH_mint-launch"));

/// Puts the launcher this mint carries into `dir` unless it's there already. A running launcher
/// (it started this mint) can't be overwritten but can be renamed, so an outdated one is moved
/// to `*.old` first. Does nothing outside Windows.
pub fn ensure_launcher(dir: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let path = dir.join(LAUNCHER);
        let old = dir.join(format!("{LAUNCHER}.old"));
        let _ = fs::remove_file(&old);
        if fs::read(&path).is_ok_and(|b| b == LAUNCHER_BIN) {
            return Ok(());
        }
        // the new one is complete before the old one goes: Steam can't start DRG without one
        let tmp = dir.join(format!(".{LAUNCHER}.new"));
        fs::write(&tmp, LAUNCHER_BIN)?;
        if fs::rename(&tmp, &path).is_err() {
            // a running launcher can't be replaced, but it can be renamed
            fs::rename(&path, &old)?;
            if let Err(e) = fs::rename(&tmp, &path) {
                let _ = fs::rename(&old, &path);
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        }
        info!("wrote {}", path.display());
    }
    #[cfg(not(windows))]
    let _ = dir;
    Ok(())
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Payload {
    /// The setup binary.
    exe: PathBuf,
    /// Where the payload starts in it: the plain mint's size.
    exe_len: u64,
    files: HashMap<String, Vec<u8>>,
}

impl Payload {
    /// The payload appended to the running binary, if there is one.
    pub fn of_current_exe() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Self::read(&exe).unwrap_or_else(|e| {
            warn!("reading the setup payload of {}: {e}", exe.display());
            None
        })
    }

    pub fn read(exe: &Path) -> io::Result<Option<Self>> {
        let mut f = fs::File::open(exe)?;
        let len = f.metadata()?.len();
        if len < 16 {
            return Ok(None);
        }
        f.seek(SeekFrom::End(-16))?;
        let mut trailer = [0; 16];
        f.read_exact(&mut trailer)?;
        if &trailer[8..] != MAGIC {
            return Ok(None);
        }
        let zip_len = u64::from_le_bytes(trailer[..8].try_into().unwrap());
        let exe_len = zip_len
            .checked_add(16)
            .and_then(|n| len.checked_sub(n))
            .ok_or_else(|| io::Error::other("payload longer than the file"))?;
        f.seek(SeekFrom::Start(exe_len))?;
        let mut zip = vec![0; zip_len as usize];
        f.read_exact(&mut zip)?;

        let mut archive = zip::ZipArchive::new(io::Cursor::new(zip)).map_err(io::Error::other)?;
        let mut files = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(io::Error::other)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(io::Error::other)?.into_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            files.insert(name, data);
        }
        Ok(Some(Self {
            exe: exe.to_path_buf(),
            exe_len,
            files,
        }))
    }

    fn file(&self, name: &str) -> io::Result<&[u8]> {
        self.files
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| io::Error::other(format!("{name} is missing from the setup file")))
    }
}

/// Where setup puts things for the current user.
pub struct Paths {
    /// mint and Oodle; the same place install.ps1 and install.sh use.
    pub install_dir: PathBuf,
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// drg-mod-integration directories, which mint would use instead of its own if they existed.
    pub legacy_dirs: Vec<PathBuf>,
    /// Start Menu and Desktop on Windows, the applications directory on Linux.
    pub shortcut_dirs: Vec<PathBuf>,
    /// Every Steam user's localconfig.vdf.
    pub steam_configs: Vec<PathBuf>,
    /// The same for a Flatpak or Snap Steam. Its sandbox can't see mint's files, so it can't start
    /// mint, and its launch options only get the game options.
    pub sandboxed_steam_configs: Vec<PathBuf>,
}

impl Paths {
    pub fn for_user() -> Option<Self> {
        let base = BaseDirs::new()?;
        let mint = ProjectDirs::from("", "", "mint")?;
        let legacy = ProjectDirs::from("", "", "drg-mod-integration")?;
        let shortcut_dirs = if cfg!(windows) {
            // the roaming AppData on Windows
            let mut dirs = vec![
                base.config_dir()
                    .join(r"Microsoft\Windows\Start Menu\Programs"),
            ];
            dirs.extend(UserDirs::new().and_then(|u| u.desktop_dir().map(Path::to_path_buf)));
            dirs
        } else {
            vec![base.data_dir().join("applications")]
        };
        let (steam_configs, sandboxed_steam_configs) = steam_configs();
        Some(Self {
            install_dir: if cfg!(windows) {
                base.data_local_dir()
            } else {
                base.data_dir()
            }
            .join("mint"),
            config_dir: mint.config_dir().to_path_buf(),
            cache_dir: mint.cache_dir().to_path_buf(),
            legacy_dirs: vec![
                legacy.config_dir().to_path_buf(),
                legacy.cache_dir().to_path_buf(),
                legacy.data_dir().to_path_buf(),
            ],
            shortcut_dirs,
            steam_configs,
            sandboxed_steam_configs,
        })
    }

    pub fn exe(&self) -> PathBuf {
        self.install_dir.join(EXE)
    }

    /// What starts mint from DRG's launch options, in front of `%command%`.
    fn wrapper(&self) -> String {
        if cfg!(windows) {
            format!("\"{}\"", self.install_dir.join(LAUNCHER).display())
        } else {
            format!("\"{}\" launch", self.exe().display())
        }
    }

    fn marker(&self) -> PathBuf {
        self.config_dir.join(MARKER)
    }

    /// The marker's settings. Markers from install.ps1/install.sh are empty files.
    fn marker_settings(&self) -> serde_json::Value {
        fs::read(self.marker())
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}))
    }

    /// Whether a friendpack was imported here before.
    pub fn set_up_before(&self) -> bool {
        self.marker().exists()
    }

    /// Whether DRG's launch options should go through mint: not if Steam was skipped during
    /// setup. Markers from install.ps1/install.sh predate the choice.
    pub fn steam_launch_wanted(&self) -> bool {
        self.marker_settings()["steam_launch"]
            .as_bool()
            .unwrap_or(true)
    }

    /// "No, and stop asking" when mint offered to put itself back into the launch options.
    /// Running setup again still puts it back.
    pub fn stop_steam_prompt(&self) -> io::Result<()> {
        let mut settings = self.marker_settings();
        settings["steam_prompt"] = false.into();
        fs::write(self.marker(), serde_json::to_vec_pretty(&settings)?)
    }

    /// Whether mint should offer to put itself back into DRG's launch options: setup put it there
    /// for `exe` (the running mint), DRG is the Steam version, and a Steam user's options don't
    /// start mint (anymore). Not after Skip in setup, or "No, and stop asking".
    pub fn launch_options_missing(&self, exe: &Path) -> bool {
        if !self.set_up_before()
            || !self.steam_launch_wanted()
            || self.marker_settings()["steam_prompt"].as_bool() == Some(false)
        {
            return false;
        }
        let same = matches!(
            (fs::canonicalize(exe), fs::canonicalize(self.exe())),
            (Ok(a), Ok(b)) if a == b
        );
        if !same || !configured_pak(self).is_some_and(|p| check_pak(&p).is_ok_and(|xbox| !xbox)) {
            return false;
        }
        let wanted = format!("{} %command%", self.wrapper());
        self.steam_configs.iter().any(|config| {
            match fs::read_to_string(config)
                .map_err(|e| e.to_string())
                .and_then(|text| vdf::drg_launch_options(&text))
            {
                // a Steam user without DRG settings never played it here
                Ok(options) => options.is_some_and(|o| !o.contains(&wanted)),
                Err(e) => {
                    warn!("{}: {e}", config.display());
                    false
                }
            }
        })
    }

    /// Whether there's a mint setup that importing would move aside.
    pub fn previous_setup(&self) -> bool {
        !self.set_up_before()
            && (self.config_dir.join("config.json").exists()
                || self.legacy_dirs.iter().any(|d| d.exists()))
    }
}

/// Every Steam user's localconfig.vdf, of every Steam here: the ones that can start mint, and the
/// Flatpak and Snap ones, which can't.
fn steam_configs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut open, mut sandboxed) = (Vec::new(), Vec::new());
    // the same Steam can be found under several names (~/.steam/steam is a link)
    let mut seen = std::collections::HashSet::new();
    for steam in steamlocate::locate_all().unwrap_or_default() {
        let Ok(users) = fs::read_dir(steam.path().join("userdata")) else {
            continue;
        };
        let dir = steam.path().to_string_lossy();
        let into =
            if dir.contains("/.var/app/com.valvesoftware.Steam/") || dir.contains("/snap/steam/") {
                &mut sandboxed
            } else {
                &mut open
            };
        into.extend(
            users
                .flatten()
                .filter(|u| {
                    u.file_name()
                        .to_str()
                        .is_some_and(|n| n.parse::<u64>().is_ok_and(|id| id > 0))
                })
                .map(|u| u.path().join("config").join("localconfig.vdf"))
                .filter(|p| p.is_file() && seen.insert(fs::canonicalize(p).unwrap_or(p.clone()))),
        );
    }
    open.sort();
    sandboxed.sort();
    (open, sandboxed)
}

/// The Steam DRG installation, if there is one.
pub fn find_drg() -> Option<PathBuf> {
    DRGInstallation::find().map(|i| i.main_pak())
}

/// Whether `pak` is DRG's main pak, and the Xbox / Game Pass one.
pub fn check_pak(pak: &Path) -> Result<bool> {
    let installation = DRGInstallation::from_pak_path(pak)?;
    anyhow::ensure!(pak.is_file(), "{} doesn't exist", pak.display());
    Ok(matches!(
        installation.installation_type,
        DRGInstallationType::Xbox
    ))
}

/// Whether Steam runs. It rewrites its config on exit, so launch options can't change meanwhile.
pub fn steam_running() -> bool {
    process_running(if cfg!(windows) { "steam.exe" } else { "steam" })
}

/// Whether mint runs. It saves its config when it exits, over anything imported meanwhile.
pub fn mint_running() -> bool {
    process_running(EXE)
}

#[cfg(windows)]
fn process_running(name: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: a process snapshot, walked with a correctly sized entry and closed again
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(name) {
                found = true;
                break;
            }
            ok = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
fn process_running(name: &str) -> bool {
    fs::read_dir("/proc").is_ok_and(|procs| {
        procs
            .flatten()
            .any(|p| fs::read_to_string(p.path().join("comm")).is_ok_and(|c| c.trim_end() == name))
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `path` renamed to `<name>.bak-<unix time>`.
fn move_aside(path: &Path, ts: u64) -> io::Result<PathBuf> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".bak-{ts}"));
    let aside = path.with_file_name(name);
    fs::rename(path, &aside)?;
    Ok(aside)
}

/// The release the mint at `exe` is, asked by running it. `None` for builds that aren't
/// releases or don't know `--release-number` yet (release 2 and older).
pub fn installed_release(exe: &Path) -> Option<u32> {
    let mut cmd = Command::new(exe);
    cmd.arg("--release-number");
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Installs this mint, without the payload, unless the installed one is the same release or
/// newer: setup files stay around for months while mint updates itself. Returns what it did.
pub fn install_exe(payload: &Payload, paths: &Paths) -> io::Result<Option<String>> {
    let target = paths.exe();
    let ours = crate::update::RELEASE;
    if target.exists()
        && let (Some(installed), Some(ours)) = (installed_release(&target), ours)
        && ours <= installed
    {
        info!("release {installed} is installed already, this is {ours}");
        return Ok(None);
    }

    fs::create_dir_all(&paths.install_dir)?;
    let tmp = paths.install_dir.join(format!(".{EXE}.new"));
    let copied = (|| {
        let mut src = fs::File::open(&payload.exe)?.take(payload.exe_len);
        let mut dst = fs::File::create(&tmp)?;
        io::copy(&mut src, &mut dst)?;
        dst.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
        }
        fs::rename(&tmp, &target)
    })();
    if let Err(e) = copied {
        let _ = fs::remove_file(&tmp);
        return Err(io::Error::new(
            e.kind(),
            format!(
                "installing {}: {e}. Is mint open? Close it and try again.",
                target.display()
            ),
        ));
    }
    let what = crate::update::describe();
    info!("installed {} ({what})", target.display());
    Ok(Some(format!("Installed mint ({what})")))
}

/// What has to sit next to mint whether or not mint itself was replaced: Oodle (some mods need it;
/// mint never downloads it) and, on Windows, the launcher Steam starts.
pub fn install_support(payload: &Payload, paths: &Paths) -> io::Result<()> {
    let oodle = paths.install_dir.join(OODLE);
    // a running mint has it loaded, so only when it's missing
    if !oodle.exists() {
        fs::write(&oodle, payload.file(OODLE)?)?;
    }
    ensure_launcher(&paths.install_dir)
}

/// Imports config, profiles and mod metadata, after moving any earlier mint setup aside. `pak`
/// is DRG's main pak, if known; `steam` whether DRG's launch options go through mint, which
/// later runs respect.
pub fn import(
    payload: &Payload,
    paths: &Paths,
    pak: Option<&Path>,
    steam: bool,
) -> io::Result<Vec<String>> {
    let ts = now();
    let mut done = Vec::new();
    for dir in paths
        .legacy_dirs
        .iter()
        .chain([&paths.config_dir, &paths.cache_dir])
    {
        if dir.exists() {
            let aside = move_aside(dir, ts)?;
            info!("moved {} to {}", dir.display(), aside.display());
            done.push(format!(
                "Kept your earlier mint setup as {}",
                aside.display()
            ));
        }
    }
    fs::create_dir_all(&paths.config_dir)?;
    fs::create_dir_all(&paths.cache_dir)?;

    let mut config: serde_json::Value = serde_json::from_slice(payload.file("config.json")?)?;
    let Some(fields) = config.as_object_mut() else {
        return Err(io::Error::other(
            "the setup file's config.json isn't an object",
        ));
    };
    fields.insert(
        "drg_pak_path".into(),
        pak.map(|p| p.to_string_lossy().into_owned()).into(),
    );
    write_private(
        &paths.config_dir.join("config.json"),
        &serde_json::to_vec_pretty(&config)?,
    )?;
    fs::write(
        paths.config_dir.join("mod_data.json"),
        payload.file("mod_data.json")?,
    )?;
    fs::write(
        paths.cache_dir.join("cache.json"),
        payload.file("cache.json")?,
    )?;
    fs::write(
        paths.marker(),
        serde_json::to_vec_pretty(&serde_json::json!({
            "release": crate::update::RELEASE,
            "installed": ts,
            "steam_launch": steam,
        }))?,
    )?;
    info!(
        "imported the friendpack into {}",
        paths.config_dir.display()
    );
    done.push("Imported the mod lists".into());
    Ok(done)
}

/// The config holds the mod.io token.
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(data)
}

/// Start Menu and Desktop shortcuts on Windows, an applications entry on Linux.
pub fn add_shortcuts(paths: &Paths) -> Vec<String> {
    let mut done = Vec::new();
    for dir in &paths.shortcut_dirs {
        match shortcut(dir, &paths.exe()) {
            Ok(()) => done.push(format!("Added a shortcut in {}", dir.display())),
            Err(e) => warn!("shortcut in {}: {e}", dir.display()),
        }
    }
    done
}

#[cfg(windows)]
fn shortcut(dir: &Path, exe: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    fs::create_dir_all(dir)?;
    // WScript.Shell is the simplest way to write a .lnk; paths go in through the environment
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:MINT_LNK); \
             $s.TargetPath = $env:MINT_EXE; $s.WorkingDirectory = $env:MINT_DIR; \
             $s.Description = 'Deep Rock Galactic mod integration'; $s.Save()",
        ])
        .env("MINT_LNK", dir.join("mint.lnk"))
        .env("MINT_EXE", exe)
        .env("MINT_DIR", exe.parent().unwrap_or(dir))
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("powershell: {status}")));
    }
    Ok(())
}

#[cfg(not(windows))]
fn shortcut(dir: &Path, exe: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(
        dir.join("mint.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=mint\nComment=Deep Rock Galactic mod \
             integration\nExec=\"{}\"\nTerminal=false\nCategories=Game;\n",
            exe.display()
        ),
    )
}

/// What changing DRG's Steam launch options did.
pub struct LaunchOptions {
    pub done: Vec<String>,
    /// Every Steam user starts DRG through mint now.
    pub through_mint: bool,
}

/// Puts mint in front of DRG in every Steam user's launch options; a Flatpak or Snap Steam gets
/// [`vdf::FLAGS`] instead. Keeps a backup of each changed localconfig.vdf. Steam must not run.
pub fn set_launch_options(paths: &Paths) -> LaunchOptions {
    let ts = now();
    let mut done = Vec::new();
    let mut ok = !paths.steam_configs.is_empty();
    if cfg!(windows) && !paths.install_dir.join(LAUNCHER).is_file() {
        // Steam would start a missing program, and DRG wouldn't start at all
        warn!("{LAUNCHER} is missing, not changing Steam's launch options");
        ok = false;
    } else {
        let wrapper = paths.wrapper();
        for config in &paths.steam_configs {
            match rewrite_launch_options(config, ts, |c| vdf::with_wrapper(c, &wrapper)) {
                Ok(true) => done.push("Deep Rock Galactic starts through mint now".into()),
                Ok(false) => {}
                Err(e) => {
                    warn!("{}: {e:#}", config.display());
                    ok = false;
                }
            }
        }
    }
    for config in &paths.sandboxed_steam_configs {
        match rewrite_launch_options(config, ts, vdf::with_flags) {
            Ok(true) => done.push(format!(
                "Added {} to DRG's launch options",
                vdf::FLAGS.join(" ")
            )),
            Ok(false) => {}
            Err(e) => warn!("{}: {e:#}", config.display()),
        }
    }
    done.dedup();
    LaunchOptions {
        done,
        through_mint: ok,
    }
}

/// Changes DRG's launch options in one localconfig.vdf to what `f` makes of them, keeping a
/// backup. Whether they changed.
fn rewrite_launch_options(config: &Path, ts: u64, f: impl FnOnce(&str) -> String) -> Result<bool> {
    let text = fs::read_to_string(config)?;
    let Some(new) = vdf::set_launch_options(&text, f).map_err(anyhow::Error::msg)? else {
        return Ok(false);
    };
    let mut backup = config.as_os_str().to_os_string();
    backup.push(format!(".bak-{ts}"));
    fs::copy(config, &backup)?;
    let tmp = config.with_extension("vdf.tmp-mint");
    fs::write(&tmp, new.as_bytes())?;
    fs::rename(&tmp, config)?;
    let options = vdf::launch_options(&new).map_err(anyhow::Error::msg)?;
    info!("{}: launch options now {options:?}", config.display());
    Ok(true)
}

/// Starts the installed mint.
pub fn start(paths: &Paths) -> io::Result<()> {
    Command::new(paths.exe())
        .current_dir(&paths.install_dir)
        .spawn()
        .map(drop)
}

/// The installed config's DRG pak, if set.
fn configured_pak(paths: &Paths) -> Option<PathBuf> {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.config_dir.join("config.json")).ok()?).ok()?;
    config["drg_pak_path"].as_str().map(PathBuf::from)
}

/// Runs a setup binary: the setup window the first time, afterwards just the installed mint.
pub fn run(payload: Payload) -> Result<()> {
    let paths = Paths::for_user().context("no home directory")?;
    fs::create_dir_all(&paths.install_dir)?;
    let _log = mint_lib::setup_logging(paths.install_dir.join("setup.log"), "mint")?;
    info!("mint setup, {}", crate::update::describe());

    if !paths.set_up_before() {
        return gui::run(payload, paths);
    }
    // set up already: bring the installed mint up to date, and start it
    if mint_running() {
        // replacing it, or a second one, would only fight over its config
        info!("mint is open already");
        rfd::MessageDialog::new()
            .set_title("mint")
            .set_description("mint is already open.")
            .show();
        return Ok(());
    }
    if let Err(e) = install_exe(&payload, &paths) {
        warn!("{e}");
    }
    if let Err(e) = install_support(&payload, &paths) {
        warn!("{e}");
    }
    let steam = configured_pak(&paths).is_some_and(|p| check_pak(&p).is_ok_and(|xbox| !xbox));
    if steam && paths.steam_launch_wanted() && !steam_running() {
        set_launch_options(&paths);
    }
    start(&paths).with_context(|| format!("starting {}", paths.exe().display()))
}

/// Shows an error when there's no window (or terminal) to show it in.
pub fn error_dialog(e: &anyhow::Error) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("mint")
        .set_description(format!("{e:#}"))
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, data) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// A fake mint with a payload appended, the way pack.sh builds it.
    fn setup_binary(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let zip = payload_zip(files);
        let mut bin = b"#!fake mint binary\n".to_vec();
        bin.extend(&zip);
        bin.extend((zip.len() as u64).to_le_bytes());
        bin.extend(MAGIC);
        let path = dir.join("mint-setup");
        fs::write(&path, bin).unwrap();
        path
    }

    fn paths(root: &Path) -> Paths {
        Paths {
            install_dir: root.join("local/mint"),
            config_dir: root.join("roaming/mint/config"),
            cache_dir: root.join("local/mint/cache"),
            legacy_dirs: vec![root.join("roaming/drg-mod-integration/config")],
            shortcut_dirs: vec![],
            steam_configs: vec![],
            sandboxed_steam_configs: vec![],
        }
    }

    /// A localconfig.vdf whose user has DRG settings with these launch options.
    fn localconfig(path: &Path, options: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let options = options.replace('\\', "\\\\").replace('"', "\\\"");
        fs::write(
            path,
            format!(
                "\"UserLocalConfigStore\"\n{{\n\t\"Software\"\n\t{{\n\t\t\"Valve\"\n\t\t{{\n\t\t\t\"Steam\"\n\t\t\t{{\n\t\t\t\t\"apps\"\n\t\t\t\t{{\n\t\t\t\t\t\"548430\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"LaunchOptions\"\t\t\"{options}\"\n\t\t\t\t\t}}\n\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t}}\n}}\n"
            ),
        )
        .unwrap();
    }

    const FILES: &[(&str, &[u8])] = &[
        (
            "config.json",
            br#"{"version":"0.0.0","drg_pak_path":null,"provider_parameters":{"modio":{"oauth":"t"}}}"#,
        ),
        ("mod_data.json", br#"{"version":"0.1.0"}"#),
        ("cache.json", b"{}"),
        (OODLE, b"oodle"),
    ];

    #[test]
    fn payload_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let bin = setup_binary(dir.path(), FILES);
        let p = Payload::read(&bin).unwrap().unwrap();
        assert_eq!(p.exe_len, b"#!fake mint binary\n".len() as u64);
        assert_eq!(p.file(OODLE).unwrap(), b"oodle");
        // a plain binary has none
        fs::write(dir.path().join("plain"), b"just a binary, long enough").unwrap();
        assert!(Payload::read(&dir.path().join("plain")).unwrap().is_none());
    }

    #[test]
    fn first_import_moves_earlier_setups_aside_and_marks_itself() {
        let dir = tempfile::tempdir().unwrap();
        let p = Payload::read(&setup_binary(dir.path(), FILES))
            .unwrap()
            .unwrap();
        let paths = paths(dir.path());
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.config_dir.join("config.json"), "upstream mint").unwrap();
        fs::create_dir_all(&paths.legacy_dirs[0]).unwrap();
        assert!(paths.previous_setup() && !paths.set_up_before());

        let pak = dir.path().join("FSD-WindowsNoEditor.pak");
        let done = import(&p, &paths, Some(&pak), true).unwrap();
        assert_eq!(done.len(), 3, "{done:?}");
        assert!(paths.set_up_before() && !paths.previous_setup());
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(paths.config_dir.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(config["drg_pak_path"], pak.to_string_lossy().as_ref());
        assert_eq!(config["provider_parameters"]["modio"]["oauth"], "t");
        // the earlier ones are kept
        let kept: Vec<_> = fs::read_dir(paths.config_dir.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .filter(|n| n.starts_with("config.bak-"))
            .collect();
        assert_eq!(kept.len(), 1);
        let upstream = paths
            .config_dir
            .with_file_name(&kept[0])
            .join("config.json");
        assert_eq!(fs::read_to_string(upstream).unwrap(), "upstream mint");
    }

    #[test]
    fn exe_is_installed_without_the_payload() {
        let dir = tempfile::tempdir().unwrap();
        let p = Payload::read(&setup_binary(dir.path(), FILES))
            .unwrap()
            .unwrap();
        let paths = paths(dir.path());
        assert!(install_exe(&p, &paths).unwrap().is_some());
        assert_eq!(fs::read(paths.exe()).unwrap(), b"#!fake mint binary\n");
        assert!(Payload::read(&paths.exe()).unwrap().is_none());
        // Oodle (and on Windows the launcher) also when mint itself wasn't replaced
        assert!(!paths.install_dir.join(OODLE).exists());
        install_support(&p, &paths).unwrap();
        assert_eq!(fs::read(paths.install_dir.join(OODLE)).unwrap(), b"oodle");
    }

    #[test]
    fn skipping_steam_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let p = Payload::read(&setup_binary(dir.path(), FILES))
            .unwrap()
            .unwrap();
        let paths = paths(dir.path());
        import(&p, &paths, None, false).unwrap();
        assert!(!paths.steam_launch_wanted());
        import(&p, &paths, None, true).unwrap();
        assert!(paths.steam_launch_wanted());
        // install.ps1 and install.sh leave an empty marker
        fs::write(paths.marker(), b"").unwrap();
        assert!(paths.steam_launch_wanted());
    }

    #[test]
    fn a_corrupt_trailer_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut bin = b"#!fake mint binary\n".to_vec();
        bin.extend(u64::MAX.to_le_bytes());
        bin.extend(MAGIC);
        let path = dir.path().join("broken");
        fs::write(&path, bin).unwrap();
        assert!(Payload::read(&path).is_err());
    }

    /// Set up with Steam, mint installed and DRG at a pak, the way setup leaves it.
    fn set_up(root: &Path, steam: bool) -> Paths {
        let p = Payload::read(&setup_binary(root, FILES)).unwrap().unwrap();
        let mut paths = paths(root);
        paths.steam_configs = vec![root.join("steam/userdata/1/config/localconfig.vdf")];
        let pak = root.join("DRG/FSD/Content/Paks/FSD-WindowsNoEditor.pak");
        fs::create_dir_all(pak.parent().unwrap()).unwrap();
        fs::write(&pak, b"pak").unwrap();
        fs::create_dir_all(&paths.install_dir).unwrap();
        fs::write(paths.exe(), b"mint").unwrap();
        fs::write(paths.install_dir.join(LAUNCHER), b"mint-launch").unwrap();
        import(&p, &paths, Some(&pak), steam).unwrap();
        paths
    }

    #[test]
    fn mint_missing_from_the_launch_options_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = set_up(dir.path(), true);
        let (config, exe) = (paths.steam_configs[0].clone(), paths.exe());
        localconfig(&config, &format!("{} %command% -dx11", paths.wrapper()));
        assert!(!paths.launch_options_missing(&exe));

        localconfig(&config, "-dx11");
        assert!(paths.launch_options_missing(&exe));
        // only the mint setup installed puts itself back
        let other = dir.path().join("elsewhere/mint");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, b"mint").unwrap();
        assert!(!paths.launch_options_missing(&other));
        // a Steam user who never played DRG here
        fs::write(&config, "\"UserLocalConfigStore\" { \"Software\" { } }").unwrap();
        assert!(!paths.launch_options_missing(&exe));

        localconfig(&config, "-dx11");
        paths.stop_steam_prompt().unwrap();
        assert!(!paths.launch_options_missing(&exe));
        // the rest of the marker stays
        assert!(paths.steam_launch_wanted());
        assert!(paths.marker_settings()["installed"].is_u64());

        // Skip in setup
        let dir = tempfile::tempdir().unwrap();
        let paths = set_up(dir.path(), false);
        localconfig(&paths.steam_configs[0], "-dx11");
        assert!(!paths.launch_options_missing(&paths.exe()));

        // install.ps1 and install.sh: an empty marker, launch options with the flags only
        let dir = tempfile::tempdir().unwrap();
        let paths = set_up(dir.path(), true);
        fs::write(paths.marker(), b"").unwrap();
        localconfig(&paths.steam_configs[0], "-disablemodding -nosplash");
        assert!(paths.launch_options_missing(&paths.exe()));
        paths.stop_steam_prompt().unwrap();
        assert!(!paths.launch_options_missing(&paths.exe()));
    }

    #[test]
    fn sandboxed_steam_only_gets_the_game_options() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = set_up(dir.path(), true);
        let flatpak = dir.path().join("flatpak/userdata/2/config/localconfig.vdf");
        paths.sandboxed_steam_configs = vec![flatpak.clone()];
        localconfig(&paths.steam_configs[0], "-dx11");
        localconfig(&flatpak, "-dx11");
        let opts = set_launch_options(&paths);
        assert!(opts.through_mint);
        let read = |p: &Path| vdf::launch_options(&fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(
            read(&paths.steam_configs[0]),
            format!("{} %command% -dx11", paths.wrapper())
        );
        assert_eq!(read(&flatpak), "-dx11 -disablemodding -nosplash");
        assert!(!paths.launch_options_missing(&paths.exe()));

        // only a Flatpak Steam: DRG doesn't start through mint
        paths.steam_configs.clear();
        assert!(!set_launch_options(&paths).through_mint);
    }
}
