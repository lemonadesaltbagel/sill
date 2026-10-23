//! sill: `msvc-env`: Visual Studio's C/C++ build environment (cl, link, msbuild, the
//! Windows SDK) in this shell.
//!
//! The usual way, `cmd /c 'call "...\vcvars64.bat" >nul && cl ...'`, cost agents more failed
//! calls than anything else: finding the install, quoting through two shells, and harness
//! rewrites inside the cmd text. Here sill finds Visual Studio with vswhere,
//! runs `vcvarsall.bat <arch>` once, and records what it changed: variables it set, and
//! entries it put in front of list variables (PATH, INCLUDE, LIB, ...). That record is
//! cached per install, architecture and toolset, and applied to the shell's environment.
//! Harnesses start a new shell for every command, so the cache is what makes
//! `msvc-env && cl ...` cost milliseconds instead of seconds.

use clap::Parser;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use brush_core::env::{EnvironmentLookup, EnvironmentScope};
use brush_core::{ExecutionExitCode, ExecutionResult, builtins, variables};

/// Load Visual Studio's C/C++ build environment (cl, link, msbuild) into this shell.
#[derive(Parser)]
pub(crate) struct MsvcEnvCommand {
    /// Architecture, as vcvarsall.bat takes it: x64, x86, arm64, x64_arm64, ...
    #[arg(default_value = "x64")]
    arch: String,

    /// Print `export` lines instead of changing this shell.
    #[arg(long)]
    print: bool,

    /// Run vcvarsall.bat again instead of using the cached result.
    #[arg(long)]
    refresh: bool,
}

brush_clap_adapter::impl_parse_args!(MsvcEnvCommand);

/// What vcvarsall.bat changed.
#[derive(Debug, Default, PartialEq)]
struct Delta {
    /// Variables it set (or changed), with their values.
    set: BTreeMap<String, String>,
    /// Entries it put in front of list variables, in order.
    prepend: BTreeMap<String, Vec<String>>,
}

/// List variables: vcvars prepends to these rather than replacing them.
fn is_list_var(name: &str) -> bool {
    const LISTS: &[&str] = &[
        "PATH", "INCLUDE", "LIB", "LIBPATH", "EXTERNAL_INCLUDE", "WINDOWSLIBPATH",
        "__VSCMD_PREINIT_PATH",
    ];
    LISTS.iter().any(|l| name.eq_ignore_ascii_case(l))
}

/// Variables not to carry over: per-invocation state.
fn is_volatile(name: &str) -> bool {
    name.is_empty()
        || name.starts_with('=')
        || ["PROMPT", "VSCMD_START_DIR", "__VSCMD_PREINIT_PATH"]
            .iter()
            .any(|v| name.eq_ignore_ascii_case(v))
}

/// An entry as compared: case, separators and a trailing separator don't matter.
fn norm(entry: &str) -> String {
    entry.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

fn split_list(v: &str) -> Vec<String> {
    v.split(';').filter(|e| !e.is_empty()).map(str::to_owned).collect()
}

fn delta(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Delta {
    let get = |name: &str| {
        before
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let mut d = Delta::default();
    for (name, value) in after {
        if is_volatile(name) || get(name) == Some(value.as_str()) {
            continue;
        }
        match get(name) {
            Some(old) if is_list_var(name) => {
                let old: Vec<String> = split_list(old).iter().map(|e| norm(e)).collect();
                let mut new: Vec<String> = vec![];
                for e in split_list(value) {
                    if !old.contains(&norm(&e)) && !new.iter().any(|n| norm(n) == norm(&e)) {
                        new.push(e);
                    }
                }
                if !new.is_empty() {
                    d.prepend.insert(name.to_ascii_uppercase(), new);
                }
            }
            _ => {
                d.set.insert(name.clone(), value.clone());
            }
        }
    }
    d
}

/// The cache file format: `set NAME=value` and `prepend NAME=entry` lines.
fn serialize(d: &Delta) -> String {
    let mut s = String::from("# sill msvc-env cache v1\n");
    for (k, v) in &d.set {
        s.push_str(&format!("set {k}={v}\n"));
    }
    for (k, entries) in &d.prepend {
        for e in entries {
            s.push_str(&format!("prepend {k}={e}\n"));
        }
    }
    s
}

fn parse(s: &str) -> Option<Delta> {
    let mut lines = s.lines();
    if lines.next()? != "# sill msvc-env cache v1" {
        return None;
    }
    let mut d = Delta::default();
    for line in lines {
        let (kind, rest) = line.split_once(' ')?;
        let (k, v) = rest.split_once('=')?;
        match kind {
            "set" => {
                d.set.insert(k.to_owned(), v.to_owned());
            }
            "prepend" => d.prepend.entry(k.to_owned()).or_default().push(v.to_owned()),
            _ => return None,
        }
    }
    Some(d)
}

/// Parses `set` output: `NAME=value` lines.
fn parse_set_output(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.trim_end_matches('\r').split_once('='))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fn valid_arch(arch: &str) -> bool {
    !arch.is_empty() && arch.len() <= 16 && arch.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for MsvcEnvCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let name = context.command_name.clone();
        let arch = self.arch.to_ascii_lowercase();
        if !valid_arch(&arch) {
            writeln!(context.stderr(), "{name}: {}: not an architecture vcvarsall.bat takes (x64, x86, arm64, x64_arm64, ...)", self.arch)?;
            return Ok(ExecutionExitCode::InvalidUsage.into());
        }

        // The environment a child of this shell gets, in Windows form.
        let before: BTreeMap<String, String> = context
            .shell
            .env()
            .iter_exported()
            .filter(|(_, v)| v.value().is_set())
            .map(|(k, v)| {
                let value = v.value().to_cow_str(context.shell).into_owned();
                let value = if k == "PATH" { win::windows_path_list(&value) } else { value };
                (k.clone(), value)
            })
            .collect();

        let found = match win::load(&arch, &before, self.refresh) {
            Ok(found) => found,
            Err(message) => {
                writeln!(context.stderr(), "{name}: {message}")?;
                return Ok(ExecutionResult::general_error());
            }
        };

        // The final values, against this shell's current ones.
        let mut values: Vec<(String, String)> = vec![];
        for (k, v) in &found.delta.set {
            values.push((k.clone(), v.clone()));
        }
        for (k, entries) in &found.delta.prepend {
            let current = context
                .shell
                .env()
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.value().to_cow_str(context.shell).into_owned())
                .unwrap_or_default();
            let value = if k == "PATH" {
                // PATH is POSIX-style inside the shell.
                let current_win = win::windows_path_list(&current);
                let have: Vec<String> = split_list(&current_win).iter().map(|e| norm(e)).collect();
                let add: Vec<&String> = entries.iter().filter(|e| !have.contains(&norm(e))).collect();
                if add.is_empty() {
                    continue;
                }
                let joined = add.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(";");
                let add_posix = win::posix_path_list(&joined);
                if current.is_empty() { add_posix } else { format!("{add_posix}:{current}") }
            } else {
                let have: Vec<String> = split_list(&current).iter().map(|e| norm(e)).collect();
                let mut list: Vec<String> = entries.iter().filter(|e| !have.contains(&norm(e))).cloned().collect();
                if list.is_empty() {
                    continue;
                }
                list.extend(split_list(&current));
                list.join(";")
            };
            values.push((k.clone(), value));
        }

        if self.print {
            for (k, v) in &values {
                writeln!(context.stdout(), "export {k}='{}'", v.replace('\'', "'\\''"))?;
            }
            return Ok(ExecutionResult::success());
        }

        for (k, v) in values {
            context.shell.env_mut().update_or_add(
                k,
                variables::ShellValueLiteral::Scalar(v),
                |var| {
                    var.export();
                    Ok(())
                },
                EnvironmentLookup::Anywhere,
                EnvironmentScope::Global,
            )?;
        }
        writeln!(
            context.stderr(),
            "{name}: {} (MSVC {}), {arch}{}",
            found.install.display(),
            found.delta.set.get("VCToolsVersion").map_or("?", String::as_str),
            if found.cached { ", cached" } else { "" }
        )?;
        Ok(ExecutionResult::success())
    }
}

struct Found {
    install: PathBuf,
    delta: Delta,
    cached: bool,
}

#[cfg(windows)]
mod win {
    use super::{Delta, Found, parse, parse_set_output, serialize};
    use std::collections::BTreeMap;
    use std::hash::{Hash, Hasher};
    use std::os::windows::process::CommandExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    pub(super) use brush_core::sys::fs::{posix_path_list, windows_path_list};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn program_files_x86() -> PathBuf {
        std::env::var_os("ProgramFiles(x86)")
            .map_or_else(|| PathBuf::from(r"C:\Program Files (x86)"), PathBuf::from)
    }

    fn vswhere() -> PathBuf {
        program_files_x86().join(r"Microsoft Visual Studio\Installer\vswhere.exe")
    }

    /// The newest install with the C++ build tools.
    fn find_install() -> Result<PathBuf, String> {
        let vswhere = vswhere();
        let none = "no Visual Studio with the C++ build tools found; install them with \
            `winget install Microsoft.VisualStudio.2022.BuildTools --override \"--quiet --add \
            Microsoft.VisualStudio.Workload.VCTools --includeRecommended\"`";
        if !vswhere.is_file() {
            return Err(none.to_owned());
        }
        for requires in [&["-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"][..], &[][..]] {
            let out = Command::new(&vswhere)
                .args(["-latest", "-products", "*", "-utf8", "-property", "installationPath"])
                .args(requires)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| format!("{}: {e}", vswhere.display()))?;
            let path = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned();
            if !path.is_empty() && Path::new(&path).join(r"VC\Auxiliary\Build\vcvarsall.bat").is_file() {
                return Ok(PathBuf::from(path));
            }
        }
        Err(none.to_owned())
    }

    /// Identifies what vcvarsall.bat's result depends on: the install, the architecture,
    /// the script, and the toolsets and SDKs present.
    fn cache_file(install: &Path, arch: &str) -> Option<PathBuf> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        install.to_string_lossy().to_ascii_lowercase().hash(&mut h);
        arch.hash(&mut h);
        let vcvarsall = install.join(r"VC\Auxiliary\Build\vcvarsall.bat");
        std::fs::metadata(&vcvarsall).and_then(|m| m.modified()).ok().hash(&mut h);
        for dir in [install.join(r"VC\Tools\MSVC"), program_files_x86().join(r"Windows Kits\10\Include")] {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            names.sort();
            names.hash(&mut h);
        }
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(base).join(r"sill\msvc-env").join(format!("{arch}-{:016x}.env", h.finish())))
    }

    /// Runs `vcvarsall.bat <arch>` in cmd.exe with `before` as its environment and returns
    /// what it changed.
    fn run_vcvarsall(install: &Path, arch: &str, before: &BTreeMap<String, String>) -> Result<Delta, String> {
        let vcvarsall = install.join(r"VC\Auxiliary\Build\vcvarsall.bat");
        let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into());
        // `/u`: internal commands (`set`) write UTF-16, whatever the code page.
        // NoDefaultCurrentDirectoryInExePath (Claude Code sets it) makes VsDevCmd fail to
        // run vswhere.exe from its own directory; this child runs only Microsoft's scripts.
        let mut env = before.clone();
        env.retain(|k, _| !k.eq_ignore_ascii_case("NoDefaultCurrentDirectoryInExePath"));
        let out = Command::new(comspec)
            .raw_arg(format!("/d /u /s /c \"\"{}\" {arch} >nul && set\"", vcvarsall.display()))
            .env_clear()
            .envs(&env)
            .current_dir(install)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("cmd.exe: {e}"))?;
        let units: Vec<u16> = out.stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let after = parse_set_output(&String::from_utf16_lossy(&units));
        if !out.status.success() || !after.keys().any(|k| k.eq_ignore_ascii_case("VCToolsInstallDir")) {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "vcvarsall.bat {arch} failed{}",
                err.lines().find(|l| !l.trim().is_empty()).map(|l| format!(": {}", l.trim())).unwrap_or_default()
            ));
        }
        Ok(super::delta(before, &after))
    }

    pub(super) fn load(arch: &str, before: &BTreeMap<String, String>, refresh: bool) -> Result<Found, String> {
        let install = find_install()?;
        let cache = cache_file(&install, arch);
        if !refresh
            && let Some(delta) = cache.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| parse(&s))
        {
            return Ok(Found { install, delta, cached: true });
        }
        let delta = run_vcvarsall(&install, arch, before)?;
        if let Some(file) = cache {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&file, serialize(&delta));
        }
        Ok(Found { install, delta, cached: false })
    }
}

#[cfg(not(windows))]
mod win {
    use super::{Delta, Found};
    use std::collections::BTreeMap;

    pub(super) fn windows_path_list(s: &str) -> String {
        s.to_owned()
    }
    pub(super) fn posix_path_list(s: &str) -> String {
        s.to_owned()
    }
    pub(super) fn load(_arch: &str, _before: &BTreeMap<String, String>, _refresh: bool) -> Result<Found, String> {
        let _ = Delta::default();
        Err("Visual Studio's build tools exist only on Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn delta_records_sets_and_prepends() {
        let before = map(&[("Path", r"C:\Windows;C:\Tools"), ("INCLUDE", ""), ("KEEP", "1"), ("PROMPT", "$P$G")]);
        let after = map(&[
            ("Path", r"C:\VS\bin;C:\SDK\bin;C:\Windows;C:\Tools"),
            ("INCLUDE", r"C:\VS\include;C:\SDK\include"),
            ("KEEP", "1"),
            ("VCToolsVersion", "14.44.35207"),
            ("PROMPT", "$P$G changed"),
            ("=C:", r"C:\x"),
        ]);
        let d = delta(&before, &after);
        assert_eq!(d.prepend.get("PATH"), Some(&vec![r"C:\VS\bin".to_owned(), r"C:\SDK\bin".to_owned()]));
        assert_eq!(d.set.get("VCToolsVersion").map(String::as_str), Some("14.44.35207"));
        // An empty list variable before: the whole value is new.
        assert_eq!(d.prepend.get("INCLUDE"), Some(&vec![r"C:\VS\include".to_owned(), r"C:\SDK\include".to_owned()]));
        assert!(!d.set.contains_key("KEEP") && !d.set.contains_key("PROMPT") && !d.set.contains_key("=C:"));
    }

    #[test]
    fn cache_round_trip() {
        let mut d = Delta::default();
        d.set.insert("VCToolsVersion".into(), "14.44".into());
        d.set.insert("WEIRD".into(), "a=b c".into());
        d.prepend.insert("PATH".into(), vec![r"C:\a b".into(), r"C:\c".into()]);
        assert_eq!(parse(&serialize(&d)), Some(d));
        assert_eq!(parse("not a cache"), None);
    }

    #[test]
    fn architectures() {
        for a in ["x64", "x86", "arm64", "x64_arm64", "amd64"] {
            assert!(valid_arch(a), "{a}");
        }
        for a in ["", "x64 & calc", "x64\"", "../x"] {
            assert!(!valid_arch(a), "{a}");
        }
    }
}
