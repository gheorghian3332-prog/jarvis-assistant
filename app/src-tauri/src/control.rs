//! Glue between the control engine (`jarvis-control`) and the shell: the `control.*` settings and
//! their validation, the executor holder, the audit sink, the command service on the bus and the
//! emergency-stop hotkey.

use crate::{
    dispatch::{
        ActionReport, AssistantMessage, CommandCore, ConfirmRequest, Engine, Shell, Source,
    },
    error::CommandError,
    settings,
    state::{lock, AppState},
};
use async_trait::async_trait;
use jarvis_control::{
    actions::{build_registry, Context},
    config::default_allowed_roots,
    Action, ActionInfo, AuditRecord, AuditSink, ControlConfig, Executor, Intent, Outcome,
    CONFIRM_TTL,
};
use jarvis_core::{CoreResult, Database, Event, EventBus, Service};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf, Prefix},
    sync::{atomic::AtomicBool, Arc, Mutex, OnceLock, RwLock},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::broadcast::error::RecvError;

pub const MAX_ALIASES: usize = 50;
pub const MAX_ROOTS: usize = 20;
pub const MAX_ALIAS_NAME: usize = 40;
pub const MAX_PATH_CHARS: usize = 260;
pub const DEFAULT_EMERGENCY_KEY: &str = "Ctrl+Alt+Pause";
/// Longest text `chat_send` accepts.
pub const MAX_CHAT_CHARS: usize = 500;

// ---------- validation of the `control.*` settings ----------

fn invalid(key: &str, message: impl std::fmt::Display) -> CommandError {
    CommandError::Validation(format!("{key}: {message}"))
}

/// Names of every registered action (a registry built with defaults; nothing is executed).
pub fn known_actions() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let context = Arc::new(Context::new(
            ControlConfig::default(),
            Arc::new(AtomicBool::new(false)),
        ));
        let mut names: Vec<String> = build_registry(context)
            .list()
            .into_iter()
            .map(|info| info.name)
            .collect();
        names.extend(
            crate::insight::ACTION_NAMES
                .iter()
                .map(|name| (*name).to_owned()),
        );
        names.extend(
            jarvis_brain::EXTRA_ACTION_NAMES
                .iter()
                .map(|name| (*name).to_owned()),
        );
        names
    })
}

pub fn parse_disabled(key: &str, value: &Value) -> Result<Vec<String>, CommandError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(key, "expected a list of action names"))?;
    let mut names = Vec::with_capacity(items.len());
    for item in items {
        let name = item
            .as_str()
            .ok_or_else(|| invalid(key, "expected a list of action names"))?;
        if !known_actions().iter().any(|known| known == name) {
            return Err(invalid(key, format!("unknown action \"{name}\"")));
        }
        if names.iter().any(|seen| seen == name) {
            return Err(invalid(key, format!("\"{name}\" is listed twice")));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

/// Lower-cased path with `\` separators and no trailing separator, for prefix comparisons.
fn comparable(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    if let Some(stripped) = text.strip_prefix("\\\\?\\") {
        text = stripped.to_owned();
    }
    while text.len() > 3 && text.ends_with('\\') {
        text.pop();
    }
    text
}

/// True when `path` is `root` or lies inside it (case-insensitive, whole components).
fn is_within(path: &Path, root: &Path) -> bool {
    let (path, root) = (comparable(path), comparable(root));
    path == root || path.starts_with(&format!("{}\\", root.trim_end_matches('\\')))
}

/// An absolute path on a drive: `X:\...`, no `..`, no UNC / device (`\\server`, `\\?\`) forms.
fn drive_path(text: &str) -> Result<PathBuf, String> {
    if text.is_empty() || text.chars().count() > MAX_PATH_CHARS || text.contains('\0') {
        return Err("expected a path of up to 260 characters".into());
    }
    if text.starts_with("\\\\") || text.starts_with("//") {
        return Err("network and device paths (UNC) are not allowed".into());
    }
    let path = Path::new(text);
    let mut components = path.components();
    let on_drive = matches!(
        components.next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
    ) && matches!(components.next(), Some(Component::RootDir));
    if !on_drive {
        return Err("expected an absolute path on a drive, like D:\\Games".into());
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("\"..\" and \".\" are not allowed in a path".into());
    }
    Ok(path.to_path_buf())
}

/// Folders that must never be a file root: the system drive root and the Windows / Program Files trees.
fn system_locations() -> Vec<PathBuf> {
    let mut places = Vec::new();
    if let Some(drive) = std::env::var_os("SystemDrive") {
        places.push(PathBuf::from(format!("{}\\", drive.to_string_lossy())));
    }
    for name in [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ] {
        if let Some(value) = std::env::var_os(name) {
            places.push(PathBuf::from(value));
        }
    }
    places
}

fn check_root(text: &str) -> Result<PathBuf, String> {
    let path = drive_path(text)?;
    if !path.is_dir() {
        return Err(format!("\"{text}\" is not an existing folder"));
    }
    let system = system_locations();
    let is_system_drive_root = system.first().is_some_and(|root| {
        // `C:\` exactly: its sub-folders are judged below.
        comparable(&path) == comparable(root)
    });
    let in_system_tree = system
        .iter()
        .skip(1)
        .any(|location| is_within(&path, location));
    if is_system_drive_root || in_system_tree {
        return Err(format!("\"{text}\" is a system location"));
    }
    Ok(path)
}

pub fn parse_roots(key: &str, value: &Value) -> Result<Vec<PathBuf>, CommandError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(key, "expected a list of folders"))?;
    if items.len() > MAX_ROOTS {
        return Err(invalid(key, format!("at most {MAX_ROOTS} folders")));
    }
    let mut roots: Vec<PathBuf> = Vec::with_capacity(items.len());
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| invalid(key, "expected a list of folders"))?;
        let path = check_root(text).map_err(|message| invalid(key, message))?;
        if roots
            .iter()
            .any(|seen| comparable(seen) == comparable(&path))
        {
            return Err(invalid(key, format!("\"{text}\" is listed twice")));
        }
        roots.push(path);
    }
    Ok(roots)
}

const STEAM_PREFIX: &str = "steam://rungameid/";

fn check_alias_target(text: &str) -> Result<(), String> {
    if let Some(id) = text.strip_prefix(STEAM_PREFIX) {
        return if (1..=12).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit()) {
            Ok(())
        } else {
            Err("a Steam link looks like steam://rungameid/<digits>".into())
        };
    }
    if text.contains("://") || text.contains(':') && !text.chars().nth(1).is_some_and(|c| c == ':')
    {
        return Err("only a file path or steam://rungameid/<digits> is allowed".into());
    }
    let path = drive_path(text)?;
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase());
    if !matches!(extension.as_deref(), Some("exe" | "lnk")) {
        return Err("expected an .exe or .lnk file".into());
    }
    if !path.is_file() {
        return Err(format!("\"{text}\" is not an existing file"));
    }
    // Resolve junctions and links: the real file must lie inside the profile or a non-system drive.
    let real = std::fs::canonicalize(&path).map_err(|error| format!("\"{text}\": {error}"))?;
    let allowed = default_allowed_roots();
    if !allowed.iter().any(|root| is_within(&real, root)) {
        return Err(format!(
            "\"{text}\" must be inside the user profile or on a non-system drive"
        ));
    }
    Ok(())
}

pub fn parse_aliases(key: &str, value: &Value) -> Result<HashMap<String, String>, CommandError> {
    let map = value
        .as_object()
        .ok_or_else(|| invalid(key, "expected an object {name: target}"))?;
    if map.len() > MAX_ALIASES {
        return Err(invalid(key, format!("at most {MAX_ALIASES} aliases")));
    }
    let mut aliases = HashMap::with_capacity(map.len());
    for (name, target) in map {
        let trimmed = name.trim();
        if trimmed.is_empty()
            || trimmed != name
            || name.chars().count() > MAX_ALIAS_NAME
            || name.chars().any(char::is_control)
        {
            return Err(invalid(
                key,
                format!("alias name \"{name}\" must be 1 to {MAX_ALIAS_NAME} characters, no edge spaces"),
            ));
        }
        let target = target
            .as_str()
            .ok_or_else(|| invalid(key, format!("the target of \"{name}\" must be a string")))?;
        check_alias_target(target).map_err(|message| invalid(key, format!("{name}: {message}")))?;
        aliases.insert(name.clone(), target.to_owned());
    }
    Ok(aliases)
}

pub fn parse_emergency_key(key: &str, value: &Value) -> Result<Shortcut, CommandError> {
    value
        .as_str()
        .and_then(|text| text.parse::<Shortcut>().ok())
        .ok_or_else(|| invalid(key, "not a valid key, for example Ctrl+Alt+Pause"))
}

/// Validates any `control.*` value.
pub fn validate(key: &str, value: &Value) -> Result<(), CommandError> {
    match key {
        settings::CONTROL_DISABLED_ACTIONS => parse_disabled(key, value).map(|_| ()),
        settings::CONTROL_FILE_ROOTS => parse_roots(key, value).map(|_| ()),
        settings::CONTROL_APP_ALIASES => parse_aliases(key, value).map(|_| ()),
        settings::CONTROL_EMERGENCY_KEY => parse_emergency_key(key, value).map(|_| ()),
        _ => Err(invalid(key, "not a known setting")),
    }
}

/// Reads the `control.*` settings. Values are validated again (a folder may have disappeared since).
pub async fn load_config(db: &Database) -> ControlConfig {
    let read = |key: &'static str| {
        let db = db.clone();
        async move { db.settings_get(key.to_owned()).await.ok().flatten() }
    };
    let mut config = ControlConfig::default();
    if let Some(names) = read(settings::CONTROL_DISABLED_ACTIONS)
        .await
        .and_then(|v| parse_disabled(settings::CONTROL_DISABLED_ACTIONS, &v).ok())
    {
        config.disabled_actions = names.into_iter().collect::<HashSet<_>>();
    }
    if let Some(roots) = read(settings::CONTROL_FILE_ROOTS)
        .await
        .and_then(|v| parse_roots(settings::CONTROL_FILE_ROOTS, &v).ok())
    {
        config.allowed_roots = roots;
    }
    if let Some(aliases) = read(settings::CONTROL_APP_ALIASES)
        .await
        .and_then(|v| parse_aliases(settings::CONTROL_APP_ALIASES, &v).ok())
    {
        config.app_aliases = aliases;
    }
    config
}

/// The stored emergency-stop key (default `Ctrl+Alt+Pause`).
pub async fn load_emergency_key(db: &Database) -> Shortcut {
    let stored = db
        .settings_get(settings::CONTROL_EMERGENCY_KEY.to_owned())
        .await
        .ok()
        .flatten();
    stored
        .and_then(|value| parse_emergency_key(settings::CONTROL_EMERGENCY_KEY, &value).ok())
        .or_else(|| DEFAULT_EMERGENCY_KEY.parse().ok())
        .unwrap_or_else(|| {
            Shortcut::new(
                Some(
                    tauri_plugin_global_shortcut::Modifiers::CONTROL
                        | tauri_plugin_global_shortcut::Modifiers::ALT,
                ),
                tauri_plugin_global_shortcut::Code::Pause,
            )
        })
}

// ---------- audit ----------

/// Writes the executor's audit records into the `audit_log` table.
pub struct DbAudit(pub Database);

#[async_trait]
impl AuditSink for DbAudit {
    async fn append(&self, record: AuditRecord) {
        let action = record.action.clone();
        if let Err(error) = self
            .0
            .audit_append(
                record.action,
                record.args,
                record.risk,
                record.ok,
                record.detail,
            )
            .await
        {
            tracing::error!(%error, action, "audit record was not written");
        }
    }
}

// ---------- the executor holder ----------

/// Holds the current executor. Roots and aliases are fixed inside an executor, so changing them
/// builds a new one (the shutdown flag is shared, so a scheduled shutdown stays cancellable).
pub struct ControlHub {
    executor: RwLock<Arc<Executor>>,
    config: Mutex<ControlConfig>,
    shutdown_flag: Arc<AtomicBool>,
    audit: Arc<dyn AuditSink>,
    /// Actions of the brain (`memory.*`, `ai.unload`), kept to rebuild the executor.
    extras: Vec<Arc<dyn Action>>,
}

fn build_executor(
    config: &ControlConfig,
    flag: &Arc<AtomicBool>,
    audit: &Arc<dyn AuditSink>,
    extras: &[Arc<dyn Action>],
) -> Arc<Executor> {
    let context = Arc::new(Context::new(config.clone(), Arc::clone(flag)));
    let mut registry = build_registry(context);
    for action in extras {
        registry.register(Arc::clone(action));
    }
    // `build_registry` knows the switched-off list only for its own actions; apply it to all.
    registry.set_disabled(config.disabled_actions.clone());
    Arc::new(Executor::with_options(
        registry,
        Arc::clone(audit),
        CONFIRM_TTL,
        Arc::clone(flag),
    ))
}

impl ControlHub {
    pub fn new(
        config: ControlConfig,
        audit: Arc<dyn AuditSink>,
        extras: Vec<Arc<dyn Action>>,
    ) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        Self {
            executor: RwLock::new(build_executor(&config, &flag, &audit, &extras)),
            config: Mutex::new(config),
            shutdown_flag: flag,
            audit,
            extras,
        }
    }

    fn current(&self) -> Arc<Executor> {
        match self.executor.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Applies new settings. Returns true when the executor was rebuilt (pending confirmations are gone).
    pub fn apply(&self, config: ControlConfig) -> bool {
        let mut current = lock(&self.config);
        let same_world = current.allowed_roots == config.allowed_roots
            && current.app_aliases == config.app_aliases;
        if same_world {
            self.current()
                .registry()
                .set_disabled(config.disabled_actions.clone());
            *current = config;
            return false;
        }
        let rebuilt = build_executor(&config, &self.shutdown_flag, &self.audit, &self.extras);
        match self.executor.write() {
            Ok(mut guard) => *guard = rebuilt,
            Err(poisoned) => *poisoned.into_inner() = rebuilt,
        }
        *current = config;
        true
    }

    pub fn actions(&self) -> Vec<ActionInfo> {
        self.current().registry().list()
    }
}

#[async_trait]
impl Engine for ControlHub {
    async fn run(&self, intent: Intent) -> Outcome {
        self.current().run(intent).await
    }

    async fn confirm(&self, action_id: &str, approved: bool) -> Outcome {
        self.current().confirm(action_id, approved).await
    }

    fn emergency_stop(&self) -> usize {
        self.current().emergency_stop()
    }
}

/// The brain proposes intents through the same executor: `Executor::run`, nothing else.
#[async_trait]
impl jarvis_brain::ActionPort for ControlHub {
    async fn run(&self, intent: Intent) -> Outcome {
        self.current().run(intent).await
    }

    fn actions(&self) -> Vec<ActionInfo> {
        ControlHub::actions(self)
    }
}

// ---------- the shell: window events, speech, state ----------

/// Connects the core to the window (events), the voice front (state) and the speech output.
pub struct AppShell {
    app: AppHandle,
}

impl AppShell {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
        if let Err(error) = self.app.emit(event, payload) {
            tracing::debug!(%error, event, "event was not delivered");
        }
    }
}

impl Shell for AppShell {
    fn set_state(&self, state: &str) {
        self.app.state::<AppState>().voice.show_state(state);
    }

    fn state(&self) -> String {
        self.app.state::<AppState>().voice.snapshot().state
    }

    fn speak(&self, text: &str) {
        if let Err(error) = self.app.state::<AppState>().tts.speak(text) {
            tracing::warn!(%error, "the reply could not be spoken");
        }
    }

    fn message(&self, message: &AssistantMessage) {
        self.emit("assistant://message", message);
    }

    fn confirm_request(&self, request: &ConfirmRequest) {
        self.emit("confirm://request", request);
    }

    fn action_result(&self, report: &ActionReport) {
        self.emit("action://result", report);
    }

    fn silence(&self) {
        let state = self.app.state::<AppState>();
        state.tts.stop();
        // Releasing the key ends a recording that push-to-talk started.
        state.ptt.send(false);
    }
}

// ---------- the service on the bus ----------

/// Feeds every `Event::Transcript` to the command core.
pub struct CommandService {
    bus: EventBus,
    core: Arc<CommandCore>,
}

impl CommandService {
    pub fn new(bus: EventBus, core: Arc<CommandCore>) -> Arc<Self> {
        Arc::new(Self { bus, core })
    }

    async fn run(&self) {
        let mut receiver = self.bus.subscribe();
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Event::Transcript { text, lang, .. } = &*event {
                        let core = Arc::clone(&self.core);
                        let (text, lang) = (text.to_string(), lang.to_string());
                        // Phrases queue up inside the core; the bus loop never waits for an action.
                        tokio::spawn(async move {
                            core.submit(&text, Some(&lang), Source::Voice).await;
                        });
                    }
                }
                // Level events may have been skipped; a transcript is rare and tiny.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    }
}

#[async_trait]
impl Service for CommandService {
    fn name(&self) -> &'static str {
        "commands"
    }

    async fn start(&self) -> CoreResult<()> {
        self.run().await;
        std::future::pending().await
    }

    async fn stop(&self) -> CoreResult<()> {
        Ok(())
    }

    fn critical(&self) -> bool {
        false
    }
}

// ---------- hotkey and settings hooks ----------

/// Registers `shortcut` as the emergency-stop key, replacing the previous one.
pub fn register_emergency(app: &AppHandle, shortcut: Shortcut) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let mut current = lock(&state.emergency_shortcut);
    if *current == Some(shortcut) {
        return Ok(());
    }
    let handle = app.clone();
    let shortcuts = app.global_shortcut();
    shortcuts
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                let core = Arc::clone(&handle.state::<AppState>().commands);
                tauri::async_runtime::spawn(async move { core.emergency_stop().await });
            }
        })
        .map_err(|error| CommandError::System(format!("emergency stop key: {error}")))?;
    if let Some(previous) = current.replace(shortcut) {
        if let Err(error) = shortcuts.unregister(previous) {
            tracing::warn!(%error, "could not release the previous emergency stop key");
        }
    }
    Ok(())
}

/// Applies a stored `control.*` setting that was just changed.
pub async fn apply_setting(app: &AppHandle, key: &str) -> Result<(), CommandError> {
    if !key.starts_with("control.") {
        return Ok(());
    }
    let state = app.state::<AppState>();
    if key == settings::CONTROL_EMERGENCY_KEY {
        let shortcut = load_emergency_key(&state.db).await;
        return register_emergency(app, shortcut);
    }
    let config = load_config(&state.db).await;
    if key != settings::CONTROL_DISABLED_ACTIONS {
        // A new executor forgets what it was asked: close the dialog first.
        state.commands.cancel_pending("settings changed").await;
    }
    state.control.apply(config);
    Ok(())
}

/// `control_actions`: what the TOOLS tab shows.
#[derive(Debug, Serialize)]
pub struct ControlInfo {
    pub actions: Vec<ActionInfo>,
    pub default_roots: Vec<String>,
}

pub fn control_info(hub: &ControlHub) -> ControlInfo {
    ControlInfo {
        actions: hub.actions(),
        default_roots: default_allowed_roots()
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn every_registered_action_can_be_switched_off() {
        let names = known_actions();
        assert!(names.iter().any(|name| name == "power.shutdown"));
        assert!(names.len() >= 20);
        let all = json!(names);
        assert!(validate(settings::CONTROL_DISABLED_ACTIONS, &all).is_ok());
    }

    #[test]
    fn disabled_actions_reject_unknown_and_malformed_values() {
        let key = settings::CONTROL_DISABLED_ACTIONS;
        assert!(validate(key, &json!([])).is_ok());
        assert!(validate(key, &json!(["power.shutdown"])).is_ok());
        for bad in [
            json!(["nope.nothing"]),
            json!(["power.shutdown", "power.shutdown"]),
            json!([1]),
            json!("power.shutdown"),
            json!(null),
        ] {
            assert_eq!(
                validate(key, &bad).expect_err("invalid").code(),
                "VALIDATION"
            );
        }
    }

    #[test]
    fn roots_accept_existing_folders_on_a_drive() {
        let dir = temp_dir();
        let path = dir.path().to_string_lossy().into_owned();
        assert!(validate(settings::CONTROL_FILE_ROOTS, &json!([path])).is_ok());
        assert!(validate(settings::CONTROL_FILE_ROOTS, &json!([])).is_ok());
    }

    #[test]
    fn roots_reject_everything_else() {
        let dir = temp_dir();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").expect("write");
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let dotdot = format!("{}\\..", dir.path().display());
        for bad in [
            json!(["relative\\folder"]),
            json!(["\\Users"]),
            json!(["\\\\server\\share"]),
            json!(["//server/share"]),
            json!(["\\\\?\\C:\\Users"]),
            json!([dotdot]),
            json!(["Z:\\definitely\\not\\here"]),
            json!([file.to_string_lossy()]),
            json!(["C:\\"]),
            json!([windows]),
            json!([dir.path().to_string_lossy(), dir.path().to_string_lossy()]),
            json!([""]),
            json!([5]),
            json!("D:\\"),
        ] {
            assert_eq!(
                validate(settings::CONTROL_FILE_ROOTS, &bad)
                    .expect_err(&format!("{bad}"))
                    .code(),
                "VALIDATION",
                "{bad}"
            );
        }
        let many: Vec<String> = (0..=MAX_ROOTS)
            .map(|_| dir.path().to_string_lossy().into_owned())
            .collect();
        assert!(validate(settings::CONTROL_FILE_ROOTS, &json!(many)).is_err());
    }

    #[test]
    fn alias_targets_accept_steam_links_and_files_inside_the_profile() {
        let dir = temp_dir();
        let exe = dir.path().join("tool.exe");
        let lnk = dir.path().join("tool.LNK");
        std::fs::write(&exe, "x").expect("write");
        std::fs::write(&lnk, "x").expect("write");
        let key = settings::CONTROL_APP_ALIASES;
        let value = json!({
            "инструмент": exe.to_string_lossy(),
            "tool two": lnk.to_string_lossy(),
            "игра": "steam://rungameid/730",
        });
        assert!(validate(key, &value).is_ok());
        assert!(validate(key, &json!({})).is_ok());
    }

    #[test]
    fn alias_targets_reject_anything_launchable_that_is_not_allowed() {
        let dir = temp_dir();
        let txt = dir.path().join("note.txt");
        let bat = dir.path().join("run.bat");
        std::fs::write(&txt, "x").expect("write");
        std::fs::write(&bat, "x").expect("write");
        let key = settings::CONTROL_APP_ALIASES;
        let system = format!(
            "{}\\System32\\notepad.exe",
            std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
        );
        for target in [
            "steam://rungameid/",
            "steam://rungameid/12a",
            "steam://rungameid/1/../2",
            "steam://run/730",
            "steam://rungameid/1234567890123",
            "https://example.com",
            "ms-settings:",
            "shell:AppsFolder\\x!y",
            "cmd /c calc",
            "notepad.exe",
            "\\\\server\\share\\a.exe",
            "Z:\\missing\\a.exe",
            &txt.to_string_lossy(),
            &bat.to_string_lossy(),
            &system,
            "",
        ] {
            assert_eq!(
                validate(key, &json!({"x": target}))
                    .expect_err(target)
                    .code(),
                "VALIDATION",
                "{target}"
            );
        }
    }

    #[test]
    fn alias_names_and_counts_are_limited() {
        let key = settings::CONTROL_APP_ALIASES;
        for name in ["", " x", "x ", &"n".repeat(MAX_ALIAS_NAME + 1), "a\nb"] {
            assert!(
                validate(key, &json!({ name: "steam://rungameid/1" })).is_err(),
                "{name:?}"
            );
        }
        let mut many = serde_json::Map::new();
        for index in 0..=MAX_ALIASES {
            many.insert(format!("game {index}"), json!("steam://rungameid/1"));
        }
        assert!(validate(key, &Value::Object(many.clone())).is_err());
        many.remove("game 0");
        assert!(validate(key, &Value::Object(many)).is_ok());
        assert!(validate(key, &json!(["x"])).is_err());
        assert!(validate(key, &json!({"x": 5})).is_err());
    }

    #[test]
    fn the_emergency_key_must_be_a_shortcut() {
        let key = settings::CONTROL_EMERGENCY_KEY;
        assert!(validate(key, &json!("Ctrl+Alt+Pause")).is_ok());
        assert!(validate(key, &json!("F12")).is_ok());
        for bad in [json!("NotAKey"), json!(""), json!(3), json!(null)] {
            assert!(validate(key, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn within_compares_whole_components_case_insensitively() {
        assert!(is_within(
            Path::new("D:\\Games\\a.exe"),
            Path::new("d:\\games")
        ));
        assert!(is_within(Path::new("D:\\Games"), Path::new("D:\\Games\\")));
        assert!(!is_within(
            Path::new("D:\\GamesExtra\\a.exe"),
            Path::new("D:\\Games")
        ));
        assert!(is_within(Path::new("\\\\?\\D:\\x\\y"), Path::new("D:\\")));
    }

    #[test]
    fn the_hub_applies_switch_offs_without_rebuilding() {
        let hub = ControlHub::new(
            ControlConfig::default(),
            Arc::new(jarvis_control::NullAudit),
            Vec::new(),
        );
        let mut config = ControlConfig::default();
        config.disabled_actions.insert("power.shutdown".into());
        assert!(!hub.apply(config.clone()));
        let info = hub.actions();
        let shutdown = info.iter().find(|a| a.name == "power.shutdown");
        assert_eq!(shutdown.map(|a| a.enabled), Some(false));
        config
            .app_aliases
            .insert("x".into(), "steam://rungameid/1".into());
        assert!(hub.apply(config));
        let shutdown = hub
            .actions()
            .into_iter()
            .find(|a| a.name == "power.shutdown");
        assert_eq!(
            shutdown.map(|a| a.enabled),
            Some(false),
            "the switch-off survives a rebuild"
        );
    }

    #[derive(Default)]
    struct RecShell {
        state: Mutex<String>,
        spoken: Mutex<Vec<String>>,
        requests: Mutex<Vec<ConfirmRequest>>,
    }

    impl Shell for RecShell {
        fn set_state(&self, state: &str) {
            *lock(&self.state) = state.to_owned();
        }
        fn state(&self) -> String {
            lock(&self.state).clone()
        }
        fn speak(&self, text: &str) {
            lock(&self.spoken).push(text.to_owned());
        }
        fn message(&self, _message: &AssistantMessage) {}
        fn confirm_request(&self, request: &ConfirmRequest) {
            lock(&self.requests).push(request.clone());
        }
        fn action_result(&self, _report: &ActionReport) {}
        fn silence(&self) {}
    }

    /// A spoken "shut down" through the bus and the real executor: the request is raised, a spoken
    /// "no" declines it, nothing is shut down and the audit says so.
    #[test]
    fn a_spoken_refusal_goes_bus_to_executor_to_audit() {
        tauri::async_runtime::block_on(async {
            let audit = Arc::new(jarvis_control::MemoryAudit::default());
            let hub = Arc::new(ControlHub::new(
                ControlConfig::default(),
                audit.clone(),
                Vec::new(),
            ));
            let shell = Arc::new(RecShell::default());
            let core = CommandCore::new(hub.clone(), shell.clone(), audit.clone());
            let bus = EventBus::new();
            let service = CommandService::new(bus.clone(), core.clone());
            let task = tokio::spawn(async move { service.run().await });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let say = |text: &str| {
                bus.publish(Event::Transcript {
                    text: text.into(),
                    lang: "ru".into(),
                    source: jarvis_core::UtteranceSource::Wake,
                })
                .expect("subscribed");
            };
            say("выключи компьютер");
            for _ in 0..100 {
                if core.pending_id().is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert!(core.pending_id().is_some(), "a confirmation must be raised");
            assert_eq!(lock(&shell.requests)[0].action, "power.shutdown");
            say("лёха, нет");
            for _ in 0..100 {
                if core.pending_id().is_none() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            task.abort();
            assert!(!hub.shutdown_flag.load(std::sync::atomic::Ordering::SeqCst));
            let records = audit.records();
            assert!(
                records.iter().any(|r| r.action == "power.shutdown"
                    && !r.ok
                    && r.detail.starts_with("declined")),
                "{records:?}"
            );
            assert_eq!(
                lock(&shell.spoken).last().map(String::as_str),
                Some("Отменено.")
            );
        });
    }

    #[test]
    fn a_disabled_action_is_denied_by_the_executor() {
        tauri::async_runtime::block_on(async {
            let mut config = ControlConfig::default();
            config.disabled_actions.insert("info.time".into());
            let hub = ControlHub::new(config, Arc::new(jarvis_control::NullAudit), Vec::new());
            let outcome = hub.run(Intent::new("info.time", json!({}))).await;
            assert!(matches!(outcome, Outcome::Denied { .. }), "{outcome:?}");
        });
    }
}
