//! Glue between the voice front (`jarvis-voice`) and the shell: settings, the push-to-talk hotkey and
//! the payloads sent to the window.

use crate::{
    error::CommandError,
    settings,
    state::{lock, AppState},
};
use jarvis_core::{Database, MicStatus};
use jarvis_voice::{VoiceConfig, VoiceService};
use serde::Serialize;
use serde_json::Value;
use std::{
    sync::{
        mpsc::{self, Sender},
        Arc,
    },
    thread,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LevelPayload {
    pub rms: f32,
}

/// `voice://status`: microphone health. `no_signal` drives the "microphone gives no signal" warning.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MicPayload {
    pub active: bool,
    pub device: String,
    pub peak: Option<f32>,
    pub no_signal: bool,
    pub error: Option<String>,
}

impl From<&MicStatus> for MicPayload {
    fn from(status: &MicStatus) -> Self {
        Self {
            active: status.active,
            device: status.device.to_string(),
            peak: status.peak,
            no_signal: status.no_signal,
            error: status.error.as_ref().map(ToString::to_string),
        }
    }
}

/// `voice://transcript`: what the last utterance said.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TranscriptPayload {
    pub text: String,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub lang: String,
}

/// `voice://wake`: the wake word was heard (no audio, no text).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WakePayload {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InputDeviceDto {
    pub name: String,
    pub is_default: bool,
}

/// Reads the voice settings from the database; missing or invalid values fall back to the defaults.
pub async fn load_config(db: &Database) -> VoiceConfig {
    let mut config = VoiceConfig::default();
    let read = |key: &'static str| {
        let db = db.clone();
        async move { db.settings_get(key.to_owned()).await.ok().flatten() }
    };
    if let Some(enabled) = read(settings::VOICE_ENABLED)
        .await
        .and_then(|v| v.as_bool())
    {
        config.enabled = enabled;
    }
    if let Some(Value::String(name)) = read(settings::VOICE_INPUT_DEVICE).await {
        if settings::validate(settings::VOICE_INPUT_DEVICE, &Value::String(name.clone())).is_ok() {
            config.input_device = Some(name);
        }
    }
    if let Some(sensitivity) = read(settings::VOICE_SENSITIVITY)
        .await
        .and_then(|v| v.as_f64())
        .filter(|v| (0.0..=1.0).contains(v))
    {
        config.sensitivity = sensitivity as f32;
    }
    if let Some(names) = read(settings::VOICE_WAKE_NAMES)
        .await
        .and_then(|v| settings::names_from(&v))
        .filter(|names| jarvis_voice::validate_wake_names(names).is_ok())
    {
        config.wake_names = names;
    }
    config
}

/// The stored push-to-talk key (default `F8`).
pub async fn load_ptt_key(db: &Database) -> Shortcut {
    let stored = db
        .settings_get(settings::VOICE_PTT_KEY.to_owned())
        .await
        .ok()
        .flatten();
    stored
        .and_then(|value| settings::parse_ptt_key(settings::VOICE_PTT_KEY, &value).ok())
        .or_else(|| settings::DEFAULT_PTT_KEY.parse().ok())
        .unwrap_or_else(|| Shortcut::new(None, tauri_plugin_global_shortcut::Code::F8))
}

/// Runs push-to-talk presses and releases one after another, off the hotkey thread
/// (opening the microphone can take a moment, and a release must never overtake its press).
pub struct PttWorker {
    sender: Sender<bool>,
}

impl PttWorker {
    /// `on_press` runs first on every press edge (speech output stops when the user starts talking).
    pub fn start(service: Arc<VoiceService>, on_press: impl Fn() + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel::<bool>();
        let spawned = thread::Builder::new()
            .name("jarvis-ptt".into())
            .spawn(move || {
                let mut held = false;
                while let Ok(pressed) = receiver.recv() {
                    // Key auto-repeat sends many presses; only the edges matter.
                    if pressed == held {
                        continue;
                    }
                    held = pressed;
                    if pressed {
                        on_press();
                    }
                    service.ptt(pressed);
                }
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "push-to-talk worker did not start");
        }
        Self { sender }
    }

    pub fn send(&self, pressed: bool) {
        // The worker only stops with the process.
        let _ = self.sender.send(pressed);
    }
}

/// Registers `shortcut` as the push-to-talk key, replacing the previous one.
pub fn register_ptt(app: &AppHandle, shortcut: Shortcut) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let mut current = lock(&state.ptt_shortcut);
    if *current == Some(shortcut) {
        return Ok(());
    }
    let handle = app.clone();
    let shortcuts = app.global_shortcut();
    shortcuts
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            handle
                .state::<AppState>()
                .ptt
                .send(event.state() == ShortcutState::Pressed);
        })
        .map_err(|error| CommandError::System(format!("push-to-talk key: {error}")))?;
    if let Some(previous) = current.replace(shortcut) {
        if let Err(error) = shortcuts.unregister(previous) {
            tracing::warn!(%error, "could not release the previous push-to-talk key");
        }
    }
    Ok(())
}

/// Applies a stored voice setting that was just changed.
pub async fn apply_setting(app: &AppHandle, key: &str) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    if key == settings::VOICE_PTT_KEY {
        let shortcut = load_ptt_key(&state.db).await;
        return register_ptt(app, shortcut);
    }
    if key.starts_with("voice.") {
        let config = load_config(&state.db).await;
        state.voice.set_config(config);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::Database;
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
        assert_eq!(config, VoiceConfig::default());
        assert!(
            !config.enabled,
            "listening is off until the user turns it on"
        );
    }

    #[test]
    fn reads_stored_values_and_ignores_invalid_ones() {
        let (_dir, db) = temp_db();
        tauri::async_runtime::block_on(async {
            db.settings_set(settings::VOICE_ENABLED.into(), json!(true))
                .await
                .expect("set");
            db.settings_set(settings::VOICE_SENSITIVITY.into(), json!(7.0))
                .await
                .expect("set");
            db.settings_set(settings::VOICE_WAKE_NAMES.into(), json!(["Jarvis"]))
                .await
                .expect("set");
            db.settings_set(settings::VOICE_INPUT_DEVICE.into(), json!("Mic"))
                .await
                .expect("set");
            let config = load_config(&db).await;
            assert!(config.enabled);
            assert_eq!(config.sensitivity, 0.5, "out of range falls back");
            assert_eq!(config.wake_names, vec!["Jarvis".to_owned()]);
            assert_eq!(config.input_device.as_deref(), Some("Mic"));
        });
    }

    #[test]
    fn push_to_talk_key_defaults_to_f8() {
        let (_dir, db) = temp_db();
        let key = tauri::async_runtime::block_on(load_ptt_key(&db));
        assert_eq!(key, "F8".parse::<Shortcut>().expect("F8"));
        tauri::async_runtime::block_on(async {
            db.settings_set(settings::VOICE_PTT_KEY.into(), json!("F9"))
                .await
                .expect("set");
            assert_eq!(
                load_ptt_key(&db).await,
                "F9".parse::<Shortcut>().expect("F9")
            );
            db.settings_set(settings::VOICE_PTT_KEY.into(), json!("garbage"))
                .await
                .expect("set");
            assert_eq!(
                load_ptt_key(&db).await,
                "F8".parse::<Shortcut>().expect("F8")
            );
        });
    }

    #[test]
    fn transcript_payload_uses_the_contract_names() {
        let payload = TranscriptPayload {
            text: "открой блокнот".into(),
            is_final: true,
            lang: "ru".into(),
        };
        assert_eq!(
            serde_json::to_value(payload).expect("serialize"),
            json!({"text": "открой блокнот", "final": true, "lang": "ru"})
        );
    }

    #[test]
    fn mic_payload_keeps_the_warning_fields() {
        let status = MicStatus {
            active: true,
            device: "Test Microphone".into(),
            peak: Some(0.0),
            no_signal: true,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(MicPayload::from(&status)).expect("serialize"),
            json!({"active": true, "device": "Test Microphone", "peak": 0.0, "no_signal": true, "error": null})
        );
    }
}
