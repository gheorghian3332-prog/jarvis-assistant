use crate::vad::FRAME_MS;
use std::collections::VecDeque;

/// Audio kept from before the first voiced frame.
pub const PRE_ROLL_MS: u32 = 300;
/// Silence that closes a speech segment.
pub const HANGOVER_MS: u32 = 400;
/// Consecutive voiced frames needed to open a segment (debounce against clicks).
pub const ONSET_FRAMES: usize = 3;
/// Hard limit of one segment.
pub const MAX_SEGMENT_MS: u32 = 12_000;

const PRE_ROLL_FRAMES: usize = (PRE_ROLL_MS / FRAME_MS) as usize;
const HANGOVER_FRAMES: u32 = HANGOVER_MS / FRAME_MS;
const MAX_FRAMES: u32 = MAX_SEGMENT_MS / FRAME_MS;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub pcm: Vec<i16>,
    pub voiced: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SegmentEvent {
    /// A segment opened; the frames are the pre-roll followed by the onset frames.
    Begin(Vec<Frame>),
    /// One more frame of the open segment (hangover frames included).
    Frame(Frame),
    /// The segment closed after the hangover (or hit the length limit). Always follows a `Frame`.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Silence,
    Speech,
}

/// Cuts a stream of 20 ms frames into speech segments with pre-roll and hangover.
#[derive(Debug)]
pub struct Segmenter {
    phase: Phase,
    ring: VecDeque<Frame>,
    voiced_run: usize,
    quiet_run: u32,
    length: u32,
}

impl Segmenter {
    pub fn new() -> Self {
        Self {
            phase: Phase::Silence,
            ring: VecDeque::new(),
            voiced_run: 0,
            quiet_run: 0,
            length: 0,
        }
    }

    pub fn in_speech(&self) -> bool {
        self.phase == Phase::Speech
    }

    /// Drops any open segment and the pre-roll without emitting events.
    pub fn reset(&mut self) {
        self.phase = Phase::Silence;
        self.ring.clear();
        self.voiced_run = 0;
        self.quiet_run = 0;
        self.length = 0;
    }

    pub fn push(&mut self, pcm: &[i16], voiced: bool) -> Vec<SegmentEvent> {
        let frame = Frame {
            pcm: pcm.to_vec(),
            voiced,
        };
        match self.phase {
            Phase::Silence => {
                self.ring.push_back(frame);
                while self.ring.len() > PRE_ROLL_FRAMES + ONSET_FRAMES {
                    self.ring.pop_front();
                }
                self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
                if self.voiced_run >= ONSET_FRAMES {
                    self.phase = Phase::Speech;
                    self.quiet_run = 0;
                    self.length = self.ring.len() as u32;
                    self.voiced_run = 0;
                    return vec![SegmentEvent::Begin(self.ring.drain(..).collect())];
                }
                Vec::new()
            }
            Phase::Speech => {
                self.length += 1;
                self.quiet_run = if voiced { 0 } else { self.quiet_run + 1 };
                let closes = self.quiet_run >= HANGOVER_FRAMES || self.length >= MAX_FRAMES;
                if closes {
                    self.reset();
                    vec![SegmentEvent::Frame(frame), SegmentEvent::End]
                } else {
                    vec![SegmentEvent::Frame(frame)]
                }
            }
        }
    }
}

impl Default for Segmenter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: i16) -> Vec<i16> {
        vec![value; 320]
    }

    /// Feeds `count` frames; returns all events.
    fn feed(
        segmenter: &mut Segmenter,
        count: usize,
        voiced: bool,
        value: i16,
    ) -> Vec<SegmentEvent> {
        (0..count)
            .flat_map(|_| segmenter.push(&frame(value), voiced))
            .collect()
    }

    #[test]
    fn silence_produces_nothing() {
        let mut segmenter = Segmenter::new();
        assert!(feed(&mut segmenter, 200, false, 0).is_empty());
        assert!(!segmenter.in_speech());
    }

    #[test]
    fn single_voiced_frames_do_not_open_a_segment() {
        let mut segmenter = Segmenter::new();
        for _ in 0..20 {
            assert!(feed(&mut segmenter, 1, true, 5).is_empty());
            assert!(feed(&mut segmenter, 4, false, 0).is_empty());
        }
    }

    #[test]
    fn begin_carries_pre_roll_and_onset() {
        let mut segmenter = Segmenter::new();
        // 50 quiet frames (value 1), then speech (value 2).
        feed(&mut segmenter, 50, false, 1);
        let events = feed(&mut segmenter, ONSET_FRAMES, true, 2);
        assert_eq!(events.len(), 1);
        let SegmentEvent::Begin(frames) = &events[0] else {
            panic!("expected Begin, got {events:?}");
        };
        // 300 ms pre-roll (15 frames) plus the onset frames.
        assert_eq!(frames.len(), PRE_ROLL_FRAMES + ONSET_FRAMES);
        assert_eq!(frames[0].pcm[0], 1);
        let voiced_tail = frames.iter().rev().take(ONSET_FRAMES).all(|f| f.voiced);
        assert!(voiced_tail);
        assert!(segmenter.in_speech());
    }

    #[test]
    fn short_history_gives_short_pre_roll() {
        let mut segmenter = Segmenter::new();
        feed(&mut segmenter, 2, false, 1);
        let events = feed(&mut segmenter, ONSET_FRAMES, true, 2);
        let SegmentEvent::Begin(frames) = &events[0] else {
            panic!("expected Begin");
        };
        assert_eq!(frames.len(), 2 + ONSET_FRAMES);
    }

    #[test]
    fn hangover_keeps_the_segment_open_for_400_ms() {
        let mut segmenter = Segmenter::new();
        feed(&mut segmenter, ONSET_FRAMES, true, 2);
        // 19 quiet frames (380 ms): still open, only Frame events.
        let events = feed(&mut segmenter, 19, false, 0);
        assert_eq!(events.len(), 19);
        assert!(events.iter().all(|e| matches!(e, SegmentEvent::Frame(_))));
        // A voiced frame resets the hangover.
        feed(&mut segmenter, 1, true, 2);
        let events = feed(&mut segmenter, 19, false, 0);
        assert!(!events.contains(&SegmentEvent::End));
        // The 20th quiet frame closes it: Frame then End.
        let events = feed(&mut segmenter, 1, false, 0);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], SegmentEvent::Frame(_)));
        assert_eq!(events[1], SegmentEvent::End);
        assert!(!segmenter.in_speech());
    }

    #[test]
    fn long_speech_is_cut_at_the_limit_and_can_restart() {
        let mut segmenter = Segmenter::new();
        let events = feed(&mut segmenter, 700, true, 3);
        let ends = events.iter().filter(|e| **e == SegmentEvent::End).count();
        assert_eq!(ends, 1);
        // Speech continues, so a new segment opens after the onset.
        let begins = events
            .iter()
            .filter(|e| matches!(e, SegmentEvent::Begin(_)))
            .count();
        assert_eq!(begins, 2);
    }

    #[test]
    fn reset_drops_the_open_segment() {
        let mut segmenter = Segmenter::new();
        feed(&mut segmenter, ONSET_FRAMES, true, 2);
        segmenter.reset();
        assert!(!segmenter.in_speech());
        assert!(feed(&mut segmenter, 30, false, 0).is_empty());
    }
}
