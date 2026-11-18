//! Night-safe checks against the real Windows session. Run explicitly:
//! `cargo test -p jarvis-control --test real -- --ignored --nocapture --test-threads=1`
//!
//! What is deliberately NOT done here: lock, sleep, shutdown, killing other processes, launching games,
//! changing the volume level, playing sounds, focusing/minimizing windows, keyboard/mouse input.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jarvis_control::apps::{self, AppIndex, AppSource};
use jarvis_control::{audio, network, procs, windows_executor, wnd};
use jarvis_control::{ControlConfig, Intent, MemoryAudit, Outcome};
use serde_json::json;

#[test]
#[ignore = "touches the real system"]
fn real_volume_read_set_same_read() {
    let before = audio::get_scalar().expect("read volume");
    println!(
        "volume before: scalar {before:.6} = {}%",
        (before * 100.0).round()
    );
    audio::set_scalar(before).expect("set the same scalar");
    let after = audio::get_scalar().expect("read volume again");
    println!(
        "volume after:  scalar {after:.6} = {}%",
        (after * 100.0).round()
    );
    assert!(
        (before - after).abs() < 1e-4,
        "level changed: {before} -> {after}"
    );
    println!(
        "muted (read only): {}",
        audio::is_muted().expect("mute state")
    );
}

#[test]
#[ignore = "touches the real system"]
fn real_app_index() {
    let started = Instant::now();
    let index = AppIndex::build(&ControlConfig::default());
    println!(
        "app index: {} entries in {:?} (start-menu {}, steam {}, uwp {}, builtin {})",
        index.len(),
        started.elapsed(),
        index.count_by_source(AppSource::StartMenu),
        index.count_by_source(AppSource::Steam),
        index.count_by_source(AppSource::Uwp),
        index.count_by_source(AppSource::Builtin),
    );
    assert!(index.len() > 10);
    for query in [
        "steam",
        "стим",
        "discord",
        "дискорд",
        "telegram",
        "телеграм",
        "dungeon quest 3",
        "данжен квест 3",
        "chrome",
        "браузер",
        "obsidian",
    ] {
        match index.best(query) {
            Some(found) => println!(
                "  {query:<14} -> {} [{}] score {:.1} launch {}",
                found.name,
                found.source.as_str(),
                found.score,
                found.launch
            ),
            None => println!("  {query:<14} -> not found"),
        }
    }
    let games: Vec<_> = index
        .entries()
        .iter()
        .filter(|entry| entry.source == AppSource::Steam)
        .map(|entry| entry.name.clone())
        .collect();
    println!("steam games kept in the index (not shadowed by a Start Menu shortcut): {games:?}");
    let roots = apps::steam::steam_roots(&[]);
    let manifests = apps::steam::discover_games(&roots);
    println!("steam roots: {roots:?}");
    println!("steam manifests parsed directly: {manifests:?}");
    assert!(!roots.is_empty(), "Steam installation was not detected");
}

#[test]
#[ignore = "touches the real system"]
fn real_windows_and_processes_list_only() {
    let windows = wnd::list_windows().expect("windows");
    println!("{} visible top-level windows", windows.len());
    for window in windows.iter().take(8) {
        println!(
            "  pid {:<6} {:<22} {}",
            window.pid, window.process, window.title
        );
    }
    assert!(!windows.is_empty());
    let by_cpu = procs::list(procs::SortKey::Cpu, 5);
    let by_ram = procs::list(procs::SortKey::Ram, 5);
    println!(
        "{} processes running; top CPU: {:?}; top RAM: {:?}",
        procs::count(),
        by_cpu
            .iter()
            .map(|p| format!("{} {:.1}%", p.name, p.cpu_percent))
            .collect::<Vec<_>>(),
        by_ram
            .iter()
            .map(|p| format!("{} {}MB", p.name, p.memory_mb))
            .collect::<Vec<_>>(),
    );
    println!(
        "hung windows: {}",
        procs::hung_windows().expect("hung").len()
    );
}

#[test]
#[ignore = "touches the real system"]
fn real_network_check() {
    let report = network::check();
    println!(
        "online: {}, dns ok: {} ({:?} ms)",
        report.online, report.dns_ok, report.dns_ms
    );
    for probe in &report.probes {
        println!(
            "  {} ok={} rtt={:?} ms",
            probe.target, probe.ok, probe.rtt_ms
        );
    }
    println!(
        "adapters: {:?}",
        report
            .adapters
            .iter()
            .map(|a| format!("{} up={}", a.name, a.up))
            .collect::<Vec<_>>()
    );
    assert!(!report.adapters.is_empty());
}

fn notepad_windows() -> Vec<wnd::WindowInfo> {
    wnd::list_windows()
        .expect("windows")
        .into_iter()
        .filter(|w| w.process.to_lowercase().contains("notepad"))
        .collect()
}

#[test]
#[ignore = "touches the real system"]
fn real_notepad_launch_find_close() {
    let before: HashSet<isize> = notepad_windows().iter().map(|w| w.hwnd).collect();
    println!("notepad windows before: {}", before.len());
    let index = AppIndex::build(&ControlConfig::default());
    let found = index.best("блокнот").expect("notepad in the index");
    println!("launching {} via {}", found.name, found.launch);
    apps::launch(&found).expect("launch");
    let deadline = Instant::now() + Duration::from_secs(15);
    let ours = loop {
        if let Some(window) = notepad_windows()
            .into_iter()
            .find(|w| !before.contains(&w.hwnd))
        {
            break window;
        }
        assert!(Instant::now() < deadline, "no new notepad window appeared");
        std::thread::sleep(Duration::from_millis(200));
    };
    println!(
        "found window hwnd={} pid={} process={} title={:?}",
        ours.hwnd, ours.pid, ours.process, ours.title
    );
    wnd::close_hwnd(ours.hwnd).expect("WM_CLOSE");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let still_there = notepad_windows().iter().any(|w| w.hwnd == ours.hwnd);
        if !still_there {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "notepad window did not close after WM_CLOSE"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("window closed by WM_CLOSE");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut process_gone = false;
    while Instant::now() < deadline {
        if procs::find(None, Some(ours.pid)).is_empty() {
            process_gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("process {} gone: {process_gone}", ours.pid);
    let after: HashSet<isize> = notepad_windows().iter().map(|w| w.hwnd).collect();
    println!("notepad windows after: {}", after.len());
    assert_eq!(before, after, "notepad window set differs from the start");
}

#[tokio::test]
#[ignore = "touches the real system"]
async fn real_recycle_bin_with_temp_file() {
    let dir = std::env::temp_dir().join(format!("jarvis-control-real-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("delete-me.txt");
    std::fs::write(&file, "jarvis-control recycle test").expect("write");
    let config = ControlConfig {
        allowed_roots: vec![dir.clone()],
        ..ControlConfig::default()
    };
    let audit = Arc::new(MemoryAudit::default());
    let executor = windows_executor(config, audit.clone());
    let outcome = executor
        .run(Intent::new(
            "files.delete",
            json!({"path": file.to_string_lossy()}),
        ))
        .await;
    let Outcome::NeedsConfirm { action_id, summary } = outcome else {
        panic!("expected NeedsConfirm, got {outcome:?}");
    };
    println!("confirmation asked: {summary}");
    assert!(file.exists(), "file must still exist before confirmation");
    let done = executor.confirm(&action_id, true).await;
    println!("after confirm: {done:?}");
    assert!(matches!(done, Outcome::Done { .. }), "{done:?}");
    assert!(!file.exists(), "file must be gone from the folder");
    println!(
        "audit: {:?}",
        audit
            .records()
            .iter()
            .map(|r| (r.action.clone(), r.ok))
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "touches the real system"]
async fn real_abort_with_nothing_scheduled_and_info() {
    let executor = windows_executor(ControlConfig::default(), Arc::new(MemoryAudit::default()));
    for action in ["power.abort", "info.time", "info.date", "audio.get_volume"] {
        let outcome = executor.run(Intent::new(action, json!({}))).await;
        println!("{action}: {outcome:?}");
        assert!(
            matches!(outcome, Outcome::Done { .. }),
            "{action}: {outcome:?}"
        );
    }
}
