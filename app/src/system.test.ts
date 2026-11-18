import { describe, expect, it } from "vitest";
import type { AlertPayload } from "./api";
import { validateSetting } from "./settings";
import {
  ALERT_DEFAULTS,
  alertText,
  formatMegahertz,
  formatRate,
  formatTemperature,
  freePercent,
  minuteToTime,
  pushAlert,
  resolveAlerts,
  timeToMinute,
  toastMillis,
  validateAlerts,
  validRttHost,
  withEnabled,
  withThreshold,
} from "./system";

const alert = (id: string, at: number, severity: AlertPayload["severity"] = "warning"): AlertPayload => ({
  id,
  kind: "gpu_temp",
  severity,
  message_ru: "Видеокарта перегрета",
  message_en: "The GPU is overheating",
  speak: false,
  at_ms: at,
});

describe("alert settings", () => {
  it("the defaults are valid and a stored partial value is merged over them", () => {
    expect(validateAlerts(ALERT_DEFAULTS)).toEqual({ ok: true, value: ALERT_DEFAULTS });
    const merged = resolveAlerts({ gpu_temp: { threshold: 80 }, quiet_hours: { enabled: true } });
    expect(merged.gpu_temp).toEqual({ enabled: true, threshold: 80, duration_s: 30 });
    expect(merged.quiet_hours).toEqual({ enabled: true, start_minute: 1320, end_minute: 420 });
    expect(resolveAlerts(null)).toEqual(ALERT_DEFAULTS);
    expect(resolveAlerts({ cpu_load: { threshold: "high" } }).cpu_load.threshold).toBe(90);
  });

  it("editing keeps the rest and the range check mirrors the backend", () => {
    const edited = withThreshold(ALERT_DEFAULTS, "gpu_temp", { threshold: 78 });
    expect(edited.gpu_temp.threshold).toBe(78);
    expect(edited.cpu_load).toEqual(ALERT_DEFAULTS.cpu_load);
    expect(withEnabled(edited, "app_crash", false).app_crash.enabled).toBe(false);
    expect(validateAlerts(withThreshold(ALERT_DEFAULTS, "cpu_load", { threshold: 20 })).ok).toBe(false);
    expect(validateAlerts(withThreshold(ALERT_DEFAULTS, "gpu_temp", { threshold: 200 })).ok).toBe(false);
    expect(validateAlerts(withThreshold(ALERT_DEFAULTS, "ram_load", { duration_s: 99999 })).ok).toBe(false);
    expect(validateAlerts({ ...ALERT_DEFAULTS, max_voice_per_hour: 2.5 }).ok).toBe(false);
    expect(validateAlerts({ ...ALERT_DEFAULTS, quiet_hours: { enabled: true, start_minute: 1500, end_minute: 0 } }).ok).toBe(false);
  });

  it("quiet hours are typed as times of day", () => {
    expect(minuteToTime(1380)).toBe("23:00");
    expect(minuteToTime(7 * 60 + 5)).toBe("07:05");
    expect(timeToMinute("23:00")).toBe(1380);
    expect(timeToMinute("7:05")).toBe(425);
    expect(timeToMinute("24:00")).toBeNull();
    expect(timeToMinute("noon")).toBeNull();
  });

  it("the latency host is a host and a port", () => {
    expect(validRttHost("1.1.1.1:443")).toBe(true);
    expect(validRttHost("example.com:80")).toBe(true);
    for (const bad of ["1.1.1.1", ":443", "a b:80", "host:0", "host:70000", "-x:1"]) expect(validRttHost(bad)).toBe(false);
  });
});

describe("the alert feed", () => {
  it("puts the newest first, drops repeats and keeps at most the limit", () => {
    let feed: AlertPayload[] = [];
    feed = pushAlert(feed, alert("a", 1));
    feed = pushAlert(feed, alert("b", 2));
    feed = pushAlert(feed, alert("b", 2));
    expect(feed.map((item) => item.id)).toEqual(["b", "a"]);
    for (let n = 0; n < 60; n += 1) feed = pushAlert(feed, alert(`x${n}`, 100 + n));
    expect(feed).toHaveLength(50);
    expect(feed[0]?.id).toBe("x59");
  });

  it("shows the text in the interface language and critical alerts longer", () => {
    expect(alertText(alert("a", 1), "ru")).toBe("Видеокарта перегрета");
    expect(alertText(alert("a", 1), "en")).toBe("The GPU is overheating");
    expect(toastMillis(alert("a", 1, "critical"))).toBeGreaterThan(toastMillis(alert("a", 1)));
  });
});

describe("number formatting", () => {
  it("never invents values for missing sensors", () => {
    expect(formatTemperature(null)).toBe("—");
    expect(formatTemperature(73.4)).toBe("73 °C");
    expect(formatRate(null)).toBe("—");
    expect(formatRate(2048)).toBe("2 KB/s");
    expect(formatRate(5 * 1024 * 1024)).toBe("5.0 MB/s");
    expect(formatMegahertz(2904)).toBe("2.90 GHz");
    expect(formatMegahertz(800)).toBe("800 MHz");
    expect(freePercent(200, 50)).toBe(25);
    expect(freePercent(0, 0)).toBe(0);
  });
});

describe("vision settings", () => {
  it("are validated like the backend", () => {
    expect(validateSetting("vision.enabled", true).ok).toBe(true);
    expect(validateSetting("vision.allow_cloud", "yes").ok).toBe(false);
    expect(validateSetting("vision.vlm_model", "qwen2.5vl:3b").ok).toBe(true);
    expect(validateSetting("vision.vlm_model", "bad model!").ok).toBe(false);
    expect(validateSetting("vision.vlm_model", "").ok).toBe(false);
  });
});
