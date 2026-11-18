use crate::{
    ai::AiParts, control::ControlHub, dispatch::CommandCore, insight::SharedVisionSettings,
    voice::PttWorker, window::WindowMode,
};
use jarvis_core::{Database, EventBus, Supervisor};
use jarvis_sysintel::SysIntel;
use jarvis_tts::{PiperEngine, SecretStore, TtsService};
use jarvis_voice::VoiceService;
use std::{
    sync::{atomic::AtomicBool, Arc, Mutex, MutexGuard},
    time::Instant,
};
use tauri::async_runtime::JoinHandle;
use tauri_plugin_global_shortcut::Shortcut;

/// Locks a mutex, recovering the data if another thread panicked while holding it.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The speech output and what its commands need.
pub struct SpeechParts {
    pub tts: TtsService,
    /// Piper (kept to list its voices).
    pub piper: PiperEngine,
    /// The Windows Credential Manager (ElevenLabs key).
    pub secrets: Arc<dyn SecretStore>,
}

/// The command side: the executor holder and the phrase router.
pub struct ControlParts {
    pub hub: Arc<ControlHub>,
    pub commands: Arc<CommandCore>,
    pub ai: Arc<AiParts>,
    /// The system intelligence (also registered in the supervisor).
    pub sys: Arc<SysIntel>,
    /// `vision.*` settings, shared with the vision actions.
    pub vision: SharedVisionSettings,
}

/// Everything the shell shares between commands, tray handlers and the window manager.
pub struct AppState {
    pub db: Database,
    pub bus: EventBus,
    pub started: Instant,
    pub supervisor: Mutex<Option<Supervisor>>,
    /// Set by the tray quit item; only then may the process exit.
    pub quitting: AtomicBool,
    /// Last visible mode (`full` or `compact`), mirrored in `ui.last_mode`.
    pub last_mode: Mutex<WindowMode>,
    /// Task forwarding bus events to the UI; exists only while the window exists.
    pub forwarder: Mutex<Option<JoinHandle<()>>>,
    /// Serializes window mode changes (create / reconfigure / destroy).
    pub mode_lock: Mutex<()>,
    /// The voice front (also registered in the supervisor).
    pub voice: Arc<VoiceService>,
    /// Push-to-talk presses and releases, in order.
    pub ptt: PttWorker,
    /// The registered push-to-talk key.
    pub ptt_shortcut: Mutex<Option<Shortcut>>,
    /// The speech output (also registered in the supervisor).
    pub tts: TtsService,
    /// Piper (kept to list its voices).
    pub piper: PiperEngine,
    /// The executor holder (settings, action list).
    pub control: Arc<ControlHub>,
    /// Phrases to executor, confirmations, emergency stop.
    pub commands: Arc<CommandCore>,
    /// The local AI brain, its configuration and memory.
    pub ai: Arc<AiParts>,
    /// The system intelligence: metrics, alerts, processes.
    pub sys: Arc<SysIntel>,
    /// `vision.*` settings, shared with the vision actions.
    pub vision: SharedVisionSettings,
    /// The registered emergency-stop key.
    pub emergency_shortcut: Mutex<Option<Shortcut>>,
    /// The Windows Credential Manager (ElevenLabs key).
    pub secrets: Arc<dyn SecretStore>,
}

impl AppState {
    pub fn new(
        db: Database,
        bus: EventBus,
        supervisor: Supervisor,
        last_mode: WindowMode,
        voice: Arc<VoiceService>,
        speech: SpeechParts,
        control: ControlParts,
    ) -> Self {
        let on_press = {
            let tts = speech.tts.clone();
            move || tts.stop()
        };
        Self {
            db,
            bus,
            started: Instant::now(),
            supervisor: Mutex::new(Some(supervisor)),
            quitting: AtomicBool::new(false),
            last_mode: Mutex::new(last_mode),
            forwarder: Mutex::new(None),
            mode_lock: Mutex::new(()),
            ptt: PttWorker::start(Arc::clone(&voice), on_press),
            voice,
            ptt_shortcut: Mutex::new(None),
            tts: speech.tts,
            piper: speech.piper,
            secrets: speech.secrets,
            control: control.hub,
            commands: control.commands,
            ai: control.ai,
            sys: control.sys,
            vision: control.vision,
            emergency_shortcut: Mutex::new(None),
        }
    }
}
