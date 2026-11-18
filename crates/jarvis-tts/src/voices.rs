//! What `tts_voices` reports: engines with their status and the voices that can be selected.

use crate::{
    config::{EngineKind, TtsConfig, DEFAULT_PIPER_VOICE_EN, DEFAULT_VOICE_RU},
    kokoro::KOKORO_VOICES,
    text::Lang,
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineStatus {
    pub engine: &'static str,
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceInfo {
    pub engine: &'static str,
    pub id: String,
    pub lang: &'static str,
    pub installed: bool,
    /// This is the voice currently chosen for its language and engine.
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoicesReport {
    pub engines: Vec<EngineStatus>,
    pub voices: Vec<VoiceInfo>,
}

/// What is installed and configured on this machine.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub piper_exe: bool,
    pub piper_voices: Vec<String>,
    pub kokoro_model: bool,
    pub kokoro_worker: bool,
    pub cloud_key: bool,
}

fn piper_lang(id: &str) -> &'static str {
    match id.get(..2) {
        Some("ru") => "ru",
        Some("en") => "en",
        _ => "other",
    }
}

pub fn report(config: &TtsConfig, inventory: &Inventory) -> VoicesReport {
    let (ru_engine, ru_voice) = config.local_for(Lang::Ru);
    let (en_engine, en_voice) = config.local_for(Lang::En);
    let chosen = |engine: EngineKind, id: &str| {
        (ru_engine == engine && ru_voice == id) || (en_engine == engine && en_voice == id)
    };

    let mut voices = Vec::new();
    let mut piper_ids = inventory.piper_voices.clone();
    for wanted in [DEFAULT_VOICE_RU, DEFAULT_PIPER_VOICE_EN] {
        if !piper_ids.iter().any(|id| id == wanted) {
            piper_ids.push(wanted.to_owned());
        }
    }
    for id in piper_ids {
        voices.push(VoiceInfo {
            engine: "piper",
            installed: inventory.piper_voices.contains(&id),
            selected: chosen(EngineKind::Piper, &id),
            lang: piper_lang(&id),
            id,
        });
    }
    for id in KOKORO_VOICES {
        voices.push(VoiceInfo {
            engine: "kokoro",
            id: id.to_owned(),
            lang: "en",
            installed: inventory.kokoro_model,
            selected: chosen(EngineKind::Kokoro, id),
        });
    }

    let cloud_ready =
        config.cloud_enabled && inventory.cloud_key && !config.eleven_voice_id.is_empty();
    let cloud_detail = if !config.cloud_enabled {
        "off (tts.cloud_enabled is false)"
    } else if !inventory.cloud_key {
        "no API key stored"
    } else if config.eleven_voice_id.is_empty() {
        "no voice id set"
    } else {
        "ready"
    };
    let engines = vec![
        EngineStatus {
            engine: "piper",
            available: inventory.piper_exe && !inventory.piper_voices.is_empty(),
            detail: if !inventory.piper_exe {
                "piper.exe not found (run scripts/fetch-models.ps1 -Piper)".into()
            } else if inventory.piper_voices.is_empty() {
                "no voices installed".into()
            } else {
                format!("{} voice(s) installed", inventory.piper_voices.len())
            },
        },
        EngineStatus {
            engine: "kokoro",
            available: inventory.kokoro_model && inventory.kokoro_worker,
            detail: if !inventory.kokoro_worker {
                "workers/tts_kokoro not found".into()
            } else if !inventory.kokoro_model {
                "model files missing (run scripts/fetch-models.ps1 -Kokoro)".into()
            } else {
                "ready (starts on demand)".into()
            },
        },
        EngineStatus {
            engine: "elevenlabs",
            available: cloud_ready,
            detail: cloud_detail.into(),
        },
    ];
    VoicesReport { engines, voices }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> Inventory {
        Inventory {
            piper_exe: true,
            piper_voices: vec!["ru_RU-dmitri-medium".into(), "ru_RU-ruslan-medium".into()],
            kokoro_model: true,
            kokoro_worker: true,
            cloud_key: false,
        }
    }

    #[test]
    fn lists_installed_voices_and_marks_the_selected_ones() {
        let report = report(&TtsConfig::default(), &inventory());
        let selected: Vec<_> = report
            .voices
            .iter()
            .filter(|voice| voice.selected)
            .map(|voice| (voice.engine, voice.id.as_str()))
            .collect();
        assert_eq!(
            selected,
            [("piper", "ru_RU-dmitri-medium"), ("kokoro", "bm_george")]
        );
        let ruslan = report
            .voices
            .iter()
            .find(|voice| voice.id == "ru_RU-ruslan-medium")
            .expect("ruslan");
        assert!(ruslan.installed && !ruslan.selected && ruslan.lang == "ru");
        let alan = report
            .voices
            .iter()
            .find(|voice| voice.id == DEFAULT_PIPER_VOICE_EN)
            .expect("english piper voice is offered");
        assert!(!alan.installed);
    }

    #[test]
    fn engine_status_explains_what_is_missing() {
        let mut inventory = inventory();
        let ok = report(&TtsConfig::default(), &inventory);
        assert!(ok.engines[0].available && ok.engines[1].available);
        assert!(!ok.engines[2].available);
        assert!(ok.engines[2].detail.contains("off"));
        inventory.piper_exe = false;
        inventory.kokoro_model = false;
        let broken = report(&TtsConfig::default(), &inventory);
        assert!(!broken.engines[0].available && broken.engines[0].detail.contains("piper.exe"));
        assert!(!broken.engines[1].available && broken.engines[1].detail.contains("model"));
    }

    #[test]
    fn the_cloud_needs_the_switch_a_key_and_a_voice() {
        let mut config = TtsConfig {
            cloud_enabled: true,
            ..TtsConfig::default()
        };
        let mut inventory = inventory();
        assert_eq!(
            report(&config, &inventory).engines[2].detail,
            "no API key stored"
        );
        inventory.cloud_key = true;
        assert_eq!(
            report(&config, &inventory).engines[2].detail,
            "no voice id set"
        );
        config.eleven_voice_id = "abc".into();
        let ready = report(&config, &inventory);
        assert!(ready.engines[2].available);
        assert_eq!(ready.engines[2].detail, "ready");
    }
}
