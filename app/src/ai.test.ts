// @vitest-environment jsdom
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AiStatus, HistoryRow, MemoryFact } from "./api";
import { AiView } from "./AiTab";
import { translate, type MessageKey } from "./i18n";
import { MemoryView } from "./MemoryTab";
import { PrivacyView } from "./PrivacyTab";
import {
  PERSONALITY_MODES,
  SETTING_DEFAULTS,
  SETTING_KEYS,
  resolveSettings,
  validCloudUrl,
  validModelName,
  validateSetting,
} from "./settings";

const t = (key: MessageKey) => translate("en", key);
const noop = () => undefined;

describe("AI, personality and privacy settings", () => {
  it("have the documented defaults", () => {
    expect(SETTING_DEFAULTS["ai.enabled"]).toBe(true);
    expect(SETTING_DEFAULTS["ai.keep_alive_minutes"]).toBe(5);
    expect(SETTING_DEFAULTS["ai.num_ctx"]).toBeLessThanOrEqual(4096);
    expect(SETTING_DEFAULTS["ai.num_thread"]).toBeLessThanOrEqual(6);
    expect(SETTING_DEFAULTS["ai.cloud_enabled"]).toBe(false);
    expect(SETTING_DEFAULTS["personality.mode"]).toBe("classic");
    expect(SETTING_DEFAULTS["privacy.history_enabled"]).toBe(true);
    expect(SETTING_DEFAULTS["privacy.history_days"]).toBe(30);
    expect(SETTING_KEYS).toContain("ai.model");
  });

  it("accept valid values", () => {
    const valid: [string, unknown][] = [
      ["ai.model", "gemma3:4b"],
      ["ai.keep_alive_minutes", 0],
      ["ai.num_ctx", 2048],
      ["ai.num_thread", 4],
      ["ai.cloud_base_url", ""],
      ["ai.cloud_base_url", "https://api.example.com/v1"],
      ["ai.cloud_model", ""],
      ["personality.mode", "friendly"],
      ["personality.custom", "я".repeat(2000)],
      ["privacy.history_days", 90],
    ];
    for (const [key, value] of valid) {
      expect(validateSetting(key as never, value).ok, `${key}=${String(value)}`).toBe(true);
    }
    for (const mode of PERSONALITY_MODES) expect(validateSetting("personality.mode", mode).ok).toBe(true);
  });

  it("reject invalid values like the backend does", () => {
    const invalid: [string, unknown][] = [
      ["ai.model", ""],
      ["ai.model", "a b"],
      ["ai.keep_alive_minutes", -1],
      ["ai.keep_alive_minutes", 1.5],
      ["ai.num_ctx", 8192],
      ["ai.num_thread", 7],
      ["ai.cloud_base_url", "http://api.example.com"],
      ["ai.cloud_base_url", "https://user:pw@api.example.com"],
      ["personality.mode", "rude"],
      ["personality.custom", "я".repeat(2001)],
      ["privacy.history_days", 0],
      ["privacy.history_days", 366],
      ["privacy.history_enabled", "yes"],
    ];
    for (const [key, value] of invalid) {
      expect(validateSetting(key as never, value).ok, `${key}=${String(value)}`).toBe(false);
    }
    expect(validModelName("qwen3:4b")).toBe(true);
    expect(validCloudUrl("ftp://x")).toBe(false);
  });

  it("fall back to defaults when stored values are bad", () => {
    const resolved = resolveSettings({ "ai.num_ctx": 99999, "personality.mode": "friendly" });
    expect(resolved["ai.num_ctx"]).toBe(SETTING_DEFAULTS["ai.num_ctx"]);
    expect(resolved["personality.mode"]).toBe("friendly");
  });
});

describe("AI tab", () => {
  const status: AiStatus = {
    enabled: true,
    running: true,
    version: "0.40.16",
    model: "qwen3:4b",
    models: [
      { name: "qwen3:4b", size_bytes: 2382000000 },
      { name: "gemma3:4b", size_bytes: 3195000000 },
    ],
    loaded: [{ name: "qwen3:4b", size_bytes: 3180000000, vram_bytes: 3180000000, expires_at: "x" }],
  };
  const render = (overrides: Partial<Parameters<typeof AiView>[0]> = {}) =>
    renderToStaticMarkup(
      createElement(AiView, {
        t,
        values: SETTING_DEFAULTS,
        status,
        keySaved: false,
        problem: null,
        onChange: noop,
        onRefresh: noop,
        onUnload: noop,
        onSaveKey: noop,
        onDeleteKey: noop,
        ...overrides,
      }),
    );

  it("lists the installed models, the loaded one and the unload button", () => {
    const html = render();
    expect(html).toContain("gemma3:4b");
    expect(html).toContain("0.40.16");
    expect(html).toContain("VRAM");
    expect(html).toContain(t("aiUnload"));
  });

  it("shows that the server is stopped and keeps the chosen model", () => {
    const html = render({ status: { ...status, running: false, version: null, models: [], loaded: [] } });
    expect(html).toContain(t("aiStopped"));
    expect(html).toContain(t("aiNothingLoaded"));
    expect(html).toContain("qwen3:4b");
  });

  it("keeps the cloud off by default and never prints a key", () => {
    const html = render();
    expect(html).toMatch(/type="checkbox"(?![^>]*checked)[^>]*/);
    expect(html).toContain('type="password"');
    expect(html).not.toContain("sk-");
  });
});

describe("PRIVACY and MEMORY tabs", () => {
  it("ask twice before deleting all memory", () => {
    const first = renderToStaticMarkup(
      createElement(PrivacyView, { t, values: SETTING_DEFAULTS, armed: false, notice: null, onChange: noop, onClearHistory: noop, onDeleteAll: noop }),
    );
    expect(first).toContain(t("privacyDeleteAll"));
    expect(first).not.toContain(t("privacyConfirmAll"));
    const second = renderToStaticMarkup(
      createElement(PrivacyView, { t, values: SETTING_DEFAULTS, armed: true, notice: null, onChange: noop, onClearHistory: noop, onDeleteAll: noop }),
    );
    expect(second).toContain(t("privacyConfirmAll"));
  });

  it("lists facts and history, each with a delete button", () => {
    const facts: MemoryFact[] = [{ id: 3, ts: "t", kind: "project", key: "мой проект", value: "C:\\Users\\example\\jarvis" }];
    const history: HistoryRow[] = [{ id: 9, ts: "t", text: "который час", lang: "ru", route: "fast", action: "info.time", ok: true }];
    const html = renderToStaticMarkup(
      createElement(MemoryView, { t, query: "", facts, history, error: null, onQuery: noop, onDeleteFact: noop, onDeleteHistory: noop }),
    );
    expect(html).toContain("мой проект");
    expect(html).toContain("который час");
    expect(html).toContain("info.time");
    expect(html).toContain("Delete #3");
    expect(html).toContain("Delete #9");
  });
});
