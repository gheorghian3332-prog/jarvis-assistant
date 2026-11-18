use std::sync::Arc;
use tokio::sync::broadcast;
pub const EVENT_BUS_CAPACITY: usize = 256;
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub cpu_percent: f32,
    pub used_memory_bytes: u64,
    pub total_memory_bytes: u64,
}
/// What started an [`Event::Utterance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtteranceSource {
    /// Wake word followed by a command.
    Wake,
    /// Push-to-talk key held.
    Ptt,
}

impl UtteranceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wake => "wake",
            Self::Ptt => "ptt",
        }
    }
}

/// Microphone health reported by the voice front (peak of the first seconds of capture).
#[derive(Debug, Clone, PartialEq)]
pub struct MicStatus {
    pub active: bool,
    pub device: Arc<str>,
    /// Peak level (0..1) of the first 3 s of capture; `None` until measured.
    pub peak: Option<f32>,
    /// True when the measured peak shows no signal at all.
    pub no_signal: bool,
    pub error: Option<Arc<str>>,
}

/// What the voice front tells the shell about itself.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceHint {
    /// Assistant state name (`IDLE`, `LISTENING`, `PROCESSING`, `SPEAKING`).
    State(Arc<str>),
    /// Input level, RMS 0..1 (sent at most 30 times a second, only while listening).
    Level(f32),
    Mic(MicStatus),
}

/// How serious an [`AlertNotice`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl AlertSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// A system alert (raised by the system intelligence) as the shell sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertNotice {
    pub id: Arc<str>,
    /// Rule name, for example `gpu_temp`.
    pub kind: Arc<str>,
    pub severity: AlertSeverity,
    pub message_ru: Arc<str>,
    pub message_en: Arc<str>,
    /// The voice layer may say it (rate limit and quiet hours are already applied).
    pub speak: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A system alert was raised.
    Alert(AlertNotice),
    /// An application ended unexpectedly.
    AppCrashed {
        name: Arc<str>,
        time_ms: u64,
    },
    StateChanged(Arc<str>),
    Metrics(Metrics),
    /// The wake word was heard.
    WakeDetected {
        name: Arc<str>,
        confidence: f32,
    },
    /// A finished phrase: mono 16 kHz PCM16, for the speech-to-text worker.
    Utterance {
        pcm16k: Arc<[i16]>,
        source: UtteranceSource,
    },
    /// What an utterance said (for the future brain and the UI).
    Transcript {
        text: Arc<str>,
        lang: Arc<str>,
        source: UtteranceSource,
    },
    VoiceStateHint(VoiceHint),
    Shutdown,
}
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Arc<Event>>,
}
impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self { sender }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.sender.subscribe()
    }
    pub fn publish(&self, event: Event) -> Result<usize, broadcast::error::SendError<Arc<Event>>> {
        self.sender.send(Arc::new(event))
    }
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}
impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn delivers_shared_event() {
        let bus = EventBus::new();
        let mut receiver = bus.subscribe();
        bus.publish(Event::Shutdown).expect("receiver exists");
        assert!(matches!(
            *receiver.recv().await.expect("event"),
            Event::Shutdown
        ));
    }
    #[tokio::test]
    async fn delivers_voice_events() {
        let bus = EventBus::new();
        let mut receiver = bus.subscribe();
        let pcm: Arc<[i16]> = Arc::from(vec![1_i16, 2, 3]);
        bus.publish(Event::Utterance {
            pcm16k: Arc::clone(&pcm),
            source: UtteranceSource::Ptt,
        })
        .expect("receiver exists");
        bus.publish(Event::WakeDetected {
            name: "лёха".into(),
            confidence: 0.9,
        })
        .expect("receiver exists");
        bus.publish(Event::VoiceStateHint(VoiceHint::Level(0.5)))
            .expect("receiver exists");
        assert_eq!(
            *receiver.recv().await.expect("event"),
            Event::Utterance {
                pcm16k: pcm,
                source: UtteranceSource::Ptt
            }
        );
        assert!(matches!(
            *receiver.recv().await.expect("event"),
            Event::WakeDetected { .. }
        ));
        assert_eq!(
            *receiver.recv().await.expect("event"),
            Event::VoiceStateHint(VoiceHint::Level(0.5))
        );
        assert_eq!(UtteranceSource::Wake.as_str(), "wake");
    }

    #[tokio::test]
    async fn reports_lagging_receiver() {
        let bus = EventBus::new();
        let mut receiver = bus.subscribe();
        for _ in 0..=EVENT_BUS_CAPACITY {
            bus.publish(Event::Shutdown).expect("receiver exists");
        }
        assert!(matches!(
            receiver.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
    }
}
