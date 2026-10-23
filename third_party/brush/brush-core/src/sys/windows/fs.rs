//! Filesystem utilities for Windows.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::error;

// Selectively re-export items from stubs that we don't override.
pub(crate) use crate::sys::stubs::fs::MetadataExt;

/// Cached list of executable extensions from the `PATHEXT` environment
/// variable. Each entry retains its leading dot (e.g. `".exe"`) and is stored
/// lowercased so case-insensitive comparisons can be done without allocating.
///
/// NOTE: This is cached for the process lifetime. Changes to `PATHEXT` made
/// inside the running shell are not reflected here. Bash itself has no
/// `PATHEXT` semantics, so this is generally acceptable for now.
static PATHEXT_EXTENSIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
});

/// Returns the stem of a PATHEXT entry (with any leading `.` removed).
///
/// `PATHEXT` canonically stores entries like `.EXE`, but tolerant parsing
/// accepts entries without the leading dot too.
fn pathext_entry_stem(entry: &str) -> &str {
    entry.strip_prefix('.').unwrap_or(entry)
}

/// Returns true if the path's extension is in the PATHEXT list.
///
/// Performs case-insensitive comparison against the cached PATHEXT entries
/// without allocating.
fn has_executable_extension(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        PATHEXT_EXTENSIONS
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(pathext_entry_stem(e)))
    })
}

/// Returns true if `path` is, by itself, an existing executable file.
///
/// Used both for the initial check in [`resolve_executable`] and for
/// [`PathExt::executable`].
fn is_executable_file(path: &Path) -> bool {
    has_executable_extension(path) && path.is_file()
}

/// Resolves an owned path to the actual on-disk executable file, if any.
///
/// If the path is already a file with a `PATHEXT` extension, it is returned
/// unchanged (no allocation). Otherwise, each `PATHEXT` extension is appended
/// in turn and the first existing file is returned.
pub fn resolve_executable(path: PathBuf) -> Option<PathBuf> {
    if is_executable_file(&path) {
        return Some(path);
    }
    // Try appending each PATHEXT extension.
    for ext in PATHEXT_EXTENSIONS.iter() {
        let mut name = path.as_os_str().to_owned();
        name.push(ext);
        let candidate = PathBuf::from(name);
        if candidate.is_file() {
            // sill: not the Store's stand-in for an uninstalled Python.
            if is_store_installer_stub(&candidate) {
                return None;
            }
            return Some(candidate);
        }
    }
    // sill: a `#!` script (`./build.sh`, `gradlew`) is a command too; see sys::shebang.
    (path.is_file() && crate::sys::shebang::file_has_shebang(&path)).then_some(path)
}

/// sill: whether `path` is one of the App Installer's stand-ins Windows puts on `PATH`
/// for programs that aren't installed (`WindowsApps\python3.exe`, `python.exe`). They
/// print nothing to a pipe and exit with an error (49 or 9009), so an agent running
/// `python3` saw silence. The real program further down `PATH` (a python.org `python.exe`)
/// is found instead, or `command not found` explains. A Store-installed Python's own
/// aliases in the same directory are real and stay.
fn is_store_installer_stub(path: &Path) -> bool {
    let in_windows_apps = path
        .parent()
        .and_then(|d| d.file_name())
        .is_some_and(|n| n.eq_ignore_ascii_case("WindowsApps"));
    if !in_windows_apps || path.metadata().map_or(true, |m| m.len() != 0) {
        return false;
    }
    app_exec_link_target(path).is_some_and(|t| t.to_ascii_lowercase().contains("appinstallerpythonredirector"))
}

/// The target executable of an App Execution Alias (reparse tag `APPEXECLINK`).
fn app_exec_link_target(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
    const IO_REPARSE_TAG_APPEXECLINK: u32 = 0x8000_001B;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the handle is checked and closed.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut buf = vec![0u8; 16 * 1024];
    let mut len = 0u32;
    // SAFETY: `buf` is a writable buffer of the given size.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null(),
            0,
            buf.as_mut_ptr().cast(),
            u32::try_from(buf.len()).unwrap_or(0),
            &mut len,
            std::ptr::null_mut(),
        )
    } != 0;
    // SAFETY: closing the handle opened above.
    unsafe { CloseHandle(handle) };
    if !ok || len < 12 {
        return None;
    }
    let tag = u32::from_le_bytes(buf[0..4].try_into().ok()?);
    if tag != IO_REPARSE_TAG_APPEXECLINK {
        return None;
    }
    // REPARSE_DATA_BUFFER header (8 bytes), then a version (4 bytes) and NUL-separated
    // UTF-16 strings: package id, app user model id, target path, ...
    let data = &buf[12..len as usize];
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let strings: Vec<String> = units.split(|&u| u == 0).map(String::from_utf16_lossy).collect();
    strings.get(2).cloned()
}

/// Resolves a path to a potential executable.
///
/// This exists because Windows needs `PATHEXT` resolution to determine if a path resolves to an executable or not.
/// For example, if the cwd is `C:/` and there's an executable at `C:/tools/my_tool.cmd`, we expect `tools/my_tool`
/// to resolve to the executable `my_tool.cmd` (assuming `.CMD` is part of `PATHEXT`).
pub(crate) fn resolve_executable_path(path: PathBuf, working_dir: &Path) -> PathBuf {
    // sill: `/c/Program Files/x.exe` (the MSYS drive form) names `C:/Program Files/x.exe`.
    let path = map_shell_path(&path).unwrap_or(path);
    resolve_executable(working_dir.join(&path)).unwrap_or(path)
}

/// sill: for a command written as a path in a POSIX system directory (`/usr/bin/ls`,
/// `/bin/sh`, as in Git Bash habits and shebang lines), returns the bare command name to
/// run instead when no such file exists. Windows has no `/usr/bin`; the command itself
/// (a builtin, a bundled tool, or one on `PATH`) is what was meant.
pub fn posix_system_command_name(name: &str) -> Option<&str> {
    const DIRS: &[&str] = &["/usr/local/bin/", "/usr/bin/", "/bin/", "/usr/local/sbin/", "/usr/sbin/", "/sbin/"];
    let rest = DIRS.iter().find_map(|dir| name.strip_prefix(dir))?;
    if rest.is_empty() || rest.contains(['/', '\\']) || Path::new(name).exists() {
        return None;
    }
    Some(rest)
}

impl crate::sys::fs::PathExt for Path {
    fn readable(&self) -> bool {
        self.exists()
    }

    fn writable(&self) -> bool {
        self.metadata().is_ok_and(|m| !m.permissions().readonly())
    }

    fn executable(&self) -> bool {
        if is_executable_file(self) {
            return true;
        }
        // Try each PATHEXT extension without allocating a separate PathBuf
        // per candidate until one exists.
        PATHEXT_EXTENSIONS.iter().any(|ext| {
            let mut name = self.as_os_str().to_owned();
            name.push(ext);
            Self::new(&name).is_file()
        }) || (self.is_file() && crate::sys::shebang::file_has_shebang(self))
    }

    fn exists_and_is_block_device(&self) -> bool {
        false
    }

    fn exists_and_is_char_device(&self) -> bool {
        // sill: the null device, which the shell opens as NUL (`try_open_special_file`).
        self.ends_with("dev/null") && self.is_absolute()
    }

    fn exists_and_is_fifo(&self) -> bool {
        false
    }

    fn exists_and_is_socket(&self) -> bool {
        false
    }

    fn exists_and_is_setgid(&self) -> bool {
        false
    }

    fn exists_and_is_setuid(&self) -> bool {
        false
    }

    fn exists_and_is_sticky_bit(&self) -> bool {
        false
    }

    fn get_device_and_inode(&self) -> Result<(u64, u64), crate::error::Error> {
        // TODO(windows): implement using file index / volume serial number.
        Err(error::ErrorKind::NotSupportedOnThisPlatform("get_device_and_inode").into())
    }
}

/// Splits a platform-specific PATH-like value into individual paths.
///
/// On Windows, this delegates to [`std::env::split_paths`].
///
/// sill: inside the shell `PATH` is POSIX-style (`/c/x:/c/y`), but values can also
/// arrive in Windows form (`C:\x;C:\y`) or as a mix (`C:\x;C:\y:/c/z`, after a
/// script appends to an imported value). All of these are accepted; see
/// [`split_path_list`].
pub fn split_paths<T: AsRef<OsStr> + ?Sized>(s: &T) -> std::vec::IntoIter<PathBuf> {
    split_path_list(&s.as_ref().to_string_lossy())
        .into_iter()
        .map(|entry| map_shell_path(Path::new(entry)).unwrap_or_else(|| PathBuf::from(entry)))
        .collect::<Vec<_>>()
        .into_iter()
}

/// sill: splits a `PATH`-like list in POSIX form, Windows form, or a mix. `;` always
/// separates; `:` separates too, except as a drive colon: the second character of an
/// entry whose first is a letter, followed by `\`, `/` or the end of the entry
/// (`C:\x`, `C:/x`, `C:`). Empty entries are dropped: in a POSIX list they would
/// mean the current directory, which Windows `PATH` values never intend.
pub fn split_path_list(s: &str) -> Vec<&str> {
    let mut entries = vec![];
    for chunk in s.split(';') {
        let bytes = chunk.as_bytes();
        let mut start = 0;
        for (i, b) in bytes.iter().enumerate() {
            if *b != b':' {
                continue;
            }
            let is_drive_colon = i == start + 1
                && bytes[start].is_ascii_alphabetic()
                && matches!(bytes.get(i + 1), None | Some(b'\\' | b'/' | b';'));
            if !is_drive_colon {
                entries.push(&chunk[start..i]);
                start = i + 1;
            }
        }
        entries.push(&chunk[start..]);
    }
    entries.retain(|e| !e.is_empty());
    entries
}

/// sill: renders a `PATH`-like list in the POSIX form the shell shows (`/c/x:/c/y`).
/// Harnesses depend on it: Claude Code probes `bash -lc 'echo "$PATH"'` and rejects
/// any value containing `;`, then appends `:/c/...` entries.
pub fn posix_path_list(s: &str) -> String {
    split_path_list(s)
        .into_iter()
        .map(posix_path_entry)
        .collect::<Vec<_>>()
        .join(":")
}

/// sill: renders a `PATH`-like list in the form the shell shows: POSIX-style here.
pub fn shell_path_list(s: &str) -> String {
    posix_path_list(s)
}

/// sill: renders a `PATH`-like list in Windows form (`C:\x;C:\y`), for child processes.
pub fn windows_path_list(s: &str) -> String {
    split_path_list(s)
        .into_iter()
        .map(|entry| {
            let path = map_shell_path(Path::new(entry)).unwrap_or_else(|| PathBuf::from(entry));
            path.to_string_lossy().replace('/', "\\")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// `C:\x\y` → `/c/x/y`, `C:` → `/c`, `\\srv\share` → `//srv/share`; anything else
/// only has its separators turned into `/`.
fn posix_path_entry(entry: &str) -> String {
    let bytes = entry.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = entry[2..].replace('\\', "/");
        let rest = rest.trim_end_matches('/');
        return if rest.is_empty() { format!("/{drive}") } else { format!("/{drive}{rest}") };
    }
    entry.replace('\\', "/")
}

/// Opens a null file that will discard all I/O.
pub fn open_null_file() -> Result<std::fs::File, error::Error> {
    let f = std::fs::File::options()
        .read(true)
        .write(true)
        .open("NUL")?;
    Ok(f)
}

/// Gives the platform an opportunity to handle a special file path (e.g. `/dev/null`).
pub fn try_open_special_file(path: &Path) -> Option<Result<std::fs::File, std::io::Error>> {
    if path.ends_with("dev/null") && path.is_absolute() {
        Some(open_null_file().map_err(std::io::Error::other))
    } else {
        None
    }
}

/// sill: maps the POSIX-style paths agents and harnesses write to Windows paths:
/// `/c` and `/c/...` (the MSYS drive form Claude Code emits) become `C:/...`.
///
/// `/tmp` is deliberately not mapped: Windows itself resolves `/tmp/x` to `\tmp\x` on
/// the current drive, and so does every program it's passed to (and Claude Code's own
/// file tools). Mapping it to `%TEMP%` as Git Bash does made the shell's `> /tmp/x`
/// and `diff /tmp/x` name different files. sill creates `\tmp` at startup.
///
/// Only a whole leading component matches (`/cache` is untouched), and
/// the `//server/share` UNC form is left alone. Returns `None` for anything else,
/// which is then used as written. This applies to paths the shell itself opens or
/// enters, never to arguments passed to other programs.
pub fn map_shell_path(path: &Path) -> Option<PathBuf> {
    let rest = path.to_str()?.strip_prefix('/')?;
    if rest.starts_with(['/', '\\']) {
        return None;
    }
    let (first, tail) = match rest.find(['/', '\\']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };

    if let [letter] = first.as_bytes()
        && letter.is_ascii_alphabetic()
    {
        let tail = if tail.is_empty() { "/" } else { tail };
        return Some(PathBuf::from(format!("{}:{tail}", letter.to_ascii_uppercase() as char)));
    }

    None
}

/// sill: renders a path the way the shell shows it: no verbatim prefix (`\\?\`, which
/// `canonicalize` adds) and `/` separators, since `\` is the shell's escape character.
/// `\\?\C:\x` becomes `C:/x`; `\\?\UNC\srv\share` becomes `//srv/share`.
pub fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        std::borrow::Cow::Owned(format!(r"\\{unc}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        std::borrow::Cow::Owned(rest.to_owned())
    } else {
        s
    };
    normalize_path_separators(&s).into_owned()
}

/// Returns the default paths where executables are typically found on Windows.
pub(crate) fn get_default_executable_search_paths() -> Vec<PathBuf> {
    default_system_paths()
}

/// Returns the default paths where standard system utilities are found on Windows.
pub fn get_default_standard_utils_paths() -> Vec<PathBuf> {
    default_system_paths()
}

fn default_system_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(sysroot) = std::env::var("SystemRoot") {
        paths.push(PathBuf::from(&sysroot).join("system32"));
        paths.push(PathBuf::from(&sysroot));
        paths.push(PathBuf::from(&sysroot).join("System32").join("Wbem"));
        paths.push(
            PathBuf::from(&sysroot)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0"),
        );
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        paths.push(
            PathBuf::from(userprofile)
                .join("AppData")
                .join("Local")
                .join("Microsoft")
                .join("WindowsApps"),
        );
    }
    paths
}

/// Returns the path to the system-wide shell profile script.
///
/// On Windows, no system profile is loaded by default.
pub const fn get_system_profile_path() -> Option<&'static Path> {
    None
}

/// Returns the path to the system-wide shell rc script.
///
/// On Windows, no system rc file is loaded by default.
pub const fn get_system_rc_path() -> Option<&'static Path> {
    None
}

/// Returns the platform default for case-insensitive pathname expansion.
///
/// On Windows, filesystems are typically case-insensitive, so this returns `true`.
pub const fn default_case_insensitive_path_expansion() -> bool {
    true
}

/// Path separator characters on Windows.
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];

/// Returns true if the string contains a path separator character.
///
/// On Windows, both `/` and `\` are considered path separators.
pub fn contains_path_separator(s: &str) -> bool {
    s.contains(PATH_SEPARATORS)
}

/// Returns true if the string ends with a path separator character.
///
/// On Windows, both `/` and `\` are considered path separators.
pub fn ends_with_path_separator(s: &str) -> bool {
    s.ends_with(PATH_SEPARATORS)
}

/// Returns the string with a trailing path separator removed, if present.
///
/// On Windows, both `/` and `\` are considered path separators.
pub fn strip_path_separator_suffix(s: &str) -> &str {
    s.strip_suffix(PATH_SEPARATORS).unwrap_or(s)
}

/// Finds the byte index of the last path separator in the string.
///
/// On Windows, both `/` and `\` are considered path separators.
pub fn rfind_path_separator(s: &str) -> Option<usize> {
    s.rfind(PATH_SEPARATORS)
}

/// Splits a string on path separator characters, returning an iterator of components.
///
/// On Windows, both `/` and `\` are used as separators.
pub fn split_path_for_pattern(s: &str) -> impl Iterator<Item = &str> {
    s.split(PATH_SEPARATORS)
}

/// Returns the root path for an absolute pattern, if the first component indicates one.
///
/// On Windows, recognizes both a leading separator (empty first component from splitting
/// a path like `/foo`) and a drive-letter prefix like `C:` as absolute.
///
/// TODO(windows): UNC paths like `\\server\share\foo` are not yet handled
/// specially; they split into `["", "", "server", "share", "foo"]`, and the
/// leading empty component causes them to be treated as if they were rooted
/// at `/`, which drops the server/share portion. Supporting UNC requires
/// peeking further into the component list.
pub fn pattern_path_root(first_component: &str) -> Option<PathBuf> {
    if first_component.is_empty() {
        // Leading separator, e.g. `/foo` split into ["", "foo"].
        Some(PathBuf::from("/"))
    } else if first_component.len() == 2
        && first_component.as_bytes()[0].is_ascii_alphabetic()
        && first_component.as_bytes()[1] == b':'
    {
        // Drive letter prefix, e.g. `c:/foo` split into ["c:", "foo"].
        let mut root = String::with_capacity(3);
        root.push_str(first_component);
        root.push('/');
        Some(PathBuf::from(root))
    } else {
        None
    }
}

/// Pushes a component onto a path for pattern expansion.
///
/// On Windows, `PathBuf::push` has special drive-letter and root-replacement
/// semantics that conflict with shell path construction (e.g. pushing `C:foo`
/// onto `D:\bar` replaces the whole path). This function always appends the
/// component as a child, operating on the underlying `OsString` so non-UTF-8
/// content in the path is preserved and no reallocation is needed.
pub fn push_path_for_pattern(path: &mut PathBuf, component: &str) {
    // Separator characters are ASCII, and WTF-8-encoded OsStr bytes are a
    // superset of UTF-8, so checking the last byte directly is safe.
    let bytes = path.as_os_str().as_encoded_bytes();
    let needs_sep = !bytes.is_empty() && !matches!(bytes.last(), Some(b'/' | b'\\'));

    let buf = path.as_mut_os_string();
    if needs_sep {
        buf.push("/");
    }
    buf.push(component);
}

/// Normalizes path separators for shell output.
///
/// On Windows, replaces `\` with `/` since backslash is the shell escape character.
pub fn normalize_path_separators(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('\\') {
        std::borrow::Cow::Owned(s.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_path_list_accepts_both_forms_and_mixtures() {
        assert_eq!(split_path_list(r"C:\a;D:\b c;"), vec![r"C:\a", r"D:\b c"]);
        assert_eq!(split_path_list("/c/a:/d/b c"), vec!["/c/a", "/d/b c"]);
        assert_eq!(split_path_list(r"C:\a;C:\b:/c/x"), vec![r"C:\a", r"C:\b", "/c/x"]);
        assert_eq!(split_path_list("/c/x:C:/y:C:"), vec!["/c/x", "C:/y", "C:"]);
        assert_eq!(split_path_list("/usr/bin::/a:"), vec!["/usr/bin", "/a"]);
        assert_eq!(split_path_list(""), Vec::<&str>::new());
    }

    #[test]
    fn path_list_round_trips_between_forms() {
        let win = r"C:\Windows\System32;C:\Program Files\Git\cmd;\\srv\tools";
        let posix = posix_path_list(win);
        assert_eq!(posix, "/c/Windows/System32:/c/Program Files/Git/cmd://srv/tools");
        assert_eq!(windows_path_list(&posix), win);
        // A POSIX entry appended to a Windows value (Claude Code's snapshot does this).
        assert_eq!(
            windows_path_list(&format!("{win}:/d/plugins")),
            format!(r"{win};D:\plugins")
        );
        assert_eq!(posix_path_list(r"C:\;C:"), "/c:/c");
    }

    #[test]
    fn split_paths_maps_posix_drive_entries() {
        let got: Vec<PathBuf> = split_paths("/c/Windows:D:\\x").collect();
        assert_eq!(got, vec![PathBuf::from("C:/Windows"), PathBuf::from(r"D:\x")]);
    }

    #[test]
    fn posix_system_command_names() {
        assert_eq!(posix_system_command_name("/usr/bin/ls"), Some("ls"));
        assert_eq!(posix_system_command_name("/bin/sh"), Some("sh"));
        assert_eq!(posix_system_command_name("/usr/local/bin/node"), Some("node"));
        assert_eq!(posix_system_command_name("/usr/bin/"), None);
        assert_eq!(posix_system_command_name("/usr/bin/x/y"), None);
        assert_eq!(posix_system_command_name("/opt/bin/ls"), None);
        assert_eq!(posix_system_command_name("ls"), None);
    }

    #[test]
    fn display_path_strips_verbatim_and_uses_slashes() {
        assert_eq!(display_path(Path::new(r"C:\Users\x")), "C:/Users/x");
        assert_eq!(display_path(Path::new(r"\\?\C:\Users\x")), "C:/Users/x");
        assert_eq!(display_path(Path::new(r"\\?\UNC\srv\share\d")), "//srv/share/d");
        assert_eq!(display_path(Path::new(r"\\srv\share")), "//srv/share");
        assert_eq!(display_path(Path::new("C:/")), "C:/");
    }

    #[test]
    fn map_shell_path_msys_drives() {
        assert_eq!(map_shell_path(Path::new("/c")), Some(PathBuf::from("C:/")));
        assert_eq!(map_shell_path(Path::new("/c/")), Some(PathBuf::from("C:/")));
        assert_eq!(
            map_shell_path(Path::new("/d/work/repo")),
            Some(PathBuf::from("D:/work/repo"))
        );
    }

    #[test]
    fn map_shell_path_leaves_others_alone() {
        for p in ["/cache", "/tmp", "/tmp/x", "/dev/null", "//server/share", "C:/x", "rel/c", "/usr/bin", ""] {
            assert_eq!(map_shell_path(Path::new(p)), None, "{p}");
        }
    }

    #[test]
    fn path_separator_helpers_both_slashes() {
        assert!(contains_path_separator("foo/bar"));
        assert!(contains_path_separator(r"foo\bar"));
        assert!(contains_path_separator(r"mixed/and\back"));
        assert!(!contains_path_separator("foobar"));

        assert!(ends_with_path_separator("foo/"));
        assert!(ends_with_path_separator(r"foo\"));
        assert!(!ends_with_path_separator("foo"));

        assert_eq!(strip_path_separator_suffix("foo/"), "foo");
        assert_eq!(strip_path_separator_suffix(r"foo\"), "foo");
        assert_eq!(strip_path_separator_suffix("foo"), "foo");

        assert_eq!(rfind_path_separator("a/b/c"), Some(3));
        assert_eq!(rfind_path_separator(r"a\b\c"), Some(3));
        assert_eq!(rfind_path_separator(r"a/b\c"), Some(3));
        assert_eq!(rfind_path_separator("abc"), None);
    }

    #[test]
    fn split_path_for_pattern_both_slashes() {
        let parts: Vec<_> = split_path_for_pattern("a/b/c").collect();
        assert_eq!(parts, vec!["a", "b", "c"]);

        let parts: Vec<_> = split_path_for_pattern(r"a\b\c").collect();
        assert_eq!(parts, vec!["a", "b", "c"]);

        let parts: Vec<_> = split_path_for_pattern(r"a/b\c").collect();
        assert_eq!(parts, vec!["a", "b", "c"]);

        let parts: Vec<_> = split_path_for_pattern("/a/b").collect();
        assert_eq!(parts, vec!["", "a", "b"]);
    }

    #[test]
    fn pattern_path_root_leading_separator() {
        assert_eq!(pattern_path_root(""), Some(PathBuf::from("/")));
    }

    #[test]
    fn pattern_path_root_drive_letters() {
        assert_eq!(pattern_path_root("c:"), Some(PathBuf::from("c:/")));
        assert_eq!(pattern_path_root("C:"), Some(PathBuf::from("C:/")));
        assert_eq!(pattern_path_root("Z:"), Some(PathBuf::from("Z:/")));
    }

    #[test]
    fn pattern_path_root_rejects_non_drive_two_char_prefix() {
        // "1:" is not a valid drive letter — must be alphabetic.
        assert_eq!(pattern_path_root("1:"), None);
        // Longer drive-like strings are not treated as roots.
        assert_eq!(pattern_path_root("cd"), None);
        assert_eq!(pattern_path_root("c:\\"), None);
        assert_eq!(pattern_path_root("foo"), None);
    }

    #[test]
    fn push_path_for_pattern_appends_with_forward_slash() {
        let mut p = PathBuf::from(r"C:\Users\reuben");
        push_path_for_pattern(&mut p, "foo");
        // Forward slash is used as the appended separator, yielding mixed
        // separators — acceptable because `normalize_path_separators` is
        // applied downstream before display.
        assert_eq!(p, PathBuf::from(r"C:\Users\reuben/foo"));
    }

    #[test]
    fn push_path_for_pattern_no_double_separator() {
        let mut p = PathBuf::from("C:/Users/reuben/");
        push_path_for_pattern(&mut p, "foo");
        assert_eq!(p, PathBuf::from("C:/Users/reuben/foo"));

        let mut p = PathBuf::from(r"C:\Users\reuben\");
        push_path_for_pattern(&mut p, "foo");
        assert_eq!(p, PathBuf::from(r"C:\Users\reuben\foo"));
    }

    #[test]
    fn push_path_for_pattern_onto_drive_root() {
        let mut p = PathBuf::from("c:/");
        push_path_for_pattern(&mut p, "foo");
        assert_eq!(p, PathBuf::from("c:/foo"));
    }

    #[test]
    fn push_path_for_pattern_onto_empty() {
        let mut p = PathBuf::new();
        push_path_for_pattern(&mut p, "foo");
        // Empty path stays un-prefixed — we only add a separator between
        // existing content and the new component.
        assert_eq!(p, PathBuf::from("foo"));
    }

    #[test]
    fn normalize_path_separators_converts_backslashes() {
        use std::borrow::Cow;
        // Already-forward-slashed input is borrowed (no allocation).
        assert!(matches!(
            normalize_path_separators("c:/foo/bar"),
            Cow::Borrowed("c:/foo/bar")
        ));
        // Mixed or backslashed input becomes owned and fully forward-slashed.
        let normalized = normalize_path_separators(r"c:\foo\bar");
        assert_eq!(normalized.as_ref(), "c:/foo/bar");
        let normalized = normalize_path_separators(r"c:\foo/bar");
        assert_eq!(normalized.as_ref(), "c:/foo/bar");
    }

    #[test]
    fn default_case_insensitive_is_true() {
        assert!(default_case_insensitive_path_expansion());
    }

    #[test]
    fn has_executable_extension_is_case_insensitive() {
        // Force the PATHEXT cache for this test's defaults.
        assert!(has_executable_extension(Path::new("foo.exe")));
        assert!(has_executable_extension(Path::new("foo.EXE")));
        assert!(has_executable_extension(Path::new("foo.Cmd")));
        assert!(!has_executable_extension(Path::new("foo.txt")));
        assert!(!has_executable_extension(Path::new("foo")));
    }

    #[test]
    fn pathext_entry_stem_strips_dot() {
        assert_eq!(pathext_entry_stem(".exe"), "exe");
        assert_eq!(pathext_entry_stem(".cmd"), "cmd");
        // Tolerant: entries without a leading dot are returned as-is.
        assert_eq!(pathext_entry_stem("exe"), "exe");
        assert_eq!(pathext_entry_stem(""), "");
    }

    #[test]
    fn resolve_executable_for_nonexistent_returns_none() {
        // A path that cannot exist on any test host.
        let path = PathBuf::from(r"C:\__brush_test_definitely_missing__");
        assert!(resolve_executable(path).is_none());
    }
}
