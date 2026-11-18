use std::time::Duration;
use thiserror::Error;

/// Errors of the speech output. Messages never contain secrets.
#[derive(Debug, Error)]
pub enum TtsError {
    /// The engine cannot work at all right now (missing files, no key, no network).
    #[error("{engine} is unavailable: {reason}")]
    Unavailable {
        engine: &'static str,
        reason: String,
    },
    /// The engine is there but this request failed.
    #[error("{engine} failed: {reason}")]
    Synthesis {
        engine: &'static str,
        reason: String,
    },
    #[error("{engine} did not answer within {limit:?}")]
    Timeout {
        engine: &'static str,
        limit: Duration,
    },
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("audio output: {0}")]
    Playback(String),
    #[error("secret store: {0}")]
    Secret(String),
}

pub type TtsResult<T> = Result<T, TtsError>;

impl TtsError {
    pub fn unavailable(engine: &'static str, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            engine,
            reason: reason.into(),
        }
    }

    pub fn synthesis(engine: &'static str, reason: impl Into<String>) -> Self {
        Self::Synthesis {
            engine,
            reason: reason.into(),
        }
    }
}
