//! `bash --mcp`: the MCP server, driven over stdio as a client drives it.

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    /// The root the client offers (`roots/list`), if any.
    root: Option<std::path::PathBuf>,
}

impl Client {
    fn start(cwd: &std::path::Path) -> Self {
        Self::start_with_root(cwd, None)
    }

    fn start_with_root(cwd: &std::path::Path, root: Option<&std::path::Path>) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bash"))
            .arg("--mcp")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start bash --mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut c = Self { child, stdin, stdout, next: 0, root: root.map(std::path::Path::to_path_buf) };
        let caps = if root.is_some() { json!({"roots": {}}) } else { json!({}) };
        let info = c.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": caps, "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert_eq!(info["serverInfo"]["name"], "sill");
        c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}));
        c
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "server closed");
            let msg: Value = serde_json::from_str(&line).unwrap();
            // The server asking for the client's roots.
            if msg["method"] == "roots/list" {
                let uri = self.root.as_ref().map(|r| {
                    let p = r.to_string_lossy().replace('\\', "/");
                    if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
                });
                let roots: Vec<Value> = uri.into_iter().map(|u| json!({"uri": u})).collect();
                self.send(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"roots": roots}}));
                continue;
            }
            if msg["id"] == id {
                assert!(msg.get("error").is_none(), "{msg}");
                return msg["result"].clone();
            }
        }
    }

    /// Calls a tool; returns its text and whether it reported an error.
    fn call(&mut self, tool: &str, args: Value) -> (String, bool) {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}));
        let text = r["content"].as_array().unwrap().iter().filter_map(|c| c["text"].as_str()).collect();
        (text, r["isError"].as_bool().unwrap_or(false))
    }

    fn bash(&mut self, command: &str) -> String {
        self.call("bash", json!({"command": command})).0
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn workdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sill-mcp-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn lists_three_tools_with_plain_schemas() {
    let mut c = Client::start(&workdir("list"));
    let tools = c.request("tools/list", json!({}));
    let mut names: Vec<_> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect();
    names.sort();
    assert_eq!(names, ["bash", "bash_kill", "bash_output"]);
    // Integer formats clients' schema checkers know (Gemini's rejects "uint32").
    assert!(!tools.to_string().contains("uint"), "{tools}");
}

#[test]
fn directory_and_exports_carry_over() {
    let mut c = Client::start(&workdir("state"));
    let r = c.bash("mkdir -p sub && cd sub && export SILL_T=kept && LOCAL_T=1");
    assert!(r.contains("--- exit 0 · cwd ") && r.contains("/sub · "), "{r}");
    let r = c.bash("echo \"[$(basename \"$PWD\")] [$SILL_T] [${LOCAL_T-unset}]\"");
    assert!(r.starts_with("[sub] [kept] [unset]\n"), "{r}");
    // However the command ends.
    let r = c.bash("cd .. && exit 3");
    assert!(r.contains("--- exit 3 · cwd ") && !r.contains("/sub · "), "{r}");
    let r = c.bash("unset SILL_T; echo out; echo err >&2");
    assert!(r.starts_with("out\n--- stderr ---\nerr\n--- exit 0"), "{r}");
    assert!(c.bash("echo \"[${SILL_T-gone}]\"").starts_with("[gone]"));
}

#[test]
fn a_commands_own_exit_trap_doesnt_lose_the_state() {
    let mut c = Client::start(&workdir("trap"));
    let r = c.bash("mkdir -p build && cd build && export SILL_T=kept; tmp=$(mktemp -d); trap 'rm -rf \"$tmp\"' EXIT");
    assert!(r.contains("/build · "), "{r}");
    let r = c.bash("echo \"[$(basename \"$PWD\")] [$SILL_T]\"");
    assert!(r.starts_with("[build] [kept]\n"), "{r}");
}

#[test]
fn functions_aliases_and_options_carry_over() {
    let mut c = Client::start(&workdir("funcs"));
    c.bash("greet() { echo \"hi $1\"; }; alias say='echo said'; set -o pipefail; set -e");
    let r = c.bash("greet there; say it; false | true; echo \"pipefail=$?\"; false; echo \"after false\"");
    assert!(r.starts_with("hi there\nsaid it\npipefail=1\nafter false\n"), "{r}");
}

#[test]
fn text_is_utf8_and_plain() {
    let mut c = Client::start(&workdir("text"));
    let r = c.bash("printf '\\033[1;31m中文 ✓\\033[0m\\r\\n'");
    assert!(r.starts_with("中文 ✓\n--- exit 0"), "{r:?}");
}

#[test]
fn a_timeout_ends_the_command_and_what_it_started() {
    let dir = workdir("timeout");
    let mut c = Client::start(&dir);
    let started = Instant::now();
    let r = c.call(
        "bash",
        json!({"command": "bash -c 'sleep 3; echo late > late.txt' & sleep 30", "timeout_ms": 1000}),
    )
    .0;
    assert!(started.elapsed() < Duration::from_secs(10), "{r}");
    assert!(r.contains("stopped after"), "{r}");
    std::thread::sleep(Duration::from_secs(4));
    assert!(!dir.join("late.txt").exists(), "the background program survived the timeout");
}

#[test]
fn a_program_left_running_doesnt_hold_the_call() {
    let mut c = Client::start(&workdir("leftover"));
    let started = Instant::now();
    let r = c.bash("bash -c 'sleep 5' & echo started");
    assert!(r.starts_with("started\n"), "{r}");
    assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
}

#[test]
fn background_jobs() {
    let mut c = Client::start(&workdir("jobs"));
    let (r, _) = c.call("bash", json!({"command": "echo one; sleep 1; echo two", "run_in_background": true}));
    assert!(r.starts_with("started background job "), "{r}");
    let id: u64 = r["started background job ".len()..].split(' ').next().unwrap().parse().unwrap();
    let mut seen = String::new();
    for _ in 0..20 {
        let (r, err) = c.call("bash_output", json!({"id": id, "wait_ms": 1000}));
        assert!(!err, "{r}");
        seen.push_str(&r);
        if r.contains("ended, exit 0") {
            break;
        }
    }
    assert!(seen.contains("one\n") && seen.contains("two\n") && seen.contains("ended, exit 0"), "{seen}");

    let (r, _) = c.call("bash", json!({"command": "sleep 60", "run_in_background": true}));
    let id: u64 = r["started background job ".len()..].split(' ').next().unwrap().parse().unwrap();
    let (r, _) = c.call("bash_kill", json!({"id": id}));
    assert!(r.contains("(stopped)"), "{r}");
    let (r, err) = c.call("bash_output", json!({"id": id}));
    assert!(err && r.contains("no background job"), "{r}");
}

#[cfg(windows)]
#[test]
fn programs_a_command_leaves_running_become_a_job() {
    let mut c = Client::start(&workdir("adopt"));
    // cmd starts ping and exits: the command is done, ping isn't.
    let r = c.bash("cmd //c 'start /b ping -n 30 127.0.0.1 >nul'; echo started");
    assert!(r.starts_with("started\n"), "{r}");
    let marker = "still running: background job ";
    let at = r.find(marker).unwrap_or_else(|| panic!("{r}")) + marker.len();
    let id: u64 = r[at..].split(' ').next().unwrap().parse().unwrap();
    let (r, _) = c.call("bash_kill", json!({"id": id}));
    assert!(r.contains("(stopped)"), "{r}");
}

#[test]
fn starts_in_the_root_the_client_offers() {
    let root = workdir("root-project");
    let mut c = Client::start_with_root(&workdir("root-elsewhere"), Some(&root));
    let r = c.bash("basename \"$PWD\"");
    assert!(r.starts_with(&format!("{}\n", root.file_name().unwrap().to_string_lossy())), "{r}");
}
