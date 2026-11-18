use crate::segmenter::Frame;
use crate::vad::FRAME_MS;

/// Silence after a command that ends it.
pub const COMMAND_SILENCE_MS: u32 = 800;
/// Longest command after the wake word.
pub const MAX_COMMAND_MS: u32 = 12_000;
/// Without speech this long after the wake word the assistant gives up.
pub const NO_SPEECH_TIMEOUT_MS: u32 = 5_000;
/// Safety limit of one push-to-talk phrase.
pub const PTT_MAX_MS: u32 = 30_000;
/// Shorter push-to-talk phrases are accidental taps and are dropped.
pub const PTT_MIN_MS: u32 = 200;
/// Silence kept around a wake command when trimming.
pub const TRIM_MS: u32 = 300;
/// Voiced frames needed before a command counts as started.
pub const SPEECH_START_FRAMES: usize = 5;

const fn frames(milliseconds: u32) -> usize {
    (milliseconds / FRAME_MS) as usize
}

#[derive(Debug, Clone, PartialEq)]
pub enum CollectorStatus {
    Collecting,
    /// The finished phrase (mono 16 kHz PCM16).
    Done(Vec<i16>),
    /// Nothing worth sending: no speech in time, or an accidental tap.
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wake,
    Ptt,
}

/// Accumulates the command phrase after the wake word, or the whole push-to-talk phrase.
#[derive(Debug)]
pub struct Collector {
    kind: Kind,
    frames: Vec<Frame>,
    voiced_total: usize,
    silence_run: u32,
    pushed: u32,
}

impl Collector {
    /// Starts collecting after a wake word; `initial` is the audio already heard after the name.
    pub fn wake(initial: Vec<Frame>) -> Self {
        let voiced_total = initial.iter().filter(|frame| frame.voiced).count();
        let silence_run = initial
            .iter()
            .rev()
            .take_while(|frame| !frame.voiced)
            .count() as u32;
        Self {
            kind: Kind::Wake,
            frames: initial,
            voiced_total,
            silence_run,
            pushed: 0,
        }
    }

    pub fn ptt() -> Self {
        Self {
            kind: Kind::Ptt,
            frames: Vec::new(),
            voiced_total: 0,
            silence_run: 0,
            pushed: 0,
        }
    }

    pub fn has_speech(&self) -> bool {
        self.voiced_total >= SPEECH_START_FRAMES
    }

    pub fn push(&mut self, pcm: &[i16], voiced: bool) -> CollectorStatus {
        self.pushed += 1;
        self.frames.push(Frame {
            pcm: pcm.to_vec(),
            voiced,
        });
        if voiced {
            self.voiced_total += 1;
            self.silence_run = 0;
        } else {
            self.silence_run += 1;
        }
        match self.kind {
            Kind::Ptt => {
                if self.frames.len() >= frames(PTT_MAX_MS) {
                    return self.finish();
                }
                CollectorStatus::Collecting
            }
            Kind::Wake => {
                if self.has_speech() {
                    if self.silence_run as usize >= frames(COMMAND_SILENCE_MS)
                        || self.frames.len() >= frames(MAX_COMMAND_MS)
                    {
                        return self.finish();
                    }
                } else if self.pushed >= frames(NO_SPEECH_TIMEOUT_MS) as u32 {
                    return CollectorStatus::Dropped;
                }
                CollectorStatus::Collecting
            }
        }
    }

    /// Push-to-talk key released: ends the phrase.
    pub fn release(&mut self) -> CollectorStatus {
        self.finish()
    }

    fn finish(&mut self) -> CollectorStatus {
        let all = std::mem::take(&mut self.frames);
        match self.kind {
            Kind::Ptt => {
                if all.len() < frames(PTT_MIN_MS) {
                    return CollectorStatus::Dropped;
                }
                CollectorStatus::Done(all.into_iter().flat_map(|frame| frame.pcm).collect())
            }
            Kind::Wake => {
                let first = all.iter().position(|frame| frame.voiced);
                let last = all.iter().rposition(|frame| frame.voiced);
                let (Some(first), Some(last)) = (first, last) else {
                    return CollectorStatus::Dropped;
                };
                let start = first.saturating_sub(frames(TRIM_MS));
                let end = (last + 1 + frames(TRIM_MS)).min(all.len());
                CollectorStatus::Done(
                    all[start..end]
                        .iter()
                        .flat_map(|frame| frame.pcm.iter().copied())
                        .collect(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(voiced: bool) -> Frame {
        Frame {
            pcm: vec![if voiced { 100 } else { 0 }; 320],
            voiced,
        }
    }

    fn feed(collector: &mut Collector, count: usize, voiced: bool) -> CollectorStatus {
        let mut status = CollectorStatus::Collecting;
        for _ in 0..count {
            status = collector.push(&frame(voiced).pcm, voiced);
            if status != CollectorStatus::Collecting {
                break;
            }
        }
        status
    }

    #[test]
    fn wake_command_ends_after_800_ms_of_silence() {
        let mut collector = Collector::wake(vec![frame(false); 20]);
        assert_eq!(feed(&mut collector, 25, false), CollectorStatus::Collecting);
        assert_eq!(feed(&mut collector, 50, true), CollectorStatus::Collecting);
        assert_eq!(feed(&mut collector, 39, false), CollectorStatus::Collecting);
        let CollectorStatus::Done(pcm) = feed(&mut collector, 1, false) else {
            panic!("expected Done after 40 quiet frames");
        };
        // 300 ms of silence either side of 50 voiced frames: 15 + 50 + 15 frames.
        assert_eq!(pcm.len(), (15 + 50 + 15) * 320);
    }

    #[test]
    fn command_spoken_with_the_wake_word_only_waits_for_the_remaining_silence() {
        // 25 voiced frames after the name plus the 20-frame hangover already heard.
        let mut initial = vec![frame(true); 25];
        initial.extend(vec![frame(false); 20]);
        let mut collector = Collector::wake(initial);
        assert!(collector.has_speech());
        assert_eq!(feed(&mut collector, 19, false), CollectorStatus::Collecting);
        assert!(matches!(
            feed(&mut collector, 1, false),
            CollectorStatus::Done(_)
        ));
    }

    #[test]
    fn gives_up_after_five_seconds_without_speech() {
        let mut collector = Collector::wake(vec![frame(false); 20]);
        assert_eq!(
            feed(&mut collector, 249, false),
            CollectorStatus::Collecting
        );
        assert_eq!(feed(&mut collector, 1, false), CollectorStatus::Dropped);
    }

    #[test]
    fn a_few_noise_frames_do_not_start_a_command() {
        let mut collector = Collector::wake(Vec::new());
        feed(&mut collector, 2, true);
        assert!(!collector.has_speech());
        assert_eq!(feed(&mut collector, 248, false), CollectorStatus::Dropped);
    }

    #[test]
    fn command_is_capped_at_twelve_seconds() {
        let mut collector = Collector::wake(Vec::new());
        assert!(matches!(
            feed(&mut collector, 700, true),
            CollectorStatus::Done(_)
        ));
        let mut collector = Collector::wake(Vec::new());
        assert_eq!(feed(&mut collector, 599, true), CollectorStatus::Collecting);
        let CollectorStatus::Done(pcm) = feed(&mut collector, 1, true) else {
            panic!("expected Done at 12 s");
        };
        assert_eq!(pcm.len(), 600 * 320);
    }

    #[test]
    fn ptt_keeps_everything_until_release() {
        let mut collector = Collector::ptt();
        assert_eq!(feed(&mut collector, 10, false), CollectorStatus::Collecting);
        assert_eq!(feed(&mut collector, 100, true), CollectorStatus::Collecting);
        // Long silence does not end a push-to-talk phrase.
        assert_eq!(
            feed(&mut collector, 100, false),
            CollectorStatus::Collecting
        );
        let CollectorStatus::Done(pcm) = collector.release() else {
            panic!("expected Done");
        };
        assert_eq!(pcm.len(), 210 * 320);
    }

    #[test]
    fn ptt_tap_is_dropped() {
        let mut collector = Collector::ptt();
        feed(&mut collector, 5, true);
        assert_eq!(collector.release(), CollectorStatus::Dropped);
    }
}
