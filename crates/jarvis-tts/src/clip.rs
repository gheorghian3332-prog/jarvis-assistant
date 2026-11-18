//! Audio data: PCM16 clips, WAV files and resampling.

use crate::error::{TtsError, TtsResult};
use std::path::Path;

/// One synthesized piece of speech: mono PCM16.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioClip {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
}

impl AudioClip {
    pub fn new(pcm: Vec<i16>, sample_rate: u32) -> Self {
        Self { pcm, sample_rate }
    }

    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.pcm.len() as f32 / self.sample_rate as f32
    }

    /// Largest absolute sample value.
    pub fn peak(&self) -> i16 {
        self.pcm
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max()
            .map_or(0, |peak| peak.min(i16::MAX as u16) as i16)
    }

    /// RMS of the whole clip, 0..1.
    pub fn rms(&self) -> f32 {
        rms_i16(&self.pcm)
    }

    pub fn to_f32(&self) -> Vec<f32> {
        self.pcm
            .iter()
            .map(|sample| f32::from(*sample) / 32768.0)
            .collect()
    }

    /// Drops silence at both ends, keeping `keep_ms` of it (Piper pads its output with a pause).
    pub fn trim_silence(&mut self, threshold: i16, keep_ms: u32) {
        let keep = (self.sample_rate as usize * keep_ms as usize) / 1000;
        let loud = |sample: &i16| sample.unsigned_abs() > threshold.unsigned_abs();
        let Some(first) = self.pcm.iter().position(loud) else {
            return;
        };
        let last = self.pcm.iter().rposition(loud).unwrap_or(first);
        let start = first.saturating_sub(keep);
        let end = (last + 1 + keep).min(self.pcm.len());
        self.pcm = self.pcm[start..end].to_vec();
    }

    /// Writes a mono PCM16 WAV file.
    pub fn write_wav(&self, path: &Path) -> TtsResult<()> {
        write_wav(path, &self.pcm, self.sample_rate)
    }

    /// Reads a PCM16 WAV file (stereo is mixed down to mono).
    pub fn read_wav(path: &Path) -> TtsResult<Self> {
        let invalid =
            |error: hound::Error| TtsError::Invalid(format!("{}: {error}", path.display()));
        let mut reader = hound::WavReader::open(path).map_err(invalid)?;
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err(TtsError::Invalid(format!(
                "{}: expected PCM16, got {:?} {} bit",
                path.display(),
                spec.sample_format,
                spec.bits_per_sample
            )));
        }
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .map_err(invalid)?;
        let channels = usize::from(spec.channels.max(1));
        let pcm = if channels == 1 {
            samples
        } else {
            samples
                .chunks_exact(channels)
                .map(|frame| {
                    (frame.iter().map(|s| i32::from(*s)).sum::<i32>() / channels as i32) as i16
                })
                .collect()
        };
        Ok(Self::new(pcm, spec.sample_rate))
    }
}

pub fn write_wav(path: &Path, pcm: &[i16], sample_rate: u32) -> TtsResult<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let io = |error: hound::Error| TtsError::Playback(format!("{}: {error}", path.display()));
    let mut writer = hound::WavWriter::create(path, spec).map_err(io)?;
    for sample in pcm {
        writer.write_sample(*sample).map_err(io)?;
    }
    writer.finalize().map_err(io)
}

pub fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|sample| {
            let value = f64::from(*sample) / 32768.0;
            value * value
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Root mean square of float samples.
pub fn rms_f32(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

const SINC_LOBES: f64 = 8.0;

/// Windowed-sinc resampler (Hann window); the cutoff sits just below the lower Nyquist frequency.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let cutoff = (f64::from(to) / f64::from(from)).min(1.0) * 0.97;
    let half = (SINC_LOBES / cutoff).ceil() as i64;
    let length = (input.len() as f64 / ratio).floor() as usize;
    let mut output = Vec::with_capacity(length);
    for index in 0..length {
        let center = index as f64 * ratio;
        let base = center.floor() as i64;
        let mut sum = 0.0f64;
        for tap in (base - half)..=(base + half) {
            if tap < 0 || tap as usize >= input.len() {
                continue;
            }
            let distance = tap as f64 - center;
            let x = distance * cutoff;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let window = 0.5 + 0.5 * (std::f64::consts::PI * distance / (half as f64 + 1.0)).cos();
            sum += f64::from(input[tap as usize]) * sinc * window * cutoff;
        }
        output.push(sum as f32);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|n| (2.0 * std::f32::consts::PI * hz * n as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn resampling_keeps_duration_and_level() {
        let input = sine(22_050, 440.0, 1.0);
        let output = resample(&input, 22_050, 48_000);
        assert!((output.len() as i64 - 48_000).abs() <= 2);
        let ratio = rms_f32(&output[2_000..46_000]) / rms_f32(&input[1_000..21_000]);
        assert!((ratio - 1.0).abs() < 0.03, "ratio {ratio}");
        let down = resample(&sine(24_000, 440.0, 1.0), 24_000, 16_000);
        assert!((down.len() as i64 - 16_000).abs() <= 2);
    }

    #[test]
    fn same_rate_is_a_copy() {
        let input = vec![0.1, -0.2, 0.3];
        assert_eq!(resample(&input, 24_000, 24_000), input);
    }

    #[test]
    fn rms_of_full_scale_square_wave_is_one() {
        let clip = AudioClip::new(vec![i16::MAX, i16::MIN, i16::MAX, i16::MIN], 8_000);
        assert!((clip.rms() - 1.0).abs() < 0.001);
        assert_eq!(AudioClip::new(vec![], 8_000).rms(), 0.0);
        assert_eq!(clip.peak(), i16::MAX);
    }

    #[test]
    fn wav_round_trip_keeps_samples_and_rate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.wav");
        let clip = AudioClip::new(vec![0, 1000, -1000, 32767, -32768], 22_050);
        clip.write_wav(&path).expect("write");
        assert_eq!(AudioClip::read_wav(&path).expect("read"), clip);
    }

    #[test]
    fn trimming_keeps_a_little_silence_at_both_ends() {
        let mut pcm = vec![0i16; 1000];
        pcm.extend(vec![5000i16; 100]);
        pcm.extend(vec![0i16; 1000]);
        let mut clip = AudioClip::new(pcm, 1000);
        clip.trim_silence(100, 50);
        assert_eq!(clip.pcm.len(), 50 + 100 + 50);
        let mut silent = AudioClip::new(vec![0; 10], 1000);
        silent.trim_silence(100, 50);
        assert_eq!(silent.pcm.len(), 10, "all silence is left alone");
    }
}
