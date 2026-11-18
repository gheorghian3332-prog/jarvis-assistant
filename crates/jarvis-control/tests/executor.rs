//! Executor behaviour: risk gating, confirmations (30 s expiry), emergency stop, audit,
//! registry switches and the real registry's risk map. Nothing here touches the system.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jarvis_control::replies::Reply;
use jarvis_control::{
    windows_executor, Action, ActionResult, ControlConfig, ControlError, ControlResult, ErrorCode,
    Executor, Intent, MemoryAudit, Outcome, Registry, Risk,
};
use serde_json::{json, Value};

struct Fake {
    name: &'static str,
    risk: Risk,
    runs: Arc<AtomicUsize>,
    fail: bool,
}

#[async_trait]
impl Action for Fake {
    fn name(&self) -> &'static str {
        self.name
    }
    fn risk(&self) -> Risk {
        self.risk
    }
    fn description_ru(&self) -> &'static str {
        "тест"
    }
    fn description_en(&self) -> &'static str {
        "test"
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        match args.get("bad").and_then(Value::as_str) {
            Some("forbidden") => Err(ControlError::forbidden("nope")),
            Some(_) => Err(ControlError::validation("bad args")),
            None => Ok(()),
        }
    }
    fn effective_risk(&self, args: &Value) -> Risk {
        if args.get("escalate").is_some() {
            Risk::Forbidden
        } else {
            self.risk
        }
    }
    async fn run(&self, _args: Value) -> ControlResult<ActionResult> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(ControlError::system("boom"));
        }
        Ok(ActionResult::new(
            Reply::new("готово", "done"),
            json!({"ran": true}),
        ))
    }
}

struct Fixture {
    executor: Executor,
    audit: Arc<MemoryAudit>,
    safe_runs: Arc<AtomicUsize>,
    confirm_runs: Arc<AtomicUsize>,
}

fn fixture(ttl: Duration, disabled: &[&str]) -> Fixture {
    let safe_runs = Arc::new(AtomicUsize::new(0));
    let confirm_runs = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new(disabled.iter().map(|s| (*s).to_owned()).collect());
    registry.register(Arc::new(Fake {
        name: "t.safe",
        risk: Risk::Safe,
        runs: safe_runs.clone(),
        fail: false,
    }));
    registry.register(Arc::new(Fake {
        name: "t.confirm",
        risk: Risk::Confirm,
        runs: confirm_runs.clone(),
        fail: false,
    }));
    registry.register(Arc::new(Fake {
        name: "t.failing",
        risk: Risk::Safe,
        runs: Arc::new(AtomicUsize::new(0)),
        fail: true,
    }));
    let audit = Arc::new(MemoryAudit::default());
    let executor = Executor::with_options(
        registry,
        audit.clone(),
        ttl,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    Fixture {
        executor,
        audit,
        safe_runs,
        confirm_runs,
    }
}

fn intent(action: &str, args: Value) -> Intent {
    Intent::new(action, args)
}

#[tokio::test]
async fn safe_action_runs_immediately_and_is_audited() {
    let f = fixture(Duration::from_secs(30), &[]);
    let outcome = f.executor.run(intent("t.safe", json!({}))).await;
    assert!(matches!(outcome, Outcome::Done { ref reply_en, .. } if reply_en == "done"));
    assert_eq!(f.safe_runs.load(Ordering::SeqCst), 1);
    let records = f.audit.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].ok);
    assert_eq!(records[0].risk, "safe");
    assert_eq!(records[0].action, "t.safe");
}

#[tokio::test]
async fn confirm_action_waits_then_runs_once_on_approval() {
    let f = fixture(Duration::from_secs(30), &[]);
    let outcome = f.executor.run(intent("t.confirm", json!({}))).await;
    let Outcome::NeedsConfirm { action_id, .. } = outcome else {
        panic!("expected NeedsConfirm");
    };
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 0);
    assert_eq!(f.executor.pending_count(), 1);
    let done = f.executor.confirm(&action_id, true).await;
    assert!(matches!(done, Outcome::Done { .. }));
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 1);
    // The id is single-use.
    let again = f.executor.confirm(&action_id, true).await;
    assert!(matches!(again, Outcome::Failed { ref code, .. } if code == "NOT_FOUND"));
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn declined_confirmation_is_denied_and_audited() {
    let f = fixture(Duration::from_secs(30), &[]);
    let Outcome::NeedsConfirm { action_id, .. } =
        f.executor.run(intent("t.confirm", json!({}))).await
    else {
        panic!("expected NeedsConfirm");
    };
    let outcome = f.executor.confirm(&action_id, false).await;
    assert!(matches!(outcome, Outcome::Denied { .. }));
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 0);
    let records = f.audit.records();
    assert_eq!(records.len(), 1);
    assert!(!records[0].ok);
}

#[tokio::test]
async fn confirmation_expires() {
    let f = fixture(Duration::from_millis(40), &[]);
    let Outcome::NeedsConfirm { action_id, .. } =
        f.executor.run(intent("t.confirm", json!({}))).await
    else {
        panic!("expected NeedsConfirm");
    };
    tokio::time::sleep(Duration::from_millis(120)).await;
    let outcome = f.executor.confirm(&action_id, true).await;
    assert!(
        matches!(outcome, Outcome::Denied { ref reason } if reason.contains("expired")),
        "{outcome:?}"
    );
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 0);
    assert!(f
        .audit
        .records()
        .iter()
        .any(|r| r.detail.contains("expired") && !r.ok));
}

#[tokio::test]
async fn stale_requests_are_purged_and_audited_on_next_run() {
    let f = fixture(Duration::from_millis(30), &[]);
    let _ = f.executor.run(intent("t.confirm", json!({}))).await;
    tokio::time::sleep(Duration::from_millis(90)).await;
    let _ = f.executor.run(intent("t.safe", json!({}))).await;
    assert_eq!(f.executor.pending_count(), 0);
    assert!(f
        .audit
        .records()
        .iter()
        .any(|r| r.detail == "confirmation expired"));
}

#[tokio::test]
async fn default_confirmation_window_is_thirty_seconds() {
    assert_eq!(jarvis_control::CONFIRM_TTL, Duration::from_secs(30));
}

#[tokio::test]
async fn emergency_stop_cancels_pending_confirmations() {
    let f = fixture(Duration::from_secs(30), &[]);
    let Outcome::NeedsConfirm { action_id, .. } =
        f.executor.run(intent("t.confirm", json!({}))).await
    else {
        panic!("expected NeedsConfirm");
    };
    let _ = f.executor.run(intent("t.confirm", json!({}))).await;
    assert_eq!(f.executor.emergency_stop(), 2);
    assert_eq!(f.executor.pending_count(), 0);
    let outcome = f.executor.confirm(&action_id, true).await;
    assert!(matches!(outcome, Outcome::Failed { .. }));
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn forbidden_by_arguments_is_denied_without_running() {
    let f = fixture(Duration::from_secs(30), &[]);
    let outcome = f
        .executor
        .run(intent("t.confirm", json!({"escalate": true})))
        .await;
    assert!(matches!(outcome, Outcome::Denied { .. }));
    assert_eq!(f.executor.pending_count(), 0);
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 0);
    assert_eq!(f.audit.records()[0].risk, "forbidden");
}

#[tokio::test]
async fn validation_errors_map_to_failed_and_forbidden_to_denied() {
    let f = fixture(Duration::from_secs(30), &[]);
    let failed = f.executor.run(intent("t.safe", json!({"bad": "x"}))).await;
    assert!(matches!(failed, Outcome::Failed { ref code, .. } if code == "VALIDATION"));
    let denied = f
        .executor
        .run(intent("t.safe", json!({"bad": "forbidden"})))
        .await;
    assert!(matches!(denied, Outcome::Denied { .. }));
    assert_eq!(f.safe_runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unknown_action_and_runtime_failure() {
    let f = fixture(Duration::from_secs(30), &[]);
    let unknown = f.executor.run(intent("nope.nothing", json!({}))).await;
    assert!(matches!(unknown, Outcome::Failed { ref code, .. } if code == "NOT_FOUND"));
    let failing = f.executor.run(intent("t.failing", json!({}))).await;
    assert!(matches!(failing, Outcome::Failed { ref code, .. } if code == "SYSTEM"));
    assert!(f
        .audit
        .records()
        .iter()
        .any(|r| !r.ok && r.detail.contains("boom")));
}

#[tokio::test]
async fn disabled_actions_are_denied_and_can_be_switched_back() {
    let f = fixture(Duration::from_secs(30), &["t.safe"]);
    let outcome = f.executor.run(intent("t.safe", json!({}))).await;
    assert!(matches!(outcome, Outcome::Denied { .. }));
    assert_eq!(f.safe_runs.load(Ordering::SeqCst), 0);
    let listed = f.executor.registry().list();
    let safe = listed.iter().find(|a| a.name == "t.safe").expect("listed");
    assert!(!safe.enabled);
    f.executor.registry().set_disabled(HashSet::new());
    let outcome = f.executor.run(intent("t.safe", json!({}))).await;
    assert!(matches!(outcome, Outcome::Done { .. }));
}

#[tokio::test]
async fn disabling_between_request_and_confirm_blocks_execution() {
    let f = fixture(Duration::from_secs(30), &[]);
    let Outcome::NeedsConfirm { action_id, .. } =
        f.executor.run(intent("t.confirm", json!({}))).await
    else {
        panic!("expected NeedsConfirm");
    };
    f.executor
        .registry()
        .set_disabled(["t.confirm".to_owned()].into_iter().collect());
    let outcome = f.executor.confirm(&action_id, true).await;
    assert!(matches!(outcome, Outcome::Denied { .. }));
    assert_eq!(f.confirm_runs.load(Ordering::SeqCst), 0);
}

// ---------- the real registry: risk map and policy (no system effects) ----------

fn real_executor() -> (Executor, Arc<MemoryAudit>) {
    let audit = Arc::new(MemoryAudit::default());
    (
        windows_executor(ControlConfig::default(), audit.clone()),
        audit,
    )
}

#[tokio::test]
async fn real_registry_risk_map() {
    let (executor, _) = real_executor();
    let list = executor.registry().list();
    let risk_of = |name: &str| {
        list.iter()
            .find(|a| a.name == name)
            .map(|a| a.risk)
            .unwrap_or(Risk::Forbidden)
    };
    for confirm in [
        "power.shutdown",
        "power.restart",
        "power.sleep",
        "power.hibernate",
        "process.kill",
        "files.delete",
    ] {
        assert_eq!(risk_of(confirm), Risk::Confirm, "{confirm}");
    }
    for safe in [
        "power.lock",
        "power.abort",
        "app.open",
        "window.close",
        "audio.set_volume",
        "network.check",
        "info.time",
        "window.list",
    ] {
        assert_eq!(risk_of(safe), Risk::Safe, "{safe}");
    }
    assert!(list.len() >= 25);
    assert!(list
        .iter()
        .all(|a| !a.description_ru.is_empty() && !a.description_en.is_empty()));
}

#[tokio::test]
async fn shutdown_and_kill_never_run_without_confirmation() {
    let (executor, audit) = real_executor();
    for action in [
        "power.shutdown",
        "power.restart",
        "power.sleep",
        "power.hibernate",
    ] {
        let outcome = executor.run(intent(action, json!({}))).await;
        let Outcome::NeedsConfirm { action_id, .. } = outcome else {
            panic!("{action} must need confirmation, got {outcome:?}");
        };
        // Decline: nothing is executed.
        let declined = executor.confirm(&action_id, false).await;
        assert!(matches!(declined, Outcome::Denied { .. }));
    }
    let kill = executor
        .run(intent("process.kill", json!({"pid": 999_999})))
        .await;
    assert!(matches!(kill, Outcome::NeedsConfirm { .. }));
    assert_eq!(executor.emergency_stop(), 1);
    assert!(audit.records().iter().all(|r| !r.ok));
}

#[tokio::test]
async fn protected_processes_are_forbidden() {
    let (executor, audit) = real_executor();
    for name in [
        "csrss", "wininit", "winlogon", "lsass", "services", "smss", "svchost", "explorer",
        "MsMpEng", "System", "Registry", "dwm", "jarvisd",
    ] {
        let outcome = executor
            .run(intent("process.kill", json!({"name": name})))
            .await;
        assert!(
            matches!(outcome, Outcome::Denied { .. }),
            "{name}: {outcome:?}"
        );
    }
    let by_pid = executor
        .run(intent("process.kill", json!({"pid": 4})))
        .await;
    assert!(matches!(by_pid, Outcome::Denied { .. }));
    let own = executor
        .run(intent("process.kill", json!({"pid": std::process::id()})))
        .await;
    assert!(matches!(own, Outcome::Denied { .. }));
    assert_eq!(executor.pending_count(), 0);
    assert!(audit
        .records()
        .iter()
        .all(|r| !r.ok && r.risk == "forbidden" || !r.ok));
}

#[tokio::test]
async fn argument_validation_on_real_actions() {
    let (executor, _) = real_executor();
    let level = executor
        .run(intent("audio.set_volume", json!({"level": 101})))
        .await;
    assert!(matches!(level, Outcome::Failed { ref code, .. } if code == "VALIDATION"));
    let extra = executor
        .run(intent(
            "audio.set_volume",
            json!({"level": 5, "loud": true}),
        ))
        .await;
    assert!(matches!(extra, Outcome::Failed { ref code, .. } if code == "VALIDATION"));
    let empty = executor
        .run(intent("app.open", json!({"query": "  "})))
        .await;
    assert!(matches!(empty, Outcome::Failed { ref code, .. } if code == "VALIDATION"));
    let both = executor
        .run(intent(
            "files.open_folder",
            json!({"name": "downloads", "path": "E:\\"}),
        ))
        .await;
    assert!(matches!(both, Outcome::Failed { ref code, .. } if code == "VALIDATION"));
}

#[tokio::test]
async fn file_actions_refuse_paths_outside_roots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = ControlConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        ..ControlConfig::default()
    };
    let executor = windows_executor(config, Arc::new(MemoryAudit::default()));
    let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    for path in [
        format!("{windows_dir}\\notepad.exe"),
        format!("{}\\..\\x.txt", dir.path().display()),
        "\\\\server\\share\\x.txt".to_owned(),
    ] {
        let delete = executor
            .run(intent("files.delete", json!({"path": path})))
            .await;
        assert!(
            matches!(delete, Outcome::Denied { .. } | Outcome::Failed { .. }),
            "{path}: {delete:?}"
        );
        assert!(!matches!(delete, Outcome::NeedsConfirm { .. }), "{path}");
        let list = executor
            .run(intent("files.list", json!({"path": path})))
            .await;
        assert!(!matches!(list, Outcome::Done { .. }), "{path}");
    }
    // The root itself can be listed but never deleted.
    let root = dir.path().to_string_lossy().into_owned();
    let delete_root = executor
        .run(intent("files.delete", json!({"path": root.clone()})))
        .await;
    assert!(matches!(delete_root, Outcome::Denied { .. }));
    let list_root = executor
        .run(intent("files.list", json!({"path": root})))
        .await;
    assert!(matches!(list_root, Outcome::Done { .. }));
}

#[tokio::test]
async fn open_folder_never_opens_files() {
    // Security: ShellExecute "open" on a file would run it, so files.open_folder must refuse
    // anything that is not a directory, even inside an allowed root.
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("payload.bat");
    std::fs::write(&script, "@echo off\r\n").expect("write");
    let config = ControlConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        ..ControlConfig::default()
    };
    let executor = windows_executor(config, Arc::new(MemoryAudit::default()));
    let outcome = executor
        .run(intent(
            "files.open_folder",
            json!({"path": script.to_string_lossy()}),
        ))
        .await;
    assert!(matches!(outcome, Outcome::Denied { .. }), "{outcome:?}");
}

#[test]
fn error_codes_use_contract_names() {
    assert_eq!(ErrorCode::Validation.as_str(), "VALIDATION");
    assert_eq!(ErrorCode::Forbidden.as_str(), "FORBIDDEN");
    assert_eq!(ErrorCode::NotFound.as_str(), "NOT_FOUND");
    assert_eq!(ErrorCode::System.as_str(), "SYSTEM");
    assert_eq!(ErrorCode::Timeout.as_str(), "TIMEOUT");
    assert_eq!(ErrorCode::Unavailable.as_str(), "UNAVAILABLE");
}
