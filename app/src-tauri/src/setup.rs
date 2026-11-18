//! First-run wizard backend: what is installed, downloads with SHA256 and progress, worker
//! environments (`uv sync --frozen`), the Whisper cache and the Ollama model.
//!
//! Everything is repeatable from the settings; a finished item is skipped. Downloads come only from the
//! official sources pinned below and are accepted only when the SHA256 matches. Nothing is installed
//! outside the user's profile, and Ollama itself is never installed (the wizard shows where to get it).

use crate::{error::CommandError, state::AppState};
use jarvis_core::{layout, WorkerCommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

/// Event with `Progress` payloads.
pub const PROGRESS_EVENT: &str = "setup://progress";
/// Where the wizard sends the user when Ollama is missing.
pub const OLLAMA_DOWNLOAD_URL: &str = "https://ollama.com/download";
/// The model the "download model" button fetches.
pub const DEFAULT_OLLAMA_MODEL: &str = "qwen3:4b";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A pinned official download.
#[derive(Debug, Clone, Copy)]
pub struct Download {
    pub url: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
}

const VOSK_MODEL: Download = Download {
    url: "https://alphacephei.com/vosk/models/vosk-model-small-ru-0.22.zip",
    file: "vosk-model-small-ru-0.22.zip",
    sha256: "961D5FF98A17F4AA6DE69864D0AA71FA5BAC682301D2B5D17A3F24C5C99A46D4",
};
const PIPER: Download = Download {
    url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
    file: "piper_windows_amd64.zip",
    sha256: "F3C58906402B24F3A96D92145F58ACBA6D86C9B5DB896D207F78DC80811EFCEA",
};
const PIPER_VOICE: Download = Download {
    url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/dmitri/medium/ru_RU-dmitri-medium.onnx",
    file: "ru_RU-dmitri-medium.onnx",
    sha256: "9ED9941BAC7373F2EC1BE36627E0E71A349A59CA5522D4363FD50701DB05193A",
};
const PIPER_VOICE_JSON: Download = Download {
    url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/ru/ru_RU/dmitri/medium/ru_RU-dmitri-medium.onnx.json",
    file: "ru_RU-dmitri-medium.onnx.json",
    sha256: "C3741C9C1558FC38E902FD80D39155C32A7D019D7D17E3A43EAC66B7BD3FBE09",
};
const KOKORO_MODEL: Download = Download {
    url: "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/kokoro-v1.0.onnx",
    file: "kokoro-v1.0.onnx",
    sha256: "BEB0D1848DEE9A49DA392CC3DF26958D46CFA35D321EDF434F52949153F0DF3A",
};
const KOKORO_VOICES: Download = Download {
    url: "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/voices-v1.0.bin",
    file: "voices-v1.0.bin",
    sha256: "BCA610B8308E8D99F32E6FE4197E7EC01679264EFED0CAC9140FE9C29F1FBF7D",
};

/// One line of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub id: &'static str,
    pub optional: bool,
    /// Rough size for the UI, in megabytes.
    pub approx_mb: u32,
}

pub const ITEMS: [Item; 7] = [
    Item {
        id: "vosk_model",
        optional: false,
        approx_mb: 45,
    },
    Item {
        id: "piper",
        optional: false,
        approx_mb: 22,
    },
    Item {
        id: "piper_voice",
        optional: false,
        approx_mb: 64,
    },
    Item {
        id: "stt_env",
        optional: false,
        approx_mb: 1500,
    },
    Item {
        id: "whisper",
        optional: false,
        approx_mb: 470,
    },
    Item {
        id: "kokoro",
        optional: true,
        approx_mb: 354,
    },
    Item {
        id: "kokoro_env",
        optional: true,
        approx_mb: 200,
    },
];

/// The item with this id, if there is one.
pub fn item(id: &str) -> Option<Item> {
    ITEMS.iter().copied().find(|candidate| candidate.id == id)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ItemStatus {
    pub id: &'static str,
    pub installed: bool,
    pub optional: bool,
    pub approx_mb: u32,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SetupStatus {
    pub items: Vec<ItemStatus>,
    /// `resources` next to `jarvis.exe` carries the worker sources (an installed copy).
    pub installed_layout: bool,
    pub models_dir: String,
    pub ollama_url: &'static str,
    pub default_model: &'static str,
}

/// `setup://progress`: `state` is `running`, `done` or `error`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Progress {
    pub item: String,
    pub state: &'static str,
    pub done: u64,
    pub total: u64,
    pub message: String,
}

fn running_items() -> &'static Mutex<HashSet<&'static str>> {
    static RUNNING: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_running(id: &str) -> bool {
    crate::state::lock(running_items()).contains(id)
}

/// The development checkout that holds the worker (`workers/<name>/pyproject.toml`), if there is one.
fn dev_root(name: &str) -> Option<PathBuf> {
    let has_worker = |dir: &Path| {
        dir.join("workers")
            .join(name)
            .join("pyproject.toml")
            .is_file()
    };
    if let Some(root) = std::env::var_os(jarvis_voice::router::REPO_ROOT_ENV).map(PathBuf::from) {
        return has_worker(&root).then_some(root);
    }
    if layout::resources_dir().is_some() {
        return None;
    }
    std::env::current_exe()
        .ok()?
        .ancestors()
        .skip(1)
        .find(|dir| has_worker(dir))
        .map(Path::to_path_buf)
}

/// Where `uv sync` runs for a worker: the repository in development, the copy in
/// `%LOCALAPPDATA%\Jarvis\workers\<name>` after an install.
fn sync_dir(name: &str) -> Result<PathBuf, String> {
    if let Some(root) = dev_root(name) {
        return Ok(root.join("workers").join(name));
    }
    layout::sync_worker_sources(name).map_err(|error| error.to_string())
}

fn env_ready(name: &str) -> bool {
    layout::venv_python(name).is_file()
        || dev_root(name).is_some_and(|root| {
            root.join("workers")
                .join(name)
                .join(".venv")
                .join("Scripts")
                .join("python.exe")
                .is_file()
        })
}

fn whisper_ready(models: &Path) -> bool {
    let snapshots = models
        .join("whisper")
        .join("models--Systran--faster-whisper-small")
        .join("snapshots");
    std::fs::read_dir(snapshots).is_ok_and(|mut entries| {
        entries.any(|entry| entry.is_ok_and(|entry| entry.path().join("model.bin").is_file()))
    })
}

fn is_installed(id: &str, models: &Path) -> bool {
    match id {
        "vosk_model" => models
            .join("vosk-model-small-ru-0.22")
            .join("am")
            .join("final.mdl")
            .is_file(),
        "piper" => models
            .join("piper")
            .join("piper")
            .join("piper.exe")
            .is_file(),
        "piper_voice" => {
            models.join("piper").join(PIPER_VOICE.file).is_file()
                && models.join("piper").join(PIPER_VOICE_JSON.file).is_file()
        }
        "stt_env" => env_ready("stt"),
        "whisper" => whisper_ready(models),
        "kokoro" => jarvis_tts::kokoro::model_installed(),
        "kokoro_env" => env_ready("tts_kokoro"),
        _ => false,
    }
}

/// What is installed, for the wizard (blocking file system checks).
pub fn status() -> SetupStatus {
    let models = jarvis_tts::models_dir();
    SetupStatus {
        items: ITEMS
            .iter()
            .map(|entry| ItemStatus {
                id: entry.id,
                installed: is_installed(entry.id, &models),
                optional: entry.optional,
                approx_mb: entry.approx_mb,
                running: is_running(entry.id),
            })
            .collect(),
        installed_layout: layout::resources_dir().is_some(),
        models_dir: models.display().to_string(),
        ollama_url: OLLAMA_DOWNLOAD_URL,
        default_model: DEFAULT_OLLAMA_MODEL,
    }
}

/// Reports progress to the UI. A missing window is fine.
#[derive(Clone)]
struct Reporter {
    app: AppHandle,
    item: String,
}

impl Reporter {
    fn send(&self, state: &'static str, done: u64, total: u64, message: impl Into<String>) {
        let payload = Progress {
            item: self.item.clone(),
            state,
            done,
            total,
            message: message.into(),
        };
        if let Err(error) = self.app.emit(PROGRESS_EVENT, payload) {
            tracing::debug!(%error, "setup progress was not delivered");
        }
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// SHA256 of a file, upper-case hex.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex_upper(&hasher.finalize()))
}

/// Downloads `download` into `dir` (verified against the pinned SHA256) and returns the file.
/// A file that is already there and matches is reused.
async fn download_verified(
    reporter: &Reporter,
    download: Download,
    dir: &Path,
) -> Result<PathBuf, String> {
    if !download.url.starts_with("https://") {
        return Err(format!("refusing a non-HTTPS source: {}", download.url));
    }
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let target = dir.join(download.file);
    if target.is_file() {
        let existing = target.clone();
        let hash = tokio::task::spawn_blocking(move || file_sha256(&existing))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if hash == download.sha256 {
            return Ok(target);
        }
        std::fs::remove_file(&target).map_err(|error| error.to_string())?;
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = client
        .get(download.url)
        .send()
        .await
        .map_err(|error| format!("{}: {error}", download.url))?;
    if !response.status().is_success() {
        return Err(format!("{}: HTTP {}", download.url, response.status()));
    }
    let total = response.content_length().unwrap_or(0);
    let partial = dir.join(format!("{}.part", download.file));
    let mut out = std::fs::File::create(&partial).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut done = 0_u64;
    let mut last_report = Instant::now();
    reporter.send("running", 0, total, format!("download {}", download.file));
    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(60), response.chunk())
            .await
            .map_err(|_| format!("{}: no data for 60 s", download.url))?
            .map_err(|error| format!("{}: {error}", download.url))?;
        let Some(chunk) = chunk else { break };
        std::io::Write::write_all(&mut out, &chunk).map_err(|error| error.to_string())?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        if last_report.elapsed() >= Duration::from_millis(250) {
            reporter.send(
                "running",
                done,
                total,
                format!("download {}", download.file),
            );
            last_report = Instant::now();
        }
    }
    drop(out);
    let actual = hex_upper(&hasher.finalize());
    if actual != download.sha256 {
        // A corrupted or substituted file is never kept.
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "SHA256 mismatch for {}: expected {}, got {actual}",
            download.file, download.sha256
        ));
    }
    std::fs::rename(&partial, &target).map_err(|error| error.to_string())?;
    reporter.send(
        "running",
        done,
        total.max(done),
        format!("verified {}", download.file),
    );
    Ok(target)
}

/// Extracts a zip into a scratch folder and moves it to `target`; a single top-level folder is
/// collapsed into `target`. Entries that would leave the folder are refused.
fn extract_zip(zip_path: &Path, scratch_parent: &Path, target: &Path) -> Result<(), String> {
    let scratch = scratch_parent.join(format!("extract-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        let file = std::fs::File::open(zip_path).map_err(|error| error.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            let Some(relative) = entry.enclosed_name() else {
                return Err(format!("unsafe path in archive: {}", entry.name()));
            };
            let destination = scratch.join(relative);
            if entry.is_dir() {
                std::fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
            } else {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let mut out =
                    std::fs::File::create(&destination).map_err(|error| error.to_string())?;
                std::io::copy(&mut entry, &mut out).map_err(|error| error.to_string())?;
            }
        }
        let roots: Vec<PathBuf> = std::fs::read_dir(&scratch)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        let source = match roots.as_slice() {
            [only] if only.is_dir() => only.clone(),
            _ => scratch.clone(),
        };
        if target.exists() {
            std::fs::remove_dir_all(target).map_err(|error| error.to_string())?;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&source, target).map_err(|error| error.to_string())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Runs a child process and forwards its output lines as progress messages.
fn run_streaming(reporter: &Reporter, mut command: Command) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    let streams: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let readers: Vec<_> = streams
        .into_iter()
        .map(|stream| {
            let reporter = reporter.clone();
            let tail = Arc::clone(&tail);
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let line = line.trim().to_owned();
                    if line.is_empty() {
                        continue;
                    }
                    reporter.send("running", 0, 0, line.clone());
                    let mut tail = crate::state::lock(&tail);
                    tail.push(line);
                    if tail.len() > 8 {
                        tail.remove(0);
                    }
                }
            })
        })
        .collect();
    let status = child.wait().map_err(|error| error.to_string())?;
    for reader in readers {
        let _ = reader.join();
    }
    if status.success() {
        Ok(())
    } else {
        let tail = crate::state::lock(&tail).join(" | ");
        Err(format!("{program} exited with {status}: {tail}"))
    }
}

fn uv_sync(reporter: &Reporter, name: &str) -> Result<(), String> {
    let home = sync_dir(name)?;
    reporter.send(
        "running",
        0,
        0,
        format!("uv sync --frozen in {}", home.display()),
    );
    let mut command = Command::new(layout::uv_exe());
    command
        .args(["sync", "--frozen", "--no-dev"])
        .current_dir(&home)
        .env("UV_NO_PROGRESS", "1");
    run_streaming(reporter, command)
}

/// The command that starts the STT worker (development or installed), for `--download-only`.
fn stt_command() -> Result<WorkerCommand, String> {
    if let Some(root) = dev_root("stt") {
        return Ok(WorkerCommand::dev_stt(&root));
    }
    layout::installed_worker("stt", "jarvis_stt")
        .ok_or_else(|| "the speech-to-text worker is not installed".to_owned())
}

fn whisper_download(reporter: &Reporter) -> Result<(), String> {
    let worker = stt_command()?;
    reporter.send("running", 0, 0, "downloading the Whisper small model");
    let mut command = Command::new(&worker.program);
    command.args(&worker.args).arg("--download-only");
    if let Some(dir) = &worker.current_dir {
        command.current_dir(dir);
    }
    command.env("JARVIS_MODELS_DIR", jarvis_tts::models_dir());
    run_streaming(reporter, command)
}

/// Whether `name` is something `ollama pull` accepts (letters, digits and `._:/-`).
pub fn valid_model_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
}

/// One NDJSON line of the Ollama pull stream: an error, or the status with its counters.
pub fn parse_pull_line(line: &str) -> Result<(String, u64, u64), String> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return Err(error.to_owned());
    }
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let counter = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Ok((status, counter("completed"), counter("total")))
}

/// Pulls a model through the local Ollama server (it is never started or installed here).
async fn pull_model(reporter: &Reporter, model: &str) -> Result<(), String> {
    if !valid_model_name(model) {
        return Err("invalid model name".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;
    let body = serde_json::json!({"name": model, "stream": true}).to_string();
    let mut response = client
        .post(format!("{}/api/pull", jarvis_brain::DEFAULT_OLLAMA_URL))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| format!("Ollama is not reachable: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Ollama answered HTTP {}", response.status()));
    }
    let mut pending = String::new();
    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(120), response.chunk())
            .await
            .map_err(|_| "Ollama sent nothing for 120 s".to_owned())?
            .map_err(|error| error.to_string())?;
        let Some(chunk) = chunk else { break };
        pending.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = pending.find('\n') {
            let line: String = pending.drain(..=end).collect();
            if line.trim().is_empty() {
                continue;
            }
            let (status, done, total) = parse_pull_line(line.trim())?;
            reporter.send("running", done, total, status);
        }
    }
    Ok(())
}

async fn extract_download(
    reporter: &Reporter,
    download: Download,
    scratch: &Path,
    target: PathBuf,
) -> Result<(), String> {
    let zip = download_verified(reporter, download, scratch).await?;
    reporter.send("running", 0, 0, "extracting");
    let parent = scratch.to_path_buf();
    tokio::task::spawn_blocking(move || extract_zip(&zip, &parent, &target))
        .await
        .map_err(|error| error.to_string())?
}

async fn blocking_step<F>(work: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

async fn install_item(reporter: &Reporter, id: &str) -> Result<(), String> {
    let models = jarvis_tts::models_dir();
    let scratch = models.join("_download");
    match id {
        "vosk_model" => {
            extract_download(
                reporter,
                VOSK_MODEL,
                &scratch,
                models.join("vosk-model-small-ru-0.22"),
            )
            .await
        }
        "piper" => {
            extract_download(
                reporter,
                PIPER,
                &scratch,
                models.join("piper").join("piper"),
            )
            .await
        }
        "piper_voice" => {
            copy_downloads(
                reporter,
                &[PIPER_VOICE, PIPER_VOICE_JSON],
                &scratch,
                &models.join("piper"),
            )
            .await
        }
        "kokoro" => {
            copy_downloads(
                reporter,
                &[KOKORO_MODEL, KOKORO_VOICES],
                &scratch,
                &models.join("kokoro"),
            )
            .await
        }
        "stt_env" => {
            let reporter = reporter.clone();
            blocking_step(move || uv_sync(&reporter, "stt")).await
        }
        "kokoro_env" => {
            let reporter = reporter.clone();
            blocking_step(move || uv_sync(&reporter, "tts_kokoro")).await
        }
        "whisper" => {
            if !env_ready("stt") {
                return Err("install the speech-to-text environment first".into());
            }
            let reporter = reporter.clone();
            blocking_step(move || whisper_download(&reporter)).await
        }
        other => Err(format!("unknown item {other}")),
    }
}

async fn copy_downloads(
    reporter: &Reporter,
    downloads: &[Download],
    scratch: &Path,
    target_dir: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(target_dir).map_err(|error| error.to_string())?;
    for download in downloads {
        let path = download_verified(reporter, *download, scratch).await?;
        std::fs::copy(&path, target_dir.join(download.file)).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Releases the "running" mark even when the task panics.
struct RunningGuard(&'static str);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        crate::state::lock(running_items()).remove(self.0);
    }
}

/// Starts installing one item in the background; progress arrives as `setup://progress`.
pub fn start(app: &AppHandle, id: &str) -> Result<(), CommandError> {
    let entry =
        item(id).ok_or_else(|| CommandError::Validation(format!("unknown item \"{id}\"")))?;
    if !crate::state::lock(running_items()).insert(entry.id) {
        return Err(CommandError::Validation(format!("{id} is already running")));
    }
    let reporter = Reporter {
        app: app.clone(),
        item: entry.id.to_owned(),
    };
    tauri::async_runtime::spawn(async move {
        let _guard = RunningGuard(entry.id);
        let result = install_item(&reporter, entry.id).await;
        finish(&reporter, entry.id, result).await;
    });
    Ok(())
}

/// Starts pulling an Ollama model.
pub fn start_pull(app: &AppHandle, model: &str) -> Result<(), CommandError> {
    if !valid_model_name(model) {
        return Err(CommandError::Validation("invalid model name".into()));
    }
    const PULL_ID: &str = "ollama_model";
    if !crate::state::lock(running_items()).insert(PULL_ID) {
        return Err(CommandError::Validation(
            "a model download is already running".into(),
        ));
    }
    let reporter = Reporter {
        app: app.clone(),
        item: PULL_ID.to_owned(),
    };
    let model = model.to_owned();
    tauri::async_runtime::spawn(async move {
        let _guard = RunningGuard(PULL_ID);
        let result = pull_model(&reporter, &model).await;
        finish(&reporter, PULL_ID, result).await;
    });
    Ok(())
}

async fn finish(reporter: &Reporter, id: &str, result: Result<(), String>) {
    let (ok, detail) = match &result {
        Ok(()) => (true, "done".to_owned()),
        Err(message) => (false, message.clone()),
    };
    match &result {
        Ok(()) => reporter.send("done", 1, 1, "done"),
        Err(message) => {
            tracing::warn!(item = id, error = %message, "setup item failed");
            reporter.send("error", 0, 0, message.clone());
        }
    }
    let db = reporter.app.state::<AppState>().db.clone();
    if let Err(error) = db
        .audit_append(
            "setup.install".into(),
            serde_json::json!({"item": id}),
            "safe".into(),
            ok,
            detail,
        )
        .await
    {
        tracing::warn!(%error, "setup audit failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_has_a_unique_id_and_pinned_hashes_are_sha256() {
        let mut ids: Vec<&str> = ITEMS.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ITEMS.len());
        for download in [
            VOSK_MODEL,
            PIPER,
            PIPER_VOICE,
            PIPER_VOICE_JSON,
            KOKORO_MODEL,
            KOKORO_VOICES,
        ] {
            assert_eq!(download.sha256.len(), 64, "{}", download.file);
            assert!(download.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(download.url.starts_with("https://"));
        }
    }

    #[test]
    fn model_names_are_validated() {
        assert!(valid_model_name("qwen3:4b"));
        assert!(valid_model_name("library/gemma3:4b"));
        assert!(!valid_model_name(""));
        assert!(!valid_model_name("-flag"));
        assert!(!valid_model_name("a b"));
        assert!(!valid_model_name("a;b"));
        assert!(!valid_model_name(&"a".repeat(65)));
    }

    #[test]
    fn pull_lines_are_parsed() {
        assert_eq!(
            parse_pull_line(r#"{"status":"pulling abc","total":100,"completed":40}"#),
            Ok(("pulling abc".to_owned(), 40, 100))
        );
        assert_eq!(
            parse_pull_line(r#"{"status":"success"}"#),
            Ok(("success".to_owned(), 0, 0))
        );
        assert_eq!(
            parse_pull_line(r#"{"error":"pull model manifest: file does not exist"}"#),
            Err("pull model manifest: file does not exist".to_owned())
        );
        assert!(parse_pull_line("not json").is_err());
    }

    #[test]
    fn sha256_of_a_file_matches_the_known_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("abc.txt");
        std::fs::write(&path, b"abc").expect("write");
        assert_eq!(
            file_sha256(&path).expect("hash"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }

    fn write_zip(path: &Path, name: &str, data: &[u8]) {
        use std::io::Write;
        let file = std::fs::File::create(path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file(name, options).expect("start");
        writer.write_all(data).expect("write");
        writer.finish().expect("finish");
    }

    #[test]
    fn zip_extraction_collapses_a_single_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let zip_path = dir.path().join("a.zip");
        write_zip(&zip_path, "root/inner/data.txt", b"hello");
        let target = dir.path().join("out");
        extract_zip(&zip_path, dir.path(), &target).expect("extract");
        assert_eq!(
            std::fs::read_to_string(target.join("inner").join("data.txt")).expect("read"),
            "hello"
        );
    }

    #[test]
    fn zip_extraction_refuses_path_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let evil = dir.path().join("evil.zip");
        write_zip(&evil, "../escape.txt", b"x");
        let error = extract_zip(&evil, dir.path(), &dir.path().join("out2")).expect_err("unsafe");
        assert!(error.contains("unsafe path"), "{error}");
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn unknown_items_are_rejected() {
        assert!(item("vosk_model").is_some());
        assert!(item("rm -rf").is_none());
    }
}
