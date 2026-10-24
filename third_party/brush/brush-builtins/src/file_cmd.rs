//! sill: `file` on Windows, for what agents use it for.
//!
//! Windows has no `file`, and agents reach for it to check line endings before an edit
//! ("ASCII text, with CRLF line terminators"), to tell text from binary, and to see what
//! an executable is. This answers those from the bytes, in `file`'s own wording:
//! text (ASCII, UTF-8, BOMs, UTF-16), line terminators, Windows PE executables and DLLs
//! (architecture, console or GUI), ELF, `#!` scripts, and common container and image
//! formats. Anything else is `data`. Options: `-b`, `-i`/`--mime`, `--mime-type`, `-L`
//! (accepted; links are followed anyway).

use clap::Parser;
use std::io::{Read, Write};

use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// Determine file type (Windows subset of file(1)).
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub(crate) struct FileCommand {
    /// Do not prepend file names.
    #[arg(short = 'b', long = "brief")]
    brief: bool,
    /// Output MIME type strings.
    #[arg(short = 'i', long = "mime")]
    mime: bool,
    /// Output the MIME type only.
    #[arg(long = "mime-type")]
    mime_type: bool,
    /// Follow symlinks (always done).
    #[arg(short = 'L', long = "dereference")]
    _deref: bool,
    files: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(FileCommand);

/// A description and a MIME type.
#[derive(Debug, PartialEq, Eq)]
struct Kind {
    desc: String,
    mime: String,
}

fn kind(desc: impl Into<String>, mime: impl Into<String>) -> Kind {
    Kind {
        desc: desc.into(),
        mime: mime.into(),
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn pe(b: &[u8]) -> Option<Kind> {
    let off = u32_at(b, 0x3c)? as usize;
    if b.get(off..off + 4)? != b"PE\0\0" {
        return Some(kind("MS-DOS executable", "application/x-dosexec"));
    }
    let machine = match u16_at(b, off + 4)? {
        0x8664 => "x86-64",
        0x014c => "Intel 80386",
        0xaa64 => "Aarch64",
        0x01c4 => "ARMv7 Thumb",
        _ => "unknown processor",
    };
    let characteristics = u16_at(b, off + 22)?;
    let opt = off + 24;
    let bits = match u16_at(b, opt)? {
        0x20b => "PE32+",
        _ => "PE32",
    };
    let subsystem = match u16_at(b, opt + 68)? {
        2 => "(GUI)",
        3 => "(console)",
        10..=13 => "(EFI application)",
        1 => "(native)",
        _ => "",
    };
    let what = if characteristics & 0x2000 != 0 {
        "executable (DLL)"
    } else {
        "executable"
    };
    Some(kind(
        format!("{bits} {what} {subsystem} {machine}, for MS Windows").replace("  ", " "),
        "application/vnd.microsoft.portable-executable",
    ))
}

fn script(first_line: &str) -> Kind {
    let line = first_line.trim_start_matches("#!").trim();
    let mut words = line.split_whitespace();
    let mut interp = words
        .next()
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_owned();
    if interp == "env" {
        words
            .find(|w| !w.starts_with('-'))
            .unwrap_or("")
            .clone_into(&mut interp);
    }
    let (desc, mime) = match interp.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.') {
        "bash" => ("Bourne-Again shell script", "text/x-shellscript"),
        "sh" | "dash" | "ash" => ("POSIX shell script", "text/x-shellscript"),
        "zsh" => ("Paul Falstad's zsh script", "text/x-shellscript"),
        "python" => ("Python script", "text/x-script.python"),
        "node" => ("Node.js script", "application/javascript"),
        "perl" => ("Perl script", "text/x-perl"),
        "ruby" => ("Ruby script", "text/x-ruby"),
        _ => return kind(format!("a {interp} script"), "text/plain"),
    };
    kind(desc, mime)
}

/// Text: encoding and line terminators, as file words them.
fn text(b: &[u8]) -> Option<Kind> {
    let (body, enc, charset) = if let Some(rest) = b.strip_prefix(b"\xef\xbb\xbf") {
        std::str::from_utf8(rest).ok()?;
        (rest, "Unicode text, UTF-8 (with BOM) text", "utf-8")
    } else if b.starts_with(b"\xff\xfe") || b.starts_with(b"\xfe\xff") {
        let le = b.starts_with(b"\xff\xfe");
        return Some(kind(
            format!(
                "Unicode text, UTF-16, {}-endian text",
                if le { "little" } else { "big" }
            ),
            format!("text/plain; charset=utf-16{}", if le { "le" } else { "be" }),
        ));
    } else if b
        .iter()
        .all(|&c| c != 0 && (c >= 0x20 || matches!(c, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)))
    {
        if b.is_ascii() {
            (b, "ASCII text", "us-ascii")
        } else if std::str::from_utf8(b).is_ok()
            || std::str::from_utf8(&b[..b.len().saturating_sub(3)]).is_ok()
        {
            (b, "Unicode text, UTF-8 text", "utf-8")
        } else {
            (b, "ISO-8859 text", "iso-8859-1")
        }
    } else {
        return None;
    };
    if body.contains(&0) {
        return None;
    }
    let (mut crlf, mut lf, mut cr, mut longest, mut cur) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'\r' if body.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
                longest = longest.max(cur);
                cur = 0;
            }
            b'\r' => {
                cr += 1;
                longest = longest.max(cur);
                cur = 0;
            }
            b'\n' => {
                lf += 1;
                longest = longest.max(cur);
                cur = 0;
            }
            _ => cur += 1,
        }
        i += 1;
    }
    longest = longest.max(cur);
    let mut parts = vec![];
    if longest > 300 {
        parts.push(format!("very long lines ({longest})"));
    }
    let terms: Vec<&str> = [(crlf, "CRLF"), (cr, "CR"), (lf, "LF")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(_, s)| *s)
        .collect();
    match terms.as_slice() {
        [] if !body.is_empty() => parts.push("no line terminators".to_owned()),
        ["LF"] | [] => {}
        _ => parts.push(format!("{} line terminators", terms.join(", "))),
    }
    let desc = if parts.is_empty() {
        enc.to_owned()
    } else {
        format!("{enc}, with {}", parts.join(", with "))
    };
    Some(kind(desc, format!("text/plain; charset={charset}")))
}

/// What the first bytes of a file say it is.
fn identify(b: &[u8]) -> Kind {
    if b.is_empty() {
        return kind("empty", "inode/x-empty");
    }
    let magic: &[(&[u8], &str, &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "PNG image data", "image/png"),
        (b"\xff\xd8\xff", "JPEG image data", "image/jpeg"),
        (b"GIF8", "GIF image data", "image/gif"),
        (b"%PDF-", "PDF document", "application/pdf"),
        (b"PK\x03\x04", "Zip archive data", "application/zip"),
        (b"PK\x05\x06", "Zip archive data (empty)", "application/zip"),
        (b"\x1f\x8b", "gzip compressed data", "application/gzip"),
        (b"BZh", "bzip2 compressed data", "application/x-bzip2"),
        (b"\xfd7zXZ\0", "XZ compressed data", "application/x-xz"),
        (
            b"7z\xbc\xaf\x27\x1c",
            "7-zip archive data",
            "application/x-7z-compressed",
        ),
        (
            b"\x28\xb5\x2f\xfd",
            "Zstandard compressed data",
            "application/zstd",
        ),
        (
            b"SQLite format 3\0",
            "SQLite 3.x database",
            "application/vnd.sqlite3",
        ),
        (
            b"\0asm",
            "WebAssembly (wasm) binary module",
            "application/wasm",
        ),
        (
            b"MSCF",
            "Microsoft Cabinet archive data",
            "application/vnd.ms-cab-compressed",
        ),
        (
            b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1",
            "Composite Document File V2 Document",
            "application/x-ole-storage",
        ),
    ];
    if b.starts_with(b"MZ") {
        if let Some(k) = pe(b) {
            return k;
        }
    }
    if b.starts_with(b"\x7fELF") {
        let bits = if b.get(4) == Some(&2) {
            "64-bit"
        } else {
            "32-bit"
        };
        return kind(
            format!("ELF {bits} LSB executable"),
            "application/x-executable",
        );
    }
    for (m, desc, mime) in magic {
        if b.starts_with(m) {
            let desc = if *m == b"%PDF-" {
                let v: String = b[5..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit() || **c == b'.')
                    .map(|&c| c as char)
                    .collect();
                format!("{desc}, version {v}")
            } else {
                (*desc).to_owned()
            };
            return kind(desc, *mime);
        }
    }
    if b.len() > 262 && &b[257..262] == b"ustar" {
        return kind("POSIX tar archive", "application/x-tar");
    }
    if let Some(t) = text(b) {
        if b.starts_with(b"#!") {
            let first = String::from_utf8_lossy(b.split(|&c| c == b'\n').next().unwrap_or(b""));
            let s = script(&first);
            let enc = t.desc.split(',').next().unwrap_or("ASCII text").to_owned();
            let rest = t.desc.strip_prefix(&enc).unwrap_or("");
            return kind(format!("{}, {enc} executable{rest}", s.desc), s.mime);
        }
        return t;
    }
    kind("data", "application/octet-stream")
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for FileCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();
        if self.files.is_empty() {
            writeln!(
                context.stderr(),
                "Usage: {name} [-bi] [--mime-type] FILE..."
            )?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        }
        let width = self.files.iter().map(|f| f.len()).max().unwrap_or(0) + 1;
        let mut status = ExecutionResult::success();
        for f in &self.files {
            let path = brush_core::sys::fs::map_shell_path(std::path::Path::new(f))
                .unwrap_or_else(|| f.into());
            let path = context.shell.absolute_path(&path);
            let k = match std::fs::metadata(&path) {
                Err(e) => {
                    status = ExecutionResult::general_error();
                    let why = if e.kind() == std::io::ErrorKind::NotFound {
                        "No such file or directory".to_owned()
                    } else {
                        e.to_string()
                    };
                    kind(format!("cannot open `{f}' ({why})"), "inode/x-error")
                }
                Ok(m) if m.is_dir() => kind("directory", "inode/directory"),
                Ok(_) => {
                    let mut head = vec![0u8; 64 * 1024];
                    let n = std::fs::File::open(&path)
                        .and_then(|mut fh| fh.read(&mut head))
                        .unwrap_or(0);
                    head.truncate(n);
                    identify(&head)
                }
            };
            let what = if self.mime_type {
                k.mime.split(';').next().unwrap_or("").to_owned()
            } else if self.mime {
                if k.mime.contains("charset") || !k.mime.starts_with("text/") {
                    k.mime.clone()
                } else {
                    format!("{}; charset=us-ascii", k.mime)
                }
            } else {
                k.desc.clone()
            };
            if self.brief {
                writeln!(context.stdout(), "{what}")?;
            } else {
                writeln!(context.stdout(), "{:<width$} {what}", format!("{f}:"))?;
            }
        }
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(b: &[u8]) -> String {
        identify(b).desc
    }

    #[test]
    fn text_and_line_endings() {
        assert_eq!(d(b"a\nb\n"), "ASCII text");
        assert_eq!(
            d(b"[server]\r\nport=1\r\n"),
            "ASCII text, with CRLF line terminators"
        );
        assert_eq!(d(b"a\r\nb\n"), "ASCII text, with CRLF, LF line terminators");
        assert_eq!(d(b"no newline"), "ASCII text, with no line terminators");
        assert_eq!(d("café\n".as_bytes()), "Unicode text, UTF-8 text");
        assert_eq!(
            d(b"\xef\xbb\xbfx\r\n"),
            "Unicode text, UTF-8 (with BOM) text, with CRLF line terminators"
        );
        assert_eq!(
            d(b"\xff\xfeh\0i\0"),
            "Unicode text, UTF-16, little-endian text"
        );
        assert!(d(&[b'x'; 400]).contains("very long lines (400)"));
        assert_eq!(d(b""), "empty");
        assert_eq!(d(b"\0\x01\x02bin"), "data");
    }

    #[test]
    fn scripts() {
        assert_eq!(
            d(b"#!/usr/bin/env bash\necho\n"),
            "Bourne-Again shell script, ASCII text executable"
        );
        assert_eq!(
            d(b"#!/bin/sh\r\necho\r\n"),
            "POSIX shell script, ASCII text executable, with CRLF line terminators"
        );
        assert_eq!(
            d(b"#!/usr/bin/env python3\nprint()\n"),
            "Python script, ASCII text executable"
        );
    }

    #[test]
    fn binaries() {
        // A minimal PE32+ console x86-64 header.
        let mut pe = vec![0u8; 512];
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[0x96..0x98].copy_from_slice(&0x0022u16.to_le_bytes());
        pe[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        pe[0x98 + 68..0x98 + 70].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(d(&pe), "PE32+ executable (console) x86-64, for MS Windows");
        pe[0x96..0x98].copy_from_slice(&0x2022u16.to_le_bytes());
        pe[0x98 + 68..0x98 + 70].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            d(&pe),
            "PE32+ executable (DLL) (GUI) x86-64, for MS Windows"
        );
        assert_eq!(d(b"\x89PNG\r\n\x1a\n...."), "PNG image data");
        assert_eq!(d(b"%PDF-1.7\n"), "PDF document, version 1.7");
        assert_eq!(d(b"PK\x03\x04rest"), "Zip archive data");
        assert_eq!(identify(b"\x1f\x8b\x08").mime, "application/gzip");
    }
}
