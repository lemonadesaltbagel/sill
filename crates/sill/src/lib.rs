//! `sill`: the brush-based Bash interpreter with sill's Windows adapter.

/// Runs the shell with the process's command line. Never returns normally.
pub fn run() {
    #[cfg(windows)]
    if std::env::args_os().nth(1).is_some_and(|a| a == console::HELPER_ARG) {
        console::run_helper();
    }
    if std::env::args_os().nth(1).is_some_and(|a| a == mcp::ENV_DUMP_ARG)
        && let Some(file) = std::env::args_os().nth(2)
    {
        mcp::dump_env(&file);
    }
    trace_invocation();
    if std::env::args_os().skip(1).eq(["--version"]) {
        println!(
            "sill {} (brush {}, {})",
            env!("CARGO_PKG_VERSION"),
            brush_shell::productinfo::PRODUCT_VERSION,
            brush_shell::productinfo::PRODUCT_GIT_VERSION
        );
        std::process::exit(0);
    }
    #[cfg(windows)]
    {
        console::own_std_handles();
        console::ensure_invisible();
        tools::put_tools_dir_on_path();
        tools::ensure_drive_tmp();
        env_defaults::apply();
        let args: Vec<String> =
            std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        if let Some(note) = script_hint::hint(&args, |name| std::env::var(name).ok()) {
            eprintln!("{note}");
        }
    }
    if std::env::args_os().skip(1).eq([mcp::ARG]) {
        mcp::serve();
    }

    brush_shell::entry::run();
}

#[cfg_attr(not(windows), allow(dead_code, reason = "used on Windows; tested everywhere"))]
mod script_hint;

mod mcp;

/// `SILL_TRACE=<file>` appends each invocation's arguments and working directory to the
/// file, one JSON line each: what a harness actually ran, after its own rewriting.
fn trace_invocation() {
    let Some(path) = std::env::var_os("SILL_TRACE").filter(|p| !p.is_empty()) else { return };
    let args: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let quote = |s: &str| {
        let mut out = String::from('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    };
    let line = format!(
        "{{\"pid\":{},\"cwd\":{},\"argv\":[{}]}}\n",
        std::process::id(),
        quote(&cwd),
        args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(",")
    );
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Started without a console (Claude Code starts the shell detached), sill would give every
/// console program it runs a new console of its own, each with a window: one flashing up
/// per command on the user's screen, and no console state shared between them (a `chcp`
/// didn't carry from one program to the next). Git Bash (Cygwin) creates an
/// invisible console for its programs in that case, and so does sill. Windows 11 24H2 and
/// later have `AllocConsoleWithOptions` for it; on older versions sill attaches to the
/// windowless console of a short-lived helper process (`attach_helper_console`). If both
/// fail, the launcher starts each program with `CREATE_NO_WINDOW` (brush-core
/// `compose_std_command`).
///
/// That console's code pages are UTF-8 (65001). The harness reads output as UTF-8, and
/// console programs (cmd, PowerShell 5.1, MSVC, ipconfig, ...) write in the console's code
/// page: with the OEM default, their text in any non-ASCII language reached the model as
/// mojibake, and characters outside the code page were lost as `?`. Under 65001 Windows'
/// own messages also come out in English. Only a console sill created is changed, never
/// one it was given. Batch files are then read as UTF-8 too: ones saved in the old local
/// code page with non-ASCII text misread, so `SILL_CONSOLE_CP=<code page>` (e.g. 936)
/// sets another one.
#[cfg(windows)]
mod console {
    use windows_sys::Win32::Foundation::{HANDLE, S_OK};
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetConsoleCP, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCP, SetConsoleOutputCP, SetStdHandle,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    /// The argument that makes sill a console helper (see `attach_helper_console`).
    pub(crate) const HELPER_ARG: &str = "--sill-console-helper";

    /// `ALLOC_CONSOLE_OPTIONS`, declared here: the function is looked up at run time, so
    /// sill still starts on Windows versions without it.
    #[repr(C)]
    struct AllocConsoleOptions {
        mode: i32,
        use_show_window: i32,
        show_window: u16,
    }
    const ALLOC_CONSOLE_MODE_NO_WINDOW: i32 = 2;
    type AllocConsoleWithOptions = unsafe extern "system" fn(*const AllocConsoleOptions, *mut i32) -> i32;

    /// Makes sill's own standard handles non-inheritable. Windows gives a child every
    /// inheritable handle of its parent, so a background job (`server > log 2>&1 &`) kept a
    /// copy of the harness's stdout pipe and the harness never saw it close: opencode (and
    /// any `communicate()`-style runner) waited until its timeout. Children still get their
    /// standard streams: the launcher passes them as duplicates made for each child.
    pub(crate) fn own_std_handles() {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: plain Win32 calls; a failure (no handle, a console) is harmless.
            unsafe {
                let h = GetStdHandle(id);
                if !h.is_null() && h != INVALID_HANDLE_VALUE {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }

    /// Creates an invisible console if sill has none. Returns whether it did.
    pub(crate) fn ensure_invisible() -> bool {
        // SAFETY: plain Win32 call; GetConsoleCP returns 0 when there's no console.
        if unsafe { GetConsoleCP() } != 0 {
            return false;
        }
        // A new or attached console becomes the standard handles that weren't redirected;
        // sill's own (the harness's pipes) must stay as they are.
        let ids: [STD_HANDLE; 3] = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        // SAFETY: plain Win32 calls.
        let saved: Vec<HANDLE> = ids.iter().map(|&id| unsafe { GetStdHandle(id) }).collect();
        let forced_fallback = std::env::var_os("SILL_CONSOLE_FALLBACK").is_some_and(|v| v == "1");
        let ok = match alloc_console_with_options() {
            Some(alloc) if !forced_fallback => alloc(),
            _ => attach_helper_console(),
        };
        for (&id, &handle) in ids.iter().zip(&saved) {
            // SAFETY: restoring the handles read above.
            unsafe { SetStdHandle(id, handle) };
        }
        if ok {
            let cp = std::env::var("SILL_CONSOLE_CP")
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(65001);
            // SAFETY: plain Win32 calls on the console just created.
            unsafe {
                SetConsoleOutputCP(cp);
                SetConsoleCP(cp);
            }
        }
        ok
    }

    /// Windows 11 24H2 and later: `AllocConsoleWithOptions(NO_WINDOW)`.
    fn alloc_console_with_options() -> Option<impl Fn() -> bool> {
        // SAFETY: plain Win32 calls; the function pointer is checked for presence.
        unsafe {
            let kernel32: Vec<u16> = "kernel32.dll".encode_utf16().chain(Some(0)).collect();
            let module = GetModuleHandleW(kernel32.as_ptr());
            if module.is_null() {
                return None;
            }
            let proc = GetProcAddress(module, c"AllocConsoleWithOptions".as_ptr().cast())?;
            // SAFETY: the documented signature of AllocConsoleWithOptions.
            let alloc: AllocConsoleWithOptions = std::mem::transmute(proc);
            Some(move || {
                let options = AllocConsoleOptions { mode: ALLOC_CONSOLE_MODE_NO_WINDOW, use_show_window: 0, show_window: 0 };
                let mut result = 0;
                alloc(&options, &mut result) == S_OK
            })
        }
    }

    /// Earlier Windows (Cygwin's workaround, in short): start a helper (sill itself) with
    /// `CREATE_NO_WINDOW`, which gives it a console with no window, attach to that console,
    /// and let the helper go. The console lives on while sill is attached.
    fn attach_helper_console() -> bool {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let Ok(exe) = std::env::current_exe() else { return false };
        let Ok(mut helper) = std::process::Command::new(exe)
            .arg(HELPER_ARG)
            .env_remove("SILL_TRACE")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return false;
        };
        // The console exists from process creation; a retry covers a slow start.
        let mut attached = false;
        for _ in 0..50 {
            // SAFETY: plain Win32 call with the helper's process id.
            if unsafe { AttachConsole(helper.id()) } != 0 {
                attached = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Closing its stdin ends the helper.
        drop(helper.stdin.take());
        let _ = helper.wait();
        attached
    }

    /// The helper: waits for its stdin to close, then exits.
    pub(crate) fn run_helper() -> ! {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        std::process::exit(0)
    }
}

/// Bundled native tools that aren't coreutils (grep, sed, awk, find, xargs, diff, ...)
/// live in `tools\` beside the executable. The coreutils are built in.
#[cfg(windows)]
mod tools {
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// Puts `<exe dir>\tools` first on `PATH`, so the bundled tools are the ones every
    /// command finds: deterministic GNU-style behaviour whatever else is installed
    /// (e.g. Git's MSYS tools, which re-parse their command lines). It's visible in
    /// `$PATH`, and `SILL_NO_TOOLS=1` turns it off.
    pub(crate) fn put_tools_dir_on_path() {
        let mut dirs = vec![];
        // sill's own directory first, so `bash script.sh` and `#!/bin/bash` run sill, not
        // whatever other bash.exe is on PATH (Git's, or WSL's launcher).
        if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
            dirs.push(exe_dir);
        }
        if !std::env::var_os("SILL_NO_TOOLS").is_some_and(|v| !v.is_empty() && v != "0")
            && let Some(dir) = tools_dir()
        {
            // SAFETY: called first thing in `main`, before the shell starts any threads.
            // The launcher treats programs in this directory as sill's own (see brush-core
            // `push_args`); it's also visible to scripts.
            unsafe { std::env::set_var("SILL_TOOLS_DIR", &dir) };
            dirs.push(dir);
        }

        let current = std::env::var_os("PATH").unwrap_or_default();
        let present: Vec<PathBuf> = std::env::split_paths(&current).collect();
        let mut path = OsString::new();
        for dir in dirs {
            if present.iter().any(|p| p.as_os_str().eq_ignore_ascii_case(dir.as_os_str())) {
                continue;
            }
            path.push(dir.as_os_str());
            path.push(";");
        }
        if path.is_empty() {
            return;
        }
        path.push(&current);
        // SAFETY: called first thing in `main`, before the shell starts any threads.
        unsafe { std::env::set_var("PATH", path) };
    }

    /// `/tmp` in sill is what Windows makes of it: `\tmp` on the current drive, the same
    /// directory every program given a `/tmp/...` path opens (and Claude Code's file tools
    /// use). Create it if it's missing so `> /tmp/x` works; failures are ignored.
    pub(crate) fn ensure_drive_tmp() {
        let Ok(cwd) = std::env::current_dir() else { return };
        let Some(std::path::Component::Prefix(prefix)) = cwd.components().next() else { return };
        let tmp = PathBuf::from(prefix.as_os_str()).join("\\tmp");
        if !tmp.is_dir() {
            let _ = std::fs::create_dir(&tmp);
        }
    }

    fn tools_dir() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.join("tools");
        dir.is_dir().then_some(dir)
    }
}

/// Agent-friendly defaults for the tools agents run (vetted against each
/// tool's docs): UTF-8 Python output through pipes, English and quiet .NET/MSBuild, no
/// prompts or pagers from git/npm/pip, telemetry off. Set only where the user hasn't set
/// the variable; `SILL_NO_ENV_DEFAULTS=1` turns this off. All visible with `env`.
#[cfg(windows)]
mod env_defaults {
    const DEFAULTS: &[(&str, &str)] = &[
        // Python on Windows writes cp1252 to a pipe and fails on non-ASCII text.
        ("PYTHONUTF8", "1"),
        ("PYTHONIOENCODING", "utf-8"),
        ("PYTHONUNBUFFERED", "1"),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
        ("PIP_NO_INPUT", "1"),
        ("DOTNET_CLI_TELEMETRY_OPTOUT", "1"),
        ("DOTNET_NOLOGO", "1"),
        ("DOTNET_CLI_UI_LANGUAGE", "en-US"),
        ("VSLANG", "1033"),
        // Lingering MSBuild nodes hold file locks.
        ("MSBUILDDISABLENODEREUSE", "1"),
        ("MSBUILDTERMINALLOGGER", "off"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GCM_INTERACTIVE", "never"),
        ("GIT_PAGER", "cat"),
        ("PAGER", "cat"),
        ("NO_UPDATE_NOTIFIER", "1"),
        ("npm_config_update_notifier", "false"),
        ("npm_config_fund", "false"),
        ("POWERSHELL_TELEMETRY_OPTOUT", "1"),
        ("POWERSHELL_UPDATECHECK", "Off"),
    ];

    /// Forced color in captured output is noise; Python 3.13+ even reads FORCE_COLOR=0 as
    /// "on". (`CI` is deliberately never touched: it changes pnpm/yarn installs.)
    const REMOVE: &[&str] = &["FORCE_COLOR", "CLICOLOR_FORCE"];

    /// git prints non-ASCII paths as octal escapes (`"\344\270\255.txt"`) unless
    /// `core.quotepath` is off. Set through `GIT_CONFIG_COUNT` (which outranks config files),
    /// so only when the user passes no config that way and their global config doesn't
    /// mention quotepath.
    fn git_quotepath_off() {
        let set = |name: &str| std::env::var_os(name).is_some();
        if set("GIT_CONFIG_COUNT") || set("GIT_CONFIG_PARAMETERS") {
            return;
        }
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        let globals = [
            std::env::var_os("GIT_CONFIG_GLOBAL").map(std::path::PathBuf::from),
            home.as_ref().map(|h| std::path::Path::new(h).join(".gitconfig")),
            std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| home.as_ref().map(|h| std::path::Path::new(h).join(".config")))
                .map(|d| d.join("git").join("config")),
        ];
        let mentioned = globals.iter().flatten().any(|f| {
            std::fs::read_to_string(f).is_ok_and(|t| t.to_ascii_lowercase().contains("quotepath"))
        });
        if mentioned {
            return;
        }
        for (name, value) in [("GIT_CONFIG_COUNT", "1"), ("GIT_CONFIG_KEY_0", "core.quotepath"), ("GIT_CONFIG_VALUE_0", "false")] {
            // SAFETY: called first thing in `main`, before the shell starts any threads.
            unsafe { std::env::set_var(name, value) };
        }
    }

    /// Windows' messages (behind every "No such file" or "Access denied" that sill and its
    /// built-in tools print) come in the user's UI language, while the programs agents run
    /// (git, the GNU tools, Python, .NET with the defaults above) speak English, and a mix
    /// of the two in one transcript is harder to read. So sill's own process prefers
    /// English, unless LC_ALL, LC_MESSAGES or LANG names another language. (Not inherited:
    /// programs sill starts keep the user's language.)
    pub(crate) fn english_messages() {
        let wanted = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()));
        if let Some(locale) = wanted
            && !(locale == "C" || locale == "POSIX" || locale.starts_with("C.") || locale.starts_with("en"))
        {
            return;
        }
        use windows_sys::Win32::Globalization::{
            MUI_LANGUAGE_NAME, SetProcessPreferredUILanguages, SetThreadUILanguage,
        };
        let languages: Vec<u16> = "en-US\0\0".encode_utf16().collect();
        let mut count = 0u32;
        // SAFETY: a double-NUL-terminated list of one language name.
        unsafe { SetProcessPreferredUILanguages(MUI_LANGUAGE_NAME, languages.as_ptr(), &raw mut count) };
        // SAFETY: plain Win32 call; 0x0409 is en-US.
        unsafe { SetThreadUILanguage(0x0409) };
    }

    pub(crate) fn apply() {
        if std::env::var_os("SILL_NO_ENV_DEFAULTS").is_some_and(|v| !v.is_empty() && v != "0") {
            return;
        }
        english_messages();
        for (name, value) in DEFAULTS {
            if std::env::var_os(name).is_none() {
                // SAFETY: called first thing in `main`, before the shell starts any threads.
                unsafe { std::env::set_var(name, value) };
            }
        }
        for name in REMOVE {
            // SAFETY: as above.
            unsafe { std::env::remove_var(name) };
        }
        git_quotepath_off();
        // ripgrep's config shipped beside it (scripts/package-windows.ps1): `/` separators.
        if std::env::var_os("RIPGREP_CONFIG_PATH").is_none()
            && let Some(dir) = std::env::var_os("SILL_TOOLS_DIR")
        {
            let rc = std::path::Path::new(&dir).join("ripgreprc");
            if rc.is_file() {
                // SAFETY: called first thing in `main`, before the shell starts any threads.
                unsafe { std::env::set_var("RIPGREP_CONFIG_PATH", rc) };
            }
        }
    }
}
