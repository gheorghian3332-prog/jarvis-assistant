//! The registry actions of system intelligence and screen vision: `system.status`,
//! `vision.describe_screen`, `vision.read_window`, `vision.explain_error`, `vision.save_screenshot`.
//!
//! All five are `Safe`: they read, and `vision.save_screenshot` writes one PNG into Pictures only
//! because the user asked for it. The sources are behind small traits so the actions are tested with
//! fakes; the real implementations wrap `SysIntel` and `jarvis-vision`.

use crate::settings;
use async_trait::async_trait;
use jarvis_brain::{config as brain_config, Ollama, SharedConfig};
use jarvis_control::{replies::Reply, Action, ActionResult, ControlError, ControlResult, Risk};
use jarvis_core::Database;
use jarvis_sysintel::{
    report::{self, Topic},
    Alert, Metrics, ProcessUsage, Rank, SysIntel,
};
use jarvis_vision::{
    Answer as VisionAnswer, CaptureObserver, Vision, VisionConfig, VisionError, WindowReading,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

pub const ACTION_NAMES: [&str; 5] = [
    "system.status",
    "vision.describe_screen",
    "vision.read_window",
    "vision.explain_error",
    "vision.save_screenshot",
];

/// Longest part of an answer that is read aloud; the full text goes to the chat.
const SPEECH_CHARS: usize = 220;
/// OCR text longer than this is cut in the chat message.
const MAX_READ_CHARS: usize = 3000;
const MAX_QUESTION_CHARS: usize = 500;

// ---------- settings ----------

/// `vision.*` settings (PRIVACY tab).
#[derive(Debug, Clone, PartialEq)]
pub struct VisionSettings {
    pub enabled: bool,
    pub allow_cloud: bool,
    pub vlm_model: String,
}

impl Default for VisionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_cloud: false,
            vlm_model: "gemma3:4b".into(),
        }
    }
}

pub type SharedVisionSettings = Arc<RwLock<VisionSettings>>;

pub fn read_settings(shared: &SharedVisionSettings) -> VisionSettings {
    shared
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Reads the stored `vision.*` settings; invalid or missing values fall back to the defaults.
pub async fn load_vision_settings(db: &Database) -> VisionSettings {
    let read = |key: &'static str| {
        let db = db.clone();
        async move {
            let value = db.settings_get(key.to_owned()).await.ok().flatten()?;
            settings::validate(key, &value).ok()?;
            Some(value)
        }
    };
    let mut config = VisionSettings::default();
    if let Some(value) = read(settings::VISION_ENABLED)
        .await
        .and_then(|v| v.as_bool())
    {
        config.enabled = value;
    }
    if let Some(value) = read(settings::VISION_ALLOW_CLOUD)
        .await
        .and_then(|v| v.as_bool())
    {
        config.allow_cloud = value;
    }
    if let Some(value) = read(settings::VISION_VLM_MODEL).await {
        if let Some(model) = value.as_str() {
            config.vlm_model = model.to_owned();
        }
    }
    config
}

/// `ai.model` when it accepts images, otherwise `vision.vlm_model`.
pub fn choose_vlm_model(ai_model: &str, ai_model_sees: bool, vlm_model: &str) -> String {
    if ai_model_sees {
        ai_model.to_owned()
    } else {
        vlm_model.to_owned()
    }
}

// ---------- the capture indicator ----------

/// Calls `emit(true)` when a capture starts and `emit(false)` when it ends (`vision://capture`).
pub struct IndicatorObserver {
    emit: Box<dyn Fn(bool) + Send + Sync>,
}

impl IndicatorObserver {
    pub fn new(emit: impl Fn(bool) + Send + Sync + 'static) -> Self {
        Self {
            emit: Box::new(emit),
        }
    }
}

impl CaptureObserver for IndicatorObserver {
    fn capture_started(&self) {
        (self.emit)(true);
    }
    fn capture_finished(&self) {
        (self.emit)(false);
    }
}

// ---------- system source ----------

#[async_trait]
pub trait SystemSource: Send + Sync {
    fn metrics(&self) -> Metrics;
    async fn top(&self, by: Rank, limit: usize) -> Vec<ProcessUsage>;
    fn alerts(&self, limit: usize) -> Vec<Alert>;
}

#[async_trait]
impl SystemSource for SysIntel {
    fn metrics(&self) -> Metrics {
        self.snapshot()
    }

    async fn top(&self, by: Rank, limit: usize) -> Vec<ProcessUsage> {
        self.top_processes(by, limit).await.unwrap_or_default()
    }

    fn alerts(&self, limit: usize) -> Vec<Alert> {
        self.recent_alerts(limit)
    }
}

// ---------- vision backend ----------

/// What a vision action produces: the chat text and what is spoken.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionReply {
    pub text: String,
    pub detail: Value,
}

#[async_trait]
pub trait VisionBackend: Send + Sync {
    async fn describe(&self, question: Option<String>) -> Result<VisionReply, VisionError>;
    async fn read_window(&self) -> Result<VisionReply, VisionError>;
    async fn explain_error(&self) -> Result<VisionReply, VisionError>;
    async fn save_screenshot(&self) -> Result<VisionReply, VisionError>;
}

/// The real backend: `jarvis-vision` with the settings of the moment.
pub struct RealVision {
    settings: SharedVisionSettings,
    ai: SharedConfig,
    ollama: Arc<Ollama>,
    observer: Arc<dyn CaptureObserver>,
    /// Raw handle of a window to look at instead of the foreground one (checks only).
    target_window: Option<isize>,
}

impl RealVision {
    pub fn new(
        settings: SharedVisionSettings,
        ai: SharedConfig,
        ollama: Arc<Ollama>,
        observer: Arc<dyn CaptureObserver>,
    ) -> Self {
        Self {
            settings,
            ai,
            ollama,
            observer,
            target_window: None,
        }
    }

    /// Looks at `hwnd` instead of the foreground window, so a check never needs the focus.
    #[cfg(test)]
    #[must_use]
    pub fn with_target_window(mut self, hwnd: isize) -> Self {
        self.target_window = Some(hwnd);
        self
    }

    async fn vision(&self) -> Vision {
        let own = read_settings(&self.settings);
        let ai = brain_config::current(&self.ai);
        let sees = self
            .ollama
            .supports_images(&ai.model)
            .await
            .unwrap_or(false);
        let config = VisionConfig {
            enabled: own.enabled,
            allow_cloud: own.allow_cloud,
            ai_cloud_enabled: ai.cloud.enabled,
            vlm_model: choose_vlm_model(&ai.model, sees, &own.vlm_model),
            text_model: ai.model.clone(),
            target_window: self.target_window,
            ..VisionConfig::default()
        };
        Vision::new(config, Arc::clone(&self.observer))
    }
}

fn answer_reply(answer: VisionAnswer) -> VisionReply {
    let window = answer.window.as_ref().map(|w| w.title.clone());
    VisionReply {
        detail: json!({
            "model": answer.model,
            "source": format!("{:?}", answer.source),
            "window": window,
            "notices": answer.notices,
            "capture_ms": answer.capture_ms as u64,
            "ocr_ms": answer.ocr_ms as u64,
            "model_ms": answer.model_ms as u64,
        }),
        text: answer.text,
    }
}

fn reading_reply(reading: &WindowReading) -> VisionReply {
    let text = reading.ocr.text.trim();
    let title = reading.window.title.as_str();
    let body = if text.is_empty() {
        format!("В окне «{title}» я не нашёл читаемого текста.")
    } else {
        let shown: String = text.chars().take(MAX_READ_CHARS).collect();
        let cut = if text.chars().count() > MAX_READ_CHARS {
            "\n…"
        } else {
            ""
        };
        let note = reading
            .ocr
            .notice
            .as_ref()
            .map(|notice| format!("\n({notice})"))
            .unwrap_or_default();
        format!("В окне «{title}» написано:\n{shown}{cut}{note}")
    };
    VisionReply {
        detail: json!({
            "window": title,
            "process": reading.window.process_name,
            "ocr_language": reading.ocr.language,
            "lines": reading.ocr.lines.len(),
            "capture_ms": reading.capture_ms as u64,
            "ocr_ms": reading.ocr_ms as u64,
        }),
        text: body,
    }
}

#[async_trait]
impl VisionBackend for RealVision {
    async fn describe(&self, question: Option<String>) -> Result<VisionReply, VisionError> {
        let vision = self.vision().await;
        vision
            .describe_screen(question.as_deref())
            .await
            .map(answer_reply)
    }

    async fn read_window(&self) -> Result<VisionReply, VisionError> {
        let vision = self.vision().await;
        vision.read_window().await.map(|r| reading_reply(&r))
    }

    async fn explain_error(&self) -> Result<VisionReply, VisionError> {
        let vision = self.vision().await;
        vision.explain_error().await.map(answer_reply)
    }

    async fn save_screenshot(&self) -> Result<VisionReply, VisionError> {
        let vision = self.vision().await;
        let path = vision.save_screenshot().await?;
        let shown = path.to_string_lossy().into_owned();
        Ok(VisionReply {
            text: format!("Скриншот сохранён: {shown}"),
            detail: json!({ "path": shown }),
        })
    }
}

fn vision_error(error: VisionError) -> ControlError {
    match error {
        VisionError::Disabled => ControlError::forbidden(
            "screen vision is switched off in settings (vision.enabled = false)",
        ),
        VisionError::CloudDenied(message) => ControlError::forbidden(message),
        VisionError::Capture(message) => {
            ControlError::unavailable(format!("screen capture failed: {message}"))
        }
        VisionError::ModelMissing(model) => {
            ControlError::unavailable(format!("model {model} is not installed in Ollama"))
        }
        VisionError::Llm(message) => ControlError::unavailable(format!("local model: {message}")),
        other => ControlError::system(other.to_string()),
    }
}

// ---------- speech ----------

/// A short spoken version of `text`: markup removed, whole sentences up to `limit` characters.
pub fn speech_text(text: &str, limit: usize) -> String {
    let plain: String = text
        .lines()
        .map(|line| line.trim_start_matches(['#', '*', '-', '>', ' ', '•']))
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['*', '`', '_'], "");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.chars().count() <= limit {
        return plain;
    }
    let mut out = String::new();
    let mut sentence = String::new();
    for ch in plain.chars() {
        sentence.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            if out.chars().count() + sentence.chars().count() > limit {
                break;
            }
            out.push_str(&sentence);
            sentence.clear();
        }
    }
    if out.is_empty() {
        // One very long sentence: cut at a word boundary.
        let cut: String = plain.chars().take(limit).collect();
        let end = cut.rfind(' ').unwrap_or(cut.len());
        return format!("{}…", cut[..end].trim_end());
    }
    out.trim().to_owned()
}

// ---------- the actions ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    topic: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeArgs {
    question: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn parse_args<T: serde::de::DeserializeOwned>(value: &Value) -> ControlResult<T> {
    let value = if value.is_null() {
        json!({})
    } else {
        value.clone()
    };
    serde_json::from_value(value).map_err(|error| ControlError::validation(error.to_string()))
}

pub struct SystemStatus {
    source: Arc<dyn SystemSource>,
}

#[async_trait]
impl Action for SystemStatus {
    fn name(&self) -> &'static str {
        "system.status"
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn description_ru(&self) -> &'static str {
        "Состояние системы: температура и загрузка видеокарты, память, процессор, что тормозит"
    }
    fn description_en(&self) -> &'static str {
        "System status: GPU temperature and load, memory, CPU, what slows the computer down"
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        let parsed: StatusArgs = parse_args(args)?;
        match parsed.topic.as_deref() {
            None => Ok(()),
            Some(name) if Topic::parse(name).is_some() => Ok(()),
            Some(name) => Err(ControlError::validation(format!(
                "unknown topic \"{name}\", expected one of {}",
                report::TOPIC_NAMES.join(", ")
            ))),
        }
    }
    async fn run(&self, args: Value) -> ControlResult<ActionResult> {
        let parsed: StatusArgs = parse_args(&args)?;
        let topic = parsed
            .topic
            .as_deref()
            .and_then(Topic::parse)
            .unwrap_or(Topic::Overview);
        let metrics = self.source.metrics();
        let (top_cpu, top_ram) = if topic.needs_processes() {
            (
                self.source.top(Rank::Cpu, 3).await,
                self.source.top(Rank::Ram, 3).await,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let alerts = self.source.alerts(5);
        let answer = report::answer(topic, &metrics, &top_cpu, &top_ram, &alerts);
        Ok(ActionResult::new(
            Reply::new(answer.ru, answer.en),
            json!({
                "topic": topic.as_str(),
                "gpu_available": metrics.gpu.is_some(),
                "alerts": alerts.len(),
            }),
        ))
    }
}

/// One of the four vision actions.
pub struct VisionAction {
    name: &'static str,
    ru: &'static str,
    en: &'static str,
    backend: Arc<dyn VisionBackend>,
}

fn reply_of(reply: VisionReply) -> ActionResult {
    let mut detail = reply.detail;
    if let Some(object) = detail.as_object_mut() {
        // The chat shows the full text; only this part is spoken.
        object.insert(
            "speech".into(),
            json!(speech_text(&reply.text, SPEECH_CHARS)),
        );
    }
    ActionResult::new(Reply::new(reply.text.clone(), reply.text), detail)
}

#[async_trait]
impl Action for VisionAction {
    fn name(&self) -> &'static str {
        self.name
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn description_ru(&self) -> &'static str {
        self.ru
    }
    fn description_en(&self) -> &'static str {
        self.en
    }
    fn validate(&self, args: &Value) -> ControlResult<()> {
        if self.name == "vision.describe_screen" {
            let parsed: DescribeArgs = parse_args(args)?;
            if let Some(question) = parsed.question {
                if question.chars().count() > MAX_QUESTION_CHARS {
                    return Err(ControlError::validation(format!(
                        "question is longer than {MAX_QUESTION_CHARS} characters"
                    )));
                }
            }
            Ok(())
        } else {
            parse_args::<NoArgs>(args).map(|_| ())
        }
    }
    async fn run(&self, args: Value) -> ControlResult<ActionResult> {
        let result = match self.name {
            "vision.describe_screen" => {
                let parsed: DescribeArgs = parse_args(&args)?;
                self.backend.describe(parsed.question).await
            }
            "vision.read_window" => self.backend.read_window().await,
            "vision.explain_error" => self.backend.explain_error().await,
            _ => self.backend.save_screenshot().await,
        };
        result.map(reply_of).map_err(vision_error)
    }
}

/// The five actions, ready for the registry.
pub fn actions(
    source: Arc<dyn SystemSource>,
    backend: Arc<dyn VisionBackend>,
) -> Vec<Arc<dyn Action>> {
    let vision = |name, ru, en| -> Arc<dyn Action> {
        Arc::new(VisionAction {
            name,
            ru,
            en,
            backend: Arc::clone(&backend),
        })
    };
    vec![
        Arc::new(SystemStatus { source }),
        vision(
            "vision.describe_screen",
            "Посмотреть на экран и ответить на вопрос о нём (по запросу)",
            "Look at the screen and answer a question about it (on request only)",
        ),
        vision(
            "vision.read_window",
            "Прочитать текст в активном окне (OCR, по запросу)",
            "Read the text in the active window (OCR, on request only)",
        ),
        vision(
            "vision.explain_error",
            "Объяснить ошибку, показанную в активном окне",
            "Explain the error shown in the active window",
        ),
        vision(
            "vision.save_screenshot",
            "Сохранить скриншот экрана в папку Изображения (только по команде)",
            "Save a screenshot into Pictures (only on command)",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_sysintel::metrics::{GpuMetrics, RamMetrics};
    use std::sync::Mutex;

    struct FakeSource {
        metrics: Metrics,
    }

    #[async_trait]
    impl SystemSource for FakeSource {
        fn metrics(&self) -> Metrics {
            self.metrics.clone()
        }
        async fn top(&self, by: Rank, _limit: usize) -> Vec<ProcessUsage> {
            let name = if by == Rank::Cpu {
                "Браузер"
            } else {
                "Редактор"
            };
            vec![ProcessUsage {
                exe: "x.exe".into(),
                display_name: name.into(),
                instances: 1,
                memory_mb: 1,
                cpu_percent: Some(1.0),
            }]
        }
        fn alerts(&self, _limit: usize) -> Vec<Alert> {
            Vec::new()
        }
    }

    #[derive(Default)]
    struct FakeVision {
        calls: Mutex<Vec<String>>,
        fail: Mutex<Option<VisionError>>,
    }

    impl FakeVision {
        fn reply(&self, call: &str, text: &str) -> Result<VisionReply, VisionError> {
            self.calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(call.to_owned());
            match self.fail.lock().unwrap_or_else(|p| p.into_inner()).take() {
                Some(error) => Err(error),
                None => Ok(VisionReply {
                    text: text.into(),
                    detail: json!({}),
                }),
            }
        }
    }

    #[async_trait]
    impl VisionBackend for FakeVision {
        async fn describe(&self, question: Option<String>) -> Result<VisionReply, VisionError> {
            self.reply(
                &format!("describe:{}", question.unwrap_or_default()),
                "На экране редактор.",
            )
        }
        async fn read_window(&self) -> Result<VisionReply, VisionError> {
            self.reply("read", "В окне написано: привет")
        }
        async fn explain_error(&self) -> Result<VisionReply, VisionError> {
            self.reply("explain", "Файл не найден. Проверьте путь.")
        }
        async fn save_screenshot(&self) -> Result<VisionReply, VisionError> {
            self.reply("save", "Скриншот сохранён")
        }
    }

    fn registry_with(metrics: Metrics) -> (Vec<Arc<dyn Action>>, Arc<FakeVision>) {
        let backend = Arc::new(FakeVision::default());
        let list = actions(Arc::new(FakeSource { metrics }), backend.clone());
        (list, backend)
    }

    fn find<'a>(list: &'a [Arc<dyn Action>], name: &str) -> &'a Arc<dyn Action> {
        list.iter().find(|a| a.name() == name).expect("action")
    }

    fn fixture() -> Metrics {
        Metrics {
            gpu: Some(GpuMetrics {
                name: Some("Test GPU".into()),
                temperature_c: Some(68),
                ..GpuMetrics::default()
            }),
            ram: RamMetrics {
                used_bytes: 4 << 30,
                total_bytes: 16 << 30,
                available_bytes: 12 << 30,
            },
            ..Metrics::default()
        }
    }

    #[test]
    fn the_registry_names_match_and_all_are_safe() {
        let (list, _) = registry_with(Metrics::default());
        let names: Vec<&str> = list.iter().map(|a| a.name()).collect();
        assert_eq!(names, ACTION_NAMES);
        assert!(list.iter().all(|a| a.risk() == Risk::Safe));
    }

    #[tokio::test]
    async fn system_status_answers_from_the_metrics() {
        let (list, _) = registry_with(fixture());
        let action = find(&list, "system.status");
        let args = json!({"topic": "gpu_temp"});
        action.validate(&args).expect("valid");
        let result = action.run(args).await.expect("run");
        assert!(
            result.reply_ru.contains("68 градусов"),
            "{}",
            result.reply_ru
        );
        assert_eq!(result.detail["topic"], "gpu_temp");
        let memory = action.run(json!({"topic": "memory"})).await.expect("run");
        assert!(memory.reply_ru.contains("Свободно 12 гигабайт из 16"));
        // no topic = an overview
        let overview = action.run(Value::Null).await.expect("run");
        assert_eq!(overview.detail["topic"], "overview");
    }

    #[test]
    fn system_status_rejects_unknown_topics_and_arguments() {
        let (list, _) = registry_with(Metrics::default());
        let action = find(&list, "system.status");
        assert!(action.validate(&json!({"topic": "weather"})).is_err());
        assert!(action.validate(&json!({"extra": 1})).is_err());
        assert!(action.validate(&json!({})).is_ok());
    }

    #[tokio::test]
    async fn vision_actions_call_the_backend_and_split_speech_from_text() {
        let (list, backend) = registry_with(Metrics::default());
        let describe = find(&list, "vision.describe_screen");
        describe
            .validate(&json!({"question": "что это?"}))
            .expect("valid");
        let result = describe
            .run(json!({"question": "что это?"}))
            .await
            .expect("run");
        assert_eq!(result.reply_ru, "На экране редактор.");
        assert_eq!(result.detail["speech"], "На экране редактор.");
        find(&list, "vision.read_window")
            .run(json!({}))
            .await
            .expect("read");
        find(&list, "vision.explain_error")
            .run(Value::Null)
            .await
            .expect("explain");
        find(&list, "vision.save_screenshot")
            .run(json!({}))
            .await
            .expect("save");
        let calls = backend
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(calls, ["describe:что это?", "read", "explain", "save"]);
    }

    #[test]
    fn vision_arguments_are_validated() {
        let (list, _) = registry_with(Metrics::default());
        let describe = find(&list, "vision.describe_screen");
        assert!(describe
            .validate(&json!({"question": "я".repeat(501)}))
            .is_err());
        assert!(describe.validate(&json!({"query": "x"})).is_err());
        assert!(find(&list, "vision.read_window")
            .validate(&json!({"x": 1}))
            .is_err());
    }

    #[tokio::test]
    async fn vision_errors_map_to_executor_codes() {
        let (list, backend) = registry_with(Metrics::default());
        let read = find(&list, "vision.read_window");
        for (error, code) in [
            (VisionError::Disabled, "FORBIDDEN"),
            (VisionError::CloudDenied("no".into()), "FORBIDDEN"),
            (VisionError::Capture("locked".into()), "UNAVAILABLE"),
            (VisionError::ModelMissing("m".into()), "UNAVAILABLE"),
            (VisionError::Ocr("x".into()), "SYSTEM"),
        ] {
            *backend.fail.lock().unwrap_or_else(|p| p.into_inner()) = Some(error);
            let failure = read.run(json!({})).await.expect_err("fails");
            assert_eq!(failure.code.as_str(), code);
        }
        *backend.fail.lock().unwrap_or_else(|p| p.into_inner()) = Some(VisionError::Disabled);
        let denied = read.run(json!({})).await.expect_err("fails");
        assert!(denied.message.contains("switched off"));
    }

    #[test]
    fn speech_is_short_plain_and_whole_sentences() {
        let long = "Это окно текстового редактора. В нём открыт файл с заметками. ".repeat(10);
        let spoken = speech_text(&long, 120);
        assert!(spoken.chars().count() <= 120, "{spoken}");
        assert!(spoken.ends_with('.'));
        assert_eq!(speech_text("**Важно:** `код`", 100), "Важно: код");
        assert_eq!(speech_text("# Заголовок\n- пункт", 100), "Заголовок пункт");
        let one = "слово ".repeat(100);
        let cut = speech_text(&one, 50);
        assert!(cut.ends_with('…') && cut.chars().count() <= 52, "{cut}");
    }

    #[test]
    fn the_model_is_the_main_one_only_when_it_sees() {
        assert_eq!(
            choose_vlm_model("gemma3:4b", true, "qwen2.5vl:3b"),
            "gemma3:4b"
        );
        assert_eq!(
            choose_vlm_model("qwen3:4b", false, "gemma3:4b"),
            "gemma3:4b"
        );
    }

    #[test]
    fn the_indicator_follows_the_capture() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let observer = IndicatorObserver::new(move |active| {
            sink.lock().unwrap_or_else(|p| p.into_inner()).push(active);
        });
        observer.capture_started();
        observer.capture_finished();
        assert_eq!(
            *seen.lock().unwrap_or_else(|p| p.into_inner()),
            [true, false]
        );
    }

    #[tokio::test]
    async fn a_real_vision_run_raises_and_clears_the_indicator() {
        // OCR of a picture that is already in memory goes through the same scope as a capture.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let observer = Arc::new(IndicatorObserver::new(move |active| {
            sink.lock().unwrap_or_else(|p| p.into_inner()).push(active);
        }));
        let vision = Vision::new(VisionConfig::default(), observer);
        let frame = jarvis_vision::Frame::new(64, 64, vec![255; 64 * 64 * 4]).expect("frame");
        let _ = vision.ocr_frame(frame).await;
        assert_eq!(
            *seen.lock().unwrap_or_else(|p| p.into_inner()),
            [true, false]
        );
        // A disabled vision never starts the indicator.
        seen.lock().unwrap_or_else(|p| p.into_inner()).clear();
        let sink = Arc::clone(&seen);
        let observer = Arc::new(IndicatorObserver::new(move |active| {
            sink.lock().unwrap_or_else(|p| p.into_inner()).push(active);
        }));
        let off = Vision::new(
            VisionConfig {
                enabled: false,
                ..VisionConfig::default()
            },
            observer,
        );
        assert!(matches!(
            off.describe_screen(None).await,
            Err(VisionError::Disabled)
        ));
        assert!(seen.lock().unwrap_or_else(|p| p.into_inner()).is_empty());
    }

    #[tokio::test]
    async fn cloud_endpoints_are_refused_without_both_flags() {
        let observer = Arc::new(jarvis_vision::NoObserver);
        for (ai_cloud, allow) in [(false, false), (true, false), (false, true)] {
            let vision = Vision::new(
                VisionConfig {
                    ollama_url: "https://api.example.com".into(),
                    ai_cloud_enabled: ai_cloud,
                    allow_cloud: allow,
                    ..VisionConfig::default()
                },
                observer.clone(),
            );
            assert!(matches!(
                vision.describe_screen(None).await,
                Err(VisionError::CloudDenied(_))
            ));
        }
    }
}
