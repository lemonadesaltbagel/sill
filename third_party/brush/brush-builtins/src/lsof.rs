//! sill: `lsof` on Windows, for the two questions agents ask it.
//!
//! - **Which process holds this file?** A delete or rename that fails because a file is
//!   in use (an editor, a dev server, a stuck build) is common on Windows and opaque:
//!   "Permission denied" or "being used by another process". `lsof FILE` and
//!   `lsof +D DIR` answer through the Restart Manager, the API installers use for it.
//! - **Which process has this port?** "Address already in use" is the other half;
//!   `lsof -i :3000` and `kill $(lsof -t -i :3000)` are the habits models bring from Linux
//!   and macOS. Answered from the system's TCP and UDP tables.
//!
//! This is a subset, and says so: files, `+D`, `-i [46][TCP|UDP][:port[-port]]`,
//! `-sTCP:STATE`, `-t`; `-n`, `-P`, `-a`, `-w`, `-l` are accepted and change nothing.
//! Any other option is an error that names what is supported. Exit status 1 when
//! nothing matches, as lsof's.

use clap::Parser;
use std::fmt::Write as _;
use std::io::Write;

use brush_core::{ExecutionExitCode, ExecutionResult, builtins};

/// List processes holding files or network ports (Windows subset of lsof).
#[derive(Parser)]
#[command(disable_help_flag = true)]
pub(crate) struct LsofCommand {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<String>,
}

brush_clap_adapter::impl_parse_args!(LsofCommand);

/// What to list.
#[derive(Debug, Default, PartialEq, Eq)]
struct Query {
    terse: bool,
    files: Vec<String>,
    dirs: Vec<String>,
    /// `-i` given; with these filters.
    net: Option<NetFilter>,
    /// `-sTCP:LISTEN` and the like: TCP states to keep (upper case).
    tcp_states: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Clone)]
struct NetFilter {
    /// 4 or 6; 0 for both.
    family: u8,
    /// "TCP", "UDP" or empty for both.
    protocol: String,
    /// Inclusive port ranges; empty for any.
    ports: Vec<(u16, u16)>,
}

const SUPPORTED: &str =
    "sill's lsof supports FILE..., +D DIR, -i [46][TCP|UDP][:PORT[-PORT]], -sTCP:STATE and -t";

fn parse_net_spec(spec: &str) -> Result<NetFilter, String> {
    let mut f = NetFilter::default();
    let mut rest = spec;
    if let Some(r) = rest.strip_prefix('4') {
        f.family = 4;
        rest = r;
    } else if let Some(r) = rest.strip_prefix('6') {
        f.family = 6;
        rest = r;
    }
    let (proto, port) = match rest.split_once(':') {
        Some((p, port)) => (p, Some(port)),
        None => (rest, None),
    };
    let proto = proto.split('@').next().unwrap_or("");
    match proto.to_ascii_uppercase().as_str() {
        "" => {}
        p @ ("TCP" | "UDP") => p.clone_into(&mut f.protocol),
        _ => return Err(format!("-i {spec}: unknown protocol `{proto}`")),
    }
    if let Some(port) = port {
        for part in port.split(',') {
            let range = match part.split_once('-') {
                Some((a, b)) => (a.parse::<u16>(), b.parse::<u16>()),
                None => (part.parse::<u16>(), part.parse::<u16>()),
            };
            match range {
                (Ok(a), Ok(b)) if a <= b => f.ports.push((a, b)),
                _ => {
                    return Err(format!(
                        "-i {spec}: ports must be numbers (`:3000`, `:8000-8010`)"
                    ));
                }
            }
        }
    }
    Ok(f)
}

/// Whether an argument after `-i` is its spec rather than a file.
fn looks_like_net_spec(a: &str) -> bool {
    let up = a.to_ascii_uppercase();
    a.starts_with(':')
        || a.starts_with('@')
        || up.starts_with("TCP")
        || up.starts_with("UDP")
        || ((a.starts_with('4') || a.starts_with('6'))
            && a.len() <= 12
            && !a.contains(['/', '\\', '.']))
}

fn parse_args(args: &[String]) -> Result<Query, String> {
    let mut q = Query::default();
    let mut i = 0;
    let mut only_files = false;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if only_files || !(a.starts_with('-') || a.starts_with('+')) || a == "-" {
            q.files.push(a.to_owned());
            continue;
        }
        match a {
            "--" => only_files = true,
            "-t" => q.terse = true,
            "-n" | "-P" | "-a" | "-w" | "-l" | "-nP" | "-Pn" => {}
            "+D" | "+d" => {
                let dir = args.get(i).ok_or("+D needs a directory")?;
                q.dirs.push(dir.clone());
                i += 1;
            }
            "-i" => {
                let spec = match args.get(i) {
                    Some(next) if looks_like_net_spec(next) => {
                        i += 1;
                        next.as_str()
                    }
                    _ => "",
                };
                q.net = Some(merge(q.net.take(), parse_net_spec(spec)?));
            }
            "-s" => {
                let spec = args.get(i).ok_or("-s needs TCP:STATE")?;
                i += 1;
                q.tcp_states.extend(parse_state_spec(spec)?);
            }
            _ if a.starts_with("-i") => {
                let spec = a.strip_prefix("-i").unwrap_or_default();
                q.net = Some(merge(q.net.take(), parse_net_spec(spec)?));
            }
            _ if a.starts_with("-s") => {
                q.tcp_states
                    .extend(parse_state_spec(a.strip_prefix("-s").unwrap_or_default())?);
            }
            _ if a.len() > 2
                && a.strip_prefix('-')
                    .is_some_and(|flags| flags.chars().all(|c| "tnPawl".contains(c))) =>
            {
                q.terse |= a.contains('t');
            }
            // Clusters ending in -i: `-ti :3000`, `-ti:3000`, `-nPi tcp:80`.
            _ if cluster_ending_in_i(a).is_some() => {
                let (flags, attached) = cluster_ending_in_i(a).unwrap_or_default();
                q.terse |= flags.contains('t');
                let spec = if !attached.is_empty() {
                    attached
                } else {
                    match args.get(i) {
                        Some(next) if looks_like_net_spec(next) => {
                            i += 1;
                            next.as_str()
                        }
                        _ => "",
                    }
                };
                q.net = Some(merge(q.net.take(), parse_net_spec(spec)?));
            }
            _ => return Err(format!("{a}: not supported on Windows ({SUPPORTED})")),
        }
    }
    if q.files.is_empty() && q.dirs.is_empty() && q.net.is_none() {
        return Err(format!(
            "listing every open file isn't supported on Windows ({SUPPORTED})"
        ));
    }
    Ok(q)
}

fn merge(old: Option<NetFilter>, new: NetFilter) -> NetFilter {
    match old {
        None => new,
        Some(mut o) => {
            o.ports.extend(new.ports);
            if o.protocol.is_empty() {
                o.protocol = new.protocol;
            }
            if o.family == 0 {
                o.family = new.family;
            }
            o
        }
    }
}

fn parse_state_spec(spec: &str) -> Result<Vec<String>, String> {
    let (proto, states) = spec
        .split_once(':')
        .ok_or_else(|| format!("-s {spec}: expected TCP:STATE"))?;
    if !proto.eq_ignore_ascii_case("TCP") {
        return Err(format!("-s {spec}: only TCP states are supported"));
    }
    Ok(states
        .split(',')
        .map(|s| s.trim_start_matches('^').to_ascii_uppercase())
        .collect())
}

/// One line of output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    command: String,
    pid: u32,
    /// `REG`, `DIR`, `IPv4`, `IPv6`.
    kind: String,
    /// `TCP`, `UDP`, or empty for files.
    node: String,
    name: String,
}

/// A flag cluster ending in `-i`, as (the flags before the `i`, what follows it):
/// `-ti:3000` is `("t", ":3000")`. The flags must all be ones that combine with `-i`.
fn cluster_ending_in_i(a: &str) -> Option<(&str, &str)> {
    let (flags, attached) = a.strip_prefix('-')?.split_once('i')?;
    (!flags.is_empty() && flags.chars().all(|c| "tnPawl".contains(c))).then_some((flags, attached))
}

fn format_entries(entries: &[Entry], terse: bool) -> String {
    if terse {
        let mut pids: Vec<u32> = entries.iter().map(|e| e.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        return pids.iter().fold(String::new(), |mut s, p| {
            let _ = writeln!(s, "{p}");
            s
        });
    }
    let w = entries
        .iter()
        .map(|e| e.command.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let mut s = format!(
        "{:<w$} {:>6} {:<4} {:<4} NAME\n",
        "COMMAND", "PID", "TYPE", "NODE"
    );
    for e in entries {
        let _ = writeln!(
            s,
            "{:<w$} {:>6} {:<4} {:<4} {}",
            e.command, e.pid, e.kind, e.node, e.name
        );
    }
    s
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for LsofCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();
        let q = match parse_args(&self.args) {
            Ok(q) => q,
            Err(message) => {
                writeln!(context.stderr(), "{name}: {message}")?;
                return Ok(ExecutionExitCode::InvalidUsage.into());
            }
        };
        let resolve = |p: &str| {
            let path = std::path::Path::new(p);
            let path =
                brush_core::sys::fs::map_shell_path(path).unwrap_or_else(|| path.to_path_buf());
            context.shell.absolute_path(&path)
        };
        let mut entries = vec![];
        let mut problems = vec![];
        for f in &q.files {
            let path = resolve(f);
            if !path.exists() {
                problems.push(format!(
                    "{name}: status error on {f}: No such file or directory"
                ));
                continue;
            }
            match win::holders(std::slice::from_ref(&path)) {
                Ok(found) => entries.extend(found.into_iter().map(|(pid, command)| Entry {
                    command,
                    pid,
                    kind: if path.is_dir() { "DIR" } else { "REG" }.to_owned(),
                    node: String::new(),
                    name: brush_core::sys::fs::display_path(&path),
                })),
                Err(e) => problems.push(format!("{name}: {f}: {e}")),
            }
        }
        for d in &q.dirs {
            let dir = resolve(d);
            let files = win::files_under(&dir, 20_000);
            match win::holders(&files) {
                Ok(found) => entries.extend(found.into_iter().map(|(pid, command)| Entry {
                    command,
                    pid,
                    kind: "DIR".to_owned(),
                    node: String::new(),
                    name: format!(
                        "{} (a file under it)",
                        brush_core::sys::fs::display_path(&dir)
                    ),
                })),
                Err(e) => problems.push(format!("{name}: {d}: {e}")),
            }
        }
        if let Some(filter) = &q.net {
            match win::sockets(filter, &q.tcp_states) {
                Ok(found) => entries.extend(found),
                Err(e) => problems.push(format!("{name}: -i: {e}")),
            }
        }
        for p in &problems {
            writeln!(context.stderr(), "{p}")?;
        }
        if entries.is_empty() {
            return Ok(ExecutionResult::general_error());
        }
        write!(context.stdout(), "{}", format_entries(&entries, q.terse))?;
        Ok(ExecutionResult::success())
    }
}

#[cfg(windows)]
mod win {
    use super::{Entry, NetFilter};
    use std::collections::HashMap;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::RestartManager::{
        RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
    };

    /// Process names by id, from a snapshot.
    fn process_names() -> HashMap<u32, String> {
        let mut map = HashMap::new();
        // SAFETY: plain Win32 call; checked against INVALID_HANDLE_VALUE.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return map;
        }
        // SAFETY: PROCESSENTRY32W is plain data; dwSize is set before use.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
        // SAFETY: `snap` is a valid snapshot handle and `entry` is sized.
        let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            map.insert(
                entry.th32ProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..len]),
            );
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap, &mut entry) } != 0;
        }
        // SAFETY: closing the snapshot handle we created.
        unsafe { CloseHandle(snap) };
        map
    }

    /// Processes holding any of `files` open, by the Restart Manager.
    pub(super) fn holders(files: &[PathBuf]) -> Result<Vec<(u32, String)>, String> {
        if files.is_empty() {
            return Ok(vec![]);
        }
        let mut session = 0u32;
        let mut key = [0u16; 64];
        // SAFETY: out-parameters sized as the API requires (key: CCH_RM_SESSION_KEY + 1).
        let rc = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
        if rc != 0 {
            return Err(format!("Restart Manager unavailable (error {rc})"));
        }
        let wide: Vec<Vec<u16>> = files
            .iter()
            .map(|f| f.as_os_str().encode_wide().chain(Some(0)).collect())
            .collect();
        let ptrs: Vec<*const u16> = wide.iter().map(|w| w.as_ptr()).collect();
        let result = (|| {
            for chunk in ptrs.chunks(1000) {
                // SAFETY: NUL-terminated wide strings that outlive the call.
                let rc = unsafe {
                    RmRegisterResources(
                        session,
                        u32::try_from(chunk.len()).unwrap_or(0),
                        chunk.as_ptr(),
                        0,
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                    )
                };
                if rc != 0 {
                    return Err(format!("Restart Manager error {rc}"));
                }
            }
            let mut infos: Vec<RM_PROCESS_INFO> = vec![RM_PROCESS_INFO::default(); 16];
            loop {
                let mut needed = 0u32;
                let mut count = u32::try_from(infos.len()).unwrap_or(0);
                let mut reasons = 0u32;
                // SAFETY: `infos` holds `count` entries.
                let rc = unsafe {
                    RmGetList(
                        session,
                        &mut needed,
                        &mut count,
                        infos.as_mut_ptr(),
                        &mut reasons,
                    )
                };
                if rc == ERROR_MORE_DATA {
                    infos.resize(needed as usize + 4, RM_PROCESS_INFO::default());
                    continue;
                }
                if rc != 0 {
                    return Err(format!("Restart Manager error {rc}"));
                }
                infos.truncate(count as usize);
                break;
            }
            let names = process_names();
            let mut out: Vec<(u32, String)> = infos
                .iter()
                .map(|i| {
                    let pid = i.Process.dwProcessId;
                    let name = names.get(&pid).cloned().unwrap_or_else(|| {
                        let len = i.strAppName.iter().position(|&c| c == 0).unwrap_or(0);
                        String::from_utf16_lossy(&i.strAppName[..len])
                    });
                    (pid, name)
                })
                .collect();
            out.sort();
            out.dedup();
            Ok(out)
        })();
        // SAFETY: ending the session started above.
        unsafe { RmEndSession(session) };
        result
    }

    /// Files under `dir`, up to `limit`.
    pub(super) fn files_under(dir: &Path, limit: usize) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                match e.file_type() {
                    Ok(t) if t.is_dir() => stack.push(p),
                    Ok(_) => out.push(p),
                    Err(_) => {}
                }
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }

    const TCP_STATES: [&str; 13] = [
        "",
        "CLOSED",
        "LISTEN",
        "SYN_SENT",
        "SYN_RCVD",
        "ESTABLISHED",
        "FIN_WAIT1",
        "FIN_WAIT2",
        "CLOSE_WAIT",
        "CLOSING",
        "LAST_ACK",
        "TIME_WAIT",
        "DELETE_TCB",
    ];

    fn table(tcp: bool, family: u16) -> Result<Vec<u8>, String> {
        let mut size = 0u32;
        let mut buf: Vec<u8> = vec![];
        for _ in 0..4 {
            // SAFETY: `buf` has `size` bytes (or is empty with size 0 for the first call).
            let rc = unsafe {
                if tcp {
                    GetExtendedTcpTable(
                        buf.as_mut_ptr().cast(),
                        &mut size,
                        1,
                        u32::from(family),
                        TCP_TABLE_OWNER_PID_ALL,
                        0,
                    )
                } else {
                    GetExtendedUdpTable(
                        buf.as_mut_ptr().cast(),
                        &mut size,
                        1,
                        u32::from(family),
                        UDP_TABLE_OWNER_PID,
                        0,
                    )
                }
            };
            if rc == 0 {
                return Ok(buf);
            }
            if rc != 122 {
                // Not ERROR_INSUFFICIENT_BUFFER.
                return Err(format!("network table error {rc}"));
            }
            buf = vec![0u8; size as usize + 256];
        }
        Err("network table kept growing".to_owned())
    }

    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap_or([0; 4]))
    }

    fn port(raw: u32) -> u16 {
        u16::from_be_bytes([(raw & 0xff) as u8, ((raw >> 8) & 0xff) as u8])
    }

    fn v4(raw: u32, port_: u16) -> String {
        let ip = std::net::Ipv4Addr::from(u32::from_be(raw));
        if ip.is_unspecified() {
            format!("*:{port_}")
        } else {
            format!("{ip}:{port_}")
        }
    }

    fn v6(bytes: &[u8], port_: u16) -> String {
        let a: [u8; 16] = bytes.try_into().unwrap_or([0; 16]);
        let ip = std::net::Ipv6Addr::from(a);
        if ip.is_unspecified() {
            format!("*:{port_}")
        } else {
            format!("[{ip}]:{port_}")
        }
    }

    pub(super) fn sockets(f: &NetFilter, states: &[String]) -> Result<Vec<Entry>, String> {
        let names = process_names();
        let wants_port =
            |p: u16| f.ports.is_empty() || f.ports.iter().any(|&(a, b)| (a..=b).contains(&p));
        let mut out = vec![];
        let mut push = |pid: u32, kind: &str, node: &str, name: String| {
            out.push(Entry {
                command: names.get(&pid).cloned().unwrap_or_else(|| "?".to_owned()),
                pid,
                kind: kind.to_owned(),
                node: node.to_owned(),
                name,
            });
        };
        for (family, kind, af) in [(4u8, "IPv4", 2u16), (6u8, "IPv6", 23u16)] {
            if f.family != 0 && f.family != family {
                continue;
            }
            if f.protocol.is_empty() || f.protocol == "TCP" {
                let b = table(true, af)?;
                let n = u32_at(&b, 0) as usize;
                let (row, base) = if family == 4 { (24, 4) } else { (56, 4) };
                for i in 0..n {
                    let r = &b[base + i * row..base + (i + 1) * row];
                    let (state, local, remote, pid) = if family == 4 {
                        let lp = port(u32_at(r, 8));
                        let rp = port(u32_at(r, 16));
                        (
                            u32_at(r, 0),
                            (v4(u32_at(r, 4), lp), lp),
                            v4(u32_at(r, 12), rp),
                            u32_at(r, 20),
                        )
                    } else {
                        let lp = port(u32_at(r, 20));
                        let rp = port(u32_at(r, 44));
                        (
                            u32_at(r, 48),
                            (v6(&r[0..16], lp), lp),
                            v6(&r[24..40], rp),
                            u32_at(r, 52),
                        )
                    };
                    let state_name = TCP_STATES.get(state as usize).copied().unwrap_or("UNKNOWN");
                    if !wants_port(local.1)
                        && !remote
                            .rsplit(':')
                            .next()
                            .and_then(|p| p.parse().ok())
                            .is_some_and(|p| wants_port(p) && !f.ports.is_empty())
                    {
                        continue;
                    }
                    if !states.is_empty() && !states.iter().any(|s| s == state_name) {
                        continue;
                    }
                    let name = if state_name == "LISTEN" {
                        format!("{} (LISTEN)", local.0)
                    } else {
                        format!("{}->{} ({state_name})", local.0, remote)
                    };
                    push(pid, kind, "TCP", name);
                }
            }
            if (f.protocol.is_empty() || f.protocol == "UDP") && states.is_empty() {
                let b = table(false, af)?;
                let n = u32_at(&b, 0) as usize;
                let row = if family == 4 { 12 } else { 28 };
                for i in 0..n {
                    let r = &b[4 + i * row..4 + (i + 1) * row];
                    let (local, pid) = if family == 4 {
                        let lp = port(u32_at(r, 4));
                        ((v4(u32_at(r, 0), lp), lp), u32_at(r, 8))
                    } else {
                        let lp = port(u32_at(r, 20));
                        ((v6(&r[0..16], lp), lp), u32_at(r, 24))
                    };
                    if wants_port(local.1) {
                        push(pid, kind, "UDP", local.0);
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
mod win {
    use super::{Entry, NetFilter};
    use std::path::{Path, PathBuf};

    pub(super) fn holders(_files: &[PathBuf]) -> Result<Vec<(u32, String)>, String> {
        Err("only on Windows".to_owned())
    }
    pub(super) const fn files_under(_dir: &Path, _limit: usize) -> Vec<PathBuf> {
        Vec::new()
    }
    pub(super) fn sockets(_f: &NetFilter, _states: &[String]) -> Result<Vec<Entry>, String> {
        Err("only on Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn common_invocations() {
        let q = parse_args(&args("-t -i :3000")).unwrap();
        assert!(q.terse);
        assert_eq!(q.net.unwrap().ports, vec![(3000, 3000)]);

        let q = parse_args(&args("-nP -iTCP:8000-8010 -sTCP:LISTEN")).unwrap();
        let n = q.net.unwrap();
        assert_eq!((n.protocol.as_str(), n.ports), ("TCP", vec![(8000, 8010)]));
        assert_eq!(q.tcp_states, vec!["LISTEN"]);

        let q = parse_args(&args("-ti tcp:5173")).unwrap();
        assert!(q.terse && q.net.unwrap().ports == vec![(5173, 5173)]);
        let q = parse_args(&args("-ti:3000")).unwrap();
        assert!(q.terse && q.net.unwrap().ports == vec![(3000, 3000)]);
        let q = parse_args(&args("-nPi :80")).unwrap();
        assert!(!q.terse && q.net.unwrap().ports == vec![(80, 80)]);

        let q = parse_args(&args("-i")).unwrap();
        assert_eq!(q.net, Some(NetFilter::default()));

        let q = parse_args(&args("build/out.log +D node_modules")).unwrap();
        assert_eq!(
            (q.files, q.dirs),
            (
                vec!["build/out.log".to_owned()],
                vec!["node_modules".to_owned()]
            )
        );

        let q = parse_args(&args("-i 4 -i :80")).unwrap();
        assert_eq!(q.net.unwrap().family, 4);
    }

    #[test]
    fn unsupported_is_explained() {
        for bad in ["-p 123", "-u me", "", "-c node", "-i :http"] {
            let err = parse_args(&args(bad)).unwrap_err();
            assert!(
                err.contains("supports")
                    || err.contains("must be numbers")
                    || err.contains("isn't supported"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn output() {
        let e = vec![
            Entry {
                command: "python.exe".into(),
                pid: 4242,
                kind: "IPv4".into(),
                node: "TCP".into(),
                name: "*:8765 (LISTEN)".into(),
            },
            Entry {
                command: "python.exe".into(),
                pid: 4242,
                kind: "IPv6".into(),
                node: "TCP".into(),
                name: "*:8765 (LISTEN)".into(),
            },
        ];
        let s = format_entries(&e, false);
        let first = s.lines().nth(1).unwrap();
        assert_eq!(
            first.split_whitespace().nth(1),
            Some("4242"),
            "PID is the second column"
        );
        assert_eq!(format_entries(&e, true), "4242\n");
    }
}
