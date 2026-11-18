//! Audit abstraction so the executor can be tested without a database.
//! P6-B wires it to `jarvis_core::Database::audit_append`.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    pub action: String,
    pub args: Value,
    pub risk: String,
    pub ok: bool,
    pub detail: String,
}

#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn append(&self, record: AuditRecord);
}

/// Discards everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullAudit;

#[async_trait]
impl AuditSink for NullAudit {
    async fn append(&self, _record: AuditRecord) {}
}

/// Keeps records in memory; used by tests and by `jarvisctl`.
#[derive(Debug, Default)]
pub struct MemoryAudit {
    records: Mutex<Vec<AuditRecord>>,
}

impl MemoryAudit {
    pub fn records(&self) -> Vec<AuditRecord> {
        self.records
            .lock()
            .map(|records| records.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl AuditSink for MemoryAudit {
    async fn append(&self, record: AuditRecord) {
        if let Ok(mut records) = self.records.lock() {
            records.push(record);
        }
    }
}
