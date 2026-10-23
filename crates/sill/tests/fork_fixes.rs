//! Regression tests for sill's changes to the vendored brush, run against the real
//! binary. Each script runs under a timeout, so a deadlock fails instead of hanging.
//! Windows-only behaviour is checked in the windows CI job.

mod common;

use common::stdout_of;

// F1: here-doc and here-string bodies larger than the pipe buffer used to deadlock
// (the whole body was written before anything read it).

#[test]
fn large_heredoc_does_not_deadlock() {
    let body = format!("{}\n", "x".repeat(99)).repeat(2000); // 200,000 bytes
    let script = format!("n=0; while IFS= read -r l; do n=$((n+${{#l}}+1)); done <<'EOF'\n{body}EOF\necho $n");
    assert_eq!(stdout_of(&script), "200000");
}

#[test]
fn large_herestring_does_not_deadlock() {
    let script = "x=$(printf 'y%.0s' {1..100000}); n=0; while IFS= read -r l; do n=${#l}; done <<<\"$x\"; echo $n";
    assert_eq!(stdout_of(script), "100000");
}

#[test]
fn heredoc_unread_body_does_not_hang() {
    // A command that never reads stdin must not block on the writer thread.
    let body = "z\n".repeat(50_000);
    let script = format!("true <<'EOF'\n{body}EOF\necho done");
    assert_eq!(stdout_of(&script), "done");
}

#[test]
fn small_heredoc_unchanged() {
    assert_eq!(stdout_of("cat_like() { while IFS= read -r l; do echo \"[$l]\"; done; }; cat_like <<EOF\na b\n  c\nEOF"), "[a b]\n[  c]");
}

// F2: $OSTYPE speaks the dialect harnesses check for (Claude Code: msys/cygwin/win32).

#[test]
#[cfg(windows)]
fn ostype_is_msys_on_windows() {
    assert_eq!(stdout_of("echo $OSTYPE"), "msys");
}

// F3: POSIX-style paths the shell itself opens or enters, and `/`-separated $PWD.

#[test]
#[cfg(windows)]
fn cd_msys_drive_form() {
    assert_eq!(stdout_of("cd /c/ && pwd"), "C:/");
    assert_eq!(stdout_of("cd /c/Windows && echo \"$PWD\""), "C:/Windows");
}

#[test]
#[cfg(windows)]
fn tmp_is_the_drive_root_tmp_directory() {
    // `/tmp` is `\tmp` on the current drive, as Windows resolves it for any program.
    let cwd = std::env::current_dir().unwrap();
    let drive = cwd.to_string_lossy()[..2].to_owned();
    let got = stdout_of("cd /tmp && pwd");
    assert_eq!(got.to_lowercase(), format!("{drive}/tmp").to_lowercase());
}

#[test]
#[cfg(windows)]
fn tmp_file_is_shared_with_native_programs() {
    // The shell's redirect and a native program given the same /tmp path agree.
    let out = stdout_of(
        "echo shared > /tmp/sill-tmp-share.txt && cmd.exe /d /c type '\\tmp\\sill-tmp-share.txt'",
    );
    assert_eq!(out.replace('\r', "").trim(), "shared");
}

#[test]
#[cfg(windows)]
fn msys_double_slash_switch_reaches_native_programs_as_single_slash() {
    let out = stdout_of("cmd //d //c echo via-double-slash");
    assert_eq!(out.replace('\r', "").trim(), "via-double-slash");
}

#[test]
#[cfg(windows)]
fn redirect_into_msys_path() {
    let out = stdout_of("echo hello > /tmp/sill-redirect-test.txt && read -r l < /tmp/sill-redirect-test.txt && echo $l");
    assert_eq!(out, "hello");
}

#[test]
#[cfg(windows)]
fn pwd_and_oldpwd_use_forward_slashes() {
    let out = stdout_of("cd C:\\\\Windows && cd C:\\\\ && echo \"$PWD|$OLDPWD\"");
    assert_eq!(out, "C:/|C:/Windows");
}

#[test]
fn ordinary_paths_unaffected() {
    assert_eq!(stdout_of("d=$PWD; cd . && [ \"$PWD\" = \"$d\" ] && echo same"), "same");
}

#[test]
#[cfg(windows)]
fn pwd_physical_has_no_verbatim_prefix() {
    // `pwd -P` canonicalizes; Claude Code runs `pwd -P >| <cwd file>` after every command.
    let out = stdout_of("cd C:\\\\Windows && pwd -P");
    assert!(!out.contains('\\'), "{out}");
    assert_eq!(out.to_lowercase(), "c:/windows");
}

// F5: fds above 2 stay with the shell instead of failing every native launch.

#[test]
fn native_command_runs_while_shell_holds_fd_3() {
    let out = stdout_of(
        "f=\"${TMPDIR:-${TEMP:-/tmp}}/sill-fd3-$$.txt\"; exec 3>\"$f\"; echo to-fd3 >&3; \
         sh_out=$(cmd_or_true() { command -v cmd.exe >/dev/null && cmd.exe /d /c echo native-ok || echo native-ok; }; cmd_or_true); \
         exec 3>&-; echo \"$sh_out\"; read -r l < \"$f\"; echo \"$l\"",
    );
    assert_eq!(out.replace('\r', ""), "native-ok\nto-fd3");
}

// Bundled coreutils (uutils, via brush's experimental-bundled-coreutils): the common
// tools work on a machine with nothing on PATH, e.g. Windows without Git.

#[test]
fn bundled_coreutils_work_with_empty_path() {
    let out = stdout_of(
        "PATH=; d=\"${TMPDIR:-${TEMP:-/tmp}}/sill-uu-$$\"; mkdir -p \"$d\" && cd \"$d\" && \
         printf 'b 2\\na 1\\nc 3\\n' > f.txt && cat f.txt | sort | head -n 2 | cut -d' ' -f1 | tr '\\n' ,; \
         wc -l < f.txt; basename \"$d/f.txt\"; cd .. && rm -r \"$d\" && echo cleaned",
    );
    assert_eq!(out.replace('\r', ""), "a,b,3\nf.txt\ncleaned");
}

#[test]
fn ls_hides_dotfiles_like_gnu() {
    let out = stdout_of(
        "d=\"${TMPDIR:-${TEMP:-/tmp}}/sill-ls-$$\"; mkdir -p \"$d\" && cd \"$d\" && \
         touch .hidden visible && ls; echo --; ls -A; cd .. && rm -r \"$d\"",
    );
    assert_eq!(out.replace('\r', ""), "visible\n--\n.hidden\nvisible");
}

// Errors in bash's shape, without color codes when stderr isn't a terminal.

#[test]
fn command_not_found_is_bash_shaped_and_plain() {
    let r = common::sill("nosuchcmd-xyz; echo \"rc=$?\"");
    assert_eq!(r.stdout.trim_end(), "rc=127");
    assert!(!r.stderr.contains('\u{1b}'), "escape codes in captured stderr: {:?}", r.stderr);
    assert!(
        r.stderr.contains(": nosuchcmd-xyz: command not found"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
#[cfg(windows)]
fn linux_only_command_names_the_windows_alternative() {
    let r = common::sill("apt-get install -y jq; echo \"rc=$?\"");
    assert_eq!(r.stdout.trim_end(), "rc=127");
    assert!(r.stderr.contains("winget"), "stderr: {:?}", r.stderr);
}

// Found by the agent end-to-end run (Claude Code on windows-2025).

#[test]
#[cfg(windows)]
fn msys_drive_path_runs_as_a_command() {
    let out = stdout_of("\"/c/Windows/System32/cmd.exe\" /d /c echo via-msys-path");
    assert_eq!(out.trim(), "via-msys-path");
}

#[test]
#[cfg(windows)]
fn usr_bin_command_runs_the_command() {
    assert_eq!(stdout_of("/usr/bin/echo usr-bin-ok; /bin/true && echo bin-ok"), "usr-bin-ok\nbin-ok");
}

#[test]
#[cfg(windows)]
fn bundled_utility_errors_name_the_utility_in_plain_english() {
    let r = common::sill("ls /c/no-such-dir-sill-xyz");
    assert!(
        r.stderr.starts_with("ls: cannot access"),
        "want `ls: cannot access ...`, got {:?}",
        r.stderr
    );
}

#[test]
#[cfg(windows)]
fn bundled_utility_accepts_msys_drive_paths() {
    let out = stdout_of(
        "ls /c/ | while IFS= read -r f; do [[ ${f,,} == windows ]] && echo windows; done; \
         ls -d \"/c/Program Files\"",
    );
    let lines: Vec<_> = out.lines().collect();
    assert_eq!(lines.first().map(|l| l.to_lowercase()), Some("windows".into()), "{out}");
    assert!(lines.get(1).is_some_and(|l| l.ends_with("Program Files")), "{out}");
}

#[test]
#[cfg(windows)]
fn command_v_prints_forward_slashes() {
    let out = stdout_of("command -v cmd");
    assert!(!out.contains('\\') && out.to_lowercase().ends_with("cmd.exe"), "{out}");
}

// cmd.exe takes the text after /c nearly raw; CRT-style \" escaping broke the canonical
// `cmd /c 'call "...\vcvars64.bat" && cl ...'` in every environment of the e2e run.

#[test]
#[cfg(windows)]
fn cmd_c_runs_a_whole_command_string_verbatim() {
    let dir = std::env::temp_dir().join("sill cmd test dir");
    std::fs::create_dir_all(&dir).unwrap();
    // (cmd expands %VAR% when it parses the line, before `call` runs, so the batch file
    // reports for itself.)
    std::fs::write(dir.join("marker.bat"), "@echo from-bat\r\n").unwrap();
    let win = dir.to_string_lossy().replace('/', "\\");
    let script = format!(
        "cmd.exe /c 'call \"{win}\\marker.bat\" && cd /d \"{win}\" && echo ok'"
    );
    assert_eq!(stdout_of(&script).replace('\r', ""), "from-bat\nok");
}

#[test]
#[cfg(windows)]
fn cmd_c_with_separate_words_quotes_spaced_ones() {
    let dir = std::env::temp_dir().join("sill cmd words dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("marker.txt"), "x").unwrap();
    let win = dir.to_string_lossy().replace('/', "\\");
    let out = stdout_of(&format!("cmd /d /c dir /b \"{win}\"; cmd.exe /d /c exit 3; echo rc=$?"));
    assert_eq!(out.replace('\r', ""), "marker.txt\nrc=3");
}

#[test]
#[cfg(windows)]
fn bundled_tools_accept_msys_drive_paths() {
    // busybox tools in tools\ are sill's own; `find /c/...` silently found nothing before.
    // (Only meaningful in the packaged layout; skipped when there's no tools dir.)
    let tools = std::path::Path::new(env!("CARGO_BIN_EXE_sill")).with_file_name("tools");
    if !tools.is_dir() {
        return;
    }
    let out = stdout_of("find /c/Windows/System32 -maxdepth 1 -iname cmd.exe");
    assert!(out.to_lowercase().contains("cmd.exe"), "{out}");
}

// Background processes: `$!`, `wait PID`, `kill` (the `server & ...; kill $!` pattern from
// the agent run). Upstream brush left `$!` empty, had no `wait PID`, defaulted `kill` to
// SIGKILL, and had no `kill` at all on Windows.

#[test]
fn background_pid_wait_and_kill() {
    let out = stdout_of(
        "sleep 30 & p=$!; [[ $p =~ ^[0-9]+$ ]] && echo has-pid; kill -0 $p && echo alive; \
         kill $p; wait $p; echo \"rc=$?\"",
    );
    assert_eq!(out.replace('\r', ""), "has-pid\nalive\nrc=143");
}

#[test]
fn wait_for_a_background_pid_returns_its_status() {
    let out = stdout_of("sleep 0.1 & p=$!; wait $p; echo \"rc=$?\"; wait 999999; echo \"rc=$?\"");
    assert_eq!(out.replace('\r', ""), "rc=0\nrc=127");
}

#[test]
#[cfg(windows)]
fn kill_ends_a_windows_process_tree() {
    // cmd.exe wrapping ping: killing the wrapper must not leave ping running.
    let out = stdout_of(
        "cmd.exe /d /c 'ping -n 60 127.0.0.1 >nul' & p=$!; sleep 1; kill $p; wait $p; \
         echo \"rc=$?\"; kill -0 $p 2>/dev/null || echo gone",
    );
    assert_eq!(out.replace('\r', ""), "rc=143\ngone");
}

// Process substitution arguments on Windows are named pipes (no /dev/fd there).

#[test]
#[cfg(windows)]
fn process_substitution_argument_is_readable_by_a_native_program() {
    // (Not cmd's `type`: it looks names up with FindFirstFile, which can't see named
    // pipes. A program that opens the path, as diff and cmp do, is the real case.)
    let out = stdout_of(
        "if command -v python >/dev/null; then \
           python -c 'import sys; print(open(sys.argv[1]).read().strip())' <(printf 'from-psub\\n'); \
         else echo from-psub; fi",
    );
    assert_eq!(out.replace('\r', "").trim(), "from-psub");
}

#[test]
fn process_substitution_redirect_still_works() {
    let out = stdout_of("while IFS= read -r l; do echo \"[$l]\"; done < <(printf 'x\\ny\\n')");
    assert_eq!(out, "[x]\n[y]");
}

// sill's own directory first on PATH, and agent-friendly env defaults (Windows).

#[test]
#[cfg(windows)]
fn bash_on_path_is_sill_itself() {
    let exe_dir = std::path::Path::new(env!("CARGO_BIN_EXE_sill"))
        .parent()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let out = stdout_of("command -v bash").to_lowercase();
    assert!(out.starts_with(&exe_dir), "bash resolves to {out}, want under {exe_dir}");
}

#[test]
#[cfg(windows)]
fn env_defaults_are_set_but_user_values_win() {
    let r = common::run(&["-c", "echo \"$PYTHONUTF8|$MSBUILDDISABLENODEREUSE|${FORCE_COLOR-unset}\""], None);
    assert_eq!(r.stdout.trim(), "1|1|unset");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_sill"));
    let out = cmd.args(["-c", "echo $PYTHONUTF8"]).env("PYTHONUTF8", "0").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0");
}

#[test]
#[cfg(windows)]
fn python_unicode_survives_a_pipe() {
    let out = stdout_of(
        "if command -v python >/dev/null; then python -c \"print('naïve 日本語 €')\" | cat; \
         else echo 'naïve 日本語 €'; fi",
    );
    assert_eq!(out.replace('\r', ""), "naïve 日本語 €");
}

// Windows variable names Cygwin (Git Bash) uppercases are uppercase here too; others keep
// their case, and native programs still find them under any spelling.

#[test]
#[cfg(windows)]
fn windows_variable_names_match_git_bash() {
    let out = stdout_of(
        "for v in COMSPEC SYSTEMROOT WINDIR PROGRAMFILES SYSTEMDRIVE COMMONPROGRAMFILES; do \
         [ -n \"${!v}\" ] && printf '%s ' \"$v\"; done; echo \"${ComSpec-unset} ${ProgramData:+ProgramData}\"",
    );
    assert_eq!(out, "COMSPEC SYSTEMROOT WINDIR PROGRAMFILES SYSTEMDRIVE COMMONPROGRAMFILES unset ProgramData");
    let native = stdout_of("cmd.exe /d /c 'echo %ComSpec%'");
    assert!(native.to_lowercase().trim().ends_with("cmd.exe"), "{native}");
}

// git prints non-ASCII paths as they are (core.quotepath off, via GIT_CONFIG_COUNT), unless
// the user passes git config through the environment themselves.

#[test]
#[cfg(windows)]
fn git_paths_are_utf8() {
    let dir = std::env::temp_dir().join(format!("sill-git-utf8-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("中文.txt"), "x").unwrap();
    let script = "git init -q && git add -A && git ls-files";
    let r = common::run(&["-c", script], Some(&dir));
    assert_eq!(r.stdout.trim(), "中文.txt", "{}", r.stderr);
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_sill"));
    let out = cmd.args(["-c", "git ls-files"]).current_dir(&dir).env("GIT_CONFIG_COUNT", "0").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("\\344"), "user's GIT_CONFIG_COUNT wins");
}

// `>/dev/null` inside text for cmd.exe (Claude Code rewrites `>nul` that way, even inside
// quotes) is cmd's `nul`, not the path \dev\null.

#[test]
#[cfg(windows)]
fn dev_null_redirection_inside_cmd_text() {
    assert_eq!(stdout_of("cmd.exe /d /c 'echo hidden >/dev/null && echo shown'"), "shown");
    assert_eq!(stdout_of("cmd.exe /d /c 'echo \">/dev/null\"'"), "\">/dev/null\"");
}

// msvc-env: Visual Studio's build environment in the shell, cached across shells.

#[test]
#[cfg(windows)]
fn msvc_env_builds_caches_and_hints() {
    let vswhere = std::path::Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.is_file() {
        eprintln!("no Visual Studio; skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sill-msvc-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hello.c"), "#include <stdio.h>\nint main(void){puts(\"hello from msvc\");return 0;}\n").unwrap();

    let r = common::run(&["-c", "msvc-env --refresh && cl /nologo hello.c /Fe:hello.exe >/dev/null && ./hello.exe"], Some(&dir));
    assert_eq!(r.stdout.trim(), "hello from msvc", "{}", r.stderr);
    assert!(r.stderr.contains("msvc-env:") && r.stderr.contains("MSVC 14."), "{}", r.stderr);

    // The next shell uses the cache; running it twice adds nothing twice.
    let r = common::run(&["-c", "msvc-env; a=$PATH; msvc-env; [ \"$a\" = \"$PATH\" ] && echo same; command -v cl"], Some(&dir));
    assert!(r.stderr.contains(", cached"), "{}", r.stderr);
    assert!(r.stdout.starts_with("same\n") && r.stdout.to_lowercase().contains("cl.exe"), "{}", r.stdout);

    // (Shell only: the test binary has no bundled grep beside it.)
    let r = common::run(&["-c", "n=0; while read -r l; do case $l in 'export PATH='*|'export INCLUDE='*) n=$((n+1));; esac; done < <(msvc-env --print 2>/dev/null); echo $n"], Some(&dir));
    assert_eq!(r.stdout.trim(), "2");

    // Without it, `cl` not found says what to do.
    let r = common::run(&["-c", "cl /nologo hello.c"], Some(&dir));
    assert!(r.stderr.contains("command not found") && r.stderr.contains("msvc-env"), "{}", r.stderr);
}

// `#!` scripts run by their interpreter, as on Linux and in Git Bash: ./build.sh, ./gradlew.

#[test]
#[cfg(windows)]
fn shebang_scripts_run() {
    let dir = std::env::temp_dir().join(format!("sill-shebang-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("build.sh"), "#!/bin/sh\necho \"sh $0 $1\"\n").unwrap();
    std::fs::write(dir.join("gradlew"), "#!/usr/bin/env bash\r\necho \"bash-noext ${BASH_VERSION:+bash}\"\r\n").unwrap();
    std::fs::write(dir.join("tool.py"), "#!/usr/bin/env python3\nimport sys; print('py', sys.argv[1])\n").unwrap();
    std::fs::write(dir.join("bin/onpath"), "#!/bin/bash\necho on-path\n").unwrap();
    std::fs::write(dir.join("nobang"), "echo should-not-run\n").unwrap();
    std::fs::write(dir.join("bad"), "#!/usr/bin/env no-such-interpreter-xyz\n").unwrap();
    let script = "./build.sh x; ./gradlew; PATH=\"$PWD/bin:$PATH\" onpath; [ -x ./build.sh ] && echo x-ok; \
                  command -v ./gradlew; ./nobang; echo rc=$?; ./bad; echo rc=$?";
    let r = common::run(&["-c", script], Some(&dir));
    let out: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(&out[..4], ["sh ./build.sh x", "bash-noext bash", "on-path", "x-ok"], "{}\n{}", r.stdout, r.stderr);
    assert_eq!(out[4], "./gradlew");
    // No `#!`: not run (no execute bit to say otherwise), with what to do; a missing
    // interpreter says so.
    assert_eq!(&out[5..], ["rc=126", "rc=126"], "{}", r.stderr);
    assert!(r.stderr.contains("no #! line") && r.stderr.contains("bash ./nobang"), "{}", r.stderr);
    assert!(r.stderr.contains("no-such-interpreter-xyz: bad interpreter"), "{}", r.stderr);
    if std::process::Command::new("python").arg("--version").output().is_ok_and(|o| o.status.success()) {
        let r = common::run(&["-c", "./tool.py arg"], Some(&dir));
        assert_eq!(r.stdout.trim(), "py arg", "{}", r.stderr);
    }
}

// The Store's stand-in python3.exe/python.exe (silent, exit 49/9009) is never the
// `python3` that runs; `command not found` says what Python is called on Windows.

#[test]
#[cfg(windows)]
fn store_python_stub_is_skipped() {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return };
    let stub = std::path::Path::new(&local).join(r"Microsoft\WindowsApps\python3.exe");
    if !stub.metadata().is_ok_and(|m| m.len() == 0) {
        eprintln!("no Store python3 stub here; skipped");
        return;
    }
    let r = common::run(&["-c", "type -a python3 python 2>&1; python3 --version"], None);
    assert!(!r.stdout.to_lowercase().contains("windowsapps/python3"), "{}", r.stdout);
    // One separator style in what `type` prints.
    assert!(!r.stdout.lines().any(|l| l.contains('/') && l.contains('\\')), "{}", r.stdout);
    assert!(
        r.stdout.contains("Python 3") || r.stderr.contains("python3: command not found (on Windows, Python 3 is `python`"),
        "{}\n{}",
        r.stdout,
        r.stderr
    );
}

// lsof: which process has a port or holds a file, and `kill $(lsof -t -i :PORT)`.
// (Shell only: the test binary has no bundled awk beside it.)

#[test]
#[cfg(windows)]
fn lsof_ports_and_files() {
    if !std::process::Command::new("python").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("no python; skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sill-lsof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = r#"
python -m http.server 8799 --bind 127.0.0.1 >/dev/null 2>&1 &
python -c "import time; f = open('held.txt', 'w'); time.sleep(20)" &
holder=$!
for i in $(seq 1 50); do lsof -t -i :8799 >/dev/null && [ -e held.txt ] && break; sleep 0.2; done
lsof -nP -iTCP:8799 -sTCP:LISTEN | while read -r cmd pid type node name rest; do [ "$pid" = PID ] || echo "listen: $cmd $name"; done
lsof held.txt | while read -r cmd pid rest; do [ "$pid" = PID ] || echo "file: $cmd"; done
kill $(lsof -t -i :8799); kill $holder
sleep 0.5
lsof -i :8799; echo "after kill: $?"
lsof -p 1; echo "unsupported: $?"
"#;
    let r = common::run(&["-c", script], Some(&dir));
    let out = r.stdout.replace('\r', "");
    assert!(out.contains("listen: python.exe 127.0.0.1:8799"), "{out}\n{}", r.stderr);
    assert!(out.contains("file: python.exe"), "{out}\n{}", r.stderr);
    assert!(out.contains("after kill: 1"), "{out}");
    assert!(out.contains("unsupported: 2") && r.stderr.contains("-p: not supported on Windows"), "{out}\n{}", r.stderr);
}

// file: line endings, text or binary, what an executable is.

#[test]
#[cfg(windows)]
fn file_identifies_what_agents_ask_about() {
    let dir = std::env::temp_dir().join(format!("sill-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("app.ini"), "[server]\r\nport=8080\r\n").unwrap();
    std::fs::write(dir.join("run.sh"), "#!/usr/bin/env bash\necho hi\n").unwrap();
    let r = common::run(&["-c", "file app.ini run.sh; file -b C:/Windows/System32/cmd.exe; file -i app.ini; file nope; echo rc=$?"], Some(&dir));
    let out = r.stdout.replace('\r', "");
    assert!(out.contains("app.ini: ASCII text, with CRLF line terminators"), "{out}");
    assert!(out.contains("run.sh:  Bourne-Again shell script, ASCII text executable"), "{out}");
    assert!(out.contains("PE32+ executable (console) x86-64, for MS Windows"), "{out}");
    assert!(out.contains("app.ini: text/plain; charset=us-ascii"), "{out}");
    assert!(out.contains("nope: cannot open `nope' (No such file or directory)") && out.ends_with("rc=1\n"), "{out}");
}

// ps / pgrep / pkill / killall, cygpath, nohup: the Linux and Git Bash habits for
// processes and paths.

#[test]
#[cfg(windows)]
fn process_and_path_builtins() {
    let script = r#"
nohup ping -n 30 127.0.0.1 > ping.log 2>&1 &
for i in $(seq 1 50); do pgrep -f "ping -n 30" >/dev/null && break; sleep 0.1; done
echo "pgrep: $(pgrep -f 'ping -n 30' | wc -l)"
ps aux | while read -r user pid rest; do case "$rest" in *"ping -n 30"*) echo "ps-aux-pid-ok";; esac; done | head -1
pkill -f "ping -n 30"; echo "pkill: $?"
sleep 0.3
pgrep -f "ping -n 30"; echo "after: $?"
killall no-such-process-xyz 2>&1; echo "killall-none: $?"
cygpath -w /c/Windows/System32; cygpath -u 'C:\Program Files\Git'; cygpath -m /c/x/y
"#;
    let dir = std::env::temp_dir().join(format!("sill-procs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let r = common::run(&["-c", script], Some(&dir));
    let out = r.stdout.replace('\r', "");
    for want in [
        "pgrep: 1", "ps-aux-pid-ok", "pkill: 0", "after: 1",
        "no-such-process-xyz: no process found", "killall-none: 1",
        "C:\\Windows\\System32", "/c/Program Files/Git", "C:/x/y",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}\n{}", r.stderr);
    }
}
