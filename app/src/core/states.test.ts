import { describe, expect, it } from "vitest";
import { ATTACK_MS, RELEASE_MS, sanitizeLevel, smoothLevel } from "./level";
import { parsePreviewParams } from "./preview";
import { approach, errorFlash, isAssistantState, paramsForState, STATES, TRANSITION_TAU_S } from "./states";

describe("state mapping", () => {
  it("covers all eight states with sane ranges", () => {
    expect(STATES).toHaveLength(8);
    for (const state of STATES) {
      const params = paramsForState(state, false);
      expect(params.intensity).toBeGreaterThan(0);
      expect(params.pulseDensity).toBeGreaterThanOrEqual(0);
      expect(params.pulseDensity).toBeLessThanOrEqual(1);
      for (const channel of params.color) {
        expect(channel).toBeGreaterThanOrEqual(0);
        expect(channel).toBeLessThanOrEqual(1);
      }
    }
  });

  it("maps colours per the table", () => {
    const [ir, ig, ib] = paramsForState("IDLE", false).color;
    expect(ib).toBeGreaterThan(ir);
    expect(ig).toBeGreaterThan(ir);
    const [wr, wg] = paramsForState("WARNING", false).color;
    expect(wr).toBeGreaterThan(0.9);
    expect(wg).toBeGreaterThan(0.5);
    const [er, eg] = paramsForState("ERROR", false).color;
    expect(er).toBeGreaterThan(0.9);
    expect(eg).toBeLessThan(0.3);
    const [, xg, xb] = paramsForState("EXECUTING", false).color;
    expect(xg).toBeGreaterThan(xb);
    const [pr] = paramsForState("PROCESSING", false).color;
    expect(pr).toBeGreaterThan(ir);
  });

  it("orders behaviour as described", () => {
    const idle = paramsForState("IDLE", false);
    expect(paramsForState("LISTENING", false).pulseDensity).toBeGreaterThan(idle.pulseDensity);
    expect(paramsForState("LISTENING", false).expand).toBeGreaterThan(idle.expand);
    expect(paramsForState("PROCESSING", false).pulseRate).toBeGreaterThan(idle.pulseRate);
    expect(paramsForState("PROCESSING", false).waveDir).toBe(0);
    expect(paramsForState("EXECUTING", false).waveDir).toBe(1);
    expect(paramsForState("SPEAKING", false).levelGain).toBe(1);
    expect(idle.levelGain).toBe(0);
    expect(paramsForState("WARNING", false).throbHz).toBe(1);
    const offline = paramsForState("OFFLINE", false);
    expect(offline.pulseDensity).toBe(0);
    expect(offline.intensity).toBeLessThan(idle.intensity);
    expect(offline.rotation).toBeLessThan(idle.rotation);
  });

  it("removes rotation and waves under reduced motion", () => {
    for (const state of STATES) {
      const reduced = paramsForState(state, true);
      expect(reduced.rotation).toBe(0);
      expect(reduced.waveAmt).toBe(0);
      expect(reduced.breath).toBe(0);
      expect(reduced.jitter).toBe(0);
    }
    expect(paramsForState("PROCESSING", false).waveAmt).toBe(1);
  });

  it("recognises state names", () => {
    expect(isAssistantState("SPEAKING")).toBe(true);
    expect(isAssistantState("speaking")).toBe(false);
    expect(isAssistantState(null)).toBe(false);
  });
});

describe("parameter interpolation", () => {
  const from = paramsForState("IDLE", false);
  const to = paramsForState("ERROR", false);

  it("does not move for zero dt and reaches 63% after one time constant", () => {
    expect(approach(from, to, 0).intensity).toBe(from.intensity);
    const step = approach(from, to, TRANSITION_TAU_S);
    const fraction = (step.intensity - from.intensity) / (to.intensity - from.intensity);
    expect(fraction).toBeCloseTo(1 - Math.exp(-1), 6);
  });

  it("is frame-rate independent", () => {
    let fine = from;
    for (let index = 0; index < 40; index += 1) fine = approach(fine, to, 0.01);
    const coarse = approach(from, to, 0.4);
    expect(fine.color[0]).toBeCloseTo(coarse.color[0], 6);
    expect(fine.flicker).toBeCloseTo(coarse.flicker, 6);
  });

  it("converges to the target", () => {
    let value = from;
    for (let index = 0; index < 200; index += 1) value = approach(value, to, 0.05);
    expect(value.intensity).toBeCloseTo(to.intensity, 4);
    expect(value.color[1]).toBeCloseTo(to.color[1], 4);
  });
});

describe("error flash", () => {
  it("is a sharp decaying spike only in ERROR", () => {
    expect(errorFlash("ERROR", 0)).toBe(1);
    expect(errorFlash("ERROR", 0.25)).toBeCloseTo(Math.exp(-1), 6);
    expect(errorFlash("ERROR", 2)).toBeLessThan(0.001);
    expect(errorFlash("IDLE", 0)).toBe(0);
    expect(errorFlash("ERROR", -1)).toBe(0);
  });
});

describe("voice level smoothing", () => {
  it("attacks faster than it releases", () => {
    const up = smoothLevel(0, 1, ATTACK_MS);
    const down = 1 - smoothLevel(1, 0, ATTACK_MS);
    expect(up).toBeGreaterThan(down);
    expect(up).toBeCloseTo(1 - Math.exp(-1), 6);
    expect(smoothLevel(1, 0, RELEASE_MS)).toBeCloseTo(Math.exp(-1), 6);
  });

  it("holds for zero dt and settles", () => {
    expect(smoothLevel(0.4, 1, 0)).toBe(0.4);
    let level = 0;
    for (let index = 0; index < 100; index += 1) level = smoothLevel(level, 0.7, 16);
    expect(level).toBeCloseTo(0.7, 4);
  });

  it("sanitizes raw input to silence or 0..1", () => {
    expect(sanitizeLevel(0.5)).toBe(0.5);
    expect(sanitizeLevel(4)).toBe(1);
    expect(sanitizeLevel(-1)).toBe(0);
    expect(sanitizeLevel(Number.NaN)).toBe(0);
    expect(sanitizeLevel("0.5")).toBe(0);
    expect(sanitizeLevel(undefined)).toBe(0);
  });
});

describe("preview URL parameters", () => {
  it("reads state, level and debug", () => {
    expect(parsePreviewParams("?state=SPEAKING&level=0.6&debug=1")).toEqual({
      state: "SPEAKING",
      level: 0.6,
      debug: true,
      quality: null,
      settings: null,
    });
    expect(parsePreviewParams("?quality=high").quality).toBe("high");
    expect(parsePreviewParams("?state=speaking").state).toBe("SPEAKING");
    expect(parsePreviewParams("?settings=voice").settings).toBe("VOICE");
  });

  it("ignores garbage", () => {
    expect(parsePreviewParams("")).toEqual({ state: null, level: 0, debug: false, quality: null, settings: null });
    expect(parsePreviewParams("?state=NOPE&level=abc&debug=yes")).toEqual({ state: null, level: 0, debug: false, quality: null, settings: null });
    expect(parsePreviewParams("?level=9").level).toBe(1);
  });
});
