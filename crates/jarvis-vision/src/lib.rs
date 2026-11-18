//! Screen vision on demand (P8-A): capture (monitor or active window, memory only), OCR through
//! Windows.Media.Ocr, and a local vision model through Ollama.
//!
//! Hard rules: no background or periodic capture; pictures are never written to disk except by the
//! explicit [`Vision::save_screenshot`]; screen content leaves the machine only when both
//! `ai.cloud_enabled` and `vision.allow_cloud` are true.

#![allow(clippy::chunks_exact_to_as_chunks)] // pixel loops read clearer with chunks_exact(4)

pub mod capture;
pub mod error;
pub mod frame;
pub mod ocr;
pub mod ollama;
pub mod service;

pub use capture::{
    active_window_info, capture_active_window, capture_primary_monitor, capture_window, WindowInfo,
};
pub use error::{Result, VisionError};
pub use frame::Frame;
pub use ocr::{recognize, OcrLine, OcrOutput, OcrWord, TextRect};
pub use ollama::{check_destination, is_local_endpoint, CloudPolicy, OllamaClient};
pub use service::{
    build_prompt, pictures_dir, save_png_into, Answer, AnswerSource, CaptureObserver, NoObserver,
    Vision, VisionConfig, WindowReading, VLM_MAX_SIDE,
};
