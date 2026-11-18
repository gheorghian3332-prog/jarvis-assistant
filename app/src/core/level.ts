export const ATTACK_MS = 30;
export const RELEASE_MS = 200;

/** Clamps the raw `assistant://level` rms to 0..1; anything non-finite counts as silence. */
export function sanitizeLevel(raw: unknown): number {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return 0;
  return Math.min(1, Math.max(0, raw));
}

/** Envelope follower: fast attack (30 ms), slow release (200 ms), frame-rate independent. */
export function smoothLevel(current: number, target: number, dtMs: number): number {
  if (dtMs <= 0) return current;
  const tau = target > current ? ATTACK_MS : RELEASE_MS;
  return current + (target - current) * (1 - Math.exp(-dtMs / tau));
}
