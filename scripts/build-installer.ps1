# Builds the per-user NSIS installer (no administrator rights, no Ollama, no models inside).
#   1. uv.exe: official astral-sh/uv release, accepted only with the pinned SHA256
#   2. stages libvosk DLLs, uv.exe and the worker sources (workers/stt, workers/tts_kokoro with uv.lock)
#      into app\src-tauri\resources (git-ignored)
#   3. `tauri build --bundles nsis` -> target\release\bundle\nsis\*.exe
# Runs at below-normal priority so the machine stays usable.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
(Get-Process -Id $PID).PriorityClass = 'BelowNormal'

$uvVersion = '0.13.8'
$uvUrl = "https://github.com/astral-sh/uv/releases/download/$uvVersion/uv-x86_64-pc-windows-msvc.zip"
$uvSha256 = '529E78A9CEC6CD7E3E014F7ECD8BDA0C113969D4ED0F2EFEEDB85C30943325B8'

$models = $env:JARVIS_MODELS_DIR
if (-not $models) { $models = Join-Path $env:LOCALAPPDATA 'Jarvis\models' }
$libvosk = if ($env:JARVIS_VOSK_LIB_DIR) { $env:JARVIS_VOSK_LIB_DIR } else { Join-Path $models 'libvosk' }
$stage = Join-Path $root 'app\src-tauri\resources'
$cache = Join-Path $root 'target\installer-cache'
New-Item -ItemType Directory -Force -Path $cache | Out-Null
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# 1. uv.exe, verified.
$uvZip = Join-Path $cache "uv-$uvVersion-x86_64-pc-windows-msvc.zip"
if (-not ((Test-Path -LiteralPath $uvZip) -and ((Get-FileHash -LiteralPath $uvZip -Algorithm SHA256).Hash -eq $uvSha256))) {
    Write-Host "Downloading $uvUrl"
    & curl.exe -L --fail --silent --show-error -o $uvZip $uvUrl
    if ($LASTEXITCODE -ne 0) { throw 'uv download failed' }
}
$actual = (Get-FileHash -LiteralPath $uvZip -Algorithm SHA256).Hash
if ($actual -ne $uvSha256) { Remove-Item -LiteralPath $uvZip -Force; throw "uv SHA256 mismatch: expected $uvSha256, got $actual" }
$uvTmp = Join-Path $cache 'uv-extract'
if (Test-Path -LiteralPath $uvTmp) { Remove-Item -LiteralPath $uvTmp -Recurse -Force }
Expand-Archive -LiteralPath $uvZip -DestinationPath $uvTmp -Force
$uvExe = Get-ChildItem -LiteralPath $uvTmp -Recurse -Filter 'uv.exe' | Select-Object -First 1
if (-not $uvExe) { throw 'uv.exe not found in the archive' }
Copy-Item -LiteralPath $uvExe.FullName -Destination (Join-Path $stage 'uv.exe')

# 2. libvosk and its runtime DLLs.
foreach ($name in 'libvosk.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll') {
    $source = Join-Path $libvosk $name
    if (-not (Test-Path -LiteralPath $source)) { throw "$name missing in $libvosk (run scripts\fetch-models.ps1)" }
    Copy-Item -LiteralPath $source -Destination (Join-Path $stage $name)
}

# 3. Worker sources with their lock files (no .venv, caches or tests).
foreach ($worker in 'stt', 'tts_kokoro') {
    $from = Join-Path $root "workers\$worker"
    $to = Join-Path $stage "workers\$worker"
    New-Item -ItemType Directory -Force -Path $to | Out-Null
    foreach ($file in 'pyproject.toml', 'uv.lock') { Copy-Item -LiteralPath (Join-Path $from $file) -Destination $to }
    $package = Get-ChildItem -LiteralPath $from -Directory | Where-Object { $_.Name -like 'jarvis_*' } | Select-Object -First 1
    Copy-Item -LiteralPath $package.FullName -Destination $to -Recurse
    Get-ChildItem -LiteralPath $to -Recurse -Directory -Filter '__pycache__' | Remove-Item -Recurse -Force
}

# 4. Frontend + bundle. custom-protocol embeds app/dist (without it the exe would look for the dev server).
$app = Join-Path $root 'app'
if (-not (Test-Path -LiteralPath (Join-Path $app 'node_modules'))) {
    & npm --prefix $app ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
}
$env:Path = "$libvosk;" + $env:Path
Get-ChildItem -LiteralPath (Join-Path $root 'target\release\bundle\nsis') -Filter '*.exe' -ErrorAction SilentlyContinue | Remove-Item -Force
# Privacy: Rust embeds absolute source paths (panic locations) in the binary. Remap the cargo home,
# the rustup home and the checkout so no local user name or folder ends up in the shipped exe.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$env:RUSTFLAGS = "--remap-path-prefix=$cargoHome=/cargo --remap-path-prefix=$rustupHome=/rustup --remap-path-prefix=$root=/jarvis"
# C/C++ dependencies (e.g. aws-lc-sys) embed __FILE__; MSVC /d1trimfile strips these prefixes.
$env:CFLAGS = "/d1trimfile:$cargoHome\ /d1trimfile:$root\"
$env:CXXFLAGS = $env:CFLAGS
$ErrorActionPreference = 'Continue'  # tauri prints progress on stderr; only the exit code matters
& npm --prefix $app run tauri -- build --bundles nsis -- --features custom-protocol
if ($LASTEXITCODE -ne 0) { throw "tauri build failed (exit $LASTEXITCODE)" }
$ErrorActionPreference = 'Stop'

$setup = Get-ChildItem -LiteralPath (Join-Path $root 'target\release\bundle\nsis') -Filter '*.exe' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { throw 'installer not found' }
# Release file name: JARVIS-Assistant_<version>_x64-setup.exe
$version = (Get-Content -LiteralPath (Join-Path $app 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
$final = Join-Path $setup.DirectoryName "JARVIS-Assistant_${version}_x64-setup.exe"
if ($setup.FullName -ne $final) { Move-Item -LiteralPath $setup.FullName -Destination $final -Force; $setup = Get-Item -LiteralPath $final }
Write-Host ("Installer: {0} ({1:N1} MB)" -f $setup.FullName, ($setup.Length / 1MB))
exit 0
