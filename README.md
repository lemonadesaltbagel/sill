# sill

**Bash for coding agents on native Windows.** sill is a drop-in `bash.exe`: a Bash interpreter (a fork of [brush](https://github.com/reubeno/brush)) with a Windows adapter built for the way agents run commands. It needs no Git, MSYS2 or WSL. Point Claude Code, opencode or Codex at it, or add it to Codex CLI or Gemini CLI as an MCP server, and the agent's shell commands behave the way the model expects from Linux, on Windows' own programs and files.

## Why

An agent on Windows loses most of its failed commands at the boundary between the shell and Windows, not in its own reasoning:

- **Arguments:** `*` and quotes get re-expanded or mangled on their way to `.exe` and `.bat` programs.
- **Paths:** `/c/...` and `C:\...` forms get mixed up.
- **Encoding:** non-English text turns to mojibake.
- **Console windows:** one flashes up per command.
- **Background servers:** they hold the harness's output pipe until it times out.
- **Missing tools:** `make`, `rg`, `ps`, `lsof` aren't there.

sill takes each of these on directly, with a test for each.

## Results

All measured on Windows 11 with the Git Bash that ships with Git for Windows as the comparison.

**Conformance suite** ([sill-bash-tests](https://github.com/lemonadesaltbagel/sill-bash-tests)): 56 cases, run through `bash.exe -c` and through the MCP server (56/56 both ways) covering what agents rely on, each with the reason it matters. Areas: arguments to native programs, paths, encoding, line endings, processes, size limits, `#!` scripts, tools, errors.

| | sill | Git Bash |
|---|---|---|
| GitHub's `windows-2025` runner (en-US) | **56/56** | 43/56 |
| A Chinese-locale laptop (code page 936) | **56/56** | 39/56 |

**Real agents, end to end:** 18 tasks with automatic checks, run with DeepSeek's low-cost model. The tasks include a Makefile project, an npm dev server started and stopped, Unicode paths, a `.bat` with `%` arguments, 4,000 files, and an MSVC build. Two runs per task on the Chinese-locale laptop:

| harness | sill | Git Bash | PowerShell |
|---|---|---|---|
| Claude Code | 34/36 passed, **1** failed call of 120, **493 s** | 35/36, 8 of 154, 637 s | 35/36, 9 of 141, 777 s |
| opencode | **35/36**, **3** of 104, **403 s** | 35/36, 18 of 149, 622 s | 29/36, 15 of 132, 904 s |
| Codex CLI | **36/36**, **6** of 136, **634 s** | 36/36, 22 of 172, 846 s | 35/36, 44 of 208, 1094 s |

The pass counts are close. The difference is in the failed calls and the time it took to get there.

**As an MCP server**, added to each harness as it comes (PowerShell remains its own shell), on the same tasks:

| harness | with sill's MCP server | PowerShell only |
|---|---|---|
| Codex CLI | **36/36** passed, **10** failed calls of 119, **501 s** | 35/36, 44 of 208, 1094 s |
| Gemini CLI | **36/36**, 5 of 90, **335 s** | 31/36, 5 of 150, 1195 s |

Given the choice, the models moved their shell work to sill: all of it under Gemini CLI, most of it under Codex.

**Bash itself:** brush's compatibility suite, with Git Bash as the reference, passes 2,434 of 2,932 runnable cases on Windows (83%). The rest are mostly:
- Unix signals and job control, which Windows has no equivalent for;
- `coproc`;
- debugging options (`extdebug`, `functrace`) and rarely used `declare` forms;
- differences in Git Bash itself, such as its `/usr/bin` paths.

## Install

With [Scoop](https://scoop.sh):

    scoop bucket add sill https://github.com/lemonadesaltbagel/scoop-sill
    scoop install sill/sill

This puts `sill` on PATH. The drop-in `bash.exe` is `~\scoop\apps\sill\current\bash.exe`; see [With an agent harness](#with-an-agent-harness).

Or with the installer script:

1. Download `sill-windows-x64.zip` and `install-windows.ps1` from the [latest release](../../releases/latest).
2. In PowerShell, run:

       powershell -ExecutionPolicy Bypass -File install-windows.ps1 -Zip sill-windows-x64.zip -AddToPath -ClaudeCode

The installer unpacks to `%LOCALAPPDATA%\Programs\sill`. The options:
- `-AddToPath` puts sill on your user PATH.
- `-ClaudeCode` points Claude Code's Bash tool at sill, in `~\.claude\settings.json`. Your other settings are kept, and the old file is saved as `settings.json.bak`.
- `-Mcp codex,gemini,claude` adds sill's MCP server to any of these:
  - Codex CLI (`~\.codex\config.toml`);
  - Gemini CLI (`~\.gemini\settings.json`);
  - Claude Desktop (`%APPDATA%\Claude\claude_desktop_config.json`).

  Each is merged with what's there, and the old file is saved as `.bak`. Approvals stay as you've set them in the client.

The installer changes nothing else. sill itself creates one folder: `\tmp` on the current drive, for `/tmp` (see [Known limits](#known-limits)).

### With an agent harness

| harness | how |
|---|---|
| Claude Code | `CLAUDE_CODE_GIT_BASH_PATH` = `...\sill\bash.exe` (what `-ClaudeCode` sets) |
| opencode | `SHELL` = `...\sill\bash.exe` |
| Codex CLI, Gemini CLI, Claude Desktop | As an MCP server: `-Mcp codex,gemini,claude` |
| any MCP client | Command `...\sill\bash.exe`, arguments `--mcp` (stdio). Allow tool calls up to 11 minutes |

### The MCP server

`bash.exe --mcp` gives the model three tools:
- `bash` runs a command and returns its stdout, stderr and exit code. The working directory and exported variables carry over to the next call; functions and aliases don't. A timeout (default 2 minutes, at most 10) ends the command and every program it started. With `run_in_background`, it starts a server or watcher and returns a job id at once.
- `bash_output` reads a background job's new output.
- `bash_kill` stops a background job and everything it started.

Each command runs in a fresh sill process, inside a Job Object it joins before it starts. So a crash or a hang can't take the server down, and a timeout or `bash_kill` ends every program the command started. Programs a command leaves running become a background job: the result says so, `bash_output` and `bash_kill` work on them, and they end with the session.

The first command runs in the project the client names (its MCP roots), if it offers one. Otherwise it runs where the client started the server, or in your home folder when that's the client's own install folder (as Claude Desktop does). `SILL_MCP_ROOT` sets it explicitly.

## What's inside

**Running programs**
- **Exact arguments:** command lines are encoded for each target, using the C runtime's rules for `.exe` and cmd.exe's rules for `.bat`/`.cmd`. `*` and `?` arrive as typed.
- **Long argument lists:** sill's own tools and the bundled GNU tools accept any number of arguments, past Windows' 32,767-character limit, through an args file. Other programs that hit the limit fail with "Argument list too long" and a note on what to use instead.
- **`#!` scripts** run through their interpreter: `./build.sh`, `./gradlew`, `#!/usr/bin/env python3`. `sh` is sill.
- **`cmd /c '... >nul'`:** Claude Code rewrites `>nul` to `>/dev/null`; sill reads that back as `nul`.
- **`exec`** runs the program and exits with its status, so wrapper scripts work.
- **`apply_patch`**, the file-editing command Codex's models are trained on, takes its patch from stdin or as an argument, and keeps each file's line endings.
- **Background jobs** (`server > log 2>&1 &`) don't hold the harness's output pipe. `kill`, `pkill` and `killall` end whole process trees, but never the harness or the Windows session's processes.

**Paths and text**
- **Paths:** `$PWD`, `~`, `$HOME` and `$TMPDIR` use `C:/...` form, and `/c/...` and `/tmp` work wherever the shell opens a file. Arguments to other programs are never rewritten.
- **No console windows:** a shell started without a console, as Claude Code starts it, gets one invisible UTF-8 console. No window opens per command, and Windows' messages arrive readable.
- **Error messages:** system errors use POSIX wording ("No such file or directory"), not Windows' sentence in the user's language.
- **git** prints non-ASCII paths unescaped.

**Bash behavior**
- `trap ... EXIT INT TERM`, process substitution (`diff <(a) <(b)`), `read -t`, `ulimit`, `$OSTYPE=msys`.
- `$EUID` is a regular user's, even when elevated, as in Git Bash.

**Package managers:** npm, npx, yarn and pnpm run `package.json` scripts in sill, unless you've configured a script shell. Scripts that look written for cmd.exe get a one-line note.

**Visual C++:** `msvc-env [arch]` loads Visual Studio's C/C++ environment into the shell, cached: about 55 ms against 2 s for `vcvars64.bat`.

**Bundled tools (all native Windows programs):**
- **Built into sill:** the coreutils (uutils) and `find` (uutils findutils, printing `/` paths, also for `-exec`).
- **GNU builds:** grep, sed, diffutils and make, with a UTF-8 code page, so any file name works.
- **Others:** ripgrep, jq, and busybox-w32 for awk, xargs, patch, gzip and the rest.
- **For Windows:** `ps`, `pgrep`, `pkill`, `killall`, `kill`, `lsof` (files and ports), `file`, `cygpath`, `nohup` and `setsid`.

**Defaults for agents:**
- UTF-8 Python.
- No pagers, prompts or update notices.
- Telemetry off.
- English .NET and MSBuild messages.

A value you've already set is kept. All of these are visible with `env`.

### Settings

| variable | effect |
|---|---|
| `SILL_NO_ENV_DEFAULTS=1` | Turns the defaults above off, and English for sill's own messages too |
| `LANG`, `LC_ALL`, `LC_MESSAGES` | A language other than English keeps sill's own messages in it |
| `SILL_CONSOLE_CP` | The code page of the invisible console (default 65001, UTF-8) |
| `SILL_TRACE=<file>` | Appends each invocation's arguments and working directory to the file, one JSON line each |

## Known limits

- **`/tmp` is `\tmp` on the current drive**, as Windows and Claude Code's own file tools read `/tmp/x`. sill creates the folder when it's missing. Mind three things:
  - It's a different folder on each drive: what was written to `/tmp` while working on `D:` isn't there from `C:`.
  - It's created at the drive's root and inherits that folder's permissions: on a machine several people use, the others can typically change files in it, as in a `/tmp` without the sticky bit.
  - Use `$TMPDIR` (your user's temporary folder) for anything private.
- **Windows has no signals.** Traps for INT and TERM are accepted and listed, but only `EXIT`, `ERR`, `DEBUG` and `RETURN` run. `kill` ends processes.
- **Unsupported:** `coproc`, stopped jobs (`fg` after Ctrl+Z), passing descriptors above 2 to a program, `exec -a`, and `cp <(cmd) file`.
- **Programs a finished command leaves running keep running** (`start`, an editor, a build daemon), as from a terminal. A command killed before it finishes, by a harness's timeout for one, takes everything it started with it. Through the MCP server, such leftovers become a background job of the session and end with it.
- **Process substitution** passes a named pipe (`\\.\pipe\...`), which programs open like a file.
- **Elevation:** when sill runs elevated, network drives mapped in your normal session aren't visible to it, as for any elevated program.
- **No `sudo`, `apt` or `systemctl`:** sill doesn't emulate Linux administration. These commands fail with a note naming the Windows way.

## Build from source

You need a Windows machine with Rust (the version in `rust-toolchain.toml`) and Visual Studio's C++ tools:

    cargo build --release -p sill

This gives `target\release\bash.exe`. The bundled tools are cross-built on Linux with mingw-w64:
- `scripts/build-busybox.sh`
- `scripts/build-gnu-tools.sh`
- `scripts/fetch-tools.sh` (pinned and checksummed)

`scripts/package-windows.ps1` assembles the zip, and `scripts/dist-check.sh` checks it on a PATH with only Windows' own directories. `.github/workflows/build.yml` runs all of this, and publishes a release for a `v*` tag.

## Layout

| path | what |
|---|---|
| `crates/sill` | The app: `bash.exe` and `sill.exe`, the console and environment setup, the MCP server (`src/mcp.rs`), and the tests that replay Claude Code's shell contract |
| `crates/sill-winargs` | argv to Windows command line, checked against `spec/argv-test-vectors.json` |
| `crates/sill-errno` | POSIX wording for Windows error codes |
| `third_party/brush` | The Bash interpreter, vendored; sill's changes are marked `sill:` |
| `third_party/findutils`, `third_party/uucore` | uutils crates, vendored for small changes, marked `sill:` |
| `third_party/gnu-tools`, `third_party/busybox-w32` | What the GNU and busybox builds add: a UTF-8 manifest, the args-file shim, a sed patch |
| `scripts/` | Building the tools, packaging, installing, and checking the distribution |

See [CHANGELOG.md](CHANGELOG.md) for what each release changed.
