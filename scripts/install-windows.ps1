# Installs sill from its release zip (sill-windows-x64.zip) for the current user.
#
#   powershell -ExecutionPolicy Bypass -File install-windows.ps1 -Zip sill-windows-x64.zip [-AddToPath] [-ClaudeCode] [-Mcp codex,gemini,claude]
#
# - Unpacks to %LOCALAPPDATA%\Programs\sill (replacing an earlier install).
# - -AddToPath puts that folder on the user PATH (new terminals only).
# - -ClaudeCode points Claude Code's Bash tool at sill: CLAUDE_CODE_GIT_BASH_PATH (and
#   CLAUDE_CODE_USE_POWERSHELL_TOOL=0, so Bash is the shell tool) in the "env" of
#   ~\.claude\settings.json, merged with what is there; the old file is kept as .bak.
# - -Mcp registers sill's MCP server (`bash.exe --mcp`) with Codex CLI (~\.codex\config.toml),
#   Gemini CLI (~\.gemini\settings.json) and/or Claude Desktop
#   (%APPDATA%\Claude\claude_desktop_config.json): harnesses whose own shell is PowerShell,
#   and a chat client with none. (Claude Desktop reads it when it starts.)
#   Each tool call may take up to 11 minutes (sill stops a command at 10). Approval settings
#   are left to the client. The old file is kept as .bak.
# Nothing else is touched. Uninstall: delete the folder, the PATH entry, and the added keys.

param(
    [Parameter(Mandatory)] [string] $Zip,
    [switch] $AddToPath,
    [switch] $ClaudeCode,
    # codex, gemini, claude (Claude Desktop), comma-separated.
    [string[]] $Mcp = @(),
    # Overrides (for tests): where to install, and which Claude Code settings file.
    [string] $Destination = (Join-Path $env:LOCALAPPDATA 'Programs\sill'),
    [string] $SettingsFile = (Join-Path $HOME '.claude\settings.json'),
    [string] $CodexConfig = (Join-Path $HOME '.codex\config.toml'),
    [string] $GeminiSettings = (Join-Path $HOME '.gemini\settings.json'),
    [string] $ClaudeDesktopConfig = (Join-Path $env:APPDATA 'Claude\claude_desktop_config.json')
)

$ErrorActionPreference = 'Stop'
# `-Mcp codex,gemini` arrives as one string through -File.
$Mcp = @($Mcp | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim().ToLower() } | Where-Object { $_ })
foreach ($m in $Mcp) { if ($m -notin 'codex', 'gemini', 'claude') { throw "-Mcp: unknown client '$m' (codex, gemini, claude)" } }
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

if ($Mcp -contains 'codex') {
    New-Item -ItemType Directory -Force (Split-Path $CodexConfig) | Out-Null
    $text = if (Test-Path $CodexConfig) { Copy-Item $CodexConfig "$CodexConfig.bak" -Force; Get-Content -Raw $CodexConfig } else { '' }
    # Replace an earlier [mcp_servers.sill] table (up to the next table), then append ours.
    $text = [regex]::Replace($text, '(?ms)^\[mcp_servers\.sill\]\r?\n.*?(?=^\[|\z)', '')
    $toml = "`n[mcp_servers.sill]`ncommand = '$bash'`nargs = [""--mcp""]`nstartup_timeout_sec = 30`ntool_timeout_sec = 660`n"
    [IO.File]::WriteAllText($CodexConfig, $text.TrimEnd() + "`n" + $toml, (New-Object Text.UTF8Encoding $false))
    "Codex CLI: sill's MCP server added to $CodexConfig"
}

# Adds sill to a JSON settings file's "mcpServers" (Gemini CLI, Claude Desktop), keeping the
# rest; the old file is kept as .bak.
function Add-JsonMcpServer([string] $file, [hashtable] $extra) {
    New-Item -ItemType Directory -Force (Split-Path $file) | Out-Null
    $settings = if (Test-Path $file) {
        Copy-Item $file "$file.bak" -Force
        Get-Content -Raw $file | ConvertFrom-Json
    } else { [pscustomobject]@{} }
    if (-not $settings.PSObject.Properties['mcpServers']) {
        $settings | Add-Member -NotePropertyName mcpServers -NotePropertyValue ([pscustomobject]@{})
    }
    $server = [ordered]@{ command = $bash; args = @('--mcp') }
    foreach ($k in $extra.Keys) { $server[$k] = $extra[$k] }
    $server = [pscustomobject]$server
    if ($settings.mcpServers.PSObject.Properties['sill']) { $settings.mcpServers.sill = $server }
    else { $settings.mcpServers | Add-Member -NotePropertyName sill -NotePropertyValue $server }
    [IO.File]::WriteAllText($file, ($settings | ConvertTo-Json -Depth 32), (New-Object Text.UTF8Encoding $false))
}

if ($Mcp -contains 'gemini') {
    Add-JsonMcpServer $GeminiSettings @{ timeout = 660000 }
    "Gemini CLI: sill's MCP server added to $GeminiSettings"
}

if ($Mcp -contains 'claude') {
    Add-JsonMcpServer $ClaudeDesktopConfig @{}
    "Claude Desktop: sill's MCP server added to $ClaudeDesktopConfig (restart Claude Desktop)"
}
