use jarvis_core::CoreError;
use serde::ser::{Serialize, SerializeStruct, Serializer};

/// Error returned by every Tauri command, serialized as `{code, message}` (API_CONTRACTS, "UI ↔ core").
// Variants beyond Validation/System are part of the contract code set and used by later phases.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    Validation(String),
    Forbidden(String),
    NotFound(String),
    System(String),
    Timeout(String),
    Unavailable(String),
}

impl CommandError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "VALIDATION",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::System(_) => "SYSTEM",
            Self::Timeout(_) => "TIMEOUT",
            Self::Unavailable(_) => "UNAVAILABLE",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Validation(message)
            | Self::Forbidden(message)
            | Self::NotFound(message)
            | Self::System(message)
            | Self::Timeout(message)
            | Self::Unavailable(message) => message,
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for CommandError {}

impl Serialize for CommandError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("CommandError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", self.message())?;
        state.end()
    }
}

impl From<CoreError> for CommandError {
    fn from(error: CoreError) -> Self {
        Self::System(error.to_string())
    }
}

impl From<tauri::Error> for CommandError {
    fn from(error: tauri::Error) -> Self {
        Self::System(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_code_and_message() {
        let error = CommandError::Validation("bad".into());
        assert_eq!(
            serde_json::to_value(&error).expect("serialize"),
            serde_json::json!({"code": "VALIDATION", "message": "bad"})
        );
    }

    #[test]
    fn maps_every_code() {
        let cases = [
            (CommandError::Forbidden(String::new()), "FORBIDDEN"),
            (CommandError::NotFound(String::new()), "NOT_FOUND"),
            (CommandError::System(String::new()), "SYSTEM"),
            (CommandError::Timeout(String::new()), "TIMEOUT"),
            (CommandError::Unavailable(String::new()), "UNAVAILABLE"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
        }
    }

    #[test]
    fn core_errors_become_system_errors() {
        let error: CommandError = CoreError::Configuration("x".into()).into();
        assert_eq!(error.code(), "SYSTEM");
    }
}
