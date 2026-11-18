import { describe, expect, it } from "vitest";
import {
  GENERAL_DEFAULTS,
  resolveGeneral,
  resolveSettings,
  SETTING_DEFAULTS,
  parseWakeNames,
  validateSetting,
  TTS_DEFAULTS,
  TTS_KEYS,
  validWakeNames,
  VISUAL_DEFAULTS,
  VOICE_DEFAULTS,
} from "./settings";

describe("settings validation", () => {
  it("accepts valid values", () => {
    expect(validateSetting("general.autostart", true)).toEqual({ ok: true, value: true });
    expect(validateSetting("general.language", "en")).toEqual({ ok: true, value: "en" });
    expect(validateSetting("general.start_mode", "tray")).toEqual({ ok: true, value: "tray" });
  });

  it("rejects wrong types and values", () => {
    expect(validateSetting("general.autostart", "yes").ok).toBe(false);
    expect(validateSetting("general.language", "de").ok).toBe(false);
    expect(validateSetting("general.start_mode", "hidden").ok).toBe(false);
  });

  it("falls back to defaults for missing or invalid stored values", () => {
    expect(resolveGeneral({})).toEqual(GENERAL_DEFAULTS);
    expect(
      resolveGeneral({
        "general.autostart": 1,
        "general.language": "en",
        "general.start_mode": "nope",
      }),
    ).toEqual({ ...GENERAL_DEFAULTS, "general.language": "en" });
  });
});

describe("visual settings", () => {
  it("has the contract defaults", () => {
    expect(VISUAL_DEFAULTS).toEqual({
      "visual.quality": "auto",
      "visual.fps_limit": 30,
      "visual.reduced_motion": false,
    });
  });

  it("accepts the whitelisted values", () => {
    for (const quality of ["auto", "low", "medium", "high"]) {
      expect(validateSetting("visual.quality", quality)).toEqual({ ok: true, value: quality });
    }
    for (const limit of [30, 45, 60]) {
      expect(validateSetting("visual.fps_limit", limit)).toEqual({ ok: true, value: limit });
    }
    expect(validateSetting("visual.reduced_motion", true)).toEqual({ ok: true, value: true });
  });

  it("rejects everything else", () => {
    expect(validateSetting("visual.quality", "ultra").ok).toBe(false);
    expect(validateSetting("visual.quality", 1).ok).toBe(false);
    expect(validateSetting("visual.fps_limit", 24).ok).toBe(false);
    expect(validateSetting("visual.fps_limit", "30").ok).toBe(false);
    expect(validateSetting("visual.fps_limit", 30.5).ok).toBe(false);
    expect(validateSetting("visual.reduced_motion", "false").ok).toBe(false);
    expect(validateSetting("visual.reduced_motion", 0).ok).toBe(false);
  });

  it("resolves stored values with fallbacks", () => {
    expect(resolveSettings({})).toEqual(SETTING_DEFAULTS);
    expect(
      resolveSettings({
        "visual.quality": "high",
        "visual.fps_limit": 99,
        "visual.reduced_motion": true,
        "general.language": "en",
      }),
    ).toEqual({
      ...SETTING_DEFAULTS,
      "visual.quality": "high",
      "visual.reduced_motion": true,
      "general.language": "en",
    });
  });
});

describe("voice settings", () => {
  it("has the contract defaults, with listening off", () => {
    expect(VOICE_DEFAULTS).toEqual({
      "voice.enabled": false,
      "voice.input_device": null,
      "voice.sensitivity": 0.5,
      "voice.wake_names": ["лёха", "лёх", "алексей"],
      "voice.ptt_key": "F8",
    });
  });

  it("validates the voice values", () => {
    expect(validateSetting("voice.enabled", true).ok).toBe(true);
    expect(validateSetting("voice.enabled", "on").ok).toBe(false);
    expect(validateSetting("voice.input_device", null).ok).toBe(true);
    expect(validateSetting("voice.input_device", "Microphone").ok).toBe(true);
    expect(validateSetting("voice.input_device", "").ok).toBe(false);
    expect(validateSetting("voice.sensitivity", 0).ok).toBe(true);
    expect(validateSetting("voice.sensitivity", 1).ok).toBe(true);
    expect(validateSetting("voice.sensitivity", 1.2).ok).toBe(false);
    expect(validateSetting("voice.sensitivity", "0.5").ok).toBe(false);
    expect(validateSetting("voice.ptt_key", "F8").ok).toBe(true);
    expect(validateSetting("voice.ptt_key", " ").ok).toBe(false);
  });

  it("accepts 1 to 5 letter-only names of up to 20 characters", () => {
    expect(validWakeNames(["лёха"])).toBe(true);
    expect(validWakeNames(["Jarvis", "Алексей"])).toBe(true);
    expect(validWakeNames([])).toBe(false);
    expect(validWakeNames(["а", "б", "в", "г", "д", "е"])).toBe(false);
    expect(validWakeNames(["ааааааааааааааааааааа"])).toBe(false);
    expect(validWakeNames(["a b"])).toBe(false);
    expect(validWakeNames(["a\"b"])).toBe(false);
    expect(validWakeNames("лёха")).toBe(false);
    expect(validateSetting("voice.wake_names", ["x1"]).ok).toBe(false);
  });

  it("splits the names field on commas and spaces", () => {
    expect(parseWakeNames("лёха, лёх  алексей;Jarvis")).toEqual(["лёха", "лёх", "алексей", "Jarvis"]);
    expect(parseWakeNames("  ,, ")).toEqual([]);
  });

  it("falls back to defaults for missing voice values", () => {
    expect(resolveSettings({ "voice.sensitivity": 5, "voice.wake_names": null })).toEqual(SETTING_DEFAULTS);
    expect(resolveSettings({ "voice.enabled": true, "voice.sensitivity": 0.8 })).toEqual({
      ...SETTING_DEFAULTS,
      "voice.enabled": true,
      "voice.sensitivity": 0.8,
    });
  });
});

describe("speech output settings", () => {
  it("has the contract defaults, with the cloud off", () => {
    expect(TTS_DEFAULTS).toEqual({
      "tts.engine_ru": "piper",
      "tts.voice_ru": "ru_RU-dmitri-medium",
      "tts.engine_en": "kokoro",
      "tts.voice_en": "bm_george",
      "tts.speed": 1.0,
      "tts.volume": 0.9,
      "tts.output_device": null,
      "tts.cloud_enabled": false,
      "tts.eleven_voice_id": "",
    });
    expect(TTS_KEYS).toHaveLength(9);
  });

  it("accepts valid values", () => {
    expect(validateSetting("tts.engine_ru", "piper").ok).toBe(true);
    expect(validateSetting("tts.engine_en", "kokoro").ok).toBe(true);
    expect(validateSetting("tts.engine_en", "piper").ok).toBe(true);
    expect(validateSetting("tts.voice_en", "bm_lewis").ok).toBe(true);
    expect(validateSetting("tts.speed", 0.7).ok).toBe(true);
    expect(validateSetting("tts.speed", 1.4).ok).toBe(true);
    expect(validateSetting("tts.volume", 0).ok).toBe(true);
    expect(validateSetting("tts.volume", 1).ok).toBe(true);
    expect(validateSetting("tts.output_device", null).ok).toBe(true);
    expect(validateSetting("tts.output_device", "Speakers").ok).toBe(true);
    expect(validateSetting("tts.cloud_enabled", true).ok).toBe(true);
    expect(validateSetting("tts.eleven_voice_id", "").ok).toBe(true);
    expect(validateSetting("tts.eleven_voice_id", "21m00Tcm4TlvDq8ikWAM").ok).toBe(true);
  });

  it("rejects everything else", () => {
    expect(validateSetting("tts.engine_ru", "kokoro").ok).toBe(false);
    expect(validateSetting("tts.engine_en", "elevenlabs").ok).toBe(false);
    expect(validateSetting("tts.voice_ru", "").ok).toBe(false);
    expect(validateSetting("tts.voice_ru", "../evil").ok).toBe(false);
    expect(validateSetting("tts.voice_en", "a b").ok).toBe(false);
    expect(validateSetting("tts.speed", 0.69).ok).toBe(false);
    expect(validateSetting("tts.speed", 1.41).ok).toBe(false);
    expect(validateSetting("tts.speed", "1").ok).toBe(false);
    expect(validateSetting("tts.volume", 1.1).ok).toBe(false);
    expect(validateSetting("tts.volume", -0.1).ok).toBe(false);
    expect(validateSetting("tts.output_device", "").ok).toBe(false);
    expect(validateSetting("tts.cloud_enabled", "true").ok).toBe(false);
    expect(validateSetting("tts.eleven_voice_id", "a/b").ok).toBe(false);
    expect(validateSetting("tts.eleven_voice_id", null).ok).toBe(false);
  });

  it("falls back to defaults for invalid stored values", () => {
    expect(resolveSettings({ "tts.speed": 9, "tts.cloud_enabled": "yes", "tts.voice_ru": "../x" })).toEqual(
      SETTING_DEFAULTS,
    );
    expect(resolveSettings({ "tts.volume": 0.3, "tts.engine_en": "piper" })).toEqual({
      ...SETTING_DEFAULTS,
      "tts.volume": 0.3,
      "tts.engine_en": "piper",
    });
  });
});
