import { mulberry32, valueNoise3 } from "./rng";

/** Axis-aligned box that contains every generated neuron (y is up, z is front-to-back). */
export const BRAIN_BOUNDS = {
  x: 1.1,
  yMin: -1.05,
  yMax: 0.76,
  z: 0.95,
} as const;

/** Hemisphere ellipsoid: half-extents and centre. The two hemispheres mirror around x = 0. */
const HEMISPHERE = { a: 0.5, b: 0.6, c: 0.85, cx: 0.54, cy: 0.08, cz: 0 } as const;
const MIN_ABS_X = 0.03;
const STEM_SHARE = 0.03;
const CEREBELLUM_SHARE = 0.08;
const HUB_SHARE = 0.03;
const SHELL_SHARE = 0.62;
/** Inner boundary of the cortex shell relative to the surface. */
const SHELL_INNER = 0.84;

export interface NeuronData {
  count: number;
  positions: Float32Array;
  sizes: Float32Array;
  phases: Float32Array;
  brights: Float32Array;
  /** 1 for the ~3% larger "hub" neurons, 0 otherwise. */
  hubs: Float32Array;
}

export interface PulseData {
  count: number;
  starts: Float32Array;
  ends: Float32Array;
  offsets: Float32Array;
  speeds: Float32Array;
}

export interface PulseTailData extends PulseData {
  /** 0 for the head of a pulse, 1.. for its trailing points. */
  tails: Float32Array;
}

function unitDirection(random: () => number): [number, number, number] {
  for (;;) {
    const x = random() * 2 - 1;
    const y = random() * 2 - 1;
    const z = random() * 2 - 1;
    const length = Math.hypot(x, y, z);
    if (length > 1e-3 && length <= 1) return [x / length, y / length, z / length];
  }
}

const clamp = (value: number, low: number, high: number): number => Math.min(high, Math.max(low, value));

/** Two hemispheres with a thicker, denser cortex shell, gyri noise on the surface and a small stem. */
export function generateNeurons(count: number, seed: number): NeuronData {
  if (!Number.isInteger(count) || count <= 0) throw new RangeError("count must be a positive integer");
  const random = mulberry32(seed);
  const positions = new Float32Array(count * 3);
  const sizes = new Float32Array(count);
  const phases = new Float32Array(count);
  const brights = new Float32Array(count);
  const hubs = new Float32Array(count);
  const stemCount = Math.round(count * STEM_SHARE);
  const cerebellumCount = Math.round(count * CEREBELLUM_SHARE);
  const brainCount = count - stemCount - cerebellumCount;

  for (let index = 0; index < count; index += 1) {
    let x: number;
    let y: number;
    let z: number;
    let bright: number;
    if (index < brainCount) {
      const side = random() < 0.5 ? -1 : 1;
      const [dx, dy, dz] = unitDirection(random);
      const shell = random() < SHELL_SHARE;
      const radius = shell ? SHELL_INNER + (1 - SHELL_INNER) * random() : SHELL_INNER * Math.cbrt(random());
      const folds =
        1 +
        0.14 * (valueNoise3(dx * 2.6 + side * 7.3, dy * 2.6, dz * 2.6, seed) - 0.5) +
        0.08 * (valueNoise3(dx * 5.4 + side * 3.1, dy * 5.4, dz * 5.4, seed + 1) - 0.5);
      const reach = radius * folds;
      x = side * Math.max(side * (HEMISPHERE.cx * side + HEMISPHERE.a * reach * dx), MIN_ABS_X);
      y = HEMISPHERE.cy + HEMISPHERE.b * reach * dy;
      z = HEMISPHERE.cz + HEMISPHERE.c * reach * dz;
      bright = shell ? 0.75 + 0.25 * random() : 0.45 + 0.3 * random();
    } else if (index < brainCount + cerebellumCount) {
      const [dx, dy, dz] = unitDirection(random);
      const radius = 0.6 + 0.4 * random();
      const folds = 1 + 0.18 * (valueNoise3(dx * 4.2, dy * 4.2, dz * 4.2, seed + 2) - 0.5);
      x = 0.38 * radius * folds * dx;
      y = -0.56 + 0.17 * radius * folds * dy;
      z = -0.64 + 0.24 * radius * folds * dz;
      bright = 0.55 + 0.3 * random();
    } else {
      // Short, thin stem that leans backward and down.
      const height = random();
      const spread = 0.085 * (1 - 0.4 * height) * Math.sqrt(random());
      const angle = random() * Math.PI * 2;
      x = spread * Math.cos(angle);
      y = -0.4 - 0.57 * height;
      z = -0.18 - 0.32 * height + spread * Math.sin(angle);
      bright = 0.4 + 0.3 * random();
    }
    positions[index * 3] = clamp(x, -BRAIN_BOUNDS.x, BRAIN_BOUNDS.x);
    positions[index * 3 + 1] = clamp(y, BRAIN_BOUNDS.yMin, BRAIN_BOUNDS.yMax);
    positions[index * 3 + 2] = clamp(z, -BRAIN_BOUNDS.z, BRAIN_BOUNDS.z);
    const hub = index < brainCount + cerebellumCount && random() < HUB_SHARE;
    hubs[index] = hub ? 1 : 0;
    sizes[index] = hub ? 3 + 1.2 * random() : 0.5 + 1.7 * random() ** 2.2;
    phases[index] = random() * Math.PI * 2;
    brights[index] = hub ? 1 : bright;
  }
  return { count, positions, sizes, phases, brights, hubs };
}

/** Link radius that keeps roughly three neighbours per neuron for a given neuron count. */
export function linkRadius(count: number): number {
  const volume = 2 * ((4 / 3) * Math.PI * HEMISPHERE.a * HEMISPHERE.b * HEMISPHERE.c);
  return 2.4 * Math.cbrt(volume / count);
}

const GRID_OFFSET = 512;
const GRID_SPAN = 1024;

/**
 * k nearest neighbours within `radius`, found through a uniform grid (O(n) for bounded density).
 * Returns flat index pairs `[a0, b0, a1, b1, ...]` with `a < b`, without duplicates or self loops.
 */
export function buildEdges(positions: Float32Array, k: number, radius: number): Uint32Array {
  if (!(radius > 0)) throw new RangeError("radius must be positive");
  if (!Number.isInteger(k) || k <= 0) throw new RangeError("k must be a positive integer");
  const count = Math.floor(positions.length / 3);
  const radiusSquared = radius * radius;
  const cellOf = (value: number): number => {
    const cell = Math.floor(value / radius);
    if (Math.abs(cell) >= GRID_OFFSET) throw new RangeError("radius too small for the point cloud");
    return cell + GRID_OFFSET;
  };
  const keyOf = (cx: number, cy: number, cz: number): number => (cx * GRID_SPAN + cy) * GRID_SPAN + cz;

  const grid = new Map<number, number[]>();
  for (let index = 0; index < count; index += 1) {
    const key = keyOf(
      cellOf(positions[index * 3] ?? 0),
      cellOf(positions[index * 3 + 1] ?? 0),
      cellOf(positions[index * 3 + 2] ?? 0),
    );
    const bucket = grid.get(key);
    if (bucket) bucket.push(index);
    else grid.set(key, [index]);
  }

  const seen = new Set<number>();
  const pairs: number[] = [];
  const bestIndex = new Array<number>(k);
  const bestDistance = new Array<number>(k);
  for (let index = 0; index < count; index += 1) {
    const px = positions[index * 3] ?? 0;
    const py = positions[index * 3 + 1] ?? 0;
    const pz = positions[index * 3 + 2] ?? 0;
    const cx = cellOf(px);
    const cy = cellOf(py);
    const cz = cellOf(pz);
    let found = 0;
    for (let ox = -1; ox <= 1; ox += 1) {
      for (let oy = -1; oy <= 1; oy += 1) {
        for (let oz = -1; oz <= 1; oz += 1) {
          const bucket = grid.get(keyOf(cx + ox, cy + oy, cz + oz));
          if (!bucket) continue;
          for (const other of bucket) {
            if (other === index) continue;
            const dx = (positions[other * 3] ?? 0) - px;
            const dy = (positions[other * 3 + 1] ?? 0) - py;
            const dz = (positions[other * 3 + 2] ?? 0) - pz;
            const distance = dx * dx + dy * dy + dz * dz;
            if (distance > radiusSquared) continue;
            // Insertion into the sorted top-k list; ties break by lower index for determinism.
            let slot = found < k ? found : k;
            while (slot > 0) {
              const previousDistance = bestDistance[slot - 1] ?? 0;
              const previousIndex = bestIndex[slot - 1] ?? 0;
              if (previousDistance < distance || (previousDistance === distance && previousIndex < other)) break;
              slot -= 1;
            }
            if (slot >= k) continue;
            const last = Math.min(found, k - 1);
            for (let move = last; move > slot; move -= 1) {
              bestDistance[move] = bestDistance[move - 1] ?? 0;
              bestIndex[move] = bestIndex[move - 1] ?? 0;
            }
            bestDistance[slot] = distance;
            bestIndex[slot] = other;
            if (found < k) found += 1;
          }
        }
      }
    }
    for (let rank = 0; rank < found; rank += 1) {
      const other = bestIndex[rank] ?? 0;
      const low = Math.min(index, other);
      const high = Math.max(index, other);
      const key = low * count + high;
      if (seen.has(key)) continue;
      seen.add(key);
      pairs.push(low, high);
    }
  }
  return Uint32Array.from(pairs);
}

/** Two vertices per edge, ready for `LineSegments`. */
export function edgeLinePositions(positions: Float32Array, edges: Uint32Array): Float32Array {
  const lines = new Float32Array(edges.length * 3);
  for (let slot = 0; slot < edges.length; slot += 1) {
    const vertex = edges[slot] ?? 0;
    lines[slot * 3] = positions[vertex * 3] ?? 0;
    lines[slot * 3 + 1] = positions[vertex * 3 + 1] ?? 0;
    lines[slot * 3 + 2] = positions[vertex * 3 + 2] ?? 0;
  }
  return lines;
}

/** Per-vertex hub flag for the line geometry (two vertices per edge). */
export function edgeLineHubs(hubs: Float32Array, edges: Uint32Array): Float32Array {
  const flags = new Float32Array(edges.length);
  for (let slot = 0; slot < edges.length; slot += 1) flags[slot] = hubs[edges[slot] ?? 0] ?? 0;
  return flags;
}

/** About 0.3 pulses per edge, never more than the quality cap. */
export function pulseCountFor(edgeCount: number, cap: number): number {
  return Math.min(cap, Math.round(edgeCount * 0.3));
}

/** Pulses travel along random edges; everything else is computed in the vertex shader. */
export function generatePulses(
  positions: Float32Array,
  edges: Uint32Array,
  count: number,
  seed: number,
): PulseData {
  const edgeCount = Math.floor(edges.length / 2);
  const total = edgeCount === 0 ? 0 : count;
  const random = mulberry32(seed);
  const starts = new Float32Array(total * 3);
  const ends = new Float32Array(total * 3);
  const offsets = new Float32Array(total);
  const speeds = new Float32Array(total);
  for (let index = 0; index < total; index += 1) {
    const edge = Math.min(edgeCount - 1, Math.floor(random() * edgeCount));
    const first = edges[edge * 2] ?? 0;
    const second = edges[edge * 2 + 1] ?? 0;
    const forward = random() < 0.5;
    const from = forward ? first : second;
    const to = forward ? second : first;
    for (let axis = 0; axis < 3; axis += 1) {
      starts[index * 3 + axis] = positions[from * 3 + axis] ?? 0;
      ends[index * 3 + axis] = positions[to * 3 + axis] ?? 0;
    }
    offsets[index] = random();
    speeds[index] = 0.25 + 0.5 * random();
  }
  return { count: total, starts, ends, offsets, speeds };
}

/** Repeats every pulse `tails` times (head first) so the shader can draw a fading trail behind it. */
export function expandPulseTails(pulses: PulseData, tails: number): PulseTailData {
  const total = pulses.count * tails;
  const starts = new Float32Array(total * 3);
  const ends = new Float32Array(total * 3);
  const offsets = new Float32Array(total);
  const speeds = new Float32Array(total);
  const tail = new Float32Array(total);
  for (let pulse = 0; pulse < pulses.count; pulse += 1) {
    for (let step = 0; step < tails; step += 1) {
      const slot = pulse * tails + step;
      for (let axis = 0; axis < 3; axis += 1) {
        starts[slot * 3 + axis] = pulses.starts[pulse * 3 + axis] ?? 0;
        ends[slot * 3 + axis] = pulses.ends[pulse * 3 + axis] ?? 0;
      }
      offsets[slot] = pulses.offsets[pulse] ?? 0;
      speeds[slot] = pulses.speeds[pulse] ?? 0;
      tail[slot] = step;
    }
  }
  return { count: total, starts, ends, offsets, speeds, tails: tail };
}
