use webrtc_vad::{SampleRate, Vad, VadMode};

/// Samples per VAD frame: 20 ms at 16 kHz.
pub const FRAME_SAMPLES: usize = 320;
pub const FRAME_MS: u32 = 20;
pub const STT_RATE: u32 = 16_000;

/// WebRTC voice activity detector for 20 ms frames of 16 kHz PCM16.
/// Not `Send`: create it on the thread that uses it.
pub struct VoiceDetector {
    vad: Vad,
}

fn mode(aggressiveness: u8) -> VadMode {
    match aggressiveness {
        0 => VadMode::Quality,
        1 => VadMode::LowBitrate,
        2 => VadMode::Aggressive,
        _ => VadMode::VeryAggressive,
    }
}

impl VoiceDetector {
    pub fn new(aggressiveness: u8) -> Self {
        Self {
            vad: Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, mode(aggressiveness)),
        }
    }

    pub fn set_aggressiveness(&mut self, aggressiveness: u8) {
        self.vad.set_mode(mode(aggressiveness));
    }

    pub fn is_voice(&mut self, frame: &[i16]) -> bool {
        if frame.len() != FRAME_SAMPLES {
            return false;
        }
        self.vad.is_voice_segment(frame).unwrap_or(false)
    }
}

pub fn to_pcm16(samples: &[f32], out: &mut Vec<i16>) {
    out.extend(
        samples
            .iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digital_silence_is_not_voice() {
        let mut detector = VoiceDetector::new(2);
        for _ in 0..20 {
            assert!(!detector.is_voice(&[0; FRAME_SAMPLES]));
        }
    }

    #[test]
    fn wrong_frame_size_is_not_voice() {
        let mut detector = VoiceDetector::new(0);
        assert!(!detector.is_voice(&[1000; 100]));
    }

    #[test]
    fn converts_and_clamps_to_pcm16() {
        let mut out = Vec::new();
        to_pcm16(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5], &mut out);
        assert_eq!(out, vec![0, 32767, -32767, 32767, -32767, 16384]);
    }
}
