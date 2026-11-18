import { describe, expect, it } from "vitest";
import { BRAIN_CENTER_Y, BRAIN_RADIUS, FILL, FOV_DEG, fitDistance } from "./camera";
import { generateNeurons } from "./geometry";

describe("camera fit", () => {
  const projected = (distance: number, aspect: number): number =>
    BRAIN_RADIUS / (Math.sqrt(distance ** 2 - BRAIN_RADIUS ** 2) * Math.tan((FOV_DEG * Math.PI) / 360) * Math.min(1, aspect));

  it("makes the sphere fill 75% of the smaller side for any aspect", () => {
    for (const aspect of [0.5, 1, 1.6, 3]) {
      expect(projected(fitDistance(BRAIN_RADIUS, FOV_DEG, aspect), aspect)).toBeCloseTo(FILL, 6);
    }
  });

  it("moves back for narrow containers and survives bad input", () => {
    expect(fitDistance(BRAIN_RADIUS, FOV_DEG, 0.5)).toBeGreaterThan(fitDistance(BRAIN_RADIUS, FOV_DEG, 1.5));
    expect(fitDistance(BRAIN_RADIUS, FOV_DEG, 0)).toBe(fitDistance(BRAIN_RADIUS, FOV_DEG, 1));
  });

  it("every neuron lies inside the bounding sphere", () => {
    const { positions } = generateNeurons(4000, 99);
    for (let index = 0; index < 4000; index += 1) {
      const distance = Math.hypot(positions[index * 3] ?? 0, (positions[index * 3 + 1] ?? 0) - BRAIN_CENTER_Y, positions[index * 3 + 2] ?? 0);
      expect(distance).toBeLessThanOrEqual(BRAIN_RADIUS);
    }
  });
});
