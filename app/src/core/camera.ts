export const FOV_DEG = 40;
/** The brain's bounding sphere: centre (0, -0.12, 0), radius 1.15 (checked by a test over every neuron). */
export const BRAIN_CENTER_Y = -0.12;
export const BRAIN_RADIUS = 1.15;
/** Share of the smaller container side that the bounding sphere should fill. */
export const FILL = 0.75;
/** Three-quarter view: azimuth 35 degrees, elevation 15 degrees. */
export const START_AZIMUTH = (35 * Math.PI) / 180;
export const ELEVATION = (15 * Math.PI) / 180;

/** Camera distance at which a sphere of `radius` spans `fill` of min(width, height) in a perspective view. */
export function fitDistance(radius: number, fovDeg: number, aspect: number, fill = FILL): number {
  const safeAspect = Number.isFinite(aspect) && aspect > 0 ? aspect : 1;
  const tanHalf = Math.tan((fovDeg * Math.PI) / 360) * Math.min(1, safeAspect);
  return radius * Math.sqrt(1 + 1 / (fill * tanHalf) ** 2);
}
