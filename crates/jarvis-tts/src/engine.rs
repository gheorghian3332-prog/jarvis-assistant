//! The engine contract and the fallback chain.

use crate::{
    clip::AudioClip,
    config::{EngineKind, TtsConfig},
    error::{TtsError, TtsResult},
    text::Lang,
};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::warn;

/// A speech synthesizer: text in, PCM out. Engines are shared and may be called concurrently;
/// each one serializes its own backend.
#[async_trait]
pub trait TtsEngine: Send + Sync {
    fn name(&self) -> &'static str;

    /// `voice` is an id the engine understands; `speed` is 0.7..1.4 (1.0 = normal).
    async fn synth(&self, text: &str, lang: Lang, voice: &str, speed: f32) -> TtsResult<AudioClip>;

    /// Releases what the engine holds (processes, connections). Nothing by default.
    async fn shutdown(&self) {}
}

/// One step of the chain: an engine and the voice to ask it for.
#[derive(Clone)]
pub struct Candidate {
    pub engine: Arc<dyn TtsEngine>,
    pub voice: String,
}

/// The engines the service can choose from.
#[derive(Clone)]
pub struct Engines {
    pub piper: Arc<dyn TtsEngine>,
    pub kokoro: Arc<dyn TtsEngine>,
    pub cloud: Option<Arc<dyn TtsEngine>>,
}

impl Engines {
    fn local(&self, kind: EngineKind) -> Arc<dyn TtsEngine> {
        match kind {
            EngineKind::Piper => Arc::clone(&self.piper),
            EngineKind::Kokoro => Arc::clone(&self.kokoro),
        }
    }
}

/// Order of attempts for one phrase: the cloud (only when it is enabled, a voice id is set and the
/// key exists), then the local engine of the phrase's language.
pub fn build_chain(
    engines: &Engines,
    config: &TtsConfig,
    lang: Lang,
    cloud_key_present: bool,
) -> Vec<Candidate> {
    let mut chain = Vec::new();
    if config.cloud_enabled && cloud_key_present && !config.eleven_voice_id.is_empty() {
        if let Some(cloud) = &engines.cloud {
            chain.push(Candidate {
                engine: Arc::clone(cloud),
                voice: config.eleven_voice_id.clone(),
            });
        }
    }
    let (kind, voice) = config.local_for(lang);
    chain.push(Candidate {
        engine: engines.local(kind),
        voice,
    });
    chain
}

/// Result of a successful synthesis through the chain.
#[derive(Debug)]
pub struct Synthesized {
    pub clip: AudioClip,
    /// Name of the engine that produced the clip.
    pub engine: &'static str,
    /// Names of the engines that failed before it, in order.
    pub skipped: Vec<&'static str>,
}

/// Tries the candidates in order; the first clip wins. Fails with the last error if all fail.
pub async fn synth_with_fallback(
    chain: &[Candidate],
    text: &str,
    lang: Lang,
    speed: f32,
) -> TtsResult<Synthesized> {
    let mut skipped = Vec::new();
    let mut last_error = None;
    for candidate in chain {
        let name = candidate.engine.name();
        match candidate
            .engine
            .synth(text, lang, &candidate.voice, speed)
            .await
        {
            Ok(clip) if !clip.pcm.is_empty() => {
                return Ok(Synthesized {
                    clip,
                    engine: name,
                    skipped,
                })
            }
            Ok(_) => {
                warn!(engine = name, "engine returned no audio");
                last_error = Some(TtsError::synthesis(name, "no audio"));
            }
            Err(error) => {
                warn!(engine = name, %error, "engine failed; trying the next one");
                last_error = Some(error);
            }
        }
        skipped.push(name);
    }
    Err(last_error.unwrap_or_else(|| TtsError::unavailable("tts", "no engine configured")))
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Test engine: returns a clip of constant samples, or fails.
    pub struct FakeEngine {
        pub name: &'static str,
        pub fail: bool,
        pub rate: u32,
        pub level: i16,
        pub millis: usize,
        pub delay: std::time::Duration,
        pub calls: Mutex<Vec<(String, Lang, String, f32)>>,
    }

    impl FakeEngine {
        pub fn new(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                fail: false,
                rate: 24_000,
                level: 8_000,
                millis: 200,
                delay: std::time::Duration::ZERO,
                calls: Mutex::new(Vec::new()),
            })
        }

        pub fn failing(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                fail: true,
                rate: 24_000,
                level: 8_000,
                millis: 200,
                delay: std::time::Duration::ZERO,
                calls: Mutex::new(Vec::new()),
            })
        }

        pub fn with(name: &'static str, millis: usize, delay: std::time::Duration) -> Arc<Self> {
            Arc::new(Self {
                name,
                fail: false,
                rate: 24_000,
                level: 8_000,
                millis,
                delay,
                calls: Mutex::new(Vec::new()),
            })
        }

        pub fn call_count(&self) -> usize {
            self.calls.lock().map_or(0, |calls| calls.len())
        }
    }

    #[async_trait]
    impl TtsEngine for FakeEngine {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn synth(
            &self,
            text: &str,
            lang: Lang,
            voice: &str,
            speed: f32,
        ) -> TtsResult<AudioClip> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((text.to_owned(), lang, voice.to_owned(), speed));
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self.fail {
                return Err(TtsError::synthesis(self.name, "fake failure"));
            }
            let samples = self.rate as usize * self.millis / 1000;
            Ok(AudioClip::new(vec![self.level; samples], self.rate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeEngine;
    use super::*;

    fn engines(cloud: Option<Arc<FakeEngine>>) -> (Engines, Arc<FakeEngine>, Arc<FakeEngine>) {
        let piper = FakeEngine::new("piper");
        let kokoro = FakeEngine::new("kokoro");
        let set = Engines {
            piper: piper.clone(),
            kokoro: kokoro.clone(),
            cloud: cloud.map(|engine| engine as Arc<dyn TtsEngine>),
        };
        (set, piper, kokoro)
    }

    fn names(chain: &[Candidate]) -> Vec<&'static str> {
        chain.iter().map(|c| c.engine.name()).collect()
    }

    #[test]
    fn russian_uses_piper_and_english_uses_kokoro_by_default() {
        let (set, _, _) = engines(None);
        let config = TtsConfig::default();
        assert_eq!(
            names(&build_chain(&set, &config, Lang::Ru, false)),
            ["piper"]
        );
        let en = build_chain(&set, &config, Lang::En, false);
        assert_eq!(names(&en), ["kokoro"]);
        assert_eq!(en[0].voice, "bm_george");
    }

    #[test]
    fn the_cloud_goes_first_only_when_enabled_keyed_and_configured() {
        let (set, _, _) = engines(Some(FakeEngine::new("elevenlabs")));
        let mut config = TtsConfig {
            cloud_enabled: true,
            eleven_voice_id: "voice123".into(),
            ..TtsConfig::default()
        };
        let chain = build_chain(&set, &config, Lang::Ru, true);
        assert_eq!(names(&chain), ["elevenlabs", "piper"]);
        assert_eq!(chain[0].voice, "voice123");
        assert_eq!(
            names(&build_chain(&set, &config, Lang::Ru, false)),
            ["piper"]
        );
        config.eleven_voice_id.clear();
        assert_eq!(
            names(&build_chain(&set, &config, Lang::Ru, true)),
            ["piper"]
        );
        config.eleven_voice_id = "voice123".into();
        config.cloud_enabled = false;
        assert_eq!(
            names(&build_chain(&set, &config, Lang::Ru, true)),
            ["piper"]
        );
        let (no_cloud, _, _) = engines(None);
        config.cloud_enabled = true;
        assert_eq!(
            names(&build_chain(&no_cloud, &config, Lang::En, true)),
            ["kokoro"]
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_next_engine_for_the_same_phrase() {
        let cloud = FakeEngine::failing("elevenlabs");
        let (set, piper, _) = engines(Some(cloud.clone()));
        let config = TtsConfig {
            cloud_enabled: true,
            eleven_voice_id: "v".into(),
            ..TtsConfig::default()
        };
        let chain = build_chain(&set, &config, Lang::Ru, true);
        let result = synth_with_fallback(&chain, "Привет", Lang::Ru, 1.0)
            .await
            .expect("falls back");
        assert_eq!(result.engine, "piper");
        assert_eq!(result.skipped, ["elevenlabs"]);
        assert_eq!(cloud.call_count(), 1);
        let calls = piper.calls.lock().expect("calls");
        assert_eq!(
            calls[0].0, "Привет",
            "the same phrase reaches the local engine"
        );
    }

    #[tokio::test]
    async fn reports_the_last_error_when_everything_fails() {
        let set = Engines {
            piper: FakeEngine::failing("piper"),
            kokoro: FakeEngine::failing("kokoro"),
            cloud: Some(FakeEngine::failing("elevenlabs")),
        };
        let config = TtsConfig {
            cloud_enabled: true,
            eleven_voice_id: "v".into(),
            ..TtsConfig::default()
        };
        let chain = build_chain(&set, &config, Lang::En, true);
        let error = synth_with_fallback(&chain, "Hi", Lang::En, 1.0)
            .await
            .expect_err("all fail");
        assert!(error.to_string().contains("kokoro"), "{error}");
        assert!(synth_with_fallback(&[], "Hi", Lang::En, 1.0).await.is_err());
    }

    #[tokio::test]
    async fn an_empty_clip_counts_as_a_failure() {
        let empty = FakeEngine::with("kokoro", 0, std::time::Duration::ZERO);
        let candidate = Candidate {
            engine: empty,
            voice: "bm_george".into(),
        };
        assert!(synth_with_fallback(&[candidate], "Hi", Lang::En, 1.0)
            .await
            .is_err());
    }
}
