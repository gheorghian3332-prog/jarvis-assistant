use crate::capture::{self, WindowInfo};
use crate::error::{Result, VisionError};
use crate::frame::Frame;
use crate::ocr::{self, OcrOutput};
use crate::ollama::{CloudPolicy, OllamaClient};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Longest side of an image sent to a vision model.
pub const VLM_MAX_SIDE: u32 = 1280;
/// OCR text passed to a model is cut to this many characters.
const MAX_OCR_PROMPT_CHARS: usize = 4000;
/// Name used when the shell has no usable Pictures folder.
const SCREENSHOT_PREFIX: &str = "Jarvis-screenshot";

/// Lets the UI show an indicator while the screen is being captured and analysed.
/// (The app maps this to the `vision://capture {active}` event.)
pub trait CaptureObserver: Send + Sync {
    fn capture_started(&self);
    fn capture_finished(&self);
}

/// Observer that does nothing.
pub struct NoObserver;

impl CaptureObserver for NoObserver {
    fn capture_started(&self) {}
    fn capture_finished(&self) {}
}

/// Calls `capture_finished` on drop, so every exit path (including errors) clears the indicator.
struct ActiveScope(Arc<dyn CaptureObserver>);

impl ActiveScope {
    fn begin(observer: &Arc<dyn CaptureObserver>) -> Self {
        observer.capture_started();
        Self(Arc::clone(observer))
    }
}

impl Drop for ActiveScope {
    fn drop(&mut self) {
        self.0.capture_finished();
    }
}

#[derive(Debug, Clone)]
pub struct VisionConfig {
    /// `vision.enabled`: captures happen only on request even when true.
    pub enabled: bool,
    /// `vision.allow_cloud` (default false).
    pub allow_cloud: bool,
    /// `ai.cloud_enabled`.
    pub ai_cloud_enabled: bool,
    /// `vision.vlm_model`: an Ollama model that accepts images.
    pub vlm_model: String,
    /// Text model used when the vision model is not available.
    pub text_model: String,
    pub ollama_url: String,
    pub ocr_languages: Vec<String>,
    pub request_timeout: Duration,
    /// `None`: the window actions use the foreground window. `Some(hwnd)`: that window instead
    /// (raw handle value), for checks that must not change the focus.
    pub target_window: Option<isize>,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_cloud: false,
            ai_cloud_enabled: false,
            vlm_model: "gemma3:4b".into(),
            text_model: "gemma3:4b".into(),
            ollama_url: "http://127.0.0.1:11434".into(),
            ocr_languages: ocr::DEFAULT_LANGUAGES
                .iter()
                .map(|tag| (*tag).to_owned())
                .collect(),
            request_timeout: Duration::from_secs(180),
            target_window: None,
        }
    }
}

impl VisionConfig {
    pub fn cloud_policy(&self) -> CloudPolicy {
        CloudPolicy {
            ai_cloud_enabled: self.ai_cloud_enabled,
            vision_allow_cloud: self.allow_cloud,
        }
    }
}

/// Window text without any model involved.
#[derive(Debug, Clone, Serialize)]
pub struct WindowReading {
    pub window: WindowInfo,
    pub ocr: OcrOutput,
    pub capture_ms: u128,
    pub ocr_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AnswerSource {
    /// A vision model looked at the (downscaled) picture.
    VisionModel,
    /// No vision model: a text model answered from the OCR text and the window title.
    TextModel,
}

#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    pub text: String,
    pub source: AnswerSource,
    pub model: String,
    pub window: Option<WindowInfo>,
    pub ocr_language: String,
    /// OCR / model-fallback remarks the UI should tell the user about.
    pub notices: Vec<String>,
    pub capture_ms: u128,
    pub ocr_ms: u128,
    pub model_ms: u128,
}

pub struct Vision {
    config: VisionConfig,
    observer: Arc<dyn CaptureObserver>,
}

impl Vision {
    pub fn new(config: VisionConfig, observer: Arc<dyn CaptureObserver>) -> Self {
        Self { config, observer }
    }

    pub fn config(&self) -> &VisionConfig {
        &self.config
    }

    fn ensure_enabled(&self) -> Result<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(VisionError::Disabled)
        }
    }

    /// Enabled, and the model endpoint may receive screen content. Checked before any capture.
    fn ensure_can_answer(&self) -> Result<()> {
        self.ensure_enabled()?;
        crate::ollama::check_destination(&self.config.ollama_url, self.config.cloud_policy())
    }

    fn languages(&self) -> Vec<String> {
        self.config.ocr_languages.clone()
    }

    /// Whole primary monitor + OCR + model answer (`vision.describe_screen`).
    pub async fn describe_screen(&self, question: Option<&str>) -> Result<Answer> {
        self.ensure_can_answer()?;
        let _active = ActiveScope::begin(&self.observer);
        let started = Instant::now();
        let window = capture::active_window_info().ok();
        let frame = blocking(capture::capture_primary_monitor).await?;
        let capture_ms = started.elapsed().as_millis();
        let question = question
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("Опиши, что сейчас на экране и чем занят пользователь.");
        self.answer(frame, window, question, capture_ms).await
    }

    /// Active window + OCR only (`vision.read_window`).
    pub async fn read_window(&self) -> Result<WindowReading> {
        self.ensure_enabled()?;
        let _active = ActiveScope::begin(&self.observer);
        let started = Instant::now();
        let target = self.config.target_window;
        let (frame, window) = blocking(move || match target {
            Some(hwnd) => capture::capture_window(hwnd),
            None => capture::capture_active_window(),
        })
        .await?;
        let capture_ms = started.elapsed().as_millis();
        let ocr_started = Instant::now();
        let languages = self.languages();
        let ocr = blocking(move || recognize_with(&frame, &languages)).await?;
        Ok(WindowReading {
            window,
            ocr,
            capture_ms,
            ocr_ms: ocr_started.elapsed().as_millis(),
        })
    }

    /// Explains the error shown in the active window (`vision.explain_error`).
    pub async fn explain_error(&self) -> Result<Answer> {
        self.ensure_can_answer()?;
        let _active = ActiveScope::begin(&self.observer);
        let started = Instant::now();
        let target = self.config.target_window;
        let (frame, window) = blocking(move || match target {
            Some(hwnd) => capture::capture_window(hwnd),
            None => capture::capture_active_window(),
        })
        .await?;
        let capture_ms = started.elapsed().as_millis();
        self.answer(
            frame,
            Some(window),
            "На экране показана ошибка или сообщение. Объясни простыми словами, что это значит, \
             вероятную причину и что сделать, чтобы это исправить.",
            capture_ms,
        )
        .await
    }

    /// Asks about a picture that is already in memory (used by `ocr-file`).
    pub async fn ask_about_frame(&self, frame: Frame, question: &str) -> Result<Answer> {
        self.ensure_can_answer()?;
        let _active = ActiveScope::begin(&self.observer);
        self.answer(frame, None, question, 0).await
    }

    /// OCR of a picture that is already in memory.
    pub async fn ocr_frame(&self, frame: Frame) -> Result<OcrOutput> {
        self.ensure_enabled()?;
        let _active = ActiveScope::begin(&self.observer);
        let languages = self.languages();
        blocking(move || recognize_with(&frame, &languages)).await
    }

    /// Saves a screenshot of the primary monitor into the Pictures folder. This is the only place
    /// where a picture is ever written, and only on the explicit `vision.save_screenshot` command.
    pub async fn save_screenshot(&self) -> Result<PathBuf> {
        self.ensure_enabled()?;
        let _active = ActiveScope::begin(&self.observer);
        let directory = pictures_dir()?;
        let frame = blocking(capture::capture_primary_monitor).await?;
        blocking(move || save_png_into(&directory, &frame)).await
    }

    async fn answer(
        &self,
        frame: Frame,
        window: Option<WindowInfo>,
        question: &str,
        capture_ms: u128,
    ) -> Result<Answer> {
        // Fail before any work if screen content may not go to the configured endpoint.
        crate::ollama::check_destination(&self.config.ollama_url, self.config.cloud_policy())?;
        let ocr_started = Instant::now();
        let languages = self.languages();
        let ocr_frame = frame.clone();
        let ocr = blocking(move || recognize_with(&ocr_frame, &languages)).await?;
        let ocr_ms = ocr_started.elapsed().as_millis();
        let mut notices: Vec<String> = ocr.notice.iter().cloned().collect();

        let prompt = build_prompt(question, window.as_ref(), &ocr.text);
        let client = OllamaClient::new(
            self.config.ollama_url.clone(),
            self.config.cloud_policy(),
            self.config.request_timeout,
        );
        let model_started = Instant::now();
        let image = blocking(move || frame.scaled_to_max(VLM_MAX_SIDE).to_png()).await?;
        let (text, source, model) =
            match client.chat(&self.config.vlm_model, &prompt, &[image]).await {
                Ok(text) => (
                    text,
                    AnswerSource::VisionModel,
                    self.config.vlm_model.clone(),
                ),
                Err(error @ (VisionError::ModelMissing(_) | VisionError::Llm(_))) => {
                    tracing::warn!(%error, "vision model unavailable, answering from OCR text");
                    notices.push(format!(
                    "vision model {} did not answer ({error}); answered from the recognized text",
                    self.config.vlm_model
                ));
                    let text = client.chat(&self.config.text_model, &prompt, &[]).await?;
                    (
                        text,
                        AnswerSource::TextModel,
                        self.config.text_model.clone(),
                    )
                }
                Err(other) => return Err(other),
            };
        Ok(Answer {
            text,
            source,
            model,
            window,
            ocr_language: ocr.language,
            notices,
            capture_ms,
            ocr_ms,
            model_ms: model_started.elapsed().as_millis(),
        })
    }
}

fn recognize_with(frame: &Frame, languages: &[String]) -> Result<OcrOutput> {
    let tags: Vec<&str> = languages.iter().map(String::as_str).collect();
    ocr::recognize(frame, &tags)
}

/// Runs blocking Win32/WinRT work off the async threads.
async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| VisionError::Capture(format!("worker task failed: {error}")))?
}

/// The prompt: the user's question, the window and the OCR text (cut to a sane size).
pub fn build_prompt(question: &str, window: Option<&WindowInfo>, ocr_text: &str) -> String {
    let mut prompt = String::from(
        "Ты помощник J.A.R.V.I.S. Отвечай коротко и по делу, на языке вопроса. \
         Ты видишь снимок экрана пользователя",
    );
    prompt.push_str(" (если снимок не приложен, опирайся на распознанный текст).\n");
    if let Some(window) = window {
        prompt.push_str(&format!(
            "Активное окно: «{}» (процесс {}).\n",
            window.title, window.process_name
        ));
    }
    let text: String = ocr_text.chars().take(MAX_OCR_PROMPT_CHARS).collect();
    if text.trim().is_empty() {
        prompt.push_str("Распознанного текста на экране нет.\n");
    } else {
        prompt.push_str("Текст, распознанный на экране (OCR, может содержать ошибки):\n");
        prompt.push_str(&text);
        prompt.push('\n');
    }
    prompt.push_str("Вопрос пользователя: ");
    prompt.push_str(question);
    prompt
}

/// The user's Pictures folder.
pub fn pictures_dir() -> Result<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    // SAFETY: the returned string is copied and then freed with CoTaskMemFree as documented.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None)
            .map_err(|error| VisionError::Io(format!("no Pictures folder: {error}")))?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0 as *const std::ffi::c_void));
        text.map(PathBuf::from)
            .map_err(|error| VisionError::Io(error.to_string()))
    }
}

/// Writes `frame` as a new timestamped PNG into `directory` (never overwrites).
pub fn save_png_into(directory: &Path, frame: &Frame) -> Result<PathBuf> {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: plain query.
    let now = unsafe { GetLocalTime() };
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
    );
    let bytes = frame.to_png()?;
    for attempt in 0..100u32 {
        let name = if attempt == 0 {
            format!("{SCREENSHOT_PREFIX}-{stamp}.png")
        } else {
            format!("{SCREENSHOT_PREFIX}-{stamp}-{attempt}.png")
        };
        let path = directory.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, &bytes)
                    .map_err(|error| VisionError::Io(error.to_string()))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(VisionError::Io(error.to_string())),
        }
    }
    Err(VisionError::Io("cannot pick a free file name".into()))
}
