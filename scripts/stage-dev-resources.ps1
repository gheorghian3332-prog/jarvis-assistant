# Creates app\src-tauri\resources for development builds and checks (cargo check/clippy/test of the Tauri crate).
# tauri-build refuses to compile when a declared bundle resource is missing. This script copies the real libvosk DLLs
# and the worker sources, and puts an EMPTY placeholder in place of uv.exe: good enough to compile and test, NOT for
# an installer. The installer is built with scripts\build-installer.ps1, which stages the real, hash-checked uv.exe.
# Run scripts\fetch-models.ps1 first (it provides libvosk).
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$models = $env:JARVIS_MODELS_DIR
if (-not $models) { $models = Join-Path $env:LOCALAPPDATA 'Jarvis\models' }
$libvosk = if ($env:JARVIS_VOSK_LIB_DIR) { $env:JARVIS_VOSK_LIB_DIR } else { Join-Path $models 'libvosk' }
$stage = Join-Path $root 'app\src-tauri\resources'
New-Item -ItemType Directory -Force -Path $stage | Out-Null
foreach ($name in 'libvosk.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll') {
    $source = Join-Path $libvosk $name
    if (-not (Test-Path -LiteralPath $source)) { throw "$name missing in $libvosk (run scripts\fetch-models.ps1)" }
    Copy-Item -LiteralPath $source -Destination (Join-Path $stage $name) -Force
}
$uv = Join-Path $stage 'uv.exe'
if (-not (Test-Path -LiteralPath $uv)) { New-Item -ItemType File -Path $uv | Out-Null }
foreach ($worker in 'stt', 'tts_kokoro') {
    $from = Join-Path $root "workers\$worker"
    $to = Join-Path $stage "workers\$worker"
    New-Item -ItemType Directory -Force -Path $to | Out-Null
    foreach ($file in 'pyproject.toml', 'uv.lock') { Copy-Item -LiteralPath (Join-Path $from $file) -Destination $to -Force }
    $package = Get-ChildItem -LiteralPath $from -Directory | Where-Object { $_.Name -like 'jarvis_*' } | Select-Object -First 1
    Copy-Item -LiteralPath $package.FullName -Destination $to -Recurse -Force
}
Write-Host "Staged development resources in $stage"
