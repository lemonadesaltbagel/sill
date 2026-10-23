# Assembles the Windows distribution of sill:
#
#   <Out>\sill\bash.exe, sill.exe     the shell (coreutils built in)
#   <Out>\sill\tools\*.exe            native busybox-w32 applets for the rest
#                                     (grep, sed, awk, find, xargs, diff, ...)
#   <Out>\sill\SETUP.txt              how to point agent harnesses at it
#   <Out>\sill-windows-x64.zip
#
# Usage: pwsh scripts/package-windows.ps1 -Release target\release -Out dist -Busybox busybox.exe -GnuTools gnu [-Ripgrep rg.exe]
param(
    [Parameter(Mandatory)] [string] $Release,
    [Parameter(Mandatory)] [string] $Out,
    # sill's busybox-w32 build (scripts/build-busybox.sh): sed keeps CR, no CRT globbing.
    [Parameter(Mandatory)] [string] $Busybox,
    # GNU grep, sed and diffutils for Windows (scripts/build-gnu-tools.sh).
    [Parameter(Mandatory)] [string] $GnuTools,
    # ripgrep's rg.exe (scripts/fetch-tools.sh); optional, default <GnuTools>\rg.exe.
    [string] $Ripgrep
)
$ErrorActionPreference = 'Stop'

# Applets shipped as tools. Not coreutils (those are built into sill), not
# interactive (vi, less), not shadowing Windows' own tar/curl, and not make
# (POSIX make fails confusingly on GNU Makefiles).
$Applets = @(
    'grep', 'egrep', 'fgrep', 'sed', 'awk', 'find', 'xargs', 'diff', 'patch',
    'which', 'timeout', 'hexdump', 'xxd', 'strings', 'bc', 'iconv',
    'dos2unix', 'unix2dos', 'unzip', 'gzip', 'gunzip', 'zcat', 'bzip2', 'xz', 'unxz'
)
# Coreutils are built into sill, but programs that start other programs (find -exec,
# xargs, make, Python's subprocess, ...) look them up on PATH, where Windows has none,
# or worse, its own unrelated sort.exe and find.exe. These copies serve those callers;
# inside sill the builtins still take precedence.
$Applets += @(
    'rm', 'cp', 'mv', 'mkdir', 'rmdir', 'ln', 'touch', 'cat', 'head', 'tail', 'wc',
    'sort', 'uniq', 'cut', 'tr', 'tee', 'ls', 'echo', 'printf', 'test', 'true', 'false',
    'basename', 'dirname', 'env', 'seq', 'sleep', 'date', 'md5sum', 'sha256sum', 'realpath'
)

$dist = Join-Path $Out 'sill'
$tools = Join-Path $dist 'tools'
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force $tools | Out-Null

foreach ($exe in 'bash.exe', 'sill.exe') {
    Copy-Item (Join-Path $Release $exe) $dist
}
# `sh` is sill too, as Git Bash's sh is bash (and `#!/bin/sh` scripts run in sill): one
# shell for `sh x.sh`, `bash x.sh` and `./x.sh`, not busybox's ash for one of them.
Copy-Item (Join-Path $Release 'bash.exe') (Join-Path $dist 'sh.exe')

$busybox = (Resolve-Path $Busybox).Path

# busybox-w32 picks the applet from its own file name, so each tool is a copy.
foreach ($applet in $Applets) {
    Copy-Item $busybox (Join-Path $tools "$applet.exe")
}

# GNU grep, sed and diffutils (busybox lacks grep --include/--exclude and diff --exclude),
# over busybox's.
foreach ($tool in 'grep', 'sed', 'diff', 'cmp', 'diff3', 'sdiff', 'make') {
    Copy-Item (Join-Path $GnuTools "$tool.exe") (Join-Path $tools "$tool.exe") -Force
}
# jq (scripts/fetch-tools.sh puts it next to the GNU tools). sill starts its own jq with
# -b, so output has LF line ends: jq.exe writes CRLF otherwise, and `$(jq -r .x)` kept a CR.
if (Test-Path (Join-Path $GnuTools 'jq.exe')) {
    Copy-Item (Join-Path $GnuTools 'jq.exe') (Join-Path $tools 'jq.exe')
}
# ripgrep: given, or fetched next to the GNU tools (CI and the lab put it there).
if (-not $Ripgrep -and (Test-Path (Join-Path $GnuTools 'rg.exe'))) { $Ripgrep = Join-Path $GnuTools 'rg.exe' }
if ($Ripgrep) {
    Copy-Item $Ripgrep (Join-Path $tools 'rg.exe')
    # `/` in the paths rg prints (it uses `\` on Windows): unquoted, `src\a.txt` reaches the
    # next command as `srca.txt`. sill points RIPGREP_CONFIG_PATH here unless it's set.
    Set-Content -Encoding ascii (Join-Path $tools 'ripgreprc') '--path-separator=/'
}

@"
sill: an agent-first Bash runtime for native Windows
====================================================

bash.exe   the shell (sill.exe and sh.exe are the same program)
tools\     GNU grep and sed, ripgrep (rg), jq, plus awk, xargs, diff, which, ... (busybox-w32).
           sill puts this directory first on PATH; set SILL_NO_TOOLS=1 to stop it.

Claude Code: add to the "env" block of ~/.claude/settings.json

    "CLAUDE_CODE_GIT_BASH_PATH": "<this folder>\\bash.exe",
    "CLAUDE_CODE_USE_POWERSHELL_TOOL": "0"

The second line makes Bash, not PowerShell, the primary shell tool.
"@ | Set-Content -Encoding utf8 (Join-Path $dist 'SETUP.txt')

$zip = Join-Path $Out 'sill-windows-x64.zip'
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $dist -DestinationPath $zip
Get-ChildItem -Recurse $dist | Measure-Object -Sum Length | ForEach-Object { "dist: $($_.Count) files, $([math]::Round($_.Sum / 1MB, 1)) MB" }
