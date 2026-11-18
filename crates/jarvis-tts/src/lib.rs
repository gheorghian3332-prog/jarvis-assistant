//! Speech output for J.A.R.V.I.S.: Piper (Russian), Kokoro (English), optional ElevenLabs, sentence
//! pipeline, playback with immediate interruption and the RMS level envelope.

pub mod clip;
pub mod config;
pub mod eleven;
pub mod engine;
pub mod error;
pub mod kokoro;
pub mod piper;
pub mod playback;
pub mod secret;
pub mod service;
pub mod text;
pub mod voices;

pub use clip::AudioClip;
pub use config::{
    is_valid_eleven_voice_id, is_valid_voice_id, models_dir, EngineKind, TtsConfig,
    DEFAULT_VOICE_EN, DEFAULT_VOICE_RU, MAX_SPEED, MIN_SPEED,
};
pub use eleven::{ElevenLabs, ElevenLabsConfig};
pub use engine::{build_chain, synth_with_fallback, Candidate, Engines, Synthesized, TtsEngine};
pub use error::{TtsError, TtsResult};
pub use kokoro::KokoroEngine;
pub use piper::{PiperConfig, PiperEngine};
pub use playback::{list_output_devices, OutputDevice, OutputSink, PlayQueue};
pub use secret::{
    CredentialStore, MemoryStore, SecretStore, SecretString, AI_CLOUD_KEY, ELEVEN_KEY, SECRET_NAMES,
};
pub use service::{
    NoObserver, SpeechHandle, SpeechObserver, SpeechOutcome, SpeechReport, TtsService, ECHO_TAIL,
};
pub use text::{detect_lang, split_sentences, Lang};
pub use voices::{EngineStatus, Inventory, VoiceInfo, VoicesReport};
