/** How long a confirmation stays valid (mirrors the backend: 30 s). */
export const CONFIRM_SECONDS = 30;

/** Whole seconds left until `deadlineMs`, never negative. */
export function secondsLeft(deadlineMs: number, nowMs: number): number {
  return Math.max(0, Math.ceil((deadlineMs - nowMs) / 1000));
}

/** Share of the time that is left, 0..1 (for the progress bar). */
export function fractionLeft(deadlineMs: number, nowMs: number, totalSeconds = CONFIRM_SECONDS): number {
  const left = Math.max(0, deadlineMs - nowMs) / 1000;
  return Math.min(1, left / totalSeconds);
}
