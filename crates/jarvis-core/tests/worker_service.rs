//! SttService / WorkerHandle against the fake worker script (a test double, not production code).

use jarvis_core::worker::{
    frame_type, tts_request, ManagedWorker, ShutdownOutcome, SttConfig, SttError, SttService,
    WorkerCommand, WorkerError, WorkerHandle,
};
use jarvis_core::Service;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::CloseHandle,
    System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

const STILL_ACTIVE: u32 = 259;

fn fake_worker(extra: &[&str]) -> WorkerCommand {
    let python = std::env::var("JARVIS_TEST_PYTHON").unwrap_or_else(|_| "python".to_owned());
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_worker.py");
    WorkerCommand::new(python)
        .args([script.into_os_string().into_string().expect("utf-8 path")])
        .args(extra.iter().map(|arg| (*arg).to_owned()))
}

fn config(extra: &[&str]) -> SttConfig {
    let mut config = SttConfig::new(fake_worker(extra));
    config.idle_timeout = Duration::from_secs(30);
    config.request_timeout = Duration::from_secs(10);
    config.first_hello_timeout = Duration::from_secs(10);
    config.hello_timeout = Duration::from_secs(10);
    config.shutdown_grace = Duration::from_secs(2);
    config
}

fn alive(pid: u32) -> bool {
    // SAFETY: plain Win32 queries on a handle opened and closed here.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == STILL_ACTIVE
    }
}

async fn wait_until_dead(pid: u32, limit: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    !alive(pid)
}

fn pcm(samples: usize) -> Vec<i16> {
    vec![100; samples]
}

fn worker_error(error: SttError) -> WorkerError {
    let SttError::Worker(error) = error;
    error
}

#[tokio::test]
async fn starts_lazily_and_transcribes() {
    let service = SttService::new(config(&[]));
    assert!(
        !service.is_running().await,
        "no process before the first request"
    );
    assert_eq!(service.worker_pid().await, None);

    let transcript = service
        .transcribe(&pcm(16_000), None)
        .await
        .expect("transcribe");
    assert_eq!(transcript.text, "fake 32000 bytes");
    assert_eq!(transcript.lang, "ru");
    assert!(transcript.is_final);
    assert_eq!(transcript.ms, 1);

    let hello = service.hello().await.expect("hello");
    assert_eq!(hello.worker, "stt");
    assert_eq!(hello.device, "cpu");
    let pid = service.worker_pid().await.expect("pid");
    assert!(alive(pid));
    service.stop().await.expect("stop");
    assert!(wait_until_dead(pid, Duration::from_secs(3)).await);
}

#[tokio::test]
async fn long_audio_is_split_into_frames_and_hint_is_forwarded() {
    let service = SttService::new(config(&[]));
    // 5 s of audio = 160 000 bytes = 10 chunks of 16 000 bytes.
    let transcript = service
        .transcribe(&pcm(80_000), Some("en"))
        .await
        .expect("transcribe");
    assert_eq!(transcript.text, "fake 160000 bytes");
    assert_eq!(transcript.lang, "en");
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn rejects_empty_and_overlong_audio_without_starting_a_worker() {
    let service = SttService::new(config(&[]));
    let empty = worker_error(service.transcribe(&[], None).await.expect_err("empty"));
    assert!(matches!(empty, WorkerError::InvalidInput(_)));
    let long = worker_error(
        service
            .transcribe(&pcm(16_000 * 121), None)
            .await
            .expect_err("too long"),
    );
    assert!(matches!(long, WorkerError::InvalidInput(_)));
    assert!(!service.is_running().await);
}

#[tokio::test]
async fn hello_timeout_fails_and_kills_the_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("pid");
    let mut config = config(&[
        "--hello-delay",
        "30",
        "--pid-file",
        pid_file.to_str().expect("utf-8"),
    ]);
    config.first_hello_timeout = Duration::from_millis(1500);
    let service = SttService::new(config);

    let started = Instant::now();
    let error = worker_error(
        service
            .transcribe(&pcm(1600), None)
            .await
            .expect_err("timeout"),
    );
    assert!(matches!(error, WorkerError::HelloTimeout(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(!service.is_running().await);
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("pid file")
        .trim()
        .parse()
        .expect("pid");
    assert!(
        wait_until_dead(pid, Duration::from_secs(3)).await,
        "worker must be killed"
    );
}

#[tokio::test]
async fn startup_failure_reports_worker_stderr() {
    let service = SttService::new(config(&["--exit-before-hello"]));
    let error = worker_error(
        service
            .transcribe(&pcm(1600), None)
            .await
            .expect_err("fails"),
    );
    let WorkerError::Startup(message) = error else {
        panic!("expected a startup error, got {error:?}");
    };
    assert!(message.contains("model load failed"), "{message}");
}

#[tokio::test]
async fn missing_executable_is_a_spawn_error() {
    let mut config = SttConfig::new(WorkerCommand::new("jarvis-no-such-program-xyz"));
    config.first_hello_timeout = Duration::from_secs(2);
    let service = SttService::new(config);
    let error = worker_error(
        service
            .transcribe(&pcm(1600), None)
            .await
            .expect_err("fails"),
    );
    assert!(matches!(error, WorkerError::Spawn(_)), "{error:?}");
}

#[tokio::test]
async fn idle_timeout_shuts_the_worker_down_and_next_request_restarts_it() {
    let mut config = config(&[]);
    config.idle_timeout = Duration::from_millis(1200);
    let service = SttService::new(config);

    service.transcribe(&pcm(1600), None).await.expect("first");
    let first = service.worker_pid().await.expect("pid");
    assert!(alive(first));

    assert!(
        wait_until_dead(first, Duration::from_secs(6)).await,
        "worker must exit after the idle timeout"
    );
    assert!(!service.is_running().await);
    assert_eq!(service.worker_pid().await, None);

    service.transcribe(&pcm(1600), None).await.expect("second");
    let second = service.worker_pid().await.expect("pid");
    assert_ne!(first, second);
    service.stop().await.expect("stop");
    assert!(wait_until_dead(second, Duration::from_secs(3)).await);
}

#[tokio::test]
async fn activity_postpones_the_idle_shutdown() {
    let mut config = config(&[]);
    config.idle_timeout = Duration::from_millis(2000);
    let service = SttService::new(config);

    service.transcribe(&pcm(1600), None).await.expect("first");
    let pid = service.worker_pid().await.expect("pid");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    service.transcribe(&pcm(1600), None).await.expect("second");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // 2.4 s after the first request but only 1.2 s after the last one.
    assert!(alive(pid));
    assert_eq!(service.worker_pid().await, Some(pid));
    assert!(wait_until_dead(pid, Duration::from_secs(6)).await);
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn crash_fails_the_request_and_the_next_one_restarts_the_worker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("crashed");
    let service = SttService::new(config(&["--crash-once", marker.to_str().expect("utf-8")]));

    let error = worker_error(
        service
            .transcribe(&pcm(1600), None)
            .await
            .expect_err("crash"),
    );
    assert!(matches!(error, WorkerError::Exited(_)), "{error:?}");
    assert!(!service.is_running().await, "no automatic restart loop");
    assert_eq!(service.worker_pid().await, None);

    let transcript = service
        .transcribe(&pcm(1600), None)
        .await
        .expect("recovered");
    assert_eq!(transcript.text, "fake 3200 bytes");
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn worker_that_died_while_idle_is_replaced_transparently() {
    let service = SttService::new(config(&[]));
    service.transcribe(&pcm(1600), None).await.expect("first");
    let pid = service.worker_pid().await.expect("pid");
    let killed = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output()
        .expect("taskkill");
    assert!(killed.status.success());
    assert!(wait_until_dead(pid, Duration::from_secs(3)).await);
    let started = Instant::now();
    while service.is_running().await && started.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    service.transcribe(&pcm(1600), None).await.expect("second");
    assert_ne!(service.worker_pid().await, Some(pid));
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn request_timeout_kills_the_worker_and_the_next_request_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("hung");
    let mut config = config(&["--hang-once", marker.to_str().expect("utf-8")]);
    config.request_timeout = Duration::from_millis(1000);
    let service = SttService::new(config);

    let started = Instant::now();
    let error = worker_error(
        service
            .transcribe(&pcm(1600), None)
            .await
            .expect_err("hang"),
    );
    assert!(matches!(error, WorkerError::Timeout(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!service.is_running().await);

    service
        .transcribe(&pcm(1600), None)
        .await
        .expect("recovered");
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn worker_error_frame_is_reported_and_keeps_the_worker() {
    let service = SttService::new(config(&["--report-error"]));
    let error = worker_error(
        service
            .transcribe(&pcm(1600), None)
            .await
            .expect_err("error"),
    );
    let WorkerError::Reported { code, message } = error else {
        panic!("expected a reported error, got {error:?}");
    };
    assert_eq!(code, "transcribe_failed");
    assert_eq!(message, "boom");
    assert!(service.is_running().await);
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn concurrent_requests_are_served_one_at_a_time() {
    let service = SttService::new(config(&["--process-delay", "0.4"]));
    service.transcribe(&pcm(1600), None).await.expect("warm up");
    let pid = service.worker_pid().await;

    let started = Instant::now();
    let (short, medium, long) = (pcm(800), pcm(1600), pcm(2400));
    let (a, b, c) = tokio::join!(
        service.transcribe(&short, None),
        service.transcribe(&medium, None),
        service.transcribe(&long, None),
    );
    let mut texts = vec![a.expect("a").text, b.expect("b").text, c.expect("c").text];
    texts.sort();
    assert_eq!(
        texts,
        ["fake 1600 bytes", "fake 3200 bytes", "fake 4800 bytes"]
    );
    assert!(
        started.elapsed() >= Duration::from_millis(1100),
        "three 0.4 s requests must not overlap: {:?}",
        started.elapsed()
    );
    assert_eq!(
        service.worker_pid().await,
        pid,
        "one worker serves the queue"
    );
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn ping_returns_pong_only_while_a_worker_runs() {
    let service = SttService::new(config(&[]));
    assert!(!service
        .ping(Duration::from_secs(2))
        .await
        .expect("no worker"));
    service.transcribe(&pcm(1600), None).await.expect("start");
    assert!(service.ping(Duration::from_secs(2)).await.expect("ping"));
    service.stop().await.expect("stop");
}

#[tokio::test]
async fn shutdown_kills_a_worker_that_ignores_it() {
    let mut worker = WorkerHandle::spawn(
        &fake_worker(&["--ignore-shutdown"]),
        Duration::from_secs(10),
    )
    .await
    .expect("spawn");
    let pid = worker.pid();
    assert!(worker.is_running());
    worker.ping(Duration::from_secs(2)).await.expect("ping");

    let started = Instant::now();
    let outcome = worker.shutdown(Duration::from_millis(700)).await;
    assert_eq!(outcome, ShutdownOutcome::Killed);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(wait_until_dead(pid, Duration::from_secs(3)).await);
}

#[tokio::test]
async fn graceful_shutdown_is_reported() {
    let worker = WorkerHandle::spawn(&fake_worker(&[]), Duration::from_secs(10))
        .await
        .expect("spawn");
    let pid = worker.pid();
    assert_eq!(
        worker.shutdown(Duration::from_secs(2)).await,
        ShutdownOutcome::Graceful
    );
    assert!(wait_until_dead(pid, Duration::from_secs(3)).await);
}

#[tokio::test]
async fn raw_handle_exchanges_frames() {
    let mut worker = WorkerHandle::spawn(&fake_worker(&[]), Duration::from_secs(10))
        .await
        .expect("spawn");
    worker.send(0x55, b"").await.expect("send");
    let reply = worker.recv().await.expect("recv");
    assert_eq!(reply.kind, frame_type::ERROR);
    assert!(String::from_utf8_lossy(&reply.payload).contains("unknown_type"));
    worker.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn tts_request_returns_pcm_through_a_managed_worker() {
    let worker = ManagedWorker::new("tts", config(&[]));
    let pcm = worker
        .run(
            ("Hello".to_owned(), "bm_george".to_owned(), 1.0f32),
            |handle, (text, voice, speed)| {
                Box::pin(async move { tts_request(handle, &text, &voice, speed).await })
            },
        )
        .await
        .expect("pcm");
    assert_eq!(pcm, vec![1000i16; 5]);
    assert!(
        worker.is_running().await,
        "the worker stays up for the next phrase"
    );
    worker.stop().await;
    assert!(!worker.is_running().await);
}

#[tokio::test]
async fn tts_error_frame_is_reported_and_keeps_the_worker() {
    let worker = ManagedWorker::new("tts", config(&[]));
    let error = worker
        .run(
            ("fail".to_owned(), "bm_nobody".to_owned(), 1.0f32),
            |handle, (text, voice, speed)| {
                Box::pin(async move { tts_request(handle, &text, &voice, speed).await })
            },
        )
        .await
        .expect_err("worker reports an error");
    assert!(
        matches!(&error, WorkerError::Reported { code, .. } if code == "bad_request"),
        "{error}"
    );
    let pid = worker
        .worker_pid()
        .await
        .expect("worker survives a reported error");
    let pcm = worker
        .run(
            ("ok".to_owned(), "bm_george".to_owned(), 1.0f32),
            |handle, (text, voice, speed)| {
                Box::pin(async move { tts_request(handle, &text, &voice, speed).await })
            },
        )
        .await
        .expect("next phrase");
    assert_eq!(pcm.len(), 2);
    assert_eq!(worker.worker_pid().await, Some(pid), "same process");
    worker.stop().await;
}
