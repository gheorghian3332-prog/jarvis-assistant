import type { AlertPayload } from "./api";
import type { Validation } from "./settings";

/** Alert rules as the backend stores them in `sysintel.alerts` (mirrors `AlertConfig`). */
export interface ThresholdRule {
  enabled: boolean;
  threshold: number;
  duration_s: number;
}

export interface DurationRule {
  enabled: boolean;
  duration_s: number;
}

export interface DiskRule {
  enabled: boolean;
  min_free_gb: number;
  min_free_percent: number;
  check_every_s: number;
}

export interface QuietHours {
  enabled: boolean;
  start_minute: number;
  end_minute: number;
}

export interface AlertConfig {
  cpu_load: ThresholdRule;
  gpu_temp: ThresholdRule;
  cpu_temp: ThresholdRule;
  ram_load: ThresholdRule;
  disk_space: DiskRule;
  internet_down: DurationRule;
  window_hung: DurationRule;
  app_crash: DurationRule;
  hysteresis: number;
  hysteresis_hold_s: number;
  cooldown_s: number;
  max_voice_per_hour: number;
  quiet_hours: QuietHours;
}

export const ALERT_DEFAULTS: AlertConfig = {
  cpu_load: { enabled: true, threshold: 90, duration_s: 60 },
  gpu_temp: { enabled: true, threshold: 83, duration_s: 30 },
  cpu_temp: { enabled: true, threshold: 90, duration_s: 30 },
  ram_load: { enabled: true, threshold: 92, duration_s: 30 },
  disk_space: { enabled: true, min_free_gb: 10, min_free_percent: 5, check_every_s: 600 },
  internet_down: { enabled: true, duration_s: 20 },
  window_hung: { enabled: true, duration_s: 15 },
  app_crash: { enabled: true, duration_s: 0 },
  hysteresis: 5,
  hysteresis_hold_s: 30,
  cooldown_s: 900,
  max_voice_per_hour: 3,
  quiet_hours: { enabled: false, start_minute: 22 * 60, end_minute: 7 * 60 },
};

export const THRESHOLD_RULES = ["cpu_load", "gpu_temp", "cpu_temp", "ram_load"] as const;
export type ThresholdName = (typeof THRESHOLD_RULES)[number];
export const SWITCH_RULES = ["disk_space", "internet_down", "window_hung", "app_crash"] as const;
export type SwitchName = (typeof SWITCH_RULES)[number];
export type RuleName = ThresholdName | SwitchName;

/** The allowed range of each threshold (mirrors the backend). */
export const THRESHOLD_RANGE: Record<ThresholdName, { min: number; max: number; unit: "%" | "°C" }> = {
  cpu_load: { min: 50, max: 100, unit: "%" },
  gpu_temp: { min: 40, max: 120, unit: "°C" },
  cpu_temp: { min: 40, max: 120, unit: "°C" },
  ram_load: { min: 50, max: 100, unit: "%" },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function bool(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function num(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/** The stored value merged over the defaults; anything of the wrong type keeps its default. */
export function resolveAlerts(stored: unknown): AlertConfig {
  const source = isRecord(stored) ? stored : {};
  const d = ALERT_DEFAULTS;
  const threshold = (name: ThresholdName): ThresholdRule => {
    const raw = isRecord(source[name]) ? source[name] : {};
    return {
      enabled: bool(raw.enabled, d[name].enabled),
      threshold: num(raw.threshold, d[name].threshold),
      duration_s: num(raw.duration_s, d[name].duration_s),
    };
  };
  const duration = (name: "internet_down" | "window_hung" | "app_crash"): DurationRule => {
    const raw = isRecord(source[name]) ? source[name] : {};
    return { enabled: bool(raw.enabled, d[name].enabled), duration_s: num(raw.duration_s, d[name].duration_s) };
  };
  const disk = isRecord(source.disk_space) ? source.disk_space : {};
  const quiet = isRecord(source.quiet_hours) ? source.quiet_hours : {};
  return {
    cpu_load: threshold("cpu_load"),
    gpu_temp: threshold("gpu_temp"),
    cpu_temp: threshold("cpu_temp"),
    ram_load: threshold("ram_load"),
    disk_space: {
      enabled: bool(disk.enabled, d.disk_space.enabled),
      min_free_gb: num(disk.min_free_gb, d.disk_space.min_free_gb),
      min_free_percent: num(disk.min_free_percent, d.disk_space.min_free_percent),
      check_every_s: num(disk.check_every_s, d.disk_space.check_every_s),
    },
    internet_down: duration("internet_down"),
    window_hung: duration("window_hung"),
    app_crash: duration("app_crash"),
    hysteresis: num(source.hysteresis, d.hysteresis),
    hysteresis_hold_s: num(source.hysteresis_hold_s, d.hysteresis_hold_s),
    cooldown_s: num(source.cooldown_s, d.cooldown_s),
    max_voice_per_hour: num(source.max_voice_per_hour, d.max_voice_per_hour),
    quiet_hours: {
      enabled: bool(quiet.enabled, d.quiet_hours.enabled),
      start_minute: num(quiet.start_minute, d.quiet_hours.start_minute),
      end_minute: num(quiet.end_minute, d.quiet_hours.end_minute),
    },
  };
}

function inRange(value: number, min: number, max: number): boolean {
  return Number.isFinite(value) && value >= min && value <= max;
}

/** The same range checks the backend applies to `sysintel.alerts`. */
export function validateAlerts(config: AlertConfig): Validation<AlertConfig> {
  const fail = (message: string): Validation<AlertConfig> => ({ ok: false, error: `sysintel.alerts: ${message}` });
  for (const name of THRESHOLD_RULES) {
    const range = THRESHOLD_RANGE[name];
    const rule = config[name];
    if (!inRange(rule.threshold, range.min, range.max)) return fail(`${name}.threshold must be ${range.min}..${range.max}`);
    if (!inRange(rule.duration_s, 0, 3600)) return fail(`${name}.duration_s must be 0..3600`);
  }
  const disk = config.disk_space;
  if (!inRange(disk.min_free_gb, 0, 2000)) return fail("disk_space.min_free_gb must be 0..2000");
  if (!inRange(disk.min_free_percent, 0, 50)) return fail("disk_space.min_free_percent must be 0..50");
  if (!inRange(disk.check_every_s, 10, 86400)) return fail("disk_space.check_every_s must be 10..86400");
  for (const name of ["internet_down", "window_hung", "app_crash"] as const) {
    if (!inRange(config[name].duration_s, 0, 3600)) return fail(`${name}.duration_s must be 0..3600`);
  }
  if (!inRange(config.hysteresis, 0, 30)) return fail("hysteresis must be 0..30");
  if (!inRange(config.hysteresis_hold_s, 0, 3600)) return fail("hysteresis_hold_s must be 0..3600");
  if (!inRange(config.cooldown_s, 0, 86400)) return fail("cooldown_s must be 0..86400");
  if (!Number.isInteger(config.max_voice_per_hour) || !inRange(config.max_voice_per_hour, 0, 60)) {
    return fail("max_voice_per_hour must be 0..60");
  }
  const quiet = config.quiet_hours;
  for (const minute of [quiet.start_minute, quiet.end_minute]) {
    if (!Number.isInteger(minute) || !inRange(minute, 0, 1439)) return fail("quiet_hours minutes must be 0..1439");
  }
  return { ok: true, value: config };
}

export function withThreshold(config: AlertConfig, name: ThresholdName, patch: Partial<ThresholdRule>): AlertConfig {
  return { ...config, [name]: { ...config[name], ...patch } };
}

export function withEnabled(config: AlertConfig, name: RuleName, enabled: boolean): AlertConfig {
  return { ...config, [name]: { ...config[name], enabled } };
}

/** `1380` -> `"23:00"`. */
export function minuteToTime(minute: number): string {
  const clamped = Math.min(1439, Math.max(0, Math.trunc(minute)));
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(Math.floor(clamped / 60))}:${pad(clamped % 60)}`;
}

/** `"23:00"` -> `1380`; anything that is not a time of day gives `null`. */
export function timeToMinute(text: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(text.trim());
  if (!match) return null;
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  return hours < 24 && minutes < 60 ? hours * 60 + minutes : null;
}

/** `host:port` for the latency probe (mirrors the backend). */
export function validRttHost(text: string): boolean {
  const index = text.lastIndexOf(":");
  if (index <= 0) return false;
  const host = text.slice(0, index);
  const port = Number(text.slice(index + 1));
  const hostOk = /^[A-Za-z0-9]([A-Za-z0-9.-]{0,98}[A-Za-z0-9])?$/.test(host);
  return hostOk && Number.isInteger(port) && port > 0 && port <= 65535;
}

export function formatRate(bytesPerSecond: number | null): string {
  if (bytesPerSecond === null || !Number.isFinite(bytesPerSecond) || bytesPerSecond < 0) return "—";
  if (bytesPerSecond >= 1024 ** 2) return `${(bytesPerSecond / 1024 ** 2).toFixed(1)} MB/s`;
  if (bytesPerSecond >= 1024) return `${Math.round(bytesPerSecond / 1024)} KB/s`;
  return `${Math.round(bytesPerSecond)} B/s`;
}

export function formatTemperature(celsius: number | null): string {
  return celsius === null || !Number.isFinite(celsius) ? "—" : `${Math.round(celsius)} °C`;
}

export function formatMegahertz(mhz: number | null): string {
  if (mhz === null || !Number.isFinite(mhz)) return "—";
  return mhz >= 1000 ? `${(mhz / 1000).toFixed(2)} GHz` : `${Math.round(mhz)} MHz`;
}

export function formatPercentOrDash(value: number | null): string {
  return value === null || !Number.isFinite(value) ? "—" : `${Math.round(value)}%`;
}

/** Free space share of a volume, 0..100. */
export function freePercent(totalBytes: number, freeBytes: number): number {
  if (totalBytes <= 0) return 0;
  return Math.min(100, Math.max(0, (freeBytes / totalBytes) * 100));
}

export const MAX_FEED = 50;

/** Adds an alert at the top of the feed (newest first); an alert that is already there is not repeated. */
export function pushAlert(feed: readonly AlertPayload[], alert: AlertPayload, max: number = MAX_FEED): AlertPayload[] {
  if (feed.some((item) => item.id === alert.id && item.at_ms === alert.at_ms)) return [...feed];
  return [alert, ...feed].slice(0, max);
}

/** The alert text in the interface language. */
export function alertText(alert: AlertPayload, language: "ru" | "en"): string {
  return language === "en" ? alert.message_en : alert.message_ru;
}

/** How long the header toast stays, by severity. */
export function toastMillis(alert: AlertPayload): number {
  return alert.severity === "critical" ? 10000 : 6000;
}
