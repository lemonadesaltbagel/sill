#[derive(Debug, Default, Clone)]
pub(crate) struct Formatter {
    pub use_color: bool,
}

impl brush_core::extensions::ErrorFormatter for Formatter {
    fn format_error(
        &self,
        err: &brush_core::error::Error,
        shell: &brush_core::Shell<impl brush_core::ShellExtensions>,
    ) -> String {
        // sill: bash's shape, `<name>: line <n>: <message>` (e.g. `build.sh: line 3: foo:
        // command not found`), which is what models expect and parse, rather than
        // `error: <message>`.
        let prefix = shell.diagnostic_prefix();
        let prefix = if self.use_color {
            color_print::cformat!("<red>{}</red>", prefix.trim_end())
                + " "
        } else {
            prefix
        };

        let message = match err.kind() {
            // sill: as bash says it for a command given as a path (`./venv/bin/python`): the
            // path is what's wrong, not a missing program.
            brush_core::error::ErrorKind::CommandNotFound(cmd) if cmd.contains(['/', '\\']) => {
                format!("{}: No such file or directory", ansi_c_quoted(cmd))
            }
            brush_core::error::ErrorKind::CommandNotFound(cmd) => {
                let shown = ansi_c_quoted(cmd);
                let hint = if is_python(cmd) && shell.find_first_executable_in_path("python").is_none() {
                    // sill: no `python` either (the Store's stand-in doesn't count).
                    Some("Python isn't installed here; `winget install -e --id Python.Python.3.13` installs it")
                        .filter(|_| cfg!(windows))
                } else {
                    unsupported_hint(cmd)
                };
                match hint {
                    Some(hint) => format!("{shown}: command not found ({hint})"),
                    None => format!("{shown}: command not found"),
                }
            }
            _ => format!("{err:#}"),
        };

        std::format!("{prefix}{message}\n")
    }
}

/// sill: Python by any of the names agents use.
fn is_python(cmd: &str) -> bool {
    matches!(cmd, "python" | "python3" | "python3.exe" | "python.exe" | "pip" | "pip3" | "py")
}

/// sill: a name as bash shows it in a diagnostic: as is, or `$'...'` when it holds a
/// character that doesn't show, such as the CR of a script with Windows line endings
/// (`$'pwd\r': command not found`) or a byte order mark.
fn ansi_c_quoted(name: &str) -> String {
    let hidden = |c: char| c.is_control() || c == '\u{feff}';
    if !name.chars().any(hidden) {
        return name.to_owned();
    }
    let mut out = String::from("$'");
    for c in name.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\x1b' => out.push_str("\\E"),
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            c if hidden(c) => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("\\{b:03o}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod quote_tests {
    #[test]
    fn hidden_characters_show() {
        use super::ansi_c_quoted as q;
        assert_eq!(q("pwd"), "pwd");
        assert_eq!(q("pwd\r"), "$'pwd\\r'");
        assert_eq!(q("\u{feff}echo"), "$'\\357\\273\\277echo'");
        assert_eq!(q("中文"), "中文");
    }
}

/// sill: for Linux-only commands agents reach for, the Windows way to do it, so a
/// failure is actionable in one line instead of a dead end. The command still fails
/// (status 127); nothing is emulated.
fn unsupported_hint(cmd: &str) -> Option<&'static str> {
    if !cfg!(windows) {
        return None;
    }
    Some(match cmd {
        "apt" | "apt-get" | "yum" | "dnf" | "pacman" | "zypper" | "apk" | "snap" => {
            "Linux package manager; on Windows use winget, e.g. `winget install --id <Id> -e`"
        }
        "systemctl" | "service" | "journalctl" => {
            "no systemd on Windows; use `sc.exe query <name>` or pwsh `Get-Service`/`Start-Service`"
        }
        "sudo" | "su" | "doas" => {
            "no sudo on Windows; elevation needs a human to run the terminal as Administrator"
        }
        "chmod" | "chown" | "chgrp" => {
            "no POSIX permission bits on Windows; executability comes from the extension (.exe, .bat), and scripts run with `bash script.sh`"
        }
        "xdg-open" | "open" => "use `cmd.exe /c start \"\" <file-or-url>`",
        "pbcopy" | "xclip" | "xsel" => "use `clip.exe` (reads stdin)",
        "ifconfig" | "ip" => "use `ipconfig` (or `ipconfig /all`)",
        "zip" => "use Windows' tar: `tar -a -c -f out.zip DIR` (or `python -m zipfile -c out.zip DIR`)",
        "wget" => "use `curl -L -o FILE URL` (curl.exe ships with Windows)",
        "make" | "gmake" => "not installed here; `winget install ezwinports.make` provides GNU make",
        "openssl" => "not bundled; Git for Windows ships one (`\"C:/Program Files/Git/usr/bin/openssl.exe\"`), or use `certutil`",
        "id" => "no uids on Windows; `whoami` (and `whoami /groups` for admin rights)",
        "free" | "uptime" | "lscpu" => "use `systeminfo` (or pwsh `Get-CimInstance Win32_OperatingSystem`)",
        "top" | "htop" | "watch" => "interactive; for a snapshot use `ps aux` (or `tasklist`)",
        "stat" => "not bundled; size: `wc -c < FILE`; times: `date -r FILE`; details: `ls -l FILE`",
        "perl" => "not bundled; for in-place edits `sed -i 's/a/b/' FILE` (GNU sed, keeps CRLF), else `python -c`",
        "ss" | "fuser" => "use `lsof -i :PORT` (or `lsof FILE`); `lsof -t` prints just the PIDs",
        "killall" | "pkill" => "use `taskkill /IM <name>.exe /F` (add /T for the whole tree)",
        "ldd" => "use `msvc-env && dumpbin /dependents <file>`",
        "python3" | "python3.exe" => "on Windows, Python 3 is `python` (or `py -3`)",
        "pip3" => "on Windows use `python -m pip`",
        "cl" | "link" | "lib" | "nmake" | "msbuild" | "dumpbin" | "editbin" | "rc" | "ml64" => {
            "Visual Studio's build tools aren't on PATH; `msvc-env` loads them into this shell (x64 by default), e.g. `msvc-env && cl /nologo hello.c`"
        }
        "strace" | "ltrace" => "no ptrace on Windows; use Process Monitor (procmon) or WPR",
        _ => return None,
    })
}
