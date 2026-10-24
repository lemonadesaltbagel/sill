use clap::Parser;
use std::{ffi::OsString, io::Write, ops::ControlFlow};
use uucore::format;

use brush_core::{Error, ErrorKind, ExecutionResult, builtins, escape, expansion};

/// Format a string.
#[derive(Parser)]
#[clap(disable_help_flag = true, disable_version_flag = true)]
pub(crate) struct PrintfCommand {
    /// If specified, the output of the command is assigned to this variable.
    #[arg(short = 'v')]
    output_variable: Option<String>,

    /// Format string + arguments to the format string.
    ///
    /// N.B. We intentionally do *not* enable `allow_hyphen_values` here. Doing so would
    /// cause an attached short-option value such as `-va` (i.e. `-v a`) to be misparsed as
    /// a positional argument. With it disabled, a format string that genuinely needs to
    /// start with a hyphen must be preceded by `--`, matching other shells' behavior.
    #[arg(trailing_var_arg = true, required = true)]
    format_and_args: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(PrintfCommand);

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for PrintfCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        // sill: conversion errors (`printf %d abc`) go to this command's stderr, in bash's
        // wording, before the output they're part of, and make it fail; uucore would print
        // them to the process's stderr.
        format::capture_errors();
        let mut output: Vec<u8> = vec![];
        let formatted = format(self.format_and_args.as_slice(), &mut output);
        let errors = format::take_captured_errors();
        for e in &errors {
            writeln!(context.stderr(), "{}: {e}", context.command_name)?;
        }
        if let Some(variable_name) = &self.output_variable {
            formatted?;
            let result_str = String::from_utf8(output).map_err(|_| {
                brush_core::ErrorKind::PrintfInvalidUsage("invalid UTF-8 output".into())
            })?;

            // Assign to the selected variable.
            expansion::assign_to_named_parameter(
                context.shell,
                &context.params,
                variable_name,
                result_str,
            )
            .await?;
        } else {
            // What was formatted before a failure is still written, as bash does.
            let mut stdout = context.stdout();
            stdout.write_all(&output)?;
            stdout.flush()?;
            formatted?;
        }
        Ok(if errors.is_empty() {
            ExecutionResult::success()
        } else {
            ExecutionResult::general_error()
        })
    }
}

fn format(format_and_args: &[String], writer: impl Write) -> Result<(), brush_core::Error> {
    match format_and_args {
        // Handle format string with arguments using uucore
        [fmt, args @ ..] => format_via_uucore(fmt, args.iter(), writer),
        // Handle case with no format string (we shouldn't be able to get here since clap will
        // fail parsing when the format string is missing)
        [] => Err(ErrorKind::PrintfInvalidUsage("missing operand".into()).into()),
    }
}

fn format_via_uucore(
    format_string: &str,
    args: impl Iterator<Item = impl Into<OsString>>,
    mut writer: impl Write,
) -> Result<(), brush_core::Error> {
    // Convert string arguments to FormatArgument::Unparsed
    let format_args: Vec<_> = args
        .map(|s| format::FormatArgument::Unparsed(s.into()))
        .collect();

    // Parse format string once.
    let format_items = parse_format_string(format_string)?;

    // Wrap the format arguments.
    let mut format_args_wrapper = format::FormatArguments::new(&format_args);

    // Determine whether the format string contains any specifiers that consume arguments. If it
    // doesn't, then we must only run through it once -- even when extra arguments are provided --
    // since otherwise we'd loop forever waiting for arguments that will never be consumed. This
    // matches the behavior of other shells, which print such a format string exactly once.
    let format_consumes_args = format_items.iter().any(|piece| {
        matches!(
            piece,
            Piece::Item(format::FormatItem::Spec(_), _) | Piece::Time { .. }
        )
    });

    // Keep going until we've exhausted all format arguments. Also make sure to run at least once
    // even if there's no format arguments.
    while format_args.is_empty() || !format_args_wrapper.is_exhausted() {
        // Process all format items, in order. We'll bail when we're told to stop.
        for piece in &format_items {
            let (item, backslash_quote) = match piece {
                Piece::Item(item, backslash_quote) => (item, backslash_quote),
                Piece::Time { format, width, align_left } => {
                    write_time(&mut writer, &mut format_args_wrapper, format, *width, *align_left)?;
                    continue;
                }
            };
            if let (format::FormatItem::Spec(format::Spec::QuotedString { position }), true) =
                (item, *backslash_quote)
            {
                let arg = format_args_wrapper.next_string(*position).to_string_lossy();
                let quoted = quote_printf_q(&arg);
                write!(writer, "{quoted}")?;
                continue;
            }

            let control_flow = item
                .write(&mut writer, &mut format_args_wrapper)
                .map_err(|e| match e {
                    // Propagate I/O errors directly so they can be handled appropriately
                    format::FormatError::IoError(io_err) => Error::from(io_err),
                    // Wrap other format errors
                    other => Error::from(ErrorKind::PrintfInvalidUsage(std::format!(
                        "printf formatting error: {other}"
                    ))),
                })?;

            if control_flow == ControlFlow::Break(()) {
                break;
            }
        }

        // If the format string doesn't consume any arguments, stop now; otherwise we'd reprocess
        // it forever since no arguments will ever be consumed.
        if !format_consumes_args {
            break;
        }

        // Start next batch if not exhausted
        if !format_args_wrapper.is_exhausted() {
            format_args_wrapper.start_next_batch();
        }

        if format_args.is_empty() {
            break;
        }
    }

    Ok(())
}

fn quote_printf_q(s: &str) -> String {
    // Like bash's, this escapes commas too, so the result is safe even inside a brace
    // expansion.
    let options = escape::QuoteOptions::builder()
        .preferred_mode(escape::QuoteMode::BackslashEscape)
        .quote_commas(true)
        .build();
    escape::quote(s, &options).into_owned()
}

type ParsedFormatItem = (format::FormatItem<format::EscapedChar>, bool);

/// A piece of a format string: what uucore formats, or (sill) bash's `%(datefmt)T`, which
/// uucore doesn't know.
enum Piece {
    Item(format::FormatItem<format::EscapedChar>, bool),
    Time {
        format: String,
        width: usize,
        align_left: bool,
    },
}

/// sill: splits `%[-][width](datefmt)T` specs out of a format string; the text between them
/// goes to uucore.
fn parse_format_string(format_string: &str) -> Result<Vec<Piece>, brush_core::Error> {
    let mut pieces = Vec::new();
    let mut literal_start = 0;
    let mut i = 0;
    let bytes = format_string.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'%') {
            i += 2;
            continue;
        }
        if let Some((spec_len, time)) = parse_time_spec(&format_string[i..]) {
            let literal = &format_string[literal_start..i];
            pieces.extend(parse_uucore_items(literal)?.into_iter().map(|(item, q)| Piece::Item(item, q)));
            pieces.push(time);
            i += spec_len;
            literal_start = i;
            continue;
        }
        i += 1;
    }
    pieces.extend(
        parse_uucore_items(&format_string[literal_start..])?
            .into_iter()
            .map(|(item, q)| Piece::Item(item, q)),
    );
    Ok(pieces)
}

/// Parses `%[-][width](datefmt)T` at the start of `s`: its length and the piece.
fn parse_time_spec(s: &str) -> Option<(usize, Piece)> {
    let after_percent = s.strip_prefix('%')?;
    let (align_left, after_flag) = match after_percent.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, after_percent),
    };
    let digits = after_flag.chars().take_while(char::is_ascii_digit).count();
    let (width, after_width) = after_flag.split_at(digits);
    let inner_and_rest = after_width.strip_prefix('(')?;
    let (format, after) = inner_and_rest.split_once(")T")?;
    let consumed = s.len() - after.len();
    Some((
        consumed,
        Piece::Time {
            format: format.to_owned(),
            width: width.parse().unwrap_or(0),
            align_left,
        },
    ))
}

/// sill: writes the next argument (seconds since the epoch; empty or -1 for now, -2 for
/// the shell's start, also taken as now) as a local time in `datefmt` (strftime), padded
/// to `width`. An empty format is `%X`, as in bash.
fn write_time(
    mut writer: impl Write,
    args: &mut format::FormatArguments<'_>,
    datefmt: &str,
    width: usize,
    align_left: bool,
) -> Result<(), brush_core::Error> {
    use chrono::TimeZone as _;
    use std::fmt::Write as _;

    // The argument, through uucore's own cursor: a `%s` written to a buffer.
    let mut as_s: &[u8] = b"s";
    let spec = format::Spec::parse(&mut as_s)
        .map_err(|_| ErrorKind::PrintfInvalidUsage("printf: %T".into()))?;
    let mut arg = Vec::new();
    let _: ControlFlow<()> = format::FormatItem::<format::EscapedChar>::Spec(spec)
        .write(&mut arg, args)
        .map_err(|e| ErrorKind::PrintfInvalidUsage(format!("printf formatting error: {e}")))?;
    let arg = String::from_utf8_lossy(&arg);
    let secs: i64 = match arg.trim() {
        "" => -1,
        n => n.parse().unwrap_or(-1),
    };
    let time = if secs < 0 {
        chrono::Local::now()
    } else {
        chrono::Local
            .timestamp_opt(secs, 0)
            .single()
            .unwrap_or_else(chrono::Local::now)
    };
    let datefmt = if datefmt.is_empty() { "%X" } else { datefmt };
    let mut text = String::new();
    // An unknown conversion makes chrono's formatter fail; then the format is shown as is.
    if write!(text, "{}", time.format(datefmt)).is_err() {
        text = datefmt.to_owned();
    }
    let padded = if align_left {
        format!("{text:<width$}")
    } else {
        format!("{text:>width$}")
    };
    writer.write_all(padded.as_bytes())?;
    Ok(())
}

fn parse_uucore_items(format_string: &str) -> Result<Vec<ParsedFormatItem>, brush_core::Error> {
    let format_items: Result<Vec<_>, _> = format::parse_spec_and_escape(format_string.as_bytes())
        .map(|result| match result {
            Ok(item @ format::FormatItem::Spec(format::Spec::QuotedString { .. })) => {
                Ok((item, true))
            }
            Ok(item) => Ok((item, false)),
            // Fixed q/Q modifiers are deliberately ignored; dynamic modifiers remain unsupported
            // until uucore exposes quoted-string metadata.
            Err(format::FormatError::SpecError(spec, span))
                if matches!(spec.last(), Some(b'q' | b'Q'))
                    && !spec.contains(&b'*')
                    && !spec.contains(&b'$') =>
            {
                let mut bare_q: &[u8] = b"q";
                let item = format::Spec::parse(&mut bare_q)
                    .map(format::FormatItem::Spec)
                    .map_err(|spec| format::FormatError::SpecError(spec.to_vec(), span))?;
                Ok((item, !spec.contains(&b'#')))
            }
            Err(error) => Err(error),
        })
        .collect();

    // Observe any errors we encountered along the way.
    let format_items = format_items
        .map_err(|e| ErrorKind::PrintfInvalidUsage(format!("printf parsing error: {e}")))?;

    Ok(format_items)
}

#[cfg(test)]
#[expect(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn sprintf_via_uucore(
        format_string: &str,
        args: impl Iterator<Item = impl Into<OsString>>,
    ) -> Result<String> {
        let mut result = vec![];
        format_via_uucore(format_string, args, &mut result)?;

        Ok(String::from_utf8(result)?)
    }

    #[test]
    fn test_basic_sprintf() -> Result<()> {
        assert_eq!(sprintf_via_uucore("%s", std::iter::once(&"xyz"))?, "xyz");
        assert_eq!(sprintf_via_uucore(r"%d\n", std::iter::once(&"1"))?, "1\n");

        Ok(())
    }

    #[test]
    fn test_sprintf_without_args() -> Result<()> {
        let empty: [&str; 0] = [];

        assert_eq!(sprintf_via_uucore("xyz", empty.iter())?, "xyz");
        assert_eq!(sprintf_via_uucore("%s|", empty.iter())?, "|");

        Ok(())
    }

    #[test]
    fn test_sprintf_with_cycles() -> Result<()> {
        assert_eq!(sprintf_via_uucore("%s|", ["x", "y"].iter())?, "x|y|");

        Ok(())
    }
}
