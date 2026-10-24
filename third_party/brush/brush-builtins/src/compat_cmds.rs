//! sill: small Windows builtins that scripts and models written for Git Bash expect.
//!
//! - `cygpath [-u|-w|-m] [-a] [-p] PATH...`: converts between the shell's `/c/...` form,
//!   Windows `C:\...` and mixed `C:/...`, as Git Bash's cygpath does. Models write
//!   `"$(cygpath -w "$PWD")"` to hand a path to a native program.
//! - `nohup COMMAND...` and `setsid [-f] COMMAND...`: run COMMAND. Windows has no hangup
//!   signal and no sessions, so there is nothing to ignore or detach from; a background
//!   job (`nohup server > log 2>&1 &`) already outlives the shell.

use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionExitCode, ExecutionResult, builtins, commands};

/// Convert paths between the shell's (`/c/...`), Windows (`C:\...`) and mixed forms.
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub(crate) struct CygpathCommand {
    /// The shell's form (`/c/Users/x`); the default.
    #[arg(short = 'u', long = "unix")]
    unix: bool,
    /// Windows form (`C:\Users\x`).
    #[arg(short = 'w', long = "windows")]
    windows: bool,
    /// Mixed form (`C:/Users/x`).
    #[arg(short = 'm', long = "mixed")]
    mixed: bool,
    /// Make the path absolute.
    #[arg(short = 'a', long = "absolute")]
    absolute: bool,
    /// The argument is a path list (`:` or `;` separated).
    #[arg(short = 'p', long = "path")]
    path_list: bool,
    paths: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(CygpathCommand);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Form {
    Unix,
    Windows,
    Mixed,
}

/// A Windows (or mixed) path in the shell's form: `C:\a\b` → `/c/a/b`, `\\srv\s` → `//srv/s`.
fn to_unix(p: &str) -> String {
    let s = p.replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        // Both bytes are ASCII, so index 2 is a character boundary.
        let rest = s.get(2..).unwrap_or_default();
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        let drive = (b[0] as char).to_ascii_lowercase();
        return if rest.is_empty() {
            format!("/{drive}")
        } else {
            format!("/{drive}/{rest}")
        };
    }
    s
}

/// A shell-form path in Windows form: `/c/a/b` → `C:\a\b`; `/tmp/x` as sill maps it.
fn to_windows(p: &str, mapped: Option<std::path::PathBuf>) -> String {
    let s = match mapped {
        Some(m) => m.to_string_lossy().into_owned(),
        None => p.to_owned(),
    };
    s.replace('/', "\\")
}

fn convert(p: &str, form: Form, mapped: Option<std::path::PathBuf>) -> String {
    match form {
        Form::Unix => {
            to_unix(&mapped.map_or_else(|| p.to_owned(), |m| m.to_string_lossy().into_owned()))
        }
        Form::Windows => to_windows(p, mapped),
        Form::Mixed => to_windows(p, mapped).replace('\\', "/"),
    }
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for CygpathCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let form = if self.windows {
            Form::Windows
        } else if self.mixed {
            Form::Mixed
        } else {
            Form::Unix
        };
        if self.paths.is_empty() {
            writeln!(
                context.stderr(),
                "{}: no path given (cygpath [-u|-w|-m] [-a] [-p] PATH...)",
                context.command_name
            )?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        }
        let one = |p: &str| -> String {
            if p.is_empty() {
                return String::new();
            }
            let path = std::path::Path::new(p);
            let mut mapped = brush_core::sys::fs::map_shell_path(path);
            if self.absolute {
                let abs = context
                    .shell
                    .absolute_path(mapped.as_deref().unwrap_or(path));
                mapped = Some(std::path::PathBuf::from(brush_core::sys::fs::display_path(
                    &abs,
                )));
            }
            convert(p, form, mapped)
        };
        for p in &self.paths {
            let out = if self.path_list {
                // A `;` list is Windows form; otherwise the shell's `:` form (no drive colons).
                let parts: Vec<&str> = if p.contains(';') {
                    p.split(';').collect()
                } else {
                    p.split(':').collect()
                };
                let sep = if form == Form::Unix { ":" } else { ";" };
                parts.iter().map(|e| one(e)).collect::<Vec<_>>().join(sep)
            } else {
                one(p)
            };
            writeln!(context.stdout(), "{out}")?;
        }
        Ok(ExecutionResult::success())
    }
}

/// Run a command (nohup/setsid: nothing to do on Windows but run it).
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub(crate) struct RunCommand {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(RunCommand);

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for RunCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        mut context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();
        // setsid's -f/-w/-c and nohup's --: options of their own, not of COMMAND.
        let skip = self
            .args
            .iter()
            .take_while(|a| {
                matches!(
                    a.as_str(),
                    "-f" | "--fork" | "-w" | "--wait" | "-c" | "--ctty" | "--"
                )
            })
            .count();
        let rest = &self.args[skip..];
        let Some(first) = rest.first() else {
            writeln!(context.stderr(), "{name}: missing operand")?;
            return Ok(ExecutionResult::new(125));
        };
        first.clone_into(&mut context.command_name);
        let mut cmd = commands::SimpleCommand::new(
            commands::ShellForCommand::ParentShell(context.shell),
            context.params,
            context.command_name,
            rest.iter()
                .map(|a| brush_core::CommandArg::from(a.as_str())),
        );
        cmd.use_functions = true;
        let spawn = cmd.execute().await?;
        Ok(spawn.wait().await?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        assert_eq!(to_unix(r"C:\Users\x y\z"), "/c/Users/x y/z");
        assert_eq!(to_unix("D:/a"), "/d/a");
        assert_eq!(to_unix(r"C:\"), "/c");
        assert_eq!(to_unix(r"\\srv\share\f"), "//srv/share/f");
        assert_eq!(to_unix("rel/a"), "rel/a");
        assert_eq!(
            convert("/c/x/y", Form::Windows, Some("C:/x/y".into())),
            r"C:\x\y"
        );
        assert_eq!(
            convert("/c/x/y", Form::Mixed, Some("C:/x/y".into())),
            "C:/x/y"
        );
        assert_eq!(convert(r"C:\x", Form::Unix, None), "/c/x");
        assert_eq!(convert("rel/a", Form::Windows, None), r"rel\a");
    }
}
