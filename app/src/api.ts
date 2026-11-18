import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ErrorCode =
  | "VALIDATION"
  | "FORBIDDEN"
  | "NOT_FOUND"
  | "SYSTEM"
  | "TIMEOUT"
  | "UNAVAILABLE"
  | "UNKNOWN";

export interface CommandError {
  code: ErrorCode;
  message: string;
}

export type WindowMode = "full" | "compact" | "tray";

export interface CoreInfo {
  version: string;
  uptime_s: number;
  features: string[];
}

export interface AuditEntry {
  id: number;
  ts: string;
  action: string;
  args: unknown;
  risk: string;
  ok: boolean;
  detail: string;
}

export interface MetricsPayload {
  cpu_percent: number;
  ram_used_bytes: number;
  ram_total_bytes: number;
}

export type AssistantState =
  | "IDLE"
  | "LISTENING"
  | "PROCESSING"
  | "SPEAKING"
  | "EXECUTING"
  | "WARNING"
  | "ERROR"
  | "OFFLINE";

const KNOWN_CODES: readonly ErrorCode[] = [
  "VALIDATION",
  "FORBIDDEN",
  "NOT_FOUND",
  "SYSTEM",
  "TIMEOUT",
  "UNAVAILABLE",
];

/** Normalizes whatever `invoke` rejected with into `{code, message}`. */
export function toCommandError(raw: unknown): CommandError {
  if (typeof raw === "object" && raw !== null) {
    const record = raw as Record<string, unknown>;
    if (typeof record.message === "string") {
      const code = KNOWN_CODES.find((known) => known === record.code);
      return { code: code ?? "UNKNOWN", message: record.message };
    }
  }
  if (typeof raw === "string") {
    return { code: "UNKNOWN", message: raw };
  }
  return { code: "UNKNOWN", message: "Unknown error" };
}

/** True inside the Tauri webview; false in a plain browser (`npm run dev`), where there is no backend. */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Subscribes to a backend event; outside Tauri it does nothing and the returned stop function is a no-op. */
export async function listenEvent<T>(name: string, handler: (payload: T) => void): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  return listen<T>(name, (event) => handler(event.payload));
}

/** Calls a Tauri command; always rejects with a `CommandError` (`UNAVAILABLE` outside Tauri). */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw { code: "UNAVAILABLE", message: "Backend is not available outside Tauri" } satisfies CommandError;
  }
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toCommandError(error);
  }
}

export const coreInfo = () => call<CoreInfo>("core_info");
export const settingsGet = (key: string) => call<unknown>("settings_get", { key });
export const settingsSet = (key: string, value: unknown) =>
  call<void>("settings_set", { key, value });
export const auditList = (limit: number) => call<AuditEntry[]>("audit_list", { limit });
export const setWindowMode = (mode: WindowMode) => call<void>("set_window_mode", { mode });

export interface InputDevice {
  name: string;
  is_default: boolean;
}

/** Microphone health from `voice://status`; `no_signal` means the device delivers nothing but zeros. */
export interface MicStatus {
  active: boolean;
  device: string;
  peak: number | null;
  no_signal: boolean;
  error: string | null;
}

export interface VoiceStatus {
  state: AssistantState;
  mic: MicStatus | null;
}

export const voiceDevices = () => call<InputDevice[]>("voice_devices");
export const voiceStatus = () => call<VoiceStatus>("voice_status");
/** Push-to-talk from the UI: `true` while the button is held, `false` on release. */
export const ptt = (pressed: boolean) => call<void>("ptt", { pressed });

/** `voice://transcript`: what the last utterance said. */
export interface TranscriptPayload {
  text: string;
  final: boolean;
  lang: string;
}

/** An output device (`audio_outputs`). */
export interface OutputDevice {
  name: string;
  is_default: boolean;
}

export type TtsEngineName = "piper" | "kokoro" | "elevenlabs";

export interface TtsEngineStatus {
  engine: TtsEngineName;
  available: boolean;
  detail: string;
}

export interface TtsVoice {
  engine: TtsEngineName;
  id: string;
  lang: "ru" | "en" | "other";
  installed: boolean;
  selected: boolean;
}

export interface TtsVoices {
  engines: TtsEngineStatus[];
  voices: TtsVoice[];
}

/** Secrets the UI may store; the value is never read back. */
export type SecretName = "elevenlabs_api_key" | "ai_cloud_api_key";
export const ELEVEN_KEY: SecretName = "elevenlabs_api_key";
export const AI_CLOUD_KEY: SecretName = "ai_cloud_api_key";

/** Speaks `text` (queued at once; a speech in progress is interrupted). */
export const ttsSpeak = (text: string) => call<void>("tts_speak", { text });
export const ttsStop = () => call<void>("tts_stop");
export const ttsVoices = () => call<TtsVoices>("tts_voices");
export const audioOutputs = () => call<OutputDevice[]>("audio_outputs");
export const secretSet = (name: SecretName, value: string) => call<void>("secret_set", { name, value });
export const secretClear = (name: SecretName) => call<void>("secret_clear", { name });
export const secretExists = (name: SecretName) => call<boolean>("secret_exists", { name });

/** What a command is allowed to do: `forbidden` actions can never be switched on. */
export type Risk = "safe" | "confirm" | "forbidden";

/** One row of the TOOLS table (`control_actions`). */
export interface ActionInfo {
  name: string;
  risk: Risk;
  description_ru: string;
  description_en: string;
  enabled: boolean;
}

export interface ControlInfo {
  actions: ActionInfo[];
  default_roots: string[];
}

/** `confirm://request`: the user must approve before anything happens. */
export interface ConfirmRequest {
  action_id: string;
  action: string;
  summary: string;
  risk: Risk;
}

/** `action://result`. */
export interface ActionResultPayload {
  action_id: string;
  ok: boolean;
  detail: string;
}

/** `assistant://message`. */
export interface AssistantMessage {
  text: string;
  lang: string;
  skill?: string | null;
}

/** A typed command: the same path a spoken phrase takes. */
export const chatSend = (text: string) => call<void>("chat_send", { text });
/** Tauri maps the Rust argument `action_id` to `actionId` on the JS side. */
export const confirmResponse = (actionId: string, approved: boolean) =>
  call<void>("confirm_response", { actionId, approved });
export const emergencyStop = () => call<void>("emergency_stop");
export const controlActions = () => call<ControlInfo>("control_actions");

export interface ModelEntry {
  name: string;
  size_bytes: number;
}

export interface LoadedModelEntry {
  name: string;
  size_bytes: number;
  vram_bytes: number;
  expires_at: string;
}

/** `ai_status`: the Ollama server as it is right now (asking never starts it). */
export interface AiStatus {
  enabled: boolean;
  running: boolean;
  version: string | null;
  model: string;
  models: ModelEntry[];
  loaded: LoadedModelEntry[];
}

export interface MemoryFact {
  id: number;
  ts: string;
  kind: string;
  key: string;
  value: string;
}

export interface HistoryRow {
  id: number;
  ts: string;
  text: string;
  lang: string;
  route: string;
  action: string | null;
  ok: boolean;
}

export const aiStatus = () => call<AiStatus>("ai_status");
export const aiUnload = () => call<void>("ai_unload");
export const memoryFacts = (query: string | null, limit: number) =>
  call<MemoryFact[]>("memory_facts", { query, limit });
export const memoryFactDelete = (id: number) => call<void>("memory_fact_delete", { id });
export const memoryHistory = (query: string | null, limit: number) =>
  call<HistoryRow[]>("memory_history", { query, limit });
export const memoryHistoryDelete = (id: number) => call<void>("memory_history_delete", { id });
export const memoryClearHistory = () => call<number>("memory_clear_history");
/** The backend refuses without `confirmed: true`; the UI asks first. */
export const memoryClearAll = (confirmed: boolean) => call<void>("memory_clear_all", { confirmed });

// ---------- system intelligence (SYSTEM tab) ----------

export interface GpuMetrics {
  name: string | null;
  utilization_percent: number | null;
  temperature_c: number | null;
  vram_used_bytes: number | null;
  vram_total_bytes: number | null;
  graphics_clock_mhz: number | null;
  memory_clock_mhz: number | null;
  power_watts: number | null;
}

export interface VolumeMetrics {
  mount: string;
  is_system: boolean;
  total_bytes: number;
  free_bytes: number;
  read_bytes_per_sec: number | null;
  write_bytes_per_sec: number | null;
}

export interface DriveHealth {
  name: string;
  status: string;
}

export interface HungWindow {
  hwnd: number;
  pid: number;
  process: string | null;
  title: string;
}

/** Everything a sensor may not provide is `null`: "unavailable on this machine", never a placeholder. */
export interface SystemMetrics {
  timestamp_ms: number;
  cpu: {
    usage_percent: number | null;
    per_core_percent: number[];
    frequency_mhz: number | null;
    temperature_c: number | null;
  };
  gpu: GpuMetrics | null;
  ram: { used_bytes: number; total_bytes: number; available_bytes: number };
  storage: { volumes: VolumeMetrics[]; health: DriveHealth[] | null };
  network: {
    link_up: boolean | null;
    internet_reachable: boolean | null;
    down_bytes_per_sec: number | null;
    up_bytes_per_sec: number | null;
    rtt_ms: number | null;
    rtt_host: string | null;
  };
  hung_windows: HungWindow[];
}

export type GpuSource = "nvml" | "d3dkmt";

/** `system_snapshot`: the latest values, the polling mode and where the GPU numbers come from. */
export interface SystemSnapshot {
  metrics: SystemMetrics;
  mode: "window" | "tray";
  gpu_source: GpuSource | null;
}

export interface ProcessUsage {
  exe: string;
  display_name: string;
  instances: number;
  memory_mb: number;
  cpu_percent: number | null;
}

export type AlertSeverity = "info" | "warning" | "critical";

/** `alert://raised` and one row of `system_alerts`. */
export interface AlertPayload {
  id: string;
  kind: string;
  severity: AlertSeverity;
  message_ru: string;
  message_en: string;
  speak: boolean;
  at_ms: number;
}

/** `vision://capture`: the screen is being captured and analysed. */
export interface CapturePayload {
  active: boolean;
}

export const systemSnapshot = () => call<SystemSnapshot>("system_snapshot");
export const systemTopProcesses = (by: "cpu" | "ram") => call<ProcessUsage[]>("system_top_processes", { by });
export const systemAlerts = () => call<AlertPayload[]>("system_alerts");

/** One line of the first-run wizard (`setup_status`). */
export interface SetupItem {
  id: string;
  installed: boolean;
  optional: boolean;
  approx_mb: number;
  running: boolean;
}

export interface SetupStatus {
  items: SetupItem[];
  installed_layout: boolean;
  models_dir: string;
  ollama_url: string;
  default_model: string;
}

/** `setup://progress`: `state` is running, done or error. */
export interface SetupProgress {
  item: string;
  state: "running" | "done" | "error";
  done: number;
  total: number;
  message: string;
}

export const setupStatus = () => call<SetupStatus>("setup_status");
export const setupInstall = (item: string) => call<void>("setup_install", { item });
export const setupPullModel = (model: string) => call<void>("setup_pull_model", { model });
export const setupOpenOllamaPage = () => call<void>("setup_open_ollama_page");
