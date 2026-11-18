//! J.A.R.V.I.S. desktop shell: hosts `jarvis-core` in-process, lives in the tray and
//! destroys the WebView whenever the window is not needed.

mod ai;
mod alerts;
mod commands;
mod control;
mod dispatch;
mod e2e;
mod error;
mod insight;
#[cfg(test)]
mod real_tests;
mod settings;
mod setup;
mod snapshot;
mod state;
mod sysconf;
mod tray;
mod tts;
mod voice;
mod window;

use anyhow::Context;
use jarvis_core::{Database, Event, EventBus, Service, Supervisor};
use jarvis_sysintel::{gpu::GpuSource, SysIntel};
use jarvis_voice::{default_transcriber, RouterService, VoiceControl, VoiceService};
use state::{lock, AppState};
use std::{fs::OpenOptions, sync::atomic::Ordering, sync::Arc};
use tauri::{async_runtime, AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use window::WindowMode;

/// The same executable doubles as the NVML helper of the system intelligence. It exists only while
/// the window is open (the tray reads the GPU through D3DKMT).
pub const GPU_WORKER_FLAG: &str = "--gpu-worker";

/// `jarvis.exe --setup-status <file.json>` writes what the first-run wizard would show and exits
/// (no window; used by the installer check).
pub const SETUP_STATUS_FLAG: &str = "--setup-status";

/// Writes the wizard status as JSON to `path`.
pub fn write_setup_status(path: &std::path::Path) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(&setup::status())?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Autostart passes this flag so the app starts without a window.
pub const TRAY_FLAG: &str = "--tray";

/// True when the process was launched with `--tray`.
pub fn starts_in_tray(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| argument == TRAY_FLAG)
}

fn init_logging(db: &Database) -> anyhow::Result<()> {
    let directory = db.logs_dir().context("resolving logs directory")?;
    std::fs::create_dir_all(&directory)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("jarvis.log"))?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initializing logging: {error}"))
}

async fn stored_mode(db: &Database, key: &str) -> Option<WindowMode> {
    let value = db.settings_get(key.to_owned()).await.ok().flatten()?;
    value.as_str().and_then(WindowMode::parse)
}

/// Graceful stop of the core; safe to call more than once.
fn shutdown_core(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(supervisor) = lock(&state.supervisor).take() else {
        return;
    };
    let result: anyhow::Result<()> = async_runtime::block_on(async {
        // Nobody may be subscribed; that is fine.
        let _ = state.bus.publish(Event::Shutdown);
        supervisor.stop().await?;
        state
            .db
            .audit_append(
                "core.stop".into(),
                serde_json::json!({}),
                "safe".into(),
                true,
                "jarvis stopped".into(),
            )
            .await?;
        Ok(())
    });
    match result {
        Ok(()) => tracing::info!("core stopped"),
        Err(error) => tracing::error!(%error, "core did not stop cleanly"),
    }
}

/// Opens the initial window (unless started with `--tray` or `general.start_mode = tray`)
/// and re-registers autostart when the setting says it is on.
fn spawn_startup(app: &AppHandle, tray_only: bool) {
    let handle = app.clone();
    async_runtime::spawn(async move {
        let db = handle.state::<AppState>().db.clone();
        if db
            .settings_get(settings::AUTOSTART.into())
            .await
            .ok()
            .flatten()
            == Some(serde_json::json!(true))
        {
            // Keeps the registry entry valid if the executable was moved.
            let for_autostart = handle.clone();
            let _ = async_runtime::spawn_blocking(move || {
                if let Err(error) = commands::set_autostart(&for_autostart, true) {
                    tracing::warn!(%error, "could not re-register autostart");
                }
            })
            .await;
        }
        if tray_only {
            return;
        }
        let start = stored_mode(&db, settings::START_MODE)
            .await
            .unwrap_or(WindowMode::Compact);
        if start != WindowMode::Tray {
            window::spawn_window_task(&handle, "startup", move |app| {
                window::apply_mode(app, start)
            });
        }
    });
}

fn register_hotkey(app: &AppHandle) {
    let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyJ);
    let result = app
        .global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                window::spawn_window_task(app, "hotkey", window::toggle);
            }
        });
    if let Err(error) = result {
        // Another application may own the combination; the tray still works.
        tracing::warn!(%error, "Ctrl+Alt+J is unavailable");
    }
}

/// Suffix that separates the single-instance identity of an isolated run (`JARVIS_DATA_DIR` set, as
/// in tests and installer checks) from the user's own instance: a test launch must never focus,
/// replace or be swallowed by the real application.
pub fn instance_suffix(data_dir: Option<&std::ffi::OsStr>) -> Option<String> {
    let text = data_dir?.to_string_lossy().to_lowercase();
    if text.is_empty() {
        return None;
    }
    // FNV-1a, 64 bit.
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(format!("iso{hash:016x}"))
}

pub fn run() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    let tray_only = starts_in_tray(&arguments);

    let mut context = tauri::generate_context!();
    if let Some(suffix) = instance_suffix(std::env::var_os("JARVIS_DATA_DIR").as_deref()) {
        let identifier = format!("{}.{suffix}", context.config().identifier);
        context.config_mut().identifier = identifier;
    }
    tauri::Builder::default()
        // Must be the first plugin: a second launch focuses the running window and exits.
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                window::spawn_window_task(app, "second-instance", |app| {
                    let last = *lock(&app.state::<AppState>().last_mode);
                    window::show_or_open(app, window::open_mode_for_start(last))
                });
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![TRAY_FLAG]),
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::core_info,
            commands::settings_get,
            commands::settings_set,
            commands::audit_list,
            commands::set_window_mode,
            commands::ptt,
            commands::voice_devices,
            commands::voice_status,
            commands::tts_speak,
            commands::tts_stop,
            commands::tts_voices,
            commands::audio_outputs,
            commands::secret_set,
            commands::secret_clear,
            commands::secret_exists,
            commands::chat_send,
            commands::confirm_response,
            commands::emergency_stop,
            commands::control_actions,
            commands::ai_status,
            commands::ai_unload,
            commands::memory_facts,
            commands::memory_fact_delete,
            commands::memory_history,
            commands::memory_history_delete,
            commands::memory_clear_history,
            commands::memory_clear_all,
            commands::system_snapshot,
            commands::system_top_processes,
            commands::system_alerts,
            commands::debug_raise_alert,
            commands::setup_status,
            commands::setup_install,
            commands::setup_pull_model,
            commands::setup_open_ollama_page,
        ])
        .setup(move |app| {
            let db =
                Database::open(Database::default_path()?).context("opening Jarvis database")?;
            init_logging(&db)?;
            tracing::info!(path = %db.path().display(), "jarvis starting");

            let bus = EventBus::new();
            // The system intelligence replaces the Phase 1 monitor: it still publishes
            // `Event::Metrics`, and reads the GPU through D3DKMT in the tray, NVML in a helper
            // process only while the window is open.
            let sys_config = async_runtime::block_on(sysconf::load_config(&db));
            let gpu_source = GpuSource::adaptive_from_current_exe(GPU_WORKER_FLAG)
                .context("locating the executable for the GPU helper")?;
            let sys = Arc::new(SysIntel::new(bus.clone(), sys_config).with_gpu_source(gpu_source));
            let voice_config = async_runtime::block_on(voice::load_config(&db));
            let voice = VoiceService::new(bus.clone(), voice_config);
            let control: Arc<dyn VoiceControl> = voice.clone();
            let router = RouterService::new(bus.clone(), default_transcriber(), control);
            let secrets: Arc<dyn jarvis_tts::SecretStore> = Arc::new(jarvis_tts::CredentialStore);
            let (engines, piper) = tts::build_engines(Arc::clone(&secrets));
            let tts_config = async_runtime::block_on(tts::load_config(&db));
            let tts_service = async_runtime::block_on(async {
                jarvis_tts::TtsService::new(
                    engines,
                    Arc::clone(&secrets),
                    Arc::new(tts::ShellObserver::new(bus.clone(), voice.clone())),
                    tts_config,
                )
            });
            let control_config = async_runtime::block_on(control::load_config(&db));
            let ai_config = async_runtime::block_on(ai::load_config(&db));
            let pieces = ai::Pieces::new(db.clone(), ai_config)?;
            // Screen vision: the indicator event follows every capture.
            let vision_settings: insight::SharedVisionSettings = Arc::new(std::sync::RwLock::new(
                async_runtime::block_on(insight::load_vision_settings(&db)),
            ));
            let (ollama, ai_shared) = pieces.shared();
            let handle_for_indicator = app.handle().clone();
            let indicator = Arc::new(insight::IndicatorObserver::new(move |active| {
                use tauri::Emitter;
                if let Err(error) = handle_for_indicator
                    .emit("vision://capture", serde_json::json!({ "active": active }))
                {
                    tracing::debug!(%error, "capture indicator was not delivered");
                }
            }));
            let vision_backend = Arc::new(insight::RealVision::new(
                Arc::clone(&vision_settings),
                ai_shared,
                ollama,
                indicator,
            ));
            let mut extras = pieces.extras();
            extras.extend(insight::actions(sys.clone(), vision_backend));
            let hub = Arc::new(control::ControlHub::new(
                control_config,
                Arc::new(control::DbAudit(db.clone())),
                extras,
            ));
            let port: Arc<dyn jarvis_brain::ActionPort> = hub.clone();
            let ai = pieces.finish(
                port,
                Arc::new(snapshot::SysIntelSnapshot::new(Arc::clone(&sys))),
                Arc::clone(&secrets),
            )?;
            let assistant: Arc<dyn dispatch::Assistant> = ai.brain.clone();
            let engine: Arc<dyn dispatch::Engine> = hub.clone();
            let shell: Arc<dyn dispatch::Shell> =
                Arc::new(control::AppShell::new(app.handle().clone()));
            let commands = dispatch::CommandCore::with_assistant(
                engine,
                shell,
                Arc::new(control::DbAudit(db.clone())),
                assistant,
            );
            let command_service = control::CommandService::new(bus.clone(), commands.clone());
            let services: Vec<Arc<dyn Service>> = vec![
                sys.clone(),
                voice.clone(),
                router,
                Arc::new(tts_service.clone()),
                command_service,
            ];
            // A service that failed or panicked is restarted by the supervisor; the user sees WARNING.
            let notifier: jarvis_core::FailureNotifier = {
                let voice = voice.clone();
                let db = db.clone();
                Arc::new(move |name, message| {
                    let voice = voice.clone();
                    let db = db.clone();
                    let message = message.to_owned();
                    async_runtime::spawn(async move {
                        voice.show_state("WARNING");
                        if let Err(error) = db
                            .audit_append(
                                "service.restart".into(),
                                serde_json::json!({ "service": name }),
                                "safe".into(),
                                false,
                                message,
                            )
                            .await
                        {
                            tracing::warn!(%error, "service failure was not audited");
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        voice.finish_processing();
                    });
                })
            };
            let mut supervisor = Supervisor::new(services).with_notifier(notifier);
            async_runtime::block_on(async {
                supervisor.start();
                db.audit_append(
                    "core.start".into(),
                    serde_json::json!({}),
                    "safe".into(),
                    true,
                    "jarvis started".into(),
                )
                .await
            })?;
            let last_mode = async_runtime::block_on(stored_mode(&db, settings::LAST_MODE))
                .filter(|mode| *mode != WindowMode::Tray)
                .unwrap_or(WindowMode::Compact);
            let ptt_key = async_runtime::block_on(voice::load_ptt_key(&db));
            app.manage(AppState::new(
                db,
                bus,
                supervisor,
                last_mode,
                voice,
                state::SpeechParts {
                    tts: tts_service,
                    piper,
                    secrets,
                },
                state::ControlParts {
                    hub,
                    commands,
                    ai: Arc::clone(&ai),
                    sys,
                    vision: vision_settings,
                },
            ));
            // Alerts: window event, notification while the window is closed, Neural Core, voice.
            let alert_bus = app.state::<AppState>().bus.clone();
            let pipeline = alerts::AlertPipeline::new(alerts::AppAlerts::new(app.handle().clone()));
            async_runtime::spawn(pipeline.run(alert_bus));

            // Retention: drop history older than the configured period.
            let memory = Arc::clone(&ai.memory);
            async_runtime::spawn(async move { memory.purge().await });
            tray::build(app.handle())?;
            register_hotkey(app.handle());
            if let Err(error) = voice::register_ptt(app.handle(), ptt_key) {
                // Another application may own the key; the UI button still works.
                tracing::warn!(%error, "push-to-talk key is unavailable");
            }
            let emergency_key =
                async_runtime::block_on(control::load_emergency_key(&app.state::<AppState>().db));
            if let Err(error) = control::register_emergency(app.handle(), emergency_key) {
                // Another application may own the key; the UI button still works.
                tracing::warn!(%error, "emergency stop key is unavailable");
            }
            e2e::install(app.handle());
            spawn_startup(app.handle(), tray_only);
            Ok(())
        })
        .build(context)
        .context("building the Tauri application")?
        .run(|app, event| match event {
            // Closing the last window must not end the process; only the tray quit item does.
            RunEvent::ExitRequested { api, code, .. } => {
                let quitting = app
                    .try_state::<AppState>()
                    .is_some_and(|state| state.quitting.load(Ordering::SeqCst));
                if code.is_none() && !quitting {
                    api.prevent_exit();
                }
            }
            RunEvent::Exit => shutdown_core(app),
            _ => {}
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_runs_get_their_own_instance_identity() {
        use std::ffi::OsStr;
        assert_eq!(instance_suffix(None), None);
        assert_eq!(instance_suffix(Some(OsStr::new(""))), None);
        let a = instance_suffix(Some(OsStr::new(r"C:\Temp\one"))).expect("suffix");
        let b = instance_suffix(Some(OsStr::new(r"C:\Temp\two"))).expect("suffix");
        assert_ne!(a, b);
        assert_eq!(
            Some(a),
            instance_suffix(Some(OsStr::new(r"c:\temp\ONE"))),
            "case does not matter on Windows"
        );
    }

    #[test]
    fn detects_the_tray_flag() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert!(starts_in_tray(&args(&["jarvis.exe", "--tray"])));
        assert!(!starts_in_tray(&args(&["jarvis.exe"])));
        assert!(!starts_in_tray(&args(&["jarvis.exe", "--tray=1"])));
    }
}
