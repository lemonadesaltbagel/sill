//! `bash.exe --mcp`: sill as an MCP server (stdio), for harnesses that can't be pointed at a
//! `bash.exe`: Codex with its PowerShell default, Gemini CLI, chat clients.
//!
//! Three tools, as harnesses with a good shell tool have them: `bash` runs a command,
//! `bash_output` reads a background job's new output, `bash_kill` ends one.
//!
//! Each command runs in a fresh `bash.exe` (this program), so a runaway or crashing command
//! can't take the server down, and a timeout ends its whole process tree (a Job Object on
//! Windows). What a shell session would keep carries over between calls: after each command
//! the working directory, the exported environment, and functions, aliases and options are
//! recorded (by the shell itself as it ends, so a command's own EXIT trap can't skip it:
//! brush-shell `session_state`), and the next command starts from them. Unexported
//! variables don't carry over, nor does `set -e`.
//!
//! Output goes to files, not pipes: a program the command leaves running in the background
//! (`server &`) would otherwise hold the pipe open and the call would never end.
//!
//! Every command runs in a Job Object it joins before it starts, so a timeout, `bash_kill`
//! or the server's exit ends everything it started, whatever happened to the processes in
//! between. Programs a foreground command leaves running are adopted as a background job:
//! the result says so, `bash_output` reads them, `bash_kill` stops them, and they end with
//! the session rather than piling up unrecorded.

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
/// Asks a command's shell to save the session's state under the base path it names
/// (brush-shell `session_state`).
pub(crate) const STATE_VAR: &str = "SILL_MCP_STATE";

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_WAIT_MS: u64 = 60_000;
/// Characters of each stream a result shows: the head and, mostly, the tail.
const SHOWN_HEAD: usize = 8_000;
const SHOWN_TAIL: usize = 22_000;

/// The server's instructions; `{tools}` is what [`tools_here`] says.
const INSTRUCTIONS: &str = "\
This Windows machine has sill: native Bash for Windows (no WSL). Run shell commands with \
sill's `bash` tool instead of PowerShell or cmd: Unix commands, scripts and Makefiles work \
there as on Linux ({tools}), and `apply_patch` \
edits files (patch on stdin or as an argument). `C:/x` and `/c/x` paths both work; there is \
no sudo or apt; `msvc-env` loads Visual Studio's C/C++ tools. The working directory and \
exported variables, functions, aliases and options carry over between calls. Start servers and \
watchers with `run_in_background`, read them with `bash_output`, stop them with `bash_kill`.";

/// The `bash` tool's description; `{tools}` is what [`tools_here`] says.
const BASH_DESCRIPTION: &str = "\
Run a command in Bash on this Windows machine (sill, native Bash: {tools}, apply_patch). Use it instead of PowerShell or cmd for shell work. \
Returns stdout, stderr and the exit code. The working directory, exported variables, functions \
and aliases carry over to the next call. Stopped after `timeout_ms` (default 120000, at most 600000) with every \
program it started; for a server or anything long-running, set `run_in_background` to get a \
job id at once.";

/// Programs agents reach for that may or may not be installed.
const OPTIONAL_TOOLS: &[&str] = &["git", "node", "npm", "python", "uv", "cargo", "go", "java", "dotnet", "docker"];

/// The tools a command can use here: sill's own, then those of [`OPTIONAL_TOOLS`] on the
/// PATH, and the rest named as missing, so a model doesn't reach for git on a machine
/// without it.
fn tools_here(on_path: impl Fn(&str) -> bool) -> String {
    let (have, missing): (Vec<&str>, Vec<&str>) = OPTIONAL_TOOLS.iter().partition(|t| on_path(t));
    let mut text = "grep, sed, awk, find, make, rg, jq".to_owned();
    for t in have {
        text.push_str(", ");
        text.push_str(t);
    }
    if !missing.is_empty() {
        text.push_str("; not installed: ");
        text.push_str(&missing.join(", "));
    }
    text
}

/// Whether `name` is a program on the PATH (Windows: with an extension from PATHEXT;
/// the Store's empty `python.exe` stand-in doesn't count).
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).any(|dir| {
        exts.iter().any(|ext| {
            std::fs::metadata(dir.join(format!("{name}{ext}"))).is_ok_and(|m| m.is_file() && m.len() > 0)
        })
    })
}

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
    /// The functions, aliases and options the last command left (shell code).
    state: String,
    /// Whether the starting directory is settled (see `Server::settle_cwd`).
    cwd_settled: bool,
    /// Said once, in the next result: why the commands don't run where the server started.
    note: Option<String>,
}

struct Job {
    /// The `bash.exe` the job started with; `None` for programs adopted from a finished
    /// foreground command.
    child: Option<tokio::process::Child>,
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
    instructions: String,
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

#[tool_router(router = tool_router)]
impl Server {
    fn new() -> std::io::Result<Self> {
        let shell = std::env::current_exe()?;
        let dir = std::env::temp_dir().join("sill-mcp").join(std::process::id().to_string());
        std::fs::create_dir_all(&dir)?;
        let root = std::env::var_os("SILL_MCP_ROOT").map(PathBuf::from).filter(|d| d.is_dir());
        let cwd_settled = root.is_some();
        let cwd = root.map_or_else(std::env::current_dir, Ok)?;
        let tools = tools_here(on_path);
        let mut tool_router = Self::tool_router();
        if let Some(route) = tool_router.map.get_mut("bash") {
            route.attr.description = Some(BASH_DESCRIPTION.replace("{tools}", &tools).into());
        }
        let env = std::env::vars_os()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
            .collect();
        Ok(Self {
            shell,
            dir,
            session: Arc::new(tokio::sync::Mutex::new(Session { cwd, env, state: String::new(), cwd_settled, note: None })),
            jobs: Arc::default(),
            next: Arc::new(AtomicU32::new(1)),
            tool_router,
            instructions: INSTRUCTIONS.replace("{tools}", &tools),
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
        self.settle_cwd(&ctx).await;
        if args.run_in_background {
            return self.start_job(&args.command).await.map_err(io_error);
        }
        // One command at a time: each starts where the last one left off.
        let mut session = self.session.lock().await;
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let files = Files::new(&self.dir, n);
        std::fs::write(&files.cmd, &args.command).map_err(io_error)?;
        std::fs::write(&files.restore, &session.state).map_err(io_error)?;
        std::fs::write(&files.wrapper, wrapper(&files)).map_err(io_error)?;

        let started = Instant::now();
        let (mut child, tree) = self
            .spawn(&session, &files.wrapper, &files.out, &files.err, Some(&files.base))
            .map_err(io_error)?;
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
        if let Ok(state) = std::fs::read_to_string(&files.state) {
            session.state = state;
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
        let mut text = session.note.take().unwrap_or_default();
        text.push_str(&report(&out, &err, &footer));
        // Programs the command left running become a background job of the session.
        let left = if matches!(ended, Ended::Exited(_)) { tree.remaining() } else { 0 };
        if left > 0 {
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            let read = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
            let job = Job {
                child: None,
                tree,
                command: args.command.clone(),
                started,
                out: files.out.clone(),
                err: files.err.clone(),
                out_pos: read(&files.out),
                err_pos: read(&files.err),
            };
            self.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id, job);
            text.push_str(&format!(
                "\n{left} program(s) it started are still running: background job {id} \
                 (bash_output to read them, bash_kill to stop them; they end with this \
                 session). Use run_in_background for servers."
            ));
            files.remove(true, true);
        } else {
            files.remove(out_cut, err_cut);
        }
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
                let running = job.running();
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
        let was_running = job.running();
        job.tree.kill();
        if let Some(child) = job.child.as_mut() {
            let _ = child.wait().await;
        }
        let mut text = job.read_new();
        text.push_str(if was_running { "\n(stopped)" } else { "\n(it had already ended)" });
        let _ = std::fs::remove_file(&job.out);
        let _ = std::fs::remove_file(&job.err);
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

impl Server {
    /// Settles where the first command runs, once: `SILL_MCP_ROOT` if set; else the
    /// client's first root (`roots/list`), when it offers them; else the directory the
    /// client started the server in, unless that's no project's (a client's own install
    /// folder, a system folder), when the home folder is a better start. A chat client like
    /// Claude Desktop starts servers in its own folder.
    #[allow(deprecated, reason = "roots are what clients offer today")]
    async fn settle_cwd(&self, ctx: &RequestContext<RoleServer>) {
        let mut session = self.session.lock().await;
        if session.cwd_settled {
            return;
        }
        session.cwd_settled = true;
        let offers_roots = ctx
            .peer
            .peer_info()
            .is_some_and(|info| info.capabilities.roots.is_some());
        if offers_roots {
            let roots = tokio::time::timeout(Duration::from_secs(3), ctx.peer.list_roots()).await;
            if let Ok(Ok(result)) = roots
                && let Some(dir) = result.roots.iter().find_map(|r| file_uri_path(&r.uri))
                && dir.is_dir()
            {
                session.cwd = dir;
                return;
            }
        }
        if is_no_projects_dir(&session.cwd)
            && let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        {
            let home = PathBuf::from(home);
            session.note = Some(format!(
                "(sill: the client started this server in {}, which is no project's folder, so \
                 commands start in {}; cd to the project, or set SILL_MCP_ROOT)\n",
                display(&session.cwd),
                display(&home)
            ));
            session.cwd = home;
        }
    }

    async fn start_job(&self, command: &str) -> std::io::Result<CallToolResult> {
        let mut session = self.session.lock().await;
        let note = session.note.take().unwrap_or_default();
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let files = Files::new(&self.dir, n);
        std::fs::write(&files.cmd, command)?;
        std::fs::write(&files.restore, &session.state)?;
        std::fs::write(&files.wrapper, wrapper(&files))?;
        let (child, tree) = self.spawn(&session, &files.wrapper, &files.out, &files.err, None)?;
        let pid = child.id().unwrap_or(0);
        let job = Job {
            child: Some(child),
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
            "{note}started background job {n} (pid {pid}) in {}; read its output with bash_output \
             (id {n}), stop it with bash_kill",
            display(&session.cwd)
        ))]))
    }

    /// Starts `bash.exe SCRIPT` in the session's directory and environment, with stdout and
    /// stderr to the given files and no stdin; with `state`, the shell saves the session's
    /// state under that base path as it ends.
    fn spawn(
        &self,
        session: &Session,
        script: &Path,
        out: &Path,
        err: &Path,
        state: Option<&Path>,
    ) -> std::io::Result<(tokio::process::Child, tree::Tree)> {
        let mut cmd = tokio::process::Command::new(&self.shell);
        // Run as `bash -c`, so errors read `bash: line 1: ...` as in a terminal, not with
        // the path of the server's temporary script.
        cmd.arg("--noprofile")
            .arg("--norc")
            .arg("-c")
            .arg(format!(". {}", single_quote(&display(script))))
            .arg("bash");
        cmd.current_dir(&session.cwd);
        cmd.env_clear();
        for (k, v) in &session.env {
            if !matches!(k.as_str(), "PWD" | "SHLVL" | "_") {
                cmd.env(k, v);
            }
        }
        // Java writes into a pipe in the ANSI code page (JEP 400), which loses what that code
        // page can't hold (`??` for Chinese on English Windows); this asks it for UTF-8. Its
        // "Picked up" note is taken out of the output ([`shape`]). A value of the user's own,
        // even an empty one, is left alone.
        if cfg!(windows) && !session.env.iter().any(|(k, _)| k.eq_ignore_ascii_case("JAVA_TOOL_OPTIONS")) {
            cmd.env("JAVA_TOOL_OPTIONS", JAVA_UTF8);
        }
        if let Some(base) = state {
            cmd.env(STATE_VAR, base);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::fs::File::create(out)?);
        cmd.stderr(std::fs::File::create(err)?);
        cmd.kill_on_drop(false);
        tree::spawn(&mut cmd)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("sill", env!("CARGO_PKG_VERSION")))
            .with_instructions(self.instructions.clone())
    }
}

enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

/// A command's files in the server's temporary directory.
struct Files {
    /// What the shell saves the state under (`BASE.cwd`, `BASE.env`, `BASE.state.sh`).
    base: PathBuf,
    cmd: PathBuf,
    wrapper: PathBuf,
    /// The state the command starts with.
    restore: PathBuf,
    out: PathBuf,
    err: PathBuf,
    cwd: PathBuf,
    env: PathBuf,
    state: PathBuf,
}

impl Files {
    fn new(dir: &Path, n: u32) -> Self {
        let f = |ext: &str| dir.join(format!("{n}.{ext}"));
        Self {
            base: dir.join(n.to_string()),
            cmd: f("cmd.sh"),
            wrapper: f("run.sh"),
            restore: f("restore.sh"),
            out: f("out"),
            err: f("err"),
            cwd: f("cwd"),
            env: f("env"),
            state: f("state.sh"),
        }
    }

    /// Removes the files, except an output whose whole text a result points to.
    fn remove(&self, keep_out: bool, keep_err: bool) {
        for (path, keep) in [
            (&self.cmd, false),
            (&self.wrapper, false),
            (&self.restore, false),
            (&self.cwd, false),
            (&self.env, false),
            (&self.state, false),
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
    // The last command's functions, aliases and options, then the command. Aliases work as
    // in an interactive shell; `errexit` isn't carried over (it would end later commands at
    // their first failure). The shell itself saves the state as it ends.
    format!(
        ". {restore} 2>/dev/null\nset +o errexit\nshopt -s expand_aliases\n. {cmd}\n",
        restore = single_quote(&display(&files.restore)),
        cmd = single_quote(&display(&files.cmd)),
    )
}

fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The local path of a `file://` URI (`file:///C:/My%20Project` is `C:/My Project`).
fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///C:/x` on Windows; `file:///home/x` elsewhere; a host part is not ours.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let path = if rest.len() > 2 && rest.as_bytes()[2] == b':' {
        &rest[1..]
    } else {
        rest
    };
    let mut bytes = Vec::with_capacity(path.len());
    let mut it = path.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex: String = [it.next()?, it.next()?].iter().map(|&c| c as char).collect();
            bytes.push(u8::from_str_radix(&hex, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// Whether `dir` is a client's or the system's own folder rather than a place for work: the
/// Windows or Program Files folders, a drive's root, or the folder of the program that
/// started the server (Claude Desktop starts servers in its own install folder). Local
/// AppData as a whole once counted too, which moved a Claude workspace kept there to the
/// home folder.
fn is_no_projects_dir(dir: &Path) -> bool {
    is_no_projects_dir_with(dir, &client_dirs())
}

fn is_no_projects_dir_with(dir: &Path, client_dirs: &[PathBuf]) -> bool {
    let var = |v: &str| std::env::var_os(v).map(|p| comparable(Path::new(&p)));
    let under = |dir: &str, p: &str| !p.is_empty() && (dir == p || dir.starts_with(&format!("{p}/")));
    let dir = comparable(dir);
    let system = ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|v| var(v))
        .any(|p| under(&dir, &p));
    let client = client_dirs.iter().map(|p| comparable(p)).any(|p| under(&dir, &p));
    let temp = ["TEMP", "TMP"].iter().filter_map(|v| var(v)).any(|p| under(&dir, &p));
    // A root (`/`, or `c:` on Windows) is no project's either.
    let root = dir == "/" || (dir.len() == 2 && dir.ends_with(':'));
    ((system || client) && !temp) || root
}

/// The install folders of the programs that started the server (its parent and theirs, a
/// few levels up): each one's folder, or for a Squirrel install (`...\AnthropicClaude\
/// app-1.2.3\claude.exe`) the folder above its versioned one.
fn client_dirs() -> Vec<PathBuf> {
    client_dirs_of(ancestor_exes())
}

fn client_dirs_of(exes: Vec<PathBuf>) -> Vec<PathBuf> {
    exes.into_iter()
        .filter_map(|exe| {
            let dir = exe.parent()?;
            let versioned = dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("app-"));
            Some(if versioned { dir.parent().unwrap_or(dir) } else { dir }.to_path_buf())
        })
        .collect()
}

/// The executables of the server's parent process and its ancestors (up to four).
#[cfg(windows)]
fn ancestor_exes() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let mut parents = HashMap::new();
    // SAFETY: a snapshot walked with a correctly sized entry, then closed.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return vec![];
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            parents.insert(e.th32ProcessID, e.th32ParentProcessID);
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    let mut out = vec![];
    let mut pid = std::process::id();
    for _ in 0..4 {
        let Some(&parent) = parents.get(&pid) else { break };
        if parent == 0 || parent == pid {
            break;
        }
        // SAFETY: the handle is checked and closed; the buffer's size is passed in.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, parent);
            if !h.is_null() {
                let mut buf = vec![0u16; 32768];
                let mut len = u32::try_from(buf.len()).unwrap_or(0);
                if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0 {
                    buf.truncate(len as usize);
                    out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
                }
                CloseHandle(h);
            }
        }
        pid = parent;
    }
    out
}

#[cfg(not(windows))]
const fn ancestor_exes() -> Vec<PathBuf> {
    Vec::new()
}

/// A path to compare with another: its long form where it exists (`RUNNER~1` is
/// `runneradmin`), `/` separators, lowercase.
fn comparable(p: &Path) -> String {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = display(&p);
    let s = s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/');
    if s.is_empty() { "/".to_owned() } else { s.to_lowercase() }
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

/// A stream's bytes as text, line by line: UTF-8, or the system's ANSI code page for a
/// line that isn't. Some programs write that code page into a pipe whatever the
/// console's is: Java since JEP 400 (GBK on Chinese Windows), some MSVC tools.
fn decode(bytes: &[u8]) -> String {
    brush_core::text::decode_output(bytes)
}

/// A stream's text for a result: UTF-8 (invalid bytes replaced), CRLF as LF, no ANSI
/// escapes, and the middle cut out of a long one, with a pointer to the whole file.
/// Returns whether it was cut.
fn shape(bytes: &[u8], file: &Path) -> (String, bool) {
    let text = strip_ansi(&decode(bytes)).replace("\r\n", "\n");
    let text = without_java_note(&text);
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

/// What the server sets `JAVA_TOOL_OPTIONS` to (see [`Server::spawn`]).
const JAVA_UTF8: &str = "-Dstdout.encoding=UTF-8 -Dstderr.encoding=UTF-8";

/// `text` without the note each JVM prints for the server's own `JAVA_TOOL_OPTIONS`.
fn without_java_note(text: &str) -> String {
    let note = format!("Picked up JAVA_TOOL_OPTIONS: {JAVA_UTF8}");
    if !text.contains(&note) {
        return text.to_owned();
    }
    text.split_inclusive('\n')
        .filter(|line| line.trim_end_matches('\n') != note)
        .collect()
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
    /// Whether anything the job started still runs (its `bash.exe`, or what it left).
    fn running(&mut self) -> bool {
        let shell_running = self
            .child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        shell_running || self.tree.remaining() > 0
    }

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
        let state = if self.running() {
            format!("running for {secs} s")
        } else {
            match self.child.as_mut().map(tokio::process::Child::try_wait) {
                Some(Ok(Some(status))) => format!(
                    "ended, exit {}",
                    status.code().map_or_else(|| "?".into(), |c| c.to_string())
                ),
                _ => "ended".into(),
            }
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

/// A command's process tree: on Windows a Job Object the command joins before it starts
/// (brush_core::sys::job), so ending it ends every program the command started; elsewhere
/// a process group.
mod tree {
    #[cfg(windows)]
    pub(super) struct Tree(std::sync::Arc<brush_core::sys::job::Job>);

    /// Starts `cmd` in a new tree whose programs all end when it's dropped (at the latest,
    /// when the server exits).
    #[cfg(windows)]
    pub(super) fn spawn(
        cmd: &mut tokio::process::Command,
    ) -> std::io::Result<(tokio::process::Child, Tree)> {
        use brush_core::sys::job;
        cmd.creation_flags(job::SPAWN_FLAGS);
        let tree = std::sync::Arc::new(job::Job::new(true)?);
        let (child, _) = job::start(cmd, Some(&tree))?;
        Ok((child, Tree(tree)))
    }

    #[cfg(windows)]
    impl Tree {
        pub(super) fn kill(&self) {
            self.0.terminate(1);
        }

        /// How many of its programs still run.
        pub(super) fn remaining(&self) -> u32 {
            self.0.active_processes()
        }
    }

    #[cfg(not(windows))]
    pub(super) struct Tree(Option<u32>);

    #[cfg(not(windows))]
    pub(super) fn spawn(
        cmd: &mut tokio::process::Command,
    ) -> std::io::Result<(tokio::process::Child, Tree)> {
        cmd.process_group(0);
        let child = cmd.spawn()?;
        let tree = Tree(child.id());
        Ok((child, tree))
    }

    #[cfg(not(windows))]
    impl Tree {
        pub(super) fn kill(&self) {
            if let Some(pgid) = self.0 {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", "--", &format!("-{pgid}")])
                    .status();
            }
        }

        /// Not tracked here.
        pub(super) const fn remaining(&self) -> u32 {
            0
        }
    }

    #[cfg(not(windows))]
    impl Drop for Tree {
        fn drop(&mut self) {}
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
    fn the_java_note_is_left_out() {
        let note = format!("Picked up JAVA_TOOL_OPTIONS: {JAVA_UTF8}\n");
        let (text, _) = shape(format!("{note}err: x\n{note}").as_bytes(), Path::new("f"));
        assert_eq!(text, "err: x\n");
        // Someone else's options are theirs to see.
        let theirs = "Picked up JAVA_TOOL_OPTIONS: -Xmx1g\n";
        assert_eq!(shape(theirs.as_bytes(), Path::new("f")).0, theirs);
    }

    #[test]
    fn the_temporary_folder_is_a_place_for_work() {
        assert!(!is_no_projects_dir(&std::env::temp_dir()));
        assert!(!is_no_projects_dir(&std::env::temp_dir().join("project")));
        assert!(is_no_projects_dir(Path::new("/")));
        #[cfg(windows)]
        {
            let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
            assert!(is_no_projects_dir(&windows));
            assert!(is_no_projects_dir(&windows.join("System32")));
        }
    }

    #[test]
    fn the_client_folder_is_no_project() {
        // Paths that don't exist compare as given.
        let clients = client_dirs_of(vec![
            PathBuf::from("/fake/AppData/Local/AnthropicClaude/app-1.2.3/claude.exe"),
            PathBuf::from("/fake/tools/editor/editor.exe"),
        ]);
        let no_project = |d: &str| is_no_projects_dir_with(Path::new(d), &clients);
        assert!(no_project("/fake/AppData/Local/AnthropicClaude"), "above a Squirrel app-* folder");
        assert!(no_project("/fake/AppData/Local/AnthropicClaude/app-1.2.3"));
        assert!(no_project("/fake/tools/editor"));
        assert!(!no_project("/fake/AppData/Local/Claude-Data/workspace"), "a workspace in Local AppData");
        assert!(!no_project("/fake/tools"));
    }

    #[test]
    fn the_tools_named_are_the_ones_here() {
        let text = tools_here(|t| t == "git" || t == "node");
        assert_eq!(
            text,
            "grep, sed, awk, find, make, rg, jq, git, node; not installed: npm, python, uv, cargo, go, java, dotnet, docker"
        );
        assert!(!BASH_DESCRIPTION.replace("{tools}", &text).contains("{tools}"));
        assert!(INSTRUCTIONS.contains("{tools}"));
    }

    #[test]
    fn file_uris() {
        assert_eq!(file_uri_path("file:///C:/My%20Project"), Some(PathBuf::from("C:/My Project")));
        assert_eq!(file_uri_path("file:///home/me/x"), Some(PathBuf::from("/home/me/x")));
        assert_eq!(file_uri_path("https://x"), None);
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
