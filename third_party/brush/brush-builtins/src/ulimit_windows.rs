//! sill: `ulimit` on Windows.
//!
//! Windows has no resource limits of the Unix kind, and brush's `ulimit` was Unix-only, so a
//! script's `ulimit -n 4096` failed with "command not found" (and stopped a `set -e` script).
//! This reports what Git Bash reports, with "unlimited" where Windows sets no limit, and
//! accepts a new limit within the hard one: the shell remembers it (later `ulimit` calls
//! show it) but Windows doesn't enforce it. Options Git Bash lacks (`-l`, `-m`, ...) are
//! invalid, as there.

use clap::Parser;
use std::io::Write;
use std::sync::Mutex;

use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// Get or set resource limits.
#[derive(Parser)]
#[allow(clippy::struct_excessive_bools, reason = "one flag per option")]
pub(crate) struct ULimitCommand {
    /// Use the soft limit.
    #[arg(short = 'S')]
    soft: bool,
    /// Use the hard limit.
    #[arg(short = 'H')]
    hard: bool,
    /// Report all limits.
    #[arg(short = 'a')]
    all: bool,
    /// Core file size.
    #[arg(short = 'c')]
    c: bool,
    /// Data segment size.
    #[arg(short = 'd')]
    d: bool,
    /// File size.
    #[arg(short = 'f')]
    f: bool,
    /// Open files.
    #[arg(short = 'n')]
    n: bool,
    /// Pipe size.
    #[arg(short = 'p')]
    p: bool,
    /// Stack size.
    #[arg(short = 's')]
    s: bool,
    /// CPU time.
    #[arg(short = 't')]
    t: bool,
    /// User processes.
    #[arg(short = 'u')]
    u: bool,
    /// Virtual memory.
    #[arg(short = 'v')]
    v: bool,
    /// The new limit: a number, `unlimited`, `soft` or `hard`.
    limit: Option<String>,
}

brush_clap_adapter::impl_parse_args!(ULimitCommand);

/// `None` is unlimited.
type Value = Option<u64>;

struct Resource {
    opt: char,
    name: &'static str,
    unit: &'static str,
    soft: Value,
    hard: Value,
    settable: bool,
}

const fn res(
    opt: char,
    name: &'static str,
    unit: &'static str,
    soft: Value,
    hard: Value,
) -> Resource {
    Resource {
        opt,
        name,
        unit,
        soft,
        hard,
        settable: true,
    }
}

/// Git Bash's values, except where it reports a Cygwin limit Windows doesn't have (open
/// files, processes, stack): those are unlimited, as on Windows.
static LIMITS: Mutex<[Resource; 9]> = Mutex::new([
    res('c', "core file size", "blocks", Some(0), None),
    res('d', "data seg size", "kbytes", None, None),
    res('f', "file size", "blocks", None, None),
    res('n', "open files", "", Some(8192), None),
    Resource {
        opt: 'p',
        name: "pipe size",
        unit: "512 bytes",
        soft: Some(8),
        hard: Some(8),
        settable: false,
    },
    res('s', "stack size", "kbytes", Some(8192), None),
    res('t', "cpu time", "seconds", None, None),
    res('u', "max user processes", "", None, None),
    res('v', "virtual memory", "kbytes", None, None),
]);

fn show(value: Value) -> String {
    value.map_or_else(|| "unlimited".into(), |n| n.to_string())
}

/// Whether `a` is at most `b` (unlimited is above any number).
const fn within(a: Value, b: Value) -> bool {
    match (a, b) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => a <= b,
    }
}

impl ULimitCommand {
    fn selected(&self) -> Vec<char> {
        let flags = [
            (self.c, 'c'),
            (self.d, 'd'),
            (self.f, 'f'),
            (self.n, 'n'),
            (self.p, 'p'),
            (self.s, 's'),
            (self.t, 't'),
            (self.u, 'u'),
            (self.v, 'v'),
        ];
        flags
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| *c)
            .collect()
    }

    /// Runs the command; output lines go to `out`, errors to `err`.
    fn run(&self, out: &mut Vec<String>, err: &mut Vec<String>) -> ExecutionExitCode {
        let mut limits = LIMITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut opts = self.selected();
        if self.all {
            opts = limits.iter().map(|r| r.opt).collect();
        } else if opts.is_empty() {
            opts.push('f');
        }

        let Some(limit) = self.limit.as_deref().filter(|_| !self.all) else {
            // Report.
            let labeled = opts.len() > 1;
            for r in limits.iter().filter(|r| opts.contains(&r.opt)) {
                let value = show(if self.hard { r.hard } else { r.soft });
                if labeled {
                    let unit = if r.unit.is_empty() {
                        format!("(-{})", r.opt)
                    } else {
                        format!("({}, -{})", r.unit, r.opt)
                    };
                    let width = 40usize.saturating_sub(r.name.len());
                    out.push(format!("{}{unit:>width$} {value}", r.name));
                } else {
                    out.push(value);
                }
            }
            return ExecutionExitCode::Success;
        };

        let mut status = ExecutionExitCode::Success;
        for r in limits.iter_mut().filter(|r| opts.contains(&r.opt)) {
            let value: Value = match limit {
                "unlimited" => None,
                "hard" => r.hard,
                "soft" => r.soft,
                n => {
                    let Ok(n) = n.parse::<u64>() else {
                        err.push(format!("{n}: invalid number"));
                        return ExecutionExitCode::GeneralError;
                    };
                    Some(n)
                }
            };
            // Without -S or -H, both limits are set.
            let (set_soft, set_hard) = (self.soft || !self.hard, self.hard || !self.soft);
            let allowed = r.settable
                && (!set_hard || within(value, r.hard))
                && (!set_soft || set_hard || within(value, r.hard))
                && (set_soft || within(r.soft, value));
            if !allowed {
                err.push(format!(
                    "{}: cannot modify limit: Operation not permitted",
                    r.name
                ));
                status = ExecutionExitCode::GeneralError;
                continue;
            }
            if set_hard {
                r.hard = value;
            }
            if set_soft {
                r.soft = value;
            }
        }
        drop(limits);
        status
    }
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for ULimitCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let (mut out, mut err) = (vec![], vec![]);
        let status = self.run(&mut out, &mut err);
        for line in out {
            writeln!(context.stdout(), "{line}")?;
        }
        for line in err {
            writeln!(context.stderr(), "{}: {line}", context.command_name)?;
        }
        Ok(status.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> (u8, Vec<String>, Vec<String>) {
        let cmd =
            ULimitCommand::try_parse_from(std::iter::once("ulimit").chain(args.iter().copied()))
                .unwrap();
        let (mut out, mut err) = (vec![], vec![]);
        let status = cmd.run(&mut out, &mut err);
        (status.into(), out, err)
    }

    // One test: the limits are shared by the process.
    #[test]
    fn report_and_set() {
        assert_eq!(run(&[]).1, ["unlimited"]);
        assert_eq!(run(&["-c"]).1, ["0"]);
        assert_eq!(run(&["-p"]).1, ["8"]);
        let all = run(&["-a"]).1;
        assert_eq!(all[0], "core file size              (blocks, -c) 0");
        assert_eq!(all[3], "open files                          (-n) 8192");
        assert_eq!(all[4], "pipe size                (512 bytes, -p) 8");

        // A soft limit within the hard one, then the hard one lowered.
        assert_eq!(run(&["-n", "65536"]).0, 0);
        assert_eq!(run(&["-n"]).1, ["65536"]);
        assert_eq!(run(&["-Hn"]).1, ["65536"]);
        assert_eq!(run(&["-Sn", "100"]).0, 0);
        assert_eq!(run(&["-n"]).1, ["100"]);
        // Raising past the hard limit fails.
        let (status, _, err) = run(&["-Sn", "70000"]);
        assert_eq!(status, 1);
        assert!(err[0].contains("cannot modify limit"), "{err:?}");
        assert_eq!(run(&["-n", "unlimited"]).0, 1);
        // The pipe size is fixed; numbers must be numbers.
        assert_eq!(run(&["-p", "16"]).0, 1);
        assert_eq!(run(&["-d", "abc"]).0, 1);
        assert_eq!(run(&["-d", "unlimited"]).0, 0);
        // Options Git Bash lacks.
        assert!(ULimitCommand::try_parse_from(["ulimit", "-l"]).is_err());
    }
}
