//! Command execution utilities.

use std::ffi::OsStr;

use crate::{ShellFd, error, openfiles};

/// Extension trait for Unix-like command extensions.
pub trait CommandExt {
    /// Sets the zeroth argument (argv[0]) of the command.
    ///
    /// # Arguments
    ///
    /// * `arg` - The argument to set as argv[0].
    fn arg0<S>(&mut self, arg: S) -> &mut Self
    where
        S: AsRef<OsStr>;

    /// Sets the process group ID of the command.
    ///
    /// # Arguments
    ///
    /// * `pgroup` - The process group ID to set.
    fn process_group(&mut self, pgroup: i32) -> &mut Self;
}

impl CommandExt for std::process::Command {
    fn arg0<S>(&mut self, _arg: S) -> &mut Self
    where
        S: AsRef<OsStr>,
    {
        // NOTE: no-op.
        self
    }

    fn process_group(&mut self, _pgroup: i32) -> &mut Self {
        // NOTE: no-op.
        self
    }
}

/// Extension trait for Unix-like exit status extensions.
pub trait ExitStatusExt {
    /// Returns the signal that terminated the process, if any.
    fn signal(&self) -> Option<i32>;
}

impl ExitStatusExt for std::process::ExitStatus {
    fn signal(&self) -> Option<i32> {
        None
    }
}

/// Replaces the current process with `command`; not supported on this platform.
#[cfg(not(windows))]
pub(crate) fn exec(_command: std::process::Command) -> std::io::Error {
    std::io::ErrorKind::Unsupported.into()
}

/// sill: Windows can't replace a process, so `exec` runs `command` and exits with its
/// status, as Git Bash's does: wrapper scripts (`gradlew`, `mvnw`, launchers) end with
/// `exec "$JAVACMD" "$@"`. The shell runs nothing more and no EXIT trap, as after a real
/// exec. Returns only if the program can't start.
#[cfg(windows)]
pub(crate) fn exec(mut command: std::process::Command) -> std::io::Error {
    // The program starts suspended (see `sys::job`): it joins a foreground job first.
    let status = match crate::sys::job::start_std(&mut command) {
        Ok((mut child, job)) => {
            let status = child.wait();
            if let Some(job) = job {
                job.release();
            }
            status
        }
        Err(err) => return err,
    };
    let code = status.map_or(126, |s| s.code().unwrap_or(1));
    std::process::exit(code)
}

/// Returns the standard stream for a process to have `file` as.
pub(crate) fn stdio_for(file: openfiles::OpenFile) -> std::io::Result<std::process::Stdio> {
    use std::process::Stdio;

    match file {
        // The shell's own output streams go to whichever stream the process has them as;
        // e.g., the shell's stderr as the process's stdout, for `>&2`.
        // TODO(redirection): the shell's stdin as an output stream (e.g., `>&0`) is
        // inherited from the stream it replaces instead.
        openfiles::OpenFile::Stdin(_) => Ok(Stdio::inherit()),
        openfiles::OpenFile::Stdout(s) => Ok(s.into()),
        openfiles::OpenFile::Stderr(s) => Ok(s.into()),
        openfiles::OpenFile::File(f) => Ok(f.try_clone()?.into()),
        openfiles::OpenFile::PipeReader(r) => Ok(r.try_clone()?.into()),
        openfiles::OpenFile::PipeWriter(w) => Ok(w.try_clone()?.into()),
        // A custom stream has no handle to give a process.
        openfiles::OpenFile::Stream(_) => Ok(Stdio::null()),
    }
}

/// Extension trait for injecting file descriptors into commands.
pub trait CommandFdInjectionExt {
    /// Injects the given open files as file descriptors into the command.
    ///
    /// # Arguments
    ///
    /// * `open_files` - A mapping of child file descriptors to open files.
    fn inject_fds(
        &mut self,
        open_files: impl Iterator<Item = (ShellFd, openfiles::OpenFile)>,
    ) -> Result<(), error::Error>;
}

impl CommandFdInjectionExt for std::process::Command {
    fn inject_fds(
        &mut self,
        open_files: impl Iterator<Item = (ShellFd, openfiles::OpenFile)>,
    ) -> Result<(), error::Error> {
        // sill: a native Windows process receives only stdin, stdout and stderr; there
        // is no general way to hand it fd 3 and up. Failing the launch (as before) broke
        // every command run while the shell had such an fd open (`exec 3>log`, coproc
        // plumbing), though the shell's own use of those fds works. So they stay with
        // the shell and aren't passed on.
        for (fd, _) in open_files {
            tracing::debug!("fd {fd} is not passed to a native Windows process");
        }

        Ok(())
    }
}

/// Extension trait for arranging for commands to take the foreground.
pub trait CommandFgControlExt {
    /// Arranges for the command to take the foreground when it is executed.
    fn take_foreground(&mut self);
    /// Arranges for the command to become a session leader when it is executed.
    fn lead_session(&mut self);
}

impl CommandFgControlExt for std::process::Command {
    fn take_foreground(&mut self) {
        // NOTE: This is a no-op.
    }

    fn lead_session(&mut self) {
        // NOTE: This is a no-op.
    }
}

/// sill: adds `args` to `command`'s command line. On Windows a program splits its own
/// command line, so the quoting is what decides what it receives.
///
/// Rust std leaves an argument without spaces or quotes unquoted, `*.txt` included.
/// Programs built with wildcard expansion (MinGW's default, MSVC's `setargv.obj`,
/// busybox-w32, uutils' `wild`) then expand it themselves, so `find . -name '*.txt'`
/// got file names instead of the pattern. `sill_winargs` also quotes `*` and `?`
/// (spec/argv-test-vectors.json; checked against the real UCRT parser in CI).
///
/// `.bat`/`.cmd` targets keep std's cmd.exe escaping (its CVE-2024-24576 fix), which
/// a raw command line would bypass.
pub(crate) fn push_args(
    command: &mut std::process::Command,
    program: &std::path::Path,
    args: Vec<String>,
) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        // Git Bash's `//c`-style switch escape means `/c` to a native program.
        let args: Vec<String> = args
            .into_iter()
            .map(|a| match sill_winargs::msys_double_slash_switch(&a) {
                Some(single) => single.to_owned(),
                None => a,
            })
            .collect();

        // sill's own tools (busybox in `SILL_TOOLS_DIR`) take the shell's `/c/...` path
        // dialect, like the bundled coreutils: `find /c/...` found nothing before. Other
        // programs' arguments are never rewritten.
        let args: Vec<String> = if is_sill_tool(program) {
            args.into_iter().map(|a| map_tool_arg(&a).unwrap_or(a)).collect()
        } else {
            args
        };
        // sill's bundled jq writes CRLF unless -b; `$(jq -r .x)` then kept a CR.
        let args = if is_sill_tool(program)
            && program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("jq"))
            && !args.iter().any(|a| a == "-b" || a == "--binary")
        {
            std::iter::once("-b".to_owned()).chain(args).collect()
        } else {
            args
        };

        // sill's GNU tools take their arguments from a file when a command line can't
        // hold them (third_party/gnu-tools/shim.c).
        let args = if is_sill_tool(program) && takes_args_file(program) && args.iter().map(|a| a.len() + 3).sum::<usize>() > 30_000 {
            match write_args_file(&args) {
                Some(file) => vec!["--sill-args-file".to_owned(), file],
                None => args,
            }
        } else {
            args
        };

        let is_batch = program.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd")
        });
        if sill_winargs::is_cmd_exe(program) {
            // cmd.exe reads the text after /c nearly raw, not with CRT rules; see
            // `encode_cmd_exe_args`. (`cmd /c 'call "C:\...\vcvars64.bat" && cl ...'`
            // failed this way for every environment in the agent end-to-end run.)
            // Claude Code turns `>nul` into `>/dev/null` even inside the text for cmd.exe;
            // see `cmd_dev_null_redirects`.
            let args: Vec<String> = args
                .iter()
                .map(|a| sill_winargs::cmd_dev_null_redirects(a).into_owned())
                .collect();
            let line = sill_winargs::encode_cmd_exe_args(&args).map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
            })?;
            command.raw_arg(line);
            return Ok(());
        }
        if is_batch {
            // std's own cmd.exe escaping for batch files (its CVE-2024-24576 fix).
            command.args(args);
        } else {
            for arg in &args {
                let quoted = sill_winargs::quote_exe_arg(arg).map_err(|err| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
                })?;
                command.raw_arg(quoted);
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = program;
        command.args(args);
        Ok(())
    }
}

/// sill: the path a native process is started with. On Windows, `/` separators become
/// `\`: programs see their own path as argv[0], and some parse it. cmd.exe started as
/// `C:/Windows/System32/cmd.exe /d /c echo hi` reads the `/cmd.exe` in its own path as
/// `/c md.exe ...` and runs `md` ("The directory or file cannot be created").
pub(crate) fn native_program_path(program: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) {
        std::path::PathBuf::from(program.to_string_lossy().replace('/', "\\"))
    } else {
        program.to_path_buf()
    }
}

/// sill: the bundled GNU tools, built with the args-file shim.
#[cfg(windows)]
fn takes_args_file(program: &std::path::Path) -> bool {
    program.file_stem().and_then(|s| s.to_str()).is_some_and(|s| {
        ["grep", "sed", "diff", "cmp", "diff3", "sdiff", "make"].iter().any(|t| s.eq_ignore_ascii_case(t))
    })
}

/// sill: writes `args` NUL-separated to a new temporary file; returns its path. The tool
/// deletes it after reading.
#[cfg(windows)]
fn write_args_file(args: &[String]) -> Option<String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!("sill-gnuargs-{}-{}.txt", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&path, args.join("\0")).ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// sill: whether `program` is one of sill's bundled tools (in `SILL_TOOLS_DIR`).
#[cfg(windows)]
fn is_sill_tool(program: &std::path::Path) -> bool {
    let Some(dir) = std::env::var_os("SILL_TOOLS_DIR") else {
        return false;
    };
    let norm = |p: &std::path::Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    program
        .parent()
        .is_some_and(|parent| norm(parent).trim_end_matches('\\') == norm(std::path::Path::new(&dir)).trim_end_matches('\\'))
}

/// sill: a `/c/...` argument to one of sill's tools, as the Windows path, when that
/// path or its parent directory exists (so a pattern like `/a/b` stays as written).
#[cfg(windows)]
fn map_tool_arg(arg: &str) -> Option<String> {
    let mapped = crate::sys::fs::map_shell_path(std::path::Path::new(arg))?;
    (mapped.exists() || mapped.parent().is_some_and(std::path::Path::exists))
        .then(|| mapped.to_string_lossy().into_owned())
}
