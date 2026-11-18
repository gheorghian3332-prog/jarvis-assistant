//! Piper through the official `piper.exe`, kept warm between phrases.
//!
//! The process is started with `--json_input`: every phrase is one JSON line
//! `{"text": ..., "output_file": <wav>}` on stdin and the answer is that path on stdout. After
//! `idle_timeout` without requests the process exits. Piper takes the speed (`--length_scale`) only
//! at start, so a different speed or voice restarts the process.

use crate::{
    clip::AudioClip,
    config::{is_valid_voice_id, models_dir, temp_dir},
    engine::TtsEngine,
    error::{TtsError, TtsResult},
    text::Lang,
};
use async_trait::async_trait;
use jarvis_core::worker::job::JobObject;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, Mutex},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};
use tracing::{debug, info, warn};

const ENGINE: &str = "piper";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STDERR_TAIL_LINES: usize = 8;

#[derive(Debug, Clone)]
pub struct PiperConfig {
    /// `piper.exe` (its folder holds the DLLs and `espeak-ng-data`).
    pub exe: PathBuf,
    /// Folder with `<voice>.onnx` and `<voice>.onnx.json`.
    pub voices_dir: PathBuf,
    /// Folder for the WAV files and the compatibility configs.
    pub tmp_dir: PathBuf,
    /// The process exits after this long without a phrase.
    pub idle_timeout: Duration,
    /// Limit for one phrase when the process is already running.
    pub request_timeout: Duration,
    /// Limit for a phrase that has to start the process first.
    pub cold_timeout: Duration,
}

impl PiperConfig {
    /// `%LOCALAPPDATA%\Jarvis\models\piper\piper\piper.exe` and the voices next to it.
    pub fn default_paths() -> Self {
        let base = models_dir().join("piper");
        Self {
            exe: base.join("piper").join("piper.exe"),
            voices_dir: base,
            tmp_dir: temp_dir(),
            idle_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(20),
            cold_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    voice: String,
    /// `length_scale` in hundredths (1.00 = 100).
    length_scale: u32,
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    key: Key,
    stderr_tail: Arc<StdMutex<VecDeque<String>>>,
    tasks: Vec<JoinHandle<()>>,
    _job: Option<JobObject>,
}

impl Process {
    fn pid(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn tail(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" | "))
            .unwrap_or_default()
    }

    /// Closes stdin (Piper exits at end of input) and kills the process if it does not.
    async fn stop(mut self) {
        drop(self.stdin);
        if timeout(Duration::from_millis(800), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    async fn kill(mut self) {
        let _ = self.child.kill().await;
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

struct State {
    process: Option<Process>,
    last_used: Instant,
    generation: u64,
    idle_task: Option<JoinHandle<()>>,
}

impl State {
    fn clear_idle_task(&mut self) {
        self.generation += 1;
        if let Some(task) = self.idle_task.take() {
            task.abort();
        }
    }
}

struct Inner {
    config: PiperConfig,
    state: Mutex<State>,
    counter: AtomicU64,
}

/// Warm Piper process behind the [`TtsEngine`] trait.
#[derive(Clone)]
pub struct PiperEngine {
    inner: Arc<Inner>,
}

impl PiperEngine {
    pub fn new(config: PiperConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                state: Mutex::new(State {
                    process: None,
                    last_used: Instant::now(),
                    generation: 0,
                    idle_task: None,
                }),
                counter: AtomicU64::new(0),
            }),
        }
    }

    /// Ids of the voices installed in the voices folder (`<id>.onnx` with its `.onnx.json`).
    pub fn installed_voices(&self) -> Vec<String> {
        installed_voices(&self.inner.config.voices_dir)
    }

    pub fn exe_present(&self) -> bool {
        self.inner.config.exe.is_file()
    }

    /// True while a Piper process is alive.
    pub async fn is_running(&self) -> bool {
        let mut state = self.inner.state.lock().await;
        state.process.as_mut().is_some_and(Process::is_running)
    }

    /// Process id of the running Piper, if any.
    pub async fn pid(&self) -> Option<u32> {
        let state = self.inner.state.lock().await;
        state.process.as_ref().map(Process::pid)
    }

    /// Stops the process now.
    pub async fn stop(&self) {
        let mut state = self.inner.state.lock().await;
        state.clear_idle_task();
        if let Some(process) = state.process.take() {
            process.stop().await;
        }
    }
}

/// Voices (`<id>.onnx` plus `<id>.onnx.json`) found in `dir`, sorted.
pub fn installed_voices(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut voices: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".onnx")?;
            (is_valid_voice_id(id) && dir.join(format!("{id}.onnx.json")).is_file())
                .then(|| id.to_owned())
        })
        .collect();
    voices.sort();
    voices
}

/// A copy of the voice configuration without the multi-codepoint phoneme keys that the last Windows
/// build of Piper (2023.11.14-2) rejects ("is not a single codepoint"). The model is untouched.
pub fn compat_config(original: &Path, target: &Path) -> TtsResult<()> {
    let text = std::fs::read_to_string(original).map_err(|error| {
        TtsError::unavailable(ENGINE, format!("{}: {error}", original.display()))
    })?;
    let mut config: Value = serde_json::from_str(&text).map_err(|error| {
        TtsError::unavailable(ENGINE, format!("{}: {error}", original.display()))
    })?;
    if let Some(map) = config
        .get_mut("phoneme_id_map")
        .and_then(Value::as_object_mut)
    {
        map.retain(|key, _| key.chars().count() == 1);
    }
    let output = serde_json::to_vec(&config)
        .map_err(|error| TtsError::synthesis(ENGINE, error.to_string()))?;
    std::fs::write(target, output)
        .map_err(|error| TtsError::synthesis(ENGINE, format!("{}: {error}", target.display())))
}

fn length_scale_hundredths(speed: f32) -> u32 {
    (100.0 / speed.clamp(0.5, 2.0)).round() as u32
}

fn same_file(line: &str, wanted: &Path) -> bool {
    let reported = Path::new(line.trim());
    reported == wanted
        || reported
            .file_name()
            .is_some_and(|name| Some(name) == wanted.file_name())
}

impl Inner {
    async fn spawn_process(&self, key: Key) -> TtsResult<Process> {
        let config = &self.config;
        if !config.exe.is_file() {
            return Err(TtsError::unavailable(
                ENGINE,
                format!(
                    "{} not found (run scripts/fetch-models.ps1 -Piper)",
                    config.exe.display()
                ),
            ));
        }
        let model = config.voices_dir.join(format!("{}.onnx", key.voice));
        let original = config.voices_dir.join(format!("{}.onnx.json", key.voice));
        if !model.is_file() || !original.is_file() {
            return Err(TtsError::unavailable(
                ENGINE,
                format!(
                    "voice {} is not installed in {}",
                    key.voice,
                    config.voices_dir.display()
                ),
            ));
        }
        std::fs::create_dir_all(&config.tmp_dir)
            .map_err(|error| TtsError::synthesis(ENGINE, format!("temp folder: {error}")))?;
        let compat = config.tmp_dir.join(format!("{}.compat.json", key.voice));
        compat_config(&original, &compat)?;

        let mut command = Command::new(&config.exe);
        command
            .arg("--model")
            .arg(&model)
            .arg("--config")
            .arg(&compat)
            .arg("--json_input")
            .arg("--length_scale")
            .arg(format!("{:.2}", f64::from(key.length_scale) / 100.0))
            .arg("--sentence_silence")
            .arg("0.15")
            .current_dir(config.exe.parent().unwrap_or_else(|| Path::new(".")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        let mut child = command.spawn().map_err(|error| {
            TtsError::unavailable(ENGINE, format!("cannot start piper: {error}"))
        })?;
        let job = JobObject::adopt(&child);
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill().await;
            return Err(TtsError::synthesis(ENGINE, "pipes are unavailable"));
        };
        let (sender, lines) = mpsc::channel(32);
        let tail = Arc::new(StdMutex::new(VecDeque::new()));
        let tasks = vec![
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    if sender.send(line).await.is_err() {
                        return;
                    }
                }
            }),
            tokio::spawn({
                let tail = Arc::clone(&tail);
                async move {
                    let mut reader = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        debug!("piper: {line}");
                        if let Ok(mut tail) = tail.lock() {
                            if tail.len() == STDERR_TAIL_LINES {
                                tail.pop_front();
                            }
                            tail.push_back(line);
                        }
                    }
                }
            }),
        ];
        info!(pid = child.id().unwrap_or(0), voice = %key.voice, "piper started");
        Ok(Process {
            child,
            stdin,
            lines,
            key,
            stderr_tail: tail,
            tasks,
            _job: job,
        })
    }

    async fn run(
        self: &Arc<Self>,
        text: String,
        voice: String,
        speed: f32,
    ) -> TtsResult<AudioClip> {
        let key = Key {
            voice,
            length_scale: length_scale_hundredths(speed),
        };
        let mut state = self.state.lock().await;
        if state
            .process
            .as_mut()
            .is_some_and(|process| process.key != key || !process.is_running())
        {
            if let Some(old) = state.process.take() {
                old.stop().await;
            }
        }
        let cold = state.process.is_none();
        if cold {
            let process = self.spawn_process(key).await?;
            state.process = Some(process);
        }
        state.clear_idle_task();
        let generation = state.generation;
        state.idle_task = Some(tokio::spawn(idle_watch(Arc::clone(self), generation)));

        let number = self.counter.fetch_add(1, Ordering::Relaxed);
        let output = self
            .config
            .tmp_dir
            .join(format!("piper-{}-{number}.wav", std::process::id()));
        let limit = if cold {
            self.config.cold_timeout
        } else {
            self.config.request_timeout
        };
        let outcome = match state.process.as_mut() {
            Some(process) => exchange(process, &text, &output, limit).await,
            None => Err(TtsError::synthesis(ENGINE, "process is not running")),
        };
        state.last_used = Instant::now();
        let outcome = match outcome {
            Ok(()) => AudioClip::read_wav(&output),
            Err(error) => {
                if let Some(process) = state.process.take() {
                    let tail = process.tail();
                    if !tail.is_empty() {
                        warn!(%tail, "piper failed");
                    }
                    process.kill().await;
                }
                Err(error)
            }
        };
        let _ = std::fs::remove_file(&output);
        let mut clip = outcome?;
        clip.trim_silence(120, 80);
        Ok(clip)
    }
}

/// Sends one phrase and waits for Piper to report its output file.
async fn exchange(
    process: &mut Process,
    text: &str,
    output: &Path,
    limit: Duration,
) -> TtsResult<()> {
    // Lines left over from an earlier phrase that was abandoned.
    while process.lines.try_recv().is_ok() {}
    let request = json!({ "text": text, "output_file": output });
    let mut line = request.to_string();
    line.push('\n');
    process
        .stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| TtsError::synthesis(ENGINE, format!("cannot write to piper: {error}")))?;
    process
        .stdin
        .flush()
        .await
        .map_err(|error| TtsError::synthesis(ENGINE, format!("cannot write to piper: {error}")))?;
    let wait = async {
        loop {
            match process.lines.recv().await {
                Some(reported) if same_file(&reported, output) => return Ok(()),
                Some(other) => debug!(line = %other, "piper: unexpected output"),
                None => {
                    return Err(TtsError::synthesis(
                        ENGINE,
                        format!("piper exited: {}", process.tail()),
                    ))
                }
            }
        }
    };
    match timeout(limit, wait).await {
        Ok(result) => result,
        Err(_) => Err(TtsError::Timeout {
            engine: ENGINE,
            limit,
        }),
    }
}

/// Stops Piper once it has been idle for `idle_timeout`.
async fn idle_watch(inner: Arc<Inner>, generation: u64) {
    let limit = inner.config.idle_timeout;
    let mut wait = limit;
    loop {
        sleep(wait).await;
        let mut state = inner.state.lock().await;
        if state.generation != generation || state.process.is_none() {
            return;
        }
        let idle = state.last_used.elapsed();
        if idle < limit {
            wait = limit - idle;
            continue;
        }
        info!("idle timeout; stopping piper");
        state.generation += 1;
        state.idle_task = None;
        if let Some(process) = state.process.take() {
            process.stop().await;
        }
        return;
    }
}

#[async_trait]
impl TtsEngine for PiperEngine {
    fn name(&self) -> &'static str {
        ENGINE
    }

    async fn synth(
        &self,
        text: &str,
        _lang: Lang,
        voice: &str,
        speed: f32,
    ) -> TtsResult<AudioClip> {
        if !is_valid_voice_id(voice) {
            return Err(TtsError::Invalid(format!("bad Piper voice id `{voice}`")));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(TtsError::Invalid("empty text".into()));
        }
        let inner = Arc::clone(&self.inner);
        let (text, voice) = (text.to_owned(), voice.to_owned());
        // A separate task: once a request is written to Piper it is finished even if the caller
        // is cancelled, so the process never sees half a line.
        tokio::spawn(async move { inner.run(text, voice, speed).await })
            .await
            .map_err(|error| TtsError::synthesis(ENGINE, format!("task failed: {error}")))?
    }

    async fn shutdown(&self) {
        self.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_becomes_a_length_scale() {
        assert_eq!(length_scale_hundredths(1.0), 100);
        assert_eq!(length_scale_hundredths(1.25), 80);
        assert_eq!(length_scale_hundredths(0.7), 143);
        assert_eq!(length_scale_hundredths(1.4), 71);
    }

    #[test]
    fn compat_config_drops_only_multi_codepoint_phoneme_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = dir.path().join("v.onnx.json");
        std::fs::write(
            &original,
            r#"{"audio":{"sample_rate":22050},"phoneme_id_map":{"a":[1],"b":[2],"ai\u0361":[3],"xy":[4]},"num_speakers":1}"#,
        )
        .expect("write");
        let target = dir.path().join("compat.json");
        compat_config(&original, &target).expect("compat");
        let value: Value =
            serde_json::from_slice(&std::fs::read(&target).expect("read")).expect("json");
        let map = value["phoneme_id_map"].as_object().expect("map");
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("a") && map.contains_key("b"));
        assert_eq!(value["audio"]["sample_rate"], 22050);
    }

    #[test]
    fn compat_config_reports_unreadable_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(compat_config(&dir.path().join("none.json"), &dir.path().join("t.json")).is_err());
    }

    #[test]
    fn installed_voices_need_both_files_and_a_safe_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in [
            "ru_RU-a-medium.onnx",
            "ru_RU-a-medium.onnx.json",
            "lonely.onnx",
            "bad name.onnx",
            "bad name.onnx.json",
            "en_GB-b-low.onnx",
            "en_GB-b-low.onnx.json",
        ] {
            std::fs::write(dir.path().join(name), b"x").expect("write");
        }
        assert_eq!(
            installed_voices(dir.path()),
            ["en_GB-b-low", "ru_RU-a-medium"]
        );
        assert!(installed_voices(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn reported_paths_match_by_file_name() {
        let wanted = Path::new(r"C:\tmp\p-1.wav");
        assert!(same_file("C:\\tmp\\p-1.wav\r", wanted));
        assert!(same_file("p-1.wav", wanted));
        assert!(!same_file(r"C:\tmp\p-2.wav", wanted));
    }

    #[tokio::test]
    async fn missing_program_or_voice_is_reported_as_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = PiperEngine::new(PiperConfig {
            exe: dir.path().join("piper.exe"),
            voices_dir: dir.path().to_path_buf(),
            tmp_dir: dir.path().join("tmp"),
            idle_timeout: Duration::from_secs(1),
            request_timeout: Duration::from_secs(1),
            cold_timeout: Duration::from_secs(1),
        });
        let error = engine
            .synth("Привет", Lang::Ru, "ru_RU-a-medium", 1.0)
            .await
            .expect_err("no piper");
        assert!(matches!(error, TtsError::Unavailable { .. }), "{error}");
        assert!(engine.synth("x", Lang::Ru, "../evil", 1.0).await.is_err());
        assert!(engine.synth("  ", Lang::Ru, "ru_RU-a", 1.0).await.is_err());
        assert!(!engine.exe_present());
        assert!(!engine.is_running().await);
    }
}
