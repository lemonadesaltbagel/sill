//! sill: `ps`, `pgrep`, `pkill`, `killall` on Windows.
//!
//! Agents find and stop processes the Linux way: `ps aux | grep node`, `pgrep -f server.py`,
//! `pkill -f "http.server"`, `killall node`. Windows has `tasklist`/`taskkill` with other
//! options and no command lines, so these failed with "command not found". Here they work
//! from a process snapshot with each process's command line (what `-f` matches and `ps`
//! prints). Ending a process ends its tree, as `kill` does on Windows (a `node` started
//! through `npm.cmd` would otherwise keep running).
//!
//! Names and command lines match case-insensitively (`PING.EXE` is `ping`), and `-f` also
//! matches the command line with its program path reduced to the name, as Linux's starts
//! (`ping -n 3 x`, not `C:\Windows\system32\PING.EXE -n 3 x`). pkill and killall never end
//! the shell's own ancestors (an agent's `pkill node` would end its harness) or Windows'
//! session processes (explorer, dwm, csrss, ...).
//!
//! `ps` supports the common forms: no options, `-e`/`-A`, `-f`, `aux`, `-p PID[,PID]`,
//! `-o FIELD,...` (pid, ppid, user, comm, args/cmd/command). Columns the Windows snapshot
//! doesn't have (%CPU, TTY, ...) print as `-`.

use clap::Parser;
use std::fmt::Write as _;
use std::io::Write;

use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// One process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// The executable's file name (`node.exe`).
    pub name: String,
    /// The full command line, if readable (same-user processes); else the name.
    pub cmdline: String,
    pub user: String,
}

/// The name without `.exe`, as Linux's comm (`node`).
fn comm(name: &str) -> &str {
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
}

// --- ps --------------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Eq)]
struct PsQuery {
    all: bool,
    full: bool,
    bsd: bool,
    pids: Vec<u32>,
    fields: Vec<String>,
}

fn parse_ps(args: &[String]) -> Result<PsQuery, String> {
    let mut q = PsQuery::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        match a {
            "-e" | "-A" => q.all = true,
            "-f" | "-F" | "-l" => q.full = true,
            "-ef" | "-eF" | "-Af" | "-fe" | "-ely" | "-el" => {
                q.all = true;
                q.full = true;
            }
            "-p" | "--pid" | "-q" => {
                let v = args.get(i).ok_or("-p needs a PID list")?;
                i += 1;
                q.pids.extend(parse_pids(v)?);
            }
            "-o" | "-O" | "--format" => {
                let v = args.get(i).ok_or("-o needs fields")?;
                i += 1;
                q.fields.extend(
                    v.split([',', ' '])
                        .filter(|f| !f.is_empty())
                        .map(|f| f.split('=').next().unwrap_or(f).to_ascii_lowercase()),
                );
            }
            "-w" | "-ww" | "--no-headers" | "-h" | "--sort" => {}
            _ if !a.starts_with('-') && a.chars().all(|c| "auxwfe".contains(c)) => {
                q.bsd = true;
                q.all |= a.contains('a') || a.contains('x');
            }
            _ if a.starts_with("-p") && a.len() > 2 => q
                .pids
                .extend(parse_pids(a.strip_prefix("-p").unwrap_or_default())?),
            _ if !a.starts_with('-') && a.chars().all(|c| c.is_ascii_digit() || c == ',') => {
                q.pids.extend(parse_pids(a)?);
            }
            _ => {
                return Err(format!(
                    "{a}: not supported on Windows (sill's ps supports -e, -f, aux, -p PID and -o pid,ppid,user,comm,args)"
                ));
            }
        }
    }
    for f in &q.fields {
        if !matches!(
            f.as_str(),
            "pid"
                | "ppid"
                | "user"
                | "uid"
                | "comm"
                | "args"
                | "cmd"
                | "command"
                | "ucomm"
                | "fname"
        ) {
            return Err(format!(
                "-o {f}: not supported on Windows (fields: pid, ppid, user, comm, args)"
            ));
        }
    }
    Ok(q)
}

fn parse_pids(v: &str) -> Result<Vec<u32>, String> {
    v.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim()
                .parse::<u32>()
                .map_err(|_| format!("{s}: not a PID"))
        })
        .collect()
}

fn format_ps(q: &PsQuery, procs: &[Proc], own_pid: u32) -> String {
    let selected: Vec<&Proc> = procs
        .iter()
        .filter(|p| {
            if q.pids.is_empty() {
                q.all || q.bsd || p.pid == own_pid || p.ppid == own_pid
            } else {
                q.pids.contains(&p.pid)
            }
        })
        .collect();
    let mut s = String::new();
    if !q.fields.is_empty() {
        let head: Vec<String> = q
            .fields
            .iter()
            .map(|f| {
                match f.as_str() {
                    "pid" => "PID",
                    "ppid" => "PPID",
                    "user" | "uid" => "USER",
                    "comm" | "ucomm" | "fname" => "COMMAND",
                    _ => "COMMAND",
                }
                .to_owned()
            })
            .collect();
        s.push_str(&head.join(" "));
        s.push('\n');
        for p in selected {
            let cols: Vec<String> = q
                .fields
                .iter()
                .map(|f| match f.as_str() {
                    "pid" => format!("{:>6}", p.pid),
                    "ppid" => format!("{:>6}", p.ppid),
                    "user" | "uid" => p.user.clone(),
                    "comm" | "ucomm" | "fname" => comm(&p.name).to_owned(),
                    _ => p.cmdline.clone(),
                })
                .collect();
            s.push_str(&cols.join(" "));
            s.push('\n');
        }
    } else if q.bsd {
        s.push_str("USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n");
        for p in selected {
            let _ = writeln!(
                s,
                "{:<10} {:>6}    -    -      -     - ?        -    -         - {}",
                p.user, p.pid, p.cmdline
            );
        }
    } else if q.full {
        s.push_str("UID          PID    PPID  C STIME TTY          TIME CMD\n");
        for p in selected {
            let _ = writeln!(
                s,
                "{:<10} {:>6} {:>7}  - -     ?               - {}",
                p.user, p.pid, p.ppid, p.cmdline
            );
        }
    } else {
        s.push_str("    PID TTY          TIME CMD\n");
        for p in selected {
            let _ = writeln!(s, "{:>7} ?               - {}", p.pid, comm(&p.name));
        }
    }
    s
}

// --- pgrep / pkill / killall --------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Eq)]
struct MatchQuery {
    full: bool,
    exact: bool,
    ignore_case: bool,
    list_name: bool,
    list_full: bool,
    count: bool,
    newest: bool,
    oldest: bool,
    signal: Option<i32>,
    patterns: Vec<String>,
}

const SIGNALS: &[(&str, i32)] = &[
    ("HUP", 1),
    ("INT", 2),
    ("QUIT", 3),
    ("KILL", 9),
    ("TERM", 15),
];

fn signal_number(spec: &str) -> Option<i32> {
    if let Ok(n) = spec.parse::<i32>() {
        return SIGNALS.iter().any(|(_, x)| *x == n).then_some(n);
    }
    let up = spec.to_ascii_uppercase();
    let name = up.strip_prefix("SIG").unwrap_or(&up);
    SIGNALS.iter().find(|(n, _)| *n == name).map(|(_, x)| *x)
}

fn parse_match(cmd: &str, args: &[String]) -> Result<MatchQuery, String> {
    let mut q = MatchQuery::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if !a.starts_with('-') || a == "-" {
            q.patterns.push(a.to_owned());
            continue;
        }
        match a {
            "--" => {
                q.patterns.extend(args[i..].iter().cloned());
                break;
            }
            "-f" | "--full" => q.full = true,
            "-x" | "--exact" | "-e" => q.exact = true,
            "-i" | "--ignore-case" | "-I" => q.ignore_case = true,
            "-l" | "--list-name" if cmd == "pgrep" => q.list_name = true,
            "-a" | "--list-full" if cmd == "pgrep" => q.list_full = true,
            "-c" | "--count" => q.count = true,
            "-n" | "--newest" => q.newest = true,
            "-o" | "--oldest" => q.oldest = true,
            "-q" | "--quiet" if cmd == "killall" => {}
            "-s" | "--signal" => {
                let v = args.get(i).ok_or("-s needs a signal")?;
                i += 1;
                q.signal = Some(signal_number(v).ok_or_else(|| format!("{v}: unknown signal"))?);
            }
            _ if cmd != "pgrep" && a.len() > 1 && signal_number(after_first(a)).is_some() => {
                q.signal = signal_number(after_first(a));
            }
            _ if a.starts_with("--signal=") => {
                let v = a.strip_prefix("--signal=").unwrap_or_default();
                q.signal = Some(signal_number(v).ok_or_else(|| format!("{a}: unknown signal"))?);
            }
            _ if a.len() > 2 && after_first(a).chars().all(|c| "fxilac".contains(c)) => {
                for c in after_first(a).chars() {
                    match c {
                        'f' => q.full = true,
                        'x' => q.exact = true,
                        'i' => q.ignore_case = true,
                        'l' => q.list_name = true,
                        'a' => q.list_full = true,
                        _ => q.count = true,
                    }
                }
            }
            _ => {
                return Err(format!(
                    "{a}: not supported on Windows (sill's {cmd} supports -f, -x, -i, -l, -a, -c, -n, -o and -SIGNAL)"
                ));
            }
        }
    }
    if q.patterns.is_empty() {
        return Err(format!("no process name given ({cmd} [-f] PATTERN)"));
    }
    if cmd == "killall" {
        q.exact = true;
    }
    Ok(q)
}

/// `a` without its first character (the `-` of an option).
fn after_first(a: &str) -> &str {
    let mut chars = a.chars();
    chars.next();
    chars.as_str()
}

/// The command line with its program path reduced to the bare name, as a Linux command
/// line starts: `"C:\Windows\system32\PING.EXE" -n 3 x` → `PING -n 3 x`.
fn short_cmdline(cmdline: &str) -> String {
    let t = cmdline.trim_start();
    let (prog, rest) = if let Some(r) = t.strip_prefix('"') {
        r.split_once('"').unwrap_or((r, ""))
    } else {
        // The rest keeps its leading whitespace.
        t.find([' ', '\t']).map_or((t, ""), |i| t.split_at(i))
    };
    let base = prog.rsplit(['\\', '/']).next().unwrap_or(prog);
    format!("{}{rest}", comm(base))
}

/// Names and command lines match case-insensitively: Windows names are (`PING.EXE` is
/// `ping`).
fn matches(q: &MatchQuery, p: &Proc) -> Result<bool, String> {
    for pat in &q.patterns {
        let hit = if q.exact {
            if q.full {
                p.cmdline.eq_ignore_ascii_case(pat)
                    || short_cmdline(&p.cmdline).eq_ignore_ascii_case(pat)
            } else {
                comm(&p.name).eq_ignore_ascii_case(comm(pat))
            }
        } else {
            let re = fancy_regex::RegexBuilder::new(pat)
                .case_insensitive(true)
                .build()
                .map_err(|e| format!("{pat}: {e}"))?;
            if q.full {
                re.is_match(&p.cmdline).unwrap_or(false)
                    || re.is_match(&short_cmdline(&p.cmdline)).unwrap_or(false)
            } else {
                re.is_match(comm(&p.name)).unwrap_or(false)
            }
        };
        if hit {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Processes pkill and killall never end: Windows' own session processes (on Linux,
/// their counterparts need root; agents here often hold an admin token).
const PROTECTED: &[&str] = &[
    "explorer",
    "dwm",
    "csrss",
    "winlogon",
    "wininit",
    "lsass",
    "services",
    "smss",
    "svchost",
    "fontdrvhost",
    "sihost",
    "ctfmon",
    "System",
    "Registry",
    "conhost",
];

fn protected(p: &Proc) -> bool {
    PROTECTED
        .iter()
        .any(|n| comm(&p.name).eq_ignore_ascii_case(n))
}

/// The shell and the processes that started it (the agent harness among them).
fn ancestors(procs: &[Proc], own_pid: u32) -> Vec<u32> {
    let mut out = vec![own_pid];
    let mut cur = own_pid;
    for _ in 0..64 {
        let Some(p) = procs.iter().find(|p| p.pid == cur) else {
            break;
        };
        if p.ppid == 0 || out.contains(&p.ppid) {
            break;
        }
        out.push(p.ppid);
        cur = p.ppid;
    }
    out
}

/// The processes `q` selects, excluding the shell itself and its ancestors' helpers.
fn select<'a>(q: &MatchQuery, procs: &'a [Proc], own_pid: u32) -> Result<Vec<&'a Proc>, String> {
    let mut out = vec![];
    let ancestors = ancestors(procs, own_pid);
    for p in procs {
        if p.pid == own_pid || p.pid == 0 || ancestors.contains(&p.pid) {
            continue;
        }
        if matches(q, p)? {
            out.push(p);
        }
    }
    // The snapshot is in creation order, roughly; -n/-o pick by PID order as a stand-in.
    if q.newest {
        out.sort_by_key(|p| p.pid);
        out = out.into_iter().last().into_iter().collect();
    } else if q.oldest {
        out.sort_by_key(|p| p.pid);
        out.truncate(1);
    }
    Ok(out)
}

fn format_pgrep(q: &MatchQuery, sel: &[&Proc]) -> String {
    if q.count {
        return format!("{}\n", sel.len());
    }
    sel.iter()
        .map(|p| {
            if q.list_full {
                format!("{} {}\n", p.pid, p.cmdline)
            } else if q.list_name {
                format!("{} {}\n", p.pid, comm(&p.name))
            } else {
                format!("{}\n", p.pid)
            }
        })
        .collect()
}

// --- the builtins ---------------------------------------------------------------------------

macro_rules! raw_args_builtin {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Parser)]
        #[command(disable_help_flag = true)]
        pub(crate) struct $name {
            #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
            args: Vec<String>,
        }
        brush_clap_adapter::impl_parse_args!($name);
    };
}

raw_args_builtin!(PsCommand, "Report processes (Windows subset of ps).");
raw_args_builtin!(PgrepCommand, "Find processes by name or command line.");
raw_args_builtin!(
    PkillCommand,
    "Signal (end) processes by name or command line."
);
raw_args_builtin!(KillallCommand, "End processes by name.");

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for PsCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let q = match parse_ps(&self.args) {
            Ok(q) => q,
            Err(m) => {
                writeln!(context.stderr(), "{}: {m}", context.command_name)?;
                return Ok(ExecutionExitCode::InvalidUsage.into());
            }
        };
        let procs = win::snapshot();
        let out = format_ps(&q, &procs, std::process::id());
        write!(context.stdout(), "{out}")?;
        Ok(if !q.pids.is_empty() && out.lines().count() <= 1 {
            ExecutionResult::general_error()
        } else {
            ExecutionResult::success()
        })
    }
}

fn run_match<SE: brush_core::ShellExtensions>(
    context: &brush_core::ExecutionContext<'_, SE>,
    args: &[String],
    kill: bool,
) -> Result<ExecutionResult, brush_core::Error> {
    let name = context.command_name.clone();
    let q = match parse_match(&name, args) {
        Ok(q) => q,
        Err(m) => {
            writeln!(context.stderr(), "{name}: {m}")?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        }
    };
    let procs = win::snapshot();
    let sel = match select(&q, &procs, std::process::id()) {
        Ok(s) => s,
        Err(m) => {
            writeln!(context.stderr(), "{name}: {m}")?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        }
    };
    let found = if !kill {
        write!(context.stdout(), "{}", format_pgrep(&q, &sel))?;
        !sel.is_empty()
    } else {
        let code = u32::try_from(128 + q.signal.unwrap_or(15)).unwrap_or(143);
        let (spared, sel): (Vec<&Proc>, Vec<&Proc>) = sel.into_iter().partition(|p| protected(p));
        for p in &spared {
            writeln!(
                context.stderr(),
                "{name}: not ending {} ({}): a Windows session process",
                p.pid,
                p.name
            )?;
        }
        for p in &sel {
            if let Err(e) = win::end_tree(p.pid, code) {
                writeln!(
                    context.stderr(),
                    "{name}: killing pid {} failed: {e}",
                    p.pid
                )?;
            }
        }
        if q.count {
            writeln!(context.stdout(), "{}", sel.len())?;
        }
        if sel.is_empty() && name == "killall" {
            for pat in &q.patterns {
                writeln!(context.stderr(), "{pat}: no process found")?;
            }
        }
        !sel.is_empty()
    };
    Ok(if found {
        ExecutionResult::success()
    } else {
        ExecutionResult::general_error()
    })
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for PgrepCommand {
    type Error = brush_core::Error;
    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        run_match(&context, &self.args, false)
    }
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for PkillCommand {
    type Error = brush_core::Error;
    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        run_match(&context, &self.args, true)
    }
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for KillallCommand {
    type Error = brush_core::Error;
    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        run_match(&context, &self.args, true)
    }
}

#[cfg(windows)]
mod win {
    use super::Proc;
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            h: HANDLE,
            class: u32,
            info: *mut core::ffi::c_void,
            len: u32,
            ret: *mut u32,
        ) -> i32;
    }
    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

    fn command_line(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u8; 8192];
        let mut ret = 0u32;
        for _ in 0..2 {
            // SAFETY: `buf` is writable for its length; the result is a UNICODE_STRING
            // followed by its buffer.
            let st = unsafe {
                NtQueryInformationProcess(
                    h,
                    PROCESS_COMMAND_LINE_INFORMATION,
                    buf.as_mut_ptr().cast(),
                    u32::try_from(buf.len()).ok()?,
                    &mut ret,
                )
            };
            if st == 0 {
                let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
                let ptr = usize::from_le_bytes(buf[8..16].try_into().ok()?) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                // SAFETY: the buffer the UNICODE_STRING points into is inside `buf`.
                let units = unsafe { std::slice::from_raw_parts(ptr, len / 2) };
                return Some(String::from_utf16_lossy(units));
            }
            if ret as usize > buf.len() {
                buf.resize(ret as usize, 0);
            } else {
                return None;
            }
        }
        None
    }

    fn user(h: HANDLE, cache: &mut HashMap<Vec<u8>, String>) -> Option<String> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: plain Win32 calls with sized buffers; handles are closed.
        unsafe {
            if OpenProcessToken(h, TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut buf = vec![0u8; 256];
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr().cast(),
                u32::try_from(buf.len()).ok()?,
                &mut len,
            ) != 0;
            CloseHandle(token);
            if !ok {
                return None;
            }
            let tu = &*(buf.as_ptr().cast::<TOKEN_USER>());
            let sid = tu.User.Sid;
            let sid_len = windows_sys::Win32::Security::GetLengthSid(sid) as usize;
            let key = std::slice::from_raw_parts(sid.cast::<u8>(), sid_len).to_vec();
            if let Some(u) = cache.get(&key) {
                return Some(u.clone());
            }
            let (mut name, mut domain) = (vec![0u16; 256], vec![0u16; 256]);
            let (mut nl, mut dl) = (256u32, 256u32);
            let mut use_: SID_NAME_USE = 0;
            if LookupAccountSidW(
                std::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut nl,
                domain.as_mut_ptr(),
                &mut dl,
                &mut use_,
            ) == 0
            {
                return None;
            }
            let u = String::from_utf16_lossy(&name[..nl as usize]).to_lowercase();
            cache.insert(key, u.clone());
            Some(u)
        }
    }

    pub(super) fn snapshot() -> Vec<Proc> {
        let mut out = vec![];
        let mut users = HashMap::new();
        // SAFETY: plain Win32 call; checked against INVALID_HANDLE_VALUE.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        // SAFETY: PROCESSENTRY32W is plain data; dwSize is set before use.
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
        // SAFETY: valid snapshot handle and sized entry.
        let mut more = unsafe { Process32FirstW(snap, &mut e) } != 0;
        while more {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            // SAFETY: plain Win32 call; a null handle means no access.
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID) };
            let (cmdline, user) = if h.is_null() {
                (None, None)
            } else {
                let c = command_line(h);
                let u = user(h, &mut users);
                // SAFETY: closing the handle opened above.
                unsafe { CloseHandle(h) };
                (c, u)
            };
            out.push(Proc {
                pid: e.th32ProcessID,
                ppid: e.th32ParentProcessID,
                cmdline: cmdline
                    .filter(|c| !c.trim().is_empty())
                    .unwrap_or_else(|| format!("[{name}]")),
                user: user.unwrap_or_else(|| "-".to_owned()),
                name,
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap, &mut e) } != 0;
        }
        // SAFETY: closing the snapshot handle.
        unsafe { CloseHandle(snap) };
        out
    }

    pub(super) fn end_tree(pid: u32, code: u32) -> Result<(), &'static str> {
        crate::kill_windows::win::terminate_tree(i32::try_from(pid).map_err(|_| "bad pid")?, code)
    }
}

#[cfg(not(windows))]
mod win {
    use super::Proc;
    pub(super) const fn snapshot() -> Vec<Proc> {
        Vec::new()
    }
    pub(super) const fn end_tree(_pid: u32, _code: u32) -> Result<(), &'static str> {
        Err("only on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs() -> Vec<Proc> {
        let p = |pid, ppid, name: &str, cmd: &str| Proc {
            pid,
            ppid,
            name: name.into(),
            cmdline: cmd.into(),
            user: "user".into(),
        };
        vec![
            p(
                100,
                4,
                "node.exe",
                r#""C:\Program Files\nodejs\node.exe" server.js --port 3000"#,
            ),
            p(200, 4, "python.exe", "python -m http.server 8765"),
            p(300, 1, "bash.exe", "bash -c ps"),
            p(400, 300, "Code.exe", "Code.exe --type=renderer"),
        ]
    }
    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn pgrep_and_pkill_selection() {
        let ps = procs();
        let sel = |cmd, args| {
            let q = parse_match(cmd, &a(args)).unwrap();
            select(&q, &ps, 300)
                .unwrap()
                .iter()
                .map(|p| p.pid)
                .collect::<Vec<_>>()
        };
        assert_eq!(sel("pgrep", "node"), vec![100]);
        assert_eq!(sel("pgrep", "-f http.server"), vec![200]);
        assert_eq!(sel("pgrep", "-f server"), vec![100, 200]);
        assert_eq!(sel("pgrep", "-x node.exe"), vec![100]);
        assert_eq!(sel("pgrep", "-i code"), vec![400]);
        assert_eq!(
            sel("pgrep", "bash"),
            Vec::<u32>::new(),
            "the shell itself is excluded"
        );
        assert_eq!(sel("pgrep", "NODE"), vec![100], "case-insensitive");
        assert_eq!(
            sel("pgrep", "-f ^node server.js"),
            vec![100],
            "program path reduced to its name"
        );
        assert_eq!(sel("killall", "node"), vec![100]);
        assert_eq!(
            sel("killall", "nod"),
            Vec::<u32>::new(),
            "killall matches whole names"
        );
        assert_eq!(parse_match("pkill", &a("-9 -f x")).unwrap().signal, Some(9));
        let q = parse_match("pgrep", &a("-af server")).unwrap();
        let out = format_pgrep(&q, &select(&q, &ps, 0).unwrap());
        assert!(
            out.starts_with("100 \"C:\\Program Files\\nodejs\\node.exe\" server.js"),
            "{out}"
        );
        assert!(
            parse_match("pgrep", &a("-P 1 x"))
                .unwrap_err()
                .contains("not supported")
        );
        // The harness that started the shell is never selected.
        let mut ps2 = ps.clone();
        ps2.push(Proc {
            pid: 1,
            ppid: 0,
            name: "node.exe".into(),
            cmdline: "node harness.js".into(),
            user: "u".into(),
        });
        let q = parse_match("pkill", &a("node")).unwrap();
        assert_eq!(
            select(&q, &ps2, 300)
                .unwrap()
                .iter()
                .map(|p| p.pid)
                .collect::<Vec<_>>(),
            vec![100]
        );
        assert_eq!(
            short_cmdline(r#""C:\Windows\system32\PING.EXE" -n 3 x"#),
            "PING -n 3 x"
        );
        assert_eq!(short_cmdline("node.exe server.js"), "node server.js");
    }

    #[test]
    fn ps_forms() {
        let ps = procs();
        let out = format_ps(&parse_ps(&a("aux")).unwrap(), &ps, 300);
        let line = out.lines().find(|l| l.contains("http.server")).unwrap();
        assert_eq!(
            line.split_whitespace().nth(1),
            Some("200"),
            "PID is $2 in `ps aux`"
        );
        let out = format_ps(&parse_ps(&a("-ef")).unwrap(), &ps, 300);
        let line = out.lines().find(|l| l.contains("server.js")).unwrap();
        assert_eq!(
            line.split_whitespace().nth(2),
            Some("4"),
            "PPID is $3 in `ps -ef`"
        );
        let out = format_ps(&parse_ps(&a("-o pid,comm -p 100")).unwrap(), &ps, 300);
        assert_eq!(out, "PID COMMAND\n   100 node\n");
        let out = format_ps(&parse_ps(&[]).unwrap(), &ps, 300);
        assert!(
            out.contains("bash") && out.contains("Code") && !out.contains("node"),
            "{out}"
        );
        assert!(parse_ps(&a("-o rss")).unwrap_err().contains("fields"));
    }
}
