# Changelog

## 0.2.0 (2026-10-23)

The first release published in this repository. `bash --version` now names sill (`sill 0.2.0 (brush 0.4.0, git:...)`).

### Bash behavior
- **`trap` takes signal names:** `trap cleanup EXIT INT TERM` failed as a whole on Windows ("INT: invalid signal specification"), so not even the EXIT cleanup ran. Signals now have their Linux names and numbers, `trap -p` and `trap -l` show them, and an invalid name no longer drops the valid ones.
- **Process substitution:** `cat <(cmd)` and other programs that look at a path before opening it failed with "All pipe instances are busy". (`cp <(cmd) file` is still a known limit: Windows' copy opens the source twice.)
- **`exec cmd`** runs the program and exits with its status, so wrappers that end with `exec "$JAVACMD" "$@"` (`gradlew`, `mvnw`) work.
- **`ulimit`** exists (it was "command not found", which stopped `set -e` scripts). It reports Git Bash's values and accepts limits within the hard one; Windows doesn't enforce them.
- **`read -t`** works on pipes, files and the console.
- **`source /dev/stdin`**, `[ -c /dev/null ]`.
- **`$UID` and `$EUID` stay a regular user's when elevated**, as in Git Bash. A root uid (brush's behavior) stopped scripts that refuse to run as root on the many machines where developers work as administrators.

### Paths and messages
- **`~`, `$HOME`, `$TMPDIR`, `~+` and `dirs`** use `/` separators inside the shell, as `$PWD` does. Programs get `HOME` and `TMPDIR` in Windows form.
- **System errors in POSIX wording:** "No such file or directory", "Permission denied", ..., from the shell and its built-in tools, instead of Windows' sentence in the user's language. sill's own messages are in English unless `LC_ALL`, `LC_MESSAGES` or `LANG` names another language.

## 0.1.0 (2026-10-23)

The first release of sill: a native Windows Bash runtime for coding agents, shipped as a drop-in `bash.exe`. Install with `scripts/install-windows.ps1 -Zip sill-windows-x64.zip [-AddToPath] [-ClaudeCode]`.

### For agent harnesses
- **Claude Code:** the `CLAUDE_CODE_GIT_BASH_PATH` contract (argv shapes, PATH probe, snapshot, cwd file). The harness rewrites `>nul` to `>/dev/null` even inside `cmd /c '...'`; sill reads that as `nul`.
- **opencode:** via `SHELL`.
- **Codex CLI:** when the model passes `shell: "bash"` (steered by AGENTS.md).
- **Shells started without a console** (as Claude Code starts them) get one invisible UTF-8 console. There is no window per program, and Windows' messages arrive as readable UTF-8, in English.
- **Background jobs** don't hold the harness's output pipe.
- **npm, npx, yarn and pnpm** run `package.json` scripts in sill unless a script shell is configured. Scripts that look written for cmd.exe get a one-line note.

### Running things
- **`#!` scripts run by their interpreter:** `./build.sh`, `./gradlew`, `#!/usr/bin/env python3`. `sh` is sill.
- **Command lines are encoded per target** (CRT rules, `.bat`/`cmd.exe` rules), and `*`/`?` arrive literally. Git Bash's `//c` idiom works.
- **Paths:** `/c/...` and `/tmp` work where the shell opens files, and `$PWD` is `C:/...`. Windows' variable names are uppercase as in Git Bash (`$COMSPEC`, ...).
- **`msvc-env`** loads Visual Studio's C/C++ environment, cached: about 55 ms against 2 s for vcvars.
- **Long argument lists:** sill's own tools and the GNU tools take any number of arguments, through an args file past Windows' 32K limit.

### Bundled tools
- **Built in:** uutils coreutils; uutils findutils `find` (`/` paths, including for `-exec`).
- **GNU builds:** grep, sed, diffutils and make, with a UTF-8 code page, so any file name works.
- **Other tools:** busybox-w32 (awk, xargs, ...), ripgrep, jq (LF output).
- **Windows builtins:** `ps`, `pgrep`, `pkill`, `killall`, `kill` (process trees), `lsof` (files and ports), `file`, `cygpath`, `nohup`, `setsid`.

### Honest failures
- **Hints:** Linux-only commands name the Windows way (`apt`, `sudo`, `systemctl`, `python3`, `perl`, `zip`, ...).
- **Startup failures say what happened:** no `#!` line, a bad interpreter, "Argument list too long", elevation required.
- **The Microsoft Store's stand-in `python3.exe`** is skipped.
- **Agent-friendly defaults:** UTF-8 Python, no pagers or prompts, telemetry off, and git paths unescaped. User values win, and `SILL_NO_ENV_DEFAULTS=1` turns them off.
