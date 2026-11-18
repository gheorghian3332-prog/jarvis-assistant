# End-to-end scenarios on the release build, no mouse and no keyboard: WebView2 is driven over the DevTools
# protocol (tests\e2e\cdp.ps1), the voice chain is fed from WAV files, speech goes to a WAV file.
#
#   headless  tray mode, no window: wake -> STT (CPU) -> command -> speech (WAV sink), then recovery after killing
#             the STT worker, the wake worker and Piper
#   window    commands, memory, confirmation, emergency stop, window modes, GPU helper and Kokoro recovery
#   llm       free question through Ollama (GPU) and the screen question (VLM, GPU) on our own test window
#   ollama    Ollama "stopped": FAST PATH works, the LLM path says the local model is unavailable
#
# `window` and `llm` open a visible window / load a model on the GPU. Before each of them the foreground
# application is checked; when it is a game or any full-screen application the step is SKIPPED and reported
# as NOT VERIFIED (run the script again later). -SkipIntrusive skips them on purpose.
# Build first: cargo build --release -p jarvis-app --features custom-protocol
[CmdletBinding()]
param(
    [string]$Exe,
    [ValidateSet('headless', 'window', 'llm', 'ollama')][string[]]$Only,
    [switch]$SkipIntrusive
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
. (Join-Path $root 'tests\e2e\cdp.ps1')
$env:Path = "$env:LOCALAPPDATA\Jarvis\models\libvosk;" + $env:Path
if (-not $Exe) { $Exe = Join-Path $root 'target\release\jarvis.exe' }
if (-not (Test-Path -LiteralPath $Exe)) { Write-Host "FAILED: $Exe not found (cargo build --release -p jarvis-app --features custom-protocol)"; exit 1 }
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("jarvis-e2e-run-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null
$results = New-Object System.Collections.Generic.List[object]
$started = New-Object System.Collections.Generic.List[int]

function Add-Result([string]$Name, [string]$Status, [string]$Detail = '') {
    $results.Add([pscustomobject]@{ Name = $Name; Status = $Status; Detail = $Detail })
    $color = switch ($Status) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
    Write-Host ("[{0}] {1} {2}" -f $Status, $Name, $Detail) -ForegroundColor $color
}
function Check([string]$Name, [bool]$Condition, [string]$Detail = '') {
    Add-Result $Name ($(if ($Condition) { 'PASS' } else { 'FAIL' })) $Detail
}
function Run-Group([string]$Group) { return (-not $Only) -or ($Group -in $Only) }

# Runs a query on the app database (read-only) through Python's sqlite3; returns the rows as JSON text.
function Get-Rows([string]$DataDir, [string]$Sql) {
    $db = (Join-Path $DataDir 'jarvis.db').Replace([string][char]92, '/')
    $code = "import sqlite3,sys,json;c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True);print(json.dumps(c.execute(sys.argv[2]).fetchall(),ensure_ascii=False))"
    $text = & python -I -c $code $db $Sql 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $text) { return '[]' }
    return [string]$text
}
function Get-Count([string]$DataDir, [string]$Sql) {
    $db = (Join-Path $DataDir 'jarvis.db').Replace([string][char]92, '/')
    $code = "import sqlite3,sys;c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True);print(c.execute(sys.argv[2]).fetchone()[0])"
    $text = & python -I -c $code $db $Sql 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $text) { return 0 }
    return [int]$text
}
function Get-Log([string]$DataDir) {
    $path = Join-Path $DataDir 'logs\jarvis.log'
    if (-not (Test-Path -LiteralPath $path)) { return '' }
    $stream = New-Object IO.FileStream($path, 'Open', 'Read', 'ReadWrite, Delete')
    try { return (New-Object IO.StreamReader($stream, [Text.Encoding]::UTF8)).ReadToEnd() } finally { $stream.Dispose() }
}
function Wait-Until([scriptblock]$Condition, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) { if (& $Condition) { return $true }; Start-Sleep -Milliseconds 500 }
    return $false
}
function Find-Children([int]$RootId, [string]$Pattern) {
    # The comma keeps a single match an array (a lone CIM object has no Count).
    return ,@(Get-ChildProcesses $RootId | Where-Object { ($_.CommandLine -match $Pattern) -or ($_.Name -match $Pattern) })
}
function Kill-Matching([int]$RootId, [string]$Pattern) {
    $victims = Find-Children $RootId $Pattern
    foreach ($victim in $victims) { Stop-Process -Id $victim.ProcessId -Force -ErrorAction SilentlyContinue }
    return [int]$victims.Count
}
function Finish-App($App) {
    if (-not $App) { return }
    Stop-ProcessTree $App.Process.Id
    # WebView2 processes that were started with this run's private data folder outlive their parent; they are ours.
    Start-Sleep -Milliseconds 500
    foreach ($leftover in @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -like "*$($App.DataDir)*" })) {
        Stop-Process -Id $leftover.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

$wavTime = Join-Path $root 'tests\audio\wake_time.wav'
$sttEnv = @{ JARVIS_STT_DEVICE = 'cpu' }

# ---------------------------------------------------------------- headless: voice chain + recovery
if (Run-Group 'headless') {
    Write-Host '== headless: wake -> STT -> command -> TTS (WAV sink), then recovery'
    $ttsWav = Join-Path $work 'headless-tts.wav'
    $env2 = @{ JARVIS_E2E_TTS_WAV = $ttsWav; JARVIS_E2E_VOICE_WAV = "$wavTime;$wavTime"; JARVIS_E2E_VOICE_GAP_SECS = '45' } + $sttEnv
    $app = Start-JarvisApp -Exe $Exe -Arguments @('--tray') -Environment $env2
    try {
        Start-Sleep -Seconds 3
        Check 'tray starts without a window' (-not $app.Process.HasExited) "pid $($app.Process.Id)"
        $first = Wait-Until { (Get-Count $app.DataDir "select count(*) from audit_log where action='info.time'") -ge 1 } 150
        Check 'voice chain: wake -> STT -> info.time executed' $first
        $spoken = Wait-Until { (Test-Path -LiteralPath $ttsWav) -and ((Get-Item -LiteralPath $ttsWav).Length -gt 8000) } 60
        $log = Get-Log $app.DataDir
        Check 'voice chain: the wake word was heard and the utterance transcribed' ($log -match 'wake word detected' -and $log -match 'utterance transcribed')
        Check 'voice chain: the answer was spoken into the WAV sink' $spoken $(if (Test-Path -LiteralPath $ttsWav) { "$((Get-Item -LiteralPath $ttsWav).Length) bytes" } else { 'no file' })
        # Recovery: kill the workers while the app runs; the next phrase (45 s later) must still work.
        $killedStt = Kill-Matching $app.Process.Id 'jarvis_stt|python|uv\.exe'
        $killedWake = Kill-Matching $app.Process.Id 'wake-worker'
        $killedPiper = Kill-Matching $app.Process.Id 'piper'
        Add-Result 'recovery: workers killed' 'PASS' "stt/python=$killedStt wake=$killedWake piper=$killedPiper"
        Remove-Item -LiteralPath $ttsWav -Force -ErrorAction SilentlyContinue
        $second = Wait-Until { (Get-Count $app.DataDir "select count(*) from audit_log where action='info.time'") -ge 2 } 170
        Check 'recovery: the next voice phrase works after killing STT, wake worker and Piper' $second
        $spoken2 = Wait-Until { (Test-Path -LiteralPath $ttsWav) -and ((Get-Item -LiteralPath $ttsWav).Length -gt 8000) } 60
        Check 'recovery: and it is spoken again (Piper restarted)' $spoken2
        Check 'recovery: the application survived' (-not $app.Process.HasExited)
        # Security: no listening port and no remote connection in the whole process tree; no secrets in the log or the audit.
        $ids = @($app.Process.Id) + @(Get-ChildProcesses $app.Process.Id | ForEach-Object { $_.ProcessId })
        $listening = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess })
        Check 'network: the application listens on no port' ($listening.Count -eq 0) (($listening | ForEach-Object { "$($_.LocalAddress):$($_.LocalPort)" }) -join ',')
        $outbound = @(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess -and $_.RemoteAddress -notin @('127.0.0.1', '::1') })
        Check 'network: no connection to a remote host' ($outbound.Count -eq 0) (($outbound | ForEach-Object { "$($_.RemoteAddress):$($_.RemotePort)" }) -join ',')
        $log = Get-Log $app.DataDir
        Check 'log: no secrets, no api keys' ($log -notmatch '(?i)api[_-]?key\s*[=:]\s*\S{8,}|sk-[A-Za-z0-9]{20,}|bearer\s+[A-Za-z0-9._-]{20,}')
    } finally { Finish-App $app }
}

# ---------------------------------------------------------------- ollama stopped
if (Run-Group 'ollama') {
    Write-Host '== ollama: unreachable model'
    if (-not (Test-SafeToIntrude) -and -not $SkipIntrusive) {
        $info = Get-ForegroundInfo
        Add-Result 'ollama-down (window)' 'SKIPPED' "NOT VERIFIED: foreground is $($info.Name) (fullscreen=$($info.Fullscreen))"
    } elseif ($SkipIntrusive) {
        Add-Result 'ollama-down (window)' 'SKIPPED' 'NOT VERIFIED: -SkipIntrusive'
    } else {
        $env3 = @{ JARVIS_OLLAMA_URL = 'http://127.0.0.1:9'; JARVIS_E2E_TTS_WAV = (Join-Path $work 'ollama-tts.wav') } + $sttEnv
        $app = Start-JarvisApp -Exe $Exe -Environment $env3 -Debugging
        try {
            $cdp = Connect-Cdp $app.Port
            [void](Hide-AppWindow $app 5)
            Install-EventCollector $cdp @('assistant://message', 'confirm://request')
            [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'tts.volume'; value = 0.05 })
            $fast = Send-Phrase $cdp 'который час'
            Check 'ollama stopped: the FAST PATH still works' ($fast.Ok -and $fast.Text -match '\d') $fast.Text
            $free = Send-Phrase $cdp 'расскажи мне историю про дракона'
            Check 'ollama stopped: the LLM path says the local model is unavailable' ($free.Ok -and $free.Text -match 'Локальная модель недоступна') $free.Text
        } finally { Finish-App $app }
    }
}

# ---------------------------------------------------------------- window: commands, safety, modes, recovery
if (Run-Group 'window') {
    Write-Host '== window: commands over CDP'
    if ($SkipIntrusive) {
        Add-Result 'window scenarios' 'SKIPPED' 'NOT VERIFIED: -SkipIntrusive'
    } elseif (-not (Test-SafeToIntrude)) {
        $info = Get-ForegroundInfo
        Add-Result 'window scenarios' 'SKIPPED' "NOT VERIFIED: foreground is $($info.Name) (fullscreen=$($info.Fullscreen))"
    } else {
        $ttsWav = Join-Path $work 'window-tts.wav'
        $env4 = @{ JARVIS_E2E_TTS_WAV = $ttsWav } + $sttEnv
        $app = Start-JarvisApp -Exe $Exe -Environment $env4 -Debugging
        try {
            $cdp = Connect-Cdp $app.Port
            [void](Hide-AppWindow $app 5)
            Install-EventCollector $cdp @('assistant://message', 'confirm://request', 'action://result')
            [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'tts.volume'; value = 0.05 })

            $t = Send-Phrase $cdp 'который час'
            Check 'time' ($t.Ok -and $t.Text -match '\d{1,2}[:. ]\d{2}|час') $t.Text
            $v = Send-Phrase $cdp 'какая сейчас громкость'
            $level = if ($v.Text -match '(\d{1,3})') { [int]$Matches[1] } else { -1 }
            Check 'volume: read' ($v.Ok -and $level -ge 0) $v.Text
            if ($level -ge 0) {
                $set = Send-Phrase $cdp "поставь громкость на $level"
                $again = Send-Phrase $cdp 'какая сейчас громкость'
                $level2 = if ($again.Text -match '(\d{1,3})') { [int]$Matches[1] } else { -2 }
                Check 'volume: set to the same level, unchanged' ($set.Ok -and [Math]::Abs($level2 - $level) -le 1) "$level -> $level2"
            }
            $g = Send-Phrase $cdp 'какая температура видеокарты'
            Check 'GPU temperature from real data' ($g.Ok -and $g.Text -match '\d') $g.Text

            $marker = 'e2e-7391'
            $r1 = Send-Phrase $cdp "запомни, что мой тестовый код $marker"
            Check 'memory: remember' $r1.Ok $r1.Text
            $r2 = Send-Phrase $cdp 'что ты помнишь'
            Check 'memory: recall' ($r2.Ok -and $r2.Text -match '7391') $r2.Text
            $f = Send-Phrase $cdp "забудь мой тестовый код $marker"
            if ($f.Ok -and $f.Confirm.Count -eq 0) {
                [void](Wait-Until { (Get-CollectedEvents $cdp 'confirm://request').Count -gt 0 } 5)
                $f.Confirm = @(Get-CollectedEvents $cdp 'confirm://request')
            }
            if ($f.Confirm.Count -gt 0) {
                [void](Invoke-TauriCommand $cdp 'confirm_response' @{ actionId = $f.Confirm[0].payload.action_id; approved = $true })
                Start-Sleep -Seconds 1
            }
            $r3 = Send-Phrase $cdp 'что ты помнишь'
            Check 'memory: forget (with confirmation)' ($f.Ok -and ($r3.Text -notmatch '7391')) "$($f.Text) | $($r3.Text)"

            # Recovery of the GPU helper: it exists while the window is open.
            [void](Invoke-TauriCommand $cdp 'system_snapshot')
            [void](Wait-Until { (Find-Children $app.Process.Id 'gpu-worker').Count -gt 0 } 20)
            $gpu = Find-Children $app.Process.Id 'gpu-worker'
            if ($gpu.Count -gt 0) {
                [void](Kill-Matching $app.Process.Id 'gpu-worker')
                Start-Sleep -Seconds 6
                $snap = Invoke-TauriCommand $cdp 'system_snapshot'
                Check 'recovery: metrics still flow after killing the GPU helper' ($snap.ok -and $snap.value.metrics.gpu -ne $null)
            } else {
                Add-Result 'recovery: GPU helper' 'SKIPPED' 'NOT VERIFIED: no gpu-worker child while the window was open'
            }

            # Recovery of Kokoro (English voice, Python worker).
            $kokoroReady = (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'Jarvis\models\kokoro\voices-v1.0.bin'))
            if ($kokoroReady) {
                [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'tts.engine_en'; value = 'kokoro' })
                [void](Invoke-TauriCommand $cdp 'tts_speak' @{ text = 'Hello, this is a test.' })
                $up = Wait-Until { (Find-Children $app.Process.Id 'kokoro').Count -gt 0 } 60
                if ($up) {
                    [void](Kill-Matching $app.Process.Id 'kokoro')
                    Start-Sleep -Seconds 1
                    Remove-Item -LiteralPath $ttsWav -Force -ErrorAction SilentlyContinue
                    $again = Invoke-TauriCommand $cdp 'tts_speak' @{ text = 'Hello again, this is a second test.' }
                    $wrote = Wait-Until { (Test-Path -LiteralPath $ttsWav) -and ((Get-Item -LiteralPath $ttsWav).Length -gt 8000) } 60
                    Check 'recovery: Kokoro answers after its worker was killed' ($again.ok -and $wrote)
                } else { Add-Result 'recovery: Kokoro' 'SKIPPED' 'NOT VERIFIED: the Kokoro worker did not start' }
            } else { Add-Result 'recovery: Kokoro' 'SKIPPED' 'NOT VERIFIED: Kokoro model is not installed' }

            # Piper recovery with a Russian phrase.
            [void](Invoke-TauriCommand $cdp 'tts_stop')
            Start-Sleep -Seconds 2
            [void](Invoke-TauriCommand $cdp 'tts_speak' @{ text = 'Проверка голоса. Это длинная проверочная фраза. И ещё одна фраза для проверки.' })
            Start-Sleep -Seconds 3
            [void](Kill-Matching $app.Process.Id 'piper')
            Remove-Item -LiteralPath $ttsWav -Force -ErrorAction SilentlyContinue
            $pp = Invoke-TauriCommand $cdp 'tts_speak' @{ text = 'Проверка после сбоя.' }
            $wrote = Wait-Until { (Test-Path -LiteralPath $ttsWav) -and ((Get-Item -LiteralPath $ttsWav).Length -gt 8000) } 30
            Check 'recovery: Piper answers after its process was killed' ($pp.ok -and $wrote) "ok=$($pp.ok) error=$($pp.error.message) wav=$(if (Test-Path -LiteralPath $ttsWav) { (Get-Item -LiteralPath $ttsWav).Length } else { 'none' })"

            # Secrets and logs: nothing sensitive in the log after all of the above.
            $log = Get-Log $app.DataDir
            Check 'log: no secrets, no api keys' ($log -notmatch '(?i)api[_-]?key\s*[=:]\s*\S{8,}|sk-[A-Za-z0-9]{20,}|bearer\s+[A-Za-z0-9._-]{20,}')
            $rows = Get-Rows $app.DataDir 'select group_concat(args || detail, char(32)) from audit_log'
            Check 'audit: no secret values' (($rows | Out-String) -notmatch 'sk-[A-Za-z0-9]{20,}')

            # Network: only loopback / Ollama connections of the whole process tree.
            $ids = @($app.Process.Id) + @(Get-ChildProcesses $app.Process.Id | ForEach-Object { $_.ProcessId })
            $listening = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess -and $_.LocalPort -ne $app.Port })
            Check 'network: the app listens on no port (the DevTools test port aside)' ($listening.Count -eq 0) (($listening | ForEach-Object { "$($_.LocalAddress):$($_.LocalPort)" }) -join ',')
            $outbound = @(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess -and $_.RemoteAddress -notin @('127.0.0.1', '::1') -and $_.RemotePort -ne $app.Port })
            Check 'network: no connections to remote hosts' ($outbound.Count -eq 0) (($outbound | ForEach-Object { "$($_.RemoteAddress):$($_.RemotePort)" }) -join ',')

            # The dangerous-action scenarios come last: afterwards the DevTools endpoint of the window is gone (the confirmation flow rebuilds the page).
            $danger = Send-Phrase $cdp 'выключи компьютер'
            if ($danger.Ok -and $danger.Confirm.Count -eq 0) {
                [void](Wait-Until { (Get-CollectedEvents $cdp 'confirm://request').Count -gt 0 } 5)
                $danger.Confirm = @(Get-CollectedEvents $cdp 'confirm://request')
            }
            Check 'dangerous action asks for confirmation' ($danger.Confirm.Count -gt 0) ($danger.Confirm | ForEach-Object { $_.payload.summary })
            if ($danger.Confirm.Count -gt 0) {
                $answer = Invoke-TauriCommand $cdp 'confirm_response' @{ actionId = $danger.Confirm[0].payload.action_id; approved = $false }
                Check 'dangerous action: refusal is accepted' $answer.ok
                $audit = Get-AuditActions $cdp 30
                Check 'no shutdown was scheduled' (-not ($audit | Where-Object { $_.action -eq 'power.shutdown' -and $_.ok -eq $true }))
            }
            $stop = Invoke-TauriCommand $cdp 'emergency_stop'
            Check 'emergency_stop' $stop.ok

        } finally { Finish-App $app }
    }
}

# Window modes: every change builds a new window and the DevTools session of the old one ends, so each mode gets its own launch.
if ((Run-Group 'window') -and -not $SkipIntrusive -and (Test-SafeToIntrude)) {
    Start-Sleep -Seconds 8
    foreach ($mode in 'tray', 'full') {
        $app = Start-JarvisApp -Exe $Exe -Environment (@{ JARVIS_E2E_TTS_WAV = (Join-Path $work "mode-$mode.wav") } + $sttEnv) -Debugging
        try {
            $cdp = Connect-Cdp $app.Port
            [void](Hide-AppWindow $app 5)
            # The tray mode destroys the window that carries the DevTools session, so the call may end with a dropped connection.
            $m = [pscustomobject]@{ ok = $true }
            try { $m = Invoke-TauriCommand $cdp 'set_window_mode' @{ mode = $mode } -NoReconnect } catch { if ($mode -ne 'tray') { throw } }
            Start-Sleep -Milliseconds 2000
            $app.Process.Refresh()
            # The tray keeps a hidden window of its own, so "no window" means: no WebView2 process left.
            $webviews = @(Get-ChildProcesses $app.Process.Id | Where-Object { $_.Name -eq 'msedgewebview2.exe' }).Count
            $hasWindow = $webviews -gt 0
            if ($mode -eq 'tray') { Check 'window mode: tray (the window is destroyed, the process stays)' ($m.ok -and -not $hasWindow -and -not $app.Process.HasExited) "ok=$($m.ok) webview2=$webviews exited=$($app.Process.HasExited)" }
            else { Check "window mode: $mode (a window exists)" ($m.ok -and $hasWindow) }
        } finally { Finish-App $app }
        Start-Sleep -Seconds 8
    }
}

# ---------------------------------------------------------------- llm: free question and the screen
if (Run-Group 'llm') {
    Write-Host '== llm: Ollama path (GPU) and the screen question'
    if ($SkipIntrusive) {
        Add-Result 'llm scenarios' 'SKIPPED' 'NOT VERIFIED: -SkipIntrusive'
    } elseif (-not (Test-SafeToIntrude)) {
        $info = Get-ForegroundInfo
        Add-Result 'llm scenarios' 'SKIPPED' "NOT VERIFIED: foreground is $($info.Name) (fullscreen=$($info.Fullscreen)); a model must not be loaded on the GPU now"
    } else {
        $ollamaUp = $false
        try { Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/version' -TimeoutSec 3 | Out-Null; $ollamaUp = $true } catch { }
        if (-not $ollamaUp) {
            Add-Result 'llm scenarios' 'SKIPPED' 'NOT VERIFIED: Ollama is not running'
        } else {
            $env5 = @{ JARVIS_E2E_TTS_WAV = (Join-Path $work 'llm-tts.wav') } + $sttEnv
            $app = Start-JarvisApp -Exe $Exe -Environment $env5 -Debugging
            $testWindow = $null
            try {
                $cdp = Connect-Cdp $app.Port
            [void](Hide-AppWindow $app 5)
                Install-EventCollector $cdp @('assistant://message', 'confirm://request')
                [void](Invoke-TauriCommand $cdp 'settings_set' @{ key = 'tts.volume'; value = 0.05 })
                $free = Send-Phrase $cdp 'Объясни одним предложением, почему небо голубое' 120
                Check 'LLM path: a free question gets an answer' ($free.Ok -and $free.Text.Length -gt 10 -and $free.Text -notmatch 'недоступна') $free.Text
                $hist = Get-Rows $app.DataDir "select route from command_history order by id desc limit 1"
                Check 'LLM path: recorded with the local route' (($hist | Out-String) -match 'llm')
                # Our own window with a marker; the foreground window is read, so it must be this one.
                $testWindow = Start-Process powershell -PassThru -ArgumentList '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $root 'tests\e2e\testwindow.ps1'), '-Marker', 'JARVIS TEST 4711', '-Seconds', '90'
                $focused = Wait-Until { (Get-ForegroundInfo).Pid -eq $testWindow.Id } 10
                if ($focused) {
                    $read = Send-Phrase $cdp 'что написано в этом окне' 60
                    Check 'screen: OCR of our own test window (its title is read from the foreground window)' ($read.Ok -and $read.Text -match 'E2E TEST WINDOW') $read.Text
                    $desc = Send-Phrase $cdp 'что у меня на экране' 180
                    Check 'screen: what is on the screen (VLM)' ($desc.Ok -and $desc.Text.Length -gt 10) $desc.Text
                } else {
                    Add-Result 'screen question' 'SKIPPED' 'NOT VERIFIED: the test window did not receive focus, so the foreground window is not ours'
                }
                [void](Invoke-TauriCommand $cdp 'ai_unload')
            } finally {
                if ($testWindow -and -not $testWindow.HasExited) { Stop-Process -Id $testWindow.Id -Force }
                Finish-App $app
            }
        }
    }
}

Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
Write-Host ''
Write-Host '==== e2e summary ===='
$results | Format-Table -AutoSize -Wrap | Out-String -Width 200 | Write-Host
$failed = @($results | Where-Object { $_.Status -eq 'FAIL' })
$skipped = @($results | Where-Object { $_.Status -eq 'SKIPPED' })
Write-Host ("PASS {0}  FAIL {1}  NOT VERIFIED {2}" -f @($results | Where-Object { $_.Status -eq 'PASS' }).Count, $failed.Count, $skipped.Count)
if ($failed.Count -gt 0) { exit 1 }
exit 0

