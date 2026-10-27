# Changelog

## Unreleased

### Process lifetimes (Windows Job Objects)
- **Every program sill starts joins a Job Object before it runs.** A process tree followed through parent PIDs loses processes whose parent has exited, and so does `taskkill /T`, how harnesses stop commands. A job doesn't.
  - **`kill $!`** ends everything a background job started, including what a wrapper that already exited left behind (`npx`, `cmd /c start`): no more dev server holding its port.
  - **Killed mid-command** (a harness's timeout), sill takes everything the command started with it.
  - **A command that finishes normally** leaves what it deliberately started running (`start`, an editor, a build daemon), as from a terminal.
- **MCP server:** commands join their job before starting (no window for a grandchild to escape). Programs a command leaves running become a background job of the session, reported in the result, and end with the session. The first command runs in the client's project root, or the home folder rather than a client's install folder.
- **`kill`** takes every signal whose default is to end a process (`USR1`, `ALRM`, ...; it ends the process tree, with 128 + the number), refuses the ones that would stop, continue or be ignored rather than end it, and `kill -l` lists or translates them as bash does (`kill -l 143` is `TERM`). An invalid signal is status 1, as in bash.
- **MCP output in the system's code page:** a line that isn't UTF-8 (Java since JEP 400 writes GBK into a pipe on Chinese Windows) is read in the ANSI code page when that reads it cleanly.

### Checks
- **The build runs the evidence:** the 56 conformance cases (directly and through MCP; all must pass), brush's compat suite on Windows against Git Bash (no failure beyond a committed baseline), and on Linux against bash.
- **Builtins' diagnostics** are written a line at a time, not a byte at a time.

### Docs
- **`/tmp` semantics** are in Known limits: one folder per drive, created at the drive's root with its permissions.

## 0.4.0 (2026-10-24)

### Installing
- **Started through a link** (as winget's Links folder does), sill still finds its bundled tools.

### MCP server
- **`install-windows.ps1 -Mcp claude`** adds the server to Claude Desktop.
- The conformance suite also runs through the MCP server (56/56).

### Bash behavior
- **Namerefs** (`declare -n`, `local -n`):
  - reads, writes, appends, unset and `test -v` go through the reference, including chains (up to 8, as in bash) and references to array elements (`arr[1]`);
  - attributes (`declare -i/-x/-r`, `export`) apply to the target;
  - `${!ref}` gives the name it refers to;
  - `for ref in a b c` retargets the reference;
  - `unset -n` removes the reference itself;
  - bash's checks: self-references, invalid target names, circular chains.
  The typical use, returning an array from a function (`f() { local -n out=$1; out=(...); }`), works.
- **Integer variables** (`declare -i`) evaluate what's assigned arithmetically: `x=20+5`, `x+=3*2`.
- **`printf`:**
  - `%b` takes a width and precision, and keeps `\"` as bash does;
  - `%(datefmt)T` formats times;
  - a character argument (`'A`) is its code;
  - an invalid number (`printf %d abc`) prints `printf: abc: invalid number` on the command's own stderr (redirections apply) and fails with status 1.

### Messages
- **Diagnostics in bash's shape, with the line:** `build.sh: line 3: cd: /nope: No such file or directory`, `bash: line 1: printf: abc: invalid number`. This covers the shell's own errors and the builtins' messages.
- **Expansion and arithmetic errors name what failed:** `u: parameter null or not set`, `5 % 0: division by 0`, `2 +* 3: arithmetic syntax error`.

## 0.3.0 (2026-10-23)

### MCP server
- **`bash.exe --mcp`** runs sill as an MCP server (stdio), for harnesses whose own shell is PowerShell (Codex CLI, Gemini CLI) and clients with none. Tools:
  - `bash`: runs a command. The working directory and exported variables carry over to the next call. A timeout (default 2 minutes, at most 10) ends the command and everything it started.
  - `bash_output` and `bash_kill`: read and stop background jobs (`run_in_background`).
- **`install-windows.ps1 -Mcp codex,gemini`** registers it in each client's config.

### Running things
- **`apply_patch`**, the file-editing command Codex's models use, is a builtin. It takes the patch from stdin or as an argument, and keeps each file's line endings.

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
