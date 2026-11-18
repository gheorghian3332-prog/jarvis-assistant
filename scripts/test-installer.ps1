# Installer check: silent per-user install into a temporary folder, start the installed jarvis.exe --tray with a
# temporary data folder, check that the wizard sees the already downloaded models (JARVIS_MODELS_DIR) and that the
# worker environment can be created with the shipped uv.exe, then uninstall silently and verify that the program
# files are gone while the data stays. No administrator rights, no window, no sound.
[CmdletBinding()]
param(
    [string]$Setup,
    [string]$ModelsDir
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Setup) { $Setup = (Get-ChildItem -LiteralPath (Join-Path $root 'target\release\bundle\nsis') -Filter '*.exe' | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName }
# Outside %LOCALAPPDATA%/%TEMP%: a process started from the Claude app (MSIX) sees those folders virtualized.
$work = Join-Path $root ('target\installer-test-' +[guid]::NewGuid().ToString('N').Substring(0, 8))
$install = Join-Path $work 'app'
$data = Join-Path $work 'data'
$workers = Join-Path $work 'workers'
New-Item -ItemType Directory -Force -Path $work, $data, $workers | Out-Null
# A private models folder with the marker files the wizard looks for: the real models are never touched.
if (-not $ModelsDir) {
    $ModelsDir = Join-Path $work 'models'
    foreach ($rel in 'vosk-model-small-ru-0.22\am\final.mdl', 'piper\piper\piper.exe', 'piper\ru_RU-dmitri-medium.onnx', 'piper\ru_RU-dmitri-medium.onnx.json') {
        $file = Join-Path $ModelsDir $rel
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $file) | Out-Null
        Set-Content -LiteralPath $file -Value 'marker'
    }
}
$failures = 0
function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
    if ($Ok) { Write-Host "[PASS] $Name $Detail" -ForegroundColor Green } else { $script:failures++; Write-Host "[FAIL] $Name $Detail" -ForegroundColor Red }
}
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Check 'this check runs without administrator rights' (-not $isAdmin)
Write-Host ("Installer: {0} ({1:N2} MB)" -f $Setup, ((Get-Item -LiteralPath $Setup).Length / 1MB))

# 1. Install (per-user, silent). /D must be the last argument and unquoted.
$p = Start-Process -FilePath $Setup -ArgumentList '/S', "/D=$install" -PassThru -Wait
Check 'silent install exits with 0' ($p.ExitCode -eq 0) "exit $($p.ExitCode)"
$exe = Join-Path $install 'jarvis.exe'
foreach ($rel in 'jarvis.exe', 'libvosk.dll', 'uv.exe', 'uninstall.exe', 'resources\workers\stt\pyproject.toml', 'resources\workers\stt\uv.lock',
    'resources\workers\tts_kokoro\pyproject.toml', 'resources\workers\tts_kokoro\uv.lock', 'resources\workers\stt\jarvis_stt\__main__.py') {
    Check "installed: $rel" (Test-Path -LiteralPath (Join-Path $install $rel))
}
$shortcuts = @(Get-ChildItem -LiteralPath ([Environment]::GetFolderPath('Programs')) -Recurse -Filter '*.lnk' -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'J.A.R.V.I.S*' })
Check 'Start menu shortcut exists' ($shortcuts.Count -ge 1) (($shortcuts | ForEach-Object { $_.FullName }) -join ';')
Check 'no desktop shortcut after a silent install (it is opt-in on the finish page)' (-not (Test-Path -LiteralPath (Join-Path ([Environment]::GetFolderPath('Desktop')) 'J.A.R.V.I.S..lnk')))
Check 'no jarvisd.exe shipped' (-not (Test-Path -LiteralPath (Join-Path $install 'jarvisd.exe')))
Check 'the manifest of the installed exe asks for asInvoker' ([Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe)).Contains('asInvoker'))

# 2. The wizard sees the models that are already there.
$env:JARVIS_DATA_DIR = $data
$env:JARVIS_MODELS_DIR = $ModelsDir
$env:JARVIS_WORKERS_DIR = $workers
$statusFile = Join-Path $work 'status.json'
$s = Start-Process -FilePath $exe -ArgumentList '--setup-status', $statusFile -PassThru -Wait
Check 'jarvis.exe --setup-status exits with 0' ($s.ExitCode -eq 0)
$status = Get-Content -LiteralPath $statusFile -Raw | ConvertFrom-Json
Check 'wizard: installed layout recognised (resources next to the exe, not JARVIS_REPO_ROOT)' ($status.installed_layout -eq $true)
foreach ($id in 'vosk_model', 'piper', 'piper_voice') {
    $item = $status.items | Where-Object { $_.id -eq $id }
    $present = switch ($id) {
        'vosk_model' { Test-Path -LiteralPath (Join-Path $ModelsDir 'vosk-model-small-ru-0.22\am\final.mdl') }
        'piper' { Test-Path -LiteralPath (Join-Path $ModelsDir 'piper\piper\piper.exe') }
        'piper_voice' { Test-Path -LiteralPath (Join-Path $ModelsDir 'piper\ru_RU-dmitri-medium.onnx') }
    }
    Check "wizard: $id is recognised in JARVIS_MODELS_DIR" (($item.installed -eq $true) -and [bool]$present) "installed=$($item.installed)"
}
$stt = $status.items | Where-Object { $_.id -eq 'stt_env' }
Check 'wizard: the STT environment is reported missing in a clean workers folder' ($stt.installed -eq $false)

# 3. Start the installed app in the tray with the temporary data folder.
$app = Start-Process -FilePath $exe -ArgumentList '--tray' -PassThru
try {
    Start-Sleep -Seconds 8
    Check 'installed jarvis.exe --tray is running' (-not $app.HasExited) "pid $($app.Id)"
    $logFile = Join-Path $data 'logs\jarvis.log'
    $log = ''
    if (Test-Path -LiteralPath $logFile) {
        $stream = New-Object IO.FileStream($logFile, 'Open', 'Read', 'ReadWrite, Delete')
        try { $log = (New-Object IO.StreamReader($stream)).ReadToEnd() } finally { $stream.Dispose() }
    }
    Check 'log: the core started' ($log -match 'jarvis starting')
    Check 'log: the installed STT worker is used (not a development checkout)' ($log -match 'installed speech-to-text worker')
    Check 'log: no development-checkout lookup' ($log -notmatch 'speech-to-text worker found root=')
} finally {
    if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force }
}

# 4. The shipped uv creates a worker environment from the shipped sources and lock file.
$uv = Join-Path $install 'uv.exe'
$src = Join-Path $install 'resources\workers\tts_kokoro'
$dst = Join-Path $workers 'tts_kokoro'
Copy-Item -LiteralPath $src -Destination $dst -Recurse
Push-Location -LiteralPath $dst
try {
    (Get-Process -Id $PID).PriorityClass = 'BelowNormal'
    $ErrorActionPreference = 'Continue'
    & $uv sync --frozen --no-dev 2>&1 | Select-Object -Last 3 | ForEach-Object { Write-Host "  uv: $_" }
    $uvExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    Check 'uv sync --frozen created the worker environment' ((Test-Path -LiteralPath (Join-Path $dst '.venv\Scripts\python.exe')) -and ($uvExit -eq 0))
    $ErrorActionPreference = 'Continue'
    $import = & (Join-Path $dst '.venv\Scripts\python.exe') -c "import kokoro_onnx, numpy; print('ok')" 2>&1
    $ErrorActionPreference = 'Stop'
    Check 'the worker environment imports its dependencies' ($import -match 'ok')
} finally { Pop-Location }

# 5. Uninstall silently: program files go, data and models stay.
$u = Start-Process -FilePath (Join-Path $install 'uninstall.exe') -ArgumentList '/S' -PassThru -Wait
$gone = $false
for ($i = 0; $i -lt 60; $i++) { if (-not (Test-Path -LiteralPath $exe)) { $gone = $true; break }; Start-Sleep -Milliseconds 500 }
Check 'uninstall removed jarvis.exe' $gone
Start-Sleep -Seconds 2
Check 'uninstall removed the program folder content' (-not (Test-Path -LiteralPath (Join-Path $install 'resources')))
$shortcutsAfter = @(Get-ChildItem -LiteralPath ([Environment]::GetFolderPath('Programs')) -Recurse -Filter '*.lnk' -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'J.A.R.V.I.S*' })
Check 'uninstall removed the Start menu shortcut' ($shortcutsAfter.Count -eq 0)
Check 'user data stays (silent uninstall never deletes it)' (Test-Path -LiteralPath (Join-Path $data 'jarvis.db'))
Check 'models stay' (Test-Path -LiteralPath (Join-Path $ModelsDir 'vosk-model-small-ru-0.22'))
Check 'worker environments stay' (Test-Path -LiteralPath (Join-Path $workers 'tts_kokoro\.venv'))

Remove-Item Env:JARVIS_DATA_DIR, Env:JARVIS_MODELS_DIR, Env:JARVIS_WORKERS_DIR -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
if ($failures -gt 0) { Write-Host "FAILED: $failures check(s)"; exit 1 }
Write-Host 'Installer check passed.'
exit 0
