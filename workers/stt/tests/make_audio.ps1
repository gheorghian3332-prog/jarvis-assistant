# Generates the offline test phrases with Windows SAPI (System.Speech): 16 kHz, 16-bit, mono WAV.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$out = Join-Path $PSScriptRoot 'audio'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$phrases = @(
    @{ File = 'ru_open_browser.wav'; Voice = 'Microsoft Irina Desktop'; Text = 'Открой браузер и включи музыку' },
    @{ File = 'en_open_browser.wav'; Voice = 'Microsoft Zira Desktop'; Text = 'Open the browser and turn on the music' }
)
foreach ($phrase in $phrases) {
    $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
    try {
        $synth.SelectVoice($phrase.Voice)
        $synth.SetOutputToWaveFile((Join-Path $out $phrase.File), $format)
        $synth.Speak($phrase.Text)
    } finally {
        $synth.Dispose()
    }
}
