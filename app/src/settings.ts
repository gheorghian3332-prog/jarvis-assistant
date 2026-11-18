import { FPS_LIMITS, type FpsLimit } from "./core/fps";
import { QUALITY_SETTINGS, type QualitySetting } from "./core/quality";

export type Language = "ru" | "en";
export type StartMode = "tray" | "compact" | "full";

export interface GeneralSettings {
  "general.autostart": boolean;
  "general.language": Language;
  "general.start_mode": StartMode;
}

export interface VisualSettings {
  "visual.quality": QualitySetting;
  "visual.fps_limit": FpsLimit;
  "visual.reduced_motion": boolean;
}

export interface VoiceSettings {
  "voice.enabled": boolean;
  "voice.input_device": string | null;
  "voice.sensitivity": number;
  "voice.wake_names": string[];
  "voice.ptt_key": string;
}

export type TtsEngineRu = "piper";
export type TtsEngineEn = "kokoro" | "piper";

export interface TtsSettings {
  "tts.engine_ru": TtsEngineRu;
  "tts.voice_ru": string;
  "tts.engine_en": TtsEngineEn;
  "tts.voice_en": string;
  "tts.speed": number;
  "tts.volume": number;
  "tts.output_device": string | null;
  "tts.cloud_enabled": boolean;
  "tts.eleven_voice_id": string;
}

export interface ControlSettings {
  "control.disabled_actions": string[];
  "control.file_roots": string[];
  "control.app_aliases": Record<string, string>;
  "control.emergency_key": string;
}

export type PersonalityMode = "formal" | "classic" | "friendly" | "custom";

export interface AiSettings {
  "ai.enabled": boolean;
  "ai.model": string;
  "ai.keep_alive_minutes": number;
  "ai.num_ctx": number;
  "ai.num_thread": number;
  "ai.cloud_enabled": boolean;
  "ai.cloud_base_url": string;
  "ai.cloud_model": string;
}

export interface PersonalitySettings {
  "personality.mode": PersonalityMode;
  "personality.custom": string;
}

export interface PrivacySettings {
  "privacy.history_enabled": boolean;
  "privacy.history_days": number;
}

export interface VisionSettings {
  "vision.enabled": boolean;
  "vision.allow_cloud": boolean;
  "vision.vlm_model": string;
}

export interface SetupSettings {
  "setup.completed": boolean;
}

export type AppSettings = SetupSettings &
  GeneralSettings &
  VisualSettings &
  VoiceSettings &
  TtsSettings &
  ControlSettings &
  AiSettings &
  PersonalitySettings &
  PrivacySettings &
  VisionSettings;
export type GeneralKey = keyof GeneralSettings;
export type VisualKey = keyof VisualSettings;
export type VoiceKey = keyof VoiceSettings;
export type TtsKey = keyof TtsSettings;
export type ControlKey = keyof ControlSettings;
export type SettingKey = keyof AppSettings;

export const GENERAL_DEFAULTS: GeneralSettings = {
  "general.autostart": false,
  "general.language": "ru",
  "general.start_mode": "compact",
};

export const VISUAL_DEFAULTS: VisualSettings = {
  "visual.quality": "auto",
  "visual.fps_limit": 30,
  "visual.reduced_motion": false,
};

export const VOICE_DEFAULTS: VoiceSettings = {
  "voice.enabled": false,
  "voice.input_device": null,
  "voice.sensitivity": 0.5,
  "voice.wake_names": ["лёха", "лёх", "алексей"],
  "voice.ptt_key": "F8",
};

export const TTS_DEFAULTS: TtsSettings = {
  "tts.engine_ru": "piper",
  "tts.voice_ru": "ru_RU-dmitri-medium",
  "tts.engine_en": "kokoro",
  "tts.voice_en": "bm_george",
  "tts.speed": 1.0,
  "tts.volume": 0.9,
  "tts.output_device": null,
  "tts.cloud_enabled": false,
  "tts.eleven_voice_id": "",
};

export const CONTROL_DEFAULTS: ControlSettings = {
  "control.disabled_actions": [],
  "control.file_roots": [],
  "control.app_aliases": {},
  "control.emergency_key": "Ctrl+Alt+Pause",
};

export const AI_DEFAULTS: AiSettings = {
  "ai.enabled": true,
  "ai.model": "qwen3:4b",
  "ai.keep_alive_minutes": 5,
  "ai.num_ctx": 4096,
  "ai.num_thread": 6,
  "ai.cloud_enabled": false,
  "ai.cloud_base_url": "",
  "ai.cloud_model": "",
};

export const PERSONALITY_DEFAULTS: PersonalitySettings = {
  "personality.mode": "classic",
  "personality.custom": "",
};

export const PRIVACY_DEFAULTS: PrivacySettings = {
  "privacy.history_enabled": true,
  "privacy.history_days": 30,
};

export const VISION_DEFAULTS: VisionSettings = {
  "vision.enabled": true,
  "vision.allow_cloud": false,
  "vision.vlm_model": "gemma3:4b",
};

export const SETUP_DEFAULTS: SetupSettings = {
  "setup.completed": false,
};

export const SETTING_DEFAULTS: AppSettings = {
  ...SETUP_DEFAULTS,
  ...GENERAL_DEFAULTS,
  ...VISUAL_DEFAULTS,
  ...VOICE_DEFAULTS,
  ...TTS_DEFAULTS,
  ...CONTROL_DEFAULTS,
  ...AI_DEFAULTS,
  ...PERSONALITY_DEFAULTS,
  ...PRIVACY_DEFAULTS,
  ...VISION_DEFAULTS,
};

export const GENERAL_KEYS: readonly GeneralKey[] = [
  "general.autostart",
  "general.language",
  "general.start_mode",
];

export const VISUAL_KEYS: readonly VisualKey[] = [
  "visual.quality",
  "visual.fps_limit",
  "visual.reduced_motion",
];

export const VOICE_KEYS: readonly VoiceKey[] = [
  "voice.enabled",
  "voice.input_device",
  "voice.sensitivity",
  "voice.wake_names",
  "voice.ptt_key",
];

export const TTS_KEYS: readonly TtsKey[] = [
  "tts.engine_ru",
  "tts.voice_ru",
  "tts.engine_en",
  "tts.voice_en",
  "tts.speed",
  "tts.volume",
  "tts.output_device",
  "tts.cloud_enabled",
  "tts.eleven_voice_id",
];

export const CONTROL_KEYS: readonly ControlKey[] = [
  "control.disabled_actions",
  "control.file_roots",
  "control.app_aliases",
  "control.emergency_key",
];

export const AI_KEYS = Object.keys(AI_DEFAULTS) as (keyof AiSettings)[];
export const PERSONALITY_KEYS = Object.keys(PERSONALITY_DEFAULTS) as (keyof PersonalitySettings)[];
export const PRIVACY_KEYS = Object.keys(PRIVACY_DEFAULTS) as (keyof PrivacySettings)[];
export const VISION_KEYS = Object.keys(VISION_DEFAULTS) as (keyof VisionSettings)[];

export const SETUP_KEYS = Object.keys(SETUP_DEFAULTS) as (keyof SetupSettings)[];

export const SETTING_KEYS: readonly SettingKey[] = [
  ...SETUP_KEYS,
  ...GENERAL_KEYS,
  ...VISUAL_KEYS,
  ...VOICE_KEYS,
  ...TTS_KEYS,
  ...CONTROL_KEYS,
  ...AI_KEYS,
  ...PERSONALITY_KEYS,
  ...PRIVACY_KEYS,
  ...VISION_KEYS,
];

export const PERSONALITY_MODES: readonly PersonalityMode[] = ["formal", "classic", "friendly", "custom"];
export const MAX_CUSTOM_CHARS = 2000;
export const NUM_CTX_RANGE = { min: 1024, max: 4096 } as const;
export const NUM_THREAD_RANGE = { min: 1, max: 6 } as const;
export const MAX_KEEP_ALIVE_MINUTES = 1440;
export const MAX_HISTORY_DAYS = 365;

const MODEL_NAME = /^[A-Za-z0-9._:/-]{1,100}$/;

/** An Ollama model tag such as `qwen3:4b` (mirrors the backend). */
export function validModelName(value: unknown): value is string {
  return typeof value === "string" && MODEL_NAME.test(value);
}

/** The cloud endpoint is empty (not set) or an https URL without credentials. */
export function validCloudUrl(value: unknown): value is string {
  if (typeof value !== "string") return false;
  if (value === "") return true;
  if (value.length > 300 || [...value].some((c) => c.charCodeAt(0) <= 32) || !value.startsWith("https://")) return false;
  const authority = value.slice("https://".length).split(/[/?#]/)[0] ?? "";
  return authority.length > 0 && !authority.includes("@");
}

function integerIn(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= min && value <= max;
}

export const TTS_SPEED_RANGE = { min: 0.7, max: 1.4, step: 0.05 } as const;
export const TTS_ENGINES_EN: readonly TtsEngineEn[] = ["kokoro", "piper"];

/** Voice ids become file names and URLs: letters, digits, `_` and `-` (mirrors the backend). */
const VOICE_ID_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

export function validVoiceId(value: unknown): value is string {
  return typeof value === "string" && VOICE_ID_PATTERN.test(value);
}

/** The ElevenLabs voice id may be empty (no cloud voice). */
export function validElevenVoiceId(value: unknown): value is string {
  return value === "" || validVoiceId(value);
}

export const MAX_WAKE_NAMES = 5;
export const MAX_NAME_LENGTH = 20;
const NAME_PATTERN = /^[A-Za-zА-яЁё]+$/;

/** 1 to 5 single words of up to 20 Cyrillic or Latin letters (mirrors the backend rule). */
export function validWakeNames(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.length >= 1 &&
    value.length <= MAX_WAKE_NAMES &&
    value.every(
      (name) => typeof name === "string" && name.length <= MAX_NAME_LENGTH && NAME_PATTERN.test(name),
    )
  );
}

/** Splits "лёха, лёх алексей" into names; separators are commas, semicolons and whitespace. */
export function parseWakeNames(text: string): string[] {
  return text
    .split(/[\s,;]+/)
    .map((name) => name.trim())
    .filter((name) => name.length > 0);
}

export const LANGUAGES: readonly Language[] = ["ru", "en"];
export const START_MODES: readonly StartMode[] = ["tray", "compact", "full"];
export { FPS_LIMITS, QUALITY_SETTINGS };

export type Validation<T> = { ok: true; value: T } | { ok: false; error: string };

/** Mirrors the backend whitelist (API_CONTRACTS, settings whitelist). */
export function validateSetting<K extends SettingKey>(
  key: K,
  value: unknown,
): Validation<AppSettings[K]> {
  const accept = (candidate: unknown): Validation<AppSettings[K]> => ({
    ok: true,
    value: candidate as AppSettings[K],
  });
  const expectBoolean = (): Validation<AppSettings[K]> =>
    typeof value === "boolean" ? accept(value) : { ok: false, error: `${key}: expected boolean` };
  switch (key) {
    case "general.autostart":
    case "visual.reduced_motion":
      return expectBoolean();
    case "general.language":
      return LANGUAGES.some((item) => item === value)
        ? accept(value)
        : { ok: false, error: `${key}: expected one of ${LANGUAGES.join(", ")}` };
    case "general.start_mode":
      return START_MODES.some((item) => item === value)
        ? accept(value)
        : { ok: false, error: `${key}: expected one of ${START_MODES.join(", ")}` };
    case "visual.quality":
      return QUALITY_SETTINGS.some((item) => item === value)
        ? accept(value)
        : { ok: false, error: `${key}: expected one of ${QUALITY_SETTINGS.join(", ")}` };
    case "visual.fps_limit":
      return FPS_LIMITS.some((item) => item === value)
        ? accept(value)
        : { ok: false, error: `${key}: expected one of ${FPS_LIMITS.join(", ")}` };
    case "voice.enabled":
      return expectBoolean();
    case "voice.input_device":
      return value === null || (typeof value === "string" && value.length > 0 && value.length <= 200)
        ? accept(value)
        : { ok: false, error: `${key}: expected null or a device name` };
    case "voice.sensitivity":
      return typeof value === "number" && value >= 0 && value <= 1
        ? accept(value)
        : { ok: false, error: `${key}: expected a number from 0 to 1` };
    case "voice.wake_names":
      return validWakeNames(value)
        ? accept(value)
        : { ok: false, error: `${key}: expected 1 to ${MAX_WAKE_NAMES} names of letters, up to ${MAX_NAME_LENGTH} characters` };
    case "voice.ptt_key":
      return typeof value === "string" && value.trim().length > 0 && value.length <= 40
        ? accept(value.trim())
        : { ok: false, error: `${key}: expected a key such as F8` };
    case "tts.engine_ru":
      return value === "piper" ? accept(value) : { ok: false, error: `${key}: expected piper` };
    case "tts.engine_en":
      return TTS_ENGINES_EN.some((item) => item === value)
        ? accept(value)
        : { ok: false, error: `${key}: expected one of ${TTS_ENGINES_EN.join(", ")}` };
    case "tts.voice_ru":
    case "tts.voice_en":
      return validVoiceId(value)
        ? accept(value)
        : { ok: false, error: `${key}: expected a voice id (letters, digits, _ and -)` };
    case "tts.speed":
      return typeof value === "number" && value >= TTS_SPEED_RANGE.min && value <= TTS_SPEED_RANGE.max
        ? accept(value)
        : { ok: false, error: `${key}: expected a number from 0.7 to 1.4` };
    case "tts.volume":
      return typeof value === "number" && value >= 0 && value <= 1
        ? accept(value)
        : { ok: false, error: `${key}: expected a number from 0 to 1` };
    case "tts.output_device":
      return value === null || (typeof value === "string" && value.length > 0 && value.length <= 200)
        ? accept(value)
        : { ok: false, error: `${key}: expected null or a device name` };
    case "tts.cloud_enabled":
      return expectBoolean();
    case "tts.eleven_voice_id":
      return validElevenVoiceId(value)
        ? accept(value)
        : { ok: false, error: `${key}: expected an empty string or a voice id` };
    case "control.disabled_actions":
      return Array.isArray(value) && value.every((item) => typeof item === "string") && new Set(value).size === value.length
        ? accept(value)
        : { ok: false, error: `${key}: expected a list of action names` };
    case "control.file_roots":
      return Array.isArray(value) && value.length <= MAX_ROOTS && value.every(validDrivePath) && new Set(value.map((item: string) => item.toLowerCase())).size === value.length
        ? accept(value)
        : { ok: false, error: `${key}: expected up to ${MAX_ROOTS} folders like D:\\Games (no network paths)` };
    case "control.app_aliases":
      return validAliases(value)
        ? accept(value)
        : { ok: false, error: `${key}: up to ${MAX_ALIASES} aliases; a target is an .exe or .lnk path on a drive or steam://rungameid/<digits>` };
    case "control.emergency_key":
      return typeof value === "string" && value.trim().length > 0 && value.length <= 40
        ? accept(value.trim())
        : { ok: false, error: `${key}: expected a key such as Ctrl+Alt+Pause` };
    case "ai.enabled":
    case "ai.cloud_enabled":
    case "privacy.history_enabled":
    case "vision.enabled":
    case "vision.allow_cloud":
    case "setup.completed":
      return expectBoolean();
    case "vision.vlm_model":
      return validModelName(value) ? accept(value) : { ok: false, error: `${key}: expected a model tag such as gemma3:4b` };
    case "ai.model":
      return validModelName(value) ? accept(value) : { ok: false, error: `${key}: expected a model tag such as qwen3:4b` };
    case "ai.keep_alive_minutes":
      return integerIn(value, 0, MAX_KEEP_ALIVE_MINUTES)
        ? accept(value)
        : { ok: false, error: `${key}: expected a whole number from 0 to ${MAX_KEEP_ALIVE_MINUTES}` };
    case "ai.num_ctx":
      return integerIn(value, NUM_CTX_RANGE.min, NUM_CTX_RANGE.max)
        ? accept(value)
        : { ok: false, error: `${key}: expected a whole number from ${NUM_CTX_RANGE.min} to ${NUM_CTX_RANGE.max}` };
    case "ai.num_thread":
      return integerIn(value, NUM_THREAD_RANGE.min, NUM_THREAD_RANGE.max)
        ? accept(value)
        : { ok: false, error: `${key}: expected a whole number from 1 to 6` };
    case "ai.cloud_base_url":
      return validCloudUrl(value) ? accept(value) : { ok: false, error: `${key}: expected an https:// URL without credentials` };
    case "ai.cloud_model":
      return value === "" || validModelName(value)
        ? accept(value)
        : { ok: false, error: `${key}: expected a model name` };
    case "personality.mode":
      return PERSONALITY_MODES.some((item) => item === value)
        ? accept(value)
        : { ok: false, error: `${key}: expected one of ${PERSONALITY_MODES.join(", ")}` };
    case "personality.custom":
      return typeof value === "string" && [...value].length <= MAX_CUSTOM_CHARS
        ? accept(value)
        : { ok: false, error: `${key}: at most ${MAX_CUSTOM_CHARS} characters` };
    case "privacy.history_days":
      return integerIn(value, 1, MAX_HISTORY_DAYS)
        ? accept(value)
        : { ok: false, error: `${key}: expected a whole number from 1 to ${MAX_HISTORY_DAYS}` };
    default:
      return { ok: false, error: `${String(key)}: unknown setting` };
  }
}

/** Builds the form state from stored values; invalid or missing entries fall back to defaults. */
export function resolveSettings(stored: Partial<Record<SettingKey, unknown>>): AppSettings {
  const result: AppSettings = { ...SETTING_DEFAULTS };
  const assign = <K extends SettingKey>(key: K): void => {
    const checked = validateSetting(key, stored[key]);
    if (checked.ok) result[key] = checked.value;
  };
  for (const key of SETTING_KEYS) assign(key);
  return result;
}

/** Same as `resolveSettings`, restricted to the general section. */
export function resolveGeneral(stored: Partial<Record<GeneralKey, unknown>>): GeneralSettings {
  const result: GeneralSettings = { ...GENERAL_DEFAULTS };
  for (const key of GENERAL_KEYS) {
    const checked = validateSetting(key, stored[key]);
    if (checked.ok) Object.assign(result, { [key]: checked.value });
  }
  return result;
}

export const MAX_ALIASES = 50;
export const MAX_ROOTS = 20;
export const MAX_ALIAS_NAME = 40;
export const MAX_PATH_LENGTH = 260;

/** `X:\folder` (no UNC, no device paths, no `..`); the backend also checks that the file or folder exists. */
export function validDrivePath(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= MAX_PATH_LENGTH &&
    /^[A-Za-z]:[\\/]/.test(value) &&
    !value.includes("\0") &&
    !value.split(/[\\/]/).some((part) => part === ".." || part === ".")
  );
}

const STEAM_LINK = /^steam:\/\/rungameid\/\d{1,12}$/;

/** An alias target: a Steam link or an `.exe` / `.lnk` path on a drive. */
export function validAliasTarget(value: unknown): value is string {
  if (typeof value !== "string") return false;
  if (STEAM_LINK.test(value)) return true;
  return validDrivePath(value) && /\.(exe|lnk)$/i.test(value);
}

export function validAliasName(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= MAX_ALIAS_NAME &&
    value === value.trim() &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u001f\u007f]/.test(value)
  );
}

export function validAliases(value: unknown): value is Record<string, string> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const entries = Object.entries(value);
  return (
    entries.length <= MAX_ALIASES &&
    entries.every(([name, target]) => validAliasName(name) && validAliasTarget(target))
  );
}
