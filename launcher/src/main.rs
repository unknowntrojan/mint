//! Steam's launch options start this instead of mint: `"<mint dir>\mint-launch.exe" %command%`.
//! It starts mint (`mint launch %command%`), which gets the mods ready and starts the game. If mint
//! can't start, is missing (an antivirus quarantine, usually) or crashes, it asks whether to start
//! the game anyway: a broken mint must never keep anyone from playing.
//!
//! Deliberately tiny and dull: no network, no file writes, nothing for an antivirus to object to.
//! mint carries this binary and puts it next to itself (see `src/setup`).

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::ffi::OsString;
use std::io;
use std::process::{Command, ExitCode};

const MINT: &str = if cfg!(windows) { "mint.exe" } else { "mint" };

fn main() -> ExitCode {
    let game: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some((program, args)) = game.split_first() else {
        notify(
            "mint-launch starts the game through mint. Steam runs it; it does nothing on its own.",
        );
        return ExitCode::FAILURE;
    };
    let problem = match std::env::current_exe() {
        Ok(exe) => match Command::new(exe.with_file_name(MINT))
            .arg("launch")
            .args(&game)
            .status()
        {
            Ok(status) if status.success() => return ExitCode::SUCCESS,
            Ok(status) => format!("mint stopped unexpectedly ({status})."),
            Err(e) => why(&e),
        },
        Err(e) => why(&e),
    };
    if !ask(&format!(
        "{problem}\n\nStart Deep Rock Galactic anyway? Mods that were installed before stay as they are."
    )) {
        return ExitCode::FAILURE;
    }
    match Command::new(program).args(args).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn why(e: &io::Error) -> String {
    match e.raw_os_error() {
        // ERROR_VIRUS_INFECTED, ERROR_VIRUS_DELETED
        Some(225 | 226) => {
            "Windows blocked mint as a virus. That's a false alarm, but mint can't run until it's \
             allowed again (Windows Security, Protection history)."
                .into()
        }
        _ if e.kind() == io::ErrorKind::NotFound => {
            "mint is missing. Usually an antivirus removed it; running mint-setup again brings it back."
                .into()
        }
        _ => format!("mint couldn't start: {e}."),
    }
}

#[cfg(windows)]
fn message_box(text: &str, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide("mint"));
    // SAFETY: NUL-terminated strings that outlive the call, no owner window
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            flags | MB_SETFOREGROUND | MB_TOPMOST,
        )
    }
}

#[cfg(windows)]
fn ask(text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONWARNING, MB_YESNO};
    message_box(text, MB_YESNO | MB_ICONWARNING) == IDYES
}

#[cfg(windows)]
fn notify(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK};
    message_box(text, MB_OK | MB_ICONINFORMATION);
}

// Linux starts mint directly from Steam; this only keeps the workspace building there.
#[cfg(not(windows))]
fn ask(text: &str) -> bool {
    eprintln!("{text}\nstarting it anyway");
    true
}

#[cfg(not(windows))]
fn notify(text: &str) {
    eprintln!("{text}");
}
