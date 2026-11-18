//! Always-on voice front, Rust only:
//! cpal -> mono -> RNNoise (48 kHz) -> 16 kHz -> WebRTC VAD -> speech segmenter -> Vosk (wake word),
//! plus push-to-talk. Finished phrases leave as `Event::Utterance` on the bus.

pub mod child;
pub mod collector;
pub mod config;
pub mod denoise;
pub mod engine;
pub mod error;
pub mod level;
pub mod pipeline;
pub mod resample;
pub mod router;
pub mod segmenter;
pub mod service;
pub mod source;
pub mod vad;
pub mod wake;
pub mod wake_worker;

pub use child::{ChildWake, WorkerCommand};
pub use config::{
    confidence_threshold, models_dir, validate_wake_names, vosk_model_dir, VoiceConfig,
    DEFAULT_WAKE_NAMES,
};
pub use engine::{process_wav, process_wav_with, Command, EngineHandle, EngineKind, WakeMode};
pub use error::{VoiceError, VoiceResult};
pub use router::{
    default_transcriber, find_repo_root, Router, RouterService, Spoken, Transcriber, VoiceControl,
};
pub use service::{VoiceService, VoiceSnapshot};
pub use source::{list_input_devices, AudioSource, InputDevice, MicSource, WavSource};
