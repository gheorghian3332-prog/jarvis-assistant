//! J.A.R.V.I.S. local AI brain: an Ollama client, the hybrid router (FAST PATH, local model,
//! optional cloud), the personality modes and the assistant's memory.
//!
//! Security rule: the model only ever proposes an intent. Every action goes through
//! [`ActionPort::run`], which in the app is `Executor::run`.

pub mod config;
pub mod error;
pub mod llm;
pub mod memory;
pub mod personality;
pub mod router;
pub mod specs;
pub mod system;
pub mod voice_rules;

pub use config::{BrainConfig, CloudConfig, SharedConfig, DEFAULT_MODEL};
pub use error::{BrainError, BrainResult};
pub use llm::{
    find_ollama, ChatMessage, CloudLlm, KeyReader, Llm, LlmReply, LlmRequest, LoadedModel,
    ModelInfo, Ollama, OllamaLlm, DEFAULT_OLLAMA_URL,
};
pub use memory::{extra_actions, Memory, EXTRA_ACTION_NAMES};
pub use personality::Mode;
pub use router::{parse_answer, ActionPort, Answer, Brain, Note, Route, Routed};
pub use system::{GpuState, MonitorSnapshot, NoSnapshot, SystemSnapshot, SystemState};
