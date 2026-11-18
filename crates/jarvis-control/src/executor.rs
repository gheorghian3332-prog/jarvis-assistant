//! `Executor`: validation, risk gating, 30-second confirmations, audit and emergency stop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::action::{Action, Intent, Outcome, Risk};
use crate::audit::{AuditRecord, AuditSink};
use crate::error::ErrorCode;
use crate::registry::Registry;

/// How long a confirmation request stays valid.
pub const CONFIRM_TTL: Duration = Duration::from_secs(30);

struct Pending {
    intent: Intent,
    summary: String,
    expires_at: Instant,
}

pub struct Executor {
    registry: Registry,
    audit: Arc<dyn AuditSink>,
    pending: Mutex<HashMap<String, Pending>>,
    counter: AtomicU64,
    ttl: Duration,
    /// Set by the power actions when a shutdown/restart was scheduled.
    shutdown_scheduled: Arc<AtomicBool>,
}

impl Executor {
    pub fn new(registry: Registry, audit: Arc<dyn AuditSink>) -> Self {
        Self::with_options(
            registry,
            audit,
            CONFIRM_TTL,
            Arc::new(AtomicBool::new(false)),
        )
    }

    pub fn with_options(
        registry: Registry,
        audit: Arc<dyn AuditSink>,
        ttl: Duration,
        shutdown_scheduled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            registry,
            audit,
            pending: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(1),
            ttl,
            shutdown_scheduled,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().map(|map| map.len()).unwrap_or(0)
    }

    async fn log(&self, action: &str, intent: &Intent, risk: Risk, ok: bool, detail: String) {
        self.audit
            .append(AuditRecord {
                action: action.to_owned(),
                args: intent.args.clone(),
                risk: risk.as_str().to_owned(),
                ok,
                detail,
            })
            .await;
    }

    fn take_expired(&self) -> Vec<(String, Pending)> {
        let now = Instant::now();
        let Ok(mut map) = self.pending.lock() else {
            return Vec::new();
        };
        let expired: Vec<String> = map
            .iter()
            .filter(|(_, pending)| pending.expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|id| map.remove(&id).map(|pending| (id, pending)))
            .collect()
    }

    async fn audit_expired(&self) {
        for (_, pending) in self.take_expired() {
            let risk = self
                .registry
                .get(&pending.intent.action)
                .map_or(Risk::Confirm, |action| {
                    action.effective_risk(&pending.intent.args)
                });
            self.log(
                &pending.intent.action,
                &pending.intent,
                risk,
                false,
                "confirmation expired".into(),
            )
            .await;
        }
    }

    /// Validates, gates by risk and either executes, asks for confirmation or refuses.
    pub async fn run(&self, intent: Intent) -> Outcome {
        self.audit_expired().await;
        let Some(action) = self.registry.get(&intent.action) else {
            return Outcome::Failed {
                code: ErrorCode::NotFound.as_str().into(),
                message: format!("unknown action \"{}\"", intent.action),
            };
        };
        if !self.registry.is_enabled(&intent.action) {
            let reason = format!("action \"{}\" is switched off", intent.action);
            self.log(
                &intent.action,
                &intent,
                action.risk(),
                false,
                reason.clone(),
            )
            .await;
            return Outcome::Denied { reason };
        }
        if let Some(outcome) = self.check(&action, &intent).await {
            return outcome;
        }
        let risk = action.effective_risk(&intent.args);
        match risk {
            Risk::Forbidden => {
                let reason = format!(
                    "action \"{}\" is forbidden for these arguments",
                    intent.action
                );
                self.log(&intent.action, &intent, risk, false, reason.clone())
                    .await;
                Outcome::Denied { reason }
            }
            Risk::Confirm => {
                let summary = action.summary(&intent.args);
                let id = format!("act-{}", self.counter.fetch_add(1, Ordering::Relaxed));
                if let Ok(mut map) = self.pending.lock() {
                    map.insert(
                        id.clone(),
                        Pending {
                            intent,
                            summary: summary.clone(),
                            expires_at: Instant::now() + self.ttl,
                        },
                    );
                }
                Outcome::NeedsConfirm {
                    action_id: id,
                    summary,
                }
            }
            Risk::Safe => self.execute(&action, intent, risk).await,
        }
    }

    /// Validation step shared by `run` and `confirm`. `Some` = the request ends here.
    async fn check(&self, action: &Arc<dyn Action>, intent: &Intent) -> Option<Outcome> {
        match action.validate(&intent.args) {
            Ok(()) => None,
            Err(error) => {
                let risk = action.risk();
                self.log(&intent.action, intent, risk, false, error.to_string())
                    .await;
                if error.code == ErrorCode::Forbidden {
                    Some(Outcome::Denied {
                        reason: error.message,
                    })
                } else {
                    Some(Outcome::failed(&error))
                }
            }
        }
    }

    async fn execute(&self, action: &Arc<dyn Action>, intent: Intent, risk: Risk) -> Outcome {
        match action.run(intent.args.clone()).await {
            Ok(result) => {
                self.log(&intent.action, &intent, risk, true, result.reply_en.clone())
                    .await;
                Outcome::Done {
                    reply_ru: result.reply_ru,
                    reply_en: result.reply_en,
                    detail: result.detail,
                }
            }
            Err(error) => {
                self.log(&intent.action, &intent, risk, false, error.to_string())
                    .await;
                if error.code == ErrorCode::Forbidden {
                    Outcome::Denied {
                        reason: error.message,
                    }
                } else {
                    Outcome::failed(&error)
                }
            }
        }
    }

    /// Answers a `NeedsConfirm`. Unknown or already used ids fail; expired ones are denied.
    pub async fn confirm(&self, action_id: &str, approved: bool) -> Outcome {
        let pending = self
            .pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(action_id));
        let Some(pending) = pending else {
            return Outcome::Failed {
                code: ErrorCode::NotFound.as_str().into(),
                message: format!("no pending confirmation \"{action_id}\""),
            };
        };
        let intent = pending.intent;
        let Some(action) = self.registry.get(&intent.action) else {
            return Outcome::Failed {
                code: ErrorCode::NotFound.as_str().into(),
                message: format!("unknown action \"{}\"", intent.action),
            };
        };
        let risk = action.effective_risk(&intent.args);
        if pending.expires_at <= Instant::now() {
            let reason = "confirmation expired".to_owned();
            self.log(&intent.action, &intent, risk, false, reason.clone())
                .await;
            return Outcome::Denied { reason };
        }
        if !approved {
            let reason = format!("declined: {}", pending.summary);
            self.log(&intent.action, &intent, risk, false, reason.clone())
                .await;
            return Outcome::Denied { reason };
        }
        // Conditions may have changed while the user was thinking.
        if !self.registry.is_enabled(&intent.action) {
            let reason = format!("action \"{}\" is switched off", intent.action);
            self.log(&intent.action, &intent, risk, false, reason.clone())
                .await;
            return Outcome::Denied { reason };
        }
        if let Some(outcome) = self.check(&action, &intent).await {
            return outcome;
        }
        if risk == Risk::Forbidden {
            let reason = "forbidden".to_owned();
            self.log(&intent.action, &intent, risk, false, reason.clone())
                .await;
            return Outcome::Denied { reason };
        }
        self.execute(&action, intent, risk).await
    }

    /// Cancels every pending confirmation and any shutdown this executor scheduled.
    /// Returns the number of cancelled confirmations.
    pub fn emergency_stop(&self) -> usize {
        let cancelled = self
            .pending
            .lock()
            .map(|mut map| {
                let count = map.len();
                map.clear();
                count
            })
            .unwrap_or(0);
        if self.shutdown_scheduled.swap(false, Ordering::SeqCst) {
            let _ = crate::power::abort_shutdown();
        }
        cancelled
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown_scheduled.clone()
    }
}
