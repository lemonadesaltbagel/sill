//! sill: bytes from programs as text.
//!
//! A shell's strings are bytes; brush's are `String`s, so output that isn't UTF-8 has
//! to be turned into text somehow. Failing the whole command (`x=$(cmd /c echo 中文)`
//! on a Chinese system) is the worst way. On Windows, programs that don't write UTF-8
//! mostly write the system's ANSI code page (cmd's builtins, Java since JEP 400, older
//! MSVC tools, legacy text files), so that's tried next; anything else is replaced.

/// A stream's bytes as text, line by line: UTF-8, or, for a line that isn't valid UTF-8
/// (and holds no NUL, so isn't binary), the system's ANSI code page if that reads it
/// cleanly; otherwise UTF-8 with invalid bytes replaced.
pub fn decode_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => decode_with(bytes, system_ansi_code_page()),
    }
}

/// [`decode_output`] with the given ANSI code page (`None`: there's none to try).
pub fn decode_with(bytes: &[u8], ansi: Option<u32>) -> String {
    let mut text = String::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        match std::str::from_utf8(line) {
            Ok(s) => text.push_str(s),
            Err(_) => match ansi
                .filter(|_| !line.contains(&0))
                .and_then(|cp| from_code_page(line, cp))
            {
                Some(s) => text.push_str(&s),
                None => text.push_str(&String::from_utf8_lossy(line)),
            },
        }
    }
    text
}

/// The ANSI code page, unless it's UTF-8 already (or there's none).
#[cfg(windows)]
pub fn system_ansi_code_page() -> Option<u32> {
    // SAFETY: plain Win32 call.
    let acp = unsafe { windows_sys::Win32::Globalization::GetACP() };
    (acp != 65001).then_some(acp)
}

/// The ANSI code page, unless it's UTF-8 already (or there's none).
#[cfg(not(windows))]
pub const fn system_ansi_code_page() -> Option<u32> {
    None
}

/// `bytes` decoded in code page `cp`, if every byte sequence is valid there.
#[cfg(windows)]
pub fn from_code_page(bytes: &[u8], cp: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};
    let len = i32::try_from(bytes.len()).ok()?;
    // SAFETY: sizing call with valid input.
    let n = unsafe {
        MultiByteToWideChar(cp, MB_ERR_INVALID_CHARS, bytes.as_ptr(), len, std::ptr::null_mut(), 0)
    };
    if n <= 0 {
        return None;
    }
    let mut wide = vec![0u16; usize::try_from(n).ok()?];
    // SAFETY: the buffer holds the `n` units the sizing call asked for.
    let m = unsafe {
        MultiByteToWideChar(cp, MB_ERR_INVALID_CHARS, bytes.as_ptr(), len, wide.as_mut_ptr(), n)
    };
    (m == n).then(|| String::from_utf16_lossy(&wide))
}

/// `bytes` decoded in code page `cp`, if every byte sequence is valid there.
#[cfg(not(windows))]
pub const fn from_code_page(_bytes: &[u8], _cp: u32) -> Option<String> {
    None
}

/// Whether `b` starts a two-byte character in code page `cp` (936, 932, 949, 950).
#[cfg(windows)]
pub fn is_lead_byte(cp: u32, b: u8) -> bool {
    // SAFETY: plain Win32 call; code pages are u16-sized.
    u16::try_from(cp)
        .is_ok_and(|cp| unsafe { windows_sys::Win32::Globalization::IsDBCSLeadByteEx(cp.into(), b) } != 0)
}

/// Whether `b` starts a two-byte character in code page `cp` (936, 932, 949, 950).
#[cfg(not(windows))]
pub const fn is_lead_byte(_cp: u32, _b: u8) -> bool {
    false
}

/// The characters for bytes that weren't UTF-8, as `read` hands them out one at a time:
/// a two-byte character of the ANSI code page takes its second byte from `more` when
/// `bytes` ends after the first; a byte the code page doesn't read stays a byte (as the
/// Latin-1 character with its value, which is how brush always showed it).
pub fn decode_stray_bytes(
    bytes: &[u8],
    ansi: Option<u32>,
    mut more: impl FnMut() -> Option<u8>,
) -> Vec<char> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest: std::collections::VecDeque<u8> = bytes.iter().copied().collect();
    while let Some(b) = rest.pop_front() {
        if let Some(cp) = ansi.filter(|_| b >= 0x80) {
            if is_lead_byte(cp, b) {
                let trail = rest.pop_front().or_else(&mut more);
                if let Some(t) = trail {
                    if let Some(s) = from_code_page(&[b, t], cp) {
                        out.extend(s.chars());
                        continue;
                    }
                    rest.push_front(t);
                }
            } else if let Some(s) = from_code_page(&[b], cp) {
                out.extend(s.chars());
                continue;
            }
        }
        out.push(char::from(b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_falls_back_line_by_line() {
        // Without an ANSI code page, invalid UTF-8 is replaced.
        assert_eq!(decode_with(b"ok\n\xff\n", None), "ok\n\u{fffd}\n");
        assert_eq!(decode_with("中文\n".as_bytes(), Some(936)), "中文\n");
        assert_eq!(decode_output(b"a\xffb"), if cfg!(windows) && system_ansi_code_page().is_some() {
            decode_with(b"a\xffb", system_ansi_code_page())
        } else {
            "a\u{fffd}b".to_owned()
        });
    }

    #[test]
    fn stray_bytes_without_a_code_page_stay_bytes() {
        assert_eq!(decode_stray_bytes(b"\xd6\xd0", None, || None), ['\u{d6}', '\u{d0}']);
    }

    #[cfg(windows)]
    #[test]
    fn gbk_reads_as_gbk() {
        // 中文 in GBK, after a UTF-8 line, and a binary line left alone.
        let bytes = b"\xe2\x9c\x93 utf8\n\xd6\xd0\xce\xc4\n\x00\xd6\n";
        assert_eq!(decode_with(bytes, Some(936)), "\u{2713} utf8\n中文\n\0\u{fffd}\n");
        // A lead byte whose second byte is still to be read; one whose second is ASCII
        // (丂 is 0x81 0x40 in GBK).
        let mut next = [0xd0u8].into_iter();
        assert_eq!(decode_stray_bytes(b"\xd6", Some(936), || next.next()), ['中']);
        assert_eq!(decode_stray_bytes(b"\x81", Some(936), || Some(b'@')), ['丂']);
        // Café in Windows-1252.
        assert_eq!(decode_stray_bytes(b"\xe9", Some(1252), || None), ['é']);
    }
}
