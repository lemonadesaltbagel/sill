# Installs sill from its release zip (sill-windows-x64.zip) for the current user.
#
#   powershell -ExecutionPolicy Bypass -File install-windows.ps1 -Zip sill-windows-x64.zip [-AddToPath] [-ClaudeCode]
#
# - Unpacks to %LOCALAPPDATA%\Programs\sill (replacing an earlier install).
# - -AddToPath puts that folder on the user PATH (new terminals only).
# - -ClaudeCode points Claude Code's Bash tool at sill: CLAUDE_CODE_GIT_BASH_PATH (and
#   CLAUDE_CODE_USE_POWERSHELL_TOOL=0, so Bash is the shell tool) in the "env" of
#   ~\.claude\settings.json, merged with what is there; the old file is kept as .bak.
# Nothing else is touched. Uninstall: delete the folder, the PATH entry, and the two keys.

param(
    [Parameter(Mandatory)] [string] $Zip,
    [switch] $AddToPath,
    [switch] $ClaudeCode,
    # Overrides (for tests): where to install, and which Claude Code settings file.
    [string] $Destination = (Join-Path $env:LOCALAPPDATA 'Programs\sill'),
    [string] $SettingsFile = (Join-Path $HOME '.claude\settings.json')
)

$ErrorActionPreference = 'Stop'
$dest = $Destination
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("sill-install-" + [Guid]::NewGuid())
Expand-Archive -Path $Zip -DestinationPath $tmp
$src = if (Test-Path (Join-Path $tmp 'sill\bash.exe')) { Join-Path $tmp 'sill' } else { $tmp }
if (-not (Test-Path (Join-Path $src 'bash.exe'))) { throw "no bash.exe in $Zip" }
if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
Move-Item $src $dest
Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
$bash = Join-Path $dest 'bash.exe'
"installed: $bash ($(& $bash --version | Select-Object -First 1))"

if ($AddToPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @($userPath -split ';' | Where-Object { $_ })
    if ($parts -notcontains $dest) {
        [Environment]::SetEnvironmentVariable('Path', (@($dest) + $parts) -join ';', 'User')
        "added to the user PATH (new terminals): $dest"
    }
}

if ($ClaudeCode) {
    $file = $SettingsFile
    $dir = Split-Path $file
    New-Item -ItemType Directory -Force $dir | Out-Null
    $settings = if (Test-Path $file) {
        Copy-Item $file "$file.bak" -Force
        Get-Content -Raw $file | ConvertFrom-Json
    } else { [pscustomobject]@{} }
    if (-not $settings.PSObject.Properties['env']) {
        $settings | Add-Member -NotePropertyName env -NotePropertyValue ([pscustomobject]@{})
    }
    foreach ($kv in @(@('CLAUDE_CODE_GIT_BASH_PATH', $bash), @('CLAUDE_CODE_USE_POWERSHELL_TOOL', '0'))) {
        if ($settings.env.PSObject.Properties[$kv[0]]) { $settings.env.($kv[0]) = $kv[1] }
        else { $settings.env | Add-Member -NotePropertyName $kv[0] -NotePropertyValue $kv[1] }
    }
    # UTF-8 without a BOM: Windows PowerShell's `-Encoding utf8` writes one, and Node's
    # JSON.parse (Claude Code) rejects it.
    [IO.File]::WriteAllText($file, ($settings | ConvertTo-Json -Depth 32), (New-Object Text.UTF8Encoding $false))
    "Claude Code now uses sill as its Bash tool ($file; previous version in settings.json.bak)"
}
