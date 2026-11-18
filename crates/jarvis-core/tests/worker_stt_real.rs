//! Real STT worker (faster-whisper) driven through `SttService`. Slow: loads the model.
//!
//! Run with `cargo test -p jarvis-core --test worker_stt_real -- --ignored --nocapture --test-threads=1`.
//! Set `JARVIS_STT_DEVICE=cpu` to measure the CPU path.

use jarvis_core::worker::{SttConfig, SttService, WorkerCommand};
use jarvis_core::Service;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_wav(path: &Path) -> Vec<i16> {
    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(&bytes[0..4], b"RIFF");
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        if id == b"fmt " {
            let channels = u16::from_le_bytes([bytes[offset + 10], bytes[offset + 11]]);
            let rate = u32::from_le_bytes([
                bytes[offset + 12],
                bytes[offset + 13],
                bytes[offset + 14],
                bytes[offset + 15],
            ]);
            assert_eq!((channels, rate), (1, 16_000), "WAV must be mono 16 kHz");
        }
        if id == b"data" {
            let end = (offset + 8 + size).min(bytes.len());
            let (pairs, _) = bytes[offset + 8..end].as_chunks::<2>();
            return pairs.iter().map(|pair| i16::from_le_bytes(*pair)).collect();
        }
        offset += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}

fn audio(name: &str) -> Vec<i16> {
    read_wav(&repo_root().join("workers/stt/tests/audio").join(name))
}

fn venv_python() -> PathBuf {
    repo_root().join("workers/stt/.venv/Scripts/python.exe")
}

fn direct_command() -> WorkerCommand {
    WorkerCommand::python_module(
        venv_python(),
        "jarvis_stt",
        Some(repo_root().join("workers/stt")),
    )
}

fn gpu_memory_used_mib() -> String {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output();
    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        Err(error) => format!("nvidia-smi unavailable: {error}"),
    }
}

/// Processes whose command line mentions `jarvis_stt`: (pid, name, working set in MiB).
fn jarvis_stt_processes() -> Vec<(u32, String, u64)> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new()
            .with_cmd(UpdateKind::Always)
            .with_memory(),
    );
    system
        .processes()
        .iter()
        .filter(|(_, process)| {
            let name = process.name().to_string_lossy().to_lowercase();
            (name.starts_with("python") || name.starts_with("uv"))
                && process
                    .cmd()
                    .iter()
                    .any(|part| part.to_string_lossy().contains("jarvis_stt"))
        })
        .map(|(pid, process)| {
            (
                pid.as_u32(),
                process.name().to_string_lossy().into_owned(),
                process.memory() / 1024 / 1024,
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "slow: loads the real Whisper model"]
async fn recognizes_phrases_and_reports_measurements() {
    let mut config = SttConfig::new(direct_command());
    config.first_hello_timeout = Duration::from_secs(300);
    let service = SttService::new(config);
    println!(
        "GPU memory used before start: {} MiB",
        gpu_memory_used_mib()
    );

    let cold = Instant::now();
    let russian = service
        .transcribe(&audio("ru_open_browser.wav"), None)
        .await
        .expect("russian phrase");
    let first_total = cold.elapsed();
    let hello = service.hello().await.expect("hello");
    let pid = service.worker_pid().await.expect("pid");
    println!("device={} model={:?}", hello.device, hello.model);
    println!(
        "first request incl. cold start: {first_total:?}; worker reported {} ms",
        russian.ms
    );
    println!("ru: {russian:?}");
    assert_eq!(russian.lang, "ru");
    let text = russian.text.to_lowercase();
    assert!(text.contains("браузер") && text.contains("музык"), "{text}");

    for _ in 0..3 {
        let started = Instant::now();
        let english = service
            .transcribe(&audio("en_open_browser.wav"), None)
            .await
            .expect("english phrase");
        println!(
            "en: {english:?}; round trip {:?} (3 s phrase)",
            started.elapsed()
        );
        assert_eq!(english.lang, "en");
        let text = english.text.to_lowercase();
        assert!(text.contains("browser") && text.contains("music"), "{text}");
    }

    println!(
        "worker processes (pid, name, working set MiB): {:?}",
        jarvis_stt_processes()
    );
    println!(
        "GPU memory used with model loaded: {} MiB (worker pid {pid})",
        gpu_memory_used_mib()
    );
    assert!(service.ping(Duration::from_secs(2)).await.expect("ping"));
    service.stop().await.expect("stop");
}

#[tokio::test]
#[ignore = "slow: loads the real Whisper model"]
async fn dev_launch_via_uv_leaves_no_process_after_idle_timeout() {
    let mut config = SttConfig::new(WorkerCommand::dev_stt(&repo_root()));
    config.first_hello_timeout = Duration::from_secs(300);
    config.idle_timeout = Duration::from_secs(5);
    let service = SttService::new(config);
    assert!(
        jarvis_stt_processes().is_empty(),
        "no worker before the first request"
    );

    let started = Instant::now();
    let transcript = service
        .transcribe(&audio("en_open_browser.wav"), Some("en"))
        .await
        .expect("transcribe");
    println!(
        "uv launch, first request incl. cold start: {:?}",
        started.elapsed()
    );
    println!("transcript: {transcript:?}");
    let running = jarvis_stt_processes();
    println!("processes while running: {running:?}");
    assert!(!running.is_empty());

    let idle_started = Instant::now();
    while !jarvis_stt_processes().is_empty() && idle_started.elapsed() < Duration::from_secs(20) {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    println!(
        "all worker processes gone {:?} after the last request",
        idle_started.elapsed()
    );
    assert!(
        jarvis_stt_processes().is_empty(),
        "idle timeout must leave no process"
    );
    assert!(!service.is_running().await);
}
