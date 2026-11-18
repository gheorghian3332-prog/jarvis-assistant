use crate::{
    settings,
    state::AppState,
    window::{self, WindowMode},
};
use std::sync::atomic::Ordering;
use tauri::{
    async_runtime,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

const ID_OPEN: &str = "open";
const ID_COMPACT: &str = "compact";
const ID_HIDE: &str = "hide";
const ID_QUIT: &str = "quit";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, ID_OPEN, "Открыть", true, None::<&str>)?,
            &MenuItem::with_id(app, ID_COMPACT, "Компактный режим", true, None::<&str>)?,
            &MenuItem::with_id(app, ID_HIDE, "Скрыть в трей", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ID_QUIT, "Выход", true, None::<&str>)?,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("J.A.R.V.I.S.")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            ID_OPEN => window::spawn_window_task(app, "tray-open", |app| {
                window::apply_mode(app, WindowMode::Full)
            }),
            ID_COMPACT => window::spawn_window_task(app, "tray-compact", |app| {
                window::apply_mode(app, WindowMode::Compact)
            }),
            ID_HIDE => window::spawn_window_task(app, "tray-hide", |app| {
                window::apply_mode(app, WindowMode::Tray)
            }),
            ID_QUIT => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                left_click(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Window exists -> destroy it (tray). Otherwise open it in `general.start_mode` (compact if that is `tray`).
fn left_click(app: &AppHandle) {
    let handle = app.clone();
    async_runtime::spawn(async move {
        let mode = if window::window_exists(&handle) {
            WindowMode::Tray
        } else {
            let stored = handle
                .state::<AppState>()
                .db
                .settings_get(settings::START_MODE.into())
                .await
                .ok()
                .flatten();
            let start = stored
                .as_ref()
                .and_then(|value| value.as_str())
                .and_then(WindowMode::parse)
                .unwrap_or(WindowMode::Compact);
            window::open_mode_for_start(start)
        };
        window::spawn_window_task(&handle, "tray-click", move |app| {
            window::apply_mode(app, mode)
        });
    });
}

/// The only way out of the process: the core is stopped in `RunEvent::Exit`.
pub fn quit(app: &AppHandle) {
    app.state::<AppState>()
        .quitting
        .store(true, Ordering::SeqCst);
    app.exit(0);
}
