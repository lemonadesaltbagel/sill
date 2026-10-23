//! POSIX wording for Windows system errors.
//!
//! A Windows error renders as Windows' own sentence ("The system cannot find the file
//! specified.", in the user's language), where Git Bash and Linux print `strerror`'s ("No
//! such file or directory"), the words agents and scripts look for. This maps the codes
//! file and process operations commonly fail with to glibc's wording, as Cygwin maps them
//! to errno values; others keep Windows' message.

/// glibc's message for the errno a Windows error code `code` stands for, if it's one of the
/// common ones.
#[must_use]
pub const fn posix_message(code: i32) -> Option<&'static str> {
    Some(match code {
        // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_INVALID_NAME, ERROR_INVALID_DRIVE,
        // ERROR_BAD_NETPATH, ERROR_BAD_NET_NAME, ERROR_BAD_PATHNAME
        2 | 3 | 15 | 53 | 67 | 123 | 161 => "No such file or directory",
        // ERROR_ACCESS_DENIED, ERROR_CANT_ACCESS_FILE
        5 | 1920 => "Permission denied",
        // ERROR_INVALID_HANDLE
        6 => "Bad file descriptor",
        // ERROR_NOT_ENOUGH_MEMORY, ERROR_OUTOFMEMORY
        8 | 14 => "Cannot allocate memory",
        // ERROR_WRITE_PROTECT
        19 => "Read-only file system",
        // ERROR_NOT_SAME_DEVICE
        17 => "Invalid cross-device link",
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION, ERROR_PIPE_BUSY, ERROR_BUSY
        32 | 33 | 231 | 170 => "Device or resource busy",
        // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
        39 | 112 => "No space left on device",
        // ERROR_NOT_SUPPORTED
        50 => "Operation not supported",
        // ERROR_FILE_EXISTS, ERROR_ALREADY_EXISTS
        80 | 183 => "File exists",
        // ERROR_INVALID_PARAMETER, ERROR_NEGATIVE_SEEK, ERROR_NOT_A_REPARSE_POINT
        87 | 131 | 4390 => "Invalid argument",
        // ERROR_BROKEN_PIPE, ERROR_NO_DATA
        109 | 232 => "Broken pipe",
        // ERROR_BAD_EXE_FORMAT, ERROR_EXE_MACHINE_TYPE_MISMATCH
        193 | 216 => "Exec format error",
        // ERROR_DIR_NOT_EMPTY
        145 => "Directory not empty",
        // ERROR_FILENAME_EXCED_RANGE
        206 => "File name too long",
        // ERROR_DIRECTORY (a file where a directory was expected)
        267 => "Not a directory",
        // ERROR_PRIVILEGE_NOT_HELD
        1314 => "Operation not permitted",
        // ERROR_CANT_RESOLVE_FILENAME (a symbolic link loop)
        1921 => "Too many levels of symbolic links",
        _ => return None,
    })
}

/// `err` as Rust renders it, with a common Windows error in POSIX wording:
/// "No such file or directory (os error 2)".
#[must_use]
pub fn message(err: &std::io::Error) -> String {
    #[cfg(windows)]
    if let Some(code) = err.raw_os_error()
        && let Some(text) = posix_message(code)
    {
        return format!("{text} (os error {code})");
    }
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_codes() {
        assert_eq!(posix_message(2), Some("No such file or directory"));
        assert_eq!(posix_message(5), Some("Permission denied"));
        assert_eq!(posix_message(145), Some("Directory not empty"));
        assert_eq!(posix_message(99_999), None);
        let custom = std::io::Error::other("custom");
        assert_eq!(message(&custom), "custom");
    }
}
