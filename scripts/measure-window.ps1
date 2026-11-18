# Splits the CPU of the open window by process role (app = Rust process, renderer = WebView2 page, webview-gpu, gpu-worker = NVML helper),
# per scenario: idle (GENERAL), system (SYSTEM section open), closed (back to GENERAL). -Ui old|new picks the DOM used to switch sections.
# Needs no game or full-screen app in the foreground (exits with 3 otherwise). Usage: scripts\measure-window.ps1 -Exe <jarvis.exe> -Label <name>
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Label,
    [ValidateSet('old', 'new')][string]$Ui = 'old',
    [int]$Seconds = 40,
    [string[]]$Scenarios = @('idle', 'system')
)
$ErrorActionPreference = 'Stop'
(Get-Process -Id $PID).PriorityClass = 'BelowNormal'
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $root 'tests\e2e\cdp.ps1')
$env:Path = "$env:LOCALAPPDATA\Jarvis\models\libvosk;" + $env:Path
if (-not (Test-SafeToIntrude)) { Write-Host 'NOT SAFE: game or fullscreen app in foreground'; exit 3 }

$dir = Join-Path ([IO.Path]::GetTempPath()) ('jarvis-p12-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$wav = Join-Path ([IO.Path]::GetTempPath()) 'jarvis-p12.wav'
$app = Start-JarvisApp -Exe $Exe -Environment @{ JARVIS_E2E_TTS_WAV = $wav } -DataDir $dir -Debugging
$cpuCount = [Environment]::ProcessorCount
try {
    $cdp = Connect-Cdp $app.Port
    [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'setup.completed'; value = $true })
    [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'tts.volume'; value = 0.05 })
    [void](Invoke-Js $cdp 'location.reload(); true')
    Start-Sleep -Seconds 10
    $cdp = Connect-Cdp $app.Port

    function Get-Roles([int]$RootId) {
        $map = @{ $RootId = 'app' }
        foreach ($p in (Get-ChildProcesses $RootId)) {
            $cl = [string]$p.CommandLine
            $role = 'other'
            if ($p.Name -like 'msedgewebview2*') {
                if ($cl -match '--type=renderer') { $role = 'renderer' }
                elseif ($cl -match '--type=gpu-process') { $role = 'webview-gpu' }
                else { $role = 'webview-other' }
            } elseif ($cl -match '--gpu-worker') { $role = 'gpu-worker' }
            else { $role = 'child:' + $p.Name }
            $map[[int]$p.ProcessId] = $role
        }
        return $map
    }

    function Measure-Window([string]$Name) {
        $roles = Get-Roles $app.Process.Id
        $prev = @{}
        $sum = @{}
        $n = 0
        $prevAt = [DateTime]::UtcNow
        for ($i = 0; $i -lt $Seconds; $i++) {
            Start-Sleep -Seconds 1
            $now = [DateTime]::UtcNow
            $elapsed = ($now - $prevAt).TotalSeconds
            $prevAt = $now
            if ($i % 10 -eq 0) { $roles = Get-Roles $app.Process.Id }
            $tick = @{}
            foreach ($id in $roles.Keys) {
                $m = Get-Process -Id $id -ErrorAction SilentlyContinue
                if (-not $m) { continue }
                try { $t = $m.TotalProcessorTime.TotalSeconds } catch { continue }
                if ($prev.ContainsKey($id) -and $i -gt 0) {
                    $d = [Math]::Max(0.0, $t - $prev[$id]) / $elapsed / $cpuCount * 100
                    $r = $roles[$id]
                    if (-not $tick.ContainsKey($r)) { $tick[$r] = 0.0 }
                    $tick[$r] += $d
                }
                $prev[$id] = $t
            }
            if ($i -gt 0) {
                foreach ($r in $tick.Keys) { if (-not $sum.ContainsKey($r)) { $sum[$r] = 0.0 }; $sum[$r] += $tick[$r] }
                $n++
            }
        }
        $total = 0.0
        $parts = @()
        foreach ($r in ($sum.Keys | Sort-Object)) { $v = $sum[$r] / $n; $total += $v; $parts += ('{0}={1:N3}' -f $r, $v) }
        Write-Host ('[{0}] {1} {2}: total={3:N3}%  {4}' -f $Label, $Ui, $Name, $total, ($parts -join '  '))
    }

    foreach ($scenario in $Scenarios) {
        switch ($scenario) {
            'idle' { }
            'system' {
                if ($Ui -eq 'old') {
                    [void](Invoke-Js $cdp "[...document.querySelectorAll('[role=tab]')].find(b=>b.textContent.startsWith('SYSTEM')).click(); true")
                } else {
                    [void](Invoke-Js $cdp "document.querySelector('button[aria-haspopup=dialog]').click(); true")
                    Start-Sleep -Milliseconds 600
                    [void](Invoke-Js $cdp "document.getElementById('tab-SYSTEM').click(); true")
                }
            }
            'closed' {
                if ($Ui -eq 'new') { [void](Invoke-Js $cdp "document.querySelector('.drawer-head button').click(); true") }
                else { [void](Invoke-Js $cdp "[...document.querySelectorAll('[role=tab]')].find(b=>b.textContent.startsWith('GENERAL')).click(); true") }
            }
        }
        Start-Sleep -Seconds 8
        Measure-Window $scenario
    }
} finally {
    Stop-ProcessTree $app.Process.Id
}
