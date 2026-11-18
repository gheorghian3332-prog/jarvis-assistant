use crate::{
    config::VoiceConfig,
    engine::{Command, EngineHandle, EngineKind, EventSink},
};
use async_trait::async_trait;
use jarvis_core::{CoreError, CoreResult, Event, EventBus, MicStatus, Service, VoiceHint};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::Duration,
};
use tokio::sync::Notify;

pub const SERVICE_NAME: &str = "voice";
/// A failed start (no microphone, no model) is retried this often while listening is enabled.
const RETRY_AFTER: Duration = Duration::from_secs(10);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The last things the voice front reported, for a window that opens later.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSnapshot {
    pub state: String,
    pub mic: Option<MicStatus>,
}

struct Shared {
    bus: EventBus,
    snapshot: Mutex<VoiceSnapshot>,
}

impl Shared {
    fn emit(&self, event: Event) {
        if let Event::VoiceStateHint(hint) = &event {
            let mut snapshot = lock(&self.snapshot);
            match hint {
                VoiceHint::State(state) => snapshot.state = state.to_string(),
                VoiceHint::Mic(status) => {
                    // A later "active" status without a peak must not erase a measured peak.
                    let mut status = status.clone();
                    if status.active && status.peak.is_none() {
                        if let Some(previous) = &snapshot.mic {
                            if previous.active && previous.device == status.device {
                                status.peak = previous.peak;
                                status.no_signal = previous.no_signal;
                            }
                        }
                    }
                    snapshot.mic = Some(status);
                }
                VoiceHint::Level(_) => {}
            }
        }
        // Nobody may be subscribed; that is fine.
        let _ = self.bus.publish(event);
    }
}

struct Inner {
    shared: Arc<Shared>,
    config: Mutex<VoiceConfig>,
    changed: Notify,
    engine: Mutex<Option<EngineHandle>>,
    level_enabled: Arc<AtomicBool>,
}

impl Inner {
    fn new(bus: EventBus, config: VoiceConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                bus,
                snapshot: Mutex::new(VoiceSnapshot {
                    state: "IDLE".into(),
                    mic: None,
                }),
            }),
            config: Mutex::new(config),
            changed: Notify::new(),
            engine: Mutex::new(None),
            level_enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn sink(&self) -> EventSink {
        let shared = Arc::clone(&self.shared);
        Arc::new(move |event| shared.emit(event))
    }

    fn snapshot(&self) -> VoiceSnapshot {
        lock(&self.shared.snapshot).clone()
    }

    fn config(&self) -> VoiceConfig {
        lock(&self.config).clone()
    }

    /// Applies new settings (asynchronously, on the service task).
    fn set_config(&self, config: VoiceConfig) {
        *lock(&self.config) = config;
        self.changed.notify_one();
    }

    /// Level events are sent only while a window exists.
    fn set_level_enabled(&self, enabled: bool) {
        self.level_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Half-duplex flag for the speech output (Phase 5): true while the assistant talks.
    fn set_speaking(&self, speaking: bool) {
        if let Some(engine) = lock(&self.engine).as_ref() {
            engine.send(Command::Speaking(speaking));
        }
    }

    /// The consumer finished with the last utterance (the state goes back to IDLE).
    fn finish_processing(&self) {
        let sent = lock(&self.engine)
            .as_ref()
            .is_some_and(|engine| !engine.is_finished() && engine.send(Command::FinishProcessing));
        if !sent {
            // No capture is running (a push-to-talk phrase closed it): the state is reset here.
            self.shared
                .emit(Event::VoiceStateHint(VoiceHint::State("IDLE".into())));
        }
    }

    /// Push-to-talk key state. Blocking when it has to open the microphone: call from a worker thread.
    fn ptt(&self, pressed: bool) {
        let mut slot = lock(&self.engine);
        if slot.as_ref().is_some_and(EngineHandle::is_finished) {
            // The previous push-to-talk engine is done; release the microphone.
            drop(slot.take());
        }
        if let Some(engine) = slot.as_ref() {
            engine.send(Command::Ptt(pressed));
            return;
        }
        if !pressed {
            return;
        }
        let config = lock(&self.config).clone();
        match EngineHandle::spawn(
            EngineKind::PushToTalk,
            &config,
            Arc::clone(&self.level_enabled),
            self.sink(),
        ) {
            Ok(engine) => {
                engine.send(Command::Ptt(true));
                *slot = Some(engine);
            }
            Err(error) => {
                tracing::error!(%error, "push-to-talk could not open the microphone");
                self.shared
                    .emit(Event::VoiceStateHint(VoiceHint::Mic(MicStatus {
                        active: false,
                        device: config.input_device.unwrap_or_default().into(),
                        peak: None,
                        no_signal: false,
                        error: Some(error.to_string().into()),
                    })));
            }
        }
    }

    /// Brings the engine in line with `config`. Blocking. Returns false if a start failed.
    fn apply(&self, config: &VoiceConfig) -> bool {
        let mut slot = lock(&self.engine);
        if slot.as_ref().is_some_and(EngineHandle::is_finished) {
            drop(slot.take());
        }
        if !config.enabled {
            if slot
                .as_ref()
                .is_some_and(|engine| engine.kind() == EngineKind::WakeWord)
            {
                // Dropping stops the thread: microphone closed, model freed.
                drop(slot.take());
            }
            return true;
        }
        if let Some(engine) = slot.as_ref() {
            if engine.kind() == EngineKind::WakeWord
                && engine.requested_device() == config.input_device.as_deref()
            {
                engine.send(Command::Reconfigure {
                    sensitivity: config.sensitivity,
                    wake_names: config.wake_names.clone(),
                });
                return true;
            }
        }
        drop(slot.take());
        match EngineHandle::spawn(
            EngineKind::WakeWord,
            config,
            Arc::clone(&self.level_enabled),
            self.sink(),
        ) {
            Ok(engine) => {
                *slot = Some(engine);
                true
            }
            Err(error) => {
                tracing::error!(%error, "voice listening could not start");
                self.shared
                    .emit(Event::VoiceStateHint(VoiceHint::Mic(MicStatus {
                        active: false,
                        device: config.input_device.clone().unwrap_or_default().into(),
                        peak: None,
                        no_signal: false,
                        error: Some(error.to_string().into()),
                    })));
                false
            }
        }
    }

    fn shutdown_engine(&self) {
        drop(lock(&self.engine).take());
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown_engine();
    }
}

/// Owns the voice front: starts and stops the capture thread as the settings change.
///
/// With `voice.enabled` the microphone and the Vosk model are open; without it nothing is open and
/// push-to-talk opens the microphone only for the duration of one phrase.
pub struct VoiceService {
    inner: Arc<Inner>,
}

impl VoiceService {
    pub fn new(bus: EventBus, config: VoiceConfig) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner::new(bus, config)),
        })
    }

    pub fn snapshot(&self) -> VoiceSnapshot {
        self.inner.snapshot()
    }

    pub fn config(&self) -> VoiceConfig {
        self.inner.config()
    }

    /// Applies new settings (asynchronously, on the service task).
    pub fn set_config(&self, config: VoiceConfig) {
        self.inner.set_config(config);
    }

    /// Level events are sent only while a window exists.
    pub fn set_level_enabled(&self, enabled: bool) {
        self.inner.set_level_enabled(enabled);
    }

    /// Half-duplex flag for the speech output (Phase 5): true while the assistant talks.
    pub fn set_speaking(&self, speaking: bool) {
        self.inner.set_speaking(speaking);
    }

    /// The consumer finished with the last utterance (the state goes back to IDLE).
    pub fn finish_processing(&self) {
        self.inner.finish_processing();
    }

    /// Push-to-talk key state. Blocking when it has to open the microphone: call from a worker thread.
    pub fn ptt(&self, pressed: bool) {
        self.inner.ptt(pressed);
    }

    /// Shows a state the voice front does not produce itself (the router reports `ERROR` this way).
    pub fn show_state(&self, state: &str) {
        self.inner
            .shared
            .emit(Event::VoiceStateHint(VoiceHint::State(state.into())));
    }
}

#[async_trait]
impl Service for VoiceService {
    fn name(&self) -> &'static str {
        SERVICE_NAME
    }

    async fn start(&self) -> CoreResult<()> {
        loop {
            let config = self.inner.config();
            // Loading the model and opening the device block: keep that off the async workers.
            let inner = Arc::clone(&self.inner);
            let wanted = config.clone();
            let ok = tokio::task::spawn_blocking(move || inner.apply(&wanted))
                .await
                .map_err(CoreError::join)?;
            if !config.enabled {
                self.inner.changed.notified().await;
            } else if ok {
                // A capture that dies later (the device was unplugged, a buffer overrun) is brought back:
                // `apply` drops the finished engine and opens the device again.
                loop {
                    tokio::select! {
                        () = self.inner.changed.notified() => break,
                        () = tokio::time::sleep(RETRY_AFTER) => {}
                    }
                    if lock(&self.inner.engine)
                        .as_ref()
                        .is_some_and(EngineHandle::is_finished)
                    {
                        tracing::warn!("the voice capture ended; opening the microphone again");
                        break;
                    }
                }
            } else {
                tokio::select! {
                    () = self.inner.changed.notified() => {}
                    () = tokio::time::sleep(RETRY_AFTER) => {}
                }
            }
        }
    }

    async fn stop(&self) -> CoreResult<()> {
        let engine = lock(&self.inner.engine).take();
        tokio::task::spawn_blocking(move || drop(engine))
            .await
            .map_err(CoreError::join)
    }

    fn critical(&self) -> bool {
        false
    }
}
