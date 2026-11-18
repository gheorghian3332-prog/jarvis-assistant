use std::path::PathBuf;

pub const DEFAULT_WAKE_NAMES: [&str; 3] = ["лёха", "лёх", "алексей"];
pub const MAX_WAKE_NAMES: usize = 5;
pub const MAX_NAME_CHARS: usize = 20;
/// Name of the Vosk model folder inside the models directory.
pub const VOSK_MODEL_NAME: &str = "vosk-model-small-ru-0.22";

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceConfig {
    /// Permanent listening (wake word). Off by default for privacy.
    pub enabled: bool,
    /// Input device name; `None` means the system default.
    pub input_device: Option<String>,
    /// 0.0..=1.0; higher means more sensitive (more detections, more false alarms).
    pub sensitivity: f32,
    pub wake_names: Vec<String>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            input_device: None,
            sensitivity: 0.5,
            wake_names: DEFAULT_WAKE_NAMES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        }
    }
}

fn is_name_char(character: char) -> bool {
    character.is_ascii_alphabetic() || matches!(character, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')
}

/// Wake names: 1 to 5 single words of up to 20 Cyrillic or Latin letters.
pub fn validate_wake_names(names: &[String]) -> Result<(), String> {
    if names.is_empty() || names.len() > MAX_WAKE_NAMES {
        return Err(format!("expected 1 to {MAX_WAKE_NAMES} names"));
    }
    for name in names {
        let length = name.chars().count();
        if length == 0 || length > MAX_NAME_CHARS {
            return Err(format!("a name must have 1 to {MAX_NAME_CHARS} characters"));
        }
        if !name.chars().all(is_name_char) {
            return Err("names may only contain Cyrillic or Latin letters".into());
        }
    }
    Ok(())
}

/// Lower case with `ё` folded to `е`, for comparing spoken and configured names.
pub fn normalize_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|character| if character == 'ё' { 'е' } else { character })
        .collect()
}

/// WebRTC VAD aggressiveness for a sensitivity: 0 (most permissive) to 3 (very aggressive).
pub fn vad_aggressiveness(sensitivity: f32) -> u8 {
    let sensitivity = sensitivity.clamp(0.0, 1.0);
    if sensitivity >= 0.8 {
        0
    } else if sensitivity >= 0.55 {
        1
    } else if sensitivity >= 0.3 {
        2
    } else {
        3
    }
}

/// Minimal Vosk word confidence accepted as the wake word.
pub fn confidence_threshold(sensitivity: f32) -> f32 {
    0.9 - 0.7 * sensitivity.clamp(0.0, 1.0)
}

/// `%LOCALAPPDATA%\Jarvis\models`, or `JARVIS_MODELS_DIR` when set.
pub fn models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JARVIS_MODELS_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join("Jarvis").join("models")
}

pub fn vosk_model_dir() -> PathBuf {
    models_dir().join(VOSK_MODEL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn accepts_default_names() {
        assert_eq!(
            validate_wake_names(&VoiceConfig::default().wake_names),
            Ok(())
        );
        assert_eq!(validate_wake_names(&names(&["Jarvis", "Лёха"])), Ok(()));
    }

    #[test]
    fn rejects_bad_names() {
        assert!(validate_wake_names(&[]).is_err());
        assert!(validate_wake_names(&names(&["а", "б", "в", "г", "д", "е"])).is_err());
        assert!(validate_wake_names(&names(&[""])).is_err());
        assert!(validate_wake_names(&names(&["a\"b"])).is_err());
        assert!(validate_wake_names(&names(&["two words"])).is_err());
        assert!(validate_wake_names(&names(&["ааааааааааааааааааааа"])).is_err());
    }

    #[test]
    fn normalizes_yo() {
        assert_eq!(normalize_name(" Лёха "), "леха");
        assert_eq!(normalize_name("АЛЕКСЕЙ"), "алексей");
    }

    #[test]
    fn sensitivity_maps_to_monotonic_settings() {
        assert_eq!(vad_aggressiveness(0.0), 3);
        assert_eq!(vad_aggressiveness(0.5), 2);
        assert_eq!(vad_aggressiveness(0.6), 1);
        assert_eq!(vad_aggressiveness(1.0), 0);
        assert!(confidence_threshold(0.0) > confidence_threshold(0.5));
        assert!(confidence_threshold(0.5) > confidence_threshold(1.0));
        assert!((confidence_threshold(0.5) - 0.55).abs() < 1e-6);
    }
}
