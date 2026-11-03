use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionResult, builtins, error, jobs};

/// Manage jobs.
#[derive(Parser)]
pub(crate) struct JobsCommand {
    /// Also show process IDs.
    #[arg(short = 'l')]
    also_show_pids: bool,

    /// List only jobs that have changed status since the last notification.
    #[arg(short = 'n')]
    list_changed_only: bool,

    /// Show only process IDs.
    #[arg(short = 'p')]
    show_pids_only: bool,

    /// Show only running jobs.
    #[arg(short = 'r')]
    running_jobs_only: bool,

    /// Show only stopped jobs.
    #[arg(short = 's')]
    stopped_jobs_only: bool,

    /// Job specs to list.
    // TODO(jobs): Add -x option
    job_specs: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(JobsCommand);

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for JobsCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<brush_core::ExecutionResult, Self::Error> {
        if self.list_changed_only {
            return error::unimp("jobs -n");
        }

        if self.job_specs.is_empty() {
            for job in &context.shell.jobs().jobs {
                self.display_job(&context, job)?;
            }
        } else {
            // sill: `jobs %1`.
            let mut result = ExecutionResult::success();
            for spec in &self.job_specs {
                let id = context.shell.jobs_mut().resolve_job_spec(spec).map(|j| j.id);
                match id.and_then(|id| context.shell.jobs().jobs.iter().find(|j| j.id == id)) {
                    Some(job) => self.display_job(&context, job)?,
                    None => {
                        writeln!(context.stderr(), "{}: {spec}: no such job", context.command_name)?;
                        result = ExecutionResult::general_error();
                    }
                }
            }
            return Ok(result);
        }

        Ok(ExecutionResult::success())
    }
}

impl JobsCommand {
    fn display_job(
        &self,
        context: &brush_core::ExecutionContext<'_, impl brush_core::ShellExtensions>,
        job: &jobs::Job,
    ) -> Result<(), brush_core::Error> {
        if self.running_jobs_only && !matches!(job.state, jobs::JobState::Running) {
            return Ok(());
        }
        if self.stopped_jobs_only && !matches!(job.state, jobs::JobState::Stopped) {
            return Ok(());
        }

        if self.show_pids_only {
            if let Some(pid) = job.representative_pid() {
                writeln!(context.stdout(), "{pid}")?;
            }
        } else if self.also_show_pids {
            // sill: `jobs -l`, as bash prints it: `[1]+ 1234 Running    sleep 10 &`.
            let pid = job.representative_pid().map_or_else(String::new, |p| p.to_string());
            writeln!(
                context.stdout(),
                "[{}]{} {pid} {:<24}{}",
                job.id,
                job.annotation(),
                job.state.to_string(),
                job.command_line
            )?;
        } else {
            writeln!(context.stdout(), "{job}")?;
        }

        Ok(())
    }
}
