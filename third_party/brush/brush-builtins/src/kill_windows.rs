//! sill: `kill` on Windows.
//!
//! Windows has no signals, process groups or `kill.exe`, and brush's `kill` was Unix-only, so
//! the common agent pattern `server & ...; kill $!` failed with "command not found". Here a
//! signal ends the target: the process and everything it started (there are no process
//! groups to do that, and stopping only a `cmd.exe` or `npm.cmd` wrapper would leave the real
//! server running). Each gets exit code 128 + the signal number, which is what bash reports
//! for a process ended by that signal. `kill -0` checks that a process exists. Signals whose
//! default is to stop, continue or be ignored (`STOP`, `CONT`, `WINCH`) can't be sent: ending
//! the process would be wrong.

use clap::Parser;
use std::io::Write;

use brush_core::traps::TrapSignal;
use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// Signal a job or process.
#[derive(Parser)]
pub(crate) struct KillCommand {
    /// Name of the signal to send.
    #[arg(short = 's', value_name = "SIG_NAME")]
    signal_name: Option<String>,

    /// Number of the signal to send.
    #[arg(short = 'n', value_name = "SIG_NUM")]
    signal_number: Option<i32>,

    /// List known signal names.
    #[arg(short = 'l', short_alias = 'L')]
    list_signals: bool,

    /// `-SIG`, then process IDs or job specs.
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(KillCommand);

/// A signal's number (0 for `-0`), by its name or number: `TERM`, `SIGTERM`, `15`.
fn parse_signal(spec: &str) -> Option<i32> {
    match spec.parse::<TrapSignal>() {
        Ok(TrapSignal::Signal(s)) => Some(s as i32),
        Ok(TrapSignal::Exit) if spec.parse::<i32>() == Ok(0) => Some(0),
        _ => None,
    }
}

/// Signals whose default is to stop, continue or be ignored: nothing to end.
const NOT_ENDING: &[&str] = &[
    "SIGCHLD", "SIGCONT", "SIGSTOP", "SIGTSTP", "SIGTTIN", "SIGTTOU", "SIGURG", "SIGWINCH",
];

fn ends_process(signal: i32) -> bool {
    match TrapSignal::try_from(signal) {
        Ok(TrapSignal::Signal(s)) => !NOT_ENDING.contains(&s.as_str()),
        _ => signal == 0,
    }
}

/// `kill -l`: the signal table; with arguments, each number's name (an exit status above
/// 128 names the signal that ended a process: `kill -l 143` is `TERM`) or each name's number.
fn list_signals(
    context: &brush_core::ExecutionContext<'_, impl brush_core::ShellExtensions>,
    args: &[String],
) -> Result<ExecutionResult, brush_core::Error> {
    if args.is_empty() {
        brush_core::traps::format_signals(
            context.stdout(),
            TrapSignal::iterator().filter(|s| !matches!(s, TrapSignal::Exit)),
        )?;
        writeln!(context.stdout())?;
        return Ok(ExecutionResult::success());
    }
    let mut result = ExecutionResult::success();
    for arg in args {
        let line = if let Ok(n) = arg.parse::<i32>() {
            let n = if n > 128 { n - 128 } else { n };
            match TrapSignal::try_from(n) {
                Ok(TrapSignal::Signal(s)) => Some(s.as_str().trim_start_matches("SIG").to_owned()),
                Ok(TrapSignal::Exit) => Some("EXIT".to_owned()),
                _ => None,
            }
        } else {
            match TrapSignal::try_from(arg.as_str()) {
                Ok(TrapSignal::Signal(s)) => Some((s as i32).to_string()),
                Ok(TrapSignal::Exit) => Some("0".to_owned()),
                _ => None,
            }
        };
        if let Some(line) = line {
            writeln!(context.stdout(), "{line}")?;
        } else {
            let name = &context.command_name;
            writeln!(
                context.stderr(),
                "{name}: {arg}: invalid signal specification"
            )?;
            result = ExecutionResult::general_error();
        }
    }
    Ok(result)
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for KillCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();

        if self.list_signals {
            return list_signals(&context, &self.args);
        }

        // bash's default signal is TERM.
        let mut signal = 15;
        let mut targets: &[String] = &self.args;
        if let Some(spec) = &self.signal_name {
            let Some(n) = parse_signal(spec) else {
                writeln!(
                    context.stderr(),
                    "{name}: {spec}: invalid signal specification"
                )?;
                return Ok(ExecutionResult::general_error());
            };
            signal = n;
        } else if let Some(n) = self.signal_number {
            if parse_signal(&n.to_string()).is_none() {
                writeln!(
                    context.stderr(),
                    "{name}: {n}: invalid signal specification"
                )?;
                return Ok(ExecutionResult::general_error());
            }
            signal = n;
        } else if let Some(first) = targets.first()
            && let Some(spec) = first.strip_prefix('-')
            && !spec.is_empty()
        {
            let Some(n) = parse_signal(spec) else {
                writeln!(
                    context.stderr(),
                    "{name}: {spec}: invalid signal specification"
                )?;
                return Ok(ExecutionResult::general_error());
            };
            signal = n;
            targets = &targets[1..];
        }

        if !ends_process(signal) {
            let sig =
                TrapSignal::try_from(signal).map_or_else(|_| signal.to_string(), |s| s.to_string());
            writeln!(
                context.stderr(),
                "{name}: {sig}: can't be sent on Windows (only signals that end a process can)"
            )?;
            return Ok(ExecutionResult::general_error());
        }

        if targets.is_empty() {
            writeln!(
                context.stderr(),
                "{name}: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l"
            )?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        }

        let mut result = ExecutionResult::success();
        for target in targets {
            let pid = if target.starts_with('%') {
                match context
                    .shell
                    .jobs_mut()
                    .resolve_job_spec(target)
                    .and_then(|j| j.representative_pid())
                {
                    Some(pid) => pid,
                    None => {
                        writeln!(context.stderr(), "{name}: {target}: no such job")?;
                        result = ExecutionResult::general_error();
                        continue;
                    }
                }
            } else if let Ok(pid) = target.parse::<i32>() {
                pid
            } else {
                writeln!(
                    context.stderr(),
                    "{name}: {target}: arguments must be process or job IDs"
                )?;
                result = ExecutionResult::general_error();
                continue;
            };

            let outcome = if signal == 0 {
                win::process_exists(pid)
            } else {
                win::terminate_tree(pid, u32::try_from(128 + signal).unwrap_or(1))
            };
            if let Err(message) = outcome {
                writeln!(context.stderr(), "{name}: ({pid}) - {message}")?;
                result = ExecutionResult::general_error();
            }
        }
        Ok(result)
    }
}

pub(crate) mod win {
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        TerminateProcess,
    };

    fn open(pid: u32, access: u32) -> Result<HANDLE, &'static str> {
        // SAFETY: plain Win32 call; a null handle means failure.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            // SAFETY: reads the calling thread's last error.
            return Err(if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
                "Operation not permitted"
            } else {
                "No such process"
            });
        }
        Ok(h)
    }

    fn to_u32(pid: i32) -> Result<u32, &'static str> {
        u32::try_from(pid).map_err(|_| "No such process")
    }

    pub(super) fn process_exists(pid: i32) -> Result<(), &'static str> {
        let h = open(to_u32(pid)?, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut code = 0u32;
        // SAFETY: `h` is a valid process handle we own.
        let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0;
        // SAFETY: closing the handle we opened.
        unsafe { CloseHandle(h) };
        if ok && code == STILL_ACTIVE as u32 {
            Ok(())
        } else {
            Err("No such process")
        }
    }

    /// Children of each process, from a snapshot of all processes.
    fn children_by_parent() -> HashMap<u32, Vec<u32>> {
        let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
        // SAFETY: plain Win32 call; checked against INVALID_HANDLE_VALUE.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return map;
        }
        // SAFETY: PROCESSENTRY32W is plain data; dwSize is set before use.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
        // SAFETY: `snap` is a valid snapshot handle and `entry` is sized.
        let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
        while more {
            if entry.th32ProcessID != entry.th32ParentProcessID {
                map.entry(entry.th32ParentProcessID)
                    .or_default()
                    .push(entry.th32ProcessID);
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap, &mut entry) } != 0;
        }
        // SAFETY: closing the snapshot handle we created.
        unsafe { CloseHandle(snap) };
        map
    }

    /// Ends `pid` and every process it started (deepest first, so none is orphaned and
    /// restarted by a parent in between), each with `exit_code`.
    pub(crate) fn terminate_tree(pid: i32, exit_code: u32) -> Result<(), &'static str> {
        // A background job's `$!`: its Job Object holds every process it started, even
        // ones whose parent has exited (`cmd /c start /b server &`), which a walk through
        // parent PIDs can't find.
        if let Some(job) = brush_core::sys::job::background_job_of(pid) {
            return if job.terminate(exit_code) {
                Ok(())
            } else {
                Err("no such process")
            };
        }
        let root = to_u32(pid)?;
        // Open the root first: a missing or protected target is the error to report.
        let root_handle = open(root, PROCESS_TERMINATE)?;

        let children = children_by_parent();
        let mut order = vec![];
        let mut stack = vec![root];
        while let Some(p) = stack.pop() {
            if let Some(kids) = children.get(&p) {
                for &k in kids {
                    if k != root && !order.contains(&k) {
                        order.push(k);
                        stack.push(k);
                    }
                }
            }
        }
        for &descendant in order.iter().rev() {
            if let Ok(h) = open(descendant, PROCESS_TERMINATE) {
                // SAFETY: `h` is a process handle we own; failures are ignored (it may
                // have exited already).
                unsafe {
                    TerminateProcess(h, exit_code);
                    CloseHandle(h);
                }
            }
        }
        // SAFETY: `root_handle` is a process handle we own.
        let ok = unsafe { TerminateProcess(root_handle, exit_code) } != 0;
        // SAFETY: closing the handle we opened.
        unsafe { CloseHandle(root_handle) };
        if ok { Ok(()) } else { Err("No such process") }
    }
}
