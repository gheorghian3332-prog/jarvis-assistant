//! Kokoro through the Python worker `workers/tts_kokoro` (protocol 0x30 TTS_REQ -> 0x31 TTS_PCM).
//!
//! The worker is started by the first phrase and stops after the idle timeout; Python never runs in
//! the background otherwise.

use crate::{
    clip::AudioClip,
    config::{is_kokoro_voice, models_dir},
    engine::TtsEngine,
    error::{TtsError, TtsResult},
    text::Lang,
};
use async_trait::async_trait;
use jarvis_core::worker::{tts_request, ManagedWorker, SttConfig, WorkerCommand};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const ENGINE: &str = "kokoro";
/// Sample rate of the Kokoro worker's PCM.
pub const KOKORO_SAMPLE_RATE: u32 = 24_000;
/// Voices offered in the UI (all British male/female voices of Kokoro v1.0 are in `voices-v1.0.bin`).
pub const KOKORO_VOICES: [&str; 4] = ["bm_george", "bm_lewis", "bm_daniel", "bm_fable"];

/// Launch command of the worker: the project's venv interpreter when it exists (no `uv` start-up
/// cost), otherwise `uv run`.
pub fn worker_command(repo_root: &Path) -> WorkerCommand {
    let project = repo_root.join("workers").join("tts_kokoro");
    let python = project.join(".venv").join("Scripts").join("python.exe");
    if python.is_file() {
        WorkerCommand::python_module(python, "jarvis_tts_kokoro", Some(project))
    } else {
        WorkerCommand::dev_kokoro(repo_root)
    }
}

/// Where the model files live.
pub fn model_files_dir() -> PathBuf {
    models_dir().join("kokoro")
}

/// True when the Kokoro model and the voices file are installed.
pub fn model_installed() -> bool {
    let dir = model_files_dir();
    dir.join("voices-v1.0.bin").is_file()
        && (dir.join("kokoro-v1.0.onnx").is_file() || dir.join("kokoro-v1.0.int8.onnx").is_file())
}

#[derive(Clone)]
pub struct KokoroEngine {
    worker: Option<ManagedWorker>,
    /// Why the worker cannot be used (set when the repository was not found).
    missing: Option<String>,
}

impl KokoroEngine {
    pub fn new(command: WorkerCommand) -> Self {
        Self::with_idle_timeout(command, Duration::from_secs(120))
    }

    /// Like [`KokoroEngine::new`] with another idle limit (the worker exits after it).
    pub fn with_idle_timeout(command: WorkerCommand, idle_timeout: Duration) -> Self {
        let mut config = SttConfig::new(command);
        config.idle_timeout = idle_timeout;
        // Loading the model takes several seconds; a phrase is a fraction of that.
        config.first_hello_timeout = Duration::from_secs(90);
        config.hello_timeout = Duration::from_secs(60);
        config.request_timeout = Duration::from_secs(30);
        Self {
            worker: Some(ManagedWorker::new("kokoro", config)),
            missing: None,
        }
    }

    /// Stands in when the worker project cannot be found: every phrase reports why.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            worker: None,
            missing: Some(reason.into()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.worker.is_some()
    }

    /// Starts the worker ahead of the first phrase (its model load then overlaps other work).
    pub async fn prewarm(&self) -> TtsResult<()> {
        let worker = self.worker()?;
        worker
            .prewarm()
            .await
            .map_err(|error| TtsError::unavailable(ENGINE, error.to_string()))
    }

    pub async fn is_running(&self) -> bool {
        match &self.worker {
            Some(worker) => worker.is_running().await,
            None => false,
        }
    }

    pub async fn pid(&self) -> Option<u32> {
        self.worker.as_ref()?.worker_pid().await
    }

    /// Stops the worker now.
    pub async fn stop(&self) {
        if let Some(worker) = &self.worker {
            worker.stop().await;
        }
    }

    fn worker(&self) -> TtsResult<&ManagedWorker> {
        self.worker.as_ref().ok_or_else(|| {
            TtsError::unavailable(
                ENGINE,
                self.missing
                    .clone()
                    .unwrap_or_else(|| "worker is not configured".into()),
            )
        })
    }
}

#[async_trait]
impl TtsEngine for KokoroEngine {
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
        if !is_kokoro_voice(voice) {
            return Err(TtsError::Invalid(format!("bad Kokoro voice id `{voice}`")));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(TtsError::Invalid("empty text".into()));
        }
        let worker = self.worker()?.clone();
        let (text, voice) = (text.to_owned(), voice.to_owned());
        // A separate task: a request in flight finishes even if the caller is cancelled, so the
        // worker is never left with an unread reply.
        let pcm = tokio::spawn(async move {
            worker
                .run((text, voice, speed), |handle, (text, voice, speed)| {
                    Box::pin(async move { tts_request(handle, &text, &voice, speed).await })
                })
                .await
        })
        .await
        .map_err(|error| TtsError::synthesis(ENGINE, format!("task failed: {error}")))?
        .map_err(|error| TtsError::synthesis(ENGINE, error.to_string()))?;
        Ok(AudioClip::new(pcm, KOKORO_SAMPLE_RATE))
    }

    async fn shutdown(&self) {
        self.stop().await;
    }
}

/// Shared handle used by the service.
pub type SharedKokoro = Arc<KokoroEngine>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_worker_every_phrase_explains_why() {
        let engine = KokoroEngine::unavailable("workers/tts_kokoro not found");
        assert!(!engine.is_available());
        let error = engine
            .synth("Hello", Lang::En, "bm_george", 1.0)
            .await
            .expect_err("unavailable");
        assert!(error.to_string().contains("workers/tts_kokoro"), "{error}");
        assert!(engine.prewarm().await.is_err());
        assert!(!engine.is_running().await);
    }

    #[tokio::test]
    async fn rejects_bad_voice_ids_and_empty_text_before_starting_anything() {
        let engine = KokoroEngine::new(WorkerCommand::new("jarvis-no-such-program"));
        assert!(matches!(
            engine.synth("Hi", Lang::En, "../x", 1.0).await,
            Err(TtsError::Invalid(_))
        ));
        assert!(matches!(
            engine.synth("   ", Lang::En, "bm_george", 1.0).await,
            Err(TtsError::Invalid(_))
        ));
        let error = engine
            .synth("Hi", Lang::En, "bm_george", 1.0)
            .await
            .expect_err("program does not exist");
        assert!(matches!(error, TtsError::Synthesis { .. }), "{error}");
    }
}
