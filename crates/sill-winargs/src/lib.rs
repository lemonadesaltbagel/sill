//! Encode an argv array into the single command-line string that Windows
//! `CreateProcessW` takes.
//!
//! Two targets need different rules:
//!
//! * [`encode_exe`]: the program parses its own command line with the C
//!   runtime rules (UCRT, the pre-2008 msvcrt rules, or `CommandLineToArgvW`).
//!   The encoder is Rust std's `append_arg` (Colascione's algorithm). It also
//!   quotes `*` and `?`, so children linked with wildcard expansion don't
//!   re-glob them, and it never emits `""`, so the result parses the same
//!   under all three rule sets.
//! * [`encode_bat`]: the target is a `.bat`/`.cmd` script run by `cmd.exe`.
//!   This mirrors Rust std's `make_bat_command_line` (the CVE-2024-24576 fix).
//!   The caller must pass `%SystemRoot%\System32\cmd.exe` as
//!   `lpApplicationName`.
//!
//! The spec and test vectors are `spec/argv-test-vectors.json`. Arguments are never
//! path-converted: `/p:Configuration=Release` passes through byte for byte.

use std::fmt;

/// `CreateProcessW` limit: 32,767 UTF-16 units including the terminating NUL.
pub const MAX_CMDLINE_UNITS: usize = 32_767 - 1;
/// `cmd.exe` limit: 8,191 characters including the terminating NUL.
pub const MAX_CMD_EXE_UNITS: usize = 8_191 - 1;

/// Characters that may appear unquoted in a batch argument.
/// ASCII alphanumerics are also safe.
const BAT_SAFE: &[char] = &['#', '$', '*', '+', '-', '.', '/', ':', '?', '@', '\\', '_'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    NulInArgument,
    NewlineInCmdCommand,
    InvalidUtf8,
    EmptyProgram,
    BadProgramName,
    ProgramHasNoFileName,
    TooLong,
    CrLfInBatchArgument,
    BadScriptName,
    BatchTooLong,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // These strings are the `expected_error` values in the spec vectors.
        f.write_str(match self {
            Self::NulInArgument => "NUL in argument",
            Self::NewlineInCmdCommand => "cmd.exe runs only the first line of a command with a newline",
            Self::InvalidUtf8 => "argument is not valid UTF-8",
            Self::EmptyProgram => "empty program name",
            Self::BadProgramName => "program name may not contain '\"' or NUL",
            Self::ProgramHasNoFileName => "program path has no file name",
            Self::TooLong => "command line exceeds 32767 UTF-16 units",
            Self::CrLfInBatchArgument => "CR/LF in batch argument",
            Self::BadScriptName => "Windows file names may not contain '\"' or end with '\\'",
            Self::BatchTooLong => "command line exceeds cmd.exe 8191-char limit",
        })
    }
}

impl std::error::Error for EncodeError {}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn exe_quote_needed(arg: &str) -> bool {
    arg.is_empty() || arg.contains([' ', '\t', '\n', '\u{0b}', '"', '*', '?'])
}

/// Quotes one argument for a CRT-parsing executable's command line (no leading
/// space). The result is what goes between the separating spaces.
pub fn quote_exe_arg(arg: &str) -> Result<String, EncodeError> {
    let mut out = String::with_capacity(arg.len() + 2);
    push_exe_arg(&mut out, arg)?;
    Ok(out)
}

fn push_exe_arg(out: &mut String, arg: &str) -> Result<(), EncodeError> {
    if arg.contains('\0') {
        return Err(EncodeError::NulInArgument);
    }
    if !exe_quote_needed(arg) {
        // Unquoted: backslashes are literal because no quote follows them.
        out.push_str(arg);
        return Ok(());
    }
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                continue;
            }
            // n backslashes before a quote become 2n+1, then the quote.
            '"' => {
                out.extend(std::iter::repeat_n('\\', 2 * backslashes + 1));
                out.push('"');
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(ch);
            }
        }
        backslashes = 0;
    }
    // Trailing backslashes are doubled so the closing quote stays a quote.
    out.extend(std::iter::repeat_n('\\', 2 * backslashes));
    out.push('"');
    Ok(())
}

/// Encode `argv` (with `argv[0]` the resolved program path) as
/// `lpCommandLine` for a CRT-parsing executable.
pub fn encode_exe<S: AsRef<str>>(argv: &[S]) -> Result<String, EncodeError> {
    let program = argv.first().map(AsRef::as_ref).unwrap_or("");
    if program.is_empty() {
        return Err(EncodeError::EmptyProgram);
    }
    if program.contains(['"', '\0']) {
        return Err(EncodeError::BadProgramName);
    }
    if program.ends_with(['\\', '/']) {
        return Err(EncodeError::ProgramHasNoFileName);
    }
    // argv[0] is always quoted and never escaped: its parsing has no escapes.
    let mut out = format!("\"{program}\"");
    for arg in &argv[1..] {
        out.push(' ');
        push_exe_arg(&mut out, arg.as_ref())?;
    }
    if utf16_len(&out) > MAX_CMDLINE_UNITS {
        return Err(EncodeError::TooLong);
    }
    Ok(out)
}

fn push_bat_arg(out: &mut String, arg: &str) -> Result<(), EncodeError> {
    if arg.contains('\0') {
        return Err(EncodeError::NulInArgument);
    }
    if arg.contains(['\r', '\n']) {
        return Err(EncodeError::CrLfInBatchArgument);
    }
    let quote = arg.is_empty()
        || arg.ends_with('\\')
        || arg.chars().any(|ch| {
            (ch.is_ascii() && !ch.is_ascii_alphanumeric() && !BAT_SAFE.contains(&ch))
                || ch.is_control()
        });
    if quote {
        out.push('"');
    }
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                out.push(ch);
                continue;
            }
            // Double the run of backslashes, then double the quote.
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push('"');
            }
            // `%` -> `%%cd:~,%`: a literal `%` followed by an empty substring
            // of %cd%, so cmd.exe can never pair it into a variable reference.
            '%' => out.push_str("%%cd:~,"),
            _ => {}
        }
        backslashes = 0;
        out.push(ch);
    }
    if quote {
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
    Ok(())
}

/// Encode `argv` (with `argv[0]` the resolved `.bat`/`.cmd` path) as
/// `lpCommandLine` for `cmd.exe`.
pub fn encode_bat<S: AsRef<str>>(argv: &[S]) -> Result<String, EncodeError> {
    let script = argv.first().map(AsRef::as_ref).unwrap_or("");
    if script.contains('"') || script.ends_with('\\') {
        return Err(EncodeError::BadScriptName);
    }
    let mut out = format!("cmd.exe /e:ON /v:OFF /d /c \"\"{script}\"");
    for arg in &argv[1..] {
        out.push(' ');
        push_bat_arg(&mut out, arg.as_ref())?;
    }
    out.push('"');
    if utf16_len(&out) > MAX_CMD_EXE_UNITS {
        return Err(EncodeError::BatchTooLong);
    }
    Ok(out)
}

/// Like [`encode_exe`], for byte-string arguments. The interpreter keeps
/// values as bytes; bytes that aren't UTF-8 can't become UTF-16.
pub fn encode_exe_bytes(argv: &[&[u8]]) -> Result<String, EncodeError> {
    let argv = argv
        .iter()
        .map(|a| std::str::from_utf8(a).map_err(|_| EncodeError::InvalidUtf8))
        .collect::<Result<Vec<_>, _>>()?;
    encode_exe(&argv)
}

/// Encodes the arguments of `cmd.exe` (everything after the program on its command
/// line). cmd.exe doesn't split its command line with the CRT rules: it takes the text
/// after `/c` (or `/k`) nearly as written, so the CRT encoding's `\"` escapes reach it
/// as literal text and break `cmd /c 'call "C:\Program Files\...\x.bat" && cl ...'`.
///
/// This uses Node's form for `shell: true`: the switches before `/c` (as plain cmd
/// words), then `/s /c "<command>"`. With `/s`, cmd removes exactly the first and last quote and runs
/// the rest as written. The command is the single argument after `/c` verbatim (the
/// usual `cmd /c '<whole command>'`), or several arguments joined by spaces, each with
/// plain quotes if it contains a space, tab or cmd metacharacter. Without `/c` or `/k`
/// the arguments are plain cmd words too. A newline is refused: cmd would run only the
/// first line.
pub fn encode_cmd_exe_args<S: AsRef<str>>(args: &[S]) -> Result<String, EncodeError> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let Some(c_at) = args
        .iter()
        .position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))
    else {
        if args.iter().any(|a| a.contains('\0')) {
            return Err(EncodeError::NulInArgument);
        }
        return Ok(args.iter().map(|a| cmd_word(a)).collect::<Vec<_>>().join(" "));
    };

    let mut out = Vec::new();
    let mut has_s = false;
    for a in &args[..c_at] {
        if a.contains('\0') {
            return Err(EncodeError::NulInArgument);
        }
        has_s |= a.eq_ignore_ascii_case("/s");
        out.push(cmd_word(a));
    }
    if !has_s {
        out.push("/s".to_owned());
    }
    out.push(args[c_at].to_owned());

    let rest = &args[c_at + 1..];
    for a in rest {
        if a.contains('\0') {
            return Err(EncodeError::NulInArgument);
        }
        if a.contains(['\r', '\n']) {
            return Err(EncodeError::NewlineInCmdCommand);
        }
    }
    let command = match rest {
        [single] => (*single).to_owned(),
        _ => rest.iter().map(|a| cmd_word(a)).collect::<Vec<_>>().join(" "),
    };
    out.push(format!("\"{command}\""));
    let line = out.join(" ");
    if utf16_len(&line) > MAX_CMD_EXE_UNITS {
        return Err(EncodeError::BatchTooLong);
    }
    Ok(line)
}

/// One word of a cmd.exe command: as written, or in plain double quotes if it has a
/// space, tab or metacharacter (cmd has no escape for a `"` inside quotes).
fn cmd_word(a: &str) -> String {
    if a.is_empty() {
        return "\"\"".to_owned();
    }
    if a.contains([' ', '\t', '&', '|', '<', '>', '(', ')', '^', ',', ';', '=']) && !a.starts_with('"') {
        format!("\"{a}\"")
    } else {
        a.to_owned()
    }
}

/// Whether `program` is cmd.exe (by file name, any case, with or without `.exe`).
pub fn is_cmd_exe(program: &std::path::Path) -> bool {
    // Split on both separators by hand: `Path` only knows `\` on Windows.
    let program = program.to_string_lossy();
    let name = program.rsplit(['/', '\\']).next().unwrap_or(&program);
    name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("cmd.exe")
}

#[cfg(test)]
mod cmd_exe_tests {
    use super::*;

    #[test]
    fn single_command_string_is_verbatim_under_s() {
        let got = encode_cmd_exe_args(&[
            "/c",
            r#"call "C:\Program Files\VS\vcvars64.bat" >nul && cl /nologo src\hello.c"#,
        ])
        .unwrap();
        assert_eq!(got, r#"/s /c "call "C:\Program Files\VS\vcvars64.bat" >nul && cl /nologo src\hello.c""#);
    }

    #[test]
    fn switches_before_c_are_kept() {
        assert_eq!(encode_cmd_exe_args(&["/d", "/c", "exit 3"]).unwrap(), r#"/d /s /c "exit 3""#);
        assert_eq!(encode_cmd_exe_args(&["/d", "/s", "/c", "ver"]).unwrap(), r#"/d /s /c "ver""#);
        assert_eq!(encode_cmd_exe_args(&["/D", "/K", "echo hi"]).unwrap(), r#"/D /s /K "echo hi""#);
    }

    #[test]
    fn several_words_are_quoted_when_needed() {
        assert_eq!(
            encode_cmd_exe_args(&["/c", "dir", "/b", r"C:\Program Files"]).unwrap(),
            r#"/s /c "dir /b "C:\Program Files"""#
        );
        assert_eq!(encode_cmd_exe_args(&["/c", "exit", "3"]).unwrap(), r#"/s /c "exit 3""#);
        assert_eq!(encode_cmd_exe_args(&["/c", "echo", "a&b"]).unwrap(), r#"/s /c "echo "a&b"""#);
    }

    #[test]
    fn newline_is_refused() {
        assert_eq!(
            encode_cmd_exe_args(&["/c", "echo a\necho b"]),
            Err(EncodeError::NewlineInCmdCommand)
        );
    }

    #[test]
    fn without_c_it_is_ordinary_encoding() {
        assert_eq!(encode_cmd_exe_args(&["/?"]).unwrap(), "/?");
        assert_eq!(encode_cmd_exe_args(&["/q", r"C:\a b"]).unwrap(), r#"/q "C:\a b""#);
    }

    #[test]
    fn detects_cmd_exe() {
        assert!(is_cmd_exe(std::path::Path::new(r"C:\Windows\System32\cmd.exe")));
        assert!(is_cmd_exe(std::path::Path::new("CMD.EXE")));
        assert!(is_cmd_exe(std::path::Path::new("cmd")));
        assert!(!is_cmd_exe(std::path::Path::new("cmdlet.exe")));
        assert!(!is_cmd_exe(std::path::Path::new("cmd.bat")));
    }
}

/// Git Bash's escape for a switch MSYS would otherwise take for a path: `//c` for `/c`
/// (`cmd //c ...`, `taskkill //F //PID 42`). Models trained on Git Bash examples write it;
/// for a native program it means the single-slash switch. Only a lone `//word` with no
/// further `/` qualifies, so UNC paths (`//server/share`) and URLs are untouched.
pub fn msys_double_slash_switch(arg: &str) -> Option<&str> {
    let rest = arg.strip_prefix("//")?;
    (!rest.is_empty() && !rest.starts_with('/') && !rest.contains(['/', '\\'])).then(|| &arg[1..])
}

#[cfg(test)]
mod msys_tests {
    use super::msys_double_slash_switch as f;

    #[test]
    fn double_slash_switches() {
        assert_eq!(f("//c"), Some("/c"));
        assert_eq!(f("//F"), Some("/F"));
        assert_eq!(f("//PID"), Some("/PID"));
        assert_eq!(f("//p:Configuration=Release"), Some("/p:Configuration=Release"));
        assert_eq!(f("//server/share"), None);
        assert_eq!(f("///x"), None);
        assert_eq!(f("//"), None);
        assert_eq!(f("/c"), None);
        assert_eq!(f("c"), None);
    }
}

/// `/dev/null` as a redirection target in text for cmd.exe, as `nul`.
///
/// Claude Code rewrites `>nul` to `>/dev/null` for its bash, including inside a quoted
/// `cmd.exe /c '... >nul && cl ...'` argument, which then reaches cmd.exe. cmd.exe takes it
/// for the path `\dev\null`, fails with "The system cannot find the path specified" and
/// never runs the rest: in every shell, the most common failure of the MSVC agent task.
/// A `/dev/null` redirection can't mean anything else to cmd.exe, so it becomes `nul`.
/// Only a redirection target outside double quotes and not `^`-escaped changes: `echo
/// "/dev/null"` and `type C:\dev\null` don't.
pub fn cmd_dev_null_redirects(text: &str) -> std::borrow::Cow<'_, str> {
    const TARGET: &str = "/dev/null";
    if !text.contains(TARGET) {
        return text.into();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut in_quotes = false;
    let mut i = 0;
    let mut changed = false;
    while i < bytes.len() {
        let c = bytes[i];
        if !in_quotes && c == b'^' && i + 1 < bytes.len() {
            // Escaped character: copied as is.
            let next = text[i + 1..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[i..i + 1 + next]);
            i += 1 + next;
            continue;
        }
        if c == b'"' {
            in_quotes = !in_quotes;
        }
        if !in_quotes && (c == b'>' || c == b'<') {
            out.push(c as char);
            i += 1;
            // `>>`
            if c == b'>' && i < bytes.len() && bytes[i] == b'>' {
                out.push('>');
                i += 1;
            }
            let ws_start = i;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            out.push_str(&text[ws_start..i]);
            let rest = &text[i..];
            let ends = |r: &str| r.is_empty() || r.starts_with([' ', '\t', '&', '|', ')', '<', '>', '\r', '\n']);
            if rest.starts_with(TARGET) && ends(&rest[TARGET.len()..]) {
                out.push_str("nul");
                i += TARGET.len();
                changed = true;
            }
            continue;
        }
        let len = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + len]);
        i += len;
    }
    if changed { out.into() } else { text.into() }
}

#[cfg(test)]
mod dev_null_tests {
    use super::cmd_dev_null_redirects as f;

    #[test]
    fn redirections_become_nul() {
        assert_eq!(f(r#"call "C:\vs\vcvars64.bat" >/dev/null && cl x.c"#), r#"call "C:\vs\vcvars64.bat" >nul && cl x.c"#);
        assert_eq!(f("a 2>/dev/null"), "a 2>nul");
        assert_eq!(f("a > /dev/null 2>&1"), "a > nul 2>&1");
        assert_eq!(f("a >>/dev/null"), "a >>nul");
        assert_eq!(f("a </dev/null"), "a <nul");
        assert_eq!(f("(a)>/dev/null&b"), "(a)>nul&b");
        assert_eq!(f("中文 >/dev/null"), "中文 >nul");
    }

    #[test]
    fn everything_else_stays() {
        for s in [
            r#"echo ">/dev/null""#,
            "echo ^>/dev/null",
            r"type C:\dev\null",
            "echo /dev/null",
            "a >/dev/nullx",
            "a >/dev/null/x",
            "no redirection",
        ] {
            assert_eq!(f(s), s, "{s}");
        }
    }
}
