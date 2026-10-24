//! sill: `apply_patch`, the file-editing command Codex's models are trained to use.
//!
//! Codex CLI intercepts `apply_patch` itself only in commands it starts with a bare `bash`
//! (not a `C:\...\bash.exe` path, and never in an MCP tool), and the program it puts on
//! PATH takes the patch as one argument, where models write a here-doc. Either way the
//! edit failed. This applies the patch from its argument or, without one, from stdin.
//!
//! The format:
//!
//! ```text
//! *** Begin Patch
//! *** Add File: path          then the file's lines, each prefixed with `+`
//! *** Delete File: path
//! *** Update File: path       optionally followed by `*** Move to: new/path`, then hunks:
//! @@ optional anchor line      context lines (` `), removals (`-`), additions (`+`)
//! *** End of File             (optional: the hunk ends at the end of the file)
//! *** End Patch
//! ```
//!
//! A hunk's lines are found after its anchor, if any, matching exactly, then ignoring
//! trailing and then surrounding whitespace. An updated file keeps its line endings
//! (CRLF stays CRLF) and whether it ended with one. Nothing is written unless every hunk
//! of every file applies.

use clap::Parser;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// Apply a patch in Codex's apply_patch format (from the argument, or stdin).
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub(crate) struct ApplyPatchCommand {
    /// The patch; read from stdin when absent.
    patch: Option<String>,
}

brush_clap_adapter::impl_parse_args!(ApplyPatchCommand);

#[derive(Debug, PartialEq, Eq)]
enum Op {
    Add { path: String, lines: Vec<String> },
    Delete { path: String },
    Update { path: String, move_to: Option<String>, hunks: Vec<Hunk> },
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Hunk {
    anchor: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    at_eof: bool,
}

fn parse(text: &str) -> Result<Vec<Op>, String> {
    let lines: Vec<&str> = text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == "*** Begin Patch")
        .ok_or("the patch must start with '*** Begin Patch'")?;
    let end = lines
        .iter()
        .rposition(|l| l.trim() == "*** End Patch")
        .ok_or("the patch must end with '*** End Patch'")?;
    if end <= start {
        return Err("'*** End Patch' comes before '*** Begin Patch'".into());
    }

    let mut ops = Vec::new();
    let mut i = start + 1;
    while i < end {
        let line = lines[i];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            i += 1;
            let mut body = Vec::new();
            while i < end && !lines[i].starts_with("*** ") {
                let l = lines[i];
                body.push(l.strip_prefix('+').ok_or_else(|| format!("in Add File {path}: line without '+': {l}"))?.to_owned());
                i += 1;
            }
            ops.push(Op::Add { path: path.trim().to_owned(), lines: body });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(Op::Delete { path: path.trim().to_owned() });
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            i += 1;
            let mut move_to = None;
            if let Some(to) = lines.get(i).and_then(|l| l.strip_prefix("*** Move to: ")) {
                move_to = Some(to.trim().to_owned());
                i += 1;
            }
            let mut hunks: Vec<Hunk> = Vec::new();
            let mut current: Option<Hunk> = None;
            while i < end {
                let l = lines[i];
                if l.starts_with("*** End of File") {
                    current.get_or_insert_with(Hunk::default).at_eof = true;
                    i += 1;
                    continue;
                }
                if l.starts_with("*** ") {
                    break;
                }
                if let Some(rest) = l.strip_prefix("@@") {
                    if let Some(h) = current.take() {
                        hunks.push(h);
                    }
                    let anchor = rest.trim();
                    current = Some(Hunk {
                        anchor: (!anchor.is_empty()).then(|| anchor.to_owned()),
                        ..Hunk::default()
                    });
                } else {
                    let h = current.get_or_insert_with(Hunk::default);
                    match l.chars().next() {
                        Some('+') => h.new.push(l[1..].to_owned()),
                        Some('-') => h.old.push(l[1..].to_owned()),
                        Some(' ') => {
                            h.old.push(l[1..].to_owned());
                            h.new.push(l[1..].to_owned());
                        }
                        // An empty line is an empty context line.
                        None => {
                            h.old.push(String::new());
                            h.new.push(String::new());
                        }
                        Some(_) => return Err(format!("in Update File {path}: unexpected line: {l}")),
                    }
                }
                i += 1;
            }
            hunks.extend(current);
            if hunks.is_empty() && move_to.is_none() {
                return Err(format!("Update File {path} has no hunks"));
            }
            ops.push(Op::Update { path: path.trim().to_owned(), move_to, hunks });
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            return Err(format!("unexpected line: {line}"));
        }
    }
    if ops.is_empty() {
        return Err("the patch changes no files".into());
    }
    Ok(ops)
}

/// Where `needle` occurs in `hay` at or after `from`, comparing lines with `eq`.
fn find_with(hay: &[String], needle: &[String], from: usize, eq: impl Fn(&str, &str) -> bool) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(hay.len()));
    }
    (from..=hay.len().checked_sub(needle.len())?).find(|&i| hay[i..i + needle.len()].iter().zip(needle).all(|(a, b)| eq(a, b)))
}

fn find(hay: &[String], needle: &[String], from: usize, at_eof: bool) -> Option<usize> {
    let tests: [fn(&str, &str) -> bool; 3] = [|a, b| a == b, |a, b| a.trim_end() == b.trim_end(), |a, b| a.trim() == b.trim()];
    if at_eof && hay.len() >= needle.len() {
        let at = hay.len() - needle.len();
        if at >= from {
            for eq in tests {
                if find_with(hay, needle, at, eq) == Some(at) {
                    return Some(at);
                }
            }
        }
    }
    tests.into_iter().find_map(|eq| find_with(hay, needle, from, eq))
}

/// A file's text split into lines, with its line ending and whether it ended with one.
struct Text {
    lines: Vec<String>,
    crlf: bool,
    final_newline: bool,
}

impl Text {
    fn from(content: &str) -> Self {
        let crlf = content.contains("\r\n");
        let final_newline = content.ends_with('\n');
        let body = content.strip_suffix('\n').unwrap_or(content);
        let body = if crlf { body.strip_suffix('\r').unwrap_or(body) } else { body };
        let lines = if content.is_empty() {
            Vec::new()
        } else {
            body.split('\n').map(|l| if crlf { l.strip_suffix('\r').unwrap_or(l) } else { l }.to_owned()).collect()
        };
        Self { lines, crlf, final_newline }
    }

    fn render(&self) -> String {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut out = self.lines.join(nl);
        if self.final_newline && !self.lines.is_empty() {
            out.push_str(nl);
        }
        out
    }
}

fn apply_hunks(path: &str, content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut text = Text::from(content);
    let mut pos = 0;
    for h in hunks {
        if let Some(anchor) = &h.anchor {
            let at = text.lines[pos..]
                .iter()
                .position(|l| l.trim() == anchor.trim())
                .or_else(|| text.lines[pos..].iter().position(|l| l.contains(anchor.trim())))
                .ok_or_else(|| format!("{path}: no line '{anchor}' to anchor a hunk"))?;
            pos += at + 1;
        }
        let at = if h.old.is_empty() {
            // Only additions: at the anchor, or at the end of the file.
            if h.anchor.is_some() && !h.at_eof { pos } else { text.lines.len() }
        } else {
            find(&text.lines, &h.old, pos, h.at_eof)
                .ok_or_else(|| format!("{path}: these lines aren't in the file:\n{}", h.old.join("\n")))?
        };
        text.lines.splice(at..at + h.old.len(), h.new.iter().cloned());
        pos = at + h.new.len();
    }
    if text.lines.is_empty() {
        text.final_newline = false;
    } else if !text.final_newline && content.is_empty() {
        text.final_newline = true;
    }
    Ok(text.render())
}

/// What applying the patch does to the files: (path, new content or `None` to delete).
type Plan = Vec<(PathBuf, Option<String>, String)>;

fn plan(ops: &[Op], resolve: &dyn Fn(&str) -> PathBuf) -> Result<Plan, String> {
    let mut plan = Vec::new();
    for op in ops {
        match op {
            Op::Add { path, lines } => {
                let mut content = lines.join("\n");
                content.push('\n');
                plan.push((resolve(path), Some(content), format!("A {path}")));
            }
            Op::Delete { path } => {
                let p = resolve(path);
                if !p.is_file() {
                    return Err(format!("{path}: No such file or directory"));
                }
                plan.push((p, None, format!("D {path}")));
            }
            Op::Update { path, move_to, hunks } => {
                let p = resolve(path);
                let content = std::fs::read_to_string(&p).map_err(|e| format!("{path}: {}", sill_message(&e)))?;
                let new = apply_hunks(path, &content, hunks)?;
                if let Some(to) = move_to {
                    plan.push((resolve(to), Some(new), format!("M {to}")));
                    plan.push((p, None, String::new()));
                } else {
                    plan.push((p, Some(new), format!("M {path}")));
                }
            }
        }
    }
    Ok(plan)
}

fn sill_message(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound { "No such file or directory".into() } else { e.to_string() }
}

fn write(plan: &Plan) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for (path, content, note) in plan {
        match content {
            Some(text) => {
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), sill_message(&e)))?;
                }
                std::fs::write(path, text).map_err(|e| format!("{}: {}", path.display(), sill_message(&e)))?;
            }
            None => std::fs::remove_file(path).map_err(|e| format!("{}: {}", path.display(), sill_message(&e)))?,
        }
        if !note.is_empty() {
            done.push(note.clone());
        }
    }
    Ok(done)
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for ApplyPatchCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();
        let patch = match self.patch {
            Some(p) => p,
            None => {
                let mut buf = Vec::new();
                context.stdin().read_to_end(&mut buf)?;
                String::from_utf8_lossy(&buf).into_owned()
            }
        };
        let resolve = |p: &str| -> PathBuf {
            let path = brush_core::sys::fs::map_shell_path(Path::new(p)).unwrap_or_else(|| p.into());
            context.shell.absolute_path(&path)
        };
        let result = parse(&patch).and_then(|ops| plan(&ops, &resolve)).and_then(|p| write(&p));
        match result {
            Ok(done) => {
                writeln!(context.stdout(), "Success. Updated the following files:")?;
                for d in done {
                    writeln!(context.stdout(), "{d}")?;
                }
                Ok(ExecutionResult::success())
            }
            Err(e) => {
                writeln!(context.stderr(), "{name}: {e}")?;
                Ok(ExecutionExitCode::GeneralError.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks(patch: &str) -> Vec<Hunk> {
        match parse(patch).unwrap().into_iter().next().unwrap() {
            Op::Update { hunks, .. } => hunks,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_every_kind() {
        let ops = parse(
            "*** Begin Patch\n*** Add File: a/new.txt\n+one\n+two\n*** Delete File: old.txt\n\
             *** Update File: x.py\n*** Move to: y.py\n@@ def f():\n-    return 1\n+    return 2\n*** End Patch\n",
        )
        .unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0], Op::Add { path: "a/new.txt".into(), lines: vec!["one".into(), "two".into()] });
        assert_eq!(ops[1], Op::Delete { path: "old.txt".into() });
        let Op::Update { move_to, hunks, .. } = &ops[2] else { panic!() };
        assert_eq!(move_to.as_deref(), Some("y.py"));
        assert_eq!(hunks[0].anchor.as_deref(), Some("def f():"));
        assert!(parse("no patch here").is_err());
    }

    #[test]
    fn keeps_crlf_and_the_rest_of_the_file() {
        let h = hunks("*** Begin Patch\r\n*** Update File: app.ini\r\n@@\r\n-port=8080\r\n+port=9090\r\n*** End Patch\r\n");
        let out = apply_hunks("app.ini", "[server]\r\nhost=0.0.0.0\r\nport=8080\r\n\r\n[log]\r\n", &h).unwrap();
        assert_eq!(out, "[server]\r\nhost=0.0.0.0\r\nport=9090\r\n\r\n[log]\r\n");
        // No final newline stays none.
        let h = hunks("*** Begin Patch\n*** Update File: f\n-a\n+b\n*** End Patch");
        assert_eq!(apply_hunks("f", "x\na", &h).unwrap(), "x\nb");
    }

    #[test]
    fn finds_lines_after_the_anchor_and_despite_whitespace() {
        let h = hunks(
            "*** Begin Patch\n*** Update File: f.py\n@@ def second():\n-    return 1\n+    return 22\n*** End Patch",
        );
        let src = "def first():\n    return 1\n\ndef second():\n    return 1   \n";
        assert_eq!(
            apply_hunks("f.py", src, &h).unwrap(),
            "def first():\n    return 1\n\ndef second():\n    return 22\n"
        );
        let h = hunks("*** Begin Patch\n*** Update File: f\n-nothing like this\n+x\n*** End Patch");
        assert!(apply_hunks("f", "a\nb\n", &h).unwrap_err().contains("aren't in the file"));
    }

    #[test]
    fn additions_only_go_to_the_end() {
        let h = hunks("*** Begin Patch\n*** Update File: f\n@@\n+c\n*** End of File\n*** End Patch");
        assert_eq!(apply_hunks("f", "a\nb\n", &h).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn writes_nothing_unless_all_of_it_applies() {
        let dir = std::env::temp_dir().join(format!("sill-apply-patch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), "a\n").unwrap();
        let resolve = |p: &str| dir.join(p);
        let ops = parse(
            "*** Begin Patch\n*** Add File: sub/new.txt\n+hi\n*** Update File: keep.txt\n-missing\n+x\n*** End Patch",
        )
        .unwrap();
        assert!(plan(&ops, &resolve).is_err());
        assert!(!dir.join("sub/new.txt").exists());

        let ops = parse(
            "*** Begin Patch\n*** Add File: sub/new.txt\n+hi\n*** Update File: keep.txt\n*** Move to: moved.txt\n-a\n+b\n*** End Patch",
        )
        .unwrap();
        let done = write(&plan(&ops, &resolve).unwrap()).unwrap();
        assert_eq!(done, ["A sub/new.txt", "M moved.txt"]);
        assert_eq!(std::fs::read_to_string(dir.join("sub/new.txt")).unwrap(), "hi\n");
        assert_eq!(std::fs::read_to_string(dir.join("moved.txt")).unwrap(), "b\n");
        assert!(!dir.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
