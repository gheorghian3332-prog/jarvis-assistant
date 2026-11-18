//! Language-model clients: Ollama on localhost (the default) and an optional OpenAI-compatible
//! cloud endpoint (off by default, never used without the user's switch and a stored key).

use crate::{
    config::{
        current, is_local_url, keep_alive_value, validate_cloud_url, SharedConfig, MAX_NUM_CTX,
        MAX_NUM_THREAD,
    },
    error::{BrainError, BrainResult},
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// A cold model may need a minute to load from disk.
const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
const QUICK_TIMEOUT: Duration = Duration::from_secs(5);
const CLOUD_TIMEOUT: Duration = Duration::from_secs(20);
const SERVE_WAIT: Duration = Duration::from_secs(20);
const SERVE_RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &'static str, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    /// JSON schema the answer must follow (Ollama `format`).
    pub schema: Option<Value>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmReply {
    pub content: String,
    pub load_ms: u64,
    pub total_ms: u64,
    pub eval_tokens: u64,
}

#[async_trait]
pub trait Llm: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> BrainResult<LlmReply>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    pub name: String,
    pub size_bytes: u64,
    pub vram_bytes: u64,
    pub expires_at: String,
}

fn client(timeout: Duration) -> BrainResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .no_proxy()
        .build()
        .map_err(|error| BrainError::Unavailable(format!("http client: {error}")))
}

fn transport(error: &reqwest::Error) -> BrainError {
    if error.is_connect() {
        BrainError::Unavailable(error.to_string())
    } else if error.is_timeout() {
        BrainError::Timeout(error.to_string())
    } else {
        BrainError::Unavailable(error.to_string())
    }
}

/// Where `ollama.exe` may be: `where ollama`, then the standard per-user install folder.
pub fn find_ollama() -> Option<PathBuf> {
    let mut command = std::process::Command::new("where.exe");
    command.arg("ollama");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    if let Ok(output) = command.output() {
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) {
            let path = PathBuf::from(line);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let local = std::env::var_os("LOCALAPPDATA")?;
    let path = PathBuf::from(local).join("Programs/Ollama/ollama.exe");
    path.is_file().then_some(path)
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The Ollama server on this machine.
pub struct Ollama {
    base: String,
    chat_http: reqwest::Client,
    quick_http: reqwest::Client,
    /// Models that refused the `think` option.
    no_think: Mutex<HashSet<String>>,
    last_spawn: tokio::sync::Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: Option<ChatMessageOut>,
    #[serde(default)]
    total_duration: u64,
    #[serde(default)]
    load_duration: u64,
    #[serde(default)]
    eval_count: u64,
}

#[derive(Deserialize)]
struct ChatMessageOut {
    content: String,
}

/// True when an `/api/show` answer lists the `vision` capability.
pub fn capabilities_include_vision(show: &Value) -> bool {
    show["capabilities"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("vision")))
}

impl Ollama {
    /// `base` must be a loopback URL: the local model never leaves the machine.
    pub fn new(base: &str) -> BrainResult<Self> {
        if !is_local_url(base) {
            return Err(BrainError::Validation(
                "the Ollama address must be on localhost".into(),
            ));
        }
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            chat_http: client(CHAT_TIMEOUT)?,
            quick_http: client(QUICK_TIMEOUT)?,
            no_think: Mutex::new(HashSet::new()),
            last_spawn: tokio::sync::Mutex::new(None),
        })
    }

    pub fn local() -> BrainResult<Self> {
        Self::new(DEFAULT_OLLAMA_URL)
    }

    async fn get(&self, path: &str) -> BrainResult<Value> {
        let response = self
            .quick_http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|error| transport(&error))?;
        parse_response(response).await
    }

    async fn post(&self, http: &reqwest::Client, path: &str, body: &Value) -> BrainResult<Value> {
        let bytes = serde_json::to_vec(body)
            .map_err(|error| BrainError::BadResponse(format!("request body: {error}")))?;
        let response = http
            .post(format!("{}{path}", self.base))
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|error| transport(&error))?;
        parse_response(response).await
    }

    /// The server version, or `UNAVAILABLE`.
    pub async fn version(&self) -> BrainResult<String> {
        let value = self.get("/api/version").await?;
        value["version"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| BrainError::BadResponse("no version".into()))
    }

    /// Installed models (`ollama list`).
    pub async fn models(&self) -> BrainResult<Vec<ModelInfo>> {
        let value = self.get("/api/tags").await?;
        Ok(value["models"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(ModelInfo {
                            name: item["name"].as_str()?.to_owned(),
                            size_bytes: item["size"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Models currently held in memory (`ollama ps`).
    pub async fn loaded(&self) -> BrainResult<Vec<LoadedModel>> {
        let value = self.get("/api/ps").await?;
        Ok(value["models"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(LoadedModel {
                            name: item["name"].as_str()?.to_owned(),
                            size_bytes: item["size"].as_u64().unwrap_or(0),
                            vram_bytes: item["size_vram"].as_u64().unwrap_or(0),
                            expires_at: item["expires_at"].as_str().unwrap_or("").to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Whether the installed model accepts images (`capabilities` of `/api/show` lists `vision`).
    /// `false` when the server does not report capabilities or the model is unknown.
    pub async fn supports_images(&self, model: &str) -> BrainResult<bool> {
        let value = self
            .post(&self.quick_http, "/api/show", &json!({"model": model}))
            .await?;
        Ok(capabilities_include_vision(&value))
    }

    /// Frees the model at once (`keep_alive: 0`).
    pub async fn unload(&self, model: &str) -> BrainResult<()> {
        self.post(
            &self.quick_http,
            "/api/generate",
            &json!({"model": model, "keep_alive": 0}),
        )
        .await
        .map(|_| ())
    }

    /// Makes sure a server answers: starts `ollama serve` without a window if none does.
    pub async fn ensure_running(&self) -> BrainResult<()> {
        match self.version().await {
            Ok(_) => return Ok(()),
            Err(BrainError::Unavailable(_)) => {}
            Err(other) => return Err(other),
        }
        let mut last = self.last_spawn.lock().await;
        if last.is_some_and(|at| at.elapsed() < SERVE_RETRY_AFTER) {
            return Err(BrainError::Unavailable(
                "the Ollama server is not running".into(),
            ));
        }
        *last = Some(Instant::now());
        let path = find_ollama()
            .ok_or_else(|| BrainError::Unavailable("Ollama is not installed".into()))?;
        let mut command = tokio::process::Command::new(path);
        command
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let child = command
            .spawn()
            .map_err(|error| BrainError::Unavailable(format!("cannot start ollama: {error}")))?;
        // The server outlives this handle on purpose: it unloads its models by itself.
        drop(child);
        let started = Instant::now();
        while started.elapsed() < SERVE_WAIT {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if self.version().await.is_ok() {
                return Ok(());
            }
        }
        Err(BrainError::Unavailable(
            "the Ollama server did not start".into(),
        ))
    }

    /// One non-streaming chat request.
    pub async fn chat(
        &self,
        model: &str,
        request: &LlmRequest,
        options: &Value,
        keep_alive: Value,
    ) -> BrainResult<LlmReply> {
        let send_think = !self
            .no_think
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(model);
        let mut attempt = self
            .chat_once(model, request, options, &keep_alive, send_think)
            .await;
        if send_think {
            if let Err(BrainError::BadResponse(message)) = &attempt {
                if message.contains("thinking") {
                    self.no_think
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(model.to_owned());
                    attempt = self
                        .chat_once(model, request, options, &keep_alive, false)
                        .await;
                }
            }
        }
        attempt
    }

    async fn chat_once(
        &self,
        model: &str,
        request: &LlmRequest,
        options: &Value,
        keep_alive: &Value,
        think: bool,
    ) -> BrainResult<LlmReply> {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
            "keep_alive": keep_alive,
            "options": options,
        });
        if think {
            body["think"] = json!(false);
        }
        if let Some(schema) = &request.schema {
            body["format"] = schema.clone();
        }
        let value = self.post(&self.chat_http, "/api/chat", &body).await?;
        let parsed: ChatResponse = serde_json::from_value(value)
            .map_err(|error| BrainError::BadResponse(error.to_string()))?;
        let content = parsed
            .message
            .map(|m| m.content)
            .ok_or_else(|| BrainError::BadResponse("no message in the response".into()))?;
        Ok(LlmReply {
            content,
            load_ms: parsed.load_duration / 1_000_000,
            total_ms: parsed.total_duration / 1_000_000,
            eval_tokens: parsed.eval_count,
        })
    }
}

async fn parse_response(response: reqwest::Response) -> BrainResult<Value> {
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| transport(&error))?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        let short: String = text.chars().take(300).collect();
        return Err(BrainError::BadResponse(format!("HTTP {status}: {short}")));
    }
    serde_json::from_slice(&bytes).map_err(|error| BrainError::BadResponse(error.to_string()))
}

/// The local model with the current settings (model, keep-alive, limits).
pub struct OllamaLlm {
    ollama: Arc<Ollama>,
    config: SharedConfig,
}

impl OllamaLlm {
    pub fn new(ollama: Arc<Ollama>, config: SharedConfig) -> Self {
        Self { ollama, config }
    }
}

#[async_trait]
impl Llm for OllamaLlm {
    async fn complete(&self, request: LlmRequest) -> BrainResult<LlmReply> {
        let config = current(&self.config);
        self.ollama.ensure_running().await?;
        let options = json!({
            "num_ctx": config.num_ctx.min(MAX_NUM_CTX),
            "num_thread": config.num_thread.clamp(1, MAX_NUM_THREAD),
            "temperature": 0.2,
            "num_predict": request.max_tokens,
        });
        self.ollama
            .chat(
                &config.model,
                &request,
                &options,
                keep_alive_value(config.keep_alive_minutes),
            )
            .await
    }
}

/// Reads the cloud key from the credential store; `None` when none is stored.
pub type KeyReader = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// An OpenAI-compatible chat endpoint. Used only when the user switched the cloud on and stored a key.
pub struct CloudLlm {
    http: reqwest::Client,
    config: SharedConfig,
    key: KeyReader,
}

impl CloudLlm {
    pub fn new(config: SharedConfig, key: KeyReader) -> BrainResult<Self> {
        Ok(Self {
            http: client(CLOUD_TIMEOUT)?,
            config,
            key,
        })
    }
}

#[async_trait]
impl Llm for CloudLlm {
    async fn complete(&self, request: LlmRequest) -> BrainResult<LlmReply> {
        let config = current(&self.config).cloud;
        if !config.enabled {
            return Err(BrainError::Unavailable("the cloud is switched off".into()));
        }
        let loopback = crate::config::is_local_url(&config.base_url);
        if !loopback {
            validate_cloud_url(&config.base_url)?;
        }
        if config.base_url.is_empty() || config.model.is_empty() {
            return Err(BrainError::Unavailable(
                "the cloud endpoint or model is not set".into(),
            ));
        }
        let key = (self.key)()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| BrainError::Unavailable("no cloud key is stored".into()))?;
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();
        let body = json!({
            "model": config.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "temperature": 0.2,
            "response_format": {"type": "json_object"},
        });
        let bytes = serde_json::to_vec(&body)
            .map_err(|error| BrainError::BadResponse(format!("request body: {error}")))?;
        let started = Instant::now();
        let response = self
            .http
            .post(format!(
                "{}/chat/completions",
                config.base_url.trim_end_matches('/')
            ))
            .header("content-type", "application/json")
            .bearer_auth(key)
            .body(bytes)
            .send()
            .await
            .map_err(|error| transport(&error))?;
        let value = parse_response(response).await?;
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| BrainError::BadResponse("no choices in the response".into()))?
            .to_owned();
        Ok(LlmReply {
            content,
            load_ms: 0,
            total_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            eval_tokens: value["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        })
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! A tiny HTTP server that plays Ollama (or a cloud endpoint) in tests.
    use std::sync::{Arc, Mutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[derive(Clone, Default)]
    pub struct Seen {
        pub requests: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    /// `handler(method, path, body) -> (status, json body)`; returns the base URL.
    pub async fn serve<F>(handler: F) -> (String, Seen)
    where
        F: Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let seen = Seen::default();
        let record = seen.clone();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let handler = Arc::clone(&handler);
                let record = record.clone();
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let mut buffer = [0u8; 8192];
                    loop {
                        let Ok(read) = socket.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        data.extend_from_slice(&buffer[..read]);
                        let text = String::from_utf8_lossy(&data).into_owned();
                        if let Some(split) = text.find("\r\n\r\n") {
                            let head = &text[..split];
                            let length = head
                                .lines()
                                .find_map(|l| {
                                    l.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if data.len() >= split + 4 + length {
                                break;
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&data).into_owned();
                    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                    let mut first = head.lines().next().unwrap_or("").split_whitespace();
                    let method = first.next().unwrap_or("").to_owned();
                    let path = first.next().unwrap_or("").to_owned();
                    record.requests.lock().expect("lock").push((
                        method.clone(),
                        path.clone(),
                        body.to_owned(),
                    ));
                    let (status, payload) = handler(&method, &path, body);
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                });
            }
        });
        (format!("http://{address}"), seen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vision_capability_is_read_from_show() {
        assert!(capabilities_include_vision(
            &json!({"capabilities": ["completion", "vision"]})
        ));
        assert!(!capabilities_include_vision(
            &json!({"capabilities": ["completion", "tools"]})
        ));
        assert!(!capabilities_include_vision(&json!({})));
    }
    use crate::config::{shared, BrainConfig};

    fn request() -> LlmRequest {
        LlmRequest {
            messages: vec![ChatMessage::new("user", "привет")],
            schema: Some(json!({"type": "object"})),
            max_tokens: 50,
        }
    }

    #[test]
    fn only_loopback_addresses_are_accepted() {
        assert!(Ollama::new("http://127.0.0.1:11434").is_ok());
        assert!(Ollama::new("http://8.8.8.8:11434").is_err());
        assert!(Ollama::new("https://api.example.com").is_err());
    }

    #[tokio::test]
    async fn lists_models_and_reports_loaded_ones() {
        let (url, _) = mock::serve(|_, path, _| match path {
            "/api/version" => (200, r#"{"version":"0.40.16"}"#.into()),
            "/api/tags" => (
                200,
                r#"{"models":[{"name":"qwen3:4b","size":2382000000}]}"#.into(),
            ),
            "/api/ps" => (
                200,
                r#"{"models":[{"name":"qwen3:4b","size":3180000000,"size_vram":3180000000,"expires_at":"x"}]}"#.into(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let ollama = Ollama::new(&url).expect("client");
        assert_eq!(ollama.version().await.expect("version"), "0.40.16");
        assert_eq!(ollama.models().await.expect("models")[0].name, "qwen3:4b");
        assert_eq!(
            ollama.loaded().await.expect("loaded")[0].vram_bytes,
            3_180_000_000
        );
    }

    #[tokio::test]
    async fn chat_sends_schema_keep_alive_options_and_think_off() {
        let (url, seen) = mock::serve(|_, path, _| {
            if path == "/api/version" {
                return (200, r#"{"version":"0.40.16"}"#.into());
            }
            (
                200,
                r#"{"message":{"role":"assistant","content":"{\"type\":\"reply\",\"text\":\"ok\"}"},"total_duration":2000000000,"load_duration":500000000,"eval_count":12}"#.into(),
            )
        })
        .await;
        let ollama = Arc::new(Ollama::new(&url).expect("client"));
        let config = shared(BrainConfig::default());
        let llm = OllamaLlm::new(ollama, config);
        let reply = llm.complete(request()).await.expect("reply");
        assert_eq!(reply.total_ms, 2000);
        assert_eq!(reply.load_ms, 500);
        let requests = seen.requests.lock().expect("lock").clone();
        let chat = requests
            .iter()
            .find(|(_, path, _)| path == "/api/chat")
            .expect("chat request");
        let body: Value = serde_json::from_str(&chat.2).expect("json");
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["keep_alive"], json!("5m"));
        assert_eq!(body["think"], json!(false));
        assert_eq!(body["format"], json!({"type": "object"}));
        assert_eq!(body["options"]["num_ctx"], 4096);
        assert_eq!(body["options"]["num_thread"], 6);
    }

    #[tokio::test]
    async fn a_model_without_thinking_is_retried_without_the_option() {
        let (url, seen) = mock::serve(|_, _, body| {
            if body.contains("\"think\"") {
                (
                    400,
                    r#"{"error":"\"gemma3:4b\" does not support thinking"}"#.into(),
                )
            } else {
                (
                    200,
                    r#"{"message":{"role":"assistant","content":"{}"}}"#.into(),
                )
            }
        })
        .await;
        let ollama = Ollama::new(&url).expect("client");
        let options = json!({});
        let first = ollama
            .chat("gemma3:4b", &request(), &options, json!("5m"))
            .await
            .expect("retry works");
        assert_eq!(first.content, "{}");
        ollama
            .chat("gemma3:4b", &request(), &options, json!("5m"))
            .await
            .expect("second");
        let chats: Vec<_> = seen
            .requests
            .lock()
            .expect("lock")
            .iter()
            .filter(|(_, path, _)| path == "/api/chat")
            .cloned()
            .collect();
        // think -> refused -> retry; the next call does not send it at all
        assert_eq!(chats.len(), 3);
        assert!(!chats[2].2.contains("\"think\""));
    }

    #[tokio::test]
    async fn unload_sends_keep_alive_zero() {
        let (url, seen) = mock::serve(|_, _, _| (200, r#"{"done":true}"#.into())).await;
        Ollama::new(&url)
            .expect("client")
            .unload("qwen3:4b")
            .await
            .expect("unload");
        let requests = seen.requests.lock().expect("lock").clone();
        let body: Value = serde_json::from_str(&requests[0].2).expect("json");
        assert_eq!(requests[0].1, "/api/generate");
        assert_eq!(body["keep_alive"], json!(0));
        assert_eq!(body["model"], "qwen3:4b");
    }

    #[tokio::test]
    async fn http_errors_and_dead_servers_are_reported() {
        let (url, _) = mock::serve(|_, _, _| (500, "boom".into())).await;
        let error = Ollama::new(&url)
            .expect("client")
            .version()
            .await
            .expect_err("500");
        assert_eq!(error.code(), "BAD_RESPONSE");
        let dead = Ollama::new("http://127.0.0.1:1").expect("client");
        assert_eq!(
            dead.version().await.expect_err("refused").code(),
            "UNAVAILABLE"
        );
    }

    #[tokio::test]
    async fn the_cloud_client_needs_the_switch_a_key_and_sends_a_bearer_token() {
        let (url, seen) = mock::serve(|_, _, _| {
            (
                200,
                r#"{"choices":[{"message":{"content":"{\"type\":\"reply\",\"text\":\"hi\"}"}}],"usage":{"completion_tokens":7}}"#.into(),
            )
        })
        .await;
        let config = shared(BrainConfig::default());
        let key: KeyReader = Arc::new(|| Some("sk-test".to_owned()));
        let cloud = CloudLlm::new(config.clone(), key).expect("client");
        // off by default
        assert_eq!(
            cloud.complete(request()).await.expect_err("off").code(),
            "UNAVAILABLE"
        );
        {
            let mut guard = config.write().expect("lock");
            guard.cloud.enabled = true;
            guard.cloud.base_url = url;
            guard.cloud.model = "m".into();
        }
        let reply = cloud.complete(request()).await.expect("reply");
        assert_eq!(reply.eval_tokens, 7);
        let requests = seen.requests.lock().expect("lock").clone();
        assert_eq!(requests[0].1, "/chat/completions");
        let nokey: KeyReader = Arc::new(|| None);
        let cloud = CloudLlm::new(config, nokey).expect("client");
        assert_eq!(
            cloud.complete(request()).await.expect_err("no key").code(),
            "UNAVAILABLE"
        );
    }
}
