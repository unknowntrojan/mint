//! DRG's Steam launch options: `userdata/<id>/config/localconfig.vdf`, value
//! UserLocalConfigStore/Software/Valve/Steam/apps/548430/LaunchOptions. Only that value is
//! changed; every other byte stays as it is. Steam rewrites the file when it exits, so it must
//! not be running. A port of friendpack/payload/drg_launch_opts.py.

use std::sync::OnceLock;

const TARGET: [&str; 6] = [
    "userlocalconfigstore",
    "software",
    "valve",
    "steam",
    "apps",
    "548430",
];

/// Game options mint needs: DRG's own mod.io integration off, no splash screen. Started through
/// Steam, mint adds them to the game's command line itself.
pub const FLAGS: [&str; 2] = ["-disablemodding", "-nosplash"];

#[derive(Debug, Default)]
struct Located {
    /// Just past the `{` of .../apps.
    apps_open: Option<usize>,
    /// Just past the `{` of .../apps/548430.
    block_open: Option<usize>,
    /// Span of the quoted LaunchOptions value, and the value.
    value: Option<(usize, usize, String)>,
}

fn locate(text: &str) -> Result<Located, String> {
    let b = text.as_bytes();
    let n = b.len();
    let (mut i, mut stack, mut key) = (0, Vec::<String>::new(), None::<String>);
    let mut r = Located::default();
    let at = |stack: &[String], depth: usize| {
        stack.len() == depth && stack.iter().zip(TARGET).all(|(a, b)| a == b)
    };
    while i < n {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(n, |j| i + j);
            }
            b'{' => {
                stack.push(key.take().unwrap_or_default().to_lowercase());
                i += 1;
                if at(&stack, 5) && r.apps_open.is_none() {
                    r.apps_open = Some(i);
                } else if at(&stack, 6) && r.block_open.is_none() {
                    r.block_open = Some(i);
                }
            }
            b'}' => {
                if stack.pop().is_none() {
                    return Err(format!("unbalanced '}}' at offset {i}"));
                }
                key = None;
                i += 1;
            }
            b'"' => {
                let (start, mut j, mut s) = (i, i + 1, Vec::new());
                while j < n && b[j] != b'"' {
                    if b[j] == b'\\' && j + 1 < n {
                        j += 1;
                    }
                    s.push(b[j]);
                    j += 1;
                }
                if j >= n {
                    return Err(format!("unterminated string at offset {start}"));
                }
                i = j + 1;
                let s = String::from_utf8_lossy(&s).into_owned();
                match key.take() {
                    None => key = Some(s),
                    Some(k) => {
                        if at(&stack, 6)
                            && k.eq_ignore_ascii_case("launchoptions")
                            && r.value.is_none()
                        {
                            r.value = Some((start, i, s));
                        }
                    }
                }
            }
            c => return Err(format!("unexpected {:?} at offset {i}", c as char)),
        }
    }
    if !stack.is_empty() {
        return Err("unbalanced '{'".into());
    }
    Ok(r)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// DRG's launch options in a localconfig.vdf.
pub fn launch_options(text: &str) -> Result<String, String> {
    Ok(locate(text)?.value.map(|v| v.2).unwrap_or_default())
}

/// DRG's launch options, or `None` if this Steam user has no DRG settings at all (never played
/// it, or doesn't own it).
pub fn drg_launch_options(text: &str) -> Result<Option<String>, String> {
    let loc = locate(text)?;
    Ok(loc
        .block_open
        .map(|_| loc.value.map(|v| v.2).unwrap_or_default()))
}

/// Replaces DRG's launch options with what `f` makes of the current ones. Returns the new file,
/// or `None` if nothing changes.
pub fn set_launch_options(
    text: &str,
    f: impl FnOnce(&str) -> String,
) -> Result<Option<String>, String> {
    let loc = locate(text)?;
    let current = loc.value.as_ref().map_or("", |v| v.2.as_str());
    let new = f(current);
    if new == current {
        return Ok(None);
    }
    let (t5, t6) = ("\t".repeat(5), "\t".repeat(6));
    let out = if let Some((start, end, _)) = &loc.value {
        format!("{}\"{}\"{}", &text[..*start], escape(&new), &text[*end..])
    } else if let Some(at) = loc.block_open {
        let line = format!("\n{t6}\"LaunchOptions\"\t\t\"{}\"", escape(&new));
        format!("{}{line}{}", &text[..at], &text[at..])
    } else if let Some(at) = loc.apps_open {
        let block = format!(
            "\n{t5}\"548430\"\n{t5}{{\n{t6}\"LaunchOptions\"\t\t\"{}\"\n{t5}}}",
            escape(&new)
        );
        format!("{}{block}{}", &text[..at], &text[at..])
    } else {
        return Err("no Software/Valve/Steam/apps section".into());
    };
    locate(&out)?; // never write a file this broke
    Ok(Some(out))
}

/// `current` with `wrapper` (what starts mint: `"C:\…\mint-launch.exe"`, or `"/…/mint" launch`
/// on Linux) in front of `%command%`. Other options stay: game options, and wrappers or variables
/// before `%command%`. mint from an earlier install, wherever it was and however it was started,
/// is replaced. [`FLAGS`] aren't added (mint passes them to the game itself); ones already there
/// stay, like any other option.
pub fn with_wrapper(current: &str, wrapper: &str) -> String {
    let current = without_mint(current);
    // without %command%, Steam passes the options to the game
    let (before, after) = match command().find(&current) {
        Some(m) => (&current[..m.start()], &current[m.end()..]),
        None => ("", &*current),
    };
    let mut out = format!("{before}{wrapper} %command%");
    let after = after.trim();
    if !after.is_empty() {
        out.push(' ');
        out.push_str(after);
    }
    out
}

/// Steam finds `%command%` in any case.
fn command() -> &'static regex::Regex {
    static COMMAND: OnceLock<regex::Regex> = OnceLock::new();
    COMMAND.get_or_init(|| regex::Regex::new("(?i)%command%").unwrap())
}

/// Whether `options` start the game through `wrapper` (see [`with_wrapper`]), however
/// `%command%` is written. With `windows`, the path's case doesn't matter either.
pub fn starts_through(options: &str, wrapper: &str, windows: bool) -> bool {
    let normal = |s: &str| {
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        let s = command().replace_all(&s, "%command%").into_owned();
        if windows { s.to_lowercase() } else { s }
    };
    normal(options).contains(&normal(&format!("{wrapper} %command%")))
}

/// `current` without mint in front of `%command%`, wherever it was and however it was started.
fn without_mint(current: &str) -> std::borrow::Cow<'_, str> {
    static MINT: OnceLock<regex::Regex> = OnceLock::new();
    let mint = MINT.get_or_init(|| {
        // Windows paths aren't case-sensitive
        regex::Regex::new(r#"(?i)"[^"]*mint(?:-launch)?(?:\.exe)?"(?:\s+launch)?\s+%command%"#)
            .unwrap()
    });
    mint.replace(current.trim(), "%command%")
}

/// `current` without mint (see [`with_wrapper`]) and with [`FLAGS`], which the game still needs
/// when mint is started by hand. A bare `%command%` in front goes too: Steam passes options
/// without it to the game all the same.
pub fn without_wrapper(current: &str) -> String {
    let current = without_mint(current);
    let current = match command().find(&current) {
        Some(m)
            if m.start() == 0
                && current[m.end()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace) =>
        {
            &current[m.end()..]
        }
        _ => &current,
    };
    with_flags(current)
}

/// `current` with [`FLAGS`] added, for when mint isn't started through Steam.
pub fn with_flags(current: &str) -> String {
    let mut out = current.trim().to_string();
    for flag in FLAGS {
        if !out.split_whitespace().any(|a| a.eq_ignore_ascii_case(flag)) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(flag);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCHER: &str = r#""C:\Users\Jo Doe\AppData\Local\mint\mint-launch.exe""#;

    fn vdf(apps: &str) -> String {
        format!(
            "\"UserLocalConfigStore\"\n{{\n\t\"Software\"\n\t{{\n\t\t\"Valve\"\n\t\t{{\n\t\t\t\"Steam\"\n\t\t\t{{\n\t\t\t\t\"apps\"\n\t\t\t\t{{{apps}\n\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t}}\n\t\"friends\" {{ \"LaunchOptions\" \"not this one\" }}\n}}\n"
        )
    }

    #[test]
    fn value_is_replaced_and_everything_else_kept() {
        let text = vdf(
            "\n\t\t\t\t\t\"548430\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LaunchOptions\"\t\t\"-nosplash\"\n\t\t\t\t\t\t\"Playtime\"\t\t\"12\"\n\t\t\t\t\t}",
        );
        let out = set_launch_options(&text, |c| with_wrapper(c, LAUNCHER))
            .unwrap()
            .unwrap();
        let want = format!("{LAUNCHER} %command% -nosplash");
        assert_eq!(launch_options(&out).unwrap(), want);
        assert_eq!(
            out,
            text.replace("\"-nosplash\"", &format!("\"{}\"", escape(&want)))
        );
        // again: nothing to change
        assert_eq!(
            set_launch_options(&out, |c| with_wrapper(c, LAUNCHER)).unwrap(),
            None
        );
    }

    #[test]
    fn missing_value_and_block_are_added() {
        let text = vdf(
            "\n\t\t\t\t\t\"548430\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"Playtime\"\t\t\"12\"\n\t\t\t\t\t}",
        );
        // DRG's settings, without launch options
        assert_eq!(drg_launch_options(&text).unwrap().as_deref(), Some(""));
        let out = set_launch_options(&text, with_flags).unwrap().unwrap();
        assert_eq!(launch_options(&out).unwrap(), "-disablemodding -nosplash");

        let text = vdf("\n\t\t\t\t\t\"1234\"\n\t\t\t\t\t{\n\t\t\t\t\t}");
        assert_eq!(drg_launch_options(&text).unwrap(), None);
        let out = set_launch_options(&text, with_flags).unwrap().unwrap();
        assert_eq!(launch_options(&out).unwrap(), "-disablemodding -nosplash");
        assert_eq!(
            drg_launch_options(&out).unwrap().as_deref(),
            Some("-disablemodding -nosplash")
        );
        assert!(out.contains("\"1234\""));
    }

    #[test]
    fn broken_files_are_refused() {
        for text in [
            "\"a\" { \"b\" \"c\"",
            "\"a\" }",
            "\"a\" { x }",
            "\"unterminated",
        ] {
            assert!(set_launch_options(text, with_flags).is_err(), "{text}");
        }
        // no apps section at all
        assert!(set_launch_options("\"UserLocalConfigStore\" { }", with_flags).is_err());
    }

    #[test]
    fn launch_options_with_mint() {
        let m = |c| with_wrapper(c, LAUNCHER);
        let w = format!("{LAUNCHER} %command%");
        assert_eq!(m(""), w);
        assert_eq!(m("-nosplash -dx11"), format!("{w} -nosplash -dx11"));
        assert_eq!(
            m("gamemoderun %command% -dx11"),
            format!("gamemoderun {w} -dx11")
        );
        assert_eq!(m("PROTON_LOG=1 %command%"), format!("PROTON_LOG=1 {w}"));
        // mint from before: elsewhere, or started without the launcher
        assert_eq!(
            m("\"/home/jo/old/mint\" launch %command% -NOSPLASH -disablemodding"),
            format!("{w} -NOSPLASH -disablemodding")
        );
        assert_eq!(
            m(r#""C:\old\mint.exe" launch %command% -dx11"#),
            format!("{w} -dx11")
        );
        assert_eq!(
            m(r#""C:\Tools\MINT.EXE" LAUNCH %command% -dx11"#),
            format!("{w} -dx11")
        );
        // Linux starts mint directly; switching replaces the launcher too
        assert_eq!(
            with_wrapper(&m("-dx11"), r#""/home/jo/.local/share/mint/mint" launch"#),
            r#""/home/jo/.local/share/mint/mint" launch %command% -dx11"#
        );
        // idempotent
        let once = m("gamemoderun %command% -dx11");
        assert_eq!(m(&once), once);
        // Steam takes %command% in any case
        assert_eq!(
            m("gamemoderun %COMMAND% -dx11"),
            format!("gamemoderun {w} -dx11")
        );
        assert_eq!(
            m(&format!("{LAUNCHER} %COMMAND% -dx11")),
            format!("{w} -dx11")
        );
    }

    #[test]
    fn mint_is_found_however_its_written() {
        let linux = r#""/home/jo/.local/share/mint/mint" launch"#;
        assert!(starts_through(
            &format!("{LAUNCHER} %command% -dx11"),
            LAUNCHER,
            true
        ));
        assert!(starts_through(
            &format!("{LAUNCHER} %COMMAND% -dx11"),
            LAUNCHER,
            true
        ));
        assert!(starts_through(
            &format!("gamemoderun {LAUNCHER}  %Command%"),
            LAUNCHER,
            true
        ));
        // Windows paths aren't case-sensitive
        assert!(starts_through(
            r#""c:\users\jo doe\appdata\local\mint\MINT-LAUNCH.EXE" %command%"#,
            LAUNCHER,
            true
        ));
        assert!(starts_through(&format!("{linux} %COMMAND%"), linux, false));
        // Linux paths are
        assert!(!starts_through(
            r#""/home/jo/.local/share/MINT/mint" launch %command%"#,
            linux,
            false
        ));
        assert!(!starts_through("-dx11 %command%", LAUNCHER, true));
        assert!(!starts_through(
            &format!("{LAUNCHER} -dx11"),
            LAUNCHER,
            true
        ));
    }

    #[test]
    fn launch_options_without_mint() {
        let w = format!("{LAUNCHER} %command%");
        assert_eq!(without_wrapper(&w), "-disablemodding -nosplash");
        assert_eq!(
            without_wrapper(&format!("{w} -dx11 -nosplash")),
            "-dx11 -nosplash -disablemodding"
        );
        assert_eq!(
            without_wrapper(
                r#"gamemoderun "/home/jo/.local/share/mint/mint" launch %command% -dx11"#
            ),
            "gamemoderun %command% -dx11 -disablemodding -nosplash"
        );
        // back to how it was before mint, plus the game options
        let m = |c| with_wrapper(c, LAUNCHER);
        assert_eq!(
            without_wrapper(&m("-dx11")),
            "-dx11 -disablemodding -nosplash"
        );
        assert_eq!(without_wrapper(&m("")), "-disablemodding -nosplash");
        // no mint in there
        assert_eq!(without_wrapper("-dx11"), "-dx11 -disablemodding -nosplash");
        assert_eq!(
            without_wrapper(&format!("{LAUNCHER} %COMMAND%")),
            "-disablemodding -nosplash"
        );
        assert_eq!(
            without_wrapper("%COMMAND% -dx11"),
            "-dx11 -disablemodding -nosplash"
        );
        assert_eq!(
            without_wrapper("%command%-foo"),
            "%command%-foo -disablemodding -nosplash"
        );
    }

    #[test]
    fn escapes_survive_a_round_trip() {
        let text = vdf("\n\t\t\t\t\t\"548430\"\n\t\t\t\t\t{\n\t\t\t\t\t}");
        let out = set_launch_options(&text, |c| with_wrapper(c, LAUNCHER))
            .unwrap()
            .unwrap();
        assert!(
            out.contains(
                r#"\"C:\\Users\\Jo Doe\\AppData\\Local\\mint\\mint-launch.exe\" %command%"#
            )
        );
        assert_eq!(
            launch_options(&out).unwrap(),
            format!("{LAUNCHER} %command%")
        );
    }
}
