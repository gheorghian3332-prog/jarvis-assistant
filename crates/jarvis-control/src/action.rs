//! Core vocabulary: risk levels, intents, outcomes and the `Action` trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ControlError, ControlResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Confirm,
    Forbidden,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Safe => "safe",
            Risk::Confirm => "confirm",
            Risk::Forbidden => "forbidden",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Ru,
    En,
}

/// A parsed command: registry action name plus JSON arguments (typed by the action itself).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

impl Intent {
    pub fn new(action: impl Into<String>, args: Value) -> Self {
        Self {
            action: action.into(),
            args,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub reply_ru: String,
    pub reply_en: String,
    #[serde(default)]
    pub detail: Value,
}

impl ActionResult {
    pub fn new(reply: crate::replies::Reply, detail: Value) -> Self {
        Self {
            reply_ru: reply.ru,
            reply_en: reply.en,
            detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Done {
        reply_ru: String,
        reply_en: String,
        detail: Value,
    },
    NeedsConfirm {
        action_id: String,
        summary: String,
    },
    Denied {
        reason: String,
    },
    Failed {
        code: String,
        message: String,
    },
}

impl Outcome {
    pub fn failed(error: &ControlError) -> Self {
        Outcome::Failed {
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
        }
    }
}

/// A registry entry. `validate` runs before `run`, always.
#[async_trait]
pub trait Action: Send + Sync {
    fn name(&self) -> &'static str;
    fn risk(&self) -> Risk;
    fn description_ru(&self) -> &'static str;
    fn description_en(&self) -> &'static str;
    fn validate(&self, args: &Value) -> ControlResult<()>;
    /// Risk for concrete arguments (e.g. killing a protected process is `Forbidden`).
    fn effective_risk(&self, _args: &Value) -> Risk {
        self.risk()
    }
    /// Short human-readable description of what is about to happen (confirmation prompt).
    fn summary(&self, args: &Value) -> String {
        format!("{} {}", self.name(), args)
    }
    async fn run(&self, args: Value) -> ControlResult<ActionResult>;
}
