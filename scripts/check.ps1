$ErrorActionPreference = 'Stop'
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)

# The voice front links libvosk and its tests load the Vosk model: run scripts\fetch-models.ps1 once.
# The libvosk folder goes on PATH so that test executables (which live in cargo's build folders) find the DLLs.
$models = $env:JARVIS_MODELS_DIR
if (-not $models) { $models = Join-Path $env:LOCALAPPDATA 'Jarvis\models' }
$libvosk = if ($env:JARVIS_VOSK_LIB_DIR) { $env:JARVIS_VOSK_LIB_DIR } else { Join-Path $models 'libvosk' }
if (-not (Test-Path -LiteralPath (Join-Path $libvosk 'libvosk.lib')) -or
    -not (Test-Path -LiteralPath (Join-Path $models 'vosk-model-small-ru-0.22\am\final.mdl'))) {
    Write-Host 'FAILED: Vosk library or model missing. Run scripts\fetch-models.ps1 first.'
    exit 1
}
$env:Path = "$libvosk;" + $env:Path
& (Join-Path $PSScriptRoot 'stage-dev-resources.ps1')  # tauri-build needs the bundle resources to exist

# Frontend first: the Tauri crate embeds app/dist at compile time, so cargo needs the build output.
$app = Join-Path (Get-Location) 'app'
if (-not (Test-Path -LiteralPath (Join-Path $app 'node_modules'))) {
    Write-Host '==> npm ci (app)'
    & npm --prefix $app ci
    if ($LASTEXITCODE -ne 0) { Write-Host "FAILED: npm ci (exit $LASTEXITCODE)"; exit $LASTEXITCODE }
}
foreach ($script in @('typecheck', 'lint', 'test', 'build')) {
    Write-Host "==> npm run $script (app)"
    & npm --prefix $app run $script
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: npm run $script (exit $LASTEXITCODE)"
        exit $LASTEXITCODE
    }
}

$steps = @(
    @('fmt', @('fmt', '--check')),
    @('clippy', @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')),
    @('test', @('test', '--workspace'))
)
foreach ($step in $steps) {
    Write-Host "==> cargo $($step[0])"
    & cargo @($step[1])
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: cargo $($step[0]) (exit $LASTEXITCODE)"
        exit $LASTEXITCODE
    }
}
# Python workers (workers/<name>/pyproject.toml): ruff, mypy, pytest (slow model tests are opt-in via -m slow).
foreach ($project in Get-ChildItem -LiteralPath (Join-Path (Get-Location) 'workers') -Directory -ErrorAction SilentlyContinue) {
    if (-not (Test-Path -LiteralPath (Join-Path $project.FullName 'pyproject.toml'))) { continue }
    Push-Location -LiteralPath $project.FullName
    try {
        foreach ($tool in @(@('ruff', 'check', '.'), @('ruff', 'format', '--check', '.'), @('mypy', '.'), @('pytest'))) {
            Write-Host "==> uv run $($tool -join ' ') (workers/$($project.Name))"
            & uv run @tool
            if ($LASTEXITCODE -ne 0) {
                Write-Host "FAILED: uv run $($tool -join ' ') in workers/$($project.Name) (exit $LASTEXITCODE)"
                exit $LASTEXITCODE
            }
        }
    } finally {
        Pop-Location
    }
}
Write-Host 'All checks passed.'

exit 0
