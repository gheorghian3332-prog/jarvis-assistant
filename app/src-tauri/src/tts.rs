//! Glue between the speech output (`jarvis-tts`) and the shell: settings, engines, the state and
//! level events, and the command payloads.

use crate::{error::CommandError, settings, state::AppState};
use jarvis_core::{Database, Event, EventBus, VoiceHint};
use jarvis_tts::{
    kokoro, voices, ElevenLabs, ElevenLabsConfig, EngineKind, Engines, Inventory, KokoroEngine,
    PiperConfig, PiperEngine, SecretStore, SpeechObserver, TtsConfig, TtsError,
};
use jarvis_voice::{router::REPO_ROOT_ENV, VoiceService};
use serde::Serialize;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Manager};

/// Longest text `tts_speak` accepts.
pub const MAX_SPEAK_CHARS: usize = 2_000;

/// Reports speech to the rest of the app: the half-duplex flag and state go to the voice front, the
/// level envelope goes to the window.
pub struct ShellObserver {
    bus: EventBus,
    voice: Arc<VoiceService>,
}

impl ShellObserver {
    pub fn new(bus: EventBus, voice: Arc<VoiceService>) -> Self {
        Self { bus, voice }
    }
}

impl SpeechObserver for ShellObserver {
    fn speaking(&self, active: bool) {
        // The half-duplex flag: the wake word is not analyzed while Jarvis talks.
        self.voice.set_speaking(active);
        if active {
            self.voice.show_state("SPEAKING");
        } else if self.voice.snapshot().state == "SPEAKING" {
            // Only if nothing newer happened: a push-to-talk press during the echo tail has
            // already moved the voice front to LISTENING.
            self.voice.show_state("IDLE");
        }
    }

    fn level(&self, rms: f32) {
        // Nobody may be subscribed; that is fine.
        let _ = self
            .bus
            .publish(Event::VoiceStateHint(VoiceHint::Level(rms)));
    }
}

/// Reads the `tts.*` settings; missing or invalid values fall back to the defaults.
pub async fn load_config(db: &Database) -> TtsConfig {
    let mut config = TtsConfig::default();
    let read = |key: &'static str| {
        let db = db.clone();
        async move {
            let value = db.settings_get(key.to_owned()).await.ok().flatten()?;
            settings::validate(key, &value).ok().map(|()| value)
        }
    };
    if let Some(kind) = read(settings::TTS_ENGINE_RU)
        .await
        .as_ref()
        .and_then(Value::as_str)
        .and_then(EngineKind::parse)
    {
        config.engine_ru = kind;
    }
    if let Some(Value::String(voice)) = read(settings::TTS_VOICE_RU).await {
        config.voice_ru = voice;
    }
    if let Some(kind) = read(settings::TTS_ENGINE_EN)
        .await
        .as_ref()
        .and_then(Value::as_str)
        .and_then(EngineKind::parse)
    {
        config.engine_en = kind;
    }
    if let Some(Value::String(voice)) = read(settings::TTS_VOICE_EN).await {
        config.voice_en = voice;
    }
    if let Some(speed) = read(settings::TTS_SPEED).await.and_then(|v| v.as_f64()) {
        config.speed = speed as f32;
    }
    if let Some(volume) = read(settings::TTS_VOLUME).await.and_then(|v| v.as_f64()) {
        config.volume = volume as f32;
    }
    if let Some(Value::String(device)) = read(settings::TTS_OUTPUT_DEVICE).await {
        config.output_device = Some(device);
    }
    if let Some(enabled) = read(settings::TTS_CLOUD_ENABLED)
        .await
        .and_then(|v| v.as_bool())
    {
        config.cloud_enabled = enabled;
    }
    if let Some(Value::String(id)) = read(settings::TTS_ELEVEN_VOICE_ID).await {
        config.eleven_voice_id = id;
    }
    config
}

/// The repository folder that holds `workers/tts_kokoro`: `JARVIS_REPO_ROOT`, or the first parent
/// of the running executable that has it.
pub fn find_kokoro_root() -> Option<PathBuf> {
    let has_worker = |dir: &Path| dir.join("workers/tts_kokoro/pyproject.toml").is_file();
    if let Some(root) = std::env::var_os(REPO_ROOT_ENV).map(PathBuf::from) {
        return has_worker(&root).then_some(root);
    }
    if jarvis_core::layout::resources_dir().is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .find(|dir| has_worker(dir))
        .map(Path::to_path_buf)
}

/// Piper, Kokoro and (when it can be built) ElevenLabs. Nothing starts until the first phrase.
pub fn build_engines(secrets: Arc<dyn SecretStore>) -> (Engines, PiperEngine) {
    let piper = PiperEngine::new(PiperConfig::default_paths());
    let kokoro = match find_kokoro_root() {
        Some(root) => {
            tracing::info!(root = %root.display(), "Kokoro TTS worker found");
            KokoroEngine::new(kokoro::worker_command(&root))
        }
        None => match jarvis_core::layout::installed_worker("tts_kokoro", "jarvis_tts_kokoro") {
            Some(command) => {
                tracing::info!("installed Kokoro TTS worker");
                KokoroEngine::new(command)
            }
            None => {
                tracing::warn!(
                    "workers/tts_kokoro not found; set {REPO_ROOT_ENV} to the repository folder"
                );
                KokoroEngine::unavailable(format!(
                    "workers/tts_kokoro not found; set {REPO_ROOT_ENV} to the repository folder"
                ))
            }
        },
    };
    let cloud = match ElevenLabs::new(ElevenLabsConfig::default(), secrets) {
        Ok(engine) => Some(Arc::new(engine) as Arc<dyn jarvis_tts::TtsEngine>),
        Err(error) => {
            tracing::warn!(%error, "ElevenLabs is unavailable");
            None
        }
    };
    let engines = Engines {
        piper: Arc::new(piper.clone()),
        kokoro: Arc::new(kokoro),
        cloud,
    };
    (engines, piper)
}

/// Applies a stored `tts.*` setting that was just changed.
pub async fn apply_setting(app: &AppHandle, key: &str) {
    if !key.starts_with("tts.") {
        return;
    }
    let state = app.state::<AppState>();
    let config = load_config(&state.db).await;
    state.tts.set_config(config);
}

impl From<TtsError> for CommandError {
    fn from(error: TtsError) -> Self {
        match error {
            TtsError::Invalid(message) => Self::Validation(message),
            TtsError::Unavailable { .. } => Self::Unavailable(error.to_string()),
            TtsError::Timeout { .. } => Self::Timeout(error.to_string()),
            TtsError::Synthesis { .. } | TtsError::Playback(_) | TtsError::Secret(_) => {
                Self::System(error.to_string())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutputDeviceDto {
    pub name: String,
    pub is_default: bool,
}

/// What is installed, for `tts_voices`. Blocking (file system and the credential store).
pub fn inventory(state: &AppState) -> Inventory {
    let piper = &state.piper;
    Inventory {
        piper_exe: piper.exe_present(),
        piper_voices: piper.installed_voices(),
        kokoro_model: kokoro::model_installed(),
        kokoro_worker: find_kokoro_root().is_some()
            || jarvis_core::layout::installed_worker("tts_kokoro", "jarvis_tts_kokoro").is_some(),
        cloud_key: state
            .secrets
            .exists(jarvis_tts::ELEVEN_KEY)
            .unwrap_or(false),
    }
}

pub fn voices_report(state: &AppState) -> voices::VoicesReport {
    let config = state.tts.config();
    voices::report(&config, &inventory(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::Database;
    use jarvis_voice::VoiceConfig;
    use serde_json::json;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path().join("jarvis.db")).expect("open");
        (dir, db)
    }

    #[test]
    fn defaults_when_nothing_is_stored() {
        let (_dir, db) = temp_db();
        let config = tauri::async_runtime::block_on(load_config(&db));
        assert_eq!(config, TtsConfig::default());
        assert!(!config.cloud_enabled, "the cloud is never on by default");
    }

    #[test]
    fn reads_stored_values_and_ignores_invalid_ones() {
        let (_dir, db) = temp_db();
        tauri::async_runtime::block_on(async {
            for (key, value) in [
                (settings::TTS_ENGINE_EN, json!("piper")),
                (settings::TTS_VOICE_EN, json!("bm_lewis")),
                (settings::TTS_SPEED, json!(1.25)),
                (settings::TTS_VOLUME, json!(3.0)),
                (settings::TTS_OUTPUT_DEVICE, json!("Speakers")),
                (settings::TTS_CLOUD_ENABLED, json!(true)),
                (settings::TTS_ELEVEN_VOICE_ID, json!("../x")),
                (settings::TTS_VOICE_RU, json!("ru_RU-denis-medium")),
            ] {
                db.settings_set(key.into(), value).await.expect("set");
            }
            let config = load_config(&db).await;
            assert_eq!(config.engine_en, EngineKind::Piper);
            assert_eq!(config.voice_en, "bm_lewis");
            assert_eq!(config.speed, 1.25);
            assert_eq!(config.volume, 0.9, "out of range falls back");
            assert_eq!(config.output_device.as_deref(), Some("Speakers"));
            assert!(config.cloud_enabled);
            assert!(config.eleven_voice_id.is_empty(), "unsafe id falls back");
            assert_eq!(config.voice_ru, "ru_RU-denis-medium");
        });
    }

    #[test]
    fn tts_errors_map_to_command_codes() {
        let cases = [
            (TtsError::Invalid("x".into()), "VALIDATION"),
            (TtsError::unavailable("piper", "x"), "UNAVAILABLE"),
            (TtsError::synthesis("piper", "x"), "SYSTEM"),
            (TtsError::Playback("x".into()), "SYSTEM"),
            (TtsError::Secret("x".into()), "SYSTEM"),
            (
                TtsError::Timeout {
                    engine: "elevenlabs",
                    limit: std::time::Duration::from_secs(2),
                },
                "TIMEOUT",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(CommandError::from(error).code(), code);
        }
    }

    #[test]
    fn the_observer_drives_the_half_duplex_state_and_the_level_events() {
        let bus = EventBus::new();
        let mut receiver = bus.subscribe();
        let voice = VoiceService::new(bus.clone(), VoiceConfig::default());
        let observer = ShellObserver::new(bus, Arc::clone(&voice));

        observer.speaking(true);
        assert_eq!(voice.snapshot().state, "SPEAKING");
        observer.level(0.42);
        observer.speaking(false);
        assert_eq!(voice.snapshot().state, "IDLE");

        let mut seen = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            seen.push((*event).clone());
        }
        assert_eq!(
            seen,
            vec![
                Event::VoiceStateHint(VoiceHint::State("SPEAKING".into())),
                Event::VoiceStateHint(VoiceHint::Level(0.42)),
                Event::VoiceStateHint(VoiceHint::State("IDLE".into())),
            ]
        );
    }

    #[test]
    fn the_end_of_speech_does_not_override_a_newer_state() {
        let bus = EventBus::new();
        let voice = VoiceService::new(bus.clone(), VoiceConfig::default());
        let observer = ShellObserver::new(bus, Arc::clone(&voice));
        observer.speaking(true);
        // The user pressed push-to-talk during the echo tail.
        voice.show_state("LISTENING");
        observer.speaking(false);
        assert_eq!(voice.snapshot().state, "LISTENING");
    }

    #[test]
    fn engines_build_without_starting_anything() {
        let secrets: Arc<dyn SecretStore> = Arc::new(jarvis_tts::MemoryStore::default());
        let (engines, piper) = build_engines(secrets);
        assert_eq!(engines.piper.name(), "piper");
        assert_eq!(engines.kokoro.name(), "kokoro");
        assert!(engines.cloud.is_some());
        assert!(tauri::async_runtime::block_on(async {
            !piper.is_running().await
        }));
    }
}
