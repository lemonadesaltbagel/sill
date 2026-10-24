//! sill: running `#!` scripts on Windows.
//!
//! Windows runs only its own executable formats, so `./build.sh`, `./gradlew` or
//! `./tool.py` failed with "command not found" (bash on Linux and Git Bash run them by
//! their `#!` line). Here a file without a Windows executable extension counts as a
//! command when it starts with `#!`, and starting it means starting its interpreter with
//! the script's path, as the kernel does on Linux:
//! - `/bin/sh`, `/bin/bash`, `/usr/bin/env bash`, ... run in sill itself;
//! - `/usr/bin/env NAME` and POSIX paths like `/usr/bin/python3` (no such directory on
//!   Windows) mean the program NAME on `PATH`. `python3` falls back to `python`, the
//!   name python.org's installer uses;
//! - an existing Windows path (`#!C:/Tools/x.exe`) is used as written.
//!
//! A program found through `PATHEXT` wins over a `#!` file of the bare name, so npm's
//! `node_modules/.bin/foo.cmd` still runs instead of the `foo` shell shim beside it.

/// A parsed `#!` line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Shebang {
    /// The interpreter as written: a path or, after `env`, a program name.
    pub interpreter: String,
    /// Arguments for the interpreter before the script path.
    pub args: Vec<String>,
    /// Whether the interpreter came through `env` (a name to look up on `PATH`).
    pub via_env: bool,
}

/// Interpreters that sill itself is.
pub(crate) fn is_shell(name: &str) -> bool {
    matches!(name, "sh" | "bash" | "dash" | "ash")
}

/// Parses the first line of a file; `None` if it isn't a `#!` line.
pub(crate) fn parse(head: &[u8]) -> Option<Shebang> {
    let rest = head.strip_prefix(b"#!")?;
    let line = rest.split(|&b| b == b'\n').next().unwrap_or(rest);
    let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r').trim();
    // Linux: the interpreter, then (all of) the rest as a single optional argument.
    let (interp, arg) = line
        .split_once([' ', '\t'])
        .map_or((line, ""), |(interp, arg)| (interp, arg.trim()));
    if interp.is_empty() {
        return None;
    }
    let base = interp.rsplit(['/', '\\']).next().unwrap_or(interp);
    if base == "env" || base == "env.exe" {
        // `env NAME [ARGS]`, `env -S NAME ARGS`: options and assignments skipped. (Linux
        // hands env the rest as one word, so `env node --flag` works there only with -S;
        // the evident intent is the same either way.)
        let mut words = arg.split_whitespace().skip_while(|w| w.starts_with('-') || w.contains('='));
        let name = words.next()?.to_owned();
        return Some(Shebang { interpreter: name, args: words.map(str::to_owned).collect(), via_env: true });
    }
    let args = if arg.is_empty() { vec![] } else { vec![arg.to_owned()] };
    Some(Shebang { interpreter: interp.to_owned(), args, via_env: false })
}

/// The name to look up on `PATH` for an interpreter written as a path that doesn't
/// exist (`/usr/bin/python3` → `python3`).
pub(crate) fn interpreter_name(interpreter: &str) -> &str {
    interpreter.rsplit(['/', '\\']).next().unwrap_or(interpreter)
}

/// Whether the file at `path` starts with `#!`.
pub(crate) fn file_has_shebang(path: &std::path::Path) -> bool {
    use std::io::Read as _;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = [0u8; 2];
    f.read_exact(&mut head).is_ok() && head == *b"#!"
}

/// The first line of the file at `path`, parsed.
pub(crate) fn read(path: &std::path::Path) -> Option<Shebang> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; 512];
    let n = f.read(&mut head).ok()?;
    head.truncate(n);
    parse(&head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<Shebang> {
        parse(s.as_bytes())
    }
    fn sb(interp: &str, args: &[&str], via_env: bool) -> Option<Shebang> {
        Some(Shebang { interpreter: interp.into(), args: args.iter().map(|s| (*s).into()).collect(), via_env })
    }

    #[test]
    fn forms() {
        assert_eq!(p("#!/bin/sh\necho"), sb("/bin/sh", &[], false));
        assert_eq!(p("#!/bin/bash -e\r\n"), sb("/bin/bash", &["-e"], false));
        assert_eq!(p("#! /usr/bin/python3 -u -X dev\n"), sb("/usr/bin/python3", &["-u -X dev"], false));
        assert_eq!(p("#!/usr/bin/env bash\n"), sb("bash", &[], true));
        assert_eq!(p("#!/usr/bin/env python3\r\n"), sb("python3", &[], true));
        assert_eq!(p("#!/usr/bin/env -S node --no-warnings\n"), sb("node", &["--no-warnings"], true));
        assert_eq!(p("#!/usr/bin/env node --harmony\n"), sb("node", &["--harmony"], true));
        assert_eq!(p("#!C:/Tools/x.exe\n"), sb("C:/Tools/x.exe", &[], false));
    }

    #[test]
    fn not_shebangs() {
        for s in ["", "#", "# comment", "#!", "#!   \n", "#!/usr/bin/env\n"] {
            assert_eq!(p(s), None, "{s:?}");
        }
        assert_eq!(parse(b"MZ\x90\0"), None);
    }

    #[test]
    fn names() {
        assert_eq!(interpreter_name("/usr/local/bin/node"), "node");
        assert_eq!(interpreter_name("python3"), "python3");
        assert!(is_shell("sh") && is_shell("bash") && !is_shell("zsh") && !is_shell("python3"));
    }
}
