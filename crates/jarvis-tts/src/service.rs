//! The speech pipeline: text -> sentences -> synthesis (one sentence ahead) -> playback, with
//! interruption, the half-duplex state and the level envelope.

use crate::{
    clip::resample,
    config::TtsConfig,
    engine::{build_chain, synth_with_fallback, Engines},
    error::{TtsError, TtsResult},
    playback::{open_sink, LevelCallback, OutputSink, PlayQueue, SinkHandle},
    secret::{SecretStore, ELEVEN_KEY},
    text::{detect_lang, split_sentences, Lang},
};
use async_trait::async_trait;
use jarvis_core::{CoreResult, Service};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::sleep,
};
use tracing::{debug, info, warn};

/// After speech ends the half-duplex flag stays on this long (echo of the room).
pub const ECHO_TAIL: Duration = Duration::from_millis(300);
/// The next sentence is taken from the queue when this much audio is left.
const LOOKAHEAD_SECS: f32 = 0.3;

/// What the rest of the application learns about the speech.
pub trait SpeechObserver: Send + Sync {
    /// `true` when the first audio of a speech starts; `false` 300 ms after the speech ended or
    /// was interrupted (state SPEAKING -> IDLE, half-duplex flag on -> off).
    fn speaking(&self, active: bool);
    /// RMS (0..1) of the audio being played, 30 times a second; only while level events are enabled.
    fn level(&self, rms: f32);
}

/// Observer that ignores everything.
pub struct NoObserver;

impl SpeechObserver for NoObserver {
    fn speaking(&self, _active: bool) {}
    fn level(&self, _rms: f32) {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechOutcome {
    /// Everything was spoken.
    Completed,
    /// `stop` (or a newer speech) cut it short.
    Interrupted,
    Failed(String),
}

/// What happened during one speech.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechReport {
    pub outcome: SpeechOutcome,
    /// From the `speak` call to the first sample handed to the output.
    pub first_audio: Option<Duration>,
    /// Sentences that were synthesized.
    pub sentences: usize,
    /// Engine that produced each sentence.
    pub engines: Vec<&'static str>,
    /// Engines that failed and were skipped (cloud timeouts, missing files).
    pub skipped: Vec<&'static str>,
    pub lang: Lang,
    /// Total length of the synthesized audio.
    pub audio_secs: f32,
}

/// Waits for a speech started by [`TtsService::speak`].
pub struct SpeechHandle {
    done: oneshot::Receiver<SpeechReport>,
    lang: Lang,
}

impl SpeechHandle {
    /// Resolves when the speech is over (or was interrupted).
    pub async fn wait(self) -> SpeechReport {
        let lang = self.lang;
        self.done.await.unwrap_or(SpeechReport {
            outcome: SpeechOutcome::Interrupted,
            first_audio: None,
            sentences: 0,
            engines: Vec::new(),
            skipped: Vec::new(),
            lang,
            audio_secs: 0.0,
        })
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Speech {
    id: u64,
    task: JoinHandle<()>,
    queue: Arc<Mutex<Option<Arc<PlayQueue>>>>,
    sink: Arc<Mutex<Option<SinkHandle>>>,
}

struct Speaking {
    on: bool,
    epoch: u64,
}

struct Inner {
    engines: Engines,
    secrets: Arc<dyn SecretStore>,
    observer: Arc<dyn SpeechObserver>,
    config: Mutex<TtsConfig>,
    sink_override: Mutex<Option<OutputSink>>,
    level_enabled: AtomicBool,
    current: Mutex<Option<Speech>>,
    speaking: Mutex<Speaking>,
    next_id: AtomicU64,
    echo_tail: Duration,
    runtime: Handle,
}

impl Inner {
    fn begin_audio(&self) {
        let mut state = lock(&self.speaking);
        state.epoch += 1;
        if !state.on {
            state.on = true;
            self.observer.speaking(true);
        }
    }

    /// The audio ended: the flag goes off after the echo tail unless speech resumes.
    fn end_audio(self: &Arc<Self>) {
        let epoch = {
            let mut state = lock(&self.speaking);
            if !state.on {
                return;
            }
            state.epoch += 1;
            state.epoch
        };
        let inner = Arc::clone(self);
        self.runtime.spawn(async move {
            sleep(inner.echo_tail).await;
            let mut state = lock(&inner.speaking);
            if state.on && state.epoch == epoch {
                state.on = false;
                inner.observer.speaking(false);
            }
        });
    }

    fn sink(&self, config: &TtsConfig) -> OutputSink {
        lock(&self.sink_override)
            .clone()
            .unwrap_or_else(|| OutputSink::Device(config.output_device.clone()))
    }

    async fn cloud_key_present(self: &Arc<Self>) -> bool {
        if self.engines.cloud.is_none() {
            return false;
        }
        let inner = Arc::clone(self);
        tokio::task::spawn_blocking(move || inner.secrets.exists(ELEVEN_KEY).unwrap_or(false))
            .await
            .unwrap_or(false)
    }
}

/// The speech output of Jarvis.
#[derive(Clone)]
pub struct TtsService {
    inner: Arc<Inner>,
}

impl TtsService {
    /// Must be called inside a tokio runtime (the service spawns its tasks there).
    pub fn new(
        engines: Engines,
        secrets: Arc<dyn SecretStore>,
        observer: Arc<dyn SpeechObserver>,
        config: TtsConfig,
    ) -> Self {
        Self::with_options(engines, secrets, observer, config, None, ECHO_TAIL)
    }

    /// Like [`TtsService::new`] with an explicit output (for example a WAV file) and echo tail.
    pub fn with_options(
        engines: Engines,
        secrets: Arc<dyn SecretStore>,
        observer: Arc<dyn SpeechObserver>,
        config: TtsConfig,
        sink: Option<OutputSink>,
        echo_tail: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                engines,
                secrets,
                observer,
                config: Mutex::new(config),
                sink_override: Mutex::new(sink),
                level_enabled: AtomicBool::new(false),
                current: Mutex::new(None),
                speaking: Mutex::new(Speaking {
                    on: false,
                    epoch: 0,
                }),
                next_id: AtomicU64::new(1),
                echo_tail,
                runtime: Handle::current(),
            }),
        }
    }

    pub fn config(&self) -> TtsConfig {
        lock(&self.inner.config).clone()
    }

    /// Applies new settings; the volume also changes the speech in progress.
    pub fn set_config(&self, config: TtsConfig) {
        let volume = config.volume_clamped();
        *lock(&self.inner.config) = config;
        if let Some(speech) = lock(&self.inner.current).as_ref() {
            if let Some(queue) = lock(&speech.queue).as_ref() {
                queue.set_gain(volume);
            }
        }
    }

    /// Replaces the output (None = the configured device).
    pub fn set_sink(&self, sink: Option<OutputSink>) {
        *lock(&self.inner.sink_override) = sink;
    }

    /// Level events are produced only while a window can show them.
    pub fn set_level_enabled(&self, enabled: bool) {
        self.inner.level_enabled.store(enabled, Ordering::Relaxed);
    }

    /// True from the first audio of a speech until 300 ms after its end.
    pub fn is_speaking(&self) -> bool {
        lock(&self.inner.speaking).on
    }

    /// True while a speech is being synthesized or played.
    pub fn is_busy(&self) -> bool {
        lock(&self.inner.current)
            .as_ref()
            .is_some_and(|speech| !speech.task.is_finished())
    }

    /// Starts speaking `text`; a speech in progress is interrupted first. Returns at once.
    pub fn speak(&self, text: &str) -> TtsResult<SpeechHandle> {
        let sentences = split_sentences(text);
        if sentences.is_empty() {
            return Err(TtsError::Invalid("nothing to say".into()));
        }
        let lang = detect_lang(text);
        self.stop();

        let inner = Arc::clone(&self.inner);
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
        let queue_slot: Arc<Mutex<Option<Arc<PlayQueue>>>> = Arc::new(Mutex::new(None));
        let sink_slot: Arc<Mutex<Option<SinkHandle>>> = Arc::new(Mutex::new(None));
        let (report_tx, report_rx) = oneshot::channel();
        let started = Instant::now();
        let task = {
            let (queue_slot, sink_slot) = (Arc::clone(&queue_slot), Arc::clone(&sink_slot));
            self.inner.runtime.spawn(async move {
                let report =
                    run_speech(&inner, sentences, lang, started, &queue_slot, &sink_slot).await;
                // The sink is closed off the async workers (joining the device thread blocks).
                let sink = lock(&sink_slot).take();
                if let Some(sink) = sink {
                    let _ = tokio::task::spawn_blocking(move || drop(sink)).await;
                }
                lock(&inner.current).take_if(|speech| speech.id == id);
                inner.end_audio();
                let _ = report_tx.send(report);
            })
        };
        *lock(&self.inner.current) = Some(Speech {
            id,
            task,
            queue: queue_slot,
            sink: sink_slot,
        });
        Ok(SpeechHandle {
            done: report_rx,
            lang,
        })
    }

    /// Interrupts the speech: the audio stops within 100 ms and the queue is cleared; the
    /// half-duplex flag goes off after the echo tail. Safe to call at any time, from any thread.
    pub fn stop(&self) {
        let speech = lock(&self.inner.current).take();
        let Some(speech) = speech else {
            return;
        };
        // Silence first: the output fades out within ~10 ms.
        if let Some(queue) = lock(&speech.queue).take() {
            queue.stop();
        }
        speech.task.abort();
        if let Some(sink) = lock(&speech.sink).take() {
            // Closing the device joins its thread: not on the caller's thread.
            std::thread::spawn(move || drop(sink));
        }
        self.inner.end_audio();
        info!("speech interrupted");
    }

    /// Speaks and waits until the speech is over.
    pub async fn speak_and_wait(&self, text: &str) -> TtsResult<SpeechReport> {
        Ok(self.speak(text)?.wait().await)
    }
}

/// Everything one speech does; returns what happened.
async fn run_speech(
    inner: &Arc<Inner>,
    sentences: Vec<String>,
    lang: Lang,
    started: Instant,
    queue_slot: &Arc<Mutex<Option<Arc<PlayQueue>>>>,
    sink_slot: &Arc<Mutex<Option<SinkHandle>>>,
) -> SpeechReport {
    let config = lock(&inner.config).clone();
    let mut report = SpeechReport {
        outcome: SpeechOutcome::Completed,
        first_audio: None,
        sentences: 0,
        engines: Vec::new(),
        skipped: Vec::new(),
        lang,
        audio_secs: 0.0,
    };
    let key_present = inner.cloud_key_present().await;
    let chain = build_chain(&inner.engines, &config, lang, key_present);
    let speed = config.speed_clamped();

    // The output opens while the first sentence is being synthesized.
    let level: Option<LevelCallback> = {
        let observer = Arc::clone(&inner.observer);
        let inner = Arc::clone(inner);
        Some(Arc::new(move |rms| {
            if inner.level_enabled.load(Ordering::Relaxed) {
                observer.level(rms);
            }
        }))
    };
    let sink = inner.sink(&config);
    let gain = config.volume_clamped();
    let opener = tokio::task::spawn_blocking(move || open_sink(&sink, gain, level));

    // One sentence ahead: a permit is taken before synthesis, so the producer is never more than
    // one finished sentence in front of the player.
    let (tx, mut rx) = mpsc::channel(1);
    let producer = tokio::spawn(async move {
        for sentence in sentences {
            let Ok(permit) = tx.reserve().await else {
                return;
            };
            let result = synth_with_fallback(&chain, &sentence, lang, speed).await;
            permit.send(result);
        }
    });
    struct AbortOnDrop(JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _producer = AbortOnDrop(producer);

    let (queue, sink) = match opener.await {
        Ok(Ok(opened)) => opened,
        Ok(Err(error)) => {
            report.outcome = SpeechOutcome::Failed(error.to_string());
            return report;
        }
        Err(error) => {
            report.outcome = SpeechOutcome::Failed(format!("audio output task failed: {error}"));
            return report;
        }
    };
    *lock(queue_slot) = Some(Arc::clone(&queue));
    *lock(sink_slot) = Some(sink);
    debug!("audio output is open");

    let mut failure: Option<String> = None;
    while let Some(item) = rx.recv().await {
        match item {
            Ok(synthesized) => {
                report.sentences += 1;
                report.audio_secs += synthesized.clip.duration_secs();
                report.engines.push(synthesized.engine);
                report.skipped.extend(synthesized.skipped);
                let rate = queue.sample_rate();
                let samples = match tokio::task::spawn_blocking(move || {
                    resample(
                        &synthesized.clip.to_f32(),
                        synthesized.clip.sample_rate,
                        rate,
                    )
                })
                .await
                {
                    Ok(samples) => samples,
                    Err(error) => {
                        failure = Some(format!("resampling failed: {error}"));
                        break;
                    }
                };
                queue.push(samples);
                if report.first_audio.is_none() {
                    report.first_audio = Some(started.elapsed());
                    inner.begin_audio();
                }
                // Take the next sentence only when this one is almost played.
                queue.wait_pending_below(LOOKAHEAD_SECS).await;
            }
            Err(error) => {
                warn!(%error, "a sentence could not be synthesized");
                failure = Some(error.to_string());
                break;
            }
        }
    }
    queue.finish();
    queue.wait_finished().await;
    if let Some(message) = failure {
        report.outcome = SpeechOutcome::Failed(message);
    }
    report
}

#[async_trait]
impl Service for TtsService {
    fn name(&self) -> &'static str {
        "tts"
    }

    /// Does nothing until cancelled: engines start on the first phrase.
    async fn start(&self) -> CoreResult<()> {
        std::future::pending().await
    }

    async fn stop(&self) -> CoreResult<()> {
        TtsService::stop(self);
        let engines = &self.inner.engines;
        engines.piper.shutdown().await;
        engines.kokoro.shutdown().await;
        if let Some(cloud) = &engines.cloud {
            cloud.shutdown().await;
        }
        Ok(())
    }

    fn critical(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clip::AudioClip,
        config::EngineKind,
        engine::fake::FakeEngine,
        secret::{MemoryStore, SecretString},
    };

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<(&'static str, Instant)>>,
        levels: Mutex<Vec<f32>>,
    }

    impl Recorder {
        fn names(&self) -> Vec<&'static str> {
            lock(&self.events).iter().map(|(name, _)| *name).collect()
        }
    }

    impl SpeechObserver for Recorder {
        fn speaking(&self, active: bool) {
            lock(&self.events).push((if active { "on" } else { "off" }, Instant::now()));
        }

        fn level(&self, rms: f32) {
            lock(&self.levels).push(rms);
        }
    }

    struct Fixture {
        service: TtsService,
        observer: Arc<Recorder>,
        piper: Arc<FakeEngine>,
        kokoro: Arc<FakeEngine>,
        cloud: Arc<FakeEngine>,
        secrets: Arc<MemoryStore>,
        _dir: tempfile::TempDir,
        wav: std::path::PathBuf,
    }

    fn fixture(piper: Arc<FakeEngine>, kokoro: Arc<FakeEngine>, cloud: Arc<FakeEngine>) -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav = dir.path().join("speech.wav");
        let observer = Arc::new(Recorder::default());
        let secrets = Arc::new(MemoryStore::default());
        let engines = Engines {
            piper: piper.clone(),
            kokoro: kokoro.clone(),
            cloud: Some(cloud.clone()),
        };
        let service = TtsService::with_options(
            engines,
            secrets.clone(),
            observer.clone(),
            TtsConfig {
                volume: 1.0,
                ..TtsConfig::default()
            },
            Some(OutputSink::Wav { path: wav.clone() }),
            Duration::from_millis(120),
        );
        service.set_level_enabled(true);
        Fixture {
            service,
            observer,
            piper,
            kokoro,
            cloud,
            secrets,
            _dir: dir,
            wav,
        }
    }

    fn simple() -> Fixture {
        fixture(
            FakeEngine::with("piper", 400, Duration::from_millis(30)),
            FakeEngine::with("kokoro", 400, Duration::from_millis(30)),
            FakeEngine::with("elevenlabs", 400, Duration::from_millis(30)),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn speaks_russian_with_piper_and_reports_the_envelope() {
        let f = simple();
        let report = f
            .service
            .speak_and_wait("Привет. Как дела? Всё хорошо.")
            .await
            .expect("speak");
        assert_eq!(report.outcome, SpeechOutcome::Completed);
        assert_eq!(report.lang, Lang::Ru);
        assert_eq!(report.sentences, 3);
        assert_eq!(report.engines, ["piper", "piper", "piper"]);
        assert_eq!(f.kokoro.call_count(), 0);
        assert!(report.first_audio.expect("first audio") < Duration::from_millis(400));
        assert!(f.service.is_speaking(), "still inside the echo tail");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!f.service.is_speaking());
        assert_eq!(f.observer.names(), ["on", "off"]);
        let levels = lock(&f.observer.levels).clone();
        assert!(
            levels.len() > 30,
            "about 30 per second for 1.2 s: {}",
            levels.len()
        );
        assert!(levels.iter().any(|level| *level > 0.2), "8000/32768 = 0.24");
        assert_eq!(levels.last(), Some(&0.0));
        let clip = AudioClip::read_wav(&f.wav).expect("wav");
        assert!(
            clip.duration_secs() >= 1.2 && clip.duration_secs() < 2.2,
            "{}",
            clip.duration_secs()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn english_goes_to_kokoro_with_the_configured_voice() {
        let f = simple();
        let report = f
            .service
            .speak_and_wait("Good evening, sir.")
            .await
            .expect("speak");
        assert_eq!(report.engines, ["kokoro"]);
        let calls = lock(&f.kokoro.calls).clone();
        assert_eq!(calls[0].2, "bm_george");
        assert_eq!(f.piper.call_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_cloud_is_used_only_when_enabled_and_keyed_and_falls_back_on_failure() {
        let f = fixture(
            FakeEngine::new("piper"),
            FakeEngine::new("kokoro"),
            FakeEngine::failing("elevenlabs"),
        );
        let mut config = f.service.config();
        config.cloud_enabled = true;
        config.eleven_voice_id = "voice123".into();
        f.service.set_config(config.clone());
        // No key: the cloud is not even tried.
        let report = f.service.speak_and_wait("Привет.").await.expect("speak");
        assert_eq!(f.cloud.call_count(), 0);
        assert_eq!(report.engines, ["piper"]);
        // Key present, cloud fails: the same phrase is spoken locally.
        f.secrets
            .set(ELEVEN_KEY, SecretString::new("not-a-real-key"))
            .expect("key");
        let report = f.service.speak_and_wait("Привет.").await.expect("speak");
        assert_eq!(f.cloud.call_count(), 1);
        assert_eq!(report.engines, ["piper"]);
        assert_eq!(report.skipped, ["elevenlabs"]);
        assert_eq!(report.outcome, SpeechOutcome::Completed);
        let calls = lock(&f.piper.calls).clone();
        assert_eq!(calls.last().expect("call").0, "Привет.");
        // Cloud switched off again: not tried.
        config.cloud_enabled = false;
        f.service.set_config(config);
        f.service.speak_and_wait("Привет.").await.expect("speak");
        assert_eq!(f.cloud.call_count(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn synthesis_runs_one_sentence_ahead_and_no_more() {
        // Each sentence is 1 s of audio and takes 50 ms to synthesize.
        let piper = FakeEngine::with("piper", 1_000, Duration::from_millis(50));
        let f = fixture(
            piper.clone(),
            FakeEngine::new("kokoro"),
            FakeEngine::new("elevenlabs"),
        );
        let handle = f
            .service
            .speak("Первая. Вторая. Третья. Четвёртая.")
            .expect("speak");
        tokio::time::sleep(Duration::from_millis(400)).await;
        // Sentence 1 is playing, sentence 2 is ready; sentence 3 waits for 2 to be taken.
        assert_eq!(piper.call_count(), 2, "one ahead of what plays");
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert!(piper.call_count() >= 3, "{}", piper.call_count());
        f.service.stop();
        let report = handle.wait().await;
        assert_eq!(report.outcome, SpeechOutcome::Interrupted);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stop_silences_within_100_ms_clears_the_queue_and_releases_the_flag_after_the_tail() {
        let f = fixture(
            FakeEngine::with("piper", 3_000, Duration::from_millis(20)),
            FakeEngine::new("kokoro"),
            FakeEngine::new("elevenlabs"),
        );
        let handle = f
            .service
            .speak("Длинная фраза. И ещё одна.")
            .expect("speak");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(f.service.is_speaking());
        let levels_before = lock(&f.observer.levels).len();
        let stopped_at = Instant::now();
        f.service.stop();
        // The level meter reports silence at once.
        let mut silent_after = None;
        while stopped_at.elapsed() < Duration::from_millis(300) {
            if lock(&f.observer.levels).last() == Some(&0.0)
                && lock(&f.observer.levels).len() > levels_before
            {
                silent_after = Some(stopped_at.elapsed());
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let silent_after = silent_after.expect("level reached zero");
        assert!(
            silent_after <= Duration::from_millis(100),
            "{silent_after:?}"
        );
        assert_eq!(handle.wait().await.outcome, SpeechOutcome::Interrupted);
        assert!(
            f.service.is_speaking(),
            "flag still on during the echo tail"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(!f.service.is_speaking());
        let events = lock(&f.observer.events).clone();
        assert_eq!(f.observer.names(), ["on", "off"]);
        let tail = events[1].1.duration_since(stopped_at);
        assert!(tail >= Duration::from_millis(110), "tail {tail:?}");
        assert!(!f.service.is_busy());
        // A second stop is harmless, and the service speaks again afterwards.
        f.service.stop();
        let report = f.service.speak_and_wait("Снова.").await.expect("speak");
        assert_eq!(report.outcome, SpeechOutcome::Completed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_new_speech_replaces_the_old_one_without_a_false_idle() {
        let f = fixture(
            FakeEngine::with("piper", 2_000, Duration::from_millis(20)),
            FakeEngine::new("kokoro"),
            FakeEngine::new("elevenlabs"),
        );
        let first = f.service.speak("Первая фраза.").expect("first");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let second = f.service.speak("Вторая фраза.").expect("second");
        assert_eq!(first.wait().await.outcome, SpeechOutcome::Interrupted);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            f.service.is_speaking(),
            "the second speech started inside the first one's tail"
        );
        assert_eq!(second.wait().await.outcome, SpeechOutcome::Completed);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            f.observer.names(),
            ["on", "off"],
            "one continuous SPEAKING period"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failing_engine_reports_failure_and_never_enters_speaking() {
        let f = fixture(
            FakeEngine::failing("piper"),
            FakeEngine::failing("kokoro"),
            FakeEngine::new("elevenlabs"),
        );
        let report = f.service.speak_and_wait("Привет.").await.expect("speak");
        assert!(
            matches!(report.outcome, SpeechOutcome::Failed(_)),
            "{report:?}"
        );
        assert_eq!(report.first_audio, None);
        assert!(f.observer.names().is_empty());
        assert!(!f.service.is_speaking());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_empty_text_is_refused() {
        let f = simple();
        assert!(f.service.speak("   ").is_err());
        assert!(f.service.speak("...").is_err());
        assert!(!f.service.is_busy());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_volume_scales_what_is_played_and_never_exceeds_it() {
        let f = simple();
        let mut config = f.service.config();
        config.volume = 0.25;
        f.service.set_config(config);
        f.service.speak_and_wait("Привет.").await.expect("speak");
        let clip = AudioClip::read_wav(&f.wav).expect("wav");
        let expected = 8_000.0 * 0.25;
        let peak = f64::from(clip.peak());
        assert!(
            peak <= expected * 1.05,
            "peak {peak} must not exceed volume * source {expected}"
        );
        assert!(peak >= expected * 0.9, "peak {peak}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn level_events_are_off_without_a_window() {
        let f = simple();
        f.service.set_level_enabled(false);
        f.service.speak_and_wait("Привет.").await.expect("speak");
        assert!(lock(&f.observer.levels).is_empty());
    }

    #[test]
    fn local_engines_follow_the_settings() {
        let config = TtsConfig {
            engine_en: EngineKind::Piper,
            ..TtsConfig::default()
        };
        assert_eq!(config.local_for(Lang::En).0, EngineKind::Piper);
    }
}
