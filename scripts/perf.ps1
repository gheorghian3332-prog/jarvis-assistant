# Performance profiles of the release build (profile.ps1 -IncludeChildren -Gpu), each with a private data folder:
#   tray          tray, voice off, 300 s
#   tray-voice    tray, wake-word listening on, 300 s
#   window        compact window, IDLE, 120 s                       (opens a visible window)
#   speech        window while Piper speaks (WAV sink, no sound)     (opens a visible window)
#   unloaded      window after the LLM was used and unloaded         (loads a model on the GPU)
# Window and GPU profiles are skipped, and reported as NOT VERIFIED, while a game or any full-screen application is in
# the foreground. Output: one CSV per profile and a summary in $Out (default: the temp folder).
[CmdletBinding()]
param(
    [string]$Exe,
    [ValidateSet('tray', 'tray-voice', 'window', 'speech', 'unloaded')][string[]]$Profiles = @('tray', 'tray-voice', 'window', 'speech', 'unloaded'),
    [string]$Out,
    [int]$TraySeconds = 300,
    [int]$WindowSeconds = 120
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
. (Join-Path $root 'tests\e2e\cdp.ps1')
$env:Path = "$env:LOCALAPPDATA\Jarvis\models\libvosk;" + $env:Path
(Get-Process -Id $PID).PriorityClass = 'BelowNormal'
if (-not $Exe) { $Exe = Join-Path $root 'target\release\jarvis.exe' }
$Exe = (Resolve-Path -LiteralPath $Exe).Path
if (-not $Out) { $Out = Join-Path ([IO.Path]::GetTempPath()) 'jarvis-perf' }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$profileScript = Join-Path $PSScriptRoot 'profile.ps1'

function Set-StoredSetting([string]$DataDir, [string]$Key, [string]$JsonValue) {
    $code = "import sqlite3,sys;c=sqlite3.connect(sys.argv[1]);c.execute('INSERT OR REPLACE INTO settings(key,value,updated_at) VALUES(?,?,CURRENT_TIMESTAMP)',(sys.argv[2],sys.argv[3]));c.commit()"
    & python -I -c $code (Join-Path $DataDir 'jarvis.db') $Key $JsonValue
}

# Creates the database (first start) in a fresh data folder and returns the folder.
function New-DataDir {
    $dir = Join-Path ([IO.Path]::GetTempPath()) ('jarvis-perf-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $app = Start-JarvisApp -Exe $Exe -Arguments @('--tray') -DataDir $dir
    Start-Sleep -Seconds 4
    Stop-ProcessTree $app.Process.Id
    Start-Sleep -Milliseconds 500
    return $dir
}

function Invoke-Profile([string]$Name, [int]$ProcessId, [int]$Seconds) {
    $csv = Join-Path $Out "$Name.csv"
    Write-Host "== profile $Name ($Seconds s)"
    $text = & $profileScript -ProcessId $ProcessId -Seconds $Seconds -Out $csv -IncludeChildren -Gpu 2>&1 | Out-String -Width 200
    Write-Host $text
    Add-Content -LiteralPath (Join-Path $Out 'summary.txt') -Value "== $Name ($Seconds s)`r`n$text"
}

function Test-Intrusive([string]$Name) {
    if (Test-SafeToIntrude) { return $true }
    $info = Get-ForegroundInfo
    $line = "[SKIPPED] $Name : NOT VERIFIED, foreground is $($info.Name) (fullscreen=$($info.Fullscreen))"
    Write-Host $line -ForegroundColor Yellow
    Add-Content -LiteralPath (Join-Path $Out 'summary.txt') -Value $line
    return $false
}

if ('tray' -in $Profiles) {
    $dir = New-DataDir
    $app = Start-JarvisApp -Exe $Exe -Arguments @('--tray') -DataDir $dir
    try { Start-Sleep -Seconds 5; Invoke-Profile 'tray' $app.Process.Id $TraySeconds } finally { Stop-ProcessTree $app.Process.Id }
}
if ('tray-voice' -in $Profiles) {
    $dir = New-DataDir
    Set-StoredSetting $dir 'voice.enabled' 'true'
    $app = Start-JarvisApp -Exe $Exe -Arguments @('--tray') -DataDir $dir
    try { Start-Sleep -Seconds 8; Invoke-Profile 'tray-voice' $app.Process.Id $TraySeconds } finally { Stop-ProcessTree $app.Process.Id }
}
if ('window' -in $Profiles -or 'speech' -in $Profiles -or 'unloaded' -in $Profiles) {
    foreach ($name in 'window', 'speech', 'unloaded') {
        if ($name -notin $Profiles) { continue }
        if (-not (Test-Intrusive $name)) { continue }
        $env2 = @{ JARVIS_E2E_TTS_WAV = (Join-Path ([IO.Path]::GetTempPath()) "jarvis-perf-$name.wav") }
        $app = Start-JarvisApp -Exe $Exe -Environment $env2 -Debugging
        try {
            $cdp = Connect-Cdp $app.Port
            [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'tts.volume'; value = 0.05 })
            Start-Sleep -Seconds 6
            switch ($name) {
                'window' { Invoke-Profile 'window' $app.Process.Id $WindowSeconds }
                'speech' {
                    $long = ('Это длинная проверочная фраза для измерения нагрузки во время речи. ' * 3).Trim()
                    [void](Invoke-TauriCommand $cdp 'tts_speak' @{ text = $long })
                    Invoke-Profile 'speech' $app.Process.Id 30
                }
                'unloaded' {
                    $ollamaUp = $false
                    try { Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/version' -TimeoutSec 3 | Out-Null; $ollamaUp = $true } catch { }
                    if (-not $ollamaUp) {
                        Add-Content -LiteralPath (Join-Path $Out 'summary.txt') -Value '[SKIPPED] unloaded : NOT VERIFIED, Ollama is not running'
                    } else {
                        Install-EventCollector $cdp @('assistant://message')
                        [void](Send-Phrase $cdp 'Объясни одним предложением, почему небо голубое' 120)
                        [void](Invoke-TauriCommand $cdp 'ai_unload')
                        Start-Sleep -Seconds 8
                        Invoke-Profile 'unloaded' $app.Process.Id 60
                    }
                }
            }
        } finally { Stop-ProcessTree $app.Process.Id }
    }
}
Write-Host "Summary: $(Join-Path $Out 'summary.txt')"
exit 0
