//! The built-in Windows actions: typed arguments, validation and execution on top of the controllers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::action::{Action, ActionResult, Risk};
use crate::apps::{self, AppCache, AppIndex};
use crate::audio::{self, MediaKey};
use crate::config::ControlConfig;
use crate::error::{ControlError, ControlResult};
use crate::files;
use crate::info;
use crate::network;
use crate::power;
use crate::procs::{self, SortKey};
use crate::registry::Registry;
use crate::replies;
use crate::wnd;

/// Shared state of the built-in actions.
pub struct Context {
    pub config: ControlConfig,
    pub roots: Vec<PathBuf>,
    apps: Mutex<AppCache>,
    pub shutdown_scheduled: Arc<AtomicBool>,
}

impl Context {
    pub fn new(config: ControlConfig, shutdown_scheduled: Arc<AtomicBool>) -> Self {
        let roots = config.effective_roots();
        Self {
            config,
            roots,
            apps: Mutex::new(AppCache::default()),
            shutdown_scheduled,
        }
    }

    /// App index, rebuilt at most every 10 minutes unless `force` is set.
    pub fn app_index(&self, force: bool) -> Arc<AppIndex> {
        match self.apps.lock() {
            Ok(mut cache) => cache.get(&self.config, force),
            Err(_) => Arc::new(AppIndex::build(&self.config)),
        }
    }
}

// ---------- typed arguments ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    query: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LevelArgs {
    level: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeltaArgs {
    delta: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaArgs {
    key: MediaKey,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceArgs {
    /// Known folder name (`downloads`, `загрузки`, ...).
    name: Option<String>,
    /// Absolute path (or a drive root such as `E:\`).
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessListArgs {
    sort: Option<SortKey>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KillArgs {
    name: Option<String>,
    pid: Option<u32>,
}

fn args<T: DeserializeOwned>(value: &Value) -> ControlResult<T> {
    let value = if value.is_null() {
        json!({})
    } else {
        value.clone()
    };
    serde_json::from_value(value).map_err(|error| ControlError::validation(error.to_string()))
}

fn non_empty(text: &str, what: &str) -> ControlResult<()> {
    if text.trim().is_empty() {
        Err(ControlError::validation(format!(
            "{what} must not be empty"
        )))
    } else {
        Ok(())
    }
}

// ---------- the generic action wrapper ----------

type ValidateFn = fn(&Context, &Value) -> ControlResult<()>;
type RunFn = fn(&Context, Value) -> ControlResult<ActionResult>;
type RiskFn = fn(&Value) -> Risk;
type SummaryFn = fn(&Value) -> String;

struct Builtin {
    name: &'static str,
    risk: Risk,
    ru: &'static str,
    en: &'static str,
    validate: ValidateFn,
    run: RunFn,
    effective: Option<RiskFn>,
    summary: Option<SummaryFn>,
    ctx: Arc<Context>,
}

#[async_trait]
impl Action for Builtin {
    fn name(&self) -> &'static str {
        self.name
    }
    fn risk(&self) -> Risk {
        self.risk
    }
    fn description_ru(&self) -> &'static str {
        self.ru
    }
    fn description_en(&self) -> &'static str {
        self.en
    }
    fn validate(&self, value: &Value) -> ControlResult<()> {
        (self.validate)(&self.ctx, value)
    }
    fn effective_risk(&self, value: &Value) -> Risk {
        self.effective.map_or(self.risk, |function| function(value))
    }
    fn summary(&self, value: &Value) -> String {
        match self.summary {
            Some(function) => function(value),
            None => format!("{} {}", self.name, value),
        }
    }
    async fn run(&self, value: Value) -> ControlResult<ActionResult> {
        let ctx = self.ctx.clone();
        let function = self.run;
        tokio::task::spawn_blocking(move || function(&ctx, value))
            .await
            .map_err(|error| ControlError::system(format!("action task failed: {error}")))?
    }
}

// ---------- validators ----------

fn v_none(_: &Context, value: &Value) -> ControlResult<()> {
    args::<NoArgs>(value).map(|_| ())
}

fn v_query(_: &Context, value: &Value) -> ControlResult<()> {
    non_empty(&args::<QueryArgs>(value)?.query, "query")
}

fn v_level(_: &Context, value: &Value) -> ControlResult<()> {
    let parsed: LevelArgs = args(value)?;
    if parsed.level > 100 {
        return Err(ControlError::validation("level must be 0..=100"));
    }
    Ok(())
}

fn v_delta(_: &Context, value: &Value) -> ControlResult<()> {
    let parsed: DeltaArgs = args(value)?;
    if parsed.delta == 0 || !(-100..=100).contains(&parsed.delta) {
        return Err(ControlError::validation(
            "delta must be -100..=100 and not 0",
        ));
    }
    Ok(())
}

fn v_media(_: &Context, value: &Value) -> ControlResult<()> {
    args::<MediaArgs>(value).map(|_| ())
}

fn place_path(ctx: &Context, parsed: &PlaceArgs) -> ControlResult<PathBuf> {
    match (&parsed.name, &parsed.path) {
        (Some(name), None) => {
            let key = files::known_folder_key(name)
                .ok_or_else(|| ControlError::validation(format!("unknown folder \"{name}\"")))?;
            files::known_folder_path(key)
        }
        (None, Some(path)) => {
            let trimmed = path.trim().trim_matches('"');
            let bytes = trimmed.as_bytes();
            let is_drive_root = bytes.len() <= 3
                && bytes.first().is_some_and(u8::is_ascii_alphabetic)
                && bytes.get(1) == Some(&b':')
                && bytes.get(2).is_none_or(|b| *b == b'\\' || *b == b'/');
            if is_drive_root {
                files::drive_root(bytes[0] as char)
            } else {
                files::resolve_in_roots(trimmed, &ctx.roots)
            }
        }
        _ => Err(ControlError::validation(
            "exactly one of \"name\" or \"path\" is required",
        )),
    }
}

fn v_place(ctx: &Context, value: &Value) -> ControlResult<()> {
    let parsed: PlaceArgs = args(value)?;
    place_path(ctx, &parsed).map(|_| ())
}

fn v_delete(ctx: &Context, value: &Value) -> ControlResult<()> {
    let parsed: DeleteArgs = args(value)?;
    files::resolve_strictly_inside(&parsed.path, &ctx.roots).map(|_| ())
}

fn v_process_list(_: &Context, value: &Value) -> ControlResult<()> {
    let parsed: ProcessListArgs = args(value)?;
    if parsed.limit.is_some_and(|limit| limit == 0 || limit > 100) {
        return Err(ControlError::validation("limit must be 1..=100"));
    }
    Ok(())
}

fn v_kill(_: &Context, value: &Value) -> ControlResult<()> {
    let parsed: KillArgs = args(value)?;
    match (&parsed.name, parsed.pid) {
        (Some(name), None) => {
            non_empty(name, "name")?;
            if procs::is_protected_name(name) {
                return Err(ControlError::forbidden(format!(
                    "{name} is a protected process"
                )));
            }
            Ok(())
        }
        (None, Some(pid)) => {
            if pid <= 4 || pid == std::process::id() {
                return Err(ControlError::forbidden(format!("pid {pid} is protected")));
            }
            Ok(())
        }
        _ => Err(ControlError::validation(
            "exactly one of \"name\" or \"pid\" is required",
        )),
    }
}

// ---------- risk and summaries ----------

fn kill_risk(value: &Value) -> Risk {
    match args::<KillArgs>(value) {
        Ok(KillArgs {
            name: Some(name), ..
        }) if procs::is_protected_name(&name) => Risk::Forbidden,
        Ok(KillArgs { pid: Some(pid), .. }) if pid <= 4 || pid == std::process::id() => {
            Risk::Forbidden
        }
        _ => Risk::Confirm,
    }
}

fn kill_summary(value: &Value) -> String {
    match args::<KillArgs>(value) {
        Ok(KillArgs {
            name: Some(name), ..
        }) => format!("Terminate process \"{name}\""),
        Ok(KillArgs { pid: Some(pid), .. }) => format!("Terminate process with PID {pid}"),
        _ => "Terminate a process".into(),
    }
}

fn delete_summary(value: &Value) -> String {
    match args::<DeleteArgs>(value) {
        Ok(parsed) => format!("Move to the Recycle Bin: {}", parsed.path),
        Err(_) => "Move a file to the Recycle Bin".into(),
    }
}

// ---------- runners ----------

fn r_app_open(ctx: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: QueryArgs = args(&value)?;
    let index = ctx.app_index(false);
    let found = match index.best(&parsed.query) {
        Some(found) => found,
        None => {
            // The cache may be stale (a freshly installed app): retry once on a fresh index.
            let fresh = ctx.app_index(true);
            fresh.best(&parsed.query).ok_or_else(|| {
                ControlError::not_found(format!("no application matches \"{}\"", parsed.query))
            })?
        }
    };
    apps::launch(&found)?;
    Ok(ActionResult::new(
        replies::open_app(&found.name),
        json!({"name": found.name, "launch": found.launch, "source": found.source.as_str(), "score": found.score}),
    ))
}

fn r_app_refresh(ctx: &Context, _: Value) -> ControlResult<ActionResult> {
    let index = ctx.app_index(true);
    let count = index.len();
    Ok(ActionResult::new(
        replies::Reply::new(
            format!("Индекс приложений обновлён: {count}."),
            format!("App index refreshed: {count} entries."),
        ),
        json!({"count": count}),
    ))
}

fn r_window_list(_: &Context, _: Value) -> ControlResult<ActionResult> {
    let windows = wnd::list_windows()?;
    Ok(ActionResult::new(
        replies::windows_listed(windows.len()),
        json!({"windows": windows}),
    ))
}

fn r_window_focus(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: QueryArgs = args(&value)?;
    let window = wnd::focus(&parsed.query)?;
    Ok(ActionResult::new(
        replies::window_focused(&window.title),
        json!({"window": window}),
    ))
}

fn r_window_minimize(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: QueryArgs = args(&value)?;
    let window = wnd::minimize(&parsed.query)?;
    Ok(ActionResult::new(
        replies::window_minimized(&window.title),
        json!({"window": window}),
    ))
}

fn r_window_maximize(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: QueryArgs = args(&value)?;
    let window = wnd::maximize(&parsed.query)?;
    Ok(ActionResult::new(
        replies::window_maximized(&window.title),
        json!({"window": window}),
    ))
}

fn r_window_close(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: QueryArgs = args(&value)?;
    let closed = wnd::close(&parsed.query)?;
    let title = closed
        .first()
        .map(|window| window.title.clone())
        .unwrap_or_default();
    Ok(ActionResult::new(
        replies::window_closed(&title),
        json!({"closed": closed}),
    ))
}

fn r_minimize_all(_: &Context, _: Value) -> ControlResult<ActionResult> {
    wnd::minimize_all()?;
    Ok(ActionResult::new(replies::minimized_all(), Value::Null))
}

fn r_volume_get(_: &Context, _: Value) -> ControlResult<ActionResult> {
    let level = audio::get_volume()?;
    let muted = audio::is_muted()?;
    Ok(ActionResult::new(
        replies::volume_is(level),
        json!({"level": level, "muted": muted}),
    ))
}

fn r_volume_set(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: LevelArgs = args(&value)?;
    let level = audio::set_volume(parsed.level)?;
    Ok(ActionResult::new(
        replies::volume_set(level),
        json!({"level": level}),
    ))
}

fn r_volume_change(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: DeltaArgs = args(&value)?;
    let level = audio::change_volume(parsed.delta)?;
    Ok(ActionResult::new(
        replies::volume_set(level),
        json!({"level": level}),
    ))
}

fn r_mute(_: &Context, _: Value) -> ControlResult<ActionResult> {
    audio::set_mute(true)?;
    Ok(ActionResult::new(replies::muted(), Value::Null))
}

fn r_unmute(_: &Context, _: Value) -> ControlResult<ActionResult> {
    audio::set_mute(false)?;
    Ok(ActionResult::new(replies::unmuted(), Value::Null))
}

fn r_media(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: MediaArgs = args(&value)?;
    audio::media_key(parsed.key)?;
    Ok(ActionResult::new(
        replies::media(parsed.key.as_str()),
        json!({"key": parsed.key.as_str()}),
    ))
}

fn r_open_folder(ctx: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: PlaceArgs = args(&value)?;
    let path = place_path(ctx, &parsed)?;
    // Security: ShellExecute "open" on a file would execute it (exe, bat, lnk...). Only folders may be opened.
    if !path.is_dir() {
        return Err(ControlError::forbidden(format!(
            "{} is not a folder; only folders can be opened",
            path.display()
        )));
    }
    files::open_in_explorer(&path)?;
    let label = parsed
        .name
        .clone()
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let reply = if path.parent().is_none() {
        replies::drive_opened(&path.to_string_lossy())
    } else {
        replies::folder_opened(&label)
    };
    Ok(ActionResult::new(reply, json!({"path": path})))
}

fn r_list_dir(ctx: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: PlaceArgs = args(&value)?;
    let path = place_path(ctx, &parsed)?;
    let (entries, total) = files::list_dir(&path)?;
    Ok(ActionResult::new(
        replies::listing(total),
        json!({"path": path, "total": total, "entries": entries}),
    ))
}

fn r_delete(ctx: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: DeleteArgs = args(&value)?;
    let path = files::resolve_strictly_inside(&parsed.path, &ctx.roots)?;
    files::delete_to_recycle_bin(&path)?;
    Ok(ActionResult::new(
        replies::deleted_to_recycle_bin(),
        json!({"path": path}),
    ))
}

fn r_process_list(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: ProcessListArgs = args(&value)?;
    let sort = parsed.sort.unwrap_or(SortKey::Cpu);
    let processes = procs::list(sort, parsed.limit.unwrap_or(10));
    Ok(ActionResult::new(
        replies::processes_listed(processes.len()),
        json!({"processes": processes}),
    ))
}

fn r_process_hung(_: &Context, _: Value) -> ControlResult<ActionResult> {
    let hung = procs::hung_windows()?;
    Ok(ActionResult::new(
        replies::hung_listed(hung.len()),
        json!({"windows": hung}),
    ))
}

fn r_process_kill(_: &Context, value: Value) -> ControlResult<ActionResult> {
    let parsed: KillArgs = args(&value)?;
    let targets = procs::find(parsed.name.as_deref(), parsed.pid);
    if targets.is_empty() {
        return Err(ControlError::not_found("no matching process is running"));
    }
    if let Some(protected) = targets.iter().find(|p| procs::is_protected(p.pid, &p.name)) {
        return Err(ControlError::forbidden(format!(
            "process {} is protected",
            protected.name
        )));
    }
    for target in &targets {
        procs::terminate(target.pid, &target.name)?;
    }
    let name = targets[0].name.clone();
    Ok(ActionResult::new(
        replies::process_killed(&name),
        json!({"terminated": targets}),
    ))
}

fn r_network(_: &Context, _: Value) -> ControlResult<ActionResult> {
    let report = network::check();
    Ok(ActionResult::new(
        replies::internet(report.online),
        json!(report),
    ))
}

fn r_lock(_: &Context, _: Value) -> ControlResult<ActionResult> {
    power::lock()?;
    Ok(ActionResult::new(replies::locked(), Value::Null))
}

fn r_sleep(_: &Context, _: Value) -> ControlResult<ActionResult> {
    power::suspend(false)?;
    Ok(ActionResult::new(replies::sleeping(), Value::Null))
}

fn r_hibernate(_: &Context, _: Value) -> ControlResult<ActionResult> {
    power::suspend(true)?;
    Ok(ActionResult::new(replies::hibernating(), Value::Null))
}

fn r_shutdown(ctx: &Context, _: Value) -> ControlResult<ActionResult> {
    power::schedule_shutdown(false)?;
    ctx.shutdown_scheduled.store(true, Ordering::SeqCst);
    Ok(ActionResult::new(
        replies::shutdown_scheduled(power::SHUTDOWN_DELAY_SECONDS),
        json!({"delay_seconds": power::SHUTDOWN_DELAY_SECONDS}),
    ))
}

fn r_restart(ctx: &Context, _: Value) -> ControlResult<ActionResult> {
    power::schedule_shutdown(true)?;
    ctx.shutdown_scheduled.store(true, Ordering::SeqCst);
    Ok(ActionResult::new(
        replies::restart_scheduled(power::SHUTDOWN_DELAY_SECONDS),
        json!({"delay_seconds": power::SHUTDOWN_DELAY_SECONDS}),
    ))
}

fn r_abort(ctx: &Context, _: Value) -> ControlResult<ActionResult> {
    let aborted = power::abort_shutdown()?;
    ctx.shutdown_scheduled.store(false, Ordering::SeqCst);
    let reply = if aborted {
        replies::shutdown_aborted()
    } else {
        replies::nothing_to_abort()
    };
    Ok(ActionResult::new(reply, json!({"aborted": aborted})))
}

fn r_time(_: &Context, _: Value) -> ControlResult<ActionResult> {
    let now = info::now();
    Ok(ActionResult::new(
        replies::time_now(now.hour, now.minute),
        json!({"hour": now.hour, "minute": now.minute}),
    ))
}

fn r_date(_: &Context, _: Value) -> ControlResult<ActionResult> {
    let now = info::now();
    Ok(ActionResult::new(
        replies::date_today(now.day, now.month, now.year, now.weekday),
        json!({"year": now.year, "month": now.month, "day": now.day, "weekday": now.weekday}),
    ))
}

/// Registers every built-in action. `disabled` comes from the outside (P6-B settings).
pub fn build_registry(ctx: Arc<Context>) -> Registry {
    let mut registry = Registry::new(ctx.config.disabled_actions.clone());
    let mut add = |name: &'static str,
                   risk: Risk,
                   ru: &'static str,
                   en: &'static str,
                   validate: ValidateFn,
                   run: RunFn,
                   effective: Option<RiskFn>,
                   summary: Option<SummaryFn>| {
        registry.register(Arc::new(Builtin {
            name,
            risk,
            ru,
            en,
            validate,
            run,
            effective,
            summary,
            ctx: ctx.clone(),
        }));
    };
    use Risk::{Confirm, Safe};
    add(
        "app.open",
        Safe,
        "Открыть приложение или игру",
        "Open an application or game",
        v_query,
        r_app_open,
        None,
        None,
    );
    add(
        "app.refresh",
        Safe,
        "Обновить индекс приложений",
        "Rebuild the application index",
        v_none,
        r_app_refresh,
        None,
        None,
    );
    add(
        "window.list",
        Safe,
        "Список открытых окон",
        "List open windows",
        v_none,
        r_window_list,
        None,
        None,
    );
    add(
        "window.focus",
        Safe,
        "Переключиться на окно",
        "Focus a window",
        v_query,
        r_window_focus,
        None,
        None,
    );
    add(
        "window.minimize",
        Safe,
        "Свернуть окно",
        "Minimize a window",
        v_query,
        r_window_minimize,
        None,
        None,
    );
    add(
        "window.maximize",
        Safe,
        "Развернуть окно",
        "Maximize a window",
        v_query,
        r_window_maximize,
        None,
        None,
    );
    add(
        "window.close",
        Safe,
        "Закрыть окно (WM_CLOSE, приложение спросит о несохранённом)",
        "Close a window (WM_CLOSE; the app prompts about unsaved work)",
        v_query,
        r_window_close,
        None,
        None,
    );
    add(
        "window.minimize_all",
        Safe,
        "Свернуть все окна",
        "Minimize all windows",
        v_none,
        r_minimize_all,
        None,
        None,
    );
    add(
        "audio.get_volume",
        Safe,
        "Текущая громкость",
        "Read the master volume",
        v_none,
        r_volume_get,
        None,
        None,
    );
    add(
        "audio.set_volume",
        Safe,
        "Установить громкость 0-100",
        "Set the master volume 0-100",
        v_level,
        r_volume_set,
        None,
        None,
    );
    add(
        "audio.change_volume",
        Safe,
        "Изменить громкость на N",
        "Change the master volume by N",
        v_delta,
        r_volume_change,
        None,
        None,
    );
    add(
        "audio.mute",
        Safe,
        "Выключить звук",
        "Mute",
        v_none,
        r_mute,
        None,
        None,
    );
    add(
        "audio.unmute",
        Safe,
        "Включить звук",
        "Unmute",
        v_none,
        r_unmute,
        None,
        None,
    );
    add(
        "audio.media",
        Safe,
        "Медиа-клавиша: пауза, следующий, предыдущий",
        "Media key: play/pause, next, previous",
        v_media,
        r_media,
        None,
        None,
    );
    add(
        "files.open_folder",
        Safe,
        "Открыть папку или диск в Проводнике",
        "Open a folder or drive in Explorer",
        v_place,
        r_open_folder,
        None,
        None,
    );
    add(
        "files.list",
        Safe,
        "Показать содержимое папки",
        "List folder contents",
        v_place,
        r_list_dir,
        None,
        None,
    );
    add(
        "files.delete",
        Confirm,
        "Удалить файл в корзину",
        "Move a file to the Recycle Bin",
        v_delete,
        r_delete,
        None,
        Some(delete_summary),
    );
    add(
        "process.list",
        Safe,
        "Список процессов по CPU или RAM",
        "List processes by CPU or RAM",
        v_process_list,
        r_process_list,
        None,
        None,
    );
    add(
        "process.hung",
        Safe,
        "Зависшие окна",
        "Windows that are not responding",
        v_none,
        r_process_hung,
        None,
        None,
    );
    add(
        "process.kill",
        Confirm,
        "Завершить процесс по имени или PID",
        "Terminate a process by name or PID",
        v_kill,
        r_process_kill,
        Some(kill_risk),
        Some(kill_summary),
    );
    add(
        "network.check",
        Safe,
        "Проверка интернета и адаптеров",
        "Check internet and adapters",
        v_none,
        r_network,
        None,
        None,
    );
    add(
        "power.lock",
        Safe,
        "Заблокировать компьютер",
        "Lock the computer",
        v_none,
        r_lock,
        None,
        None,
    );
    add(
        "power.sleep",
        Confirm,
        "Спящий режим",
        "Sleep",
        v_none,
        r_sleep,
        None,
        Some(|_| "Put the computer to sleep".into()),
    );
    add(
        "power.hibernate",
        Confirm,
        "Гибернация",
        "Hibernate",
        v_none,
        r_hibernate,
        None,
        Some(|_| "Hibernate the computer".into()),
    );
    add(
        "power.shutdown",
        Confirm,
        "Выключить компьютер через 30 секунд",
        "Shut down in 30 seconds",
        v_none,
        r_shutdown,
        None,
        Some(|_| "Shut down the computer in 30 seconds".into()),
    );
    add(
        "power.restart",
        Confirm,
        "Перезагрузить компьютер через 30 секунд",
        "Restart in 30 seconds",
        v_none,
        r_restart,
        None,
        Some(|_| "Restart the computer in 30 seconds".into()),
    );
    add(
        "power.abort",
        Safe,
        "Отменить запланированное выключение",
        "Cancel a scheduled shutdown",
        v_none,
        r_abort,
        None,
        None,
    );
    add(
        "info.time",
        Safe,
        "Текущее время",
        "Current time",
        v_none,
        r_time,
        None,
        None,
    );
    add(
        "info.date",
        Safe,
        "Текущая дата",
        "Current date",
        v_none,
        r_date,
        None,
        None,
    );
    registry
}
