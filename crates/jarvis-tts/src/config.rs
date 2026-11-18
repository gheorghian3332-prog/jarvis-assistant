//! Speech settings (`tts.*`) and the folders the engines use.

use crate::text::Lang;
use serde::Serialize;
use std::path::PathBuf;

pub const MIN_SPEED: f32 = 0.7;
pub const MAX_SPEED: f32 = 1.4;
/// The voice chosen for Russian (see the P5-A report for the comparison).
pub const DEFAULT_VOICE_RU: &str = "ru_RU-dmitri-medium";
pub const DEFAULT_VOICE_EN: &str = "bm_george";
/// Piper voice used for English when `tts.engine_en = "piper"` (not shipped by default).
pub const DEFAULT_PIPER_VOICE_EN: &str = "en_GB-alan-medium";

/// A local synthesis engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Piper,
    Kokoro,
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Piper => "piper",
            Self::Kokoro => "kokoro",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "piper" => Some(Self::Piper),
            "kokoro" => Some(Self::Kokoro),
            _ => None,
        }
    }
}

/// Settings of the speech output; mirrors the `tts.*` keys.
#[derive(Debug, Clone, PartialEq)]
pub struct TtsConfig {
    pub engine_ru: EngineKind,
    pub voice_ru: String,
    pub engine_en: EngineKind,
    pub voice_en: String,
    pub speed: f32,
    pub volume: f32,
    pub output_device: Option<String>,
    pub cloud_enabled: bool,
    pub eleven_voice_id: String,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine_ru: EngineKind::Piper,
            voice_ru: DEFAULT_VOICE_RU.to_owned(),
            engine_en: EngineKind::Kokoro,
            voice_en: DEFAULT_VOICE_EN.to_owned(),
            speed: 1.0,
            volume: 0.9,
            output_device: None,
            cloud_enabled: false,
            eleven_voice_id: String::new(),
        }
    }
}

impl TtsConfig {
    /// Engine and voice for a language, with the English voice mapped to what the engine understands.
    pub fn local_for(&self, lang: Lang) -> (EngineKind, String) {
        match lang {
            Lang::Ru => (self.engine_ru, self.voice_ru.clone()),
            Lang::En => {
                let voice = match self.engine_en {
                    EngineKind::Kokoro => self.voice_en.clone(),
                    // A Kokoro voice id (`bm_george`) means nothing to Piper.
                    EngineKind::Piper if is_kokoro_voice(&self.voice_en) => {
                        DEFAULT_PIPER_VOICE_EN.to_owned()
                    }
                    EngineKind::Piper => self.voice_en.clone(),
                };
                (self.engine_en, voice)
            }
        }
    }

    pub fn speed_clamped(&self) -> f32 {
        self.speed.clamp(MIN_SPEED, MAX_SPEED)
    }

    pub fn volume_clamped(&self) -> f32 {
        self.volume.clamp(0.0, 1.0)
    }
}

/// Voice ids are used in file names and URLs: ASCII letters, digits, `_` and `-` only.
pub fn is_valid_voice_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `bm_george`, `af_sarah`: two letters, an underscore and a name.
pub fn is_kokoro_voice(id: &str) -> bool {
    let mut parts = id.splitn(2, '_');
    matches!(
        (parts.next(), parts.next()),
        (Some(prefix), Some(name))
            if prefix.len() == 2
                && prefix.chars().all(|c| c.is_ascii_lowercase())
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_lowercase())
    )
}

/// The ElevenLabs voice id is empty (cloud not used) or a URL-safe id.
pub fn is_valid_eleven_voice_id(id: &str) -> bool {
    id.is_empty() || is_valid_voice_id(id)
}

/// `%LOCALAPPDATA%\Jarvis\models` (or `JARVIS_MODELS_DIR`).
pub fn models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JARVIS_MODELS_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Jarvis").join("models")
}

/// Folder for the short-lived WAV files of Piper (`%LOCALAPPDATA%\Jarvis\tmp\tts`).
pub fn temp_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Jarvis").join("tmp").join("tts")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let config = TtsConfig::default();
        assert_eq!(config.engine_ru, EngineKind::Piper);
        assert_eq!(config.engine_en, EngineKind::Kokoro);
        assert_eq!(config.voice_en, "bm_george");
        assert_eq!(config.speed, 1.0);
        assert_eq!(config.volume, 0.9);
        assert_eq!(config.output_device, None);
        assert!(!config.cloud_enabled, "the cloud is off by default");
        assert!(config.eleven_voice_id.is_empty());
    }

    #[test]
    fn voice_ids_cannot_escape_the_models_folder() {
        for good in ["ru_RU-dmitri-medium", "bm_george", "abc123", "x"] {
            assert!(is_valid_voice_id(good), "{good}");
        }
        for bad in ["", "..", "a/b", "a\\b", "a.b", "a b", "ру", &"x".repeat(65)] {
            assert!(!is_valid_voice_id(bad), "{bad}");
        }
        assert!(is_valid_eleven_voice_id(""));
        assert!(is_valid_eleven_voice_id("21m00Tcm4TlvDq8ikWAM"));
        assert!(!is_valid_eleven_voice_id("../x"));
    }

    #[test]
    fn kokoro_voice_ids_are_recognized() {
        assert!(is_kokoro_voice("bm_george"));
        assert!(is_kokoro_voice("bm_lewis"));
        assert!(!is_kokoro_voice("en_GB-alan-medium"));
        assert!(!is_kokoro_voice("george"));
    }

    #[test]
    fn english_piper_does_not_get_a_kokoro_voice_id() {
        let mut config = TtsConfig::default();
        assert_eq!(
            config.local_for(Lang::En),
            (EngineKind::Kokoro, "bm_george".to_owned())
        );
        config.engine_en = EngineKind::Piper;
        assert_eq!(
            config.local_for(Lang::En),
            (EngineKind::Piper, DEFAULT_PIPER_VOICE_EN.to_owned())
        );
        config.voice_en = "en_US-ryan-high".into();
        assert_eq!(
            config.local_for(Lang::En),
            (EngineKind::Piper, "en_US-ryan-high".to_owned())
        );
        assert_eq!(
            config.local_for(Lang::Ru),
            (EngineKind::Piper, DEFAULT_VOICE_RU.to_owned())
        );
    }

    #[test]
    fn speed_and_volume_are_clamped() {
        let config = TtsConfig {
            speed: 5.0,
            volume: -1.0,
            ..TtsConfig::default()
        };
        assert_eq!(config.speed_clamped(), MAX_SPEED);
        assert_eq!(config.volume_clamped(), 0.0);
    }
}
