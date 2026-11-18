# Builds tests/audio/*.wav (16 kHz mono PCM16) for the wake word tests:
# phrases are synthesized locally with Piper (voice ru_RU-dmitri-medium), plus silence and white noise.
# Needs the models from `scripts/fetch-models.ps1 -Piper` (run automatically if they are missing).
# Nothing leaves the machine; no microphone is used.
[CmdletBinding()]
param(
    [string]$OutDir
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $root 'tests\audio' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$models = $env:JARVIS_MODELS_DIR
if (-not $models) { $models = Join-Path $env:LOCALAPPDATA 'Jarvis\models' }
$piperExe = Join-Path $models 'piper\piper\piper.exe'
$voice = Join-Path $models 'piper\ru_RU-dmitri-medium.onnx'
if (-not (Test-Path -LiteralPath $piperExe) -or -not (Test-Path -LiteralPath $voice)) {
    & (Join-Path $PSScriptRoot 'fetch-models.ps1') -Piper
}

# piper 2023.11.14-2 (the last Windows build) rejects the multi-character phoneme keys of newer voice files
# (for example "ai" written with an IPA vowel) with "is not a single codepoint". Use a copy without them; the model is untouched.
$compatConfig = Join-Path ([IO.Path]::GetTempPath()) ('jarvis-piper-' + [IO.Path]::GetRandomFileName() + '.json')
$configText = [IO.File]::ReadAllText($voice + '.json', (New-Object Text.UTF8Encoding($false)))
$configText = [regex]::Replace($configText, ',\s*"[^"\\]{2,}":\s*\[\s*\d+\s*\]', '')
[IO.File]::WriteAllText($compatConfig, $configText, (New-Object Text.UTF8Encoding($false)))

Add-Type -TypeDefinition @'
using System;
using System.IO;

public static class WavTools
{
    // Reads a PCM16 mono WAV and returns samples and the sample rate.
    public static short[] Read(string path, out int rate)
    {
        byte[] data = File.ReadAllBytes(path);
        rate = BitConverter.ToInt32(data, 24);
        int position = 12;
        while (position + 8 <= data.Length)
        {
            string id = System.Text.Encoding.ASCII.GetString(data, position, 4);
            int size = BitConverter.ToInt32(data, position + 4);
            if (id == "data")
            {
                int available = Math.Min(size, data.Length - position - 8);
                short[] samples = new short[available / 2];
                Buffer.BlockCopy(data, position + 8, samples, 0, samples.Length * 2);
                return samples;
            }
            position += 8 + size + (size & 1);
        }
        throw new InvalidDataException("no data chunk in " + path);
    }

    // Windowed-sinc resampler (Hann window, cutoff just below the new Nyquist).
    public static short[] Resample(short[] input, int inRate, int outRate)
    {
        if (inRate == outRate) return input;
        double ratio = (double)inRate / outRate;
        double cutoff = Math.Min(1.0, (double)outRate / inRate) * 0.95;
        int half = (int)Math.Ceiling(24.0 / cutoff);
        int length = (int)((long)input.Length * outRate / inRate);
        short[] output = new short[length];
        for (int i = 0; i < length; i++)
        {
            double center = i * ratio;
            int first = (int)Math.Floor(center) - half;
            int last = (int)Math.Floor(center) + half;
            double sum = 0;
            for (int j = first; j <= last; j++)
            {
                if (j < 0 || j >= input.Length) continue;
                double x = (j - center) * cutoff;
                double sinc = Math.Abs(x) < 1e-9 ? 1.0 : Math.Sin(Math.PI * x) / (Math.PI * x);
                double window = 0.5 + 0.5 * Math.Cos(Math.PI * (j - center) / (half + 1));
                sum += input[j] * sinc * window * cutoff;
            }
            output[i] = (short)Math.Max(-32768, Math.Min(32767, Math.Round(sum)));
        }
        return output;
    }

    public static void Write(string path, short[] samples, int rate)
    {
        using (FileStream stream = File.Create(path))
        using (BinaryWriter writer = new BinaryWriter(stream))
        {
            int bytes = samples.Length * 2;
            writer.Write(System.Text.Encoding.ASCII.GetBytes("RIFF"));
            writer.Write(36 + bytes);
            writer.Write(System.Text.Encoding.ASCII.GetBytes("WAVEfmt "));
            writer.Write(16);
            writer.Write((short)1);
            writer.Write((short)1);
            writer.Write(rate);
            writer.Write(rate * 2);
            writer.Write((short)2);
            writer.Write((short)16);
            writer.Write(System.Text.Encoding.ASCII.GetBytes("data"));
            writer.Write(bytes);
            byte[] raw = new byte[bytes];
            Buffer.BlockCopy(samples, 0, raw, 0, bytes);
            writer.Write(raw);
        }
    }

    // Deterministic white noise (fixed seed) so that the file is reproducible.
    public static short[] Noise(int count, int amplitude)
    {
        Random random = new Random(20241118);
        short[] samples = new short[count];
        for (int i = 0; i < count; i++) samples[i] = (short)random.Next(-amplitude, amplitude + 1);
        return samples;
    }

    public static short[] Pad(short[] input, int before, int after)
    {
        short[] output = new short[before + input.Length + after];
        Array.Copy(input, 0, output, before, input.Length);
        return output;
    }
}
'@

$rate = 16000
$phrases = [ordered]@{
    'wake_open_notepad'   = 'Лёха, открой блокнот'
    'wake_only'           = 'Лёха'
    'alexey_louder'       = 'Алексей, сделай громче'
    'wake_time'           = 'Лёха, скажи, сколько сейчас времени'
    'neg_hello'           = 'привет, как дела'
    'neg_name_mid_phrase' = 'вчера мы с друзьями долго обсуждали, что леха опять опоздал на встречу и никому ничего не объяснил'
}
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('jarvis-wavs-' + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $temporary | Out-Null
try {
    foreach ($name in $phrases.Keys) {
        $textFile = Join-Path $temporary "$name.txt"
        $rawWav = Join-Path $temporary "$name.wav"
        $errorFile = Join-Path $temporary "$name.err"
        [IO.File]::WriteAllText($textFile, $phrases[$name], (New-Object Text.UTF8Encoding($false)))
        $process = Start-Process -FilePath $piperExe -ArgumentList @('--model', $voice, '--config', $compatConfig, '--output_file', $rawWav) `
            -RedirectStandardInput $textFile -RedirectStandardError $errorFile -NoNewWindow -PassThru -Wait
        if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $rawWav)) {
            throw "piper failed for '$name' (exit $($process.ExitCode)): $(Get-Content -LiteralPath $errorFile -Raw)"
        }
        $sourceRate = 0
        $samples = [WavTools]::Read($rawWav, [ref]$sourceRate)
        $resampled = [WavTools]::Resample($samples, $sourceRate, $rate)
        # Half a second of room before and after the phrase.
        $padded = [WavTools]::Pad($resampled, [int]($rate * 0.5), [int]($rate * 0.5))
        [WavTools]::Write((Join-Path $OutDir "$name.wav"), $padded, $rate)
        Write-Host ("{0}.wav  {1:N2} s  '{2}'" -f $name, ($padded.Length / $rate), $phrases[$name])
    }
}
finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $compatConfig -Force -ErrorAction SilentlyContinue
}

[WavTools]::Write((Join-Path $OutDir 'silence_5s.wav'), (New-Object 'int16[]' ($rate * 5)), $rate)
Write-Host 'silence_5s.wav  5.00 s  digital silence'
[WavTools]::Write((Join-Path $OutDir 'noise_5s.wav'), [WavTools]::Noise($rate * 5, 3000), $rate)
Write-Host 'noise_5s.wav  5.00 s  white noise (about -21 dBFS peak)'
exit 0
