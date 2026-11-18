//! What happens with a system alert in the shell: `alert://raised` for the window, a Windows
//! notification while the window is closed, a short `WARNING` / `ERROR` of the Neural Core, and the
//! spoken message (only when the alert says `speak`, and never over a conversation).
//!
//! Nothing here acts on applications: an alert is only a message.

use async_trait::async_trait;
use jarvis_core::{AlertNotice, AlertSeverity, Event, EventBus};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast::error::RecvError;

/// How long the Neural Core shows `WARNING`.
pub const WARNING_HOLD: Duration = Duration::from_secs(4);
/// How long it shows `ERROR` for a critical alert.
pub const ERROR_HOLD: Duration = Duration::from_secs(2);
/// A spoken alert waits for a free voice at most this long, then stays UI-only.
pub const SPEECH_MAX_WAIT: Duration = Duration::from_secs(120);
const SPEECH_POLL: Duration = Duration::from_millis(500);

/// `alert://raised`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlertPayload {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub message_ru: String,
    pub message_en: String,
    pub speak: bool,
    /// Unix time in milliseconds.
    pub at_ms: u64,
}

impl AlertPayload {
    pub fn from_notice(notice: &AlertNotice, at_ms: u64) -> Self {
        Self {
            id: notice.id.to_string(),
            kind: notice.kind.to_string(),
            severity: notice.severity.as_str().to_owned(),
            message_ru: notice.message_ru.to_string(),
            message_en: notice.message_en.to_string(),
            speak: notice.speak,
            at_ms,
        }
    }
}

/// The outside world of the pipeline (the window, notifications, the voice front).
#[async_trait]
pub trait AlertShell: Send + Sync {
    fn window_open(&self) -> bool;
    /// Sends `alert://raised` to the window.
    fn raise(&self, payload: &AlertPayload);
    /// A system notification (used while the window is closed).
    fn notify(&self, title: &str, body: &str);
    /// The assistant state as the Neural Core shows it.
    fn voice_state(&self) -> String;
    fn show_state(&self, state: &str);
    fn speak(&self, text: &str);
    /// `general.language` is English.
    async fn english(&self) -> bool;
}

pub struct AlertPipeline {
    shell: Arc<dyn AlertShell>,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The voice is busy with the user: the alert must wait.
fn voice_busy(state: &str) -> bool {
    matches!(state, "SPEAKING" | "LISTENING")
}

impl AlertPipeline {
    pub fn new(shell: Arc<dyn AlertShell>) -> Arc<Self> {
        Arc::new(Self { shell })
    }

    /// Delivers one alert everywhere it belongs.
    pub async fn handle(self: &Arc<Self>, notice: &AlertNotice) {
        self.shell
            .raise(&AlertPayload::from_notice(notice, unix_ms()));
        let english = self.shell.english().await;
        let text = if english {
            notice.message_en.to_string()
        } else {
            notice.message_ru.to_string()
        };
        if !self.shell.window_open() {
            self.shell.notify("J.A.R.V.I.S.", &text);
        }
        self.flash_core(notice.severity);
        if notice.speak {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.speak_when_free(&text).await });
        }
    }

    /// `WARNING` for 4 s (`ERROR` for 2 s when critical), unless the user is in a conversation.
    fn flash_core(self: &Arc<Self>, severity: AlertSeverity) {
        if self.shell.voice_state() != "IDLE" {
            return;
        }
        let (state, hold) = match severity {
            AlertSeverity::Critical => ("ERROR", ERROR_HOLD),
            AlertSeverity::Warning | AlertSeverity::Info => ("WARNING", WARNING_HOLD),
        };
        self.shell.show_state(state);
        let this = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(hold).await;
            // Back to IDLE only if nothing else (a recording, a speech) took the state meanwhile.
            if this.shell.voice_state() == state {
                this.shell.show_state("IDLE");
            }
        });
    }

    async fn speak_when_free(&self, text: &str) {
        let started = tokio::time::Instant::now();
        loop {
            if !voice_busy(&self.shell.voice_state()) {
                self.shell.speak(text);
                return;
            }
            if started.elapsed() >= SPEECH_MAX_WAIT {
                tracing::info!("alert not spoken: the voice stayed busy, the window shows it");
                return;
            }
            tokio::time::sleep(SPEECH_POLL).await;
        }
    }

    /// Listens to the bus until it closes.
    pub async fn run(self: Arc<Self>, bus: EventBus) {
        let mut receiver = bus.subscribe();
        loop {
            match receiver.recv().await {
                Ok(event) => match &*event {
                    Event::Alert(notice) => self.handle(notice).await,
                    Event::AppCrashed { name, time_ms } => {
                        tracing::info!(%name, time_ms, "application crash noticed");
                    }
                    Event::Shutdown => return,
                    _ => {}
                },
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    }
}

/// The real shell: the Tauri window, the notification plugin, the voice front and the speech.
pub struct AppAlerts {
    app: tauri::AppHandle,
}

impl AppAlerts {
    pub fn new(app: tauri::AppHandle) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

#[async_trait]
impl AlertShell for AppAlerts {
    fn window_open(&self) -> bool {
        crate::window::window_exists(&self.app)
    }

    fn raise(&self, payload: &AlertPayload) {
        use tauri::Emitter;
        if let Err(error) = self.app.emit("alert://raised", payload) {
            tracing::debug!(%error, "alert event was not delivered");
        }
    }

    fn notify(&self, title: &str, body: &str) {
        use tauri_plugin_notification::NotificationExt;
        if let Err(error) = self
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            tracing::warn!(%error, "the notification was not shown");
        }
    }

    fn voice_state(&self) -> String {
        use tauri::Manager;
        self.app
            .state::<crate::state::AppState>()
            .voice
            .snapshot()
            .state
    }

    fn show_state(&self, state: &str) {
        use tauri::Manager;
        self.app
            .state::<crate::state::AppState>()
            .voice
            .show_state(state);
    }

    fn speak(&self, text: &str) {
        use tauri::Manager;
        if let Err(error) = self.app.state::<crate::state::AppState>().tts.speak(text) {
            tracing::warn!(%error, "the alert could not be spoken");
        }
    }

    async fn english(&self) -> bool {
        use tauri::Manager;
        let db = self.app.state::<crate::state::AppState>().db.clone();
        db.settings_get(crate::settings::LANGUAGE.to_owned())
            .await
            .ok()
            .flatten()
            .is_some_and(|value| value.as_str() == Some("en"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        window: Mutex<bool>,
        state: Mutex<String>,
        raised: Mutex<Vec<AlertPayload>>,
        notes: Mutex<Vec<String>>,
        spoken: Mutex<Vec<String>>,
        shown: Mutex<Vec<String>>,
    }

    fn get<T: Clone>(m: &Mutex<T>) -> T {
        m.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn put<T>(m: &Mutex<T>, value: T) {
        *m.lock().unwrap_or_else(|p| p.into_inner()) = value;
    }

    #[async_trait]
    impl AlertShell for Fake {
        fn window_open(&self) -> bool {
            get(&self.window)
        }
        fn raise(&self, payload: &AlertPayload) {
            self.raised
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(payload.clone());
        }
        fn notify(&self, _title: &str, body: &str) {
            self.notes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(body.to_owned());
        }
        fn voice_state(&self) -> String {
            get(&self.state)
        }
        fn show_state(&self, state: &str) {
            put(&self.state, state.to_owned());
            self.shown
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(state.to_owned());
        }
        fn speak(&self, text: &str) {
            self.spoken
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(text.to_owned());
        }
        async fn english(&self) -> bool {
            false
        }
    }

    fn fake(state: &str, window: bool) -> Arc<Fake> {
        let fake = Arc::new(Fake::default());
        put(&fake.state, state.to_owned());
        put(&fake.window, window);
        fake
    }

    fn notice(severity: AlertSeverity, speak: bool) -> AlertNotice {
        AlertNotice {
            id: "a-1".into(),
            kind: "gpu_temp".into(),
            severity,
            message_ru: "Видеокарта перегрета".into(),
            message_en: "The GPU is overheating".into(),
            speak,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_alert_reaches_the_window_and_a_closed_window_gets_a_notification() {
        let shell = fake("IDLE", false);
        let pipeline = AlertPipeline::new(shell.clone());
        pipeline
            .handle(&notice(AlertSeverity::Warning, false))
            .await;
        let raised = get(&shell.raised);
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].kind, "gpu_temp");
        assert_eq!(raised[0].severity, "warning");
        assert_eq!(get(&shell.notes), ["Видеокарта перегрета"]);

        let open = fake("IDLE", true);
        AlertPipeline::new(open.clone())
            .handle(&notice(AlertSeverity::Warning, false))
            .await;
        assert_eq!(get(&open.raised).len(), 1);
        assert!(get(&open.notes).is_empty(), "the window shows it itself");
    }

    #[tokio::test(start_paused = true)]
    async fn the_core_warns_for_four_seconds_and_errors_for_two() {
        let shell = fake("IDLE", true);
        let pipeline = AlertPipeline::new(shell.clone());
        pipeline
            .handle(&notice(AlertSeverity::Warning, false))
            .await;
        assert_eq!(get(&shell.state), "WARNING");
        tokio::time::sleep(Duration::from_millis(3900)).await;
        assert_eq!(get(&shell.state), "WARNING");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(get(&shell.state), "IDLE");

        pipeline
            .handle(&notice(AlertSeverity::Critical, false))
            .await;
        assert_eq!(get(&shell.state), "ERROR");
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert_eq!(get(&shell.state), "IDLE");
    }

    #[tokio::test(start_paused = true)]
    async fn a_conversation_is_never_interrupted_by_the_core_state() {
        for busy in ["LISTENING", "SPEAKING", "PROCESSING"] {
            let shell = fake(busy, true);
            AlertPipeline::new(shell.clone())
                .handle(&notice(AlertSeverity::Critical, false))
                .await;
            assert!(get(&shell.shown).is_empty(), "{busy}");
            assert_eq!(get(&shell.state), busy);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_state_during_the_warning_is_left_alone() {
        let shell = fake("IDLE", true);
        AlertPipeline::new(shell.clone())
            .handle(&notice(AlertSeverity::Warning, false))
            .await;
        put(&shell.state, "LISTENING".to_owned());
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(get(&shell.state), "LISTENING");
    }

    #[tokio::test(start_paused = true)]
    async fn only_a_speak_alert_is_spoken_and_at_once_when_free() {
        let shell = fake("IDLE", true);
        let pipeline = AlertPipeline::new(shell.clone());
        pipeline
            .handle(&notice(AlertSeverity::Warning, false))
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(get(&shell.spoken).is_empty());
        pipeline.handle(&notice(AlertSeverity::Warning, true)).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(get(&shell.spoken), ["Видеокарта перегрета"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_spoken_alert_waits_for_the_conversation_to_end() {
        let shell = fake("SPEAKING", true);
        let pipeline = AlertPipeline::new(shell.clone());
        pipeline.handle(&notice(AlertSeverity::Warning, true)).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(get(&shell.spoken).is_empty(), "still busy");
        put(&shell.state, "IDLE".to_owned());
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(get(&shell.spoken).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_spoken_alert_gives_up_after_two_minutes() {
        let shell = fake("LISTENING", true);
        let pipeline = AlertPipeline::new(shell.clone());
        pipeline.handle(&notice(AlertSeverity::Warning, true)).await;
        tokio::time::sleep(SPEECH_MAX_WAIT + Duration::from_secs(5)).await;
        put(&shell.state, "IDLE".to_owned());
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(get(&shell.spoken).is_empty(), "UI only after two minutes");
        assert_eq!(get(&shell.raised).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_pipeline_follows_the_bus() {
        let shell = fake("IDLE", true);
        let bus = EventBus::new();
        let pipeline = AlertPipeline::new(shell.clone());
        let task = tokio::spawn(pipeline.run(bus.clone()));
        tokio::task::yield_now().await;
        bus.publish(Event::Alert(notice(AlertSeverity::Info, false)))
            .expect("subscribed");
        bus.publish(Event::AppCrashed {
            name: "x.exe".into(),
            time_ms: 1,
        })
        .expect("subscribed");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(get(&shell.raised).len(), 1);
        bus.publish(Event::Shutdown).expect("subscribed");
        let _ = task.await;
    }
}
