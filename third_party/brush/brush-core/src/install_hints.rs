//! sill: how to install the programs agents reach for first, for the messages that say
//! one isn't there ("command not found", a `#!` interpreter, the MCP server's tool list).
//! `git status` on a machine without Git said only "git: command not found".

/// Programs by the names agents run them, with the winget package that installs them.
const PACKAGES: &[(&[&str], &str, &str)] = &[
    (&["git"], "Git", "Git.Git"),
    (&["node", "npm", "npx"], "Node.js", "OpenJS.NodeJS.LTS"),
    (&["python", "python3", "pip", "pip3", "py"], "Python", "Python.Python.3.13"),
    (&["cargo", "rustc", "rustup"], "Rust", "Rustlang.Rustup"),
    (&["go", "gofmt"], "Go", "GoLang.Go"),
    (&["java", "javac"], "Java", "Microsoft.OpenJDK.21"),
    (&["dotnet"], "the .NET SDK", "Microsoft.DotNet.SDK.8"),
    (&["docker"], "Docker", "Docker.DockerDesktop"),
    (&["uv"], "uv", "astral-sh.uv"),
];

/// "Git isn't installed here; `winget install -e --id Git.Git` installs it", for a
/// program in the table.
pub fn install_hint(name: &str) -> Option<String> {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    PACKAGES.iter().find(|(names, _, _)| names.contains(&name)).map(|(_, what, id)| {
        format!("{what} isn't installed here; `winget install -e --id {id}` installs it")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_programs_have_hints() {
        assert!(super::install_hint("git").unwrap().contains("Git.Git"));
        assert!(super::install_hint("npx").unwrap().contains("OpenJS.NodeJS.LTS"));
        assert!(super::install_hint("python3.exe").unwrap().contains("Python"));
        assert!(super::install_hint("ls").is_none());
    }
}
