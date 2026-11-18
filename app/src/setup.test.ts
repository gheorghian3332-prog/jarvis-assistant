import { describe, expect, it } from "vitest";
import type { SetupProgress } from "./api";
import { percent } from "./SetupTab";
import { validateSetting } from "./settings";

const progress = (done: number, total: number): SetupProgress => ({
  item: "piper",
  state: "running",
  done,
  total,
  message: "",
});

describe("setup wizard", () => {
  it("computes the download percentage", () => {
    expect(percent(undefined)).toBeNull();
    expect(percent(progress(5, 0))).toBeNull();
    expect(percent(progress(50, 200))).toBe(25);
    expect(percent(progress(300, 200))).toBe(100);
  });

  it("validates the completed flag as a boolean", () => {
    expect(validateSetting("setup.completed", true).ok).toBe(true);
    expect(validateSetting("setup.completed", "yes").ok).toBe(false);
  });
});
