# Changelog

## Unreleased

### Fixes
- **`diff -r` on a Chinese-locale system printed "cannot compare file names ... Invalid argument"** for a folder holding a name the system code page can't hold (`文件 €.txt` in GBK): the GNU tools compared names in that code page though they're UTF-8. They compare in byte order now (UTF-8 code point order, as in the C locale), and make's case-insensitive name comparison likewise (`third_party/gnu-tools/collate.c`).

### Checks
- **Conformance:** the GNU tools case requires no error output (it passed while `diff` printed one).

## 0.6.3 (2026-11-04)

Fixes from an agent's run of 0.6.2 on a Chinese-locale Windows 11 (code page 936), with a space in the user name and no Git, Node or Python.

### Fixes
- **`make`, `grep -r` and `diff -r` stopped with "Invalid argument"** in a folder holding a file such as `文件 ü.txt`: in the tools' UTF-8 code page, Windows' ANSI directory listing fails on a file whose 8.3 short name (`文件Ü~1.TXT`) is longer than 14 bytes in UTF-8. The GNU tools list directories with the wide functions now (`third_party/gnu-tools/findfile.c`).
- **No install hint for a missing `git` or `node`** (Python had one): Git, Node.js (`node`, `npm`, `npx`), Python, Rust, Go, Java, .NET, Docker and uv say which `winget install` gets them, for a command and for a `#!` interpreter.
- **`uname -s` said `Windows_NT`**, which matched none of the `MINGW*|MSYS*` branches scripts use while `$OSTYPE` is `msys`: it's `MINGW64_NT-10.0-<build>`, and `uname -o` `Msys`, as in Git Bash.
- **`$TMPDIR` had 8.3 short names** (`C:/Users/JANEDO~1/...`) where `$HOME` didn't; it's spelled out, and so are the MCP server's paths.
- **Syntax errors** say what bash says (`unexpected EOF while looking for matching '"'`, `syntax error: unexpected end of file`), without `at 1,6 (detected near line 1 col 19)`.
- **`stat`** is bundled (busybox's): `stat -c %s FILE`, `stat -c %Y FILE`.
- **`file` on a code-page-936 machine** calls GBK text `GBK text (code page 936)`, not `ISO-8859 text` (likewise Big5, Shift_JIS, EUC-KR).
- **`ls -l`** showed `somebody somegroup` and a `total` in bytes: it shows your user name (and `None` for the group, as Git Bash) and 1 KiB blocks.

### MCP server
- **A syntax error's line** counted from the server's wrapper (`line 4` for a one-line command).
- **`bash_output` and `bash_kill`** take `job_id`, `bash_id` and `shell_id` for `id`.

### Docs
- **PowerShell-only harnesses:** use the MCP server or a script file, not `bash.exe -c '...'` with quotes (PowerShell 5.1 drops them). **Parallel MCP calls** run one at a time in no set order.

### Checks
- **Conformance:** GNU tools in a folder of such names, `uname`, `stat`, `$TMPDIR`, install hints, syntax-error wording (108 cases).

## 0.6.2 (2026-11-03)

Fixes from an agent's run of 0.6.1 (Claude in Claude Code's desktop app, English Windows 11, a space in the user name, no Git, Node or Python).

### Fixes
- **Traps in subshells and functions didn't run:** `( trap 'rm -rf "$tmp"' EXIT; ... )` left the folder behind, `$( trap ... EXIT; ... )` lost the trap's output, a function's own `trap ... RETURN` and `trap ... ERR` and a subshell's `trap ... ERR` never fired, and a sourced script's end didn't run the RETURN trap. They run as in bash; a subshell never runs its parent's EXIT trap. brush's compat suite: 10 trap cases pass that didn't.
- **A script with Windows (CRLF) line endings failed with errors that hid the cause** (`cd: /tmp: No such file or directory`, `pwd: command not found`, a lone "syntax error at end of input"). sill says up front that the file has CRLF line endings and how to convert it, and names in errors show the CR as bash shows it (`$'pwd\r': command not found`). A UTF-8 byte order mark (PowerShell 5.1 writes one) gets the same.
- **`set -u`** says `X: unbound variable`, bash's words, not `expanding unset variable: ${X}`.
- **`jobs -l`** and **`jobs %N`** were "not yet implemented".
- **Windows' own `sudo`, turned off,** still ran: the check compared `C:/WINDOWS/system32` with `C:\Windows\System32`.
- **`brush: error flushing stdout: The pipe is being closed. (os error 232)`** after `... | head`: a reader that went away isn't reported.
- **`ln -s` without the symbolic-link privilege** said only "Operation not permitted"; it says Windows creates symbolic links only with Developer Mode on or as Administrator.
- **`python3: command not found`** suggested `python` where there's no Python either; then it says Python isn't installed and how to install it.
- **`cygpath -w /tmp`** gave `\tmp`, with no drive; it gives `C:\tmp` (the current folder's drive).
- **`bash.exe --help`** began with brush's banner and never mentioned sill; it begins with what sill is, `--mcp` and the `SILL_*` settings.

### MCP server
- **The `bash` tool's description listed `apply_patch` among the tools "not installed".**
- **`timeout`** (the name in Claude Code's own Bash tool) was silently ignored, so `sleep 30` ran its 30 s; it's taken as `timeout_ms`. Other unknown arguments are an error naming them (`description` is accepted).
- **Syntax errors named the server's temporary script** (`bash: line 1: C:/.../5.cmd.sh: syntax error ...`); they read `bash: line 1: syntax error ...`.
- **Job ids** count from 1, apart from commands, and `"3"` is taken as well as `3`. A timeout says `stopped after 1.5 s`.

### Packaging
- **The zip holds the shell twice, not three times** (`bash.exe`, and `sh.exe` for make and `#!/bin/sh`): about 27 MB less. `install-windows.ps1` links `sill.exe` to `bash.exe`; Scoop and winget name `bash.exe` `sill`.

### Checks
- **Conformance:** CRLF and byte-order-mark scripts, a subshell's EXIT trap and a function's RETURN and ERR traps, `cygpath -w /tmp`, `set -u` (102 cases).

## 0.6.1 (2026-11-02)

Fixes from a run of 0.6.0 on a Chinese-locale Windows 11 (code page 936), with a space in the user name and no Git, Node or Python.

### Fixes
- **`$(...)` of output that isn't UTF-8 failed the whole command** ("stream did not contain valid UTF-8"): `x=$(cmd /c echo 中文)` on a Chinese system, `x=$(cat file.bin)`. Output is read as UTF-8, else (line by line) in the system's code page, else with invalid bytes replaced; `read` and `mapfile` read such lines in the system's code page too.
- **`pkill PING` ended the user's SnippingTool:** patterns matched case-insensitively. They match as procps' do (case-sensitively; `-i` for not), a whole name in any case (`pkill ping` still ends `PING.EXE`). A process with a window open isn't ended by a pattern that only matches part of its name (its exact name or its PID does), and each process a pattern ends is named on stderr. `pkill -e` echoes as procps' does.
- **make's recipes all failed when sill was installed in a folder with a space** (`C:\Users\Jane Doe\scoop\...`): make split the shell's path at the space ("CreateProcess(C:/Users/Jane, ...) failed", e=193). The bundled make keeps it one word.
- **Console programs' text came out in the local code page (GBK) under harnesses that start sill with a hidden console** (`CREATE_NO_WINDOW`, Node's `windowsHide`): sill only set UTF-8 on a console it created. A windowless console sill was started with alone is its own and is set to UTF-8 (or `SILL_CONSOLE_CP`) too; a terminal's never is.
- **`cmd /c build.bat` said "'build.bat' is not recognized"** when the harness set `NoDefaultCurrentDirectoryInExePath` (Claude Code does). A bare `NAME.bat`/`NAME.cmd` that's in the current folder is passed as `.\NAME.bat`; names without an extension still go through the PATH.
- **`disown`** said "not implemented" (status 99). It removes jobs from the table (`-a`, `-r`, `%N`, a PID); a background job already outlives the shell.
- **A background job left running at exit with the shell's output as its own** (`bash -c 'server &'`) kept the caller waiting for the output to close, with no sign why (bash does the same). sill says so on stderr, with the redirection that avoids it.
- **`cmd | less` without Git** said "less: command not found" (Windows has no `less`): into a pipe or file a missing `more`/`less` runs as `cat`, as an installed one already did.
- **`$BASH`** was `$0` (`bash` under the MCP server); it's the shell's full path, as in bash.
- **`which python`** named the Store's stand-in `python.exe` while `python` said "command not found". `which` is a builtin on Windows, from the shell's own lookup.
- **`seq ... | head`** printed "write error: Broken pipe": Windows has no SIGPIPE to end the writer quietly. The bundled tools don't print it.
- **Windows' own `sudo`, turned off,** printed an escape sequence and a pointer to Settings; sill says sudo is off and elevation needs a person.
- **`time` showed 0 for user and sys:** they come from the programs' Job Objects now.

### MCP server
- **A workspace in Local AppData** (as Claude Desktop keeps one) was taken for the client's install folder, and commands silently started in the home folder. Only the folder of the program that started the server (and a drive root or Windows' folders) counts now, and the first result says when commands start elsewhere and why.
- **The `bash` tool's description lists the tools this machine has** (git, node, python, ...) and those it doesn't, instead of a fixed list that named git, npm and python on machines without them.
- **Errors read `bash: line 1: ...`**, not with the path of the server's temporary script.

### Checks
- **Conformance runs sill from a folder with a space in its name, a second time with a windowless console, and in the Chinese-locale container**; new cases for bytes in `$(...)` and `read`, `cmd /c NAME.bat`, `disown`, `pkill` beside an app whose name contains the pattern, make recipes, `which`, `seq | head` and `time`.

### Docs
- **Results:** Windows-boundary tasks (`winbench`), on GitHub's runner and on a Chinese-locale Windows: every shell solves them; sill takes the fewest turns.

## 0.6.0 (2026-11-02)

### Docs
- **Results** lead with where Windows gets in the way (conformance 85/85 against Git Bash's 64; fewer failed commands for real agents; the MCP server for PowerShell harnesses) and show that on general coding benchmarks (Terminal-Bench 2.1, SWE-bench Verified) sill solves as many tasks as Git Bash and PowerShell, with fewer failed shell calls.

### Fixes
- **A command given as a path that doesn't exist** (`./venv/bin/python`) said "command not found"; it says "No such file or directory", as bash does.
- **`python /d/work/solve.py`** opened `D:\\d\\work\\solve.py`: a `/c/...` path reached native programs as written. Now a whole argument in that form that names something existing is passed as its Windows path, as Git Bash would; switches (`/c`, `/s`), a bare drive, paths that don't exist and `--opt=/c/...` are left alone (look-alike cases check each).
- **`chmod` was missing** (`chmod +x build.sh && ./build.sh` failed with "command not found" without Git's tools on PATH, and Git's MSYS `chmod` reads `/tmp` as its own folder). It's a builtin now: a mode with the owner's write permission makes a file writable, one without makes it read-only; execute bits succeed and change nothing (Windows runs programs by extension; sill runs `#!` scripts as they are).
- **`$(...)` of a native program's output kept a CR** (`x=$(python -c 'print(42)')` was `42\r`): trailing CRLF line endings are trimmed as trailing newlines are, as in Git Bash. Found by Terminal-Bench (a path from `$(python ...)` didn't exist).
- **A loop reading a process substitution inside `$(...)` hung** (`x=$(while read -r l; do ...; done < <(find ...); echo $n)`): the producer was queued behind the blocked reader on the same runtime thread. It runs on a thread of its own now. Found by running Terminal-Bench's reference solutions; likely also the compat suite's rare `redirection.yaml` hang.
- **`sed '/c/d'`, `awk '/c/ {n++}'` and `grep '/c/'`** reached the tool as `C:/d`, `C:/ {n++}` and `C:/`: arguments to sill's bundled tools that look like `/c/...` paths were rewritten whenever `C:\` existed. Scripts and patterns (the first operand of grep, sed and awk, `-e` values, `find -name`/`-path` patterns) are no longer rewritten, and a path is rewritten only when it exists, or its parent does and isn't just the drive.
- **`find -path` and `-regex` matched nothing with a `/` in the pattern** (`find . -path '*/src/*'`): they were matched against Windows' `.\\src\\a.txt`, though find prints `./src/a.txt`. They're matched against what it prints. Found by the new look-alike cases.
- **`more` into a pipe** paged (the bundled `more`); Windows' own `more`, which sill runs as `cat` there, is used instead.
- **MCP server: a command's own `trap ... EXIT` lost the session's state** (its `cd` and exports), silently: the server recorded them in an EXIT trap of its own, which the command's replaced. The shell now records them itself as it ends, after the command's trap.

### MCP server
- **Functions, aliases and options carry over** between calls (`deactivate` after activating a virtual environment, `set -o pipefail`), not only the directory and exported variables. `set -e` doesn't.
- **One process fewer per call:** the state is saved by the command's shell, not by a second sill.

### Programs that wait for a person
- **`git commit` without `-m` stops at once** instead of waiting for an editor until the harness's timeout: when sill has no terminal and nothing names an editor for git, git's editor is sill, which says why and leaves the message as it is. `git commit --amend` and `git rebase --continue` keep theirs.
- **`vi notes.txt`** (and vim, nano, emacs, ...) with no terminal says there's none and fails, instead of waiting.
- **`more` and `less` writing to a pipe or file** are `cat` (Windows' `more` waited for a key).

### Process start
- **Starting a program resumes it with one call** (`NtResumeProcess`) instead of a snapshot of every thread on the machine: about 17 ms per program on GitHub's runner, from 30 (more where many threads run, as on a developer's machine).

### Checks
- **Benchmarks on native Windows** (workflows in sill-bash-tests): Terminal-Bench 2.1 and SWE-bench Verified, limited to the tasks that work on Windows (each task's reference solution passes there), and Windows-boundary tasks; each run records why it ended, runs that say nothing about the shell (API errors, a task that didn't set up, an agent that looked up the answer) aren't counted, and an interrupted run resumes.
- **Conformance:** Windows file-system cases (paths past 260 characters, files another program holds open, junctions, read-only files and git's objects, case-only renames, symlinks), programs that prompt, and look-alikes (inputs that resemble what a sill rule acts on, which it must leave alone).

## 0.5.0 (2026-10-27)

### Process lifetimes (Windows Job Objects)
- **Every program sill starts joins a Job Object before it runs.** A process tree followed through parent PIDs loses processes whose parent has exited, and so does `taskkill /T`, how harnesses stop commands. A job doesn't.
  - **`kill $!`** ends everything a background job started, including what a wrapper that already exited left behind (`npx`, `cmd /c start`): no more dev server holding its port.
  - **Killed mid-command** (a harness's timeout), sill takes everything the command started with it.
  - **A command that finishes normally** leaves what it deliberately started running (`start`, an editor, a build daemon), as from a terminal.
- **MCP server:** commands join their job before starting (no window for a grandchild to escape). Programs a command leaves running become a background job of the session, reported in the result, and end with the session. The first command runs in the client's project root, or the home folder rather than a client's install folder (a folder in the temporary folder stays where it is).
- **`kill`** takes every signal whose default is to end a process (`USR1`, `ALRM`, ...; it ends the process tree, with 128 + the number), refuses the ones that would stop, continue or be ignored rather than end it, and `kill -l` lists or translates them as bash does (`kill -l 143` is `TERM`). An invalid signal is status 1, as in bash.
- **MCP output in the system's code page:** a line that isn't UTF-8 (Java since JEP 400 writes GBK into a pipe on Chinese Windows) is read in the ANSI code page when that reads it cleanly.
- **Java through the MCP server writes UTF-8:** the server sets `JAVA_TOOL_OPTIONS=-Dstdout.encoding=UTF-8 -Dstderr.encoding=UTF-8` for its commands (unless you set your own) and leaves out the note each JVM prints for it. Without it, Java loses what the ANSI code page can't hold (`??` for Chinese on English Windows). Measured on GitHub's runner and in a zh-CN Windows container (code page 936). Through `bash.exe -c`, Java's settings are left alone; Known limits says how to get UTF-8 there.

### Checks
- **The build runs the evidence:** the 59 conformance cases, directly and through MCP (all must pass; three are new, for process lifetimes), brush's compat suite on Windows against Git Bash (no failure beyond a committed baseline), and on Linux against bash.
- **Builtins' diagnostics** are written a line at a time, not a byte at a time.

### Docs
- **Java's output encoding** is in Known limits.
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
