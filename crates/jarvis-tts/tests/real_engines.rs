//! Real engines (Piper, Kokoro) and the real STT worker. Slow, so every test is `#[ignore]`.
//!
//! Nothing is played: speech goes to WAV files (`OutputSink::Wav`).
//!
//! Run with
//! `cargo test -p jarvis-tts --test real_engines -- --ignored --nocapture --test-threads=1`
//! (the models must be installed: `scripts/fetch-models.ps1 -Piper -Kokoro`; the Kokoro and STT
//! workers need `uv sync` in `workers/tts_kokoro` and `workers/stt`).

use jarvis_core::worker::{SttConfig, SttService, WorkerCommand};
use jarvis_tts::{
    clip::resample, kokoro, AudioClip, Engines, KokoroEngine, Lang, MemoryStore, OutputSink,
    PiperConfig, PiperEngine, SpeechObserver, SpeechOutcome, TtsConfig, TtsEngine, TtsService,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const RU_PHRASE: &str = "Добрый вечер. Все системы работают в штатном режиме.";
const EN_PHRASE: &str = "Good evening, sir. All systems are running normally.";
const RU_WORDS: [&str; 5] = ["добрый", "вечер", "системы", "работают", "режиме"];
const EN_WORDS: [&str; 5] = ["good", "evening", "sir", "systems", "running"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn out_dir() -> PathBuf {
    let dir = std::env::var_os("JARVIS_TTS_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("jarvis-tts-it"));
    std::fs::create_dir_all(&dir).expect("output folder");
    dir
}

fn piper_voice() -> String {
    std::env::var("JARVIS_TTS_RU_VOICE").unwrap_or_else(|_| jarvis_tts::DEFAULT_VOICE_RU.to_owned())
}

fn piper(idle: Duration) -> PiperEngine {
    let mut config = PiperConfig::default_paths();
    config.idle_timeout = idle;
    PiperEngine::new(config)
}

fn kokoro_engine(idle: Duration) -> KokoroEngine {
    KokoroEngine::with_idle_timeout(kokoro::worker_command(&repo_root()), idle)
}

/// Working set (MiB) and CPU (percent of one core) of a process and its children.
fn tree_usage(system: &System, root: u32) -> (u64, f32) {
    let mut memory = 0u64;
    let mut cpu = 0f32;
    for (pid, process) in system.processes() {
        let mut current = Some(*pid);
        let mut inside = false;
        while let Some(candidate) = current {
            if candidate.as_u32() == root {
                inside = true;
                break;
            }
            current = system.process(candidate).and_then(|p| p.parent());
        }
        if inside {
            memory += process.memory();
            cpu += process.cpu_usage();
        }
    }
    (memory / 1024 / 1024, cpu)
}

fn alive(pid: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::new(),
    );
    system.process(Pid::from_u32(pid)).is_some()
}

/// Samples RAM and CPU of `pid` while `work` runs; returns (peak MiB, average CPU %, peak CPU %).
async fn sample_while<F, T>(pid: u32, work: F) -> (T, u64, f32, f32)
where
    F: std::future::Future<Output = T>,
{
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let samples: Arc<Mutex<Vec<(u64, f32)>>> = Arc::new(Mutex::new(Vec::new()));
    let sampler = {
        let (stop, samples) = (Arc::clone(&stop), Arc::clone(&samples));
        tokio::task::spawn_blocking(move || {
            let mut system = System::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                system.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::new().with_cpu().with_memory(),
                );
                if let Ok(mut samples) = samples.lock() {
                    samples.push(tree_usage(&system, pid));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
    };
    let result = work.await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = sampler.await;
    let samples = samples.lock().expect("samples").clone();
    // The first CPU sample of a process is always 0: skip it.
    let useful: Vec<_> = samples.iter().skip(1).collect();
    let peak_mem = samples.iter().map(|s| s.0).max().unwrap_or(0);
    let peak_cpu = useful.iter().map(|s| s.1).fold(0.0, f32::max);
    let average = if useful.is_empty() {
        0.0
    } else {
        useful.iter().map(|s| s.1).sum::<f32>() / useful.len() as f32
    };
    (result, peak_mem, average, peak_cpu)
}

fn assert_audible(clip: &AudioClip, label: &str) {
    println!(
        "{label}: {:.2} s at {} Hz, peak {}, rms {:.3}",
        clip.duration_secs(),
        clip.sample_rate,
        clip.peak(),
        clip.rms()
    );
    assert!(clip.duration_secs() > 0.5, "{label}: too short");
    assert!(clip.peak() > 1_000, "{label}: silent");
    assert!(clip.rms() > 0.01, "{label}: silent");
}

fn keywords_found(text: &str, words: &[&str]) -> usize {
    let lower = text.to_lowercase().replace('ё', "е");
    words.iter().filter(|word| lower.contains(*word)).count()
}

#[tokio::test]
#[ignore = "real Piper: needs piper.exe and the voice"]
async fn piper_synthesizes_a_russian_phrase_and_exits_when_idle() {
    let engine = piper(Duration::from_secs(4));
    let voice = piper_voice();
    println!("voice: {voice}");

    let cold = Instant::now();
    let clip = engine
        .synth(RU_PHRASE, Lang::Ru, &voice, 1.0)
        .await
        .expect("cold synthesis");
    let cold_time = cold.elapsed();
    assert_audible(&clip, "piper ru");
    clip.write_wav(&out_dir().join("piper_ru.wav"))
        .expect("wav");
    let pid = engine.pid().await.expect("piper is running");

    let warm = Instant::now();
    let again = engine
        .synth(RU_PHRASE, Lang::Ru, &voice, 1.0)
        .await
        .expect("warm synthesis");
    let warm_time = warm.elapsed();
    assert_audible(&again, "piper ru (warm)");
    println!(
        "piper: cold start + synthesis {cold_time:?}, warm synthesis {warm_time:?} for {:.2} s of audio",
        clip.duration_secs()
    );
    assert_eq!(engine.pid().await, Some(pid), "the same process is reused");

    // Another speed restarts the process (Piper takes length_scale at start).
    let slower = engine
        .synth(RU_PHRASE, Lang::Ru, &voice, 0.8)
        .await
        .expect("slower");
    assert!(
        slower.duration_secs() > clip.duration_secs() * 1.15,
        "slower speech is longer"
    );

    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(!engine.is_running().await, "idle timeout stopped piper");
    let still_alive = alive(pid);
    println!(
        "piper idle exit: running={}, old pid {pid} alive={still_alive}",
        engine.is_running().await
    );
    assert!(!still_alive);
}

#[tokio::test]
#[ignore = "real Kokoro: needs workers/tts_kokoro (uv sync) and the model files"]
async fn kokoro_synthesizes_an_english_phrase_and_exits_when_idle() {
    let engine = kokoro_engine(Duration::from_secs(6));
    let cold = Instant::now();
    let clip = engine
        .synth(EN_PHRASE, Lang::En, "bm_george", 1.0)
        .await
        .expect("cold synthesis");
    let cold_time = cold.elapsed();
    assert_audible(&clip, "kokoro en");
    assert_eq!(clip.sample_rate, 24_000);
    clip.write_wav(&out_dir().join("kokoro_en.wav"))
        .expect("wav");
    let pid = engine.pid().await.expect("worker is running");

    let warm = Instant::now();
    let again = engine
        .synth(EN_PHRASE, Lang::En, "bm_george", 1.0)
        .await
        .expect("warm synthesis");
    let warm_time = warm.elapsed();
    assert_audible(&again, "kokoro en (warm)");
    println!(
        "kokoro: cold start + synthesis {cold_time:?}, warm synthesis {warm_time:?} for {:.2} s of audio",
        clip.duration_secs()
    );

    let lewis = engine
        .synth("Good evening.", Lang::En, "bm_lewis", 1.0)
        .await
        .expect("alternative voice");
    assert_audible(&lewis, "kokoro en bm_lewis");
    assert!(engine
        .synth("Hello", Lang::En, "bm_nobody", 1.0)
        .await
        .is_err());

    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(
        !engine.is_running().await,
        "idle timeout stopped the worker"
    );
    let still_alive = alive(pid);
    println!(
        "kokoro idle exit: running={}, old pid {pid} alive={still_alive}",
        engine.is_running().await
    );
    assert!(!still_alive);
}

struct Silent;

impl SpeechObserver for Silent {
    fn speaking(&self, _active: bool) {}
    fn level(&self, _rms: f32) {}
}

fn service(wav: PathBuf, idle: Duration) -> (TtsService, PiperEngine, KokoroEngine) {
    let piper = piper(idle);
    let kokoro = kokoro_engine(idle);
    let engines = Engines {
        piper: Arc::new(piper.clone()),
        kokoro: Arc::new(kokoro.clone()),
        cloud: None,
    };
    let service = TtsService::with_options(
        engines,
        Arc::new(MemoryStore::default()),
        Arc::new(Silent),
        TtsConfig {
            voice_ru: piper_voice(),
            volume: 0.9,
            ..TtsConfig::default()
        },
        Some(OutputSink::Wav { path: wav }),
        Duration::from_millis(300),
    );
    (service, piper, kokoro)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real Piper and Kokoro, plays into WAV files"]
async fn pipeline_speaks_with_real_engines_and_interrupts_within_100_ms() {
    let (service, piper, kokoro) =
        service(out_dir().join("pipeline.wav"), Duration::from_secs(120));
    service.set_level_enabled(true);

    // Russian, cold then warm.
    let cold = service.speak_and_wait(RU_PHRASE).await.expect("speak");
    assert_eq!(cold.outcome, SpeechOutcome::Completed, "{cold:?}");
    println!(
        "RU cold: first audio after {:?}, {} sentences, {:.1} s of audio, engines {:?}",
        cold.first_audio.expect("audio"),
        cold.sentences,
        cold.audio_secs,
        cold.engines
    );
    let warm = service.speak_and_wait(RU_PHRASE).await.expect("speak");
    println!(
        "RU warm: first audio after {:?}",
        warm.first_audio.expect("audio")
    );
    assert_audible(
        &AudioClip::read_wav(&out_dir().join("pipeline.wav")).expect("wav"),
        "pipeline ru",
    );

    // English, cold then warm.
    let cold = service.speak_and_wait(EN_PHRASE).await.expect("speak");
    assert_eq!(cold.outcome, SpeechOutcome::Completed, "{cold:?}");
    println!(
        "EN cold: first audio after {:?}, engines {:?}",
        cold.first_audio.expect("audio"),
        cold.engines
    );
    let warm = service.speak_and_wait(EN_PHRASE).await.expect("speak");
    println!(
        "EN warm: first audio after {:?}",
        warm.first_audio.expect("audio")
    );

    // Interruption of a long Russian speech.
    let long = "Это длинная фраза для проверки прерывания. Вторая фраза тоже довольно длинная. \
                Третья фраза не должна прозвучать до конца.";
    let handle = service.speak(long).expect("speak");
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(service.is_speaking(), "speech is audible by now");
    let started = Instant::now();
    service.stop();
    let report = handle.wait().await;
    let latency = started.elapsed();
    println!("interrupt: stop() to finished report {latency:?}");
    assert_eq!(report.outcome, SpeechOutcome::Interrupted);
    assert!(latency <= Duration::from_millis(100), "{latency:?}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!service.is_speaking());
    let played = AudioClip::read_wav(&out_dir().join("pipeline.wav")).expect("wav");
    assert!(played.duration_secs() < 10.0);

    jarvis_core::Service::stop(&service).await.expect("stop");
    assert!(!piper.is_running().await);
    assert!(!kokoro.is_running().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real Piper and Kokoro: RAM and CPU while speaking"]
async fn resource_use_while_speaking() {
    let long_ru = format!("{RU_PHRASE} {RU_PHRASE} {RU_PHRASE}");
    let long_en = format!("{EN_PHRASE} {EN_PHRASE} {EN_PHRASE}");
    let (service, piper, kokoro) =
        service(out_dir().join("resources.wav"), Duration::from_secs(120));
    // Warm both engines first.
    service.speak_and_wait("Проверка.").await.expect("warm ru");
    service.speak_and_wait("Check.").await.expect("warm en");
    let piper_pid = piper.pid().await.expect("piper pid");
    let kokoro_pid = kokoro.pid().await.expect("kokoro pid");

    let (report, memory, average, peak) =
        sample_while(piper_pid, async { service.speak_and_wait(&long_ru).await }).await;
    let report = report.expect("speak");
    println!(
        "piper during speech ({:.1} s of audio): RAM {memory} MiB, CPU avg {average:.1}% peak {peak:.1}% of one core",
        report.audio_secs
    );
    let (report, memory, average, peak) =
        sample_while(kokoro_pid, async { service.speak_and_wait(&long_en).await }).await;
    let report = report.expect("speak");
    println!(
        "kokoro during speech ({:.1} s of audio): RAM {memory} MiB, CPU avg {average:.1}% peak {peak:.1}% of one core",
        report.audio_secs
    );
    jarvis_core::Service::stop(&service).await.expect("stop");
}

fn stt() -> SttService {
    let python = repo_root().join("workers/stt/.venv/Scripts/python.exe");
    let command = if python.is_file() {
        WorkerCommand::python_module(python, "jarvis_stt", Some(repo_root().join("workers/stt")))
    } else {
        WorkerCommand::dev_stt(&repo_root())
    };
    let mut config = SttConfig::new(command);
    config.first_hello_timeout = Duration::from_secs(300);
    SttService::new(config)
}

fn to_16k(clip: &AudioClip) -> Vec<i16> {
    resample(&clip.to_f32(), clip.sample_rate, 16_000)
        .into_iter()
        .map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

#[tokio::test]
#[ignore = "real Piper, Kokoro and the Whisper STT worker"]
async fn synthesized_speech_is_recognized_by_the_stt_worker() {
    let piper = piper(Duration::from_secs(60));
    let kokoro = kokoro_engine(Duration::from_secs(60));
    let stt = stt();

    let ru = piper
        .synth(RU_PHRASE, Lang::Ru, &piper_voice(), 1.0)
        .await
        .expect("ru");
    let ru_text = stt.transcribe(&to_16k(&ru), None).await.expect("stt ru");
    println!(
        "RU round trip: spoken {RU_PHRASE:?} -> heard {:?} (lang {})",
        ru_text.text, ru_text.lang
    );
    let ru_hits = keywords_found(&ru_text.text, &RU_WORDS);
    assert!(ru_hits >= 4, "{ru_hits}/5 keywords in {:?}", ru_text.text);
    assert_eq!(ru_text.lang, "ru");

    let en = kokoro
        .synth(EN_PHRASE, Lang::En, "bm_george", 1.0)
        .await
        .expect("en");
    let en_text = stt.transcribe(&to_16k(&en), None).await.expect("stt en");
    println!(
        "EN round trip: spoken {EN_PHRASE:?} -> heard {:?} (lang {})",
        en_text.text, en_text.lang
    );
    let en_hits = keywords_found(&en_text.text, &EN_WORDS);
    assert!(en_hits >= 4, "{en_hits}/5 keywords in {:?}", en_text.text);
    assert_eq!(en_text.lang, "en");

    jarvis_core::Service::stop(&stt).await.expect("stop stt");
    piper.shutdown().await;
    kokoro.shutdown().await;
}

/// Compares the installed Russian voices with the same STT round trip; prints a table.
#[tokio::test]
#[ignore = "real Piper (several voices) and the Whisper STT worker"]
async fn compare_russian_voices() {
    let stt = stt();
    let phrases = [
        "Добрый вечер. Все системы работают в штатном режиме.",
        "Открой браузер и включи музыку.",
        "Поставь будильник на семь утра и закрой все окна.",
        "Какая сегодня погода в Брюсселе, и нужен ли мне зонт?",
        "Напомни мне позвонить маме после обеда.",
        "Громкость уменьшена, режим не беспокоить включён.",
    ];
    for voice in [
        "ru_RU-dmitri-medium",
        "ru_RU-ruslan-medium",
        "ru_RU-denis-medium",
    ] {
        let engine = piper(Duration::from_secs(60));
        let mut audio_secs = 0.0;
        let mut synth_secs = 0.0;
        let mut matched = 0usize;
        let mut total = 0usize;
        for phrase in phrases {
            let started = Instant::now();
            let clip = match engine.synth(phrase, Lang::Ru, voice, 1.0).await {
                Ok(clip) => clip,
                Err(error) => {
                    println!("{voice}: not usable ({error})");
                    break;
                }
            };
            synth_secs += started.elapsed().as_secs_f32();
            audio_secs += clip.duration_secs();
            let heard = stt
                .transcribe(&to_16k(&clip), Some("ru"))
                .await
                .expect("stt");
            let expected: Vec<String> = phrase
                .to_lowercase()
                .replace('ё', "е")
                .split(|c: char| !c.is_alphabetic())
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect();
            let heard_text = heard.text.to_lowercase().replace('ё', "е");
            total += expected.len();
            matched += expected
                .iter()
                .filter(|w| heard_text.contains(w.as_str()))
                .count();
            println!("  {voice}: {phrase:?} -> {:?}", heard.text);
            let _ = clip.write_wav(&out_dir().join(format!("compare_{voice}.wav")));
        }
        println!(
            "{voice}: word match {matched}/{total}, synthesis speed {:.1}x real time",
            audio_secs / synth_secs.max(0.001)
        );
        engine.shutdown().await;
    }
    jarvis_core::Service::stop(&stt).await.expect("stop stt");
}

#[derive(Default)]
struct Levels(Mutex<Vec<f32>>);

impl SpeechObserver for Levels {
    fn speaking(&self, _active: bool) {}
    fn level(&self, rms: f32) {
        if let Ok(mut levels) = self.0.lock() {
            levels.push(rms);
        }
    }
}

/// The real audio device path (cpal callback, channel expansion), at a volume of 0.02 (about -34 dB).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "plays very quietly (volume 0.02) on the default output device"]
async fn device_output_works_at_whisper_volume() {
    let observer = Arc::new(Levels::default());
    let piper = piper(Duration::from_secs(60));
    let engines = Engines {
        piper: Arc::new(piper.clone()),
        kokoro: Arc::new(KokoroEngine::unavailable("not needed")),
        cloud: None,
    };
    let service = TtsService::with_options(
        engines,
        Arc::new(MemoryStore::default()),
        observer.clone(),
        TtsConfig {
            voice_ru: piper_voice(),
            volume: 0.02,
            ..TtsConfig::default()
        },
        None,
        Duration::from_millis(300),
    );
    service.set_level_enabled(true);
    let handle = service
        .speak("Раз. Два. Три. Четыре. Пять. Шесть. Семь. Восемь.")
        .expect("speak");
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let before = observer.0.lock().expect("levels").len();
    let started = Instant::now();
    service.stop();
    let report = handle.wait().await;
    assert_eq!(report.outcome, SpeechOutcome::Interrupted);
    let levels = observer.0.lock().expect("levels").clone();
    println!(
        "device: first audio {:?}, {} level windows, max level {:.4} (volume 0.02), stop took {:?}",
        report.first_audio,
        levels.len(),
        levels.iter().copied().fold(0.0, f32::max),
        started.elapsed()
    );
    assert!(before > 30, "the callback delivered windows: {before}");
    assert!(
        levels.iter().copied().fold(0.0, f32::max) <= 0.02 * 1.01,
        "never above the volume"
    );
    assert_eq!(levels.last(), Some(&0.0));
    piper.shutdown().await;
}
