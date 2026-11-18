//! Error type shared by all controllers. Codes follow `API_CONTRACTS.md`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Validation,
    Forbidden,
    NotFound,
    System,
    Timeout,
    Unavailable,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Validation => "VALIDATION",
            ErrorCode::Forbidden => "FORBIDDEN",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::System => "SYSTEM",
            ErrorCode::Timeout => "TIMEOUT",
            ErrorCode::Unavailable => "UNAVAILABLE",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
}

pub type ControlResult<T> = Result<T, ControlError>;

impl ControlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Validation, message)
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn system(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::System, message)
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }
}

impl From<windows::core::Error> for ControlError {
    fn from(error: windows::core::Error) -> Self {
        ControlError::system(format!("WinAPI: {error}"))
    }
}
