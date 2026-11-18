use crate::error::{VoiceError, VoiceResult};
use rubato::{audioadapter_buffers::direct::InterleavedSlice, Fft, FixedSync, Resampler};

/// Streaming mono resampler (FFT based, fixed rates). Passes audio through when the rates match.
pub struct StreamResampler {
    inner: Option<Fft<f32>>,
    pending: Vec<f32>,
    scratch: Vec<f32>,
}

fn convert<E: std::fmt::Display>(error: E) -> VoiceError {
    VoiceError::Resample(error.to_string())
}

impl StreamResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> VoiceResult<Self> {
        let inner = if input_rate == output_rate {
            None
        } else {
            let chunk = (input_rate / 50).max(1) as usize;
            Some(
                Fft::<f32>::new(
                    input_rate as usize,
                    output_rate as usize,
                    chunk,
                    1,
                    FixedSync::Input,
                )
                .map_err(convert)?,
            )
        };
        Ok(Self {
            inner,
            pending: Vec::new(),
            scratch: Vec::new(),
        })
    }

    /// Appends the resampled `input` to `out`. Output lags by the filter delay (a few ms).
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> VoiceResult<()> {
        let Some(resampler) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            return Ok(());
        };
        self.pending.extend_from_slice(input);
        loop {
            let needed = resampler.input_frames_next();
            if self.pending.len() < needed {
                return Ok(());
            }
            let capacity = resampler.output_frames_next();
            self.scratch.clear();
            self.scratch.resize(capacity, 0.0);
            let source =
                InterleavedSlice::new(&self.pending[..needed], 1, needed).map_err(convert)?;
            let mut target =
                InterleavedSlice::new_mut(&mut self.scratch, 1, capacity).map_err(convert)?;
            let (consumed, produced) = resampler
                .process_into_buffer(&source, &mut target, None)
                .map_err(convert)?;
            out.extend_from_slice(&self.scratch[..produced]);
            self.pending.drain(..consumed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::rms;

    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn run(input_rate: u32, output_rate: u32, signal: &[f32]) -> Vec<f32> {
        let mut resampler = StreamResampler::new(input_rate, output_rate).expect("resampler");
        let mut out = Vec::new();
        // Odd-sized pieces exercise the internal buffering.
        for piece in signal.chunks(777) {
            resampler.process(piece, &mut out).expect("process");
        }
        out
    }

    #[test]
    fn downsamples_48k_to_16k_and_keeps_level() {
        let input = sine(48_000, 440.0, 1.0);
        let out = run(48_000, 16_000, &input);
        assert!(out.len() > 15_000 && out.len() <= 16_000, "{}", out.len());
        let tail = &out[out.len() / 2..];
        assert!((rms(tail) - rms(&input)).abs() < 0.02, "{}", rms(tail));
    }

    #[test]
    fn converts_44k1_to_48k() {
        let input = sine(44_100, 1000.0, 1.0);
        let out = run(44_100, 48_000, &input);
        assert!(out.len() > 46_000 && out.len() <= 48_000, "{}", out.len());
        let tail = &out[out.len() / 2..];
        assert!((rms(tail) - rms(&input)).abs() < 0.02);
    }

    #[test]
    fn removes_content_above_the_new_nyquist() {
        // 12 kHz is above 8 kHz (Nyquist of 16 kHz) and must be filtered out.
        let input = sine(48_000, 12_000.0, 1.0);
        let out = run(48_000, 16_000, &input);
        let tail = &out[out.len() / 2..];
        assert!(rms(tail) < 0.05, "{}", rms(tail));
    }

    #[test]
    fn same_rate_is_passed_through() {
        let input = vec![0.25_f32; 100];
        assert_eq!(run(16_000, 16_000, &input), input);
    }
}
