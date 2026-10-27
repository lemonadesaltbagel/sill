//! sill: `chmod` on Windows.
//!
//! Windows has no chmod (and Git for Windows' MSYS one, where it's on PATH, reads `/tmp` as
//! its own folder), so `chmod +x build.sh && ./build.sh`, about the most common thing an
//! agent writes, failed with "command not found". Windows' only permission bit a mode maps
//! to is write: a mode without the owner's write permission makes the file read-only, one
//! with it makes it writable. Execute and the rest have no equivalent (a program runs by its
//! extension; sill runs `#!` scripts whatever their mode), so `+x` succeeds and changes
//! nothing.

use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// Change file mode bits (on Windows: whether a file is read-only).
#[derive(Parser)]
pub(crate) struct ChmodCommand {
    /// Change files and directories recursively.
    #[arg(short = 'R', long = "recursive")]
    recursive: bool,

    /// Suppress most error messages.
    #[arg(short = 'f', long = "silent", visible_alias = "quiet")]
    quiet: bool,

    /// Output a diagnostic for every file processed.
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Like verbose but report only when a change is made.
    #[arg(short = 'c', long = "changes")]
    changes: bool,

    /// MODE, then FILE...
    #[arg(allow_hyphen_values = true, required = true)]
    args: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(ChmodCommand);

/// Whether a file is writable after applying `mode` to one that is (`writable`): `None`
/// for a mode that can't be read. Octal modes look at the owner's write bit (0200);
/// symbolic ones (`u+w`, `a-w`, `go=r`, `+x`, comma-separated) at what they do to the
/// owner's write permission (`who` empty or containing `u` or `a`).
pub(crate) fn writable_after(mode: &str, writable: bool) -> Option<bool> {
    if !mode.is_empty() && mode.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        let bits = u32::from_str_radix(mode, 8).ok()?;
        if mode.len() > 4 {
            return None;
        }
        return Some(bits & 0o200 != 0);
    }
    let mut w = writable;
    for clause in mode.split(',') {
        let op_at = clause.find(['+', '-', '='])?;
        let (who, rest) = clause.split_at(op_at);
        if !who.bytes().all(|b| b"ugoa".contains(&b)) {
            return None;
        }
        let owner = who.is_empty() || who.contains('u') || who.contains('a');
        // `rest` is one or more `<op><perms>` actions: `u+w-x`.
        let mut chars = rest.chars().peekable();
        while let Some(op) = chars.next() {
            if !matches!(op, '+' | '-' | '=') {
                return None;
            }
            let mut perms = String::new();
            while let Some(&c) = chars.peek() {
                if matches!(c, '+' | '-' | '=') {
                    break;
                }
                perms.push(c);
                chars.next();
            }
            if !perms.chars().all(|c| "rwxXstugo".contains(c)) {
                return None;
            }
            if owner {
                let has_w = perms.contains('w');
                match op {
                    '+' if has_w => w = true,
                    '-' if has_w => w = false,
                    '=' => w = has_w,
                    _ => {}
                }
            }
        }
    }
    Some(w)
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for ChmodCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();
        let Some((mode, files)) = self.args.split_first().filter(|(_, f)| !f.is_empty()) else {
            writeln!(context.stderr(), "{name}: missing operand")?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        };
        if writable_after(mode, true).is_none() {
            writeln!(context.stderr(), "{name}: invalid mode: '{mode}'")?;
            return Ok(ExecutionResult::general_error());
        }

        let mut result = ExecutionResult::success();
        for file in files {
            let path = context.shell.absolute_path(file);
            let mut targets = vec![path.clone()];
            if self.recursive && path.is_dir() {
                let mut stack = vec![path];
                while let Some(dir) = stack.pop() {
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_dir() {
                                stack.push(p.clone());
                            }
                            targets.push(p);
                        }
                    }
                }
            }
            for target in targets {
                let shown = if target.as_path() == context.shell.absolute_path(file) {
                    file.clone()
                } else {
                    target.to_string_lossy().replace('\\', "/")
                };
                let meta = match std::fs::metadata(&target) {
                    Ok(meta) => meta,
                    Err(e) => {
                        if !self.quiet {
                            writeln!(
                                context.stderr(),
                                "{name}: cannot access '{shown}': {}",
                                brush_core::error::io_message(&e)
                            )?;
                        }
                        result = ExecutionResult::general_error();
                        continue;
                    }
                };
                let mut perms = meta.permissions();
                let was_writable = !perms.readonly();
                let writable = writable_after(mode, was_writable).unwrap_or(was_writable);
                if writable != was_writable {
                    perms.set_readonly(!writable);
                    if let Err(e) = std::fs::set_permissions(&target, perms) {
                        if !self.quiet {
                            writeln!(
                                context.stderr(),
                                "{name}: changing permissions of '{shown}': {}",
                                brush_core::error::io_message(&e)
                            )?;
                        }
                        result = ExecutionResult::general_error();
                        continue;
                    }
                    if self.verbose || self.changes {
                        writeln!(context.stdout(), "mode of '{shown}' changed ({})", if writable { "writable" } else { "read-only" })?;
                    }
                } else if self.verbose {
                    writeln!(context.stdout(), "mode of '{shown}' retained")?;
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::writable_after as w;

    #[test]
    fn modes_as_write_permission() {
        assert_eq!(w("+x", true), Some(true));
        assert_eq!(w("+x", false), Some(false));
        assert_eq!(w("a-w", true), Some(false));
        assert_eq!(w("-w", true), Some(false));
        assert_eq!(w("u+w", false), Some(true));
        assert_eq!(w("go-w", true), Some(true));
        assert_eq!(w("u=rx", true), Some(false));
        assert_eq!(w("u=rw,go=r", false), Some(true));
        assert_eq!(w("u+w-x", false), Some(true));
        assert_eq!(w("755", false), Some(true));
        assert_eq!(w("0444", true), Some(false));
        assert_eq!(w("644", false), Some(true));
        assert_eq!(w("z+w", true), None);
        assert_eq!(w("u+q", true), None);
        assert_eq!(w("99", true), None);
        assert_eq!(w("", true), None);
    }
}
