//! Command line checks for the system intelligence crate: `--metrics` and `--alerts-demo`.

use anyhow::Context;
use jarvis_core::{EventBus, Service};
use jarvis_sysintel::gpu::GpuSource;
use jarvis_sysintel::{
    Alert, AlertConfig, AlertEngine, AlertInput, AppCrashed, DiskSample, HungWindow, Rank,
    SysEvent, SysIntel, SysIntelConfig,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_SECONDS: u64 = 10;
/// Internal flag: run as the NVML helper process of the sampling service.
pub const GPU_WORKER_FLAG: &str = "--gpu-worker";

/// The sampling service with NVML read through a helper process (this executable).
pub fn new_service(bus: EventBus, config: SysIntelConfig) -> anyhow::Result<SysIntel> {
    let source = GpuSource::worker_from_current_exe(GPU_WORKER_FLAG)
        .context("locating the jarvisd executable for the GPU worker")?;
    Ok(SysIntel::new(bus, config).with_gpu_source(source))
}

fn parse_seconds(flags: &[String]) -> anyhow::Result<u64> {
    match flags {
        [] => Ok(DEFAULT_SECONDS),
        [flag, value] if flag == "--seconds" => {
            let seconds: u64 = value.parse().context("invalid --seconds value")?;
            anyhow::ensure!(seconds > 0, "--seconds must be positive");
            Ok(seconds)
        }
        _ => anyhow::bail!("usage: jarvisd --metrics [--seconds <n>]"),
    }
}

/// Prints the real metrics once a second (window-open rates), then the top RAM consumers and the
/// last full sample.
pub async fn run_metrics(flags: &[String]) -> anyhow::Result<()> {
    let seconds = parse_seconds(flags)?;
    let service = Arc::new(new_service(EventBus::new(), SysIntelConfig::default())?);
    let mut metrics = service.subscribe_metrics();
    let mut events = service.subscribe_events();
    let runner = Arc::clone(&service);
    let task = tokio::spawn(async move { runner.start().await });

    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let mut last_print: Option<Instant> = None;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => break,
            received = metrics.recv() => {
                let Ok(sample) = received else { continue };
                let now = Instant::now();
                if last_print.is_some_and(|at| now.duration_since(at) < Duration::from_millis(900)) {
                    continue;
                }
                last_print = Some(now);
                println!(
                    "t={:>2}s {}",
                    started.elapsed().as_secs(),
                    serde_json::to_string(&*sample)?
                );
            }
            received = events.recv() => {
                if let Ok(event) = received {
                    println!("EVENT {event:?}");
                }
            }
        }
    }

    let top = service.top_processes(Rank::Ram, 5).await?;
    println!("top 5 by RAM (requested on demand):");
    for process in &top {
        println!(
            "  {} ({}) x{}: {} MB",
            process.display_name, process.exe, process.instances, process.memory_mb
        );
    }
    println!(
        "final sample:\n{}",
        serde_json::to_string_pretty(&service.snapshot())?
    );
    service.stop().await?;
    task.await.context("sysintel task")??;
    Ok(())
}

/// Replays recorded synthetic series through the alert engine. NOT real sensor data.
pub fn run_alerts_demo() {
    println!("=== ALERTS DEMO: synthetic series, not real sensor readings ===");
    let mut engine = AlertEngine::new(AlertConfig::default());
    let hung = [HungWindow {
        hwnd: 1,
        pid: 100,
        process: Some("demo-app.exe".into()),
        title: "Demo window".into(),
    }];
    let mut consumer = || Some("Google Chrome".to_string());
    for second in (0..=120u64).step_by(5) {
        let input = AlertInput {
            now_ms: second * 1000,
            local_minute: 12 * 60,
            // CPU stays at 96% from the start (fires after 60 s).
            cpu_percent: Some(96.0),
            // GPU is hot from t=10 s (fires after 30 s of that).
            gpu_temp_c: Some(if second >= 10 { 88.0 } else { 70.0 }),
            cpu_temp_c: None,
            // RAM at 94% from t=20 s.
            ram_percent: Some(if second >= 20 { 94.0 } else { 50.0 }),
            // 8 GB free on the system drive.
            system_disk: Some(DiskSample {
                free_bytes: 8 << 30,
                total_bytes: 500 << 30,
            }),
            // Internet drops at t=30 s.
            internet_up: Some(second < 30),
            // A window hangs from t=40 s.
            hung: if second >= 40 { &hung } else { &[] },
        };
        for alert in engine.evaluate(&input, &mut consumer) {
            print_alert(second, &alert);
        }
    }
    let crash = AppCrashed {
        name: "demo-crash.exe".into(),
        time_ms: 0,
    };
    if let Some(alert) = engine.on_crash(&crash, 125_000, 12 * 60) {
        println!("[DEMO] event {:?}", SysEvent::AppCrashed(crash));
        print_alert(125, &alert);
    }
}

fn print_alert(second: u64, alert: &Alert) {
    println!(
        "[DEMO] t={second:>3}s id={} kind={} severity={} speak={}\n         ru: {}\n         en: {}",
        alert.id,
        alert.kind.as_str(),
        alert.severity.as_str(),
        alert.speak,
        alert.message_ru,
        alert.message_en
    );
}
