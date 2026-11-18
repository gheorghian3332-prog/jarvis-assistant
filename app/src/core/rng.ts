/** Deterministic pseudo-random helpers: the same seed always yields the same brain. */

/** Mulberry32: tiny seeded PRNG returning floats in [0, 1). */
export function mulberry32(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let mixed = state;
    mixed = Math.imul(mixed ^ (mixed >>> 15), mixed | 1);
    mixed ^= mixed + Math.imul(mixed ^ (mixed >>> 7), mixed | 61);
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

/** Integer lattice hash mapped to [0, 1). */
export function hash3(x: number, y: number, z: number, seed: number): number {
  let h = (Math.imul(x | 0, 374761393) + Math.imul(y | 0, 668265263) + Math.imul(z | 0, 2147483647)) | 0;
  h = (h ^ Math.imul(seed | 0, 1274126177)) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  h = Math.imul(h ^ (h >>> 16), 2246822519);
  return ((h ^ (h >>> 15)) >>> 0) / 4294967296;
}

const smooth = (t: number): number => t * t * (3 - 2 * t);

/** Smooth 3D value noise in [0, 1). */
export function valueNoise3(x: number, y: number, z: number, seed: number): number {
  const ix = Math.floor(x);
  const iy = Math.floor(y);
  const iz = Math.floor(z);
  const fx = smooth(x - ix);
  const fy = smooth(y - iy);
  const fz = smooth(z - iz);
  const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
  const corner = (dx: number, dy: number, dz: number): number => hash3(ix + dx, iy + dy, iz + dz, seed);
  const x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), fx);
  const x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), fx);
  const x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), fx);
  const x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), fx);
  return lerp(lerp(x00, x10, fy), lerp(x01, x11, fy), fz);
}
