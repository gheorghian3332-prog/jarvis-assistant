import type { AssistantState } from "../api";

export type FpsLimit = 30 | 45 | 60;
export const FPS_LIMITS: readonly FpsLimit[] = [30, 45, 60];

/** IDLE and OFFLINE never exceed 30 fps; the active states may use the whole `visual.fps_limit`. */
export function targetFps(state: AssistantState, limit: FpsLimit): number {
  return state === "IDLE" || state === "OFFLINE" ? Math.min(limit, 30) : limit;
}

/** A frame is allowed this much (ms) early, so vsync jitter does not skip a whole tick. */
const EARLY_TOLERANCE_MS = 4;

/** Pure throttle: is a new frame due `nowMs`, given the last rendered frame and the target fps? */
export function frameDue(nowMs: number, lastRenderMs: number | null, fps: number): boolean {
  if (lastRenderMs === null) return true;
  return nowMs - lastRenderMs >= 1000 / fps - EARLY_TOLERANCE_MS;
}

/** Frame intervals above this are pauses (hidden window, debugger), not slowness. */
export const MAX_TICK_GAP_MS = 250;

/** The browser services the loop depends on; injectable so the loop is testable. */
export interface LoopEnv {
  requestFrame(callback: (timeMs: number) => void): number;
  cancelFrame(handle: number): void;
  isHidden(): boolean;
  /** Subscribes to visibility changes; returns the unsubscribe function. */
  onVisibilityChange(callback: () => void): () => void;
}

export interface FrameLoopHandlers {
  getFps: () => number;
  /** Called on every display tick with the interval since the previous tick (never across a pause). */
  onTick?: (intervalMs: number, nowMs: number) => void;
  /** Called only on ticks that pass the FPS gate. */
  onRender: (nowMs: number) => void;
}

export interface FrameLoop {
  start(): void;
  stop(): void;
}

/**
 * Own requestAnimationFrame loop. It asks for a render no more often than `getFps()` allows and
 * stops completely (no scheduled frame) while the document is hidden.
 */
export function createFrameLoop(env: LoopEnv, handlers: FrameLoopHandlers): FrameLoop {
  let handle: number | null = null;
  let running = false;
  let lastTick: number | null = null;
  let lastRender: number | null = null;
  let unsubscribe: (() => void) | null = null;

  const schedule = (): void => {
    if (!running || handle !== null || env.isHidden()) return;
    handle = env.requestFrame(tick);
  };

  const tick = (nowMs: number): void => {
    handle = null;
    if (!running) return;
    if (env.isHidden()) return;
    if (lastTick !== null) {
      const interval = nowMs - lastTick;
      if (interval > 0 && interval <= MAX_TICK_GAP_MS) handlers.onTick?.(interval, nowMs);
    }
    lastTick = nowMs;
    if (frameDue(nowMs, lastRender, handlers.getFps())) {
      lastRender = nowMs;
      handlers.onRender(nowMs);
    }
    schedule();
  };

  const onVisibility = (): void => {
    if (env.isHidden()) {
      if (handle !== null) {
        env.cancelFrame(handle);
        handle = null;
      }
      lastTick = null;
      return;
    }
    lastTick = null;
    schedule();
  };

  return {
    start() {
      if (running) return;
      running = true;
      lastTick = null;
      lastRender = null;
      unsubscribe = env.onVisibilityChange(onVisibility);
      schedule();
    },
    stop() {
      running = false;
      if (handle !== null) {
        env.cancelFrame(handle);
        handle = null;
      }
      unsubscribe?.();
      unsubscribe = null;
    },
  };
}

/** The real browser environment. */
export function browserLoopEnv(): LoopEnv {
  return {
    requestFrame: (callback) => window.requestAnimationFrame(callback),
    cancelFrame: (handle) => window.cancelAnimationFrame(handle),
    isHidden: () => document.hidden,
    onVisibilityChange: (callback) => {
      document.addEventListener("visibilitychange", callback);
      return () => document.removeEventListener("visibilitychange", callback);
    },
  };
}
