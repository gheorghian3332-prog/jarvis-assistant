# Helpers for the end-to-end scripts: start the release exe with an isolated data folder, talk to its
# WebView2 over the DevTools protocol (no mouse, no keyboard), and guard against intruding on a game.
# Dot-source this file. PowerShell 5.1.

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class JarvisFg {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")] public static extern int GetWindowLong(IntPtr h, int index);
}
'@

# Process names that count as games or other full-screen applications even when they are windowed.
$script:GamePatterns = @('eldenring', 'steam_app', 'Cyberpunk', 'witcher', 'valorant', 'csgo', 'cs2', 'dota2',
    'fortnite', 'gta', 'RDR2', 'Overwatch', 'LeagueClient', 'minecraft', 'Warframe', 'Starfield', 'hl2', 'UnrealEditor')

function Get-ForegroundInfo {
    $handle = [JarvisFg]::GetForegroundWindow()
    $pid2 = [uint32]0
    [void][JarvisFg]::GetWindowThreadProcessId($handle, [ref]$pid2)
    $name = ''
    $path = ''
    try { $p = Get-Process -Id $pid2 -ErrorAction Stop; $name = $p.ProcessName; $path = $p.Path } catch { }
    $rect = New-Object JarvisFg+RECT
    [void][JarvisFg]::GetWindowRect($handle, [ref]$rect)
    $width = [JarvisFg]::GetSystemMetrics(0)
    $height = [JarvisFg]::GetSystemMetrics(1)
    # A window that covers the whole screen and has no title bar is full-screen (games, F11 video); a
    # maximized normal window keeps its caption and is fine.
    $style = [JarvisFg]::GetWindowLong($handle, -16)
    $hasCaption = ($style -band 0x00C00000) -eq 0x00C00000
    $covers = (($rect.Right - $rect.Left) -ge $width) -and (($rect.Bottom - $rect.Top) -ge $height) -and (-not $hasCaption)
    [pscustomobject]@{ Name = $name; Path = $path; Fullscreen = $covers; Pid = $pid2 }
}

# True when it is fine to open a window, load a model on the GPU or play sound.
function Test-SafeToIntrude {
    $info = Get-ForegroundInfo
    if ($info.Name -in @('explorer', 'ShellExperienceHost', 'SearchHost', 'StartMenuExperienceHost', 'LockApp') -and -not $info.Fullscreen) { return $true }
    if ($info.Fullscreen) { return $false }
    foreach ($pattern in $script:GamePatterns) { if ($info.Name -like "*$pattern*") { return $false } }
    if ($info.Path -and $info.Path -match '\\steamapps\\|\\Epic Games\\|\\GOG Galaxy\\Games\\') { return $false }
    return $true
}

function Get-FreePort {
    $listener = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = ($listener.LocalEndpoint).Port
    $listener.Stop()
    return $port
}

# Starts jarvis.exe with the given environment. Returns an object with Process, DataDir, Port.
function Start-JarvisApp {
    param(
        [Parameter(Mandatory)][string]$Exe,
        [string[]]$Arguments = @(),
        [hashtable]$Environment = @{},
        [string]$DataDir,
        [switch]$Debugging
    )
    if (-not $DataDir) { $DataDir = Join-Path ([IO.Path]::GetTempPath()) ("jarvis-e2e-" + [guid]::NewGuid().ToString('N').Substring(0, 8)) }
    New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
    $port = 0
    $saved = @{}
    $set = @{ JARVIS_DATA_DIR = $DataDir }
    if ($Debugging) {
        $port = Get-FreePort
        $set.WEBVIEW2_USER_DATA_FOLDER = Join-Path $DataDir 'webview'
        $set.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$port --remote-allow-origins=*"
    }
    foreach ($key in $Environment.Keys) { $set[$key] = $Environment[$key] }
    foreach ($key in $set.Keys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process'); [Environment]::SetEnvironmentVariable($key, [string]$set[$key], 'Process') }
    try {
        $startArgs = @{ FilePath = $Exe; PassThru = $true }
        if ($Arguments.Count -gt 0) { $startArgs.ArgumentList = $Arguments }
        $process = Start-Process @startArgs
    } finally {
        foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process') }
    }
    [pscustomobject]@{ Process = $process; DataDir = $DataDir; Port = $port }
}

# Minimizes the main window of a process we started (without activating it), so a test window does not stay on the
# user's screen. DevTools keeps working on a minimized window.
function Hide-AppWindow($App, [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $App.Process.Refresh()
        if ($App.Process.MainWindowHandle -ne [IntPtr]::Zero) {
            [void][JarvisFg]::ShowWindow($App.Process.MainWindowHandle, 7)   # SW_SHOWMINNOACTIVE
            return $true
        }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Stop-JarvisApp($App) {
    if ($App.Process -and -not $App.Process.HasExited) {
        # Closes the main window gently first; the tray quit item is not reachable without the mouse.
        [void]$App.Process.CloseMainWindow()
        if (-not $App.Process.WaitForExit(3000)) { Stop-Process -Id $App.Process.Id -Force }
    }
}

# Kills the leftover children of a process id (wake worker, gpu worker, STT python started by it).
function Stop-ProcessTree([int]$RootId) {
    $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$RootId" -ErrorAction SilentlyContinue
    foreach ($child in $children) { Stop-ProcessTree $child.ProcessId }
    Stop-Process -Id $RootId -Force -ErrorAction SilentlyContinue
}

function Get-ChildProcesses([int]$RootId) {
    $result = @()
    foreach ($child in (Get-CimInstance Win32_Process -Filter "ParentProcessId=$RootId" -ErrorAction SilentlyContinue)) {
        $result += $child
        $result += Get-ChildProcesses $child.ProcessId
    }
    return $result
}

# ---- DevTools protocol over a WebSocket ----

function Connect-Cdp([int]$Port, [int]$TimeoutSeconds = 40) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $target = $null
    while ([DateTime]::UtcNow -lt $deadline -and -not $target) {
        try {
            $list = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 2
            $target = $list | Where-Object { $_.type -eq 'page' -and $_.webSocketDebuggerUrl -and $_.url -like '*tauri.localhost*' } | Select-Object -First 1
        } catch { }
        if (-not $target) { Start-Sleep -Milliseconds 300 }
    }
    if (-not $target) { throw "no DevTools page on port $Port" }
    $socket = New-Object System.Net.WebSockets.ClientWebSocket
    $socket.ConnectAsync([Uri]$target.webSocketDebuggerUrl, [Threading.CancellationToken]::None).Wait()
    $session = [pscustomobject]@{ Socket = $socket; Next = 0; Port = $Port; Events = @(); Reconnects = 0 }
    # The page may still be loading: wait until the Tauri bridge exists.
    while ([DateTime]::UtcNow -lt $deadline) {
        $ready = $false
        try { $ready = [bool](Invoke-Js $session 'typeof window.__TAURI_INTERNALS__ !== "undefined"' -NoRetry) } catch { }
        if ($ready) { return $session }
        Start-Sleep -Milliseconds 300
    }
    throw 'the Tauri bridge did not appear in the page'
}

function Invoke-Cdp($Session, [string]$Method, $Params = @{}) {
    $Session.Next++
    $id = $Session.Next
    $payload = @{ id = $id; method = $Method; params = $Params } | ConvertTo-Json -Depth 20 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($payload)
    $Session.Socket.SendAsync([ArraySegment[byte]]$bytes, 'Text', $true, [Threading.CancellationToken]::None).Wait()
    $buffer = New-Object byte[] 65536
    while ($true) {
        $builder = New-Object Text.StringBuilder
        do {
            $segment = New-Object 'ArraySegment[byte]' -ArgumentList (, $buffer)
            $task = $Session.Socket.ReceiveAsync($segment, [Threading.CancellationToken]::None)
            if (-not $task.Wait(90000)) { throw "CDP ${Method}: no answer for 90 s" }
            $received = $task.Result
            [void]$builder.Append([Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count))
        } while (-not $received.EndOfMessage)
        $message = $builder.ToString() | ConvertFrom-Json
        if ($message.id -eq $id) {
            if ($message.error) { throw "CDP $Method failed: $($message.error.message)" }
            return $message.result
        }
    }
}

# Evaluates JavaScript (awaiting promises) and returns the value. When the page was rebuilt or the connection dropped,
# the session reconnects once (and re-installs the event collector) and tries again.
function Invoke-Js($Session, [string]$Expression, [switch]$NoRetry) {
    try {
        $result = Invoke-Cdp $Session 'Runtime.evaluate' @{ expression = $Expression; awaitPromise = $true; returnByValue = $true }
    } catch {
        if ($NoRetry -or -not $Session.Port) { throw }
        $fresh = Connect-Cdp $Session.Port 20
        $Session.Socket = $fresh.Socket
        $Session.Reconnects++
        if ($Session.Events.Count -gt 0) { Install-EventCollector $Session $Session.Events }
        $result = Invoke-Cdp $Session 'Runtime.evaluate' @{ expression = $Expression; awaitPromise = $true; returnByValue = $true }
    }
    if ($result.exceptionDetails) {
        $text = $result.exceptionDetails.exception.description
        if (-not $text) { $text = $result.exceptionDetails.text }
        throw "JS error: $text"
    }
    return $result.result.value
}

# Calls a Tauri command from the page. Returns @{ ok; value | error }.
function Invoke-TauriCommand($Session, [string]$Command, $Arguments = @{}, [switch]$NoReconnect) {
    $json = $Arguments | ConvertTo-Json -Depth 10 -Compress
    $js = "window.__TAURI_INTERNALS__.invoke('$Command', $json).then(v => JSON.stringify({ok:true,value:v ?? null}), e => JSON.stringify({ok:false,error:e}))"
    $text = Invoke-Js $Session $js -NoRetry:$NoReconnect
    return $text | ConvertFrom-Json
}

# Collects backend events into window.__e2e so a script can wait for them.
function Install-EventCollector($Session, [string[]]$Events) {
    $Session.Events = $Events
    $names = ($Events | ForEach-Object { "'$_'" }) -join ','
    $js = @"
(async () => {
  window.__e2e = window.__e2e || [];
  const internals = window.__TAURI_INTERNALS__;
  for (const name of [$names]) {
    const handler = internals.transformCallback((event) => { window.__e2e.push({ name, payload: event.payload, at: Date.now() }); });
    await internals.invoke('plugin:event|listen', { event: name, target: { kind: 'Any' }, handler });
  }
  return true;
})()
"@
    [void](Invoke-Js $Session $js -NoRetry)
}

function Get-CollectedEvents($Session, [string]$Name) {
    $text = Invoke-Js $Session "JSON.stringify(window.__e2e.filter(e => e.name === '$Name'))"
    return @($text | ConvertFrom-Json)
}

function Clear-CollectedEvents($Session) { [void](Invoke-Js $Session 'window.__e2e.length = 0; true') }

# Types a command the way the UI does (chat_send) and waits for the next assistant message.
function Send-Phrase($Session, [string]$Text, [int]$TimeoutSeconds = 20) {
    Clear-CollectedEvents $Session
    $reply = Invoke-TauriCommand $Session 'chat_send' @{ text = $Text }
    if (-not $reply.ok) { return [pscustomobject]@{ Ok = $false; Text = ''; Error = $reply.error.message; Events = @() } }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $messages = Get-CollectedEvents $Session 'assistant://message'
        if ($messages.Count -gt 0) {
            $confirm = Get-CollectedEvents $Session 'confirm://request'
            return [pscustomobject]@{ Ok = $true; Text = $messages[-1].payload.text; Skill = $messages[-1].payload.skill; Confirm = $confirm; Error = '' }
        }
        $confirm = Get-CollectedEvents $Session 'confirm://request'
        if ($confirm.Count -gt 0) { return [pscustomobject]@{ Ok = $true; Text = ''; Skill = $null; Confirm = $confirm; Error = '' } }
        Start-Sleep -Milliseconds 150
    }
    return [pscustomobject]@{ Ok = $false; Text = ''; Error = 'timeout'; Confirm = @() }
}

# Reads the audit log straight from the app (the same table the UI shows).
function Get-AuditActions($Session, [int]$Limit = 50) {
    $r = Invoke-TauriCommand $Session 'audit_list' @{ limit = $Limit }
    if (-not $r.ok) { return @() }
    return @($r.value)
}
