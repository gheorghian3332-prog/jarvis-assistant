//! The assistant's memory as the brain uses it: facts for the prompt, the command history, and the
//! registry actions through which the model (or a voice phrase) may change memory.
//!
//! The model never touches the database: `memory.remember` (safe) and `memory.forget` (asks for
//! confirmation) are ordinary registry actions, so they go through `Executor::run` like any other.

use crate::config::{current, SharedConfig};
use crate::llm::Ollama;
use async_trait::async_trait;
use jarvis_control::{replies::Reply, Action, ActionResult, ControlError, ControlResult, Risk};
use jarvis_core::{Database, Fact, NewHistory};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const KINDS: [&str; 5] = ["preference", "project", "device", "scenario", "note"];
pub const MAX_KEY_CHARS: usize = 60;
pub const MAX_VALUE_CHARS: usize = 300;
/// Facts that go into one prompt.
pub const PROMPT_FACTS: usize = 10;
const PURGE_EVERY: Duration = Duration::from_secs(3600);

/// Facts and history on top of the database, honoring the privacy settings.
pub struct Memory {
    db: Database,
    config: SharedConfig,
    last_purge: Mutex<Option<Instant>>,
}

impl Memory {
    pub fn new(db: Database, config: SharedConfig) -> Self {
        Self {
            db,
            config,
            last_purge: Mutex::new(None),
        }
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    /// Facts relevant to the phrase, for the prompt (at most [`PROMPT_FACTS`]).
    pub async fn facts_for(&self, text: &str) -> Vec<Fact> {
        match self.db.fact_relevant(text.to_owned(), PROMPT_FACTS).await {
            Ok(facts) => facts,
            Err(error) => {
                tracing::warn!(%error, "memory facts could not be read");
                Vec::new()
            }
        }
    }

    /// Writes one history row unless history is switched off; drops rows past the retention
    /// period about once an hour.
    pub async fn record(&self, entry: NewHistory) {
        let config = current(&self.config);
        if !config.history_enabled {
            return;
        }
        if let Err(error) = self.db.history_add(entry).await {
            tracing::warn!(%error, "history row was not written");
            return;
        }
        let due = {
            let mut last = self
                .last_purge
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let due = last.is_none_or(|at| at.elapsed() >= PURGE_EVERY);
            if due {
                *last = Some(Instant::now());
            }
            due
        };
        if due {
            self.purge().await;
        }
    }

    /// Applies the retention period now (also at startup).
    pub async fn purge(&self) {
        let days = current(&self.config).history_days;
        match self.db.history_purge(days).await {
            Ok(count) if count > 0 => tracing::info!(count, "old history rows deleted"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "history purge failed"),
        }
    }
}

/// How a fact is shown to the model and to the user.
pub fn fact_line(fact: &Fact) -> String {
    format!("{} / {}: {}", fact.kind, fact.key, fact.value)
}

fn clean_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

// ---------- arguments ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberArgs {
    kind: String,
    key: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgetArgs {
    id: i64,
    /// What the fact says, for the confirmation prompt only.
    preview: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecallArgs {
    query: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn parse_args<T: serde::de::DeserializeOwned>(value: &Value) -> ControlResult<T> {
    let value = if value.is_null() {
        json!({})
    } else {
        value.clone()
    };
    serde_json::from_value(value).map_err(|error| ControlError::validation(error.to_string()))
}

fn check_text(what: &str, text: &str, max: usize) -> ControlResult<()> {
    let length = text.trim().chars().count();
    if length == 0 || length > max {
        return Err(ControlError::validation(format!(
            "{what} must be 1 to {max} characters"
        )));
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ControlError::validation(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(())
}

fn db_error(error: &jarvis_core::CoreError) -> ControlError {
    ControlError::system(format!("memory: {error}"))
}

// ---------- memory.remember ----------

pub struct Remember {
    db: Database,
}

#[async_trait]
impl Action for Remember {
    fn name(&self) -> &'static str {
        "memory.remember"
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn description_ru(&self) -> &'static str {
        "Запомнить факт о пользователе (предпочтение, проект, устройство, сценарий)"
    }
    fn description_en(&self) -> &'static str {
        "Remember a fact about the user (preference, project, device, scenario, note)"
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        let parsed: RememberArgs = parse_args(args)?;
        if !KINDS.contains(&parsed.kind.as_str()) {
            return Err(ControlError::validation(format!(
                "kind must be one of {}",
                KINDS.join(", ")
            )));
        }
        check_text("key", &parsed.key, MAX_KEY_CHARS)?;
        check_text("value", &parsed.value, MAX_VALUE_CHARS)
    }
    fn summary(&self, args: &Value) -> String {
        format!("Remember {args}")
    }
    async fn run(&self, args: Value) -> ControlResult<ActionResult> {
        let parsed: RememberArgs = parse_args(&args)?;
        let kind = parsed.kind;
        let key = clean_text(&parsed.key);
        let value = clean_text(&parsed.value);
        let stored = self
            .db
            .fact_upsert(kind.clone(), key.clone(), value.clone())
            .await
            .map_err(|error| db_error(&error))?;
        let Some(id) = stored else {
            return Err(ControlError::unavailable(
                "memory is full: forget something first",
            ));
        };
        Ok(ActionResult::new(
            Reply::new("Запомнил.", "Noted."),
            json!({"id": id, "kind": kind, "key": key, "value": value}),
        ))
    }
}

// ---------- memory.forget ----------

pub struct Forget {
    db: Database,
}

#[async_trait]
impl Action for Forget {
    fn name(&self) -> &'static str {
        "memory.forget"
    }
    fn risk(&self) -> Risk {
        Risk::Confirm
    }
    fn description_ru(&self) -> &'static str {
        "Забыть запомненный факт по номеру"
    }
    fn description_en(&self) -> &'static str {
        "Forget a remembered fact by id"
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        let parsed: ForgetArgs = parse_args(args)?;
        if parsed.id < 1 {
            return Err(ControlError::validation("id must be positive"));
        }
        if let Some(preview) = &parsed.preview {
            if preview.chars().count() > 2 * MAX_VALUE_CHARS {
                return Err(ControlError::validation("preview is too long"));
            }
        }
        Ok(())
    }
    fn summary(&self, args: &Value) -> String {
        match parse_args::<ForgetArgs>(args) {
            Ok(ForgetArgs {
                preview: Some(preview),
                ..
            }) => format!("Forget: {}", clean_text(&preview)),
            Ok(ForgetArgs { id, .. }) => format!("Forget the fact number {id}"),
            Err(_) => "Forget a fact".into(),
        }
    }
    async fn run(&self, args: Value) -> ControlResult<ActionResult> {
        let parsed: ForgetArgs = parse_args(&args)?;
        let removed = self
            .db
            .fact_delete(parsed.id)
            .await
            .map_err(|error| db_error(&error))?;
        if !removed {
            return Err(ControlError::not_found(format!(
                "no remembered fact with id {}",
                parsed.id
            )));
        }
        Ok(ActionResult::new(
            Reply::new("Забыл.", "Forgotten."),
            json!({"id": parsed.id}),
        ))
    }
}

// ---------- memory.recall ----------

pub struct Recall {
    db: Database,
}

#[async_trait]
impl Action for Recall {
    fn name(&self) -> &'static str {
        "memory.recall"
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn description_ru(&self) -> &'static str {
        "Перечислить, что запомнено о пользователе"
    }
    fn description_en(&self) -> &'static str {
        "List what is remembered about the user"
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        parse_args::<RecallArgs>(args).map(|_| ())
    }
    async fn run(&self, args: Value) -> ControlResult<ActionResult> {
        let parsed: RecallArgs = parse_args(&args)?;
        let facts = self
            .db
            .fact_list(parsed.query, 20)
            .await
            .map_err(|error| db_error(&error))?;
        let detail = json!({"facts": facts.iter().map(|f| json!({
            "id": f.id, "kind": f.kind, "key": f.key, "value": f.value
        })).collect::<Vec<_>>()});
        if facts.is_empty() {
            return Ok(ActionResult::new(
                Reply::new(
                    "Пока я ничего о вас не помню.",
                    "I do not remember anything about you yet.",
                ),
                detail,
            ));
        }
        let shown: Vec<String> = facts
            .iter()
            .take(5)
            .map(|f| format!("{}: {}", f.key, f.value))
            .collect();
        let more = facts.len().saturating_sub(5);
        let tail_ru = if more > 0 {
            format!(" И ещё {more}.")
        } else {
            String::new()
        };
        let tail_en = if more > 0 {
            format!(" And {more} more.")
        } else {
            String::new()
        };
        Ok(ActionResult::new(
            Reply::new(
                format!("Я помню: {}.{tail_ru}", shown.join("; ")),
                format!("I remember: {}.{tail_en}", shown.join("; ")),
            ),
            detail,
        ))
    }
}

// ---------- ai.unload ----------

/// "Unload the model": frees the memory Ollama holds for it.
pub struct Unload {
    ollama: Arc<Ollama>,
    config: SharedConfig,
}

#[async_trait]
impl Action for Unload {
    fn name(&self) -> &'static str {
        "ai.unload"
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn description_ru(&self) -> &'static str {
        "Выгрузить языковую модель из памяти"
    }
    fn description_en(&self) -> &'static str {
        "Unload the language model from memory"
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        parse_args::<NoArgs>(args).map(|_| ())
    }
    async fn run(&self, _args: Value) -> ControlResult<ActionResult> {
        let model = current(&self.config).model;
        // A server that is not running holds no model: nothing to free.
        if self.ollama.version().await.is_err() {
            return Ok(ActionResult::new(
                Reply::new("Модель не загружена.", "No model is loaded."),
                json!({"model": model, "was_loaded": false}),
            ));
        }
        let loaded = self
            .ollama
            .loaded()
            .await
            .map_err(|error| ControlError::unavailable(error.to_string()))?;
        let was_loaded = loaded.iter().any(|m| m.name == model);
        self.ollama
            .unload(&model)
            .await
            .map_err(|error| ControlError::unavailable(error.to_string()))?;
        Ok(ActionResult::new(
            Reply::new("Модель выгружена.", "Model unloaded."),
            json!({"model": model, "was_loaded": was_loaded}),
        ))
    }
}

/// Names of the actions the brain adds to the registry.
pub const EXTRA_ACTION_NAMES: [&str; 4] = [
    "memory.remember",
    "memory.forget",
    "memory.recall",
    "ai.unload",
];

/// The brain's registry actions; the app registers them next to the built-ins.
pub fn extra_actions(
    db: &Database,
    ollama: &Arc<Ollama>,
    config: &SharedConfig,
) -> Vec<Arc<dyn Action>> {
    vec![
        Arc::new(Remember { db: db.clone() }),
        Arc::new(Forget { db: db.clone() }),
        Arc::new(Recall { db: db.clone() }),
        Arc::new(Unload {
            ollama: Arc::clone(ollama),
            config: Arc::clone(config),
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{shared, BrainConfig};

    fn temp_db() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().expect("tempdir");
        let db = Database::open(directory.path().join("t.db")).expect("db");
        (directory, db)
    }

    fn actions(db: &Database) -> Vec<Arc<dyn Action>> {
        let ollama = Arc::new(Ollama::new("http://127.0.0.1:1").expect("client"));
        extra_actions(db, &ollama, &shared(BrainConfig::default()))
    }

    fn by_name(list: &[Arc<dyn Action>], name: &str) -> Arc<dyn Action> {
        list.iter()
            .find(|a| a.name() == name)
            .cloned()
            .expect("action")
    }

    #[test]
    fn the_extra_actions_match_the_published_names() {
        let (_dir, db) = temp_db();
        let names: Vec<&str> = actions(&db).iter().map(|a| a.name()).collect();
        assert_eq!(names, EXTRA_ACTION_NAMES);
        for name in EXTRA_ACTION_NAMES {
            assert!(crate::specs::args_of(name).is_some(), "{name}");
        }
    }

    #[test]
    fn risks_follow_the_spec() {
        let (_dir, db) = temp_db();
        let list = actions(&db);
        assert_eq!(by_name(&list, "memory.remember").risk(), Risk::Safe);
        assert_eq!(by_name(&list, "memory.forget").risk(), Risk::Confirm);
    }

    #[test]
    fn remember_rejects_bad_arguments() {
        let (_dir, db) = temp_db();
        let remember = by_name(&actions(&db), "memory.remember");
        assert!(remember
            .validate(&json!({"kind": "project", "key": "путь", "value": r"E:\x"}))
            .is_ok());
        for bad in [
            json!({"kind": "secret", "key": "a", "value": "b"}),
            json!({"kind": "note", "key": "", "value": "b"}),
            json!({"kind": "note", "key": "a", "value": ""}),
            json!({"kind": "note", "key": "a".repeat(61), "value": "b"}),
            json!({"kind": "note", "key": "a", "value": "b".repeat(301)}),
            json!({"kind": "note", "key": "a", "value": "b", "extra": 1}),
            json!({"kind": "note", "key": "a\u{0}", "value": "b"}),
            json!({"kind": "note", "key": "a"}),
            json!(null),
        ] {
            assert!(remember.validate(&bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn remember_recall_forget_round_trip() {
        let (_dir, db) = temp_db();
        let list = actions(&db);
        let remember = by_name(&list, "memory.remember");
        let result = remember
            .run(json!({"kind": "project", "key": "мой проект", "value": r"C:\Users\example\jarvis"}))
            .await
            .expect("remember");
        let id = result.detail["id"].as_i64().expect("id");
        let recall = by_name(&list, "memory.recall");
        let said = recall.run(json!({})).await.expect("recall");
        assert!(
            said.reply_ru.contains(r"C:\Users\example\jarvis"),
            "{}",
            said.reply_ru
        );
        let forget = by_name(&list, "memory.forget");
        assert!(forget
            .summary(&json!({"id": id, "preview": "мой проект"}))
            .contains("мой проект"));
        forget.run(json!({"id": id})).await.expect("forget");
        assert!(forget.run(json!({"id": id})).await.is_err());
        let empty = recall.run(json!({})).await.expect("recall");
        assert!(empty.reply_ru.contains("ничего"));
    }

    #[tokio::test]
    async fn history_respects_the_privacy_switch() {
        let (_dir, db) = temp_db();
        let config = shared(BrainConfig::default());
        let memory = Memory::new(db.clone(), config.clone());
        let entry = || NewHistory {
            text: "привет".into(),
            lang: "ru".into(),
            route: "llm".into(),
            action: None,
            ok: true,
        };
        memory.record(entry()).await;
        assert_eq!(db.history_list(None, 10).await.expect("list").len(), 1);
        config.write().expect("lock").history_enabled = false;
        memory.record(entry()).await;
        assert_eq!(db.history_list(None, 10).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn purge_uses_the_configured_retention() {
        let (_dir, db) = temp_db();
        let config = shared(BrainConfig::default());
        let memory = Memory::new(db.clone(), config.clone());
        let old = db
            .history_add(NewHistory {
                text: "старая".into(),
                lang: "ru".into(),
                route: "fast".into(),
                action: None,
                ok: true,
            })
            .await
            .expect("add");
        db.history_backdate(old, 10).await.expect("backdate");
        config.write().expect("lock").history_days = 30;
        memory.purge().await;
        assert_eq!(db.history_list(None, 10).await.expect("list").len(), 1);
        config.write().expect("lock").history_days = 7;
        memory.purge().await;
        assert!(db.history_list(None, 10).await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn unload_without_a_server_is_a_calm_no_op() {
        let (_dir, db) = temp_db();
        let unload = by_name(&actions(&db), "ai.unload");
        let result = unload.run(json!({})).await.expect("unload");
        assert_eq!(result.detail["was_loaded"], json!(false));
    }
}
