//! Environment variable retrieval for Windows.
//!
//! On Windows, well-known environment variable names are normalized to their
//! canonical POSIX forms (e.g. `Path` → `PATH`), and `HOME` is synthesized
//! from `USERPROFILE` or `HOMEDRIVE`+`HOMEPATH` if not already present.

use std::collections::BTreeMap;

/// Retrieves environment variables from the host process, applying
/// Windows-specific fixups.
///
/// Normalizes well-known variable names to POSIX conventions, copies
/// `TEMP`/`TMP` to `TMPDIR` (preserving originals for native Windows apps),
/// and synthesizes `HOME` if it is not natively defined.
///
/// A [`BTreeMap`] is used (rather than `HashMap`) so iteration order is
/// deterministic across runs. If two source variables collide under the same
/// canonical name (e.g. both `Path` and `PATH` are set to different values),
/// the conflict is logged and the last-seen value wins.
pub(crate) fn get_host_env_vars() -> impl Iterator<Item = (String, String)> {
    collect_host_env_vars(std::env::vars())
}

/// Collects and normalizes a set of environment variables. Exposed as a
/// pure function (taking the source iterator) so it can be unit-tested
/// without touching the process environment.
fn collect_host_env_vars<I>(source: I) -> std::collections::btree_map::IntoIter<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut vars = BTreeMap::new();

    // Normalize host env vars and inject them into the map.
    for (k, v) in source {
        let normalized = normalize_env_name(&k);
        if let Some(existing) = vars.get(&normalized)
            && existing != &v
        {
            tracing::warn!(
                "environment variable collision under canonical name {normalized}: \
                 two different values were supplied (last-write wins)"
            );
        }
        vars.insert(normalized, v);
    }

    // Synthesize HOME from Windows-native variables if not already present.
    if !vars.contains_key("HOME") {
        let home = vars.get("USERPROFILE").cloned().or_else(|| {
            let d = vars.get("HOMEDRIVE")?;
            let p = vars.get("HOMEPATH")?;
            Some(format!("{d}{p}"))
        });
        if let Some(home) = home {
            vars.insert("HOME".to_string(), home);
        }
    }

    // sill: inside the shell, PATH is POSIX-style (see `child_env_value` for the way out).
    if let Some(path) = vars.get_mut("PATH") {
        *path = crate::sys::fs::posix_path_list(path);
    }

    // Copy TEMP/TMP to TMPDIR if TMPDIR doesn't already exist.
    if !vars.contains_key("TMPDIR")
        && let Some(tmp) = vars.get("TEMP").or_else(|| vars.get("TMP")).cloned()
    {
        vars.insert("TMPDIR".to_string(), tmp);
    }

    // sill: HOME and TMPDIR with `/` separators, as $PWD has them: `~/x` and `$TMPDIR/x`
    // came out as `C:\Users\me/x`, and `\` is the shell's escape character.
    for name in SLASHED_PATH_VARS {
        if let Some(value) = vars.get_mut(*name) {
            *value = crate::sys::fs::normalize_path_separators(value).into_owned();
        }
    }

    vars.into_iter()
}

/// sill: single-path variables the shell shows with `/` separators.
const SLASHED_PATH_VARS: &[&str] = &["HOME", "TMPDIR"];

fn is_drive_path(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

/// sill: converts a variable's value for a child process's environment. `PATH` is
/// POSIX-style inside the shell and goes to native programs in Windows form.
pub(crate) fn child_env_value(name: &str, value: String) -> String {
    if name == "PATH" {
        crate::sys::fs::windows_path_list(&value)
    } else if SLASHED_PATH_VARS.contains(&name) && is_drive_path(&value) {
        // As Git Bash passes them: `C:\Users\me`.
        value.replace('/', "\\")
    } else {
        value
    }
}

/// Normalizes the case of well-known environment variable names to their
/// canonical POSIX forms (`Path` → `PATH`, `home` → `HOME`).
///
/// # Arguments
///
/// * `name` - The environment variable name to normalize.
fn normalize_env_name(name: &str) -> String {
    // Normalize well-known variable names so that later lookups by
    // canonical (uppercase) spelling always succeed regardless of the
    // host's original casing.
    const WELL_KNOWN: &[&str] = &[
        "PATH",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "TEMP",
        "TMP",
        "TMPDIR",
        // sill: the rest of Cygwin's list (winsup/cygwin/environ.cc `renv_arr`), so
        // `$COMSPEC`, `$SYSTEMROOT`, `$WINDIR`, ... exist as in Git Bash, the spelling
        // shell scripts use. Children are unaffected: Windows looks names
        // up case-insensitively.
        "COMMONPROGRAMFILES",
        "COMSPEC",
        "PROGRAMFILES",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "WINDIR",
    ];

    for &canonical in WELL_KNOWN {
        if name.eq_ignore_ascii_case(canonical) {
            return canonical.to_string();
        }
    }

    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &[(&str, &str)]) -> BTreeMap<String, String> {
        collect_host_env_vars(
            source
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
        )
        .collect()
    }

    #[test]
    fn normalize_env_name_canonicalizes_well_known() {
        assert_eq!(normalize_env_name("Path"), "PATH");
        assert_eq!(normalize_env_name("path"), "PATH");
        assert_eq!(normalize_env_name("PATH"), "PATH");
        assert_eq!(normalize_env_name("Home"), "HOME");
        assert_eq!(normalize_env_name("UserProfile"), "USERPROFILE");
        assert_eq!(normalize_env_name("Temp"), "TEMP");
        assert_eq!(normalize_env_name("Tmp"), "TMP");
        assert_eq!(normalize_env_name("TmpDir"), "TMPDIR");
    }

    #[test]
    fn normalize_env_name_matches_cygwin() {
        for (windows, cygwin) in [
            ("CommonProgramFiles", "COMMONPROGRAMFILES"),
            ("ComSpec", "COMSPEC"),
            ("ProgramFiles", "PROGRAMFILES"),
            ("SystemDrive", "SYSTEMDRIVE"),
            ("SystemRoot", "SYSTEMROOT"),
            ("windir", "WINDIR"),
        ] {
            assert_eq!(normalize_env_name(windows), cygwin);
        }
        // Not on Cygwin's list: kept as Windows spells them.
        for name in ["ProgramData", "ProgramFiles(x86)", "CommonProgramFiles(x86)", "ProgramW6432"] {
            assert_eq!(normalize_env_name(name), name);
        }
    }

    #[test]
    fn normalize_env_name_leaves_unknown_alone() {
        assert_eq!(normalize_env_name("FOO"), "FOO");
        assert_eq!(normalize_env_name("myVar"), "myVar");
        // Does not uppercase unknown names.
        assert_eq!(normalize_env_name("AppData"), "AppData");
    }

    #[test]
    fn synthesizes_home_from_userprofile() {
        let vars = run(&[("UserProfile", r"C:\Users\reuben")]);
        // sill: with `/` separators.
        assert_eq!(vars.get("HOME").map(String::as_str), Some("C:/Users/reuben"));
        assert_eq!(
            vars.get("USERPROFILE").map(String::as_str),
            Some(r"C:\Users\reuben")
        );
    }

    #[test]
    fn synthesizes_home_from_homedrive_homepath_when_no_userprofile() {
        let vars = run(&[("HomeDrive", "C:"), ("HomePath", r"\Users\reuben")]);
        assert_eq!(vars.get("HOME").map(String::as_str), Some("C:/Users/reuben"));
    }

    #[test]
    fn preserves_existing_home() {
        let vars = run(&[("HOME", "/already/set"), ("UserProfile", r"C:\Users\other")]);
        assert_eq!(vars.get("HOME").map(String::as_str), Some("/already/set"));
    }

    #[test]
    fn copies_temp_to_tmpdir() {
        let vars = run(&[("Temp", r"C:\Windows\Temp")]);
        assert_eq!(vars.get("TMPDIR").map(String::as_str), Some("C:/Windows/Temp"));
        // TEMP itself stays as it was.
        assert_eq!(vars.get("TEMP").map(String::as_str), Some(r"C:\Windows\Temp"));
    }

    #[test]
    fn slashed_vars_go_to_children_in_windows_form() {
        assert_eq!(child_env_value("HOME", "C:/Users/me".into()), r"C:\Users\me");
        assert_eq!(child_env_value("TMPDIR", "/tmp".into()), "/tmp");
        assert_eq!(child_env_value("OTHER", "C:/x".into()), "C:/x");
    }

    #[test]
    fn prefers_temp_over_tmp_for_tmpdir() {
        let vars = run(&[("TEMP", "one"), ("TMP", "two")]);
        assert_eq!(vars.get("TMPDIR").map(String::as_str), Some("one"));
    }

    #[test]
    fn falls_back_to_tmp_when_no_temp() {
        let vars = run(&[("TMP", "two")]);
        assert_eq!(vars.get("TMPDIR").map(String::as_str), Some("two"));
    }

    #[test]
    fn preserves_existing_tmpdir() {
        let vars = run(&[("TMPDIR", "original"), ("TEMP", "other")]);
        assert_eq!(vars.get("TMPDIR").map(String::as_str), Some("original"));
    }

    #[test]
    fn deterministic_iteration_order() {
        // BTreeMap iteration order is determined by the sorted key order,
        // so the outputs must be identical across invocations regardless of
        // input ordering.
        let a = run(&[("Path", "first"), ("ZETA", "zz"), ("Alpha", "aa")]);
        let b = run(&[("ZETA", "zz"), ("Alpha", "aa"), ("Path", "first")]);
        let keys_a: Vec<_> = a.keys().collect();
        let keys_b: Vec<_> = b.keys().collect();
        assert_eq!(keys_a, keys_b);
    }

    #[test]
    fn collision_last_write_wins() {
        // When two source names normalize to the same canonical key,
        // the later one should overwrite the earlier one (and it should not
        // panic).
        let vars = run(&[("Path", "first"), ("PATH", "second")]);
        assert_eq!(vars.get("PATH").map(String::as_str), Some("second"));
    }
}
