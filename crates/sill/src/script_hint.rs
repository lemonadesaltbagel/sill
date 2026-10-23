//! When sill runs a `package.json` script (as npm's, yarn's or pnpm's script shell) that
//! looks written for cmd.exe, it says so on stderr, then runs the script as written.
//!
//! sill is the default script shell on Windows (brush-core `sys::script_shell`). The few
//! scripts written for cmd.exe mostly fail loudly under bash, but two forms do something
//! else without an error: `set NAME=value&& cmd` sets bash's positional parameters, not
//! NAME, and `%NAME%` stays literal. A `.\x` path loses its backslashes. Guessing the
//! language and running cmd.exe instead would make the same line mean different things
//! depending on a heuristic; a one-line note keeps the behaviour fixed and visible.

/// The note for this process, if it runs a package.json script that looks written for
/// cmd.exe. `args` are the process's arguments (without argv[0]); `var` reads the
/// environment.
pub(crate) fn hint(args: &[String], var: impl Fn(&str) -> Option<String>) -> Option<String> {
    // Package managers start the script shell as `<shell> -c <script>` with
    // `npm_lifecycle_script` set. The variable is inherited by everything a script starts,
    // so the `-c` text must contain it: then this process is the script's own shell.
    let event = var("npm_lifecycle_event")?;
    let script = var("npm_lifecycle_script")?;
    let pos = args.iter().position(|a| a == "-c")?;
    if script.trim().is_empty() || !args.get(pos + 1)?.contains(script.as_str()) {
        return None;
    }
    let found = cmd_construct(&script, &var)?;
    Some(format!(
        "sill: the \"{event}\" script looks written for cmd.exe ({found}); it runs as bash \
         here, where that means something else. To run it with cmd.exe: \
         npm run {event} --script-shell cmd.exe"
    ))
}

/// The first cmd.exe-only construct in `script` that bash would take differently.
fn cmd_construct(script: &str, var: &impl Fn(&str) -> Option<String>) -> Option<String> {
    for segment in script.split(['&', '|', ';', '(']) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        match words.as_slice() {
            // `set NAME=value`: in bash, positional parameters.
            [set, assignment, ..]
                if set.eq_ignore_ascii_case("set")
                    && assignment.split_once('=').is_some_and(|(n, _)| is_name(n)) =>
            {
                return Some(format!("`set {assignment}`"));
            }
            // `.\bin\tool`: in bash, `\b` is `b`.
            [first, ..] if first.starts_with(".\\") || first.starts_with("..\\") => {
                return Some(format!("`{first}`"));
            }
            _ => {}
        }
    }
    // `%NAME%` for a variable that exists (so `date +%Y%m%d` isn't one).
    let mut rest = script;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(len) = after.find('%') else { break };
        let name = &after[..len];
        if is_name(name) && var(name).is_some() {
            return Some(format!("`%{name}%`"));
        }
        rest = after;
    }
    None
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(script: &str, c_arg: &str, extra: &[(&str, &str)]) -> Option<String> {
        let mut vars = vec![
            ("npm_lifecycle_event".to_owned(), "build".to_owned()),
            ("npm_lifecycle_script".to_owned(), script.to_owned()),
            ("OS".to_owned(), "Windows_NT".to_owned()),
        ];
        vars.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        let args = vec!["-c".to_owned(), c_arg.to_owned()];
        hint(&args, |name| vars.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()))
    }

    fn check(script: &str) -> Option<String> {
        run(script, script, &[])
    }

    #[test]
    fn cmd_constructs() {
        let h = check("set NODE_ENV=umd&& rollup -c").unwrap();
        assert!(h.contains("`set NODE_ENV=umd`") && h.contains("npm run build --script-shell cmd.exe"), "{h}");
        assert!(check("tsc && SET X=1 && node a.js").is_some());
        assert!(check(r"node .\scripts\hello.js").is_none(), "only a leading path is a command");
        assert!(check(r".\node_modules\.bin\electron js\electron.js").unwrap().contains(r"`.\node_modules\.bin\electron`"));
        assert!(check(r"cd android && ..\gradlew build").is_some());
        assert!(check("echo %OS%").unwrap().contains("`%OS%`"));
    }

    #[test]
    fn bash_scripts_get_no_note() {
        for s in [
            "rm -rf dist && mkdir -p dist",
            "NODE_ENV=production node build.js",
            "set -e; node a.js",
            "set -- a b",
            "date +%Y%m%d",
            "echo %NOT_A_SET_VAR%",
            "node -e \"console.log(1)\" && tsc",
        ] {
            assert_eq!(check(s), None, "{s}");
        }
    }

    #[test]
    fn only_the_scripts_own_shell() {
        // npm appends `-- args`; pnpm may add a prefix; both still contain the script.
        assert!(run("set X=1&& a", "trap 'exit' INT; set X=1&& a 'x'", &[]).is_some());
        // A shell the script started (the variable is inherited) runs other text.
        assert_eq!(run("set X=1&& a", "echo hi", &[]), None);
        let args = vec!["script.sh".to_owned()];
        assert_eq!(hint(&args, |_| Some("set X=1&& a".to_owned())), None);
    }
}
