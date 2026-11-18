//! Glue between the brain (`jarvis-brain`) and the shell: the `ai.*`, `personality.*` and
//! `privacy.*` settings, the brain's construction, and the data the AI, PRIVACY and MEMORY tabs show.

use crate::{error::CommandError, settings, state::AppState};
use jarvis_brain::{
    config::{self, BrainConfig, CloudConfig},
    extra_actions, ActionPort, Brain, CloudLlm, KeyReader, Llm, Memory, Mode, Ollama, OllamaLlm,
    SharedConfig, SystemSnapshot,
};
use jarvis_control::Action;
use jarvis_core::Database;
use jarvis_tts::{SecretStore, AI_CLOUD_KEY};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Longest list the MEMORY tab asks for.
pub const MAX_LIST: usize = 500;

/// Reads the brain settings. Missing or invalid values fall back to the defaults.
pub async fn load_config(db: &Database) -> BrainConfig {
    let read = |key: &'static str| {
        let db = db.clone();
        async move {
            let value = db.settings_get(key.to_owned()).await.ok().flatten()?;
            settings::validate(key, &value).ok()?;
            Some(value)
        }
    };
    let mut config = BrainConfig::default();
    if let Some(value) = read(settings::AI_ENABLED).await.and_then(|v| v.as_bool()) {
        config.enabled = value;
    }
    if let Some(value) = read(settings::AI_MODEL).await {
        if let Some(model) = value.as_str() {
            config.model = model.to_owned();
        }
    }
    let number = |value: Option<Value>| value.and_then(|v| v.as_u64()).map(|n| n as u32);
    if let Some(value) = number(read(settings::AI_KEEP_ALIVE).await) {
        config.keep_alive_minutes = value;
    }
    if let Some(value) = number(read(settings::AI_NUM_CTX).await) {
        config.num_ctx = value;
    }
    if let Some(value) = number(read(settings::AI_NUM_THREAD).await) {
        config.num_thread = value;
    }
    let mut cloud = CloudConfig::default();
    if let Some(value) = read(settings::AI_CLOUD_ENABLED)
        .await
        .and_then(|v| v.as_bool())
    {
        cloud.enabled = value;
    }
    if let Some(value) = read(settings::AI_CLOUD_BASE_URL).await {
        cloud.base_url = value.as_str().unwrap_or_default().to_owned();
    }
    if let Some(value) = read(settings::AI_CLOUD_MODEL).await {
        cloud.model = value.as_str().unwrap_or_default().to_owned();
    }
    config.cloud = cloud;
    if let Some(mode) = read(settings::PERSONALITY_MODE)
        .await
        .and_then(|v| v.as_str().and_then(Mode::parse))
    {
        config.mode = mode;
    }
    if let Some(value) = read(settings::PERSONALITY_CUSTOM).await {
        config.custom_instructions = value.as_str().unwrap_or_default().to_owned();
    }
    if let Some(value) = read(settings::PRIVACY_HISTORY_ENABLED)
        .await
        .and_then(|v| v.as_bool())
    {
        config.history_enabled = value;
    }
    if let Some(value) = number(read(settings::PRIVACY_HISTORY_DAYS).await) {
        config.history_days = value;
    }
    config
}

/// What the registry needs before the brain can exist (the brain needs the executor holder).
pub struct Pieces {
    ollama: Arc<Ollama>,
    config: SharedConfig,
    memory: Arc<Memory>,
    db: Database,
}

/// Environment variable that points the app at another local Ollama address.
pub const OLLAMA_URL_ENV: &str = "JARVIS_OLLAMA_URL";

impl Pieces {
    pub fn new(db: Database, config: BrainConfig) -> Result<Self, CommandError> {
        // `JARVIS_OLLAMA_URL` (tests: a closed port stands for "Ollama is stopped"); only local URLs pass.
        let url = std::env::var(OLLAMA_URL_ENV)
            .unwrap_or_else(|_| jarvis_brain::DEFAULT_OLLAMA_URL.to_owned());
        let ollama =
            Arc::new(Ollama::new(&url).map_err(|error| CommandError::System(error.to_string()))?);
        let config = config::shared(config);
        let memory = Arc::new(Memory::new(db.clone(), Arc::clone(&config)));
        Ok(Self {
            ollama,
            config,
            memory,
            db,
        })
    }

    /// The Ollama client and the shared brain settings (the vision actions pick their model from them).
    pub fn shared(&self) -> (Arc<Ollama>, SharedConfig) {
        (Arc::clone(&self.ollama), Arc::clone(&self.config))
    }

    /// `memory.*` and `ai.unload`, registered next to the built-in actions.
    pub fn extras(&self) -> Vec<Arc<dyn Action>> {
        extra_actions(&self.db, &self.ollama, &self.config)
    }

    pub fn finish(
        self,
        port: Arc<dyn ActionPort>,
        system: Arc<dyn SystemSnapshot>,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Arc<AiParts>, CommandError> {
        let local: Arc<dyn Llm> = Arc::new(OllamaLlm::new(
            Arc::clone(&self.ollama),
            Arc::clone(&self.config),
        ));
        let key: KeyReader = Arc::new(move || {
            secrets
                .get(AI_CLOUD_KEY)
                .ok()
                .flatten()
                .map(|secret| secret.expose().to_owned())
        });
        let cloud: Arc<dyn Llm> = Arc::new(
            CloudLlm::new(Arc::clone(&self.config), key)
                .map_err(|error| CommandError::System(error.to_string()))?,
        );
        let brain = Arc::new(Brain::new(
            local,
            Some(cloud),
            port,
            Arc::clone(&self.memory),
            system,
            Arc::clone(&self.config),
        ));
        Ok(Arc::new(AiParts {
            ollama: self.ollama,
            config: self.config,
            memory: self.memory,
            brain,
        }))
    }
}

/// The brain and what the AI tabs need.
pub struct AiParts {
    pub ollama: Arc<Ollama>,
    pub config: SharedConfig,
    pub memory: Arc<Memory>,
    pub brain: Arc<Brain>,
}

/// Reloads the brain after one of its settings changed.
pub async fn apply_setting(app: &AppHandle, key: &str) {
    let prefixes = ["ai.", "personality.", "privacy."];
    if !prefixes.iter().any(|prefix| key.starts_with(prefix)) {
        return;
    }
    let state = app.state::<AppState>();
    let fresh = load_config(&state.db).await;
    {
        let mut guard = state
            .ai
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = fresh;
    }
    if key == settings::PRIVACY_HISTORY_DAYS {
        state.ai.memory.purge().await;
    }
}

// ---------- what the tabs show ----------

#[derive(Debug, Serialize, PartialEq)]
pub struct ModelDto {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LoadedDto {
    pub name: String,
    pub size_bytes: u64,
    pub vram_bytes: u64,
    pub expires_at: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AiStatus {
    pub enabled: bool,
    pub running: bool,
    pub version: Option<String>,
    pub model: String,
    pub models: Vec<ModelDto>,
    pub loaded: Vec<LoadedDto>,
}

/// Status of the Ollama server. Never starts it.
pub async fn status(ai: &AiParts) -> AiStatus {
    let config = config::current(&ai.config);
    let version = ai.ollama.version().await.ok();
    let (models, loaded) = if version.is_some() {
        (
            ai.ollama.models().await.unwrap_or_default(),
            ai.ollama.loaded().await.unwrap_or_default(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    AiStatus {
        enabled: config.enabled,
        running: version.is_some(),
        version,
        model: config.model,
        models: models
            .into_iter()
            .map(|m| ModelDto {
                name: m.name,
                size_bytes: m.size_bytes,
            })
            .collect(),
        loaded: loaded
            .into_iter()
            .map(|m| LoadedDto {
                name: m.name,
                size_bytes: m.size_bytes,
                vram_bytes: m.vram_bytes,
                expires_at: m.expires_at,
            })
            .collect(),
    }
}

/// The "Unload model" button: frees whatever Ollama holds for the chosen model.
pub async fn unload(ai: &AiParts) -> Result<(), CommandError> {
    let model = config::current(&ai.config).model;
    ai.ollama
        .unload(&model)
        .await
        .map_err(|error| CommandError::Unavailable(error.to_string()))
}

#[derive(Debug, Serialize, PartialEq)]
pub struct FactDto {
    pub id: i64,
    pub ts: String,
    pub kind: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct HistoryDto {
    pub id: i64,
    pub ts: String,
    pub text: String,
    pub lang: String,
    pub route: String,
    pub action: Option<String>,
    pub ok: bool,
}

/// Longest search text the memory commands accept.
pub const MAX_QUERY_CHARS: usize = 200;

fn checked_query(query: Option<String>) -> Result<Option<String>, CommandError> {
    match query {
        Some(text) if text.chars().count() > MAX_QUERY_CHARS => Err(CommandError::Validation(
            format!("query is longer than {MAX_QUERY_CHARS} characters"),
        )),
        other => Ok(other),
    }
}

pub async fn facts(
    db: &Database,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<FactDto>, CommandError> {
    let query = checked_query(query)?;
    Ok(db
        .fact_list(query, limit.clamp(1, MAX_LIST))
        .await?
        .into_iter()
        .map(|f| FactDto {
            id: f.id,
            ts: f.ts,
            kind: f.kind,
            key: f.key,
            value: f.value,
        })
        .collect())
}

pub async fn history(
    db: &Database,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<HistoryDto>, CommandError> {
    let query = checked_query(query)?;
    Ok(db
        .history_list(query, limit.clamp(1, MAX_LIST))
        .await?
        .into_iter()
        .map(|h| HistoryDto {
            id: h.id,
            ts: h.ts,
            text: h.text,
            lang: h.lang,
            route: h.route,
            action: h.action,
            ok: h.ok,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tauri::async_runtime::block_on;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path().join("jarvis.db")).expect("open");
        (dir, db)
    }

    #[test]
    fn overlong_search_text_is_refused() {
        let (_dir, db) = temp_db();
        let long = "a".repeat(MAX_QUERY_CHARS + 1);
        let error = block_on(facts(&db, Some(long.clone()), 10)).expect_err("too long");
        assert_eq!(error.code(), "VALIDATION");
        assert!(block_on(history(&db, Some(long), 10)).is_err());
        assert!(block_on(facts(&db, Some("ok".into()), 10)).is_ok());
    }

    #[test]
    fn defaults_apply_until_settings_are_stored() {
        let (_dir, db) = temp_db();
        let config = block_on(load_config(&db));
        assert_eq!(config, BrainConfig::default());
        assert!(!config.cloud.enabled);
    }

    #[test]
    fn stored_settings_are_read_and_invalid_ones_ignored() {
        let (_dir, db) = temp_db();
        block_on(async {
            for (key, value) in [
                (settings::AI_MODEL, json!("gemma3:4b")),
                (settings::AI_KEEP_ALIVE, json!(10)),
                (settings::AI_NUM_CTX, json!(2048)),
                (settings::PERSONALITY_MODE, json!("formal")),
                (settings::PRIVACY_HISTORY_ENABLED, json!(false)),
                (settings::PRIVACY_HISTORY_DAYS, json!(7)),
                (settings::AI_CLOUD_ENABLED, json!(true)),
                (
                    settings::AI_CLOUD_BASE_URL,
                    json!("https://api.example.com/v1"),
                ),
                // invalid values written behind the validator's back are ignored
                (settings::AI_NUM_THREAD, json!(64)),
                (settings::PERSONALITY_CUSTOM, json!("я".repeat(2500))),
            ] {
                db.settings_set(key.into(), value).await.expect("set");
            }
            let config = load_config(&db).await;
            assert_eq!(config.model, "gemma3:4b");
            assert_eq!(config.keep_alive_minutes, 10);
            assert_eq!(config.num_ctx, 2048);
            assert_eq!(config.mode, Mode::Formal);
            assert!(!config.history_enabled);
            assert_eq!(config.history_days, 7);
            assert!(config.cloud.enabled);
            assert_eq!(config.cloud.base_url, "https://api.example.com/v1");
            assert_eq!(config.num_thread, 6);
            assert!(config.custom_instructions.is_empty());
        });
    }

    #[test]
    fn the_tab_lists_search_and_delete_through_the_database() {
        let (_dir, db) = temp_db();
        block_on(async {
            db.fact_upsert("project".into(), "мой проект".into(), "C:\\x".into())
                .await
                .expect("fact");
            let found = facts(&db, Some("проект".into()), 10).await.expect("facts");
            assert_eq!(found.len(), 1);
            assert!(db.fact_delete(found[0].id).await.expect("delete"));
            assert!(facts(&db, None, 10).await.expect("facts").is_empty());
            assert!(history(&db, None, 10).await.expect("history").is_empty());
        });
    }
}
