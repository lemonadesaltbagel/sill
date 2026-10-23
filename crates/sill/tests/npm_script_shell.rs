//! npm runs package.json scripts in sill unless a script shell is configured
//! (brush-core `sys::script_shell`), and a script that looks written for cmd.exe gets a
//! note (`script_hint`). Needs `npm` on PATH; skipped without it.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPTS: &str = r#"{ "name": "t", "version": "1.0.0", "private": true, "scripts": {
  "unix": "rm -rf d && mkdir -p d/e && NODE_ENV=x node -e \"console.log(process.env.NODE_ENV)\" ; echo after",
  "cmdset": "set X=1&& node -e \"console.log(process.env.X||'unset')\"",
  "os": "echo %OS%"
} }"#;

fn have_npm() -> bool {
    Command::new("cmd.exe").args(["/d", "/c", "where npm"]).output().is_ok_and(|o| o.status.success())
}

fn project(name: &str, npmrc: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sill-npm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("package.json"), SCRIPTS).unwrap();
    if let Some(rc) = npmrc {
        std::fs::write(dir.join(".npmrc"), rc).unwrap();
    }
    dir
}

/// Runs `script` in sill in `dir`, without any script shell from the test's environment
/// and with an empty user `.npmrc`.
fn sill_in(dir: &Path, script: &str, env: &[(&str, &str)]) -> (String, String) {
    let userrc = dir.join("user.npmrc");
    std::fs::write(&userrc, "").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bash"));
    cmd.args(["--norc", "--noprofile", "-c", script])
        .current_dir(dir)
        .env_remove("npm_config_script_shell")
        .env_remove("pnpm_config_script_shell")
        .env("npm_config_userconfig", &userrc);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run sill");
    let text = |b: &[u8]| String::from_utf8_lossy(b).replace('\r', "");
    (text(&out.stdout), text(&out.stderr))
}

#[test]
fn unix_scripts_run_in_sill_by_default() {
    if !have_npm() {
        eprintln!("npm not found; skipped");
        return;
    }
    let dir = project("default", None);
    let (out, err) = sill_in(&dir, "npm run --silent unix", &[]);
    assert_eq!(out, "x\nafter\n", "stderr: {err}");
    assert!(dir.join("d/e").is_dir());

    let (out, _) = sill_in(&dir, "npm config get script-shell", &[]);
    let exe = std::fs::canonicalize(env!("CARGO_BIN_EXE_bash")).unwrap();
    let got = std::fs::canonicalize(out.trim()).unwrap();
    assert_eq!(got, exe, "{out}");
    assert!(!out.contains('\\'), "forward slashes: {out}");
}

#[test]
fn also_through_cmd_and_powershell() {
    if !have_npm() {
        eprintln!("npm not found; skipped");
        return;
    }
    let dir = project("indirect", None);
    for script in [
        "cmd.exe /d /c 'npm run --silent unix'",
        // `npm.cmd`: client Windows' default execution policy refuses `npm.ps1`.
        "powershell -NoProfile -Command 'npm.cmd run --silent unix'",
    ] {
        let (out, err) = sill_in(&dir, script, &[]);
        assert_eq!(out, "x\nafter\n", "{script}: {err}");
    }
}

#[test]
fn a_configured_script_shell_is_kept() {
    if !have_npm() {
        eprintln!("npm not found; skipped");
        return;
    }
    // A project .npmrc choosing cmd.exe.
    let dir = project("npmrc", Some("script-shell=C:\\Windows\\System32\\cmd.exe\n"));
    let (out, err) = sill_in(&dir, "npm run --silent os", &[]);
    assert_eq!(out.trim(), "Windows_NT", "stderr: {err}");

    // The user's variable, even empty (npm then uses its default, cmd.exe).
    let dir = project("env", None);
    let (out, err) = sill_in(&dir, "npm_config_script_shell= npm run --silent os", &[]);
    assert_eq!(out.trim(), "Windows_NT", "stderr: {err}");

    // SILL_NO_ENV_DEFAULTS turns it off.
    let (out, err) = sill_in(&dir, "npm run --silent os", &[("SILL_NO_ENV_DEFAULTS", "1")]);
    assert_eq!(out.trim(), "Windows_NT", "stderr: {err}");
}

#[test]
fn a_cmd_style_script_gets_a_note() {
    if !have_npm() {
        eprintln!("npm not found; skipped");
        return;
    }
    let dir = project("hint", None);
    let (out, err) = sill_in(&dir, "npm run --silent cmdset", &[]);
    // Run as written (bash `set` sets $1), with the note on stderr.
    assert_eq!(out.trim(), "unset");
    assert!(
        err.contains("\"cmdset\" script looks written for cmd.exe (`set X=1`)")
            && err.contains("npm run cmdset --script-shell cmd.exe"),
        "{err}"
    );
    // No note for a bash script.
    let (_, err) = sill_in(&dir, "npm run --silent unix", &[]);
    assert!(!err.contains("cmd.exe"), "{err}");
}
