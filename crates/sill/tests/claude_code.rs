//! Claude Code drop-in contract: the argv shapes, PATH probe, snapshot and command
//! wrapper Claude Code (2.1.305) uses with `CLAUDE_CODE_GIT_BASH_PATH`, replayed
//! against the real binary, as observed from Claude Code's own shell integration.
//!
//! The wrapper and snapshot here use only shell builtins; the parts of Claude Code's
//! snapshot script that need external tools (head, cut, grep, awk, sed) depend on
//! bundled coreutils, which come later.

mod common;

use common::{run, stdout_of};
use std::path::{Path, PathBuf};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sill-cc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// How Claude Code writes a path into a command: `O0()` gives `/c/x/y` on Windows.
fn cc_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let b = s.as_bytes();
    if cfg!(windows) && b.len() >= 2 && b[1] == b':' {
        format!("/{}{}", (b[0] as char).to_ascii_lowercase(), &s[2..])
    } else {
        s
    }
}

/// Claude Code's `So()` quoting: bare if `[A-Za-z0-9_./:=@+,-]+`, else single-quoted.
fn cc_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./:=@+,-".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r#"'"'"'"#))
    }
}

/// Claude Code's command wrapper, joined with " && ".
fn cc_wrapper(snapshot: &Path, model_command: &str, cwd_file: &Path) -> String {
    let temp = std::env::temp_dir().to_string_lossy().into_owned();
    [
        format!("source {} 2>/dev/null || true", cc_path(snapshot)),
        format!("export TEMP={} TMP={}", cc_quote(&temp), cc_quote(&temp)),
        "shopt -u extglob 2>/dev/null || true".to_owned(),
        r"{ \builtin unalias -- 'unsetenv'; \builtin unset -f -- 'unsetenv'; } >/dev/null 2>&1 || true".to_owned(),
        format!("eval {} < /dev/null", cc_quote(model_command)),
        format!("pwd -P >| {}", cc_path(cwd_file)),
    ]
    .join(" && ")
}

#[test]
fn version_answers_with_status_zero() {
    let r = run(&["--version"], None);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(!r.stdout.trim().is_empty());
}

#[test]
fn dash_c_then_dash_l_takes_the_first_non_option_as_the_command() {
    // getSpawnArgs: ["-c", ...(snapshot ? [] : ["-l"]), cmd]
    for args in [&["-c", "-l", "echo ok"][..], &["-lc", "echo ok"], &["-l", "-c", "echo ok"], &["-c", "echo ok"]] {
        let r = run(args, None);
        assert_eq!((r.code, r.stdout.trim()), (Some(0), "ok"), "{args:?}: {}", r.stderr);
    }
}

#[test]
fn dash_c_positional_parameters() {
    let r = run(&["-c", "echo \"$0|$1|$2\"", "name", "a b", "c"], None);
    assert_eq!(r.stdout.trim(), "name|a b|c");
}

#[test]
#[cfg(windows)]
fn path_probe_is_posix_style() {
    // sbr(): accepted iff exit 0, the last line starts with "/" and has no ";".
    let r = run(&["-lc", "echo \"$PATH\""], None);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let last = r.stdout.trim_end().lines().last().unwrap_or_default();
    assert!(last.starts_with('/') && !last.contains(';'), "PATH probe rejected: {last}");
}

#[test]
#[cfg(windows)]
fn appended_posix_path_entry_is_searched_and_children_get_windows_form() {
    let tools = scratch_dir("path");
    std::fs::write(tools.join("sill-cc-probe.cmd"), "@echo probe-found\r\n").unwrap();
    let script = format!(
        "export PATH=\"$PATH:{}\"; sill-cc-probe; cmd.exe /d /c \"echo %PATH%\"",
        cc_path(&tools)
    );
    let out = stdout_of(&script);
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("probe-found"));
    let child_path = lines.next().unwrap_or_default();
    assert!(child_path.contains(';'), "child PATH not in Windows form: {child_path}");
    assert!(!child_path.contains(":/"), "POSIX entry leaked to child: {child_path}");
    assert!(
        child_path.to_lowercase().contains(&tools.to_string_lossy().to_lowercase()),
        "appended dir missing from child PATH: {child_path}"
    );
}

#[test]
fn snapshot_round_trip_with_builtins() {
    // The snapshot is made by `bash -c -l <script>` and later sourced by every command.
    let dir = scratch_dir("snap");
    let snap = dir.join("snapshot-bash-1-abc.sh");
    let plugin = dir.join("plugin bin");
    std::fs::create_dir_all(&plugin).unwrap();
    let make = format!(
        r##"SNAPSHOT_FILE={snap}
myfn() {{ echo "fn ok: $1"; }}
alias ll='echo alias ok'
echo "# Snapshot file" >| "$SNAPSHOT_FILE"
unalias -a 2>/dev/null || true
alias ll='echo alias ok'
for func in $(declare -F | while read -r _ _ name; do echo "$name"; done); do
  printf 'eval %q > /dev/null 2>&1\n' "$(declare -f "$func")" >> "$SNAPSHOT_FILE"
done
echo "shopt -s expand_aliases" >> "$SNAPSHOT_FILE"
alias | while read -r line; do echo "alias -- ${{line#alias }}" >> "$SNAPSHOT_FILE"; done
__cc_path=$PATH
for __cc_dir in {plugin}; do
  case ":$__cc_path:" in *":$__cc_dir:"*) ;; *) __cc_path=${{__cc_path:+$__cc_path:}}$__cc_dir ;; esac
done
builtin printf 'export PATH=%q\n' "$__cc_path" >> "$SNAPSHOT_FILE""##,
        snap = cc_quote(&cc_path(&snap)),
        plugin = cc_quote(&cc_path(&plugin)),
    );
    let r = run(&["-c", "-l", &make], None);
    assert_eq!(r.code, Some(0), "snapshot creation failed: {}", r.stderr);

    let use_it = format!("source {} && myfn x && ll && echo \"$PATH\"", cc_quote(&cc_path(&snap)));
    let out = stdout_of(&use_it);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.first().copied(), Some("fn ok: x"), "{out}");
    assert_eq!(lines.get(1).copied(), Some("alias ok"), "{out}");
    let path = lines.get(2).copied().unwrap_or_default();
    assert!(path.ends_with(&cc_path(&plugin)), "plugin dir not last in PATH: {path}");
    if cfg!(windows) {
        assert!(!path.contains(';'), "Windows separator leaked into snapshot PATH: {path}");
    }
}

#[test]
fn command_wrapper_runs_and_reports_cwd() {
    let dir = scratch_dir("wrap");
    std::fs::create_dir_all(dir.join("sub dir")).unwrap();
    let snap = dir.join("snapshot.sh");
    std::fs::write(&snap, "export CC_SNAP_VAR=from-snapshot\nccfn() { echo \"ccfn $*\"; }\n").unwrap();
    let cwd_file = dir.join("claude-1a2b-cwd");
    let model = "echo \"$CC_SNAP_VAR\"; ccfn 'it''s'; cd 'sub dir' && printf '%s\\n' \"a\\\\b\"";
    let wrapper = cc_wrapper(&snap, model, &cwd_file);

    let r = run(&["-c", &wrapper], Some(&dir));
    assert_eq!(r.code, Some(0), "wrapper failed: {}\n{wrapper}", r.stderr);
    assert_eq!(r.stdout, "from-snapshot\nccfn its\na\\b\n");

    let reported = std::fs::read_to_string(&cwd_file).expect("cwd file written");
    let reported = reported.trim_end();
    assert!(reported.ends_with("/sub dir"), "cwd file: {reported}");
    if cfg!(windows) {
        assert!(!reported.contains('\\'), "cwd file must use / (and no \\\\?\\): {reported}");
        assert_eq!(reported.as_bytes().get(1), Some(&b':'), "cwd file: {reported}");
    }
}

#[test]
#[cfg(windows)]
fn dev_null_never_creates_a_nul_file() {
    let dir = scratch_dir("nul");
    let r = run(&["-c", "echo hi > /dev/null; echo err 2>/dev/null >&2; true"], Some(&dir));
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).map(|e| e.file_name()).collect();
    assert!(names.is_empty(), "stray files: {names:?}");
}

/// Claude Code starts the shell detached, with no console. sill then creates an invisible
/// one that the programs it runs share (as Git Bash does), instead of each program getting
/// a console window of its own: a code page set by one program holds for the next, and
/// sill's own output still goes to the harness's pipe.
#[test]
#[cfg(windows)]
fn detached_start_gets_one_shared_invisible_console() {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bash"))
        .args(["-c", "-l", "echo own-output; chcp.com 1252 >/dev/null; chcp.com"])
        .creation_flags(DETACHED_PROCESS)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run sill");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("own-output"), "{text:?}");
    // "Active code page: 1252", in the system's language.
    assert!(text.trim_end().ends_with("1252"), "{text:?}");
}

/// That console is UTF-8, so console programs' text reaches the harness as UTF-8 (and
/// Windows' own messages in English); `SILL_CONSOLE_CP` picks another code page.
#[test]
#[cfg(windows)]
fn detached_console_is_utf8() {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let run = |extra: Option<(&str, &str)>, script: &str| {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_bash"));
        cmd.args(["-c", "-l", script])
            .creation_flags(DETACHED_PROCESS)
            .stdin(std::process::Stdio::null())
            .env_remove("SILL_CONSOLE_CP");
        if let Some((k, v)) = extra {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run sill");
        (String::from_utf8(out.stdout).expect("UTF-8 stdout"), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (out, _) = run(None, "chcp.com; cmd.exe /d /c 'echo 中文 Русский 😀'");
    assert!(out.lines().next().unwrap_or("").trim_end().ends_with("65001"), "{out:?}");
    assert!(out.contains("中文 Русский 😀"), "{out:?}");
    let (out, _) = run(None, "cmd.exe /d /c nosuchcommand_xyz 2>&1");
    assert!(out.contains("is not recognized"), "{out:?}");
    let (out, _) = run(Some(("SILL_CONSOLE_CP", "437")), "chcp.com");
    assert!(out.trim_end().ends_with("437"), "{out:?}");
}

/// Before Windows 11 24H2 there's no AllocConsoleWithOptions; sill attaches to a helper's
/// windowless console instead. `SILL_CONSOLE_FALLBACK=1` takes that path here too.
#[test]
#[cfg(windows)]
fn detached_console_fallback_for_older_windows() {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let t0 = std::time::Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bash"))
        .args(["-c", "-l", "echo own-output; chcp.com; cmd.exe /d /c 'echo 中文 😀'"])
        .creation_flags(DETACHED_PROCESS)
        .env("SILL_CONSOLE_FALLBACK", "1")
        .env_remove("SILL_CONSOLE_CP")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run sill");
    let text = String::from_utf8(out.stdout).expect("UTF-8 stdout");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first().copied(), Some("own-output"), "{text:?}");
    assert!(lines.get(1).is_some_and(|l| l.trim_end().ends_with("65001")), "{text:?}");
    assert!(text.contains("中文 😀"), "{text:?}");
    assert!(t0.elapsed() < std::time::Duration::from_secs(10));
}

/// A background job doesn't keep the harness's stdout pipe open: harnesses that wait for
/// the pipe to close (opencode, Python's communicate()) saw it only when the job exited.
#[test]
#[cfg(windows)]
fn background_job_does_not_hold_the_harness_pipe() {
    use std::io::Read as _;
    let dir = std::env::temp_dir().join(format!("sill-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t0 = std::time::Instant::now();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_bash"))
        .args(["-c", "-l", "ping -n 9 127.0.0.1 > bg.log 2>&1 & echo started"])
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run sill");
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    let eof = t0.elapsed();
    let _ = child.wait();
    assert_eq!(out.trim(), "started");
    assert!(eof < std::time::Duration::from_secs(4), "pipe closed only after {eof:?}");
    let _ = std::process::Command::new("taskkill").args(["/F", "/IM", "PING.EXE"]).output();
}
