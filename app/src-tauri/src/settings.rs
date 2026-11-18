use crate::error::CommandError;
use serde_json::Value;
use tauri_plugin_global_shortcut::Shortcut;

pub const AUTOSTART: &str = "general.autostart";
pub const LANGUAGE: &str = "general.language";
pub const START_MODE: &str = "general.start_mode";
pub const LAST_MODE: &str = "ui.last_mode";
pub const VISUAL_QUALITY: &str = "visual.quality";
pub const VISUAL_FPS_LIMIT: &str = "visual.fps_limit";
pub const VISUAL_REDUCED_MOTION: &str = "visual.reduced_motion";
pub const VOICE_ENABLED: &str = "voice.enabled";
pub const VOICE_INPUT_DEVICE: &str = "voice.input_device";
pub const VOICE_SENSITIVITY: &str = "voice.sensitivity";
pub const VOICE_WAKE_NAMES: &str = "voice.wake_names";
pub const VOICE_PTT_KEY: &str = "voice.ptt_key";
pub const TTS_ENGINE_RU: &str = "tts.engine_ru";
pub const TTS_VOICE_RU: &str = "tts.voice_ru";
pub const TTS_ENGINE_EN: &str = "tts.engine_en";
pub const TTS_VOICE_EN: &str = "tts.voice_en";
pub const TTS_SPEED: &str = "tts.speed";
pub const TTS_VOLUME: &str = "tts.volume";
pub const TTS_OUTPUT_DEVICE: &str = "tts.output_device";
pub const TTS_CLOUD_ENABLED: &str = "tts.cloud_enabled";
pub const TTS_ELEVEN_VOICE_ID: &str = "tts.eleven_voice_id";
pub const CONTROL_DISABLED_ACTIONS: &str = "control.disabled_actions";
pub const CONTROL_FILE_ROOTS: &str = "control.file_roots";
pub const CONTROL_APP_ALIASES: &str = "control.app_aliases";
pub const CONTROL_EMERGENCY_KEY: &str = "control.emergency_key";
pub const AI_ENABLED: &str = "ai.enabled";
pub const AI_MODEL: &str = "ai.model";
pub const AI_KEEP_ALIVE: &str = "ai.keep_alive_minutes";
pub const AI_NUM_CTX: &str = "ai.num_ctx";
pub const AI_NUM_THREAD: &str = "ai.num_thread";
pub const AI_CLOUD_ENABLED: &str = "ai.cloud_enabled";
pub const AI_CLOUD_BASE_URL: &str = "ai.cloud_base_url";
pub const AI_CLOUD_MODEL: &str = "ai.cloud_model";
pub const PERSONALITY_MODE: &str = "personality.mode";
pub const PERSONALITY_CUSTOM: &str = "personality.custom";
pub const PRIVACY_HISTORY_ENABLED: &str = "privacy.history_enabled";
pub const PRIVACY_HISTORY_DAYS: &str = "privacy.history_days";
pub const VISION_ENABLED: &str = "vision.enabled";
pub const VISION_ALLOW_CLOUD: &str = "vision.allow_cloud";
pub const VISION_VLM_MODEL: &str = "vision.vlm_model";
pub const SYSINTEL_ALERTS: &str = "sysintel.alerts";
pub const SYSINTEL_RTT_HOST: &str = "sysintel.rtt_host";
pub const SETUP_COMPLETED: &str = "setup.completed";
pub const DEFAULT_PTT_KEY: &str = "F8";

/// Keys accepted by `settings_get` / `settings_set` (API_CONTRACTS, "Whitelist настроек").
pub const WHITELIST: [&str; 43] = [
    AUTOSTART,
    LANGUAGE,
    START_MODE,
    LAST_MODE,
    VISUAL_QUALITY,
    VISUAL_FPS_LIMIT,
    VISUAL_REDUCED_MOTION,
    VOICE_ENABLED,
    VOICE_INPUT_DEVICE,
    VOICE_SENSITIVITY,
    VOICE_WAKE_NAMES,
    VOICE_PTT_KEY,
    TTS_ENGINE_RU,
    TTS_VOICE_RU,
    TTS_ENGINE_EN,
    TTS_VOICE_EN,
    TTS_SPEED,
    TTS_VOLUME,
    TTS_OUTPUT_DEVICE,
    TTS_CLOUD_ENABLED,
    TTS_ELEVEN_VOICE_ID,
    CONTROL_DISABLED_ACTIONS,
    CONTROL_FILE_ROOTS,
    CONTROL_APP_ALIASES,
    CONTROL_EMERGENCY_KEY,
    AI_ENABLED,
    AI_MODEL,
    AI_KEEP_ALIVE,
    AI_NUM_CTX,
    AI_NUM_THREAD,
    AI_CLOUD_ENABLED,
    AI_CLOUD_BASE_URL,
    AI_CLOUD_MODEL,
    PERSONALITY_MODE,
    PERSONALITY_CUSTOM,
    PRIVACY_HISTORY_ENABLED,
    PRIVACY_HISTORY_DAYS,
    VISION_ENABLED,
    VISION_ALLOW_CLOUD,
    VISION_VLM_MODEL,
    SYSINTEL_ALERTS,
    SYSINTEL_RTT_HOST,
    SETUP_COMPLETED,
];

/// An integer setting inside `min..=max`.
fn int_range(key: &str, value: &Value, min: u64, max: u64) -> Result<(), CommandError> {
    match value.as_u64() {
        Some(number) if (min..=max).contains(&number) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected a whole number from {min} to {max}"
        ))),
    }
}

fn brain_error(key: &str, error: jarvis_brain::BrainError) -> CommandError {
    CommandError::Validation(format!("{key}: {error}"))
}

fn text_with<F>(key: &str, value: &Value, check: F) -> Result<(), CommandError>
where
    F: Fn(&str) -> Result<(), jarvis_brain::BrainError>,
{
    let text = value
        .as_str()
        .ok_or_else(|| CommandError::Validation(format!("{key}: expected text")))?;
    check(text).map_err(|error| brain_error(key, error))
}

fn one_of(key: &str, value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    match value.as_str() {
        Some(text) if allowed.contains(&text) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected one of {}",
            allowed.join(", ")
        ))),
    }
}

/// Frame-rate caps offered by `visual.fps_limit`.
const FPS_LIMITS: [u64; 3] = [30, 45, 60];

fn fps_limit(key: &str, value: &Value) -> Result<(), CommandError> {
    match value.as_u64() {
        Some(limit) if FPS_LIMITS.contains(&limit) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected one of 30, 45, 60"
        ))),
    }
}

fn boolean(key: &str, value: &Value) -> Result<(), CommandError> {
    if value.is_boolean() {
        Ok(())
    } else {
        Err(CommandError::Validation(format!("{key}: expected boolean")))
    }
}

fn input_device(key: &str, value: &Value) -> Result<(), CommandError> {
    match value {
        Value::Null => Ok(()),
        Value::String(name) if !name.is_empty() && name.chars().count() <= 200 => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected null or a device name"
        ))),
    }
}

fn sensitivity(key: &str, value: &Value) -> Result<(), CommandError> {
    match value.as_f64() {
        Some(number) if (0.0..=1.0).contains(&number) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected a number from 0.0 to 1.0"
        ))),
    }
}

fn tts_speed(key: &str, value: &Value) -> Result<(), CommandError> {
    match value.as_f64() {
        Some(number) if (0.7..=1.4).contains(&number) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected a number from 0.7 to 1.4"
        ))),
    }
}

fn voice_id(key: &str, value: &Value) -> Result<(), CommandError> {
    match value.as_str() {
        Some(id) if jarvis_tts::is_valid_voice_id(id) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected a voice id (letters, digits, _ and -, up to 64 characters)"
        ))),
    }
}

fn eleven_voice_id(key: &str, value: &Value) -> Result<(), CommandError> {
    match value.as_str() {
        Some(id) if jarvis_tts::is_valid_eleven_voice_id(id) => Ok(()),
        _ => Err(CommandError::Validation(format!(
            "{key}: expected an empty string or a voice id (letters, digits, _ and -)"
        ))),
    }
}

/// The list of wake names as strings, if `value` is one.
pub fn names_from(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn wake_names(key: &str, value: &Value) -> Result<(), CommandError> {
    let names = names_from(value)
        .ok_or_else(|| CommandError::Validation(format!("{key}: expected a list of names")))?;
    jarvis_voice::validate_wake_names(&names)
        .map_err(|message| CommandError::Validation(format!("{key}: {message}")))
}

/// The push-to-talk key must be something the global shortcut plugin can register (for example `F8`).
pub fn parse_ptt_key(key: &str, value: &Value) -> Result<Shortcut, CommandError> {
    value
        .as_str()
        .and_then(|text| text.parse::<Shortcut>().ok())
        .ok_or_else(|| CommandError::Validation(format!("{key}: not a valid key, for example F8")))
}

/// Rejects unknown keys and values of the wrong type or outside the allowed set.
pub fn validate(key: &str, value: &Value) -> Result<(), CommandError> {
    match key {
        AUTOSTART | VISUAL_REDUCED_MOTION | VOICE_ENABLED | SETUP_COMPLETED => boolean(key, value),
        VOICE_INPUT_DEVICE => input_device(key, value),
        VOICE_SENSITIVITY => sensitivity(key, value),
        VOICE_WAKE_NAMES => wake_names(key, value),
        VOICE_PTT_KEY => parse_ptt_key(key, value).map(|_| ()),
        TTS_ENGINE_RU => one_of(key, value, &["piper"]),
        TTS_ENGINE_EN => one_of(key, value, &["kokoro", "piper"]),
        TTS_VOICE_RU | TTS_VOICE_EN => voice_id(key, value),
        TTS_SPEED => tts_speed(key, value),
        TTS_VOLUME => sensitivity(key, value),
        TTS_OUTPUT_DEVICE => input_device(key, value),
        TTS_CLOUD_ENABLED => boolean(key, value),
        TTS_ELEVEN_VOICE_ID => eleven_voice_id(key, value),
        CONTROL_DISABLED_ACTIONS
        | CONTROL_FILE_ROOTS
        | CONTROL_APP_ALIASES
        | CONTROL_EMERGENCY_KEY => crate::control::validate(key, value),
        AI_ENABLED | AI_CLOUD_ENABLED | PRIVACY_HISTORY_ENABLED => boolean(key, value),
        VISION_ENABLED | VISION_ALLOW_CLOUD => boolean(key, value),
        VISION_VLM_MODEL => text_with(key, value, jarvis_brain::config::validate_model_name),
        SYSINTEL_ALERTS | SYSINTEL_RTT_HOST => crate::sysconf::validate(key, value),
        AI_MODEL => text_with(key, value, jarvis_brain::config::validate_model_name),
        AI_KEEP_ALIVE => int_range(
            key,
            value,
            0,
            u64::from(jarvis_brain::config::MAX_KEEP_ALIVE_MINUTES),
        ),
        AI_NUM_CTX => int_range(
            key,
            value,
            u64::from(jarvis_brain::config::MIN_NUM_CTX),
            u64::from(jarvis_brain::config::MAX_NUM_CTX),
        ),
        AI_NUM_THREAD => int_range(
            key,
            value,
            1,
            u64::from(jarvis_brain::config::MAX_NUM_THREAD),
        ),
        AI_CLOUD_BASE_URL => text_with(key, value, jarvis_brain::config::validate_cloud_url),
        AI_CLOUD_MODEL => text_with(key, value, |text| {
            if text.is_empty() {
                Ok(())
            } else {
                jarvis_brain::config::validate_model_name(text)
            }
        }),
        PERSONALITY_MODE => one_of(key, value, &["formal", "classic", "friendly", "custom"]),
        PERSONALITY_CUSTOM => text_with(key, value, jarvis_brain::config::validate_custom),
        PRIVACY_HISTORY_DAYS => int_range(
            key,
            value,
            1,
            u64::from(jarvis_brain::config::MAX_HISTORY_DAYS),
        ),
        LANGUAGE => one_of(key, value, &["ru", "en"]),
        START_MODE => one_of(key, value, &["tray", "compact", "full"]),
        LAST_MODE => one_of(key, value, &["compact", "full"]),
        VISUAL_QUALITY => one_of(key, value, &["auto", "low", "medium", "high"]),
        VISUAL_FPS_LIMIT => fps_limit(key, value),
        _ => Err(CommandError::Validation(format!(
            "{key}: not a known setting"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_valid_values() {
        for (key, value) in [
            (AUTOSTART, json!(true)),
            (AUTOSTART, json!(false)),
            (LANGUAGE, json!("ru")),
            (LANGUAGE, json!("en")),
            (START_MODE, json!("tray")),
            (START_MODE, json!("compact")),
            (START_MODE, json!("full")),
            (LAST_MODE, json!("compact")),
            (LAST_MODE, json!("full")),
            (VISUAL_QUALITY, json!("auto")),
            (VISUAL_QUALITY, json!("low")),
            (VISUAL_QUALITY, json!("medium")),
            (VISUAL_QUALITY, json!("high")),
            (VISUAL_FPS_LIMIT, json!(30)),
            (VISUAL_FPS_LIMIT, json!(45)),
            (VISUAL_FPS_LIMIT, json!(60)),
            (VISUAL_REDUCED_MOTION, json!(true)),
            (VISUAL_REDUCED_MOTION, json!(false)),
            (VOICE_ENABLED, json!(true)),
            (VOICE_INPUT_DEVICE, json!(null)),
            (VOICE_INPUT_DEVICE, json!("Microphone (Realtek)")),
            (VOICE_SENSITIVITY, json!(0.0)),
            (VOICE_SENSITIVITY, json!(0.5)),
            (VOICE_SENSITIVITY, json!(1)),
            (VOICE_WAKE_NAMES, json!(["лёха", "лёх", "алексей"])),
            (VOICE_WAKE_NAMES, json!(["Jarvis"])),
            (VOICE_PTT_KEY, json!("F8")),
            (VOICE_PTT_KEY, json!("Ctrl+Alt+K")),
            (TTS_ENGINE_RU, json!("piper")),
            (TTS_VOICE_RU, json!("ru_RU-dmitri-medium")),
            (TTS_ENGINE_EN, json!("kokoro")),
            (TTS_ENGINE_EN, json!("piper")),
            (TTS_VOICE_EN, json!("bm_george")),
            (TTS_VOICE_EN, json!("bm_lewis")),
            (TTS_SPEED, json!(0.7)),
            (TTS_SPEED, json!(1.0)),
            (TTS_SPEED, json!(1.4)),
            (TTS_VOLUME, json!(0)),
            (TTS_VOLUME, json!(0.9)),
            (TTS_VOLUME, json!(1)),
            (TTS_OUTPUT_DEVICE, json!(null)),
            (TTS_OUTPUT_DEVICE, json!("Speakers (Realtek)")),
            (TTS_CLOUD_ENABLED, json!(false)),
            (TTS_CLOUD_ENABLED, json!(true)),
            (TTS_ELEVEN_VOICE_ID, json!("")),
            (TTS_ELEVEN_VOICE_ID, json!("21m00Tcm4TlvDq8ikWAM")),
        ] {
            assert_eq!(validate(key, &value), Ok(()), "{key}={value}");
        }
    }

    #[test]
    fn rejects_unknown_keys() {
        let error = validate("general.nope", &json!(true)).expect_err("unknown key");
        assert_eq!(error.code(), "VALIDATION");
        assert!(validate("", &json!(null)).is_err());
    }

    #[test]
    fn rejects_wrong_types_and_values() {
        for (key, value) in [
            (AUTOSTART, json!("true")),
            (AUTOSTART, json!(1)),
            (AUTOSTART, json!(null)),
            (LANGUAGE, json!("de")),
            (LANGUAGE, json!(true)),
            (START_MODE, json!("hidden")),
            (LAST_MODE, json!("tray")),
            (LAST_MODE, json!(["full"])),
            (VISUAL_QUALITY, json!("ultra")),
            (VISUAL_QUALITY, json!(1)),
            (VISUAL_FPS_LIMIT, json!(24)),
            (VISUAL_FPS_LIMIT, json!(30.5)),
            (VISUAL_FPS_LIMIT, json!("30")),
            (VISUAL_FPS_LIMIT, json!(-30)),
            (VISUAL_REDUCED_MOTION, json!("false")),
            (VISUAL_REDUCED_MOTION, json!(0)),
            (VOICE_ENABLED, json!("yes")),
            (VOICE_INPUT_DEVICE, json!("")),
            (VOICE_INPUT_DEVICE, json!(5)),
            (VOICE_SENSITIVITY, json!(1.5)),
            (VOICE_SENSITIVITY, json!(-0.1)),
            (VOICE_SENSITIVITY, json!("0.5")),
            (VOICE_WAKE_NAMES, json!([])),
            (VOICE_WAKE_NAMES, json!(["a", "b", "c", "d", "e", "f"])),
            (VOICE_WAKE_NAMES, json!(["очень длинное имя"])),
            (VOICE_WAKE_NAMES, json!(["x\"y"])),
            (VOICE_WAKE_NAMES, json!([1])),
            (VOICE_WAKE_NAMES, json!("лёха")),
            (VOICE_PTT_KEY, json!("NotAKey")),
            (VOICE_PTT_KEY, json!("")),
            (VOICE_PTT_KEY, json!(8)),
            (TTS_ENGINE_RU, json!("kokoro")),
            (TTS_ENGINE_RU, json!("")),
            (TTS_ENGINE_EN, json!("elevenlabs")),
            (TTS_ENGINE_EN, json!(1)),
            (TTS_VOICE_RU, json!("")),
            (TTS_VOICE_RU, json!("../../evil")),
            (TTS_VOICE_RU, json!("a b")),
            (TTS_VOICE_RU, json!(null)),
            (TTS_VOICE_EN, json!("x".repeat(65))),
            (TTS_SPEED, json!(0.69)),
            (TTS_SPEED, json!(1.41)),
            (TTS_SPEED, json!("1.0")),
            (TTS_SPEED, json!(null)),
            (TTS_VOLUME, json!(1.01)),
            (TTS_VOLUME, json!(-0.01)),
            (TTS_VOLUME, json!("0.5")),
            (TTS_OUTPUT_DEVICE, json!("")),
            (TTS_OUTPUT_DEVICE, json!(3)),
            (TTS_CLOUD_ENABLED, json!("true")),
            (TTS_CLOUD_ENABLED, json!(1)),
            (TTS_ELEVEN_VOICE_ID, json!("a/b")),
            (TTS_ELEVEN_VOICE_ID, json!(null)),
            (TTS_ELEVEN_VOICE_ID, json!(12)),
        ] {
            let error = validate(key, &value).expect_err("invalid");
            assert_eq!(error.code(), "VALIDATION", "{key}={value}");
        }
    }

    #[test]
    fn voice_keys_are_whitelisted() {
        for key in [
            VOICE_ENABLED,
            VOICE_INPUT_DEVICE,
            VOICE_SENSITIVITY,
            VOICE_WAKE_NAMES,
            VOICE_PTT_KEY,
        ] {
            assert!(WHITELIST.contains(&key), "{key}");
        }
    }

    #[test]
    fn tts_keys_are_whitelisted_and_secrets_are_not_settings() {
        for key in [
            TTS_ENGINE_RU,
            TTS_VOICE_RU,
            TTS_ENGINE_EN,
            TTS_VOICE_EN,
            TTS_SPEED,
            TTS_VOLUME,
            TTS_OUTPUT_DEVICE,
            TTS_CLOUD_ENABLED,
            TTS_ELEVEN_VOICE_ID,
        ] {
            assert!(WHITELIST.contains(&key), "{key}");
        }
        for key in ["tts.api_key", "tts.eleven_api_key", "elevenlabs_api_key"] {
            assert!(!WHITELIST.contains(&key), "{key}");
            assert!(validate(key, &json!("secret")).is_err(), "{key}");
        }
    }

    #[test]
    fn control_keys_are_whitelisted_and_validated() {
        for key in [
            CONTROL_DISABLED_ACTIONS,
            CONTROL_FILE_ROOTS,
            CONTROL_APP_ALIASES,
            CONTROL_EMERGENCY_KEY,
        ] {
            assert!(WHITELIST.contains(&key), "{key}");
        }
        assert!(validate(CONTROL_DISABLED_ACTIONS, &json!(["power.shutdown"])).is_ok());
        assert!(validate(CONTROL_DISABLED_ACTIONS, &json!(["bogus"])).is_err());
        assert!(validate(CONTROL_FILE_ROOTS, &json!([r"\\server\share"])).is_err());
        assert!(validate(CONTROL_APP_ALIASES, &json!({"x": "steam://rungameid/1"})).is_ok());
        assert!(validate(CONTROL_EMERGENCY_KEY, &json!("Ctrl+Alt+Pause")).is_ok());
    }

    #[test]
    fn ai_personality_and_privacy_settings_are_validated() {
        for (key, value) in [
            (AI_ENABLED, json!(true)),
            (AI_MODEL, json!("qwen3:4b")),
            (AI_KEEP_ALIVE, json!(0)),
            (AI_KEEP_ALIVE, json!(5)),
            (AI_NUM_CTX, json!(4096)),
            (AI_NUM_CTX, json!(2048)),
            (AI_NUM_THREAD, json!(6)),
            (AI_CLOUD_ENABLED, json!(false)),
            (AI_CLOUD_BASE_URL, json!("")),
            (AI_CLOUD_BASE_URL, json!("https://api.example.com/v1")),
            (AI_CLOUD_MODEL, json!("")),
            (AI_CLOUD_MODEL, json!("gpt-4o-mini")),
            (PERSONALITY_MODE, json!("friendly")),
            (PERSONALITY_CUSTOM, json!("")),
            (PERSONALITY_CUSTOM, json!("я".repeat(2000))),
            (PRIVACY_HISTORY_ENABLED, json!(false)),
            (PRIVACY_HISTORY_DAYS, json!(30)),
        ] {
            assert_eq!(validate(key, &value), Ok(()), "{key}={value}");
        }
        for (key, value) in [
            (AI_ENABLED, json!("yes")),
            (AI_MODEL, json!("")),
            (AI_MODEL, json!("a b;c")),
            (AI_KEEP_ALIVE, json!(-1)),
            (AI_KEEP_ALIVE, json!(1441)),
            (AI_NUM_CTX, json!(8192)),
            (AI_NUM_CTX, json!(100)),
            (AI_NUM_THREAD, json!(7)),
            (AI_NUM_THREAD, json!(0)),
            (AI_CLOUD_BASE_URL, json!("http://api.example.com")),
            (AI_CLOUD_BASE_URL, json!("https://u:p@api.example.com")),
            (AI_CLOUD_MODEL, json!("a b")),
            (PERSONALITY_MODE, json!("rude")),
            (PERSONALITY_CUSTOM, json!("я".repeat(2001))),
            (PERSONALITY_CUSTOM, json!(5)),
            (PRIVACY_HISTORY_ENABLED, json!(1)),
            (PRIVACY_HISTORY_DAYS, json!(0)),
            (PRIVACY_HISTORY_DAYS, json!(366)),
        ] {
            let error = validate(key, &value).expect_err("invalid");
            assert_eq!(error.code(), "VALIDATION", "{key}={value}");
        }
        for key in ["ai.api_key", "ai.cloud_key", "ai.cloud_api_key"] {
            assert!(!WHITELIST.contains(&key), "{key}");
            assert!(validate(key, &json!("secret")).is_err(), "{key}");
        }
    }

    #[test]
    fn vision_and_sysintel_settings_are_validated() {
        for (key, value) in [
            (VISION_ENABLED, json!(true)),
            (VISION_ENABLED, json!(false)),
            (VISION_ALLOW_CLOUD, json!(false)),
            (VISION_VLM_MODEL, json!("qwen2.5vl:3b")),
            (SYSINTEL_RTT_HOST, json!("1.1.1.1:443")),
            (SYSINTEL_ALERTS, json!({"gpu_temp": {"threshold": 80.0}})),
        ] {
            assert!(validate(key, &value).is_ok(), "{key} {value}");
            assert!(WHITELIST.contains(&key), "{key}");
        }
        for (key, value) in [
            (VISION_ENABLED, json!("yes")),
            (VISION_ALLOW_CLOUD, json!(1)),
            (VISION_VLM_MODEL, json!("")),
            (VISION_VLM_MODEL, json!("bad model!")),
            (VISION_VLM_MODEL, json!(5)),
            (SYSINTEL_RTT_HOST, json!("no-port")),
            (SYSINTEL_ALERTS, json!({"cpu_load": {"threshold": 5.0}})),
        ] {
            assert_eq!(
                validate(key, &value).expect_err("invalid").code(),
                "VALIDATION",
                "{key} {value}"
            );
        }
    }

    #[test]
    fn visual_keys_are_whitelisted() {
        for key in [VISUAL_QUALITY, VISUAL_FPS_LIMIT, VISUAL_REDUCED_MOTION] {
            assert!(WHITELIST.contains(&key), "{key}");
        }
    }

    #[test]
    fn whitelist_keys_all_validate_somehow() {
        for key in WHITELIST {
            // `voice.input_device` is the one setting where null is valid (the system default device).
            if key == VOICE_INPUT_DEVICE || key == TTS_OUTPUT_DEVICE {
                assert!(validate(key, &json!(null)).is_ok());
            } else {
                assert!(validate(key, &json!(null)).is_err());
            }
        }
    }
}
