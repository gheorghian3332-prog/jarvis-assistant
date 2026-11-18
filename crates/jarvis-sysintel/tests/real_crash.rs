//! Crash-event subscription proof against real Windows Error Reporting: a throwaway child process
//! (this test executable, re-run in "crash" mode) dies with an access violation, and `EvtSubscribe`
//! must deliver its Application Error event. Run on purpose:
//! `cargo test -p jarvis-sysintel --test real_crash -- --ignored --nocapture`.

use jarvis_sysintel::crash::CrashWatcher;
use std::time::Duration;

const CRASH_ENV: &str = "JARVIS_SYSINTEL_CRASH_NOW";

/// The child side: does nothing unless the parent asked for a crash.
#[test]
#[ignore = "helper of the subscription test"]
fn crash_child() {
    if std::env::var_os(CRASH_ENV).is_some() {
        // SAFETY: deliberately invalid; the process is expected to die with an access violation.
        unsafe {
            (0x10 as *mut u8).write_volatile(1);
        }
    }
}

#[test]
#[ignore = "crashes a throwaway process and waits for WER; run explicitly"]
fn subscription_delivers_a_real_application_error() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _watcher = CrashWatcher::start(tx).expect("EvtSubscribe");

    let me = std::env::current_exe().expect("test exe");
    let own_name = me
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("name")
        .to_lowercase();
    let status = std::process::Command::new(&me)
        .args(["--ignored", "--exact", "crash_child"])
        .env(CRASH_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn child");
    assert!(!status.success(), "the child must crash: {status:?}");

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(crash) => {
                println!("received crash event: {crash:?}");
                if crash.name.to_lowercase().contains(&own_name) {
                    return;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    panic!("no Application Error event for {own_name} within 120 s");
}
