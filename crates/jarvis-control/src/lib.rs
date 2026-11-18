//! J.A.R.V.I.S. Windows control engine: action registry, WinAPI controllers and the rule-based
//! fast path. Integration with the app is P6-B; this crate has no dependency on `jarvis-core`
//! (audit goes through the [`AuditSink`] trait).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

pub mod action;
pub mod actions;
pub mod apps;
pub mod audio;
pub mod audit;
pub mod com;
pub mod config;
pub mod error;
pub mod executor;
pub mod files;
pub mod info;
pub mod network;
pub mod parse;
pub mod power;
pub mod procs;
pub mod registry;
pub mod replies;
pub mod text;
pub mod wnd;

pub use action::{Action, ActionResult, Intent, Lang, Outcome, Risk};
pub use audit::{AuditRecord, AuditSink, MemoryAudit, NullAudit};
pub use config::ControlConfig;
pub use error::{ControlError, ControlResult, ErrorCode};
pub use executor::{Executor, CONFIRM_TTL};
pub use parse::parse;
pub use registry::{ActionInfo, Registry};
pub use replies::{restyle, Style};

/// Executor wired to the real Windows controllers.
pub fn windows_executor(config: ControlConfig, audit: Arc<dyn AuditSink>) -> Executor {
    let flag = Arc::new(AtomicBool::new(false));
    let ctx = Arc::new(actions::Context::new(config, flag.clone()));
    Executor::with_options(actions::build_registry(ctx), audit, CONFIRM_TTL, flag)
}

/// Same as [`windows_executor`] with a custom confirmation lifetime (tests, tooling).
pub fn windows_executor_with_ttl(
    config: ControlConfig,
    audit: Arc<dyn AuditSink>,
    ttl: Duration,
) -> Executor {
    let flag = Arc::new(AtomicBool::new(false));
    let ctx = Arc::new(actions::Context::new(config, flag.clone()));
    Executor::with_options(actions::build_registry(ctx), audit, ttl, flag)
}
