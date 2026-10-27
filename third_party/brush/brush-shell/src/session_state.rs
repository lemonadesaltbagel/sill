//! sill: the state an MCP session carries from one command to the next.
//!
//! sill's MCP server runs each command in a fresh shell. When asked to ([`save_on_exit`]),
//! the shell records, as it ends, where it is and what the next command should start with:
//! `BASE.cwd` (the working directory), `BASE.env` (the exported environment,
//! `NAME=value` entries separated by NUL) and `BASE.state.sh` (functions, aliases and
//! options, as `declare -f`, `alias -p`, `shopt -p` and `set +o` print them). It runs
//! after the script and its EXIT trap, in the shell itself, so a script's own
//! `trap ... EXIT` can't replace it (as it did a trap the server set).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static BASE: OnceLock<PathBuf> = OnceLock::new();

/// Asks the shell to save its state under `base` when it ends.
pub fn save_on_exit(base: PathBuf) {
    let _ = BASE.set(base);
}

fn with_suffix(base: &Path, suffix: &str) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Saves the state, if asked to. Failures are silent: the next command then starts where
/// this one did.
pub(crate) async fn save_if_asked(shell: &mut brush_core::Shell<impl brush_core::ShellExtensions>) {
    let Some(base) = BASE.get() else {
        return;
    };
    let _ = std::fs::write(
        with_suffix(base, ".cwd"),
        shell.working_dir().to_string_lossy().as_bytes(),
    );
    let mut env = Vec::new();
    for (name, value) in shell.exported_environment() {
        env.extend_from_slice(name.as_bytes());
        env.push(b'=');
        env.extend_from_slice(value.as_bytes());
        env.push(0);
    }
    let _ = std::fs::write(with_suffix(base, ".env"), env);

    let target = with_suffix(base, ".state.sh").to_string_lossy().replace('\\', "/").replace('\'', "'\\''");
    let script = format!(
        "{{ builtin declare -f; builtin alias -p; builtin shopt -p; builtin set +o; }} >| '{target}' 2>/dev/null"
    );
    let params = shell.default_exec_params();
    let _ = shell
        .run_string(script, &brush_core::SourceInfo::from("sill-mcp-state"), &params)
        .await;
}
