"""The conformance cases: what a coding agent relies on from a Bash on Windows.

Each case is a bash script run in a fresh directory. Its expectation is what bash does on
Linux (the behaviour agents are trained on) or, where Windows forces a choice, what lets
the agent's next command work. `why` gives the reason.

Cases outside the `argv` category that start cmd.exe set MSYS_NO_PATHCONV=1, so Git
Bash's conversion of `/d /c` into paths (measured once, by `cmd-c-string`) doesn't decide
cases about something else. Other shells ignore the variable.

Fields:
  id, cat   identifier and category
  why       the agent-facing reason
  script    bash, run as `<shell> -c <script>`; {echoargs} is the echoargs.exe path
  files     fixture files {relative path: str or bytes}
  expect    exact stdout (CRLF read as LF); or
  check     fn(out: bytes, err: bytes, code: int, secs: float) -> bool
  needs     programs that must exist on the machine for the case to apply
  after_no_process  a command-line fragment no process may still have after the case
  after_running     a command-line fragment a process must still have after the case (it
                    is then stopped)
  timeout   seconds after which the shell is killed, as a harness's timeout does
  direct_only  not run through the MCP server (whose sessions end what commands leave behind)
"""

import json


def _json(b):
    try:
        return json.loads(b.decode("utf-8"))
    except Exception:  # noqa: BLE001
        return None


def text(b):
    return b.decode("utf-8", "replace").replace("\r\n", "\n")


def utf8(b):
    try:
        b.decode("utf-8")
        return True
    except UnicodeDecodeError:
        return False


def argv(id, words, want, why):
    return {"id": id, "cat": "argv", "why": why, "script": '"{echoargs}" ' + words,
            "check": lambda o, e, c, s, want=want: _json(o) == want}


NAMES = ["中文", "日本語", "한국어", "Русский", "café", "😀"]

CASES = [
    # --- argv: a native program receives exactly the shell's words -----------------------
    argv("argv-spaces", '"a b" c', ["a b", "c"], "A word with spaces stays one argument."),
    argv("argv-empty", '"" x', ["", "x"], "An empty word is an argument, as bash passes it."),
    argv("argv-quotes", "'say \"hi\"'", ['say "hi"'], "Embedded double quotes arrive intact."),
    argv("argv-trailing-backslash", "'C:\\dir\\'", ["C:\\dir\\"],
         "A Windows directory with a trailing backslash; the quoting must double it."),
    argv("argv-glob-literal", "'*.txt' '?'", ["*.txt", "?"],
         "A quoted glob reaches the program as written; `find -name '*.txt'` depends on it."),
    argv("argv-unicode", "'中文 😀'", ["中文 😀"], "Non-ASCII arguments arrive as written."),
    argv("argv-slash-options", "/p:Configuration=Release /EHsc", ["/p:Configuration=Release", "/EHsc"],
         "MSBuild and cl switches are not paths; rewriting them breaks build commands."),
    argv("argv-option-value", "--prefix=/usr/local", ["--prefix=/usr/local"],
         "An option value is not rewritten into a Windows path behind the program's back."),
    {"id": "argv-long", "cat": "argv", "why": "A 20,000-character argument (generated JSON, code) arrives whole.",
     "script": '"{echoargs}" "$(printf "%020000d" 0)"',
     "check": lambda o, e, c, s: (_json(o) or [""])[0] == "0" * 20000},
    {"id": "bat-arg-space", "cat": "argv", "why": "A batch script receives a quoted argument with a space as one.",
     "files": {"t.bat": b"@echo off\r\necho [%~1]\r\n"},
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c t.bat 'release build'", "expect": "[release build]\n"},
    {"id": "cmd-c-string", "cat": "argv", "why": "`cmd /c '...'` runs its command text as cmd would.",
     "script": "cmd.exe /d /c 'echo one&& echo two'", "expect": "one\ntwo\n"},
    {"id": "cmd-double-slash", "cat": "argv", "why": "Git Bash's `cmd //c` idiom, common in models' output.",
     "script": "cmd //c 'echo ok'", "expect": "ok\n"},

    # --- paths ---------------------------------------------------------------------------------
    {"id": "path-drive-form", "cat": "paths", "why": "`/c/...` paths (Claude Code's own form) open.",
     "script": "ls -d /c/Windows/System32 >/dev/null && echo ok", "expect": "ok\n"},
    {"id": "path-windows-form", "cat": "paths", "why": "A pasted `C:\\...` path opens.",
     "script": "test -f 'C:\\Windows\\System32\\cmd.exe' && echo ok", "expect": "ok\n"},
    {"id": "path-tmp-shared", "cat": "paths", "needs": ["python"],
     "why": "A file written to /tmp is the file a native program (and the harness's file tools) reads at /tmp.",
     "script": "echo shared > /tmp/conformance-tmp.txt && python -c \"print(open('/tmp/conformance-tmp.txt').read().strip())\"",
     "expect": "shared\n"},
    {"id": "path-dev-null", "cat": "paths", "why": "`> /dev/null` discards output and creates no file.",
     "script": "echo x > /dev/null; ls", "expect": ""},
    {"id": "path-home", "cat": "paths", "why": "`~` is the user's home directory.",
     "script": "test -d ~ && test -d \"$HOME\" && echo ok", "expect": "ok\n"},

    # --- encoding: what reaches the harness through the pipe is readable UTF-8 ---------------------
    {"id": "enc-cmd-echo", "cat": "encoding", "why": "Text a console program prints arrives as UTF-8.",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'echo 中文 Русский'", "expect": "中文 Русский\n"},
    {"id": "enc-system-message", "cat": "encoding",
     "why": "Windows' own error messages arrive as UTF-8 the model can read (any system language).",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c nosuchcommand_xyz 2>&1",
     "check": lambda o, e, c, s: utf8(o) and b"nosuchcommand_xyz" in o},
    {"id": "enc-powershell", "cat": "encoding", "why": "Windows PowerShell's output arrives as UTF-8.",
     "script": "powershell.exe -NoProfile -Command \"Write-Output '中文 Русский'\"", "expect": "中文 Русский\n"},
    {"id": "enc-python", "cat": "encoding", "needs": ["python"], "why": "Python's print reaches a pipe as UTF-8.",
     "script": "python -c \"print('日本語 😀')\"", "expect": "日本語 😀\n"},
    {"id": "enc-grep-names", "cat": "encoding", "why": "grep opens files named in any script.",
     "files": {f"{n}.txt": "needle\n" for n in NAMES},
     "script": "grep -l needle *.txt | sort",
     "check": lambda o, e, c, s: sorted(text(o).split()) == sorted(f"{n}.txt" for n in NAMES)},
    {"id": "enc-dir-names", "cat": "encoding", "why": "A native listing shows names in any script.",
     "files": {f"{n}.txt": "x" for n in NAMES},
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'dir /b *.txt'",
     "check": lambda o, e, c, s: sorted(text(o).split()) == sorted(f"{n}.txt" for n in NAMES)},
    {"id": "enc-git-paths", "cat": "encoding", "needs": ["git"], "why": "git prints non-ASCII paths readably.",
     "files": {"中文.txt": "x"},
     "script": "git init -q && git add -A && git ls-files", "expect": "中文.txt\n"},

    # --- line endings: bytes in, bytes out ---------------------------------------------------------
    {"id": "crlf-sed-keeps", "cat": "crlf", "why": "`sed -i` on a CRLF file keeps CRLF (GNU sed on Linux does).",
     "files": {"f.ini": b"a=1\r\nb=2\r\n"},
     "script": "sed -i 's/a=1/a=9/' f.ini && od -An -c f.ini | tr -d ' \\n'", "expect": "a=9\\r\\nb=2\\r\\n"},
    {"id": "crlf-grep-sees-cr", "cat": "crlf", "why": "grep sees the CR in a CRLF line.",
     "files": {"f.txt": b"x\r\n"}, "script": "grep -c $'\\r' f.txt", "expect": "1\n"},
    {"id": "crlf-binary-pipe", "cat": "crlf", "why": "A pipe passes binary bytes unchanged.",
     "script": "printf 'a\\0b\\r\\nc' | cat | od -An -tx1 | tr -d ' \\n'", "expect": "6100620d0a63"},
    {"id": "crlf-heredoc-lf", "cat": "crlf", "why": "A here-doc writes LF line ends.",
     "script": "cat > f <<'EOF'\nx\nEOF\nod -An -tx1 f | tr -d ' \\n'", "expect": "780a"},

    # --- processes -------------------------------------------------------------------------------
    {"id": "proc-bg-pid", "cat": "process", "why": "`$!` is the background program's PID; `kill -0` sees it.",
     "script": "ping -n 30 127.0.0.1 >/dev/null & kill -0 $! && echo alive; kill $! 2>/dev/null",
     "expect": "alive\n"},
    {"id": "proc-wait-status", "cat": "process", "why": "`wait PID` returns the program's exit status.",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'exit 7' & wait $!; echo $?", "expect": "7\n"},
    {"id": "proc-kill-tree", "cat": "process",
     "why": "`kill $!` on a wrapper (npm.cmd, cmd /c) stops what it started, not just the wrapper.",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'ping -n 41 127.0.0.1 >nul' & sleep 1; kill $!; sleep 1; echo done",
     "expect": "done\n", "after_no_process": "-n 41"},
    {"id": "proc-kill-orphan", "cat": "process",
     "why": "`kill $!` also stops what a wrapper started and left behind when it exited (`npx`, `cmd /c start`): the dev server that otherwise keeps its port.",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'start /b ping -n 42 127.0.0.1 >nul' & sleep 2; kill $! 2>/dev/null; sleep 1; echo done",
     "expect": "done\n", "after_no_process": "-n 42"},
    {"id": "proc-harness-timeout", "cat": "process",
     "why": "When a harness kills the shell at its timeout, what the command started ends too, not only the shell.",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'ping -n 43 127.0.0.1 >nul'; echo never",
     "timeout": 3, "check": lambda o, e, c, s: b"never" not in o, "after_no_process": "-n 43"},
    {"id": "proc-detached-survives", "cat": "process",
     "why": "A program a finished command deliberately left running (`start`, an editor, a build daemon) keeps running after the shell exits.",
     "script": "MSYS_NO_PATHCONV=1 cmd.exe /d /c 'start /b ping -n 44 127.0.0.1 >nul 2>&1'; echo started",
     "expect": "started\n", "after_running": "-n 44", "direct_only": True},
    {"id": "proc-bg-releases-pipe", "cat": "process",
     "why": "A background job doesn't keep the harness's output pipe open (harnesses wait for it).",
     "script": "ping -n 20 127.0.0.1 >/dev/null 2>&1 & echo started",
     "check": lambda o, e, c, s: text(o).strip() == "started" and s < 5},
    {"id": "proc-pipefail", "cat": "process", "why": "`set -o pipefail` reports a failing stage.",
     "script": "set -o pipefail; false | true; echo $?", "expect": "1\n"},

    # --- size limits -------------------------------------------------------------------------------
    {"id": "size-heredoc-200k", "cat": "limits", "needs": ["python"],
     "why": "A 200 KB here-doc reaches a native program whole.",
     "script": "python -c \"import sys; print(len(sys.stdin.read()))\" <<EOF\n" + ("x" * 99 + "\n") * 2000 + "EOF",
     "expect": "200000\n"},
    {"id": "size-long-script", "cat": "limits", "why": "A 20,000-character command runs whole.",
     "script": "x=" + "a" * 20000 + "; echo ${#x}", "expect": "20000\n"},
    {"id": "size-many-args-ls", "cat": "limits", "why": "`ls dir/*` over 3,000 files works, as on Linux.",
     "files": {f"d/file_with_a_longer_name_{i}.txt": "" for i in range(3000)},
     "script": "ls d/* | wc -l", "check": lambda o, e, c, s: text(o).strip() == "3000"},
    {"id": "size-many-args-grep", "cat": "limits", "why": "`grep -l x dir/*` over 3,000 files works.",
     "files": {**{f"d/file_with_a_longer_name_{i}.txt": "" for i in range(3000)}, "d/hit.txt": "needle\n"},
     "script": "grep -l needle d/*", "expect": "d/hit.txt\n"},

    # --- running scripts -------------------------------------------------------------------------
    {"id": "script-shebang-sh", "cat": "scripts", "why": "`./build.sh` runs through its #! line.",
     "files": {"build.sh": "#!/bin/sh\necho built\n"}, "script": "./build.sh", "expect": "built\n"},
    {"id": "script-shebang-env", "cat": "scripts", "why": "An extensionless `#!/usr/bin/env bash` script (gradlew) runs.",
     "files": {"gradlew": "#!/usr/bin/env bash\necho gradle-ok\n"}, "script": "./gradlew", "expect": "gradle-ok\n"},
    {"id": "script-shebang-python", "cat": "scripts", "needs": ["python"],
     "why": "`#!/usr/bin/env python3` scripts run.",
     "files": {"tool.py": "#!/usr/bin/env python3\nprint('py-ok')\n"}, "script": "./tool.py", "expect": "py-ok\n"},
    {"id": "script-sh-command", "cat": "scripts", "why": "`sh script.sh` runs.",
     "files": {"s.sh": "echo sh-ok\n"}, "script": "sh s.sh", "expect": "sh-ok\n"},
    {"id": "script-python3-not-silent", "cat": "scripts",
     "why": "`python3` either runs Python or says why not; never a silent failure.",
     "script": "python3 -c 'print(1)'", "check": lambda o, e, c, s: text(o).strip() == "1" or len(e.strip()) > 0},

    # --- tools agents use ------------------------------------------------------------------------
    {"id": "tool-grep-include", "cat": "tools", "why": "`grep -rn --include`",
     "files": {"src/a.py": "TODO x\n", "src/b.js": "TODO y\n"},
     "script": "grep -rn --include='*.py' TODO src", "expect": "src/a.py:1:TODO x\n"},
    {"id": "tool-find-exec", "cat": "tools", "why": "`find -exec {} +` gets `/` paths.",
     "files": {"d/s/n.txt": "x\n"}, "script": "find d -name n.txt -exec grep -l x {} +", "expect": "d/s/n.txt\n"},
    {"id": "tool-xargs", "cat": "tools", "why": "`find -print0 | xargs -0`",
     "files": {"a b.txt": "x\n"}, "script": "find . -name '*.txt' -print0 | xargs -0 cat", "expect": "x\n"},
    {"id": "tool-awk", "cat": "tools", "why": "awk", "script": "echo 'a b' | awk '{print $2}'", "expect": "b\n"},
    {"id": "tool-diff", "cat": "tools", "why": "`diff -u`", "files": {"a": "1\n", "b": "2\n"},
     "script": "diff -u a b | tail -2", "expect": "-1\n+2\n"},
    {"id": "tool-make", "cat": "tools", "why": "GNU make with a Unix Makefile.",
     "files": {"Makefile": "all:\n\t@echo made\n"}, "script": "make -s", "expect": "made\n"},
    {"id": "tool-jq", "cat": "tools", "why": "`jq -r` output usable in `$(...)` (no stray CR).",
     "script": "v=$(echo '{\"a\":\"x\"}' | jq -r .a) && [ \"$v\" = x ] && echo ok", "expect": "ok\n"},
    {"id": "tool-rg", "cat": "tools", "why": "ripgrep with `/` paths.",
     "files": {"s/a.txt": "needle\n"}, "script": "rg -l needle s", "expect": "s/a.txt\n"},
    {"id": "tool-ps-pgrep", "cat": "tools", "why": "`pgrep -f` finds a program by its command line.",
     "script": "ping -n 30 127.0.0.1 >/dev/null & sleep 1; pgrep -f 'ping -n 30' >/dev/null && echo found; kill $!",
     "expect": "found\n"},
    {"id": "tool-file", "cat": "tools", "why": "`file` reports CRLF line ends.",
     "files": {"x.ini": b"a\r\n"}, "script": "file -b x.ini", "expect": "ASCII text, with CRLF line terminators\n"},
    {"id": "tool-cygpath", "cat": "tools", "why": "`cygpath -w`, a Git Bash habit.",
     "script": "cygpath -w /c/Windows", "expect": "C:\\Windows\n"},

    # --- errors and identity -------------------------------------------------------------------------
    {"id": "err-not-found", "cat": "errors", "why": "A missing command: status 127 and bash's message shape.",
     "script": "nosuchcmd_xyz; echo $?",
     "check": lambda o, e, c, s: text(o).strip() == "127" and b"command not found" in e},
    {"id": "id-ostype", "cat": "errors", "why": "`$OSTYPE` is one Claude Code accepts (msys, cygwin, win32).",
     "script": "echo $OSTYPE", "check": lambda o, e, c, s: text(o).strip() in ("msys", "cygwin", "win32")},
    {"id": "id-path-posix", "cat": "errors", "why": "`$PATH` has no `;` (Claude Code's PATH probe rejects it).",
     "script": "case $PATH in *\\;*) echo semicolon;; *) echo ok;; esac", "expect": "ok\n"},
]
