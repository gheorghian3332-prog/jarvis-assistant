//! Minimal Ollama chat client plus the cloud policy: screen content (images and OCR text) goes
//! only to a loopback endpoint unless BOTH `ai.cloud_enabled` and `vision.allow_cloud` are set.

use crate::error::{Result, VisionError};
use base64::Engine;
use serde_json::json;
use std::net::IpAddr;
use std::time::Duration;

/// The two flags that together allow screen content to leave the machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudPolicy {
    /// `ai.cloud_enabled`.
    pub ai_cloud_enabled: bool,
    /// `vision.allow_cloud` (default false).
    pub vision_allow_cloud: bool,
}

impl CloudPolicy {
    pub fn permits_remote(&self) -> bool {
        self.ai_cloud_enabled && self.vision_allow_cloud
    }
}

/// True when `endpoint` points at this machine (localhost or a loopback IP).
pub fn is_local_endpoint(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Fails unless sending screen content to `endpoint` is allowed by the policy.
pub fn check_destination(endpoint: &str, policy: CloudPolicy) -> Result<()> {
    if is_local_endpoint(endpoint) || policy.permits_remote() {
        return Ok(());
    }
    Err(VisionError::CloudDenied(
        "screen content is only sent to a local model; a remote endpoint needs both \
         ai.cloud_enabled and vision.allow_cloud"
            .into(),
    ))
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    endpoint: String,
    policy: CloudPolicy,
    timeout: Duration,
}

impl OllamaClient {
    pub fn new(endpoint: impl Into<String>, policy: CloudPolicy, timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            policy,
            timeout,
        }
    }

    /// One non-streaming chat turn; `images` are raw PNG/JPEG bytes (base64 is done here).
    pub async fn chat(&self, model: &str, prompt: &str, images: &[Vec<u8>]) -> Result<String> {
        // Checked before any connection is made.
        check_destination(&self.endpoint, self.policy)?;
        let encoded: Vec<String> = images
            .iter()
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
            .collect();
        let mut message = json!({"role": "user", "content": prompt});
        if !encoded.is_empty() {
            message["images"] = json!(encoded);
        }
        let body = json!({
            "model": model,
            "messages": [message],
            "stream": false,
            "options": {"temperature": 0.2},
        });
        let mut builder = reqwest::Client::builder().timeout(self.timeout);
        if is_local_endpoint(&self.endpoint) {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|error| VisionError::Llm(error.to_string()))?;
        let response = client
            .post(format!("{}/api/chat", self.endpoint))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|error| VisionError::Llm(short_error(error)))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| VisionError::Llm(short_error(error)))?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(VisionError::ModelMissing(model.to_owned()));
        }
        if !status.is_success() {
            let detail: String = text.chars().take(200).collect();
            return Err(VisionError::Llm(format!("HTTP {status}: {detail}")));
        }
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| VisionError::Llm(error.to_string()))?;
        value["message"]["content"]
            .as_str()
            .map(|answer| answer.trim().to_owned())
            .filter(|answer| !answer.is_empty())
            .ok_or_else(|| VisionError::Llm("the model returned an empty answer".into()))
    }
}

fn short_error(error: reqwest::Error) -> String {
    if error.is_connect() {
        "cannot connect to Ollama (is it running?)".into()
    } else if error.is_timeout() {
        "the model did not answer in time".into()
    } else {
        error.without_url().to_string()
    }
}
