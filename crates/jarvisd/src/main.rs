use anyhow::Context;
use jarvis_core::{selfstat, Database, Event, EventBus, Supervisor};
use jarvis_sysintel::SysIntelConfig;
use std::{env, fs::OpenOptions, sync::Arc, time::Duration};
use tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_shutdown};

mod sysintel_cli;
mod vision_cli;

const USAGE: &str =
    "usage: jarvisd [--selfstat-interval <seconds>] | jarvisd --voice-wav <path.wav> \
[--no-wake | --worker] [--stt [--no-prewarm] [--realtime]] | jarvisd --metrics [--seconds <n>] \
| jarvisd --alerts-demo | jarvisd --vision <describe|read-window|explain-error|ocr-file <png>> \n[--question <text>] | jarvisd --gpu-worker (internal)";

/// What the WAV run should do after the wake word pipeline.
struct WavOptions {
    wake: jarvis_voice::WakeMode,
    stt: bool,
    prewarm: bool,
    realtime: bool,
}

fn parse_wav_options(flags: &[String]) -> anyhow::Result<WavOptions> {
    use jarvis_voice::{WakeMode, WorkerCommand};
    let mut options = WavOptions {
        wake: WakeMode::InProcess,
        stt: false,
        prewarm: true,
        realtime: false,
    };
    for flag in flags {
        match flag.as_str() {
            "--no-wake" => options.wake = WakeMode::Off,
            // Vosk in a worker process (this executable), the way the app runs it.
            "--worker" => options.wake = WakeMode::Worker(WorkerCommand::detect()?),
            "--stt" => options.stt = true,
            "--no-prewarm" => options.prewarm = false,
            "--realtime" => options.realtime = true,
            _ => anyhow::bail!(USAGE),
        }
    }
    Ok(options)
}

/// Runs a WAV file through the voice pipeline and prints the events (no audio is stored or sent).
/// With `--stt` every utterance is also transcribed by the STT worker, the way the app does it
/// (the worker is prewarmed when the wake word is heard, unless `--no-prewarm`).
async fn run_voice_wav(arguments: &[String]) -> anyhow::Result<()> {
    use jarvis_core::{Event, VoiceHint};
    use jarvis_voice::{default_transcriber, process_wav_with, vosk_model_dir, VoiceConfig};
    use std::sync::Mutex;
    use std::time::Instant;

    let path = arguments
        .get(1)
        .context("--voice-wav requires a WAV file path")?
        .clone();
    let options = parse_wav_options(&arguments[2..])?;
    let stt = options.stt.then(default_transcriber);
    let handle = tokio::runtime::Handle::current();
    type Job = tokio::task::JoinHandle<(Instant, Result<jarvis_voice::Spoken, String>)>;
    let jobs: Arc<Mutex<Vec<(Instant, Job)>>> = Arc::default();
    let wake_at: Arc<Mutex<Option<Instant>>> = Arc::default();

    let worker_jobs = Arc::clone(&jobs);
    let worker_wake = Arc::clone(&wake_at);
    let worker_stt = stt.clone();
    let prewarm = options.prewarm;
    let events = tokio::task::spawn_blocking(move || {
        let on_event = move |event: &Event| {
            let Some(stt) = worker_stt.clone() else {
                return;
            };
            match event {
                Event::WakeDetected { .. } => {
                    *worker_wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                    if prewarm {
                        handle.spawn(async move {
                            let _ = stt.prewarm().await;
                        });
                    }
                }
                Event::Utterance { pcm16k, .. } => {
                    let pcm = Arc::clone(pcm16k);
                    let ready = Instant::now();
                    let job = handle.spawn(async move {
                        let result = stt.transcribe(&pcm).await;
                        (Instant::now(), result)
                    });
                    worker_jobs
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((ready, job));
                }
                _ => {}
            }
        };
        process_wav_with(
            std::path::Path::new(&path),
            &VoiceConfig::default(),
            &vosk_model_dir(),
            options.wake,
            options.realtime,
            Box::new(on_event),
        )
        .map_err(|error| anyhow::anyhow!(error.to_string()))
    })
    .await??;

    let mut wakes = 0;
    let mut utterances = 0;
    for event in &events {
        match event {
            Event::WakeDetected { name, confidence } => {
                wakes += 1;
                println!("WakeDetected name={name} confidence={confidence:.2}");
            }
            Event::Utterance { pcm16k, source } => {
                utterances += 1;
                let source = source.as_str();
                println!(
                    "Utterance source={source} samples={} seconds={:.2}",
                    pcm16k.len(),
                    pcm16k.len() as f32 / 16_000.0
                );
            }
            Event::VoiceStateHint(VoiceHint::State(state)) => println!("State {state}"),
            Event::VoiceStateHint(VoiceHint::Mic(status)) => {
                println!("Mic peak={:?} no_signal={}", status.peak, status.no_signal)
            }
            _ => {}
        }
    }
    let pending: Vec<_> = std::mem::take(&mut *jobs.lock().unwrap_or_else(|e| e.into_inner()));
    let wake_time = *wake_at.lock().unwrap_or_else(|e| e.into_inner());
    for (ready, job) in pending {
        let (done, result) = job.await?;
        match result {
            Ok(spoken) => println!(
                "Transcript lang={} text={} ms={} utterance_to_transcript_ms={} wake_to_transcript_ms={}",
                spoken.lang,
                spoken.text,
                spoken.ms,
                done.duration_since(ready).as_millis(),
                wake_time.map_or(0, |wake| done.duration_since(wake).as_millis())
            ),
            Err(error) => println!("TranscriptError {error}"),
        }
    }
    println!("Summary wake_events={wakes} utterances={utterances}");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some(jarvis_voice::wake_worker::WORKER_FLAG) {
        let model_dir = arguments
            .get(1)
            .context("--wake-worker requires a model folder")?;
        std::process::exit(jarvis_voice::wake_worker::run(std::path::Path::new(
            model_dir,
        )));
    }
    if arguments.first().map(String::as_str) == Some(sysintel_cli::GPU_WORKER_FLAG) {
        std::process::exit(jarvis_sysintel::gpu::run_worker());
    }
    if arguments.first().map(String::as_str) == Some("--voice-wav") {
        return run_voice_wav(&arguments).await;
    }
    if arguments.first().map(String::as_str) == Some("--metrics") {
        return sysintel_cli::run_metrics(&arguments[1..]).await;
    }
    if arguments.first().map(String::as_str) == Some(vision_cli::FLAG) {
        return vision_cli::run(&arguments[1..]).await;
    }
    if arguments.first().map(String::as_str) == Some("--alerts-demo") {
        sysintel_cli::run_alerts_demo();
        return Ok(());
    }
    let interval = parse_selfstat_interval()?;
    let database = Database::open(Database::default_path()?).context("opening Jarvis database")?;
    init_logging(&database)?;
    tracing::info!(path = %database.path().display(), "jarvisd starting");
    database
        .audit_append(
            "core.start".into(),
            serde_json::json!({}),
            "safe".into(),
            true,
            "jarvisd started".into(),
        )
        .await?;

    let bus = EventBus::new();
    // sysintel replaces the Phase 1 monitor: it still publishes `Event::Metrics`, at the same
    // idle rate (0.2 Hz), and additionally watches the alert rules.
    // NVML (+20 MB private, cannot be unloaded) lives in a helper process, not in this one.
    let sysintel = Arc::new(sysintel_cli::new_service(
        bus.clone(),
        SysIntelConfig::default(),
    )?);
    let mut supervisor = Supervisor::new(vec![sysintel]);
    supervisor.start();
    let stats = interval.map(spawn_selfstat_logger);

    wait_for_shutdown().await?;
    tracing::info!("shutdown requested");

    // Nobody may be subscribed; that is fine.
    let _ = bus.publish(Event::Shutdown);
    if let Some(task) = stats {
        task.abort();
    }
    supervisor.stop().await?;
    database
        .audit_append(
            "core.stop".into(),
            serde_json::json!({}),
            "safe".into(),
            true,
            "jarvisd stopped".into(),
        )
        .await?;
    tracing::info!("jarvisd stopped");
    drop(database);
    Ok(())
}

/// Resolves on Ctrl+C, Ctrl+Break, console close, or system shutdown.
async fn wait_for_shutdown() -> anyhow::Result<()> {
    let mut brk = ctrl_break().context("registering Ctrl+Break")?;
    let mut close = ctrl_close().context("registering console close")?;
    let mut shutdown = ctrl_shutdown().context("registering shutdown")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("waiting for Ctrl+C")?,
        _ = brk.recv() => {}
        _ = close.recv() => {}
        _ = shutdown.recv() => {}
    }
    Ok(())
}

fn parse_selfstat_interval() -> anyhow::Result<Option<Duration>> {
    let mut arguments = env::args().skip(1);
    match arguments.next() {
        None => Ok(None),
        Some(flag) if flag == "--selfstat-interval" => {
            let seconds: u64 = arguments
                .next()
                .context("--selfstat-interval requires seconds")?
                .parse()
                .context("invalid selfstat interval")?;
            if arguments.next().is_some() || seconds == 0 {
                anyhow::bail!(USAGE);
            }
            Ok(Some(Duration::from_secs(seconds)))
        }
        _ => anyhow::bail!(USAGE),
    }
}

fn spawn_selfstat_logger(interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut previous = selfstat::sample().ok().map(|stat| stat.cpu_100ns);
        loop {
            tokio::time::sleep(interval).await;
            match selfstat::sample() {
                Ok(stat) => {
                    // FILETIME ticks are 100 ns; CPU% is relative to one core.
                    let cpu_percent = previous.map_or(0.0, |before| {
                        stat.cpu_100ns.saturating_sub(before) as f64
                            / (interval.as_secs_f64() * 1e7)
                            * 100.0
                    });
                    previous = Some(stat.cpu_100ns);
                    tracing::info!(
                        private_bytes = stat.private_bytes,
                        working_set = stat.working_set,
                        cpu_ms_total = stat.cpu_100ns / 10_000,
                        cpu_percent = format_args!("{cpu_percent:.3}"),
                        "selfstat"
                    );
                }
                Err(error) => tracing::warn!(%error, "selfstat unavailable"),
            }
        }
    })
}

fn init_logging(database: &Database) -> anyhow::Result<()> {
    let log_directory = database.logs_dir()?;
    std::fs::create_dir_all(&log_directory)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_directory.join("jarvisd.log"))?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initializing logging: {error}"))
}
