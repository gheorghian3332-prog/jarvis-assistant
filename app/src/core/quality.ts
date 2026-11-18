export type QualityLevel = "low" | "medium" | "high";
export type QualitySetting = "auto" | QualityLevel;

export const QUALITY_LEVELS: readonly QualityLevel[] = ["low", "medium", "high"];
export const QUALITY_SETTINGS: readonly QualitySetting[] = ["auto", "low", "medium", "high"];

export interface QualitySpec {
  neurons: number;
  /** Upper bound for the number of travelling impulses. */
  pulseCap: number;
  /** Neuron sprite scale: fewer neurons are drawn larger so the shape stays readable. */
  sizeScale: number;
}

export const QUALITY_SPECS: Record<QualityLevel, QualitySpec> = {
  low: { neurons: 300, pulseCap: 80, sizeScale: 0.075 },
  medium: { neurons: 1500, pulseCap: 400, sizeScale: 0.04 },
  high: { neurons: 4000, pulseCap: 1100, sizeScale: 0.026 },
};

const rank = (level: QualityLevel): number => QUALITY_LEVELS.indexOf(level);

/** Device-pixel-ratio cap per level: low 0.75, medium 1.0, high min(devicePixelRatio, 1.5). */
export function dprFor(level: QualityLevel, devicePixelRatio: number): number {
  if (level === "low") return 0.75;
  if (level === "medium") return 1;
  const native = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  return Math.min(native, 1.5);
}

/** The compact window never renders above `medium`. */
export function capForMode(level: QualityLevel, compact: boolean): QualityLevel {
  return compact && rank(level) > rank("medium") ? "medium" : level;
}

/** Highest level the current window mode allows. */
export function maxLevelFor(compact: boolean): QualityLevel {
  return compact ? "medium" : "high";
}

/** Level actually rendered: the fixed setting, or the adaptive controller's pick for `auto`. */
export function resolveQuality(setting: QualitySetting, autoLevel: QualityLevel, compact: boolean): QualityLevel {
  return capForMode(setting === "auto" ? autoLevel : setting, compact);
}

/** `low-power` for low/medium (and auto, which starts there); `high` leaves the choice to the browser. */
export function powerPreferenceFor(setting: QualitySetting): "low-power" | "default" {
  return setting === "high" ? "default" : "low-power";
}

export interface AdaptiveConfig {
  /** Frame time above which the picture is "slow". */
  slowMs: number;
  /** Frame time below which the picture is "fast". */
  fastMs: number;
  /** Sustained slowness before stepping down. */
  slowAfterMs: number;
  /** Sustained speed before stepping up. */
  fastAfterMs: number;
  /** Minimum time between two switches. */
  cooldownMs: number;
  /** Time constant of the frame-time moving average. */
  emaTauMs: number;
}

export const ADAPTIVE_DEFAULTS: AdaptiveConfig = {
  slowMs: 25,
  fastMs: 12,
  slowAfterMs: 3000,
  fastAfterMs: 10000,
  cooldownMs: 5000,
  emaTauMs: 400,
};

export interface AdaptiveState {
  level: QualityLevel;
  emaMs: number | null;
  slowSince: number | null;
  fastSince: number | null;
  lastSwitchAt: number;
}

export function createAdaptiveState(level: QualityLevel, now: number): AdaptiveState {
  return { level, emaMs: null, slowSince: null, fastSince: null, lastSwitchAt: now };
}

/**
 * Pure quality controller. Feed it the frame time of each frame; it returns the next state.
 * Steps down after `slowAfterMs` above `slowMs`, up after `fastAfterMs` below `fastMs` (never above
 * `maxLevel`), and never switches twice within `cooldownMs`. The band between the two thresholds
 * is the hysteresis: it resets both timers.
 */
export function stepAdaptive(
  state: AdaptiveState,
  frameMs: number,
  now: number,
  maxLevel: QualityLevel,
  config: AdaptiveConfig = ADAPTIVE_DEFAULTS,
): AdaptiveState {
  if (rank(state.level) > rank(maxLevel)) {
    return { ...state, level: maxLevel, slowSince: null, fastSince: null, lastSwitchAt: now };
  }
  const alpha = 1 - Math.exp(-frameMs / config.emaTauMs);
  const emaMs = state.emaMs === null ? frameMs : state.emaMs + (frameMs - state.emaMs) * alpha;
  const slowSince = emaMs > config.slowMs ? (state.slowSince ?? now) : null;
  const fastSince = emaMs < config.fastMs ? (state.fastSince ?? now) : null;
  const canSwitch = now - state.lastSwitchAt >= config.cooldownMs;
  const index = rank(state.level);
  if (canSwitch && slowSince !== null && now - slowSince >= config.slowAfterMs && index > 0) {
    return { level: QUALITY_LEVELS[index - 1] ?? state.level, emaMs, slowSince: null, fastSince: null, lastSwitchAt: now };
  }
  if (canSwitch && fastSince !== null && now - fastSince >= config.fastAfterMs && index < rank(maxLevel)) {
    return { level: QUALITY_LEVELS[index + 1] ?? state.level, emaMs, slowSince: null, fastSince: null, lastSwitchAt: now };
  }
  return { ...state, emaMs, slowSince, fastSince };
}

/** Median of the observed frame intervals, used as the display refresh estimate. */
export function estimateRefreshMs(samples: readonly number[]): number {
  if (samples.length === 0) return ADAPTIVE_DEFAULTS.fastMs;
  const sorted = [...samples].sort((left, right) => left - right);
  return sorted[Math.floor(sorted.length / 2)] ?? ADAPTIVE_DEFAULTS.fastMs;
}

/**
 * On a 60 Hz display the frame interval never drops under 16.7 ms, so a fixed 12 ms "fast" bound could
 * never promote. "Fast" is therefore 20% over the measured refresh interval, kept within 12..20 ms.
 */
export function adaptiveConfigFor(refreshMs: number): AdaptiveConfig {
  const fastMs = Math.min(20, Math.max(ADAPTIVE_DEFAULTS.fastMs, refreshMs * 1.2));
  return { ...ADAPTIVE_DEFAULTS, fastMs };
}
