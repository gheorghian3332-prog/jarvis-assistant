/// Gain applied to the RMS before it is sent to the UI (speech RMS is far below 1.0).
pub const LEVEL_GAIN: f32 = 4.0;
/// A first-seconds peak below this means the device delivers no signal at all.
pub const NO_SIGNAL_PEAK: f32 = 0.0002;
/// Length of the capture window used to measure the peak.
pub const PEAK_WINDOW_SECONDS: f32 = 3.0;

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum / samples.len() as f32).sqrt()
}

pub fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

/// RMS scaled for display and clamped to 0..=1.
pub fn display_level(rms: f32) -> f32 {
    (rms * LEVEL_GAIN).clamp(0.0, 1.0)
}

/// Decides when the next level event may be sent (at most `max_hz` times per second of audio).
#[derive(Debug, Clone)]
pub struct LevelLimiter {
    interval_samples: u64,
    since_last: u64,
}

impl LevelLimiter {
    pub fn new(sample_rate: u32, max_hz: u32) -> Self {
        Self {
            interval_samples: u64::from(sample_rate / max_hz.max(1)),
            since_last: u64::MAX / 2,
        }
    }

    /// Registers `samples` of audio and returns true if a level event is due now.
    pub fn tick(&mut self, samples: usize) -> bool {
        self.since_last = self.since_last.saturating_add(samples as u64);
        if self.since_last >= self.interval_samples {
            self.since_last = 0;
            true
        } else {
            false
        }
    }
}

/// Peak of the first `PEAK_WINDOW_SECONDS` of capture.
#[derive(Debug, Clone)]
pub struct PeakMeter {
    remaining: u64,
    peak: f32,
    done: bool,
}

impl PeakMeter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            remaining: (f64::from(sample_rate) * f64::from(PEAK_WINDOW_SECONDS)) as u64,
            peak: 0.0,
            done: false,
        }
    }

    /// Feeds samples; returns the final peak once, when the window has been filled.
    pub fn feed(&mut self, samples: &[f32]) -> Option<f32> {
        if self.done {
            return None;
        }
        let take = (samples.len() as u64).min(self.remaining) as usize;
        self.peak = self.peak.max(peak(&samples[..take]));
        self.remaining -= take as u64;
        if self.remaining == 0 {
            self.done = true;
            Some(self.peak)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_constant_and_empty() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-6);
        let square: Vec<f32> = (0..100)
            .map(|i| if i % 2 == 0 { 0.3 } else { -0.3 })
            .collect();
        assert!((rms(&square) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn display_level_is_clamped() {
        assert_eq!(display_level(0.0), 0.0);
        assert!((display_level(0.1) - 0.4).abs() < 1e-6);
        assert_eq!(display_level(0.9), 1.0);
    }

    #[test]
    fn limiter_caps_the_rate_at_30_hz() {
        let mut limiter = LevelLimiter::new(16_000, 30);
        let mut events = 0;
        // One second of audio in 20 ms frames.
        for _ in 0..50 {
            if limiter.tick(320) {
                events += 1;
            }
        }
        assert!(events <= 30, "{events}");
        assert!(events >= 20, "{events}");
    }

    #[test]
    fn peak_meter_reports_once_after_three_seconds() {
        let mut meter = PeakMeter::new(1000);
        assert_eq!(meter.feed(&[0.1; 1000]), None);
        assert_eq!(meter.feed(&[0.4, 0.2]), None);
        let result = meter.feed(&[0.2; 1998]);
        assert_eq!(result, Some(0.4));
        assert_eq!(meter.feed(&[0.9; 10]), None);
    }

    #[test]
    fn peak_meter_ignores_samples_past_the_window() {
        let mut meter = PeakMeter::new(10);
        let mut samples = vec![0.0; 30];
        samples.push(0.9);
        assert_eq!(meter.feed(&samples), Some(0.0));
    }

    #[test]
    fn zero_signal_is_flagged() {
        assert!(peak(&[0.0, 0.0]) < NO_SIGNAL_PEAK);
        assert!(peak(&[0.0, -0.01]) > NO_SIGNAL_PEAK);
    }
}
