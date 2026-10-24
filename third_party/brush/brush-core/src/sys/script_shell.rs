//! sill: package.json scripts run in sill, unless a script shell is configured.
//!
//! On Windows npm, yarn 1 and pnpm run `package.json` scripts with cmd.exe. Most scripts
//! are written for `sh` (in a survey of 28,858 scripts, ~8% fail or silently misbehave
//! under cmd.exe, ~0.06% under bash), and with sill's tools on PATH cmd.exe
//! half-runs them. So when sill starts one of these package managers it points their
//! script shell at itself: `npm_config_script_shell` (npm, yarn 1, pnpm 10,
//! concurrently) and `pnpm_config_script_shell` (pnpm 11+).
//!
//! The same goes for `cmd /c npm run x` and `powershell -c "npm run x"`: cmd.exe,
//! PowerShell or pwsh started with an argument that names a package manager as a word.
//! Other programs never get the variables, though they may start npm: they include
//! long-lived ones (`code .`) that would pass them on to the user's own terminals.
//!
//! The rule is "only when unset". If any script-shell setting exists, in the environment
//! (even empty), an `.npmrc`, `pnpm-workspace.yaml` or pnpm's global `config.yaml`, sill
//! adds nothing: an environment variable outranks `.npmrc` for npm, so setting it
//! regardless would silently override a project that chose cmd.exe. Processes started
//! by the package manager inherit the variables, and those are then set. Dependency
//! install scripts are included. `SILL_NO_ENV_DEFAULTS=1` turns this off.

#![cfg_attr(not(windows), allow(dead_code, reason = "used on Windows; tested everywhere"))]

use std::path::{Path, PathBuf};

/// The programs whose script shell sill sets, by file stem (`npm.cmd`, `pnpm.exe`, ...).
const PACKAGE_MANAGERS: &[&str] = &["npm", "npx", "pnpm", "pnpx", "yarn", "yarnpkg"];

/// Shells that get the variables when an argument names a package manager.
const COMMAND_SHELLS: &[&str] = &["cmd", "powershell", "pwsh"];

const NPM_VAR: &str = "npm_config_script_shell";
const PNPM_VAR: &str = "pnpm_config_script_shell";

/// Adds the script-shell variables to `env` (a child's environment) when `program` is a
/// package manager, or a command shell told to run one, and no script shell is
/// configured for `working_dir`.
pub(crate) fn add_defaults<S: AsRef<str>>(
    program: &Path,
    args: &[S],
    working_dir: &Path,
    env: &mut Vec<(String, String)>,
) {
    let starts_package_manager =
        is_package_manager(program) || (is_command_shell(program) && names_package_manager(args));
    if !starts_package_manager {
        return;
    }
    if get(env, "SILL_NO_ENV_DEFAULTS").is_some_and(|v| !v.is_empty() && v != "0") {
        return;
    }
    let Some(shell) = own_path() else { return };
    if is_configured(&Sources::new(program, working_dir, env)) {
        return;
    }
    env.push((NPM_VAR.to_owned(), shell.clone()));
    env.push((PNPM_VAR.to_owned(), shell));
}

fn is_package_manager(program: &Path) -> bool {
    program
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| PACKAGE_MANAGERS.iter().any(|pm| stem.eq_ignore_ascii_case(pm)))
}

fn is_command_shell(program: &Path) -> bool {
    program
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| COMMAND_SHELLS.iter().any(|sh| stem.eq_ignore_ascii_case(sh)))
}

/// Whether an argument has a package manager's name as a word: `npm run x`,
/// `C:\nodejs\npm.cmd test`, `cd app && pnpm i`; not `npm-run-all` or `npmrc`.
fn names_package_manager<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|arg| {
        arg.as_ref()
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .any(|word| PACKAGE_MANAGERS.iter().any(|pm| word.eq_ignore_ascii_case(pm)))
    })
}

/// sill's own executable, with `/` separators: npm runs a bare `npx` or `npm explore` as
/// `<shell> -c <shell path>`, and bash would read `\` there as escapes.
fn own_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.to_str()?.replace('\\', "/"))
}

/// Where a script shell could be configured.
#[derive(Debug, Default)]
struct Sources {
    /// Names of the variables in the child's environment.
    env_names: Vec<String>,
    /// `.npmrc`-format files: project (every ancestor of the working directory), user,
    /// global and npm's builtin.
    npmrc_files: Vec<PathBuf>,
    /// pnpm YAML settings: `pnpm-workspace.yaml` in every ancestor, global `config.yaml`.
    pnpm_yaml_files: Vec<PathBuf>,
}

impl Sources {
    fn new(program: &Path, working_dir: &Path, env: &[(String, String)]) -> Self {
        let mut s = Self {
            env_names: env.iter().map(|(k, _)| k.clone()).collect(),
            ..Self::default()
        };
        for dir in working_dir.ancestors() {
            s.npmrc_files.push(dir.join(".npmrc"));
            s.pnpm_yaml_files.push(dir.join("pnpm-workspace.yaml"));
        }

        // User config: `npm_config_userconfig`, else `~/.npmrc` (npm uses os.homedir()).
        let home = get(env, "USERPROFILE").or_else(|| get(env, "HOME"));
        match npm_env(env, "userconfig") {
            Some(path) => s.npmrc_files.push(path.into()),
            None => s.npmrc_files.extend(home.map(|h| Path::new(h).join(".npmrc"))),
        }

        // Global config: `npm_config_globalconfig`, else `<prefix>/etc/npmrc`. The prefix
        // is `npm_config_prefix`/`PREFIX`, else what the Node.js installer's builtin
        // npmrc says (`%APPDATA%\npm`), else Node's own directory.
        let program_dir = program.parent();
        if let Some(path) = npm_env(env, "globalconfig") {
            s.npmrc_files.push(path.into());
        } else {
            let prefixes = npm_env(env, "prefix")
                .or_else(|| get(env, "PREFIX"))
                .map(PathBuf::from)
                .into_iter()
                .chain(get(env, "APPDATA").map(|a| Path::new(a).join("npm")))
                .chain(program_dir.map(Path::to_path_buf));
            s.npmrc_files.extend(prefixes.map(|p| p.join("etc").join("npmrc")));
        }
        // npm's builtin config, beside npm itself.
        if let Some(dir) = program_dir {
            s.npmrc_files.push(dir.join("node_modules").join("npm").join("npmrc"));
        }

        // pnpm's global config.yaml.
        if let Some(xdg) = get(env, "XDG_CONFIG_HOME") {
            s.pnpm_yaml_files.push(Path::new(xdg).join("pnpm").join("config.yaml"));
        } else if let Some(local) = get(env, "LOCALAPPDATA") {
            s.pnpm_yaml_files.push(Path::new(local).join("pnpm").join("config").join("config.yaml"));
        }
        s
    }
}

fn is_configured(sources: &Sources) -> bool {
    sources.env_names.iter().any(|name| {
        let key = normalize_key(name);
        key == NPM_VAR || key == PNPM_VAR
    }) || sources
        .npmrc_files
        .iter()
        .any(|f| std::fs::read_to_string(f).is_ok_and(|text| npmrc_sets_script_shell(&text)))
        || sources
            .pnpm_yaml_files
            .iter()
            .any(|f| std::fs::read_to_string(f).is_ok_and(|text| yaml_sets_script_shell(&text)))
}

/// Config keys compare case-insensitively, with `-` and `_` alike (npm's env names).
fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace('-', "_")
}

/// Whether an `.npmrc` (ini) text has a `script-shell` key.
fn npmrc_sets_script_shell(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        !line.starts_with([';', '#'])
            && line.split_once('=').is_some_and(|(key, _)| normalize_key(key) == "script_shell")
    })
}

/// Whether a pnpm YAML settings text has a top-level `scriptShell` key.
fn yaml_sets_script_shell(text: &str) -> bool {
    text.lines().any(|line| {
        !line.starts_with([' ', '\t', '#'])
            && line.split_once(':').is_some_and(|(key, _)| {
                let key = key.trim().trim_matches(['"', '\'']);
                key == "scriptShell" || key == "script-shell"
            })
    })
}

/// A variable in `env`, by name, ignoring case (Windows names are case-insensitive).
fn get<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// An `npm_config_<key>` variable, as npm reads it.
fn npm_env<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    let want = format!("npm_config_{key}");
    env.iter()
        .rev()
        .find(|(k, _)| normalize_key(k) == want)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_ARGS: &[&str] = &[];

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn package_managers() {
        for p in ["C:/nodejs/npm.cmd", "npx.cmd", "pnpm.exe", "PNPM.CMD", "yarn.cmd", "/usr/bin/npm"] {
            assert!(is_package_manager(Path::new(p)), "{p}");
        }
        for p in ["node.exe", "bun.exe", "npm-run-all.cmd", "concurrently.cmd"] {
            assert!(!is_package_manager(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn command_shells_running_a_package_manager() {
        for (program, args) in [
            ("C:/Windows/System32/cmd.exe", &["/d", "/s", "/c", "npm run build"][..]),
            ("cmd.exe", &["/c", "cd app && pnpm install"][..]),
            ("powershell.exe", &["-NoProfile", "-Command", "npx tsc"][..]),
            ("pwsh", &["-c", r"& 'C:\nodejs\npm.cmd' test"][..]),
            ("cmd", &["/c", "yarn"][..]),
        ] {
            assert!(
                is_command_shell(Path::new(program)) && names_package_manager(args),
                "{program} {args:?}"
            );
        }
        for args in [&["/c", "npm-run-all build"][..], &["/c", "type .npmrc"][..], &["/c", "code ."][..]] {
            assert!(!names_package_manager(args), "{args:?}");
        }
        assert!(!is_command_shell(Path::new("node.exe")));
    }

    #[test]
    fn via_cmd_exe() {
        let root = dir();
        let mut e = isolated(&root, &[]);
        add_defaults(Path::new("C:/Windows/System32/cmd.exe"), &["/c", "npm run x"], &root, &mut e);
        assert!(get(&e, NPM_VAR).is_some());
        let mut e = isolated(&root, &[]);
        add_defaults(Path::new("C:/Windows/System32/cmd.exe"), &["/c", "code ."], &root, &mut e);
        assert_eq!(get(&e, NPM_VAR), None);
    }

    #[test]
    fn npmrc_keys() {
        assert!(npmrc_sets_script_shell("registry=x\nscript-shell=C:\\Windows\\System32\\cmd.exe\n"));
        assert!(npmrc_sets_script_shell("  Script_Shell = pwsh\r\n"));
        assert!(npmrc_sets_script_shell("script-shell="));
        assert!(!npmrc_sets_script_shell("; script-shell=cmd\n# script-shell=cmd\nshell=cmd\n"));
        assert!(!npmrc_sets_script_shell("//registry.npmjs.org/:_authToken=script-shell"));
    }

    #[test]
    fn yaml_keys() {
        assert!(yaml_sets_script_shell("packages:\n  - a\nscriptShell: bash\n"));
        assert!(yaml_sets_script_shell("\"scriptShell\": cmd.exe"));
        assert!(!yaml_sets_script_shell("packages:\n  scriptShell: x\n# scriptShell: y\n"));
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sill-script-shell-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(d.join("proj/sub")).unwrap();
        d
    }

    /// `env` plus a home and app-data dir inside `root`, so the real ones aren't read.
    fn isolated(root: &Path, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut e = env(extra);
        for k in ["USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
            e.push((k.to_owned(), root.join("home").to_string_lossy().into_owned()));
        }
        e
    }

    fn npm(root: &Path) -> PathBuf {
        root.join("nodejs").join("npm.cmd")
    }

    #[test]
    fn sets_both_when_unset() {
        let root = dir();
        let mut e = isolated(&root, &[("PATH", "x")]);
        add_defaults(&npm(&root), NO_ARGS, &root.join("proj/sub"), &mut e);
        let shell = get(&e, NPM_VAR).expect("npm var");
        assert_eq!(get(&e, PNPM_VAR), Some(shell));
        assert!(!shell.contains('\\'), "{shell}");
    }

    #[test]
    fn not_for_other_programs() {
        let root = dir();
        let mut e = isolated(&root, &[]);
        add_defaults(&root.join("node.exe"), NO_ARGS, &root, &mut e);
        assert_eq!(get(&e, NPM_VAR), None);
    }

    #[test]
    fn env_setting_wins_even_empty_and_any_case() {
        let root = dir();
        for name in ["npm_config_script_shell", "NPM_CONFIG_SCRIPT_SHELL", "npm_config_script-shell", "pnpm_config_script_shell"] {
            let mut e = isolated(&root, &[(name, "")]);
            add_defaults(&npm(&root), NO_ARGS, &root.join("proj"), &mut e);
            assert_eq!(e.len(), 4, "{name}: {e:?}");
        }
    }

    #[test]
    fn opt_out() {
        let root = dir();
        let mut e = isolated(&root, &[("SILL_NO_ENV_DEFAULTS", "1")]);
        add_defaults(&npm(&root), NO_ARGS, &root, &mut e);
        assert_eq!(get(&e, NPM_VAR), None);
    }

    #[test]
    fn project_and_ancestor_npmrc_win() {
        let root = dir();
        std::fs::write(root.join("proj/.npmrc"), "script-shell=cmd.exe\n").unwrap();
        let mut e = isolated(&root, &[]);
        add_defaults(&npm(&root), NO_ARGS, &root.join("proj/sub"), &mut e);
        assert_eq!(get(&e, NPM_VAR), None);
        assert_eq!(get(&e, PNPM_VAR), None);
    }

    #[test]
    fn user_global_and_builtin_npmrc_win() {
        let root = dir();
        std::fs::create_dir_all(root.join("home/npm/etc")).unwrap();
        std::fs::create_dir_all(root.join("nodejs/node_modules/npm")).unwrap();
        for (file, extra) in [
            (root.join("home/.npmrc"), vec![]),
            (root.join("home/npm/etc/npmrc"), vec![]),
            (root.join("nodejs/node_modules/npm/npmrc"), vec![]),
            (root.join("custom-rc"), vec![("npm_config_userconfig", root.join("custom-rc").to_string_lossy().into_owned())]),
        ] {
            std::fs::write(&file, "script-shell=pwsh\n").unwrap();
            let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let mut e = isolated(&root, &extra);
            add_defaults(&npm(&root), NO_ARGS, &root.join("proj"), &mut e);
            assert_eq!(get(&e, NPM_VAR), None, "{}", file.display());
            std::fs::remove_file(&file).unwrap();
        }
    }

    #[test]
    fn pnpm_settings_win() {
        let root = dir();
        std::fs::write(root.join("proj/pnpm-workspace.yaml"), "scriptShell: cmd.exe\n").unwrap();
        let mut e = isolated(&root, &[]);
        add_defaults(&root.join("pnpm.cmd"), NO_ARGS, &root.join("proj/sub"), &mut e);
        assert_eq!(get(&e, PNPM_VAR), None);
        assert_eq!(get(&e, NPM_VAR), None);

        std::fs::remove_file(root.join("proj/pnpm-workspace.yaml")).unwrap();
        std::fs::create_dir_all(root.join("home/pnpm/config")).unwrap();
        std::fs::write(root.join("home/pnpm/config/config.yaml"), "scriptShell: pwsh\n").unwrap();
        let mut e = isolated(&root, &[]);
        add_defaults(&root.join("pnpm.cmd"), NO_ARGS, &root.join("proj"), &mut e);
        assert_eq!(get(&e, PNPM_VAR), None);
    }
}
