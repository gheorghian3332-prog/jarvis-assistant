use crate::{
    collector::{Collector, CollectorStatus},
    config::{confidence_threshold, vad_aggressiveness},
    denoise::{Denoiser, RNNOISE_RATE},
    error::VoiceResult,
    level::{display_level, rms, LevelLimiter, PeakMeter, NO_SIGNAL_PEAK},
    resample::StreamResampler,
    segmenter::{Frame, SegmentEvent, Segmenter},
    vad::{to_pcm16, VoiceDetector, FRAME_MS, FRAME_SAMPLES, STT_RATE},
    wake::{evaluate, HeardWord, WakeMatch, WakeRecognizer},
};
use jarvis_core::{Event, MicStatus, UtteranceSource, VoiceHint};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// After a wake word nothing else is recognized for this long.
pub const COOLDOWN_MS: u32 = 1_500;
/// Without `finish_processing` the pipeline returns to idle after this long.
pub const PROCESSING_TIMEOUT_MS: u32 = 30_000;
/// Level events per second at most.
pub const LEVEL_MAX_HZ: u32 = 30;
/// Skipped after the end of the name: its tail must not count as the start of the command.
const NAME_TAIL_MARGIN_FRAMES: usize = 5;
/// Voice activity this long after the cut (Vosk end times run early) is still counted as the name, not
/// as the start of a command. The audio itself is kept.
const NAME_GUARD_FRAMES: usize = 15;

const fn frames(milliseconds: u32) -> u32 {
    milliseconds / FRAME_MS
}

pub trait VoiceActivity {
    fn is_voice(&mut self, frame: &[i16]) -> bool;
    fn set_aggressiveness(&mut self, aggressiveness: u8);
}

impl VoiceActivity for VoiceDetector {
    fn is_voice(&mut self, frame: &[i16]) -> bool {
        VoiceDetector::is_voice(self, frame)
    }
    fn set_aggressiveness(&mut self, aggressiveness: u8) {
        VoiceDetector::set_aggressiveness(self, aggressiveness);
    }
}

/// The wake word recognizer as the pipeline sees it.
pub trait WakeBackend {
    fn accept(&mut self, pcm: &[i16]);
    fn finish(&mut self) -> Vec<HeardWord>;
    fn discard(&mut self);
    fn set_names(&mut self, names: &[String]) -> VoiceResult<()>;
    /// Called once per frame while nobody is speaking; lets a backend release memory after a quiet spell.
    fn idle_tick(&mut self) {}
}

impl WakeBackend for WakeRecognizer {
    fn accept(&mut self, pcm: &[i16]) {
        WakeRecognizer::accept(self, pcm);
    }
    fn finish(&mut self) -> Vec<HeardWord> {
        WakeRecognizer::finish(self)
    }
    fn discard(&mut self) {
        WakeRecognizer::discard(self);
    }
    fn set_names(&mut self, names: &[String]) -> VoiceResult<()> {
        WakeRecognizer::set_names(self, names)
    }
}

enum Mode {
    /// Waiting for the wake word (or doing nothing when there is no recognizer).
    Idle,
    Listening(Collector, UtteranceSource),
    /// An utterance was handed over; waits for `finish_processing`.
    Processing {
        frames_left: u32,
    },
}

type Emit = Box<dyn FnMut(Event)>;

/// Everything between the raw microphone samples and the events on the bus. All timing counts samples,
/// so a WAV file is processed exactly like live audio, only faster.
pub struct Pipeline {
    device: Arc<str>,
    to_48k: StreamResampler,
    denoiser: Denoiser,
    to_16k: StreamResampler,
    vad: Box<dyn VoiceActivity>,
    wake: Option<Box<dyn WakeBackend>>,
    sensitivity: f32,
    wake_names: Vec<String>,
    segmenter: Segmenter,
    segment: Vec<Frame>,
    mode: Mode,
    speaking: bool,
    cooldown_frames: u32,
    state: &'static str,
    carry: Vec<i16>,
    limiter: LevelLimiter,
    meter: PeakMeter,
    level_enabled: Arc<AtomicBool>,
    emit: Emit,
    scratch_48k: Vec<f32>,
    scratch_clean: Vec<f32>,
    scratch_16k: Vec<f32>,
}

impl Pipeline {
    pub fn new(
        input_rate: u32,
        device: &str,
        sensitivity: f32,
        wake_names: Vec<String>,
        wake: Option<Box<dyn WakeBackend>>,
        level_enabled: Arc<AtomicBool>,
        emit: Emit,
    ) -> VoiceResult<Self> {
        Ok(Self::with_parts(
            StreamResampler::new(input_rate, RNNOISE_RATE)?,
            StreamResampler::new(RNNOISE_RATE, STT_RATE)?,
            Box::new(VoiceDetector::new(vad_aggressiveness(sensitivity))),
            input_rate,
            device,
            sensitivity,
            wake_names,
            wake,
            level_enabled,
            emit,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn with_parts(
        to_48k: StreamResampler,
        to_16k: StreamResampler,
        vad: Box<dyn VoiceActivity>,
        input_rate: u32,
        device: &str,
        sensitivity: f32,
        wake_names: Vec<String>,
        wake: Option<Box<dyn WakeBackend>>,
        level_enabled: Arc<AtomicBool>,
        emit: Emit,
    ) -> Self {
        Self {
            device: device.into(),
            to_48k,
            denoiser: Denoiser::new(),
            to_16k,
            vad,
            wake,
            sensitivity,
            wake_names,
            segmenter: Segmenter::new(),
            segment: Vec::new(),
            mode: Mode::Idle,
            speaking: false,
            cooldown_frames: 0,
            state: "IDLE",
            carry: Vec::new(),
            limiter: LevelLimiter::new(STT_RATE, LEVEL_MAX_HZ),
            meter: PeakMeter::new(input_rate),
            level_enabled,
            emit,
            scratch_48k: Vec::new(),
            scratch_clean: Vec::new(),
            scratch_16k: Vec::new(),
        }
    }

    pub fn state(&self) -> &'static str {
        self.state
    }

    /// Feeds raw mono samples at the input rate.
    pub fn push(&mut self, samples: &[f32]) -> VoiceResult<()> {
        if let Some(peak) = self.meter.feed(samples) {
            self.report_peak(peak);
        }
        self.scratch_48k.clear();
        self.scratch_clean.clear();
        self.scratch_16k.clear();
        self.to_48k.process(samples, &mut self.scratch_48k)?;
        self.denoiser
            .process(&self.scratch_48k, &mut self.scratch_clean);
        self.to_16k
            .process(&self.scratch_clean, &mut self.scratch_16k)?;
        to_pcm16(&self.scratch_16k, &mut self.carry);
        let mut offset = 0;
        while self.carry.len() - offset >= FRAME_SAMPLES {
            let frame: Vec<i16> = self.carry[offset..offset + FRAME_SAMPLES].to_vec();
            offset += FRAME_SAMPLES;
            self.frame(&frame);
        }
        self.carry.drain(..offset);
        Ok(())
    }

    fn report_peak(&mut self, peak: f32) {
        let no_signal = peak < NO_SIGNAL_PEAK;
        if no_signal {
            tracing::warn!(device = %self.device, peak, "microphone delivers no signal");
        } else {
            tracing::info!(device = %self.device, peak, "microphone peak of the first 3 s");
        }
        (self.emit)(Event::VoiceStateHint(VoiceHint::Mic(MicStatus {
            active: true,
            device: Arc::clone(&self.device),
            peak: Some(peak),
            no_signal,
            error: None,
        })));
    }

    fn set_state(&mut self, state: &'static str) {
        if self.state != state {
            self.state = state;
            (self.emit)(Event::VoiceStateHint(VoiceHint::State(state.into())));
        }
    }

    fn idle_state(&self) -> &'static str {
        if self.speaking {
            "SPEAKING"
        } else {
            "IDLE"
        }
    }

    fn go_idle(&mut self) {
        self.mode = Mode::Idle;
        self.segmenter.reset();
        self.segment.clear();
        if let Some(wake) = self.wake.as_mut() {
            wake.discard();
        }
        let state = self.idle_state();
        self.set_state(state);
    }

    /// Half-duplex flag: while true (the assistant is talking) the wake word is not analyzed.
    pub fn set_speaking(&mut self, speaking: bool) {
        self.speaking = speaking;
        if matches!(self.mode, Mode::Idle) {
            self.segmenter.reset();
            self.segment.clear();
            if let Some(wake) = self.wake.as_mut() {
                wake.discard();
            }
            let state = self.idle_state();
            self.set_state(state);
        }
    }

    /// The consumer is done with the last utterance.
    pub fn finish_processing(&mut self) {
        if matches!(self.mode, Mode::Processing { .. }) {
            self.go_idle();
        }
    }

    /// Push-to-talk: pressing starts listening at once, releasing ends the phrase.
    pub fn set_ptt(&mut self, pressed: bool) {
        if pressed {
            if matches!(self.mode, Mode::Listening(_, UtteranceSource::Ptt)) {
                return;
            }
            self.segmenter.reset();
            self.segment.clear();
            if let Some(wake) = self.wake.as_mut() {
                wake.discard();
            }
            self.mode = Mode::Listening(Collector::ptt(), UtteranceSource::Ptt);
            self.set_state("LISTENING");
        } else if let Mode::Listening(collector, UtteranceSource::Ptt) = &mut self.mode {
            let status = collector.release();
            self.handle_status(status, UtteranceSource::Ptt);
        }
    }

    /// True once a push-to-talk phrase has been handed over or dropped.
    pub fn is_idle(&self) -> bool {
        matches!(self.mode, Mode::Idle)
    }

    pub fn reconfigure(&mut self, sensitivity: f32, wake_names: &[String]) -> VoiceResult<()> {
        self.sensitivity = sensitivity;
        self.vad.set_aggressiveness(vad_aggressiveness(sensitivity));
        if self.wake_names != wake_names {
            self.wake_names = wake_names.to_vec();
            if let Some(wake) = self.wake.as_mut() {
                wake.set_names(wake_names)?;
            }
            self.segmenter.reset();
            self.segment.clear();
        }
        Ok(())
    }

    fn handle_status(&mut self, status: CollectorStatus, source: UtteranceSource) {
        match status {
            CollectorStatus::Collecting => {}
            CollectorStatus::Done(pcm) => {
                (self.emit)(Event::Utterance {
                    pcm16k: Arc::from(pcm),
                    source,
                });
                self.mode = Mode::Processing {
                    frames_left: frames(PROCESSING_TIMEOUT_MS),
                };
                self.set_state("PROCESSING");
            }
            CollectorStatus::Dropped => self.go_idle(),
        }
    }

    /// Processes one 20 ms frame of 16 kHz PCM16.
    pub(crate) fn frame(&mut self, frame: &[i16]) {
        self.cooldown_frames = self.cooldown_frames.saturating_sub(1);
        if self.state == "LISTENING"
            && self.level_enabled.load(Ordering::Relaxed)
            && self.limiter.tick(FRAME_SAMPLES)
        {
            let samples: Vec<f32> = frame.iter().map(|s| f32::from(*s) / 32768.0).collect();
            (self.emit)(Event::VoiceStateHint(VoiceHint::Level(display_level(rms(
                &samples,
            )))));
        }
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Listening(mut collector, source) => {
                // Push-to-talk ends with the key, not with silence, so it needs no VAD.
                let voiced = source == UtteranceSource::Wake && self.vad.is_voice(frame);
                let status = collector.push(frame, voiced);
                self.mode = Mode::Listening(collector, source);
                self.handle_status(status, source);
            }
            Mode::Processing { frames_left } => {
                if frames_left <= 1 {
                    tracing::warn!("processing was never finished; back to idle");
                    self.go_idle();
                } else {
                    self.mode = Mode::Processing {
                        frames_left: frames_left - 1,
                    };
                }
            }
            Mode::Idle => self.idle_frame(frame),
        }
    }

    fn idle_frame(&mut self, frame: &[i16]) {
        if self.wake.is_none() {
            return;
        }
        if !self.segmenter.in_speech() {
            if let Some(wake) = self.wake.as_mut() {
                wake.idle_tick();
            }
        }
        if self.speaking || self.cooldown_frames > 0 {
            if self.segmenter.in_speech() || !self.segment.is_empty() {
                if let Some(wake) = self.wake.as_mut() {
                    wake.discard();
                }
                self.segment.clear();
            }
            self.segmenter.reset();
            return;
        }
        let voiced = self.vad.is_voice(frame);
        for event in self.segmenter.push(frame, voiced) {
            match event {
                SegmentEvent::Begin(frames) => {
                    if let Some(wake) = self.wake.as_mut() {
                        for item in &frames {
                            wake.accept(&item.pcm);
                        }
                    }
                    self.segment = frames;
                }
                SegmentEvent::Frame(item) => {
                    if let Some(wake) = self.wake.as_mut() {
                        wake.accept(&item.pcm);
                    }
                    self.segment.push(item);
                }
                SegmentEvent::End => self.segment_ended(),
            }
        }
    }

    fn segment_ended(&mut self) {
        let segment = std::mem::take(&mut self.segment);
        let Some(wake) = self.wake.as_mut() else {
            return;
        };
        let words = wake.finish();
        let first = segment.iter().position(|frame| frame.voiced);
        let last = segment.iter().rposition(|frame| frame.voiced);
        let (Some(first), Some(last)) = (first, last) else {
            return;
        };
        let speech_seconds = (last - first + 1) as f32 * FRAME_MS as f32 / 1000.0;
        let speech_start = first as f32 * FRAME_MS as f32 / 1000.0;
        let threshold = confidence_threshold(self.sensitivity);
        let Some(found) = evaluate(
            &words,
            &self.wake_names,
            threshold,
            speech_seconds,
            speech_start,
        ) else {
            return;
        };
        self.on_wake(found, segment);
    }

    fn on_wake(&mut self, found: WakeMatch, segment: Vec<Frame>) {
        tracing::info!(name = %found.name, confidence = found.confidence, "wake word detected");
        (self.emit)(Event::WakeDetected {
            name: found.name.as_str().into(),
            confidence: found.confidence,
        });
        self.cooldown_frames = frames(COOLDOWN_MS);
        let cut = ((found.end_seconds * 1000.0 / FRAME_MS as f32).ceil() as usize
            + NAME_TAIL_MARGIN_FRAMES)
            .min(segment.len());
        let remainder = segment
            .into_iter()
            .skip(cut)
            .enumerate()
            .map(|(index, frame)| Frame {
                voiced: frame.voiced && index >= NAME_GUARD_FRAMES,
                ..frame
            })
            .collect();
        self.mode = Mode::Listening(Collector::wake(remainder), UtteranceSource::Wake);
        self.set_state("LISTENING");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    struct FakeVad;
    impl VoiceActivity for FakeVad {
        /// Loud frames are voice.
        fn is_voice(&mut self, frame: &[i16]) -> bool {
            frame.first().is_some_and(|sample| *sample > 1000)
        }
        fn set_aggressiveness(&mut self, _aggressiveness: u8) {}
    }

    struct FakeWake {
        words: Vec<HeardWord>,
        accepted: Rc<RefCell<usize>>,
    }
    impl WakeBackend for FakeWake {
        fn accept(&mut self, _pcm: &[i16]) {
            *self.accepted.borrow_mut() += 1;
        }
        fn finish(&mut self) -> Vec<HeardWord> {
            self.words.clone()
        }
        fn discard(&mut self) {}
        fn set_names(&mut self, _names: &[String]) -> VoiceResult<()> {
            Ok(())
        }
    }

    type Events = Rc<RefCell<Vec<Event>>>;

    fn names() -> Vec<String> {
        vec!["лёха".into()]
    }

    fn build(
        words: Vec<HeardWord>,
        with_wake: bool,
    ) -> (Pipeline, Events, Rc<RefCell<usize>>, Arc<AtomicBool>) {
        let events: Events = Rc::default();
        let sink = Rc::clone(&events);
        let accepted = Rc::new(RefCell::new(0));
        let level = Arc::new(AtomicBool::new(true));
        let wake: Option<Box<dyn WakeBackend>> = with_wake.then(|| {
            Box::new(FakeWake {
                words,
                accepted: Rc::clone(&accepted),
            }) as Box<dyn WakeBackend>
        });
        let pipeline = Pipeline::with_parts(
            StreamResampler::new(16_000, 16_000).expect("resampler"),
            StreamResampler::new(16_000, 16_000).expect("resampler"),
            Box::new(FakeVad),
            16_000,
            "test mic",
            0.5,
            names(),
            wake,
            Arc::clone(&level),
            Box::new(move |event| sink.borrow_mut().push(event)),
        );
        (pipeline, events, accepted, level)
    }

    fn name_word() -> HeardWord {
        HeardWord {
            word: "лёха".into(),
            conf: 0.9,
            start: 0.3,
            end: 0.7,
        }
    }

    fn feed(pipeline: &mut Pipeline, count: usize, loud: bool) {
        let frame = vec![if loud { 3000 } else { 0 }; FRAME_SAMPLES];
        for _ in 0..count {
            pipeline.frame(&frame);
        }
    }

    fn count<F: Fn(&Event) -> bool>(events: &Events, predicate: F) -> usize {
        events
            .borrow()
            .iter()
            .filter(|event| predicate(event))
            .count()
    }

    fn is_wake(event: &Event) -> bool {
        matches!(event, Event::WakeDetected { .. })
    }
    fn is_utterance(event: &Event) -> bool {
        matches!(event, Event::Utterance { .. })
    }

    #[test]
    fn wake_then_command_gives_an_utterance() {
        let (mut pipeline, events, _, _) = build(vec![name_word()], true);
        feed(&mut pipeline, 10, false);
        // Name plus a command in one breath: 60 loud frames, then 20 quiet ones close the segment.
        feed(&mut pipeline, 60, true);
        feed(&mut pipeline, 20, false);
        assert_eq!(count(&events, is_wake), 1);
        assert_eq!(pipeline.state(), "LISTENING");
        // 400 ms of the 800 ms command silence are already behind; 20 more quiet frames finish it.
        feed(&mut pipeline, 19, false);
        assert_eq!(count(&events, is_utterance), 0);
        feed(&mut pipeline, 1, false);
        assert_eq!(count(&events, is_utterance), 1);
        assert_eq!(pipeline.state(), "PROCESSING");
        let events = events.borrow();
        let Some(Event::Utterance { pcm16k, source }) =
            events.iter().find(|event| is_utterance(event))
        else {
            panic!("no utterance");
        };
        assert_eq!(*source, UtteranceSource::Wake);
        // The name (about 0.7 s) is cut away; the command is what remains.
        assert!(pcm16k.len() > 20 * FRAME_SAMPLES);
        assert!(pcm16k.len() < 70 * FRAME_SAMPLES);
    }

    #[test]
    fn standalone_name_waits_for_the_command() {
        let word = HeardWord {
            end: 0.5,
            ..name_word()
        };
        let (mut pipeline, events, _, _) = build(vec![word], true);
        feed(&mut pipeline, 30, true);
        feed(&mut pipeline, 20, false);
        assert_eq!(count(&events, is_wake), 1);
        assert_eq!(pipeline.state(), "LISTENING");
        // The command arrives two seconds later.
        feed(&mut pipeline, 100, false);
        feed(&mut pipeline, 50, true);
        feed(&mut pipeline, 40, false);
        assert_eq!(count(&events, is_utterance), 1);
    }

    #[test]
    fn silence_after_the_name_returns_to_idle_after_five_seconds() {
        let word = HeardWord {
            end: 0.5,
            ..name_word()
        };
        let (mut pipeline, events, _, _) = build(vec![word], true);
        feed(&mut pipeline, 30, true);
        feed(&mut pipeline, 20, false);
        assert_eq!(pipeline.state(), "LISTENING");
        feed(&mut pipeline, 249, false);
        assert_eq!(pipeline.state(), "LISTENING");
        feed(&mut pipeline, 1, false);
        assert_eq!(pipeline.state(), "IDLE");
        assert_eq!(count(&events, is_utterance), 0);
    }

    #[test]
    fn no_wake_word_means_no_events() {
        let (mut pipeline, events, _, _) = build(Vec::new(), true);
        feed(&mut pipeline, 60, true);
        feed(&mut pipeline, 40, false);
        assert_eq!(count(&events, is_wake), 0);
        assert_eq!(pipeline.state(), "IDLE");
    }

    #[test]
    fn recognizer_hears_only_speech() {
        let (mut pipeline, _, accepted, _) = build(Vec::new(), true);
        feed(&mut pipeline, 200, false);
        assert_eq!(*accepted.borrow(), 0, "silence must not reach Vosk");
        feed(&mut pipeline, 30, true);
        assert!(*accepted.borrow() >= 30);
    }

    #[test]
    fn speaking_blocks_the_wake_word() {
        let (mut pipeline, events, accepted, _) = build(vec![name_word()], true);
        pipeline.set_speaking(true);
        assert_eq!(pipeline.state(), "SPEAKING");
        feed(&mut pipeline, 60, true);
        feed(&mut pipeline, 40, false);
        assert_eq!(count(&events, is_wake), 0);
        assert_eq!(*accepted.borrow(), 0);
        pipeline.set_speaking(false);
        assert_eq!(pipeline.state(), "IDLE");
        feed(&mut pipeline, 60, true);
        feed(&mut pipeline, 20, false);
        assert_eq!(count(&events, is_wake), 1);
    }

    #[test]
    fn cooldown_blocks_a_second_wake_for_1_5_seconds() {
        let (mut pipeline, events, _, _) = build(vec![name_word()], true);
        feed(&mut pipeline, 30, true);
        feed(&mut pipeline, 20, false);
        assert_eq!(count(&events, is_wake), 1);
        // Finish the command quickly: a short command, then silence.
        feed(&mut pipeline, 10, true);
        feed(&mut pipeline, 40, false);
        assert_eq!(count(&events, is_utterance), 1);
        pipeline.finish_processing();
        assert_eq!(pipeline.state(), "IDLE");
        // The cooldown (75 frames from the wake) is partly spent; speak again inside it.
        feed(&mut pipeline, 5, true);
        let wakes_during_cooldown = count(&events, is_wake);
        assert_eq!(wakes_during_cooldown, 1);
        // After the cooldown the next phrase is recognized again.
        feed(&mut pipeline, 100, false);
        feed(&mut pipeline, 30, true);
        feed(&mut pipeline, 20, false);
        assert_eq!(count(&events, is_wake), 2);
    }

    #[test]
    fn ptt_records_until_release_without_wake_word() {
        let (mut pipeline, events, accepted, _) = build(Vec::new(), true);
        pipeline.set_ptt(true);
        assert_eq!(pipeline.state(), "LISTENING");
        feed(&mut pipeline, 5, false);
        feed(&mut pipeline, 100, true);
        feed(&mut pipeline, 100, false);
        assert_eq!(count(&events, is_utterance), 0, "silence must not end PTT");
        pipeline.set_ptt(false);
        assert_eq!(count(&events, is_utterance), 1);
        assert_eq!(pipeline.state(), "PROCESSING");
        assert_eq!(*accepted.borrow(), 0, "Vosk is not used for PTT");
        let events = events.borrow();
        let Some(Event::Utterance { pcm16k, source }) =
            events.iter().find(|event| is_utterance(event))
        else {
            panic!("no utterance");
        };
        assert_eq!(*source, UtteranceSource::Ptt);
        assert_eq!(pcm16k.len(), 205 * FRAME_SAMPLES);
    }

    #[test]
    fn ptt_works_without_a_recognizer_and_tap_is_dropped() {
        let (mut pipeline, events, _, _) = build(Vec::new(), false);
        pipeline.set_ptt(true);
        feed(&mut pipeline, 3, true);
        pipeline.set_ptt(false);
        assert_eq!(count(&events, is_utterance), 0);
        assert_eq!(pipeline.state(), "IDLE");
        assert!(pipeline.is_idle());
        pipeline.set_ptt(true);
        feed(&mut pipeline, 50, true);
        pipeline.set_ptt(false);
        assert_eq!(count(&events, is_utterance), 1);
    }

    #[test]
    fn processing_ends_on_finish_or_timeout() {
        let (mut pipeline, _, _, _) = build(Vec::new(), false);
        pipeline.set_ptt(true);
        feed(&mut pipeline, 50, true);
        pipeline.set_ptt(false);
        assert_eq!(pipeline.state(), "PROCESSING");
        pipeline.finish_processing();
        assert_eq!(pipeline.state(), "IDLE");
        pipeline.set_ptt(true);
        feed(&mut pipeline, 50, true);
        pipeline.set_ptt(false);
        feed(&mut pipeline, 1499, false);
        assert_eq!(pipeline.state(), "PROCESSING");
        feed(&mut pipeline, 1, false);
        assert_eq!(pipeline.state(), "IDLE");
    }

    #[test]
    fn levels_are_sent_only_while_listening_and_enabled() {
        let (mut pipeline, events, _, level) = build(Vec::new(), false);
        let levels = |events: &Events| {
            count(events, |event| {
                matches!(event, Event::VoiceStateHint(VoiceHint::Level(_)))
            })
        };
        feed(&mut pipeline, 50, true);
        assert_eq!(levels(&events), 0, "idle sends no level");
        pipeline.set_ptt(true);
        feed(&mut pipeline, 50, true);
        let sent = levels(&events);
        assert!(sent > 15 && sent <= 30, "{sent} events in 1 s");
        level.store(false, Ordering::Relaxed);
        feed(&mut pipeline, 50, true);
        assert_eq!(levels(&events), sent, "no window, no level");
    }

    #[test]
    fn reports_the_microphone_peak_after_three_seconds() {
        let (mut pipeline, events, _, _) = build(Vec::new(), false);
        pipeline.push(&vec![0.0; 16_000 * 3]).expect("push");
        let events = events.borrow();
        let status = events.iter().find_map(|event| match event {
            Event::VoiceStateHint(VoiceHint::Mic(status)) => Some(status.clone()),
            _ => None,
        });
        let status = status.expect("mic status");
        assert!(status.no_signal);
        assert_eq!(status.peak, Some(0.0));
        assert_eq!(&*status.device, "test mic");
    }

    #[test]
    fn a_live_signal_is_not_flagged() {
        let (mut pipeline, events, _, _) = build(Vec::new(), false);
        let signal: Vec<f32> = (0..16_000 * 3)
            .map(|i| 0.1 * ((i % 50) as f32 / 50.0 - 0.5))
            .collect();
        pipeline.push(&signal).expect("push");
        let events = events.borrow();
        let status = events.iter().find_map(|event| match event {
            Event::VoiceStateHint(VoiceHint::Mic(status)) => Some(status.clone()),
            _ => None,
        });
        assert!(!status.expect("mic status").no_signal);
    }
}
