use nnnoiseless::DenoiseState;

/// RNNoise works on 48 kHz audio in frames of 480 samples scaled like 16-bit PCM.
pub const RNNOISE_RATE: u32 = 48_000;
const FRAME: usize = DenoiseState::FRAME_SIZE;
const PCM_SCALE: f32 = 32768.0;

/// Streaming RNNoise wrapper: takes any number of 48 kHz samples in -1..1, returns denoised samples.
pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    pending: Vec<f32>,
    input: [f32; FRAME],
    output: [f32; FRAME],
    first_frame: bool,
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            pending: Vec::new(),
            input: [0.0; FRAME],
            output: [0.0; FRAME],
            first_frame: true,
        }
    }

    pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(samples);
        let mut consumed = 0;
        while self.pending.len() - consumed >= FRAME {
            for (slot, sample) in self
                .input
                .iter_mut()
                .zip(&self.pending[consumed..consumed + FRAME])
            {
                *slot = sample * PCM_SCALE;
            }
            self.state.process_frame(&mut self.output, &self.input);
            consumed += FRAME;
            // The first RNNoise frame is a fade-in artifact.
            if self.first_frame {
                self.first_frame = false;
                continue;
            }
            out.extend(self.output.iter().map(|sample| sample / PCM_SCALE));
        }
        self.pending.drain(..consumed);
    }
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::rms;

    #[test]
    fn keeps_frame_alignment_across_odd_chunks() {
        let mut denoiser = Denoiser::new();
        let mut out = Vec::new();
        let input = vec![0.0_f32; 48_000];
        for piece in input.chunks(333) {
            denoiser.process(piece, &mut out);
        }
        // 100 frames in, the first one is dropped.
        assert_eq!(out.len(), 99 * FRAME);
    }

    #[test]
    fn suppresses_steady_white_noise() {
        // Deterministic pseudo-random noise (LCG), quiet.
        let mut seed = 12345_u32;
        let noise: Vec<f32> = (0..96_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.1
            })
            .collect();
        let mut denoiser = Denoiser::new();
        let mut out = Vec::new();
        denoiser.process(&noise, &mut out);
        let tail = &out[out.len() / 2..];
        assert!(
            rms(tail) < rms(&noise) * 0.5,
            "{} vs {}",
            rms(tail),
            rms(&noise)
        );
    }
}
