# Downloads what the voice front needs into %LOCALAPPDATA%\Jarvis\models (nothing is committed to git):
#   libvosk\                       Vosk 0.3.45 win64 (libvosk.dll + import library, needed to build and run)
#   vosk-model-small-ru-0.22\      Vosk small Russian model (wake word)
#   piper\ (only with -Piper)      Piper TTS + voice ru_RU-dmitri-medium (Russian speech output, make-test-wavs.ps1)
#   kokoro\ (only with -Kokoro)    Kokoro v1.0 model (326 MB) + voices (28 MB) for the English speech output
# Every archive is checked against a pinned SHA256 before it is used. Run it again at any time: finished
# parts are skipped. Set JARVIS_MODELS_DIR to use another folder.
[CmdletBinding()]
param(
    [switch]$Piper,
    [switch]$Kokoro,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$models = $env:JARVIS_MODELS_DIR
if (-not $models) { $models = Join-Path $env:LOCALAPPDATA 'Jarvis\models' }
New-Item -ItemType Directory -Force -Path $models | Out-Null
$download = Join-Path $models '_download'
New-Item -ItemType Directory -Force -Path $download | Out-Null

$items = @{
    vosk = @{
        Url    = 'https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-win64-0.3.45.zip'
        File   = 'vosk-win64-0.3.45.zip'
        Sha256 = 'F1DCC9CCA460630F81EA8F71794F69C80BED6556D2A4E6237B5785E1D2DFF34B'
    }
    model = @{
        Url    = 'https://alphacephei.com/vosk/models/vosk-model-small-ru-0.22.zip'
        File   = 'vosk-model-small-ru-0.22.zip'
        Sha256 = '961D5FF98A17F4AA6DE69864D0AA71FA5BAC682301D2B5D17A3F24C5C99A46D4'
    }
    piper = @{
        Url    = 'https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip'
        File   = 'piper_windows_amd64.zip'
        Sha256 = 'F3C58906402B24F3A96D92145F58ACBA6D86C9B5DB896D207F78DC80811EFCEA'
    }
    voice = @{
        Url    = 'https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/dmitri/medium/ru_RU-dmitri-medium.onnx'
        File   = 'ru_RU-dmitri-medium.onnx'
        Sha256 = '9ED9941BAC7373F2EC1BE36627E0E71A349A59CA5522D4363FD50701DB05193A'
    }
    kokoroModel = @{
        Url    = 'https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/kokoro-v1.0.onnx'
        File   = 'kokoro-v1.0.onnx'
        Sha256 = 'BEB0D1848DEE9A49DA392CC3DF26958D46CFA35D321EDF434F52949153F0DF3A'
    }
    kokoroVoices = @{
        Url    = 'https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/voices-v1.0.bin'
        File   = 'voices-v1.0.bin'
        Sha256 = 'BCA610B8308E8D99F32E6FE4197E7EC01679264EFED0CAC9140FE9C29F1FBF7D'
    }
    voiceJson = @{
        Url    = 'https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/dmitri/medium/ru_RU-dmitri-medium.onnx.json'
        File   = 'ru_RU-dmitri-medium.onnx.json'
        Sha256 = 'C3741C9C1558FC38E902FD80D39155C32A7D019D7D17E3A43EAC66B7BD3FBE09'
    }
}

function Get-Verified([string]$Key) {
    $item = $items[$Key]
    $path = Join-Path $download $item.File
    if (Test-Path -LiteralPath $path) {
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -eq $item.Sha256) { return $path }
        Remove-Item -LiteralPath $path -Force
    }
    Write-Host "Downloading $($item.Url)"
    & curl.exe -L --fail --silent --show-error -o $path $item.Url
    if ($LASTEXITCODE -ne 0) { throw "download failed: $($item.Url)" }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
    if ($actual -ne $item.Sha256) {
        Remove-Item -LiteralPath $path -Force
        throw "SHA256 mismatch for $($item.File): expected $($item.Sha256), got $actual"
    }
    return $path
}

function Expand-Clean([string]$Zip, [string]$Target) {
    $temporary = Join-Path $download ([IO.Path]::GetRandomFileName())
    Expand-Archive -LiteralPath $Zip -DestinationPath $temporary -Force
    if (Test-Path -LiteralPath $Target) { Remove-Item -LiteralPath $Target -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Target) | Out-Null
    $root = @(Get-ChildItem -LiteralPath $temporary)
    if ($root.Count -eq 1 -and $root[0].PSIsContainer) {
        Move-Item -LiteralPath $root[0].FullName -Destination $Target
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
    else {
        Move-Item -LiteralPath $temporary -Destination $Target
    }
}

$libvosk = Join-Path $models 'libvosk'
if ($Force -or -not (Test-Path -LiteralPath (Join-Path $libvosk 'libvosk.lib'))) {
    Expand-Clean (Get-Verified 'vosk') $libvosk
}
else { Write-Host 'libvosk: present' }

$modelDir = Join-Path $models 'vosk-model-small-ru-0.22'
if ($Force -or -not (Test-Path -LiteralPath (Join-Path $modelDir 'am\final.mdl'))) {
    Expand-Clean (Get-Verified 'model') $modelDir
}
else { Write-Host 'vosk-model-small-ru-0.22: present' }

if ($Piper) {
    $piperDir = Join-Path $models 'piper'
    if ($Force -or -not (Test-Path -LiteralPath (Join-Path $piperDir 'piper\piper.exe'))) {
        New-Item -ItemType Directory -Force -Path $piperDir | Out-Null
        Expand-Clean (Get-Verified 'piper') (Join-Path $piperDir 'piper')
    }
    else { Write-Host 'piper: present' }
    foreach ($key in 'voice', 'voiceJson') {
        $target = Join-Path $piperDir $items[$key].File
        if ($Force -or -not (Test-Path -LiteralPath $target)) {
            Copy-Item -LiteralPath (Get-Verified $key) -Destination $target -Force
        }
    }
}

if ($Kokoro) {
    $kokoroDir = Join-Path $models 'kokoro'
    New-Item -ItemType Directory -Force -Path $kokoroDir | Out-Null
    foreach ($key in 'kokoroModel', 'kokoroVoices') {
        $target = Join-Path $kokoroDir $items[$key].File
        if ($Force -or -not (Test-Path -LiteralPath $target)) {
            Copy-Item -LiteralPath (Get-Verified $key) -Destination $target -Force
        }
        else { Write-Host "kokoro\$($items[$key].File): present" }
    }
}

Write-Host "Models are in $models"
exit 0
