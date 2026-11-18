import { describe, expect, it } from "vitest";
import {
  ADAPTIVE_DEFAULTS,
  adaptiveConfigFor,
  capForMode,
  createAdaptiveState,
  dprFor,
  estimateRefreshMs,
  maxLevelFor,
  powerPreferenceFor,
  QUALITY_SPECS,
  resolveQuality,
  stepAdaptive,
  type AdaptiveState,
  type QualityLevel,
} from "./quality";

/** Feeds a constant frame time at 60 Hz ticks and returns the state plus every level change. */
function run(
  start: AdaptiveState,
  frameMs: number,
  durationMs: number,
  maxLevel: QualityLevel = "high",
  from = start.lastSwitchAt,
): { state: AdaptiveState; switches: Array<{ at: number; level: QualityLevel }> } {
  let state = start;
  const switches: Array<{ at: number; level: QualityLevel }> = [];
  for (let now = from; now <= from + durationMs; now += 16) {
    const next = stepAdaptive(state, frameMs, now, maxLevel);
    if (next.level !== state.level) switches.push({ at: now - from, level: next.level });
    state = next;
  }
  return { state, switches };
}

describe("quality specs", () => {
  it("match the architect's table", () => {
    expect(QUALITY_SPECS.low.neurons).toBe(300);
    expect(QUALITY_SPECS.medium.neurons).toBe(1500);
    expect(QUALITY_SPECS.high.neurons).toBe(4000);
  });

  it("caps the pixel ratio per level", () => {
    expect(dprFor("low", 2)).toBe(0.75);
    expect(dprFor("medium", 2)).toBe(1);
    expect(dprFor("high", 2)).toBe(1.5);
    expect(dprFor("high", 1.25)).toBe(1.25);
    expect(dprFor("high", Number.NaN)).toBe(1);
  });

  it("limits the compact window to medium", () => {
    expect(capForMode("high", true)).toBe("medium");
    expect(capForMode("low", true)).toBe("low");
    expect(capForMode("high", false)).toBe("high");
    expect(maxLevelFor(true)).toBe("medium");
    expect(resolveQuality("high", "low", true)).toBe("medium");
    expect(resolveQuality("auto", "high", true)).toBe("medium");
    expect(resolveQuality("auto", "low", false)).toBe("low");
    expect(resolveQuality("low", "high", false)).toBe("low");
  });

  it("asks for low power only below the high setting", () => {
    expect(powerPreferenceFor("low")).toBe("low-power");
    expect(powerPreferenceFor("medium")).toBe("low-power");
    expect(powerPreferenceFor("auto")).toBe("low-power");
    expect(powerPreferenceFor("high")).toBe("default");
  });
});

describe("adaptive controller", () => {
  it("steps down after 3 s of slow frames, not before", () => {
    const { switches } = run(createAdaptiveState("high", 0), 40, 6000);
    expect(switches[0]?.level).toBe("medium");
    expect(switches[0]?.at ?? 0).toBeGreaterThanOrEqual(5000 - 16);
  });

  it("never switches twice within the 5 s cooldown", () => {
    const { switches } = run(createAdaptiveState("high", 0), 60, 30000);
    expect(switches.length).toBe(2);
    expect(switches.map((entry) => entry.level)).toEqual(["medium", "low"]);
    const gap = (switches[1]?.at ?? 0) - (switches[0]?.at ?? 0);
    expect(gap).toBeGreaterThanOrEqual(ADAPTIVE_DEFAULTS.cooldownMs);
  });

  it("does not go below low", () => {
    const { state } = run(createAdaptiveState("low", 0), 80, 30000);
    expect(state.level).toBe("low");
  });

  it("steps up after 10 s of fast frames but never above high", () => {
    const early = run(createAdaptiveState("low", 0), 8, 9000);
    expect(early.state.level).toBe("low");
    const later = run(createAdaptiveState("low", 0), 8, 40000);
    expect(later.state.level).toBe("high");
    expect(later.switches.map((entry) => entry.level)).toEqual(["medium", "high"]);
  });

  it("respects a lower ceiling (compact window)", () => {
    const { state } = run(createAdaptiveState("low", 0), 8, 40000, "medium");
    expect(state.level).toBe("medium");
  });

  it("clamps immediately when the ceiling drops", () => {
    const next = stepAdaptive(createAdaptiveState("high", 0), 16, 100, "medium");
    expect(next.level).toBe("medium");
  });

  it("holds in the hysteresis band between the thresholds", () => {
    const { switches, state } = run(createAdaptiveState("medium", 0), 18, 60000);
    expect(switches).toEqual([]);
    expect(state.level).toBe("medium");
  });

  it("a brief spike does not trigger a switch", () => {
    let state = createAdaptiveState("high", 0);
    let now = 0;
    for (; now < 20000; now += 16) {
      const frame = now >= 4000 && now < 4600 ? 70 : 16;
      state = stepAdaptive(state, frame, now, "high");
    }
    expect(state.level).toBe("high");
  });

  it("slow-then-fast resets the timers", () => {
    let state = createAdaptiveState("high", 0);
    let now = 0;
    for (; now < 2500; now += 16) state = stepAdaptive(state, 60, now, "high");
    for (; now < 5000; now += 16) state = stepAdaptive(state, 8, now, "high");
    expect(state.level).toBe("high");
  });
});

describe("refresh-aware thresholds", () => {
  it("uses the median interval and keeps the fast bound within 12..20 ms", () => {
    expect(estimateRefreshMs([16, 17, 16.7, 40, 16.6])).toBeCloseTo(16.7, 1);
    expect(estimateRefreshMs([])).toBe(ADAPTIVE_DEFAULTS.fastMs);
    expect(adaptiveConfigFor(16.7).fastMs).toBeCloseTo(20, 5);
    expect(adaptiveConfigFor(6.9).fastMs).toBe(12);
    expect(adaptiveConfigFor(33).fastMs).toBe(20);
    expect(adaptiveConfigFor(16.7).slowMs).toBe(25);
  });

  it("lets a 60 Hz display promote the quality", () => {
    const config = adaptiveConfigFor(16.7);
    let state = createAdaptiveState("low", 0);
    for (let now = 0; now <= 30000; now += 17) state = stepAdaptive(state, 16.7, now, "high", config);
    expect(state.level).toBe("high");
  });
});
