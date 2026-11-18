//! Brain settings (the `ai.*`, `personality.*` and `privacy.*` keys), their limits and validation.

use crate::{error::BrainError, personality::Mode};
use std::sync::{Arc, RwLock};

/// Default model; chosen by the 30-phrase evaluation.
pub const DEFAULT_MODEL: &str = "qwen3:4b";
pub const DEFAULT_KEEP_ALIVE_MINUTES: u32 = 5;
pub const MAX_KEEP_ALIVE_MINUTES: u32 = 1440;
pub const MAX_NUM_CTX: u32 = 4096;
pub const MIN_NUM_CTX: u32 = 1024;
pub const MAX_NUM_THREAD: u32 = 6;
pub const MAX_CUSTOM_CHARS: usize = 2000;
pub const MAX_HISTORY_DAYS: u32 = 365;
pub const DEFAULT_HISTORY_DAYS: u32 = 30;
pub const MAX_URL_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainConfig {
    pub enabled: bool,
    pub model: String,
    pub keep_alive_minutes: u32,
    pub num_ctx: u32,
    pub num_thread: u32,
    pub mode: Mode,
    pub custom_instructions: String,
    pub cloud: CloudConfig,
    pub history_enabled: bool,
    pub history_days: u32,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: DEFAULT_MODEL.to_owned(),
            keep_alive_minutes: DEFAULT_KEEP_ALIVE_MINUTES,
            num_ctx: MAX_NUM_CTX,
            num_thread: MAX_NUM_THREAD,
            mode: Mode::Classic,
            custom_instructions: String::new(),
            cloud: CloudConfig::default(),
            history_enabled: true,
            history_days: DEFAULT_HISTORY_DAYS,
        }
    }
}

/// The configuration shared by the router, the Ollama client and the memory actions.
pub type SharedConfig = Arc<RwLock<BrainConfig>>;

pub fn shared(config: BrainConfig) -> SharedConfig {
    Arc::new(RwLock::new(config))
}

/// Reads the current configuration (a poisoned lock still holds a usable value).
pub fn current(config: &SharedConfig) -> BrainConfig {
    config
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// `keep_alive` as Ollama wants it: `"5m"`, or `0` to unload at once.
pub fn keep_alive_value(minutes: u32) -> serde_json::Value {
    if minutes == 0 {
        serde_json::json!(0)
    } else {
        serde_json::json!(format!("{minutes}m"))
    }
}

/// An Ollama model tag: letters, digits and `. _ : / -`, up to 100 characters.
pub fn validate_model_name(name: &str) -> Result<(), BrainError> {
    let ok = !name.is_empty()
        && name.chars().count() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'));
    if ok {
        Ok(())
    } else {
        Err(BrainError::Validation(
            "expected a model tag such as qwen3:4b".into(),
        ))
    }
}

/// Host part of an `http(s)://host[:port]/...` URL.
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    if let Some(stripped) = authority.strip_prefix('[') {
        return stripped.split(']').next();
    }
    authority.split(':').next().filter(|host| !host.is_empty())
}

pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// A loopback URL (Ollama is only ever reached on this machine).
pub fn is_local_url(url: &str) -> bool {
    url.starts_with("http://") && host_of(url).is_some_and(is_loopback_host)
}

/// The cloud endpoint must be HTTPS and carry no credentials. Empty means "not set".
pub fn validate_cloud_url(url: &str) -> Result<(), BrainError> {
    if url.is_empty() {
        return Ok(());
    }
    let ok = url.chars().count() <= MAX_URL_CHARS
        && url.starts_with("https://")
        && host_of(url).is_some()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(BrainError::Validation(
            "expected an https:// URL without credentials".into(),
        ))
    }
}

pub fn validate_custom(text: &str) -> Result<(), BrainError> {
    if text.chars().count() <= MAX_CUSTOM_CHARS {
        Ok(())
    } else {
        Err(BrainError::Validation(format!(
            "custom instructions are limited to {MAX_CUSTOM_CHARS} characters"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_within_the_spec() {
        let config = BrainConfig::default();
        assert!(config.num_ctx <= 4096 && config.num_thread <= 6);
        assert_eq!(config.keep_alive_minutes, 5);
        assert!(!config.cloud.enabled);
        assert!(config.history_enabled);
        assert_eq!(config.history_days, 30);
        assert_eq!(config.mode, Mode::Classic);
    }

    #[test]
    fn keep_alive_formats() {
        assert_eq!(keep_alive_value(5), serde_json::json!("5m"));
        assert_eq!(keep_alive_value(0), serde_json::json!(0));
    }

    #[test]
    fn model_names() {
        assert!(validate_model_name("qwen3:4b").is_ok());
        assert!(validate_model_name("hf.co/org/model-GGUF:Q4_K_M").is_ok());
        for bad in ["", "a b", "x;rm", &"m".repeat(101)] {
            assert!(validate_model_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls() {
        assert!(is_local_url("http://127.0.0.1:11434"));
        assert!(is_local_url("http://localhost:1/x"));
        assert!(is_local_url("http://[::1]:11434"));
        assert!(!is_local_url("http://example.com:11434"));
        assert!(!is_local_url("https://127.0.0.1"));
        assert!(!is_local_url("http://127.0.0.1@evil.com"));
        assert!(validate_cloud_url("").is_ok());
        assert!(validate_cloud_url("https://api.example.com/v1").is_ok());
        for bad in [
            "http://api.example.com",
            "https://user:pw@api.example.com",
            "https://",
            "ftp://x",
            "https://a b.com",
        ] {
            assert!(validate_cloud_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn custom_instructions_are_limited() {
        assert!(validate_custom(&"я".repeat(2000)).is_ok());
        assert!(validate_custom(&"я".repeat(2001)).is_err());
    }
}
