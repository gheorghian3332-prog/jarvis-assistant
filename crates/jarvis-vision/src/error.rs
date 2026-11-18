use thiserror::Error;

/// Everything that can go wrong in the vision pipeline.
#[derive(Debug, Error)]
pub enum VisionError {
    #[error("vision is disabled in settings (vision.enabled = false)")]
    Disabled,
    #[error("screen capture failed: {0}")]
    Capture(String),
    #[error("OCR failed: {0}")]
    Ocr(String),
    #[error("image error: {0}")]
    Image(String),
    #[error("LLM request failed: {0}")]
    Llm(String),
    #[error("model {0} is not installed in Ollama")]
    ModelMissing(String),
    #[error("{0}")]
    CloudDenied(String),
    #[error("file error: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, VisionError>;
