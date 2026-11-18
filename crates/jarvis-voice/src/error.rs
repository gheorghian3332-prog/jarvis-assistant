use thiserror::Error;

#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("audio device error: {0}")]
    Device(String),
    #[error("resampler error: {0}")]
    Resample(String),
    #[error("speech model error: {0}")]
    Model(String),
    #[error("audio file error: {0}")]
    File(String),
    #[error("configuration error: {0}")]
    Config(String),
}

pub type VoiceResult<T> = Result<T, VoiceError>;
