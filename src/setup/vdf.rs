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
    static EARLIER: OnceLock<regex::Regex> = OnceLock::new();
    let earlier = EARLIER.get_or_init(|| {
        // Windows paths aren't case-sensitive
        regex::Regex::new(r#"(?i)"[^"]*mint(?:-launch)?(?:\.exe)?"(?:\s+launch)?\s+%command%"#)
            .unwrap()
    });
    let current = earlier.replace(current.trim(), "%command%");
    // without %command%, Steam passes the options to the game
    let (before, after) = match current.find("%command%") {
        Some(i) => (&current[..i], &current[i + "%command%".len()..]),
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
        let out = set_launch_options(&text, with_flags).unwrap().unwrap();
        assert_eq!(launch_options(&out).unwrap(), "-disablemodding -nosplash");

        let text = vdf("\n\t\t\t\t\t\"1234\"\n\t\t\t\t\t{\n\t\t\t\t\t}");
        let out = set_launch_options(&text, with_flags).unwrap().unwrap();
        assert_eq!(launch_options(&out).unwrap(), "-disablemodding -nosplash");
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
