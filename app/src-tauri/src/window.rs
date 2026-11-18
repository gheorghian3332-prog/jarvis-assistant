use crate::voice::{LevelPayload, MicPayload, TranscriptPayload, WakePayload};
use crate::{error::CommandError, state::lock, state::AppState};
use jarvis_core::{Event, Metrics, VoiceHint};
use serde::{Deserialize, Serialize};
use std::{thread, time::Duration};
use tauri::{
    async_runtime, webview::PageLoadEvent, window::Color, AppHandle, Emitter, LogicalSize, Manager,
    PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tokio::sync::broadcast::error::RecvError;

pub const MAIN_LABEL: &str = "main";
pub const FULL_SIZE: (f64, f64) = (1280.0, 800.0);
pub const COMPACT_SIZE: (f64, f64) = (420.0, 640.0);
const BACKGROUND: Color = Color(8, 12, 16, 255);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Full,
    Compact,
    Tray,
}

impl WindowMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
            Self::Tray => "tray",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "full" => Some(Self::Full),
            "compact" => Some(Self::Compact),
            "tray" => Some(Self::Tray),
            _ => None,
        }
    }
}

/// Mode for opening the window from the tray icon: `general.start_mode`, or compact if that is `tray`.
pub fn open_mode_for_start(start_mode: WindowMode) -> WindowMode {
    match start_mode {
        WindowMode::Tray => WindowMode::Compact,
        visible => visible,
    }
}

/// Top-left physical position that puts a `logical_size` window in the bottom-right corner of a work area.
pub fn compact_position(
    work_position: (i32, i32),
    work_size: (u32, u32),
    scale: f64,
    logical_size: (f64, f64),
) -> (i32, i32) {
    let width = (logical_size.0 * scale).round() as i32;
    let height = (logical_size.1 * scale).round() as i32;
    let right = work_position.0.saturating_add(work_size.0 as i32);
    let bottom = work_position.1.saturating_add(work_size.1 as i32);
    (
        (right - width).max(work_position.0),
        (bottom - height).max(work_position.1),
    )
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetricsPayload {
    pub cpu_percent: f32,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
}

impl From<&Metrics> for MetricsPayload {
    fn from(metrics: &Metrics) -> Self {
        Self {
            cpu_percent: metrics.cpu_percent,
            ram_used_bytes: metrics.used_memory_bytes,
            ram_total_bytes: metrics.total_memory_bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatePayload {
    pub state: String,
}

/// The state the window should show when it opens (the voice front may already be listening).
fn current_state(app: &AppHandle) -> StatePayload {
    StatePayload {
        state: app.state::<AppState>().voice.snapshot().state,
    }
}

/// Subscribes to the bus, tells the system intelligence that a window is open (1 Hz sampling, the
/// NVML helper) and pushes events to the UI.
fn start_forwarder(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut slot = lock(&state.forwarder);
    if slot.is_some() {
        return;
    }
    let mut receiver = state.bus.subscribe();
    state.sys.set_window_open(true);
    // Level events are only wanted while a window can show them.
    state.voice.set_level_enabled(true);
    state.tts.set_level_enabled(true);
    let handle = app.clone();
    *slot = Some(async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => match &*event {
                    Event::Metrics(metrics) => {
                        let _ = handle.emit("system://metrics", MetricsPayload::from(metrics));
                    }
                    Event::StateChanged(name) => {
                        let _ = handle.emit(
                            "assistant://state",
                            StatePayload {
                                state: name.to_string(),
                            },
                        );
                    }
                    Event::VoiceStateHint(hint) => match hint {
                        VoiceHint::State(name) => {
                            let _ = handle.emit(
                                "assistant://state",
                                StatePayload {
                                    state: name.to_string(),
                                },
                            );
                        }
                        VoiceHint::Level(rms) => {
                            let _ = handle.emit("assistant://level", LevelPayload { rms: *rms });
                        }
                        VoiceHint::Mic(status) => {
                            let _ = handle.emit("voice://status", MicPayload::from(status));
                        }
                    },
                    Event::WakeDetected { name, .. } => {
                        let _ = handle.emit(
                            "voice://wake",
                            WakePayload {
                                name: name.to_string(),
                            },
                        );
                    }
                    // Audio goes to the speech-to-text worker, never to the window.
                    Event::Utterance { .. } => {}
                    Event::Transcript { text, lang, .. } => {
                        let _ = handle.emit(
                            "voice://transcript",
                            TranscriptPayload {
                                text: text.to_string(),
                                is_final: true,
                                lang: lang.to_string(),
                            },
                        );
                    }
                    // Alerts are handled by the alert pipeline (alerts.rs), not by the forwarder.
                    Event::Alert(_) | Event::AppCrashed { .. } => {}
                    Event::Shutdown => break,
                },
                // Some metrics were skipped; the next one is fresh anyway.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    }));
}

/// Drops the bus subscription; the system intelligence falls back to the tray rates and ends the
/// NVML helper.
fn stop_forwarder(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.sys.set_window_open(false);
    state.voice.set_level_enabled(false);
    state.tts.set_level_enabled(false);
    let task = lock(&state.forwarder).take();
    if let Some(task) = task {
        task.abort();
    }
}

fn build_window(app: &AppHandle) -> Result<WebviewWindow, CommandError> {
    let window = WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App("index.html".into()))
        .title("J.A.R.V.I.S.")
        .visible(false)
        .background_color(BACKGROUND)
        .inner_size(FULL_SIZE.0, FULL_SIZE.1)
        .on_page_load(|webview, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                let app = webview.app_handle();
                let _ = webview.emit("assistant://state", current_state(app));
                if let Some(mic) = app.state::<AppState>().voice.snapshot().mic {
                    let _ = webview.emit("voice://status", MicPayload::from(&mic));
                }
            }
        })
        .build()?;
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        // The close button means "go to tray": the window is destroyed, not hidden.
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            spawn_window_task(&handle, "close-to-tray", |app| {
                apply_mode(app, WindowMode::Tray)
            });
        }
        WindowEvent::Destroyed => stop_forwarder(&handle),
        _ => {}
    });
    Ok(window)
}

fn configure(
    app: &AppHandle,
    window: &WebviewWindow,
    mode: WindowMode,
) -> Result<(), CommandError> {
    window.unmaximize()?;
    match mode {
        WindowMode::Full => {
            window.set_always_on_top(false)?;
            window.set_skip_taskbar(false)?;
            window.set_decorations(true)?;
            window.set_resizable(true)?;
            window.set_maximizable(true)?;
            window.set_size(LogicalSize::new(FULL_SIZE.0, FULL_SIZE.1))?;
            window.center()?;
        }
        WindowMode::Compact => {
            window.set_decorations(false)?;
            window.set_maximizable(false)?;
            window.set_resizable(false)?;
            window.set_always_on_top(true)?;
            window.set_skip_taskbar(true)?;
            window.set_size(LogicalSize::new(COMPACT_SIZE.0, COMPACT_SIZE.1))?;
            if let Some(monitor) = app.primary_monitor()? {
                let area = monitor.work_area();
                let (x, y) = compact_position(
                    (area.position.x, area.position.y),
                    (area.size.width, area.size.height),
                    monitor.scale_factor(),
                    COMPACT_SIZE,
                );
                // Windows keeps an invisible resize border around the client area; aim the visible
                // (inner) rectangle at the corner, not the outer one.
                let outer = window.outer_position()?;
                let inner = window.inner_position()?;
                window.set_position(PhysicalPosition::new(
                    x - (inner.x - outer.x),
                    y - (inner.y - outer.y),
                ))?;
            }
        }
        WindowMode::Tray => {}
    }
    Ok(())
}

fn destroy_main(app: &AppHandle) -> Result<(), CommandError> {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        window.destroy()?;
        // `destroy` completes on the event loop; wait so a following build can reuse the label.
        for _ in 0..200 {
            if app.get_webview_window(MAIN_LABEL).is_none() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    stop_forwarder(app);
    Ok(())
}

/// Switches the window mode. Blocking: call from a worker thread, never from the event loop thread.
pub fn apply_mode(app: &AppHandle, mode: WindowMode) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.mode_lock);
    match mode {
        WindowMode::Tray => destroy_main(app),
        WindowMode::Full | WindowMode::Compact => {
            let window = match app.get_webview_window(MAIN_LABEL) {
                Some(window) => window,
                None => build_window(app)?,
            };
            configure(app, &window, mode)?;
            window.show()?;
            window.set_focus()?;
            start_forwarder(app);
            remember_mode(app, mode);
            Ok(())
        }
    }
}

fn remember_mode(app: &AppHandle, mode: WindowMode) {
    let state = app.state::<AppState>();
    {
        let mut last = lock(&state.last_mode);
        if *last == mode {
            return;
        }
        *last = mode;
    }
    let db = state.db.clone();
    async_runtime::spawn(async move {
        if let Err(error) = db
            .settings_set(
                crate::settings::LAST_MODE.into(),
                serde_json::json!(mode.as_str()),
            )
            .await
        {
            tracing::warn!(%error, "could not persist ui.last_mode");
        }
    });
}

pub fn window_exists(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN_LABEL).is_some()
}

/// Brings an existing window to the front, or opens one in `fallback`.
pub fn show_or_open(app: &AppHandle, fallback: WindowMode) -> Result<(), CommandError> {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        Ok(())
    } else {
        apply_mode(app, fallback)
    }
}

/// Hotkey behaviour: tray <-> last visible mode.
pub fn toggle(app: &AppHandle) -> Result<(), CommandError> {
    if window_exists(app) {
        apply_mode(app, WindowMode::Tray)
    } else {
        let last = *lock(&app.state::<AppState>().last_mode);
        apply_mode(app, open_mode_for_start(last))
    }
}

/// Runs a blocking window action on the worker pool and logs failures.
pub fn spawn_window_task<F>(app: &AppHandle, name: &'static str, task: F)
where
    F: FnOnce(&AppHandle) -> Result<(), CommandError> + Send + 'static,
{
    let handle = app.clone();
    async_runtime::spawn_blocking(move || {
        if let Err(error) = task(&handle) {
            tracing::warn!(%error, task = name, "window action failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(WindowMode::parse("full"), Some(WindowMode::Full));
        assert_eq!(WindowMode::parse("compact"), Some(WindowMode::Compact));
        assert_eq!(WindowMode::parse("tray"), Some(WindowMode::Tray));
        assert_eq!(WindowMode::parse("hidden"), None);
        assert_eq!(WindowMode::Tray.as_str(), "tray");
    }

    #[test]
    fn deserializes_command_argument() {
        let mode: WindowMode = serde_json::from_str("\"compact\"").expect("mode");
        assert_eq!(mode, WindowMode::Compact);
        assert!(serde_json::from_str::<WindowMode>("\"minimized\"").is_err());
    }

    #[test]
    fn tray_start_opens_compact() {
        assert_eq!(open_mode_for_start(WindowMode::Tray), WindowMode::Compact);
        assert_eq!(open_mode_for_start(WindowMode::Full), WindowMode::Full);
        assert_eq!(
            open_mode_for_start(WindowMode::Compact),
            WindowMode::Compact
        );
    }

    #[test]
    fn compact_window_sits_in_the_bottom_right_corner() {
        // 1920x1040 work area (taskbar excluded), 100% scale.
        assert_eq!(
            compact_position((0, 0), (1920, 1040), 1.0, COMPACT_SIZE),
            (1500, 400)
        );
        // 150% scale: the window is 630x960 physical pixels.
        assert_eq!(
            compact_position((0, 0), (2560, 1380), 1.5, COMPACT_SIZE),
            (1930, 420)
        );
        // Secondary-origin monitors and tiny work areas never push the window off the area.
        assert_eq!(
            compact_position((-1920, 0), (1920, 1040), 1.0, COMPACT_SIZE),
            (-420, 400)
        );
        assert_eq!(
            compact_position((10, 20), (300, 300), 1.0, COMPACT_SIZE),
            (10, 20)
        );
    }

    #[test]
    fn maps_metrics_payload() {
        let payload = MetricsPayload::from(&Metrics {
            cpu_percent: 12.5,
            used_memory_bytes: 4,
            total_memory_bytes: 8,
        });
        assert_eq!(
            serde_json::to_value(payload).expect("serialize"),
            serde_json::json!({"cpu_percent": 12.5, "ram_used_bytes": 4, "ram_total_bytes": 8})
        );
    }
}
