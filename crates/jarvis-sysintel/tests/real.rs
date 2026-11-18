//! Real-machine checks of the sampling service (no mocks): sensors of this PC are read.

use jarvis_core::{EventBus, Service};
use jarvis_sysintel::{Mode, SysIntel, SysIntelConfig};
use std::sync::Arc;
use std::time::Duration;

fn nvml_present() -> bool {
    std::path::Path::new(r"C:\Windows\System32\nvml.dll").exists()
}

#[tokio::test(flavor = "current_thread")]
async fn window_mode_reads_real_sensors() {
    let service = Arc::new(SysIntel::new(EventBus::new(), SysIntelConfig::default()));
    assert_eq!(service.mode(), Mode::Tray, "no subscribers: tray rates");
    let mut stream = service.subscribe_metrics();
    assert_eq!(
        service.mode(),
        Mode::Window,
        "a subscriber selects window rates"
    );
    let runner = Arc::clone(&service);
    let task = tokio::spawn(async move { runner.start().await });

    // The second sample has rates; wait for one that has network and disk activity filled in.
    let mut last = None;
    for _ in 0..40 {
        let sample = tokio::time::timeout(Duration::from_secs(5), stream.recv())
            .await
            .expect("sample in time")
            .expect("stream open");
        let ready = sample.network.internet_reachable.is_some()
            && sample
                .storage
                .volumes
                .iter()
                .any(|v| v.read_bytes_per_sec.is_some());
        last = Some(sample);
        if ready {
            break;
        }
    }
    let sample = last.expect("a sample");
    assert!(sample.cpu.usage_percent.is_some());
    assert!(!sample.cpu.per_core_percent.is_empty());
    assert!(sample.ram.total_bytes > 0 && sample.ram.used_bytes <= sample.ram.total_bytes);
    assert!(sample
        .storage
        .volumes
        .iter()
        .any(|v| v.is_system && v.total_bytes > 0));
    assert!(sample.network.link_up.is_some());
    // No LibreHardwareMonitor here: the CPU temperature is unknown, never invented.
    if sample.cpu.temperature_c.is_some() {
        assert!(sample.cpu.temperature_c.unwrap_or(0.0) > 0.0);
    }
    if nvml_present() {
        let gpu = sample
            .gpu
            .as_ref()
            .expect("NVML is installed, so the GPU must be read");
        assert!(gpu.temperature_c.is_some());
        assert!(gpu.vram_total_bytes.unwrap_or(0) > 0);
    }
    service.stop().await.expect("stop");
    task.await.expect("join").expect("service result");
}

#[tokio::test(flavor = "current_thread")]
async fn tray_mode_without_subscribers_still_keeps_a_snapshot() {
    let service = Arc::new(SysIntel::new(EventBus::new(), SysIntelConfig::default()));
    let runner = Arc::clone(&service);
    let task = tokio::spawn(async move { runner.start().await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let snapshot = service.snapshot();
    assert_eq!(service.mode(), Mode::Tray);
    assert!(snapshot.ram.total_bytes > 0);
    service.stop().await.expect("stop");
    task.await.expect("join").expect("service result");
}
