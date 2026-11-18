//! The GPU source follows the window: D3DKMT in the tray, a helper process while the window is
//! open, and the helper is gone again after the window closes. The helper here is a small
//! PowerShell loop that speaks the helper protocol (one reply line per request), so the check does
//! not depend on NVML.

use jarvis_core::{EventBus, Service};
use jarvis_sysintel::gpu::GpuSource;
use jarvis_sysintel::{Mode, SysIntel, SysIntelConfig};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};

fn fake_helper() -> GpuSource {
    GpuSource::Adaptive {
        exe: "powershell.exe".into(),
        args: vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "while (($line = [Console]::In.ReadLine()) -ne $null) { [Console]::Out.WriteLine('null') }"
                .into(),
        ],
    }
}

fn process_exists(pid: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system.process(Pid::from_u32(pid)).is_some()
}

async fn wait_for(what: &str, limit: Duration, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_helper_lives_only_while_the_window_is_open() {
    let service = Arc::new(
        SysIntel::new(EventBus::new(), SysIntelConfig::default()).with_gpu_source(fake_helper()),
    );
    assert_eq!(service.mode(), Mode::Tray);
    let runner = Arc::clone(&service);
    let task = tokio::spawn(async move { runner.start().await });

    // Tray: kernel statistics, no helper process at all.
    wait_for("the first tray sample", Duration::from_secs(10), || {
        service.snapshot().cpu.usage_percent.is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(service.gpu_worker_pid(), None, "no helper in the tray");

    // Window open: the helper starts.
    service.set_window_open(true);
    assert_eq!(service.mode(), Mode::Window);
    wait_for("the helper to start", Duration::from_secs(20), || {
        service.gpu_worker_pid().is_some()
    })
    .await;
    let pid = service.gpu_worker_pid().expect("helper pid");
    assert!(process_exists(pid), "the helper process runs");
    // The helper answers `null` (no NVML in this fake), so the kernel statistics fill in.
    let reading = service.snapshot().gpu;
    eprintln!("window-mode reading with a silent helper: {reading:?}");

    // Window closed: the helper is ended and really gone.
    service.set_window_open(false);
    wait_for("the helper to end", Duration::from_secs(10), || {
        service.gpu_worker_pid().is_none() && !process_exists(pid)
    })
    .await;

    // And it can come back with the next window.
    service.set_window_open(true);
    wait_for("the helper to restart", Duration::from_secs(20), || {
        service.gpu_worker_pid().is_some()
    })
    .await;
    let second = service.gpu_worker_pid().expect("second helper pid");
    assert_ne!(second, pid);
    service.set_window_open(false);
    wait_for("the second helper to end", Duration::from_secs(10), || {
        service.gpu_worker_pid().is_none() && !process_exists(second)
    })
    .await;

    service.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_d3dkmt_source_reads_the_gpu_without_any_helper() {
    let service = Arc::new(
        SysIntel::new(EventBus::new(), SysIntelConfig::default())
            .with_gpu_source(GpuSource::D3dkmt),
    );
    let runner = Arc::clone(&service);
    let task = tokio::spawn(async move { runner.start().await });
    wait_for("a GPU reading", Duration::from_secs(15), || {
        service.snapshot().gpu.is_some()
    })
    .await;
    let gpu = service.snapshot().gpu.expect("gpu");
    eprintln!("tray D3DKMT reading: {gpu:?}");
    assert!(gpu.name.is_some());
    assert_eq!(service.gpu_worker_pid(), None);
    service.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

/// With `JARVIS_EXE` pointing at the real application executable, the helper is the real NVML
/// helper (`jarvis.exe --gpu-worker`): window mode shows NVML readings (watts), the tray does not
/// run it. Skipped when the variable is not set.
#[tokio::test(flavor = "current_thread")]
async fn the_real_application_helper_comes_and_goes_with_the_window() {
    let Some(exe) = std::env::var_os("JARVIS_EXE") else {
        eprintln!("JARVIS_EXE is not set: skipping the real helper check");
        return;
    };
    let source = GpuSource::Adaptive {
        exe: exe.into(),
        args: vec!["--gpu-worker".into()],
    };
    let service =
        Arc::new(SysIntel::new(EventBus::new(), SysIntelConfig::default()).with_gpu_source(source));
    let runner = Arc::clone(&service);
    let task = tokio::spawn(async move { runner.start().await });
    wait_for("the tray reading", Duration::from_secs(15), || {
        service.snapshot().gpu.is_some()
    })
    .await;
    assert_eq!(service.gpu_worker_pid(), None);
    let tray = service.snapshot().gpu.expect("gpu");
    assert_eq!(tray.power_watts, None, "the tray reads D3DKMT: no watts");

    service.set_window_open(true);
    wait_for("the helper", Duration::from_secs(30), || {
        service.gpu_worker_pid().is_some()
    })
    .await;
    let pid = service.gpu_worker_pid().expect("pid");
    wait_for("an NVML reading", Duration::from_secs(30), || {
        service
            .snapshot()
            .gpu
            .is_some_and(|gpu| gpu.power_watts.is_some())
    })
    .await;
    eprintln!(
        "window mode, NVML helper pid {pid}: {:?}",
        service.snapshot().gpu
    );
    assert!(process_exists(pid));

    service.set_window_open(false);
    wait_for("the helper to end", Duration::from_secs(10), || {
        service.gpu_worker_pid().is_none() && !process_exists(pid)
    })
    .await;
    eprintln!("tray again: helper {pid} is gone");
    service.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}
