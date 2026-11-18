use thiserror::Error;

/// Why the language model could not answer.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BrainError {
    /// The Ollama server is not running and could not be started (FAST PATH keeps working).
    #[error("UNAVAILABLE: {0}")]
    Unavailable(String),
    #[error("TIMEOUT: {0}")]
    Timeout(String),
    /// The server answered, but not with something usable.
    #[error("BAD_RESPONSE: {0}")]
    BadResponse(String),
    #[error("VALIDATION: {0}")]
    Validation(String),
}

impl BrainError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "UNAVAILABLE",
            Self::Timeout(_) => "TIMEOUT",
            Self::BadResponse(_) => "BAD_RESPONSE",
            Self::Validation(_) => "VALIDATION",
        }
    }
}

pub type BrainResult<T> = Result<T, BrainError>;
