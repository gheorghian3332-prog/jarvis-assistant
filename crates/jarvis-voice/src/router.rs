//! Wake word -> speech-to-text -> transcript.
//!
//! The router listens to the bus: `WakeDetected` prewarms the STT worker (its cold start of a few seconds
//! then overlaps the user's command), `Utterance` is transcribed and published as `Event::Transcript`,
//! and the voice front is released (`finish_processing`, state IDLE). A failure shows the ERROR state
//! for two seconds first. An empty transcript goes straight back to IDLE.

use crate::service::VoiceService;
use async_trait::async_trait;
use jarvis_core::{
    CoreResult, Event, EventBus, Service, SttConfig, SttService, UtteranceSource, WorkerCommand,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, error, info, warn};

/// How long the ERROR state stays visible after a failed transcription.
pub const ERROR_HOLD: Duration = Duration::from_secs(2);
/// Environment variable with the repository root (where `workers/stt` lives).
pub const REPO_ROOT_ENV: &str = "JARVIS_REPO_ROOT";

/// What the speech-to-text worker returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spoken {
    pub text: String,
    pub lang: String,
    pub ms: u64,
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, pcm16k: &[i16]) -> Result<Spoken, String>;
    /// Starts the recognizer ahead of time. Failures are not fatal: the next `transcribe` reports them.
    async fn prewarm(&self) -> Result<(), String>;
}

#[async_trait]
impl Transcriber for SttService {
    async fn transcribe(&self, pcm16k: &[i16]) -> Result<Spoken, String> {
        SttService::transcribe(self, pcm16k, None)
            .await
            .map(|transcript| Spoken {
                text: transcript.text,
                lang: transcript.lang,
                ms: transcript.ms,
            })
            .map_err(|error| error.to_string())
    }

    async fn prewarm(&self) -> Result<(), String> {
        SttService::prewarm(self)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Stands in when the STT worker cannot be found: every request reports why.
pub struct UnavailableTranscriber {
    reason: String,
}

impl UnavailableTranscriber {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl Transcriber for UnavailableTranscriber {
    async fn transcribe(&self, _pcm16k: &[i16]) -> Result<Spoken, String> {
        Err(format!("UNAVAILABLE: {}", self.reason))
    }

    async fn prewarm(&self) -> Result<(), String> {
        Err(format!("UNAVAILABLE: {}", self.reason))
    }
}

/// The part of the voice front the router drives.
pub trait VoiceControl: Send + Sync {
    /// The utterance has been dealt with: back to IDLE.
    fn finish_processing(&self);
    /// Shows a state (for example `ERROR`) in the UI.
    fn show_state(&self, state: &str);
}

impl VoiceControl for VoiceService {
    fn finish_processing(&self) {
        VoiceService::finish_processing(self);
    }

    fn show_state(&self, state: &str) {
        VoiceService::show_state(self, state);
    }
}

/// Finds the repository root (the folder with `workers/stt/pyproject.toml`): `JARVIS_REPO_ROOT`, or the
/// first parent of the running executable that has it.
pub fn find_repo_root() -> Option<PathBuf> {
    let has_worker = |dir: &Path| dir.join("workers/stt/pyproject.toml").is_file();
    if let Some(root) = std::env::var_os(REPO_ROOT_ENV).map(PathBuf::from) {
        return has_worker(&root).then_some(root);
    }
    if jarvis_core::layout::resources_dir().is_some() {
        // An installed copy never mistakes a repository that happens to be above it for itself.
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .find(|dir| has_worker(dir))
        .map(Path::to_path_buf)
}

/// The STT backend for the app: the dev worker (`uv run ... jarvis_stt`) when the repository is found,
/// the installed worker (`%LOCALAPPDATA%\Jarvis\workers\stt\.venv`) next to an installed `jarvis.exe`,
/// otherwise a stand-in that reports UNAVAILABLE.
pub fn default_transcriber() -> Arc<dyn Transcriber> {
    if let Some(root) = find_repo_root() {
        info!(root = %root.display(), "speech-to-text worker found");
        return Arc::new(SttService::new(SttConfig::new(WorkerCommand::dev_stt(
            &root,
        ))));
    }
    if let Some(command) = jarvis_core::layout::installed_worker("stt", "jarvis_stt") {
        info!("installed speech-to-text worker");
        return Arc::new(SttService::new(SttConfig::new(command)));
    }
    error!("UNAVAILABLE: workers/stt not found; set {REPO_ROOT_ENV} to the repository folder");
    Arc::new(UnavailableTranscriber::new(format!(
        "workers/stt not found (set {REPO_ROOT_ENV})"
    )))
}

pub struct Router {
    bus: EventBus,
    stt: Arc<dyn Transcriber>,
    control: Arc<dyn VoiceControl>,
}

impl Router {
    pub fn new(bus: EventBus, stt: Arc<dyn Transcriber>, control: Arc<dyn VoiceControl>) -> Self {
        Self { bus, stt, control }
    }

    /// Reacts to one bus event.
    pub async fn handle(&self, event: &Event) {
        match event {
            Event::WakeDetected { .. } => {
                let stt = Arc::clone(&self.stt);
                tokio::spawn(async move {
                    if let Err(error) = stt.prewarm().await {
                        warn!(%error, "speech-to-text prewarm failed");
                    }
                });
            }
            Event::Utterance { pcm16k, source } => self.transcribe(pcm16k, *source).await,
            _ => {}
        }
    }

    async fn transcribe(&self, pcm16k: &[i16], source: UtteranceSource) {
        match self.stt.transcribe(pcm16k).await {
            Ok(spoken) if !spoken.text.trim().is_empty() => {
                info!(lang = %spoken.lang, ms = spoken.ms, "utterance transcribed");
                // Nobody may be subscribed; that is fine.
                let _ = self.bus.publish(Event::Transcript {
                    text: spoken.text.trim().into(),
                    lang: spoken.lang.as_str().into(),
                    source,
                });
            }
            Ok(_) => debug!("empty transcript"),
            Err(error) => {
                error!(%error, "speech-to-text failed");
                self.control.show_state("ERROR");
                tokio::time::sleep(ERROR_HOLD).await;
            }
        }
        self.control.finish_processing();
    }

    /// Runs until the task is dropped.
    pub async fn run(&self) {
        let mut receiver = self.bus.subscribe();
        loop {
            match receiver.recv().await {
                Ok(event) => self.handle(&event).await,
                // Level events may have been skipped during a long transcription.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    }
}

/// Runs the router under the supervisor.
pub struct RouterService {
    router: Router,
}

impl RouterService {
    pub fn new(
        bus: EventBus,
        stt: Arc<dyn Transcriber>,
        control: Arc<dyn VoiceControl>,
    ) -> Arc<Self> {
        Arc::new(Self {
            router: Router::new(bus, stt, control),
        })
    }
}

#[async_trait]
impl Service for RouterService {
    fn name(&self) -> &'static str {
        "voice-router"
    }

    async fn start(&self) -> CoreResult<()> {
        self.router.run().await;
        std::future::pending().await
    }

    async fn stop(&self) -> CoreResult<()> {
        Ok(())
    }

    fn critical(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    struct FakeStt {
        reply: Mutex<Result<Spoken, String>>,
        prewarms: AtomicUsize,
        transcribed: AtomicUsize,
    }

    impl FakeStt {
        fn new(reply: Result<Spoken, String>) -> Arc<Self> {
            Arc::new(Self {
                reply: Mutex::new(reply),
                prewarms: AtomicUsize::new(0),
                transcribed: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl Transcriber for FakeStt {
        async fn transcribe(&self, _pcm16k: &[i16]) -> Result<Spoken, String> {
            self.transcribed.fetch_add(1, Ordering::SeqCst);
            self.reply.lock().expect("reply").clone()
        }
        async fn prewarm(&self) -> Result<(), String> {
            self.prewarms.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeControl {
        finished: AtomicUsize,
        states: Mutex<Vec<String>>,
    }

    impl VoiceControl for FakeControl {
        fn finish_processing(&self) {
            self.finished.fetch_add(1, Ordering::SeqCst);
        }
        fn show_state(&self, state: &str) {
            self.states.lock().expect("states").push(state.to_owned());
        }
    }

    fn utterance() -> Event {
        Event::Utterance {
            pcm16k: Arc::from(vec![1_i16; 1600]),
            source: UtteranceSource::Wake,
        }
    }

    fn spoken(text: &str) -> Result<Spoken, String> {
        Ok(Spoken {
            text: text.into(),
            lang: "ru".into(),
            ms: 5,
        })
    }

    fn setup(reply: Result<Spoken, String>) -> (Router, Arc<FakeStt>, Arc<FakeControl>, EventBus) {
        let bus = EventBus::new();
        let stt = FakeStt::new(reply);
        let control = Arc::new(FakeControl::default());
        let router = Router::new(bus.clone(), stt.clone(), control.clone());
        (router, stt, control, bus)
    }

    #[tokio::test]
    async fn publishes_the_transcript_then_releases_the_front() {
        let (router, stt, control, bus) = setup(spoken("  открой блокнот "));
        let mut events = bus.subscribe();
        router.handle(&utterance()).await;
        let event = events.recv().await.expect("event");
        assert_eq!(
            *event,
            Event::Transcript {
                text: "открой блокнот".into(),
                lang: "ru".into(),
                source: UtteranceSource::Wake,
            }
        );
        assert_eq!(stt.transcribed.load(Ordering::SeqCst), 1);
        assert_eq!(control.finished.load(Ordering::SeqCst), 1);
        assert!(control.states.lock().expect("states").is_empty());
    }

    #[tokio::test]
    async fn empty_transcript_goes_back_to_idle_silently() {
        let (router, _, control, bus) = setup(spoken("   "));
        let mut events = bus.subscribe();
        router.handle(&utterance()).await;
        assert!(events.try_recv().is_err(), "no transcript event");
        assert_eq!(control.finished.load(Ordering::SeqCst), 1);
        assert!(control.states.lock().expect("states").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn failure_shows_error_for_two_seconds_then_idle() {
        let (router, _, control, bus) = setup(Err("worker died".into()));
        let mut events = bus.subscribe();
        let started = tokio::time::Instant::now();
        router.handle(&utterance()).await;
        assert!(started.elapsed() >= ERROR_HOLD);
        assert!(events.try_recv().is_err(), "no transcript on failure");
        assert_eq!(*control.states.lock().expect("states"), vec!["ERROR"]);
        assert_eq!(control.finished.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn wake_word_prewarms_but_plain_speech_events_do_not() {
        let (router, stt, control, _) = setup(spoken("x"));
        router
            .handle(&Event::WakeDetected {
                name: "лёха".into(),
                confidence: 1.0,
            })
            .await;
        // The prewarm runs in the background.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(stt.prewarms.load(Ordering::SeqCst), 1);
        assert_eq!(stt.transcribed.load(Ordering::SeqCst), 0);
        router.handle(&Event::Shutdown).await;
        assert_eq!(stt.prewarms.load(Ordering::SeqCst), 1);
        assert_eq!(control.finished.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unavailable_backend_reports_the_reason() {
        let backend = UnavailableTranscriber::new("no worker");
        let error = backend.transcribe(&[1]).await.expect_err("unavailable");
        assert!(error.starts_with("UNAVAILABLE"), "{error}");
        assert!(backend.prewarm().await.is_err());
    }

    #[tokio::test]
    async fn run_loop_routes_bus_events() {
        let (router, _, control, bus) = setup(spoken("громче"));
        let router = Arc::new(router);
        let task = tokio::spawn({
            let router = Arc::clone(&router);
            async move { router.run().await }
        });
        let mut events = bus.subscribe();
        // The router subscribes when it first runs.
        while bus.receiver_count() < 2 {
            tokio::task::yield_now().await;
        }
        bus.publish(utterance()).expect("receivers");
        loop {
            let event = events.recv().await.expect("event");
            if matches!(*event, Event::Transcript { .. }) {
                break;
            }
        }
        while control.finished.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert_eq!(control.finished.load(Ordering::SeqCst), 1);
    }
}
