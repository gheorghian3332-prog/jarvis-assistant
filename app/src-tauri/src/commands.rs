use crate::{
    ai::{self, AiStatus, FactDto, HistoryDto},
    alerts::AlertPayload,
    control::{self, ControlInfo, MAX_CHAT_CHARS},
    dispatch::Source,
    error::CommandError,
    settings,
    state::{lock, AppState},
    tts::{self, OutputDeviceDto},
    voice::{self, InputDeviceDto, MicPayload},
    window::{self, WindowMode},
};
use jarvis_core::{AuditEntry, Database};
use jarvis_tts::{secret, SecretString};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{async_runtime, AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Serialize)]
pub struct CoreInfo {
    pub version: &'static str,
    pub uptime_s: u64,
    pub features: Vec<&'static str>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AuditEntryDto {
    pub id: i64,
    pub ts: String,
    pub action: String,
    pub args: Value,
    pub risk: String,
    pub ok: bool,
    pub detail: String,
}

impl From<AuditEntry> for AuditEntryDto {
    fn from(entry: AuditEntry) -> Self {
        Self {
            id: entry.id,
            ts: entry.ts,
            action: entry.action,
            args: entry.args,
            risk: entry.risk,
            ok: entry.ok,
            detail: entry.detail,
        }
    }
}

pub const MAX_AUDIT_LIMIT: u32 = 200;

#[tauri::command]
pub async fn core_info(state: State<'_, AppState>) -> Result<CoreInfo, CommandError> {
    Ok(CoreInfo {
        version: env!("CARGO_PKG_VERSION"),
        uptime_s: state.started.elapsed().as_secs(),
        features: vec![
            "settings", "audit", "metrics", "voice", "tts", "control", "ai", "memory",
        ],
    })
}

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>, key: String) -> Result<Value, CommandError> {
    read_setting(&state.db, key).await
}

#[tauri::command]
pub async fn settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    value: Value,
) -> Result<(), CommandError> {
    settings::validate(&key, &value)?;
    if key == settings::AUTOSTART {
        let enabled = value.as_bool().unwrap_or(false);
        let handle = app.clone();
        // The plugin touches the registry; keep it off the async workers.
        async_runtime::spawn_blocking(move || set_autostart(&handle, enabled))
            .await
            .map_err(|error| CommandError::System(error.to_string()))??;
    }
    store_setting(&state.db, &key, &value).await?;
    voice::apply_setting(&app, &key).await?;
    tts::apply_setting(&app, &key).await;
    control::apply_setting(&app, &key).await?;
    ai::apply_setting(&app, &key).await;
    apply_system_setting(&state, &key).await;
    if key == settings::LAST_MODE {
        if let Some(mode) = value.as_str().and_then(WindowMode::parse) {
            *lock(&state.last_mode) = mode;
        }
    }
    Ok(())
}

/// Applies a changed `sysintel.*` / `vision.*` setting to the running services.
async fn apply_system_setting(state: &AppState, key: &str) {
    if key.starts_with("sysintel.") {
        state
            .sys
            .set_config(crate::sysconf::load_config(&state.db).await);
    } else if key.starts_with("vision.") {
        let fresh = crate::insight::load_vision_settings(&state.db).await;
        *state
            .vision
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = fresh;
    }
}

/// What the SYSTEM tab shows: the latest metrics and where the GPU numbers come from.
#[derive(Debug, Serialize)]
pub struct SystemSnapshotDto {
    pub metrics: jarvis_sysintel::Metrics,
    /// `window` while the window polls at 1 Hz, `tray` otherwise.
    pub mode: &'static str,
    /// `nvml` (the helper process), `d3dkmt` (kernel statistics) or `null` (no GPU reading).
    pub gpu_source: Option<&'static str>,
}

pub fn gpu_source_of(
    metrics: &jarvis_sysintel::Metrics,
    helper_running: bool,
) -> Option<&'static str> {
    let gpu = metrics.gpu.as_ref()?;
    // Only NVML reports watts.
    Some(if helper_running && gpu.power_watts.is_some() {
        "nvml"
    } else {
        "d3dkmt"
    })
}

#[tauri::command]
pub async fn system_snapshot(
    state: State<'_, AppState>,
) -> Result<SystemSnapshotDto, CommandError> {
    let metrics = state.sys.snapshot();
    let gpu_source = gpu_source_of(&metrics, state.sys.gpu_worker_pid().is_some());
    Ok(SystemSnapshotDto {
        mode: match state.sys.mode() {
            jarvis_sysintel::Mode::Window => "window",
            jarvis_sysintel::Mode::Tray => "tray",
        },
        metrics,
        gpu_source,
    })
}

/// The "top processes" button: sampled now, never in the background.
#[tauri::command]
pub async fn system_top_processes(
    state: State<'_, AppState>,
    by: String,
) -> Result<Vec<jarvis_sysintel::ProcessUsage>, CommandError> {
    let rank = match by.as_str() {
        "cpu" => jarvis_sysintel::Rank::Cpu,
        "ram" => jarvis_sysintel::Rank::Ram,
        _ => return Err(CommandError::Validation("by: expected cpu or ram".into())),
    };
    state
        .sys
        .top_processes(rank, 10)
        .await
        .map_err(|error| CommandError::System(error.to_string()))
}

/// The alert feed of the SYSTEM tab (newest first).
#[tauri::command]
pub async fn system_alerts(state: State<'_, AppState>) -> Result<Vec<AlertPayload>, CommandError> {
    Ok(state
        .sys
        .recent_alerts_timed(50)
        .into_iter()
        .map(|(at_ms, alert)| {
            AlertPayload::from_notice(&jarvis_sysintel::bridge::notice_from(&alert), at_ms)
        })
        .collect())
}

/// A synthetic alert for checking the pipeline. Debug builds only; a release build refuses.
#[tauri::command]
pub async fn debug_raise_alert(
    state: State<'_, AppState>,
    severity: String,
    speak: bool,
) -> Result<(), CommandError> {
    #[cfg(debug_assertions)]
    {
        use jarvis_sysintel::alerts::{Alert, AlertKind, Severity};
        let severity = match severity.as_str() {
            "info" => Severity::Info,
            "warning" => Severity::Warning,
            "critical" => Severity::Critical,
            _ => {
                return Err(CommandError::Validation(
                    "severity: expected info, warning or critical".into(),
                ))
            }
        };
        state
            .sys
            .inject_event(jarvis_sysintel::SysEvent::Alert(Alert {
                id: "debug-1".into(),
                kind: AlertKind::CpuLoad,
                severity,
                message_ru: "Тестовое предупреждение системы".into(),
                message_en: "Test system alert".into(),
                speak,
            }));
        Ok(())
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (&state, &severity, speak);
        Err(CommandError::Forbidden(
            "synthetic alerts exist only in debug builds".into(),
        ))
    }
}

#[tauri::command]
pub async fn audit_list(
    state: State<'_, AppState>,
    limit: u32,
) -> Result<Vec<AuditEntryDto>, CommandError> {
    let entries = state.db.audit_list(limit.clamp(1, MAX_AUDIT_LIMIT)).await?;
    Ok(entries.into_iter().map(AuditEntryDto::from).collect())
}

#[derive(Debug, Serialize, PartialEq)]
pub struct VoiceStatusDto {
    pub state: String,
    pub mic: Option<MicPayload>,
}

/// Push-to-talk from the UI button: `true` while it is held.
#[tauri::command]
pub async fn ptt(state: State<'_, AppState>, pressed: bool) -> Result<(), CommandError> {
    state.ptt.send(pressed);
    Ok(())
}

/// Input devices of the default audio host.
#[tauri::command]
pub async fn voice_devices() -> Result<Vec<InputDeviceDto>, CommandError> {
    let devices = async_runtime::spawn_blocking(jarvis_voice::list_input_devices)
        .await
        .map_err(|error| CommandError::System(error.to_string()))?
        .map_err(|error| CommandError::System(error.to_string()))?;
    Ok(devices
        .into_iter()
        .map(|device| InputDeviceDto {
            name: device.name,
            is_default: device.is_default,
        })
        .collect())
}

/// Last known voice state and microphone health, for a window that opens later.
#[tauri::command]
pub async fn voice_status(state: State<'_, AppState>) -> Result<VoiceStatusDto, CommandError> {
    let snapshot = state.voice.snapshot();
    Ok(VoiceStatusDto {
        state: snapshot.state,
        mic: snapshot.mic.as_ref().map(MicPayload::from),
    })
}

/// Speaks `text` with the engine for its language (the "check voice" button and, later, the brain).
/// Returns as soon as the speech is queued; a speech in progress is interrupted.
#[tauri::command]
pub async fn tts_speak(state: State<'_, AppState>, text: String) -> Result<(), CommandError> {
    if text.chars().count() > tts::MAX_SPEAK_CHARS {
        return Err(CommandError::Validation(format!(
            "text is longer than {} characters",
            tts::MAX_SPEAK_CHARS
        )));
    }
    state.tts.speak(&text)?;
    Ok(())
}

/// Stops the speech at once (push-to-talk does the same).
#[tauri::command]
pub async fn tts_stop(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.tts.stop();
    Ok(())
}

/// Engines with their status and the voices that can be chosen.
#[tauri::command]
pub async fn tts_voices(app: AppHandle) -> Result<jarvis_tts::VoicesReport, CommandError> {
    async_runtime::spawn_blocking(move || tts::voices_report(&app.state::<AppState>()))
        .await
        .map_err(|error| CommandError::System(error.to_string()))
}

/// Output devices of the default audio host.
#[tauri::command]
pub async fn audio_outputs() -> Result<Vec<OutputDeviceDto>, CommandError> {
    let devices = async_runtime::spawn_blocking(jarvis_tts::list_output_devices)
        .await
        .map_err(|error| CommandError::System(error.to_string()))??;
    Ok(devices
        .into_iter()
        .map(|device| OutputDeviceDto {
            name: device.name,
            is_default: device.is_default,
        })
        .collect())
}

/// What the audit log records about a secret: its name, never its value.
fn secret_audit_args(name: &str) -> Value {
    json!({ "name": name })
}

/// Stores a secret (the ElevenLabs key) in the Windows Credential Manager. The value is never
/// returned, logged or audited.
#[tauri::command]
pub async fn secret_set(
    state: State<'_, AppState>,
    name: String,
    value: String,
) -> Result<(), CommandError> {
    let name = secret::validate_name(&name)?;
    let value = SecretString::new(value);
    let store = Arc::clone(&state.secrets);
    async_runtime::spawn_blocking(move || store.set(name, value))
        .await
        .map_err(|error| CommandError::System(error.to_string()))??;
    state
        .db
        .audit_append(
            "secret.set".into(),
            secret_audit_args(name),
            "medium".into(),
            true,
            format!("{name} stored"),
        )
        .await?;
    Ok(())
}

/// Deletes a secret from the Windows Credential Manager.
#[tauri::command]
pub async fn secret_clear(state: State<'_, AppState>, name: String) -> Result<(), CommandError> {
    let name = secret::validate_name(&name)?;
    let store = Arc::clone(&state.secrets);
    async_runtime::spawn_blocking(move || store.clear(name))
        .await
        .map_err(|error| CommandError::System(error.to_string()))??;
    state
        .db
        .audit_append(
            "secret.clear".into(),
            secret_audit_args(name),
            "medium".into(),
            true,
            format!("{name} deleted"),
        )
        .await?;
    Ok(())
}

/// Whether a secret is stored; the value itself never leaves the credential store.
#[tauri::command]
pub async fn secret_exists(state: State<'_, AppState>, name: String) -> Result<bool, CommandError> {
    let name = secret::validate_name(&name)?;
    let store = Arc::clone(&state.secrets);
    let exists = async_runtime::spawn_blocking(move || store.exists(name))
        .await
        .map_err(|error| CommandError::System(error.to_string()))??;
    Ok(exists)
}

/// A typed command (the same path a spoken phrase takes): FAST PATH, then the executor.
#[tauri::command]
pub async fn chat_send(state: State<'_, AppState>, text: String) -> Result<(), CommandError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CommandError::Validation("text is empty".into()));
    }
    if text.chars().count() > MAX_CHAT_CHARS {
        return Err(CommandError::Validation(format!(
            "text is longer than {MAX_CHAT_CHARS} characters"
        )));
    }
    state.commands.submit(text, None, Source::Text).await;
    Ok(())
}

/// The answer to a `confirm://request` (the dialog buttons).
#[tauri::command]
pub async fn confirm_response(
    state: State<'_, AppState>,
    action_id: String,
    approved: bool,
) -> Result<(), CommandError> {
    if state.commands.confirm_response(&action_id, approved).await {
        Ok(())
    } else {
        Err(CommandError::NotFound(format!(
            "no pending confirmation \"{action_id}\""
        )))
    }
}

/// Stops the speech, cancels confirmations and a scheduled shutdown, ends a recording.
#[tauri::command]
pub async fn emergency_stop(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.commands.emergency_stop().await;
    Ok(())
}

/// The action table of the TOOLS tab and the default file roots.
#[tauri::command]
pub async fn control_actions(state: State<'_, AppState>) -> Result<ControlInfo, CommandError> {
    Ok(control::control_info(&state.control))
}

/// Ollama status for the AI tab (never starts the server).
#[tauri::command]
pub async fn ai_status(state: State<'_, AppState>) -> Result<AiStatus, CommandError> {
    Ok(ai::status(&state.ai).await)
}

/// The "Unload model" button.
#[tauri::command]
pub async fn ai_unload(state: State<'_, AppState>) -> Result<(), CommandError> {
    ai::unload(&state.ai).await?;
    state
        .db
        .audit_append(
            "ai.unload".into(),
            json!({"source": "ui"}),
            "safe".into(),
            true,
            "model unloaded".into(),
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn memory_facts(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<FactDto>, CommandError> {
    ai::facts(&state.db, query, limit).await
}

#[tauri::command]
pub async fn memory_fact_delete(state: State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    if state.db.fact_delete(id).await? {
        state
            .db
            .audit_append(
                "memory.forget".into(),
                json!({"id": id, "source": "ui"}),
                "safe".into(),
                true,
                "fact deleted".into(),
            )
            .await?;
        Ok(())
    } else {
        Err(CommandError::NotFound(format!("no fact {id}")))
    }
}

#[tauri::command]
pub async fn memory_history(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<HistoryDto>, CommandError> {
    ai::history(&state.db, query, limit).await
}

#[tauri::command]
pub async fn memory_history_delete(
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), CommandError> {
    if state.db.history_delete(id).await? {
        Ok(())
    } else {
        Err(CommandError::NotFound(format!("no history row {id}")))
    }
}

/// "Clear history": the commands only, facts stay.
#[tauri::command]
pub async fn memory_clear_history(state: State<'_, AppState>) -> Result<usize, CommandError> {
    let count = state.db.history_clear().await?;
    state
        .db
        .audit_append(
            "memory.clear_history".into(),
            json!({"rows": count}),
            "confirm".into(),
            true,
            "history cleared".into(),
        )
        .await?;
    Ok(count)
}

/// "Delete all memory": facts, history and registered apps. The UI asks first; the command
/// refuses to run without `confirmed`.
#[tauri::command]
pub async fn memory_clear_all(
    state: State<'_, AppState>,
    confirmed: bool,
) -> Result<(), CommandError> {
    if !confirmed {
        return Err(CommandError::Validation(
            "deleting all memory needs a confirmation".into(),
        ));
    }
    state.db.memory_clear_all().await?;
    state
        .db
        .audit_append(
            "memory.clear_all".into(),
            json!({}),
            "confirm".into(),
            true,
            "all memory deleted".into(),
        )
        .await?;
    Ok(())
}

/// What the first-run wizard shows: installed pieces, sizes and where Ollama comes from.
#[tauri::command]
pub async fn setup_status() -> Result<crate::setup::SetupStatus, CommandError> {
    async_runtime::spawn_blocking(crate::setup::status)
        .await
        .map_err(|error| CommandError::System(error.to_string()))
}

/// Starts installing one wizard item (`vosk_model`, `piper`, ...); progress arrives as `setup://progress`.
#[tauri::command]
pub async fn setup_install(app: AppHandle, item: String) -> Result<(), CommandError> {
    crate::setup::start(&app, &item)
}

/// Pulls an Ollama model through the local server (only a valid model name is accepted).
#[tauri::command]
pub async fn setup_pull_model(app: AppHandle, model: String) -> Result<(), CommandError> {
    crate::setup::start_pull(&app, &model)
}

/// Opens the Ollama download page in the default browser (Ollama is never installed by Jarvis).
#[tauri::command]
pub async fn setup_open_ollama_page() -> Result<(), CommandError> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("rundll32.exe")
        .args([
            "url.dll,FileProtocolHandler",
            crate::setup::OLLAMA_DOWNLOAD_URL,
        ])
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|error| CommandError::System(error.to_string()))
}

#[tauri::command]
pub async fn set_window_mode(app: AppHandle, mode: WindowMode) -> Result<(), CommandError> {
    async_runtime::spawn_blocking(move || window::apply_mode(&app, mode))
        .await
        .map_err(|error| CommandError::System(error.to_string()))?
}

async fn read_setting(db: &Database, key: String) -> Result<Value, CommandError> {
    if !settings::WHITELIST.contains(&key.as_str()) {
        return Err(CommandError::Validation(format!(
            "{key}: not a known setting"
        )));
    }
    Ok(db.settings_get(key).await?.unwrap_or(Value::Null))
}

/// Persists an already validated setting and records it in the audit log.
async fn store_setting(db: &Database, key: &str, value: &Value) -> Result<(), CommandError> {
    db.settings_set(key.to_owned(), value.clone()).await?;
    db.audit_append(
        "settings.set".into(),
        json!({"key": key, "value": value}),
        "safe".into(),
        true,
        format!("{key} updated"),
    )
    .await?;
    Ok(())
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), CommandError> {
    let manager = app.autolaunch();
    let currently = manager
        .is_enabled()
        .map_err(|error| CommandError::System(error.to_string()))?;
    if currently == enabled {
        return Ok(());
    }
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|error| CommandError::System(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path().join("jarvis.db")).expect("open");
        (dir, db)
    }

    #[test]
    fn stores_and_reads_whitelisted_settings() {
        let (_dir, db) = temp_db();
        async_runtime::block_on(async {
            assert_eq!(
                read_setting(&db, settings::LANGUAGE.into()).await,
                Ok(Value::Null)
            );
            store_setting(&db, settings::LANGUAGE, &json!("en"))
                .await
                .expect("store");
            assert_eq!(
                read_setting(&db, settings::LANGUAGE.into()).await,
                Ok(json!("en"))
            );
            let audit = db.audit_list(5).await.expect("audit");
            assert_eq!(audit[0].action, "settings.set");
        });
    }

    #[test]
    fn refuses_to_read_unknown_keys() {
        let (_dir, db) = temp_db();
        let error = async_runtime::block_on(read_setting(&db, "secret.token".into()))
            .expect_err("unknown key");
        assert_eq!(error.code(), "VALIDATION");
    }

    #[test]
    fn secret_audit_records_the_name_only() {
        let args = secret_audit_args(secret::ELEVEN_KEY);
        assert_eq!(args, json!({"name": "elevenlabs_api_key"}));
        let (_dir, db) = temp_db();
        async_runtime::block_on(async {
            db.audit_append(
                "secret.set".into(),
                args,
                "medium".into(),
                true,
                format!("{} stored", secret::ELEVEN_KEY),
            )
            .await
            .expect("audit");
            let entries: Vec<AuditEntryDto> = db
                .audit_list(5)
                .await
                .expect("list")
                .into_iter()
                .map(AuditEntryDto::from)
                .collect();
            let text = serde_json::to_string(&entries).expect("serialize");
            assert!(text.contains("elevenlabs_api_key"));
            assert!(!text.contains("value"), "{text}");
        });
    }

    #[test]
    fn unknown_secret_names_are_validation_errors() {
        let error: CommandError = secret::validate_name("password")
            .expect_err("unknown")
            .into();
        assert_eq!(error.code(), "VALIDATION");
    }

    #[test]
    fn audit_dto_keeps_all_fields() {
        let dto = AuditEntryDto::from(AuditEntry {
            id: 7,
            ts: "t".into(),
            action: "a".into(),
            args: json!({}),
            risk: "safe".into(),
            ok: true,
            detail: "d".into(),
        });
        assert_eq!(
            serde_json::to_value(dto).expect("serialize"),
            json!({"id": 7, "ts": "t", "action": "a", "args": {}, "risk": "safe", "ok": true, "detail": "d"})
        );
    }
}
