//! sill: `disown`, which agents use after `server &` so the server outlives the shell.
//! It was "not implemented" (exit 99), which read as a failure of the whole command.

use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionResult, builtins};

/// Remove jobs from the shell's job table.
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub(crate) struct DisownCommand {
    /// Keep the jobs in the table, marked not to get SIGHUP (a no-op on Windows, where a
    /// background job already outlives the shell).
    #[arg(short = 'h')]
    no_sighup: bool,

    /// All jobs.
    #[arg(short = 'a')]
    all: bool,

    /// Only running jobs.
    #[arg(short = 'r')]
    running: bool,

    /// Job specs (`%1`) or process IDs; the current job if none.
    jobs: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(DisownCommand);

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for DisownCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let mut ids = vec![];
        let mut missing = vec![];
        let jobs = context.shell.jobs_mut();
        if self.all || self.running {
            ids.extend(
                jobs.jobs
                    .iter()
                    .filter(|j| !self.running || matches!(j.state, brush_core::jobs::JobState::Running))
                    .map(|j| j.id),
            );
        } else if self.jobs.is_empty() {
            match jobs.current_job() {
                Some(job) => ids.push(job.id),
                None => missing.push("current".to_owned()),
            }
        }
        for spec in &self.jobs {
            let job = if spec.starts_with('%') {
                jobs.resolve_job_spec(spec)
            } else {
                spec.parse().ok().and_then(|pid| jobs.find_job_by_pid(pid))
            };
            match job {
                Some(job) => ids.push(job.id),
                None => missing.push(spec.clone()),
            }
        }
        if !self.no_sighup {
            jobs.jobs.retain(|j| !ids.contains(&j.id));
        }
        for spec in &missing {
            writeln!(context.stderr(), "{}: {spec}: no such job", context.command_name)?;
        }
        Ok(if missing.is_empty() {
            ExecutionResult::success()
        } else {
            ExecutionResult::general_error()
        })
    }
}
