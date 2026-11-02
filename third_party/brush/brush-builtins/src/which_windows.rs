//! sill: `which` on Windows, from the shell's own lookup. busybox's `which` searched the
//! PATH by itself, so it named the Store's stand-in `python.exe` (`WindowsApps`) while
//! running `python` said "command not found": an agent took Python to be installed.

use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionResult, builtins};

use crate::lookup::{self, Resolved};

/// Show where the programs a name runs are.
#[derive(Parser)]
pub(crate) struct WhichCommand {
    /// Every match on the PATH, not only the first.
    #[arg(short = 'a')]
    all: bool,

    /// Print nothing; only the exit status.
    #[arg(short = 's')]
    silent: bool,

    /// Program names.
    names: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(WhichCommand);

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for WhichCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let options = lookup::Options {
            force_path_search: true,
            suppress_func_lookup: true,
            all_locations: self.all,
            path_dirs: None,
        };
        let mut found_all = true;
        for name in &self.names {
            let paths: Vec<_> = lookup::resolve(context.shell, name, &options)
                .into_iter()
                .filter_map(|r| match r {
                    Resolved::File { path, .. } => Some(path),
                    _ => None,
                })
                .collect();
            if paths.is_empty() {
                found_all = false;
                if !self.silent {
                    writeln!(context.stderr(), "which: no {name} in PATH")?;
                }
                continue;
            }
            if !self.silent {
                for path in paths.iter().take(if self.all { usize::MAX } else { 1 }) {
                    writeln!(context.stdout(), "{}", brush_core::sys::fs::display_path(path))?;
                }
            }
        }
        Ok(if found_all && !self.names.is_empty() {
            ExecutionResult::success()
        } else {
            ExecutionResult::general_error()
        })
    }
}
