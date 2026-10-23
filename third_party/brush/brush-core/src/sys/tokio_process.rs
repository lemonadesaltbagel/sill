//! Process management utilities

use std::process::Stdio;

use futures::FutureExt as _;

use crate::{
    ExecutionResult, ShellFd,
    openfiles::OpenFiles,
    processes::{ChildProcess, LaunchRequest, ProcessGroup},
    sys::commands::{self, CommandExt, CommandFdInjectionExt, CommandFgControlExt},
};

pub(crate) type ProcessId = i32;

/// Starts a native child process.
///
/// # Arguments
///
/// * `command` - The process to start.
pub(crate) fn spawn(command: LaunchRequest) -> std::io::Result<ChildProcess> {
    let process_group = command.process_group;
    // See `CreateOptions::kill_external_commands_on_drop` (false for ordinary shells).
    let kill_on_drop = command.kill_on_drop;

    let mut command = tokio::process::Command::from(compose_std_command(command)?);
    command.kill_on_drop(kill_on_drop);
    let child = command.spawn()?;

    #[expect(clippy::cast_possible_wrap)]
    let pid = child.id().map(|id| id as ProcessId);
    if pid.is_none() {
        tracing::warn!("could not retrieve pid for child process");
    }

    let pgid = match process_group {
        ProcessGroup::Inherit => None,
        ProcessGroup::Join(pgid) => Some(pgid),
        ProcessGroup::New { .. } | ProcessGroup::NewSession => pid,
    };

    let exited = child
        .wait_with_output()
        .map(|output| output.map(ExecutionResult::from));
    Ok(ChildProcess::from_future(exited).native_child(pid, pgid))
}

/// Replaces the current process with a new one. Returns only if that fails, with
/// the reason why.
///
/// # Arguments
///
/// * `command` - The process to replace the current one with.
pub(crate) fn exec(command: LaunchRequest) -> std::io::Error {
    match compose_std_command(command) {
        Ok(command) => commands::exec(command),
        Err(err) => err,
    }
}

/// Composes the `std::process::Command` that starts a process.
fn compose_std_command(command: LaunchRequest) -> std::io::Result<std::process::Command> {
    // sill: a `#!` script starts as its interpreter with the script's path.
    #[cfg(windows)]
    let command = shebang_launch(command)?;
    let mut cmd = std::process::Command::new(commands::native_program_path(&command.program));

    // Override argv[0].
    // NOTE: Not supported on all platforms.
    cmd.arg0(command.argv0);
    // sill: platform-specific argument encoding (see `push_args`).
    commands::push_args(&mut cmd, &command.program, command.args)?;
    cmd.current_dir(command.working_dir);
    // sill: without a console of the shell's own (sill creates an invisible one where
    // Windows allows; see crates/sill `console`), each console program would get a new
    // console with a window. Start it with a hidden one instead.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // SAFETY: plain Win32 call; 0 means the process has no console.
        if unsafe { windows_sys::Win32::System::Console::GetConsoleCP() } == 0 {
            cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
    }
    cmd.env_clear();
    cmd.envs(command.env);

    // A closed standard stream gets the null device.
    // TODO(redirection): close it instead, as bash does (e.g., with a `pre_exec`
    // on Unix), so that the process fails to use it.
    let mut open_files = command.open_files;
    cmd.stdin(take_stdio(&mut open_files, OpenFiles::STDIN_FD)?);
    cmd.stdout(take_stdio(&mut open_files, OpenFiles::STDOUT_FD)?);
    cmd.stderr(take_stdio(&mut open_files, OpenFiles::STDERR_FD)?);

    cmd.inject_fds(open_files.iter_fds().map(|(fd, file)| (fd, file.clone())))
        .map_err(std::io::Error::other)?;

    match command.process_group {
        ProcessGroup::Inherit => {}
        ProcessGroup::Join(pgid) => {
            cmd.process_group(pgid);
        }
        ProcessGroup::New { take_foreground } => {
            cmd.process_group(0);
            if take_foreground {
                cmd.take_foreground();
            }
        }
        // setsid() makes a new process group too.
        ProcessGroup::NewSession => cmd.lead_session(),
    }

    Ok(cmd)
}

/// Takes the file for standard stream `fd` out of `open_files`, for the process
/// to have as that stream: the null device if it's closed.
fn take_stdio(open_files: &mut OpenFiles, fd: ShellFd) -> std::io::Result<Stdio> {
    match open_files.remove_fd(fd) {
        Some(file) => commands::stdio_for(file),
        None => Ok(Stdio::null()),
    }
}

/// sill: if `command.program` is a `#!` script rather than a Windows executable, the
/// request to start its interpreter instead (see `sys::shebang`).
#[cfg(windows)]
fn shebang_launch(mut command: LaunchRequest) -> std::io::Result<LaunchRequest> {
    use crate::sys::shebang;

    let program = if command.program.is_relative() {
        command.working_dir.join(&command.program)
    } else {
        command.program.clone()
    };
    let is_windows_executable = program.extension().is_some_and(|ext| {
        ["exe", "com", "bat", "cmd"].iter().any(|e| ext.eq_ignore_ascii_case(e))
    });
    if is_windows_executable {
        return Ok(command);
    }
    let Some(sb) = shebang::read(&program) else {
        return Ok(command);
    };

    let path_var = command
        .env
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let find = |name: &str| -> Option<std::path::PathBuf> {
        crate::sys::fs::split_paths(&path_var)
            .find_map(|dir| crate::sys::fs::resolve_executable(dir.join(name)).filter(|p| !p.is_dir()))
    };
    let written = std::path::Path::new(&sb.interpreter);
    let interpreter = if !sb.via_env && written.is_absolute() && written.is_file() {
        Some(written.to_path_buf())
    } else {
        let name = shebang::interpreter_name(&sb.interpreter);
        if shebang::is_shell(name) {
            std::env::current_exe().ok()
        } else {
            find(name).or_else(|| (name == "python3").then(|| find("python")).flatten())
        }
    };
    let Some(interpreter) = interpreter else {
        return Err(std::io::Error::other(format!(
            "{}: bad interpreter: No such file or directory",
            sb.interpreter
        )));
    };

    // The script as the command named it (`./build.sh`, which becomes its `$0`), or the
    // path a PATH search found.
    let script = if crate::sys::fs::contains_path_separator(&command.argv0) {
        command.argv0.clone()
    } else {
        command.program.to_string_lossy().replace('\\', "/")
    };
    let mut args = sb.args;
    args.push(script);
    args.append(&mut command.args);
    command.args = args;
    command.program = interpreter;
    Ok(command)
}
