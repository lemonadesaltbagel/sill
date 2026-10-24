//! `bash.exe --mcp`: sill as an MCP server (stdio), for harnesses that can't be pointed at a
//! `bash.exe`: Codex with its PowerShell default, Gemini CLI, chat clients.
//!
//! Three tools, as harnesses with a good shell tool have them: `bash` runs a command,
//! `bash_output` reads a background job's new output, `bash_kill` ends one.
//!
//! Each command runs in a fresh `bash.exe` (this program), so a runaway or crashing command
//! can't take the server down, and a timeout ends its whole process tree (a Job Object on
//! Windows). What a shell session would keep carries over between calls: after each command
//! the working directory and the exported environment are recorded, and the next command
//! starts from them. Functions, aliases and unexported variables don't carry over.
//!
//! Output goes to files, not pipes: a program the command leaves running in the background
//! (`server &`) would otherwise hold the pipe open and the call would never end.

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler, ServiceExt as _, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

/// The argument that makes sill an MCP server. (An option, so that it can't be a script's
/// name: `bash mcp` runs the script `mcp`.)
pub(crate) const ARG: &str = "--mcp";

/// The hidden argument with which a command's wrapper records its exported environment
/// (see `wrapper`): `bash.exe --sill-mcp-env FILE` writes its own environment there.
pub(crate) const ENV_DUMP_ARG: &str = "--sill-mcp-env";

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_WAIT_MS: u64 = 60_000;
/// Characters of each stream a result shows: the head and, mostly, the tail.
const SHOWN_HEAD: usize = 8_000;
const SHOWN_TAIL: usize = 22_000;

const INSTRUCTIONS: &str = "\
This Windows machine has sill: native Bash for Windows (no WSL). Run shell commands with \
sill's `bash` tool instead of PowerShell or cmd: Unix commands, scripts and Makefiles work \
there as on Linux (grep, sed, awk, find, make, rg, jq, git, npm, python), and `apply_patch` \
edits files (patch on stdin or as an argument). `C:/x` and `/c/x` paths both work; there is \
no sudo or apt; `msvc-env` loads Visual Studio's C/C++ tools. The working directory and \
exported variables carry over between calls; functions and aliases don't. Start servers and \
watchers with `run_in_background`, read them with `bash_output`, stop them with `bash_kill`.";

const BASH_DESCRIPTION: &str = "\
Run a command in Bash on this Windows machine (sill, native Bash: grep, sed, awk, find, make, \
rg, jq, git, npm, python, apply_patch). Use it instead of PowerShell or cmd for shell work. \
Returns stdout, stderr and the exit code. The working directory and exported variables carry \
over to the next call. Stopped after `timeout_ms` (default 120000, at most 600000) with every \
program it started; for a server or anything long-running, set `run_in_background` to get a \
job id at once.";

/// Arguments of `bash`.
#[derive(Deserialize, JsonSchema)]
pub struct BashArgs {
    /// The command, in Bash.
    pub command: String,
    /// Milliseconds before the command is stopped (default 120000, at most 600000).
    #[serde(default = "default_timeout")]
    #[schemars(default = "default_timeout", with = "i64")]
    pub timeout_ms: u64,
    /// Start the command as a background job and return its id at once.
    #[serde(default)]
    pub run_in_background: bool,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// Arguments of `bash_output`.
#[derive(Deserialize, JsonSchema)]
pub struct OutputArgs {
    /// The job id `bash` returned.
    #[schemars(with = "i64")]
    pub id: u32,
    /// Wait up to this many milliseconds for new output or for the job to end (default 0,
    /// at most 60000).
    #[serde(default)]
    #[schemars(with = "i64")]
    pub wait_ms: u64,
}

/// Arguments of `bash_kill`.
#[derive(Deserialize, JsonSchema)]
pub struct KillArgs {
    /// The job id `bash` returned.
    #[schemars(with = "i64")]
    pub id: u32,
}

/// What carries over from one command to the next.
struct Session {
    cwd: PathBuf,
    env: Vec<(String, String)>,
}

struct Job {
    child: tokio::process::Child,
    tree: tree::Tree,
    command: String,
    started: Instant,
    out: PathBuf,
    err: PathBuf,
    out_pos: u64,
    err_pos: u64,
}

/// The server.
#[derive(Clone)]
pub struct Server {
    shell: PathBuf,
    dir: PathBuf,
    session: Arc<tokio::sync::Mutex<Session>>,
    jobs: Arc<std::sync::Mutex<HashMap<u32, Job>>>,
    next: Arc<AtomicU32>,
    tool_router: ToolRouter<Self>,
}

/// Runs the server on stdin and stdout until the client goes away, then exits.
pub(crate) fn serve() -> ! {
    let code = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt.block_on(async {
            let server = match Server::new() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("sill mcp: {e}");
                    return 1;
                }
            };
            let dir = server.dir.clone();
            let code = match server.serve(rmcp::transport::stdio()).await {
                Ok(running) => i32::from(running.waiting().await.is_err()),
                Err(e) => {
                    eprintln!("sill mcp: {e}");
                    1
                }
            };
            let _ = std::fs::remove_dir_all(dir);
            code
        }),
        Err(e) => {
            eprintln!("sill mcp: {e}");
            1
        }
    };
    // Background jobs end with the server: their Job Objects close with the process.
    std::process::exit(code)
}

/// `bash.exe --sill-mcp-env FILE`: writes this process's environment (the shell's exported
/// variables, as the shell passed them) to FILE as NUL-separated `NAME=value` entries.
pub(crate) fn dump_env(file: &std::ffi::OsStr) -> ! {
    let mut out = Vec::new();
    for (k, v) in std::env::vars_os() {
        out.extend_from_slice(k.to_string_lossy().as_bytes());
        out.push(b'=');
        out.extend_from_slice(v.to_string_lossy().as_bytes());
        out.push(0);
    }
    let code = i32::from(std::fs::write(file, out).is_err());
    std::process::exit(code)
}

#[tool_router(router = tool_router)]
impl Server {
    fn new() -> std::io::Result<Self> {
        let shell = std::env::current_exe()?;
        let dir = std::env::temp_dir().join("sill-mcp").join(std::process::id().to_string());
        std::fs::create_dir_all(&dir)?;
        let cwd = std::env::var_os("SILL_MCP_ROOT")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .map_or_else(std::env::current_dir, Ok)?;
        let env = std::env::vars_os()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
            .collect();
        Ok(Self {
            shell,
            dir,
            session: Arc::new(tokio::sync::Mutex::new(Session { cwd, env })),
            jobs: Arc::default(),
            next: Arc::new(AtomicU32::new(1)),
            tool_router: Self::tool_router(),
        })
    }

    #[tool(
        description = BASH_DESCRIPTION,
        annotations(title = "Bash (sill)", read_only_hint = false, destructive_hint = true, open_world_hint = true)
    )]
    async fn bash(
        &self,
        params: Parameters<BashArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = params.0;
        if args.run_in_background {
            return self.start_job(&args.command).await.map_err(io_error);
        }
        // One command at a time: each starts where the last one left off.
        let mut session = self.session.lock().await;
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let files = Files::new(&self.dir, n);
        std::fs::write(&files.cmd, &args.command).map_err(io_error)?;
        std::fs::write(&files.wrapper, wrapper(&files)).map_err(io_error)?;

        let started = Instant::now();
        let (mut child, tree) =
            self.spawn(&session, &files.wrapper, &files.out, &files.err, false).map_err(io_error)?;
        let timeout = Duration::from_millis(args.timeout_ms.clamp(1, MAX_TIMEOUT_MS));
        let ended = tokio::select! {
            status = child.wait() => Ended::Exited(status.map_err(io_error)?.code()),
            () = tokio::time::sleep(timeout) => Ended::TimedOut,
            () = ctx.ct.cancelled() => Ended::Cancelled,
        };
        if !matches!(ended, Ended::Exited(_)) {
            tree.kill();
            let _ = child.wait().await;
        }
        let secs = started.elapsed().as_secs_f64();

        // Carry the state over, if the command got as far as recording it.
        if let Ok(cwd) = std::fs::read_to_string(&files.cwd) {
            let cwd = PathBuf::from(cwd.trim_end_matches(['\r', '\n']));
            if cwd.is_dir() {
                session.cwd = cwd;
            }
        }
        if let Ok(env) = std::fs::read(&files.env) {
            let env = parse_env(&env);
            if !env.is_empty() {
                session.env = env;
            }
        }

        let out = std::fs::read(&files.out).unwrap_or_default();
        let err = std::fs::read(&files.err).unwrap_or_default();
        let (out, out_cut) = shape(&out, &files.out);
        let (err, err_cut) = shape(&err, &files.err);
        let footer = match ended {
            Ended::Exited(code) => format!(
                "exit {} · cwd {} · {secs:.1} s",
                code.map_or_else(|| "?".into(), |c| c.to_string()),
                display(&session.cwd)
            ),
            Ended::TimedOut => format!(
                "stopped after {:.0} s (timeout_ms {}), with the programs it started; for \
                 something long-running use run_in_background",
                secs, args.timeout_ms
            ),
            Ended::Cancelled => "cancelled; the command was stopped".into(),
        };
        let text = report(&out, &err, &footer);
        files.remove(out_cut, err_cut);
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(
        description = "Read a background job's output since the last read, and whether it is still running.",
        annotations(title = "Bash job output (sill)", read_only_hint = true, open_world_hint = false)
    )]
    async fn bash_output(&self, params: Parameters<OutputArgs>) -> Result<CallToolResult, ErrorData> {
        let args = params.0;
        let deadline = Instant::now() + Duration::from_millis(args.wait_ms.min(MAX_WAIT_MS));
        loop {
            let ready = {
                let mut jobs = self.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(job) = jobs.get_mut(&args.id) else {
                    return Ok(unknown_job(args.id, &jobs));
                };
                let running = matches!(job.child.try_wait(), Ok(None));
                let fresh = job.has_new_output();
                if !running || fresh || Instant::now() >= deadline {
                    Some(job.read_new())
                } else {
                    None
                }
            };
            if let Some(text) = ready {
                return Ok(CallToolResult::success(vec![ContentBlock::text(text)]));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    #[tool(
        description = "Stop a background job and every program it started.",
        annotations(title = "Stop a Bash job (sill)", read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn bash_kill(&self, params: Parameters<KillArgs>) -> Result<CallToolResult, ErrorData> {
        let id = params.0.id;
        let job = {
            let mut jobs = self.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match jobs.remove(&id) {
                Some(job) => job,
                None => return Ok(unknown_job(id, &jobs)),
            }
        };
        let mut job = job;
        let was_running = matches!(job.child.try_wait(), Ok(None));
        job.tree.kill();
        let _ = job.child.wait().await;
        let mut text = job.read_new();
        text.push_str(if was_running { "\n(stopped)" } else { "\n(it had already ended)" });
        let _ = std::fs::remove_file(&job.out);
        let _ = std::fs::remove_file(&job.err);
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

impl Server {
    async fn start_job(&self, command: &str) -> std::io::Result<CallToolResult> {
        let session = self.session.lock().await;
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let files = Files::new(&self.dir, n);
        std::fs::write(&files.cmd, command)?;
        let (child, tree) = self.spawn(&session, &files.cmd, &files.out, &files.err, true)?;
        let pid = child.id().unwrap_or(0);
        let job = Job {
            child,
            tree,
            command: command.to_owned(),
            started: Instant::now(),
            out: files.out.clone(),
            err: files.err.clone(),
            out_pos: 0,
            err_pos: 0,
        };
        self.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(n, job);
        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "started background job {n} (pid {pid}) in {}; read its output with bash_output \
             (id {n}), stop it with bash_kill",
            display(&session.cwd)
        ))]))
    }

    /// Starts `bash.exe SCRIPT` in the session's directory and environment, with stdout and
    /// stderr to the given files and no stdin.
    fn spawn(
        &self,
        session: &Session,
        script: &Path,
        out: &Path,
        err: &Path,
        background: bool,
    ) -> std::io::Result<(tokio::process::Child, tree::Tree)> {
        let mut cmd = tokio::process::Command::new(&self.shell);
        cmd.arg("--noprofile").arg("--norc").arg(script);
        cmd.current_dir(&session.cwd);
        cmd.env_clear();
        for (k, v) in &session.env {
            if !matches!(k.as_str(), "PWD" | "SHLVL" | "_") {
                cmd.env(k, v);
            }
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::fs::File::create(out)?);
        cmd.stderr(std::fs::File::create(err)?);
        cmd.kill_on_drop(false);
        tree::prepare(&mut cmd);
        let child = cmd.spawn()?;
        // A background job ends with the server; a foreground command's leftovers
        // (`server &`) keep running, as they would in a terminal.
        let tree = tree::Tree::new(&child, background)?;
        Ok((child, tree))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("sill", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

/// A command's files in the server's temporary directory.
struct Files {
    cmd: PathBuf,
    wrapper: PathBuf,
    out: PathBuf,
    err: PathBuf,
    cwd: PathBuf,
    env: PathBuf,
}

impl Files {
    fn new(dir: &Path, n: u32) -> Self {
        let f = |ext: &str| dir.join(format!("{n}.{ext}"));
        Self {
            cmd: f("cmd.sh"),
            wrapper: f("run.sh"),
            out: f("out"),
            err: f("err"),
            cwd: f("cwd"),
            env: f("env"),
        }
    }

    /// Removes the files, except an output whose whole text a result points to.
    fn remove(&self, keep_out: bool, keep_err: bool) {
        for (path, keep) in [
            (&self.cmd, false),
            (&self.wrapper, false),
            (&self.cwd, false),
            (&self.env, false),
            (&self.out, keep_out),
            (&self.err, keep_err),
        ] {
            if !keep {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// The script a foreground command runs as: the command itself (sourced, so `cd` and
/// `export` act on this shell), and an EXIT trap that records where it ended up, however it
/// ends (`exit`, `set -e`, the last command).
fn wrapper(files: &Files) -> String {
    let q = |p: &Path| single_quote(&display(p));
    format!(
        "__sill_mcp_cwd={cwd}\n__sill_mcp_env={env}\n__sill_mcp_exe={exe}\n\
         trap '__sill_mcp_status=$?; pwd -P >| \"$__sill_mcp_cwd\" 2>/dev/null; \
         \"$__sill_mcp_exe\" {dump} \"$__sill_mcp_env\" 2>/dev/null; exit $__sill_mcp_status' EXIT\n\
         . {cmd}\n",
        cwd = q(&files.cwd),
        env = q(&files.env),
        exe = q(&std::env::current_exe().unwrap_or_default()),
        dump = ENV_DUMP_ARG,
        cmd = q(&files.cmd),
    )
}

fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A path as the shell shows it: `/` separators.
fn display(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `NAME=value` entries, NUL-separated. (Windows' hidden `=C:=C:\x` names start with `=`.)
fn parse_env(bytes: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter_map(|entry| {
            let i = entry.get(1..)?.find('=')? + 1;
            Some((entry[..i].to_owned(), entry[i + 1..].to_owned()))
        })
        .collect()
}

/// A stream's text for a result: UTF-8 (invalid bytes replaced), CRLF as LF, no ANSI
/// escapes, and the middle cut out of a long one, with a pointer to the whole file.
/// Returns whether it was cut.
fn shape(bytes: &[u8], file: &Path) -> (String, bool) {
    let text = strip_ansi(&String::from_utf8_lossy(bytes)).replace("\r\n", "\n");
    let count = text.chars().count();
    if count <= SHOWN_HEAD + SHOWN_TAIL {
        return (text, false);
    }
    let head: String = text.chars().take(SHOWN_HEAD).collect();
    let tail: String = text.chars().skip(count - SHOWN_TAIL).collect();
    let cut = count - SHOWN_HEAD - SHOWN_TAIL;
    (
        format!(
            "{head}\n[... {cut} characters left out; the whole output is in {} ...]\n{tail}",
            display(file)
        ),
        true,
    )
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then a final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn report(out: &str, err: &str, footer: &str) -> String {
    let mut text = String::new();
    text.push_str(out);
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("--- stderr ---\n");
        text.push_str(err);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("--- ");
    text.push_str(footer);
    text.push_str(" ---");
    text
}

fn unknown_job(id: u32, jobs: &HashMap<u32, Job>) -> CallToolResult {
    let mut ids: Vec<_> = jobs.keys().map(u32::to_string).collect();
    ids.sort();
    let known = if ids.is_empty() { "none".into() } else { ids.join(", ") };
    CallToolResult::error(vec![ContentBlock::text(format!(
        "no background job {id} (running or unread jobs: {known})"
    ))])
}

impl Job {
    fn has_new_output(&self) -> bool {
        let len = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
        len(&self.out) > self.out_pos || len(&self.err) > self.err_pos
    }

    /// The output since the last read, and the job's state.
    fn read_new(&mut self) -> String {
        let out = read_from(&self.out, &mut self.out_pos);
        let err = read_from(&self.err, &mut self.err_pos);
        let (out, _) = shape(&out, &self.out);
        let (err, _) = shape(&err, &self.err);
        let secs = self.started.elapsed().as_secs();
        let state = match self.child.try_wait() {
            Ok(None) => format!("running for {secs} s"),
            Ok(Some(status)) => format!(
                "ended, exit {}",
                status.code().map_or_else(|| "?".into(), |c| c.to_string())
            ),
            Err(e) => format!("unknown ({e})"),
        };
        let command: String = self.command.chars().take(80).collect();
        report(&out, &err, &format!("job: {command} · {state}"))
    }
}

fn read_from(path: &Path, pos: &mut u64) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Ok(mut f) = std::fs::File::open(path)
        && f.seek(SeekFrom::Start(*pos)).is_ok()
        && f.read_to_end(&mut buf).is_ok()
    {
        *pos += buf.len() as u64;
    }
    buf
}

fn io_error(e: std::io::Error) -> ErrorData {
    ErrorData::internal_error(format!("sill: {e}"), None)
}

/// A command's process tree: on Windows a Job Object, so that ending it ends every program
/// the command started; elsewhere a process group.
mod tree {
    #[cfg(windows)]
    pub(super) struct Tree(isize);

    #[cfg(windows)]
    pub(super) fn prepare(_cmd: &mut tokio::process::Command) {}

    #[cfg(windows)]
    impl Tree {
        /// Puts `child` in a new Job Object; with `kill_on_close`, everything in it ends when
        /// the handle closes (at the latest, when the server exits).
        pub(super) fn new(child: &tokio::process::Child, kill_on_close: bool) -> std::io::Result<Self> {
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            };
            // SAFETY: no name, default security; checked below.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let tree = Self(job as isize);
            if kill_on_close {
                // SAFETY: a zeroed limit structure is valid; only the flag is set.
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                // SAFETY: `job` is valid; `info` is the structure the class names.
                unsafe {
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        (&raw const info).cast(),
                        u32::try_from(std::mem::size_of_val(&info)).unwrap_or(0),
                    )
                };
            }
            if let Some(handle) = child.raw_handle() {
                // SAFETY: both handles are valid. (Fails only if the process already ended.)
                unsafe { AssignProcessToJobObject(job, handle.cast()) };
            }
            Ok(tree)
        }

        pub(super) fn kill(&self) {
            // SAFETY: a Job Object handle this owns.
            unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1) };
        }
    }

    #[cfg(windows)]
    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: a handle this owns, closed once.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0 as _) };
        }
    }

    #[cfg(not(windows))]
    pub(super) struct Tree(Option<u32>);

    #[cfg(not(windows))]
    pub(super) fn prepare(cmd: &mut tokio::process::Command) {
        cmd.process_group(0);
    }

    #[cfg(not(windows))]
    impl Tree {
        pub(super) fn new(child: &tokio::process::Child, _kill_on_close: bool) -> std::io::Result<Self> {
            Ok(Self(child.id()))
        }

        pub(super) fn kill(&self) {
            if let Some(pgid) = self.0 {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", "--", &format!("-{pgid}")])
                    .status();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_entries() {
        let env = parse_env(b"A=1\0=C:=C:\\x\0B=x=y\0\0");
        assert_eq!(
            env,
            [("A".into(), "1".into()), ("=C:".into(), "C:\\x".into()), ("B".into(), "x=y".into())]
        );
    }

    #[test]
    fn output_is_shaped() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m \x1b]0;title\x07ok"), "red ok");
        let (text, cut) = shape(b"a\r\nb\r\n", Path::new("f"));
        assert_eq!((text.as_str(), cut), ("a\nb\n", false));
        let long = "x".repeat(SHOWN_HEAD + SHOWN_TAIL + 5);
        let (text, cut) = shape(long.as_bytes(), Path::new("C:\\t\\out"));
        assert!(cut);
        assert!(text.contains("5 characters left out; the whole output is in C:/t/out"), "{text}");
    }

    #[test]
    fn report_layout() {
        assert_eq!(report("hi\n", "", "exit 0"), "hi\n--- exit 0 ---");
        assert_eq!(report("hi", "oops\n", "exit 1"), "hi\n--- stderr ---\noops\n--- exit 1 ---");
        assert_eq!(report("", "", "exit 0"), "--- exit 0 ---");
    }

    #[test]
    fn quoting() {
        assert_eq!(single_quote("C:/it's"), r"'C:/it'\''s'");
    }
}
