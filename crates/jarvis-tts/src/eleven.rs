//! ElevenLabs (optional cloud engine): streaming `/v1/text-to-speech/{voice_id}/stream`, PCM 24 kHz.
//!
//! The API key is read from the credential store for each request and goes only into the
//! `xi-api-key` header. If the first audio byte does not arrive within `first_byte_timeout` the
//! request fails and the service speaks the same phrase with the local engine. The voice is the
//! stock voice the user picked (`voice_id`); nothing is cloned.

use crate::{
    clip::AudioClip,
    config::{is_valid_voice_id, MAX_SPEED},
    engine::TtsEngine,
    error::{TtsError, TtsResult},
    secret::{SecretStore, ELEVEN_KEY},
    text::Lang,
};
use async_trait::async_trait;
use serde_json::json;
use std::{fmt, sync::Arc, time::Duration};
use tokio::time::{timeout, Instant};

const ENGINE: &str = "elevenlabs";
pub const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io";
const MODEL_ID: &str = "eleven_multilingual_v2";
pub const SAMPLE_RATE: u32 = 24_000;
/// ElevenLabs accepts a speed of 0.7 to 1.2.
const API_MAX_SPEED: f32 = 1.2;
/// Longest text accepted per request.
const MAX_CHARS: usize = 2_500;

#[derive(Clone)]
pub struct ElevenLabsConfig {
    pub base_url: String,
    /// Limit for the first audio byte (headers included).
    pub first_byte_timeout: Duration,
    /// Limit for the whole stream.
    pub total_timeout: Duration,
}

impl Default for ElevenLabsConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            first_byte_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
pub struct ElevenLabs {
    client: reqwest::Client,
    config: ElevenLabsConfig,
    secrets: Arc<dyn SecretStore>,
}

impl fmt::Debug for ElevenLabs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElevenLabs")
            .field("base_url", &self.config.base_url)
            .finish_non_exhaustive()
    }
}

impl ElevenLabs {
    pub fn new(config: ElevenLabsConfig, secrets: Arc<dyn SecretStore>) -> TtsResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent("jarvis-tts")
            .build()
            .map_err(|error| TtsError::unavailable(ENGINE, error.to_string()))?;
        Ok(Self {
            client,
            config,
            secrets,
        })
    }

    fn url(&self, voice: &str) -> String {
        format!(
            "{}/v1/text-to-speech/{voice}/stream?output_format=pcm_24000",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

fn request_error(error: &reqwest::Error) -> TtsError {
    // `reqwest::Error` can contain the URL; it never contains the header, but keep the text short.
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    TtsError::synthesis(ENGINE, kind)
}

#[async_trait]
impl TtsEngine for ElevenLabs {
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
        if voice.is_empty() || !is_valid_voice_id(voice) {
            return Err(TtsError::unavailable(ENGINE, "no valid voice id"));
        }
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_CHARS {
            return Err(TtsError::Invalid(format!(
                "text must be 1 to {MAX_CHARS} characters"
            )));
        }
        let key = self
            .secrets
            .get(ELEVEN_KEY)?
            .ok_or_else(|| TtsError::unavailable(ENGINE, "no API key stored"))?;
        let body = json!({
            "text": text,
            "model_id": MODEL_ID,
            "voice_settings": { "speed": speed.clamp(0.7, API_MAX_SPEED.min(MAX_SPEED)) },
        });
        let started = Instant::now();
        let first_byte = self.config.first_byte_timeout;
        let send = async {
            let mut response = self
                .client
                .post(self.url(voice))
                .header("xi-api-key", key.expose())
                .header("accept", "audio/pcm")
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await
                .map_err(|error| request_error(&error))?;
            if !response.status().is_success() {
                return Err(TtsError::synthesis(
                    ENGINE,
                    format!("HTTP {}", response.status().as_u16()),
                ));
            }
            let first = response
                .chunk()
                .await
                .map_err(|error| request_error(&error))?;
            Ok((response, first))
        };
        let (mut response, first) = match timeout(first_byte, send).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(TtsError::Timeout {
                    engine: ENGINE,
                    limit: first_byte,
                })
            }
        };
        drop(key);

        let mut bytes: Vec<u8> = first.map(|chunk| chunk.to_vec()).unwrap_or_default();
        let remaining = self.config.total_timeout.saturating_sub(started.elapsed());
        let rest = async {
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| request_error(&error))?
            {
                bytes.extend_from_slice(&chunk);
            }
            Ok::<(), TtsError>(())
        };
        match timeout(remaining, rest).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(TtsError::Timeout {
                    engine: ENGINE,
                    limit: self.config.total_timeout,
                })
            }
        }
        if bytes.len() < 2 {
            return Err(TtsError::synthesis(ENGINE, "empty audio"));
        }
        let (pairs, _) = bytes.as_chunks::<2>();
        let pcm = pairs.iter().map(|pair| i16::from_le_bytes(*pair)).collect();
        Ok(AudioClip::new(pcm, SAMPLE_RATE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::{MemoryStore, SecretString};
    use std::sync::Mutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    const TEST_KEY: &str = "test-key-not-a-real-secret";

    /// What the mock server saw and how it answers.
    #[derive(Clone, Default)]
    struct Mock {
        status: u16,
        first_delay: Duration,
        body: Vec<u8>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    async fn serve(mock: Mock) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mock = mock.clone();
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let mut buffer = [0u8; 4096];
                    let header_end = loop {
                        let Ok(read) = socket.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        data.extend_from_slice(&buffer[..read]);
                        if let Some(position) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                            break position + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while data.len() < header_end + length {
                        let Ok(read) = socket.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            break;
                        }
                        data.extend_from_slice(&buffer[..read]);
                    }
                    let request = String::from_utf8_lossy(&data).to_string();
                    if let Ok(mut seen) = mock.seen.lock() {
                        seen.push(request);
                    }
                    tokio::time::sleep(mock.first_delay).await;
                    let reply = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: audio/pcm\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        mock.status,
                        mock.body.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let (first, second) = mock.body.split_at(mock.body.len() / 2);
                    let _ = socket.write_all(first).await;
                    let _ = socket.flush().await;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let _ = socket.write_all(second).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://{address}")
    }

    fn engine(base_url: String, with_key: bool, first_byte: Duration) -> ElevenLabs {
        let store = MemoryStore::default();
        if with_key {
            store
                .set(ELEVEN_KEY, SecretString::new(TEST_KEY))
                .expect("set key");
        }
        ElevenLabs::new(
            ElevenLabsConfig {
                base_url,
                first_byte_timeout: first_byte,
                total_timeout: Duration::from_secs(5),
            },
            Arc::new(store),
        )
        .expect("engine")
    }

    fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[tokio::test]
    async fn streams_pcm_and_sends_the_key_only_in_the_header() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve(Mock {
            status: 200,
            body: pcm_bytes(&[100, -200, 300, -400, 500]),
            seen: seen.clone(),
            ..Mock::default()
        })
        .await;
        let clip = engine(url, true, Duration::from_secs(2))
            .synth("Привет, мир", Lang::Ru, "voice123", 1.0)
            .await
            .expect("clip");
        assert_eq!(clip.pcm, [100, -200, 300, -400, 500]);
        assert_eq!(clip.sample_rate, 24_000);
        let request = seen.lock().expect("seen")[0].clone();
        assert!(
            request.starts_with("POST /v1/text-to-speech/voice123/stream?output_format=pcm_24000")
        );
        assert!(request
            .to_lowercase()
            .contains(&format!("xi-api-key: {TEST_KEY}")));
        let body = request.split("\r\n\r\n").nth(1).expect("body");
        assert!(!body.contains(TEST_KEY), "the key is never in the body");
        assert!(body.contains("Привет, мир"));
        assert!(body.contains("eleven_multilingual_v2"));
    }

    #[tokio::test]
    async fn a_slow_first_byte_fails_fast_so_the_caller_can_fall_back() {
        let url = serve(Mock {
            status: 200,
            first_delay: Duration::from_millis(1_500),
            body: pcm_bytes(&[1, 2, 3, 4]),
            ..Mock::default()
        })
        .await;
        let started = Instant::now();
        let error = engine(url, true, Duration::from_millis(200))
            .synth("Hello", Lang::En, "voice123", 1.0)
            .await
            .expect_err("too slow");
        assert!(matches!(error, TtsError::Timeout { .. }), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!error.to_string().contains(TEST_KEY));
    }

    #[tokio::test]
    async fn http_errors_are_reported_without_the_key() {
        let url = serve(Mock {
            status: 401,
            body: b"{\"detail\":\"bad key\"}".to_vec(),
            ..Mock::default()
        })
        .await;
        let error = engine(url, true, Duration::from_secs(2))
            .synth("Hello", Lang::En, "voice123", 1.0)
            .await
            .expect_err("401");
        assert!(error.to_string().contains("401"), "{error}");
        assert!(!format!("{error:?}").contains(TEST_KEY));
    }

    #[tokio::test]
    async fn no_key_or_voice_means_no_request_at_all() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve(Mock {
            status: 200,
            body: pcm_bytes(&[1, 2]),
            seen: seen.clone(),
            ..Mock::default()
        })
        .await;
        let no_key = engine(url.clone(), false, Duration::from_secs(1));
        assert!(matches!(
            no_key.synth("Hi", Lang::En, "voice123", 1.0).await,
            Err(TtsError::Unavailable { .. })
        ));
        let with_key = engine(url, true, Duration::from_secs(1));
        assert!(with_key.synth("Hi", Lang::En, "", 1.0).await.is_err());
        assert!(with_key.synth("Hi", Lang::En, "../x", 1.0).await.is_err());
        assert!(with_key
            .synth("   ", Lang::En, "voice123", 1.0)
            .await
            .is_err());
        assert!(seen.lock().expect("seen").is_empty());
    }

    #[tokio::test]
    async fn connection_failure_is_an_error_not_a_hang() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        drop(listener);
        let error = engine(
            format!("http://{address}"),
            true,
            Duration::from_millis(500),
        )
        .synth("Hi", Lang::En, "voice123", 1.0)
        .await
        .expect_err("refused");
        assert!(
            matches!(error, TtsError::Synthesis { .. } | TtsError::Timeout { .. }),
            "{error}"
        );
    }

    #[test]
    fn debug_output_has_no_secrets() {
        let text = format!(
            "{:?}",
            engine("http://x".into(), true, Duration::from_secs(1))
        );
        assert!(!text.contains(TEST_KEY));
    }
}
