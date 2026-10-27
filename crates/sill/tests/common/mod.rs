//! Shared helpers: run the real `sill` binary with a timeout, so a hang fails the test.
#![allow(dead_code, reason = "each test file uses a different subset")]

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Run {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

/// Command lines are capped at 32,767 UTF-16 units on Windows; longer scripts go
/// through a file, as a harness would have to.
const MAX_DASH_C: usize = 30_000;

pub fn sill(script: &str) -> Run {
    if script.len() <= MAX_DASH_C {
        return run(&["--norc", "--noprofile", "-c", script], None);
    }
    let path = std::env::temp_dir().join(format!(
        "sill-test-{}-{:x}.sh",
        std::process::id(),
        script.len()
    ));
    std::fs::write(&path, script).expect("write script file");
    let r = run(&["--norc", "--noprofile", path.to_str().expect("utf-8 temp path")], None);
    let _ = std::fs::remove_file(&path);
    r
}

/// Runs `sill` with exactly `args` (as a harness would), optionally in `cwd`.
pub fn run(args: &[&str], cwd: Option<&std::path::Path>) -> Run {
    run_within(args, cwd, TIMEOUT)
}

/// As [`run`], for something slow by nature (Visual Studio's `vcvarsall`).
pub fn run_within(args: &[&str], cwd: Option<&std::path::Path>, timeout: Duration) -> Run {
    let script = args.join(" ");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sill"));
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sill");
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            panic!("sill timed out after {timeout:?} running: {}", &script[..script.len().min(200)]);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Run {
        stdout: out_t.join().unwrap(),
        stderr: err_t.join().unwrap(),
        code: status.code(),
    }
}

pub fn stdout_of(script: &str) -> String {
    let r = sill(script);
    assert_eq!(r.code, Some(0), "script failed; stderr: {}", r.stderr);
    r.stdout.trim_end_matches(['\r', '\n']).to_owned()
}

