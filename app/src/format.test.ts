import { describe, expect, it } from "vitest";
import { clampPercent, formatBytes, formatMetrics, formatPercent, formatUptime } from "./format";

describe("format", () => {
  it("formats bytes as MB or GB", () => {
    expect(formatBytes(512 * 1024 ** 2)).toBe("512 MB");
    expect(formatBytes(8.25 * 1024 ** 3)).toBe("8.3 GB");
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
  });

  it("clamps percentages", () => {
    expect(clampPercent(150)).toBe(100);
    expect(clampPercent(-5)).toBe(0);
    expect(formatPercent(12.345)).toBe("12.3%");
    expect(formatPercent(Number.POSITIVE_INFINITY)).toBe("—");
  });

  it("builds the metrics view", () => {
    const view = formatMetrics({
      cpu_percent: 7.5,
      ram_used_bytes: 4 * 1024 ** 3,
      ram_total_bytes: 16 * 1024 ** 3,
    });
    expect(view.cpuText).toBe("7.5%");
    expect(view.ramText).toBe("4.0 GB / 16.0 GB");
    expect(view.ramPercent).toBe(25);
  });

  it("survives a zero total memory", () => {
    const view = formatMetrics({ cpu_percent: 0, ram_used_bytes: 1, ram_total_bytes: 0 });
    expect(view.ramPercent).toBe(0);
  });

  it("formats uptime", () => {
    expect(formatUptime(3725)).toBe("01:02:05");
    expect(formatUptime(-1)).toBe("—");
  });
});
