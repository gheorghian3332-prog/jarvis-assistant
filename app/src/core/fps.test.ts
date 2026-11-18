import { describe, expect, it } from "vitest";
import { createFrameLoop, frameDue, MAX_TICK_GAP_MS, targetFps, type LoopEnv } from "./fps";

describe("target fps", () => {
  it("caps IDLE and OFFLINE at 30", () => {
    expect(targetFps("IDLE", 60)).toBe(30);
    expect(targetFps("OFFLINE", 45)).toBe(30);
    expect(targetFps("IDLE", 30)).toBe(30);
  });

  it("lets active states use the whole limit", () => {
    for (const state of ["LISTENING", "SPEAKING", "PROCESSING", "EXECUTING", "WARNING", "ERROR"] as const) {
      expect(targetFps(state, 60)).toBe(60);
      expect(targetFps(state, 45)).toBe(45);
      expect(targetFps(state, 30)).toBe(30);
    }
  });
});

describe("frame gate", () => {
  it("renders the first frame, then throttles", () => {
    expect(frameDue(0, null, 30)).toBe(true);
    expect(frameDue(16.7, 0, 30)).toBe(false);
    expect(frameDue(33.3, 0, 30)).toBe(true);
  });

  it("yields about the target rate on 60 Hz ticks", () => {
    for (const [fps, expected] of [
      [30, 30],
      [60, 60],
    ] as const) {
      let last: number | null = null;
      let rendered = 0;
      for (let tick = 0; tick < 60; tick += 1) {
        const now = tick * (1000 / 60);
        if (frameDue(now, last, fps)) {
          last = now;
          rendered += 1;
        }
      }
      expect(Math.abs(rendered - expected)).toBeLessThanOrEqual(1);
    }
  });

  it("tolerates vsync jitter at 30 fps", () => {
    let last: number | null = null;
    let rendered = 0;
    for (let tick = 0; tick < 120; tick += 1) {
      const jitter = tick % 2 === 0 ? -1.5 : 1.5;
      const now = tick * (1000 / 60) + jitter;
      if (frameDue(now, last, 30)) {
        last = now;
        rendered += 1;
      }
    }
    expect(rendered).toBeGreaterThanOrEqual(58);
    expect(rendered).toBeLessThanOrEqual(61);
  });
});

class FakeEnv implements LoopEnv {
  hidden = false;
  pending = new Map<number, (time: number) => void>();
  listeners = new Set<() => void>();
  next = 1;
  cancelled = 0;

  requestFrame(callback: (time: number) => void): number {
    const handle = this.next;
    this.next += 1;
    this.pending.set(handle, callback);
    return handle;
  }

  cancelFrame(handle: number): void {
    this.cancelled += 1;
    this.pending.delete(handle);
  }

  isHidden(): boolean {
    return this.hidden;
  }

  onVisibilityChange(callback: () => void): () => void {
    this.listeners.add(callback);
    return () => this.listeners.delete(callback);
  }

  /** Delivers the single pending frame at `time`. */
  fire(time: number): void {
    const entry = this.pending.entries().next();
    if (entry.done) throw new Error("no frame scheduled");
    const [handle, callback] = entry.value;
    this.pending.delete(handle);
    callback(time);
  }

  setHidden(hidden: boolean): void {
    this.hidden = hidden;
    for (const listener of this.listeners) listener();
  }
}

describe("frame loop", () => {
  it("renders at the target rate and ticks on every frame", () => {
    const env = new FakeEnv();
    let renders = 0;
    let ticks = 0;
    const loop = createFrameLoop(env, { getFps: () => 30, onRender: () => (renders += 1), onTick: () => (ticks += 1) });
    loop.start();
    for (let index = 0; index < 60; index += 1) env.fire(index * (1000 / 60));
    expect(Math.abs(renders - 30)).toBeLessThanOrEqual(1);
    expect(ticks).toBe(59);
    loop.stop();
  });

  it("follows a changing fps target", () => {
    const env = new FakeEnv();
    let fps = 30;
    let renders = 0;
    const loop = createFrameLoop(env, { getFps: () => fps, onRender: () => (renders += 1) });
    loop.start();
    for (let index = 0; index < 60; index += 1) env.fire(index * (1000 / 60));
    const slow = renders;
    fps = 60;
    for (let index = 60; index < 120; index += 1) env.fire(index * (1000 / 60));
    expect(renders - slow).toBeGreaterThanOrEqual(58);
  });

  it("stops completely while hidden and resumes when visible", () => {
    const env = new FakeEnv();
    let renders = 0;
    let intervals: number[] = [];
    const loop = createFrameLoop(env, {
      getFps: () => 60,
      onRender: () => (renders += 1),
      onTick: (interval) => intervals.push(interval),
    });
    loop.start();
    env.fire(0);
    env.fire(16);
    expect(env.pending.size).toBe(1);
    env.setHidden(true);
    expect(env.pending.size).toBe(0);
    expect(env.cancelled).toBe(1);
    const before = renders;
    env.setHidden(false);
    expect(env.pending.size).toBe(1);
    intervals = [];
    env.fire(60000);
    env.fire(60016);
    expect(renders).toBe(before + 2);
    // The pause is never reported as a slow frame.
    expect(intervals).toEqual([16]);
    loop.stop();
    expect(env.pending.size).toBe(0);
    expect(env.listeners.size).toBe(0);
  });

  it("does not schedule when started hidden", () => {
    const env = new FakeEnv();
    env.hidden = true;
    const loop = createFrameLoop(env, { getFps: () => 30, onRender: () => undefined });
    loop.start();
    expect(env.pending.size).toBe(0);
    env.setHidden(false);
    expect(env.pending.size).toBe(1);
    loop.stop();
  });

  it("ignores intervals longer than the pause threshold", () => {
    const env = new FakeEnv();
    const intervals: number[] = [];
    const loop = createFrameLoop(env, {
      getFps: () => 30,
      onRender: () => undefined,
      onTick: (interval) => intervals.push(interval),
    });
    loop.start();
    env.fire(0);
    env.fire(16);
    env.fire(16 + MAX_TICK_GAP_MS + 1);
    env.fire(16 + MAX_TICK_GAP_MS + 17);
    expect(intervals).toEqual([16, 16]);
    loop.stop();
  });
});
