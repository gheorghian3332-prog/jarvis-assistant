import type { AssistantState } from "../api";

export type Rgb = readonly [number, number, number];

/** Target look of the core for one assistant state. All values are smoothed towards by `approach`. */
export interface CoreParams {
  color: Rgb;
  intensity: number;
  /** Speed multiplier of the travelling impulses. */
  pulseRate: number;
  /** Share (0..1) of impulses that are visible. */
  pulseDensity: number;
  /** Uniform expansion of the structure. */
  expand: number;
  /** How strongly the voice level drives neuron size and brightness. */
  levelGain: number;
  /** Rotation of the group, rad/s. */
  rotation: number;
  /** Amplitude of the slow "breathing" expansion. */
  breath: number;
  throbHz: number;
  throbAmp: number;
  /** 0 = no wave, 1 = full wave. */
  waveAmt: number;
  /** 0 = radial wave from the centre, 1 = wave rising from the bottom. */
  waveDir: number;
  /** Wave cycles per second. */
  waveSpeed: number;
  /** Share of neurons that blink off at random. */
  flicker: number;
  /** Positional shake of the neurons (world units). */
  jitter: number;
}

export const STATES: readonly AssistantState[] = [
  "IDLE",
  "LISTENING",
  "PROCESSING",
  "SPEAKING",
  "EXECUTING",
  "WARNING",
  "ERROR",
  "OFFLINE",
];

const BASE: CoreParams = {
  color: [0.1, 0.78, 0.86],
  intensity: 0.8,
  pulseRate: 0.6,
  pulseDensity: 0.12,
  expand: 0,
  levelGain: 0,
  rotation: 0.12,
  breath: 0.02,
  throbHz: 0,
  throbAmp: 0,
  waveAmt: 0,
  waveDir: 0,
  waveSpeed: 0,
  flicker: 0,
  jitter: 0,
};

const TARGETS: Record<AssistantState, CoreParams> = {
  IDLE: BASE,
  LISTENING: {
    ...BASE,
    color: [0.22, 0.72, 1],
    intensity: 1.05,
    pulseRate: 1,
    pulseDensity: 0.38,
    expand: 0.045,
    levelGain: 1,
    rotation: 0.16,
    breath: 0.025,
  },
  PROCESSING: {
    ...BASE,
    color: [0.78, 0.92, 1],
    intensity: 0.95,
    pulseRate: 2.6,
    pulseDensity: 0.6,
    expand: 0.015,
    rotation: 0.34,
    waveAmt: 1,
    waveDir: 0,
    waveSpeed: 0.9,
  },
  SPEAKING: {
    ...BASE,
    color: [0.1, 0.9, 0.8],
    intensity: 1,
    pulseRate: 1.1,
    pulseDensity: 0.3,
    levelGain: 1,
    rotation: 0.15,
  },
  EXECUTING: {
    ...BASE,
    color: [0.14, 0.95, 0.55],
    intensity: 1.1,
    pulseRate: 1.8,
    pulseDensity: 0.55,
    rotation: 0.2,
    waveAmt: 1,
    waveDir: 1,
    waveSpeed: 0.7,
  },
  WARNING: {
    ...BASE,
    color: [1, 0.69, 0.15],
    intensity: 1,
    pulseRate: 0.8,
    pulseDensity: 0.2,
    rotation: 0.1,
    breath: 0.01,
    throbHz: 1,
    throbAmp: 0.45,
  },
  ERROR: {
    ...BASE,
    color: [1, 0.2, 0.2],
    intensity: 0.65,
    pulseRate: 0.5,
    pulseDensity: 0.1,
    rotation: 0.04,
    breath: 0,
    flicker: 0.35,
    jitter: 0.004,
  },
  OFFLINE: {
    ...BASE,
    color: [0.3, 0.4, 0.52],
    intensity: 0.32,
    pulseRate: 0,
    pulseDensity: 0,
    rotation: 0.02,
    breath: 0,
  },
};

/** Pure mapping state -> target parameters. Reduced motion removes rotation, waves, breathing and shake. */
export function paramsForState(state: AssistantState, reducedMotion: boolean): CoreParams {
  const target = TARGETS[state];
  if (!reducedMotion) return target;
  return { ...target, rotation: 0, breath: 0, waveAmt: 0, waveSpeed: 0, jitter: 0 };
}

/** Smoothing time constant of every parameter transition (about 400 ms). */
export const TRANSITION_TAU_S = 0.4;

const blend = (current: number, target: number, k: number): number => current + (target - current) * k;

/** One step of exponential approach; frame-rate independent. Returns a new object. */
export function approach(current: CoreParams, target: CoreParams, dtSeconds: number, tau = TRANSITION_TAU_S): CoreParams {
  const k = dtSeconds <= 0 ? 0 : 1 - Math.exp(-dtSeconds / tau);
  return {
    color: [
      blend(current.color[0], target.color[0], k),
      blend(current.color[1], target.color[1], k),
      blend(current.color[2], target.color[2], k),
    ],
    intensity: blend(current.intensity, target.intensity, k),
    pulseRate: blend(current.pulseRate, target.pulseRate, k),
    pulseDensity: blend(current.pulseDensity, target.pulseDensity, k),
    expand: blend(current.expand, target.expand, k),
    levelGain: blend(current.levelGain, target.levelGain, k),
    rotation: blend(current.rotation, target.rotation, k),
    breath: blend(current.breath, target.breath, k),
    throbHz: target.throbHz,
    throbAmp: blend(current.throbAmp, target.throbAmp, k),
    waveAmt: blend(current.waveAmt, target.waveAmt, k),
    waveDir: blend(current.waveDir, target.waveDir, k),
    waveSpeed: blend(current.waveSpeed, target.waveSpeed, k),
    flicker: blend(current.flicker, target.flicker, k),
    jitter: blend(current.jitter, target.jitter, k),
  };
}

/** Sharp flash right after entering ERROR that decays within a fraction of a second; 0 for every other state. */
export function errorFlash(state: AssistantState, sinceEnterSeconds: number): number {
  if (state !== "ERROR" || sinceEnterSeconds < 0) return 0;
  return Math.exp(-sinceEnterSeconds / 0.25);
}

export function isAssistantState(value: unknown): value is AssistantState {
  return typeof value === "string" && STATES.some((state) => state === value);
}
