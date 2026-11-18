import { describe, expect, it } from "vitest";
import {
  BRAIN_BOUNDS,
  buildEdges,
  edgeLineHubs,
  edgeLinePositions,
  expandPulseTails,
  generateNeurons,
  generatePulses,
  linkRadius,
  pulseCountFor,
} from "./geometry";
import { getModelData } from "./model";
import { QUALITY_SPECS } from "./quality";
import { hash3, mulberry32, valueNoise3 } from "./rng";

describe("rng", () => {
  it("is deterministic and in range", () => {
    const first = mulberry32(7);
    const second = mulberry32(7);
    for (let index = 0; index < 100; index += 1) {
      const value = first();
      expect(value).toBe(second());
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThan(1);
    }
    expect(mulberry32(1)()).not.toBe(mulberry32(2)());
  });

  it("noise is stable and bounded", () => {
    expect(hash3(1, 2, 3, 4)).toBe(hash3(1, 2, 3, 4));
    for (let index = 0; index < 50; index += 1) {
      const value = valueNoise3(index * 0.37, index * 0.11, index * 0.73, 5);
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThanOrEqual(1);
    }
  });
});

describe("neuron generation", () => {
  it("is deterministic for the same seed and differs for another", () => {
    const first = generateNeurons(500, 42);
    const second = generateNeurons(500, 42);
    expect(Array.from(first.positions)).toEqual(Array.from(second.positions));
    expect(Array.from(first.sizes)).toEqual(Array.from(second.sizes));
    expect(Array.from(generateNeurons(500, 43).positions)).not.toEqual(Array.from(first.positions));
  });

  it("keeps every neuron inside the bounding box", () => {
    for (const count of [300, 1500, 4000]) {
      const { positions, sizes, phases, brights } = generateNeurons(count, 99);
      expect(positions.length).toBe(count * 3);
      expect(sizes.length).toBe(count);
      expect(phases.length).toBe(count);
      expect(brights.length).toBe(count);
      for (let index = 0; index < count; index += 1) {
        expect(Math.abs(positions[index * 3] ?? 0)).toBeLessThanOrEqual(BRAIN_BOUNDS.x);
        expect(positions[index * 3 + 1]).toBeGreaterThanOrEqual(BRAIN_BOUNDS.yMin);
        expect(positions[index * 3 + 1]).toBeLessThanOrEqual(BRAIN_BOUNDS.yMax);
        expect(Math.abs(positions[index * 3 + 2] ?? 0)).toBeLessThanOrEqual(BRAIN_BOUNDS.z);
      }
    }
  });

  it("leaves a gap between the hemispheres and keeps a stem below", () => {
    const { positions } = generateNeurons(4000, 1);
    let left = 0;
    let right = 0;
    let stem = 0;
    for (let index = 0; index < 4000; index += 1) {
      const x = positions[index * 3] ?? 0;
      const y = positions[index * 3 + 1] ?? 0;
      // The stem and cerebellum (below y = -0.37) sit on the midline; everything above belongs to a hemisphere.
      if (y > -0.3) expect(Math.abs(x)).toBeGreaterThanOrEqual(0.03 - 1e-6);
      if (x < 0) left += 1;
      else right += 1;
      if (y < -0.7) stem += 1;
    }
    expect(left).toBeGreaterThan(1500);
    expect(right).toBeGreaterThan(1500);
    expect(stem).toBeGreaterThan(30);
  });

  it("is denser in the cortex shell than in the core", () => {
    const { positions } = generateNeurons(4000, 5);
    let outer = 0;
    let inner = 0;
    for (let index = 0; index < 4000; index += 1) {
      const side = (positions[index * 3] ?? 0) < 0 ? -1 : 1;
      const y = positions[index * 3 + 1] ?? 0;
      if (y < -0.3) continue;
      const nx = ((positions[index * 3] ?? 0) - side * 0.54) / 0.5;
      const ny = (y - 0.08) / 0.6;
      const nz = (positions[index * 3 + 2] ?? 0) / 0.85;
      const radius = Math.hypot(nx, ny, nz);
      if (radius > 0.9) outer += 1;
      else if (radius < 0.7) inner += 1;
    }
    expect(outer).toBeGreaterThan(inner);
  });

  it("has about 3% larger hub neurons, a cerebellum behind and a stem leaning backward", () => {
    const { positions, sizes, hubs } = generateNeurons(4000, 21);
    let hubCount = 0;
    let cerebellum = 0;
    let stemBack = 0;
    for (let index = 0; index < 4000; index += 1) {
      if (hubs[index] === 1) {
        hubCount += 1;
        expect(sizes[index]).toBeGreaterThanOrEqual(3);
      } else {
        expect(sizes[index]).toBeLessThan(2.3);
      }
      const x = positions[index * 3] ?? 0;
      const y = positions[index * 3 + 1] ?? 0;
      const z = positions[index * 3 + 2] ?? 0;
      if (Math.abs(x) < 0.4 && y < -0.4 && y > -0.75 && z < -0.45) cerebellum += 1;
      if (y < -0.9) stemBack += z < -0.3 ? 1 : 0;
    }
    expect(hubCount).toBeGreaterThan(80);
    expect(hubCount).toBeLessThan(160);
    expect(cerebellum).toBeGreaterThan(150);
    expect(stemBack).toBeGreaterThan(0);
  });

  it("rejects invalid counts", () => {
    expect(() => generateNeurons(0, 1)).toThrow(RangeError);
    expect(() => generateNeurons(1.5, 1)).toThrow(RangeError);
  });
});

describe("nearest-neighbour edges", () => {
  const count = 1500;
  const { positions } = generateNeurons(count, 11);
  const radius = linkRadius(count);
  const edges = buildEdges(positions, 3, radius);

  it("has no self loops, no duplicates and ordered pairs", () => {
    expect(edges.length % 2).toBe(0);
    expect(edges.length).toBeGreaterThan(count);
    const seen = new Set<number>();
    for (let slot = 0; slot < edges.length; slot += 2) {
      const a = edges[slot] ?? 0;
      const b = edges[slot + 1] ?? 0;
      expect(a).not.toBe(b);
      expect(a).toBeLessThan(b);
      expect(b).toBeLessThan(count);
      const key = a * count + b;
      expect(seen.has(key)).toBe(false);
      seen.add(key);
    }
  });

  it("only links neurons within the radius", () => {
    for (let slot = 0; slot < edges.length; slot += 2) {
      const a = edges[slot] ?? 0;
      const b = edges[slot + 1] ?? 0;
      const distance = Math.hypot(
        (positions[a * 3] ?? 0) - (positions[b * 3] ?? 0),
        (positions[a * 3 + 1] ?? 0) - (positions[b * 3 + 1] ?? 0),
        (positions[a * 3 + 2] ?? 0) - (positions[b * 3 + 2] ?? 0),
      );
      expect(distance).toBeLessThanOrEqual(radius + 1e-6);
    }
  });

  it("matches a brute-force k-nearest search on a small cloud", () => {
    const small = generateNeurons(120, 3).positions;
    const limit = linkRadius(120);
    const fast = buildEdges(small, 3, limit);
    const expected = new Set<number>();
    for (let index = 0; index < 120; index += 1) {
      const candidates: Array<{ other: number; distance: number }> = [];
      for (let other = 0; other < 120; other += 1) {
        if (other === index) continue;
        const distance = Math.hypot(
          (small[index * 3] ?? 0) - (small[other * 3] ?? 0),
          (small[index * 3 + 1] ?? 0) - (small[other * 3 + 1] ?? 0),
          (small[index * 3 + 2] ?? 0) - (small[other * 3 + 2] ?? 0),
        );
        if (distance <= limit) candidates.push({ other, distance });
      }
      candidates.sort((left, right) => left.distance - right.distance || left.other - right.other);
      for (const { other } of candidates.slice(0, 3)) {
        expected.add(Math.min(index, other) * 120 + Math.max(index, other));
      }
    }
    const actual = new Set<number>();
    for (let slot = 0; slot < fast.length; slot += 2) actual.add((fast[slot] ?? 0) * 120 + (fast[slot + 1] ?? 0));
    expect(actual).toEqual(expected);
  });

  it("is deterministic", () => {
    expect(Array.from(buildEdges(positions, 3, radius))).toEqual(Array.from(edges));
  });

  it("validates its arguments", () => {
    expect(() => buildEdges(positions, 3, 0)).toThrow(RangeError);
    expect(() => buildEdges(positions, 0, 0.1)).toThrow(RangeError);
    expect(buildEdges(new Float32Array(0), 3, 0.1).length).toBe(0);
  });

  it("flags hub endpoints per vertex", () => {
    const hubs = new Float32Array(count);
    const target = edges[0] ?? 0;
    hubs[target] = 1;
    const flags = edgeLineHubs(hubs, edges);
    expect(flags.length).toBe(edges.length);
    expect(flags[0]).toBe(1);
    for (let slot = 0; slot < edges.length; slot += 1) expect(flags[slot]).toBe(edges[slot] === target ? 1 : 0);
  });

  it("expands to two vertices per edge", () => {
    const lines = edgeLinePositions(positions, edges);
    expect(lines.length).toBe(edges.length * 3);
    const first = edges[0] ?? 0;
    expect(lines[0]).toBe(positions[first * 3]);
  });
});

describe("pulses", () => {
  const { positions } = generateNeurons(300, 8);
  const edges = buildEdges(positions, 3, linkRadius(300));

  it("scales with the edge count and respects the cap", () => {
    expect(pulseCountFor(1000, 5000)).toBe(300);
    expect(pulseCountFor(1000, 80)).toBe(80);
    expect(pulseCountFor(0, 80)).toBe(0);
  });

  it("travel along existing edges with valid attributes", () => {
    const count = pulseCountFor(edges.length / 2, QUALITY_SPECS.low.pulseCap);
    const pulses = generatePulses(positions, edges, count, 5);
    expect(pulses.count).toBe(count);
    const pairs = new Set<string>();
    for (let slot = 0; slot < edges.length; slot += 2) {
      pairs.add(`${edges[slot]}-${edges[slot + 1]}`);
    }
    for (let index = 0; index < pulses.count; index += 1) {
      expect(pulses.offsets[index]).toBeGreaterThanOrEqual(0);
      expect(pulses.offsets[index]).toBeLessThan(1);
      expect(pulses.speeds[index]).toBeGreaterThan(0);
    }
    expect(Array.from(generatePulses(positions, edges, count, 5).starts)).toEqual(Array.from(pulses.starts));
    expect(generatePulses(positions, new Uint32Array(0), 10, 1).count).toBe(0);
  });
});

describe("pulse tails", () => {
  it("repeats each pulse head-first with the same path", () => {
    const { positions } = generateNeurons(300, 8);
    const edges = buildEdges(positions, 3, linkRadius(300));
    const pulses = generatePulses(positions, edges, 10, 5);
    const tails = expandPulseTails(pulses, 3);
    expect(tails.count).toBe(30);
    expect(Array.from(tails.tails.slice(0, 6))).toEqual([0, 1, 2, 0, 1, 2]);
    expect(tails.offsets[4]).toBe(pulses.offsets[1]);
    expect(tails.ends[(7 * 3 + 1) * 3 + 1]).toBe(pulses.ends[7 * 3 + 1]);
  });
});

describe("model data per quality level", () => {
  it("matches the quality specs and is cached", () => {
    for (const level of ["low", "medium", "high"] as const) {
      const data = getModelData(level);
      expect(data.neurons.count).toBe(QUALITY_SPECS[level].neurons);
      expect(data.pulses.count).toBeLessThanOrEqual(QUALITY_SPECS[level].pulseCap);
      expect(data.pulses.count).toBeLessThanOrEqual(Math.round(data.edgeCount * 0.3));
      expect(data.linePositions.length).toBe(data.edgeCount * 2 * 3);
      expect(data.lineHubs.length).toBe(data.edgeCount * 2);
      expect(data.pulseTails.count).toBe(data.pulses.count * 3);
      expect(getModelData(level)).toBe(data);
    }
  });
});
