// @vitest-environment jsdom
import { act, createElement, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ActionInfo, AuditEntry, ConfirmRequest } from "./api";
import { CommandBarView } from "./CommandBar";
import { ConfirmView } from "./ConfirmDialog";
import { CONFIRM_SECONDS, fractionLeft, secondsLeft } from "./confirm";
import { translate, type MessageKey } from "./i18n";
import {
  SETTING_DEFAULTS,
  resolveSettings,
  validateSetting,
  validAliasTarget,
  validDrivePath,
  type AppSettings,
} from "./settings";
import { ToolsView } from "./ToolsTab";
import {
  addAlias,
  addRoot,
  isEnabled,
  recentActions,
  removeAlias,
  removeRoot,
  riskClass,
  toggleAction,
} from "./tools";

const t = (key: MessageKey) => translate("ru", key);

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Renders into jsdom; the container is removed after the test. */
function mount(element: ReactElement): HTMLElement {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  act(() => root.render(element));
  cleanups.push(() => {
    act(() => root.unmount());
    container.remove();
  });
  return container;
}
const cleanups: Array<() => void> = [];
afterEach(() => {
  while (cleanups.length) cleanups.pop()?.();
});

const buttons = (container: HTMLElement) => Array.from(container.querySelectorAll("button"));
const boxes = (container: HTMLElement) => Array.from(container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'));
const click = (element: HTMLElement | undefined) => {
  if (!element) throw new Error("element not found");
  act(() => element.click());
};

const REQUEST: ConfirmRequest = {
  action_id: "act-7",
  action: "power.shutdown",
  summary: "Выключить компьютер через 60 с",
  risk: "confirm",
};

describe("confirmation timing", () => {
  it("counts whole seconds down and never goes negative", () => {
    expect(CONFIRM_SECONDS).toBe(30);
    expect(secondsLeft(30_000, 0)).toBe(30);
    expect(secondsLeft(30_000, 100)).toBe(30);
    expect(secondsLeft(30_000, 29_100)).toBe(1);
    expect(secondsLeft(30_000, 30_000)).toBe(0);
    expect(secondsLeft(30_000, 99_000)).toBe(0);
  });

  it("reports the share of time left", () => {
    expect(fractionLeft(30_000, 0)).toBe(1);
    expect(fractionLeft(30_000, 15_000)).toBeCloseTo(0.5);
    expect(fractionLeft(30_000, 31_000)).toBe(0);
    expect(fractionLeft(60_000, 0)).toBe(1);
  });
});

describe("confirmation dialog", () => {
  it("shows the summary, the countdown and both buttons", () => {
    const html = renderToStaticMarkup(
      createElement(ConfirmView, { t, request: REQUEST, seconds: 27, fraction: 0.9, error: null, onAnswer: () => undefined }),
    );
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain("Выключить компьютер через 60 с");
    expect(html).toContain("Выполнить");
    expect(html).toContain("Отмена");
    expect(html).toContain(">27<");
    expect(html).toContain("width:90%");
    expect(html).toContain("power.shutdown");
  });

  it("answers with the buttons", () => {
    const onAnswer = vi.fn();
    const container = mount(createElement(ConfirmView, { t, request: REQUEST, seconds: 5, fraction: 0.1, error: null, onAnswer }));
    const found = buttons(container);
    expect(found.map((button) => button.textContent)).toEqual(["Выполнить", "Отмена"]);
    click(found[0]);
    click(found[1]);
    expect(onAnswer.mock.calls).toEqual([[true], [false]]);
  });

  it("focuses the safe answer (Cancel) by default and shows an error", () => {
    const html = renderToStaticMarkup(
      createElement(ConfirmView, { t, request: REQUEST, seconds: 5, fraction: 0.1, error: "boom", onAnswer: () => undefined }),
    );
    expect(html).toContain("boom");
    const container = mount(createElement(ConfirmView, { t, request: REQUEST, seconds: 5, fraction: 0.1, error: null, onAnswer: () => undefined }));
    expect(document.activeElement?.textContent).toBe("Отмена");
    expect(container.querySelector("[role=dialog]")).not.toBeNull();
  });
});

const ACTIONS: ActionInfo[] = [
  { name: "info.time", risk: "safe", description_ru: "Время", description_en: "Time", enabled: true },
  { name: "power.shutdown", risk: "confirm", description_ru: "Выключить", description_en: "Shut down", enabled: true },
  { name: "process.kill", risk: "forbidden", description_ru: "Завершить", description_en: "Kill", enabled: false },
];

const AUDIT: AuditEntry[] = Array.from({ length: 25 }, (_, index) => ({
  id: 100 - index,
  ts: "2026-11-18 03:00:00",
  action: index === 0 ? "power.shutdown" : "info.time",
  args: {},
  risk: "safe",
  ok: index !== 0,
  detail: index === 0 ? "declined: shutdown" : "ok",
}));

function viewProps(overrides: Partial<AppSettings> = {}, onChange = vi.fn()) {
  return {
    t,
    language: "ru" as const,
    actions: ACTIONS,
    defaultRoots: ["C:\\Users\\me", "D:\\"],
    values: { ...SETTING_DEFAULTS, ...overrides },
    recent: AUDIT,
    error: null,
    onChange,
    onEmergencyStop: vi.fn(),
  };
}

describe("TOOLS tab", () => {
  it("lists every action with a coloured risk and a switch", () => {
    const html = renderToStaticMarkup(createElement(ToolsView, viewProps({ "control.disabled_actions": ["power.shutdown"] })));
    for (const action of ACTIONS) expect(html).toContain(action.name);
    expect(html).toContain("risk risk-safe");
    expect(html).toContain("risk risk-confirm");
    expect(html).toContain("risk risk-forbidden");
    const container = mount(createElement(ToolsView, viewProps({ "control.disabled_actions": ["power.shutdown"] })));
    const switches = boxes(container);
    expect(switches.map((box) => box.checked)).toEqual([true, false, false]);
    expect(switches.map((box) => box.disabled)).toEqual([false, false, true]);
  });

  it("a forbidden action cannot be switched on", () => {
    expect(isEnabled({ name: "process.kill", risk: "forbidden" }, [])).toBe(false);
    const container = mount(createElement(ToolsView, viewProps()));
    const forbidden = boxes(container)[2] as HTMLInputElement;
    expect(forbidden.disabled).toBe(true);
    expect(forbidden.title).toBe(t("forbiddenLocked"));
    click(forbidden);
    expect(forbidden.checked).toBe(false);
  });

  it("toggling writes the new list of switched-off actions", () => {
    const onChange = vi.fn();
    const container = mount(createElement(ToolsView, viewProps({}, onChange)));
    click(boxes(container)[0]);
    expect(onChange).toHaveBeenCalledWith("control.disabled_actions", ["info.time"]);
  });

  it("shows the default roots until the user sets their own, and removes roots and aliases", () => {
    const empty = renderToStaticMarkup(createElement(ToolsView, viewProps()));
    expect(empty).toContain("C:\\Users\\me");
    const onChange = vi.fn();
    const props = viewProps({ "control.file_roots": ["D:\\Games"], "control.app_aliases": { игра: "steam://rungameid/730" } }, onChange);
    const html = renderToStaticMarkup(createElement(ToolsView, props));
    expect(html).toContain("D:\\Games");
    expect(html).toContain("steam://rungameid/730");
    expect(html).not.toContain("C:\\Users\\me");
    const container = mount(createElement(ToolsView, props));
    const removers = buttons(container).filter((button) => button.textContent === "Убрать");
    expect(removers).toHaveLength(2);
    click(removers[0]);
    click(removers[1]);
    expect(onChange).toHaveBeenNthCalledWith(1, "control.file_roots", []);
    expect(onChange).toHaveBeenNthCalledWith(2, "control.app_aliases", {});
  });

  it("shows the last 20 audit entries and the emergency button", () => {
    const html = renderToStaticMarkup(createElement(ToolsView, viewProps()));
    expect(html.match(/<li class="[^"]*"><span class="mono ts"/g)).toHaveLength(20);
    expect(html).toContain("declined: shutdown");
    expect(html).toContain("Аварийный стоп");
  });

  it("speaks English when asked to", () => {
    const html = renderToStaticMarkup(createElement(ToolsView, { ...viewProps(), language: "en", t: (key: MessageKey) => translate("en", key) }));
    expect(html).toContain("Shut down");
    expect(html).toContain("Emergency stop");
  });
});

describe("tools helpers", () => {
  it("toggles actions without duplicates", () => {
    expect(toggleAction([], "b", false)).toEqual(["b"]);
    expect(toggleAction(["b"], "b", false)).toEqual(["b"]);
    expect(toggleAction(["b", "a"], "b", true)).toEqual(["a"]);
    expect(toggleAction(["b"], "a", false)).toEqual(["a", "b"]);
  });

  it("adds roots only when they look like drive paths", () => {
    expect(addRoot([], " D:\\Games ")).toEqual({ ok: true, value: ["D:\\Games"] });
    expect(addRoot(["D:\\Games"], "d:\\games").ok).toBe(false);
    for (const bad of ["", "Games", "\\\\server\\share", "D:\\a\\..\\b", "//x/y", "C:"]) {
      expect(addRoot([], bad).ok, bad).toBe(false);
    }
    expect(removeRoot(["a", "b"], "a")).toEqual(["b"]);
  });

  it("adds aliases only with a valid name and target", () => {
    expect(addAlias({}, "игра", "steam://rungameid/730")).toEqual({ ok: true, value: { игра: "steam://rungameid/730" } });
    expect(addAlias({}, "tool", "D:\\Apps\\tool.exe").ok).toBe(true);
    expect(addAlias({}, "tool", "D:\\Apps\\tool.LNK").ok).toBe(true);
    for (const target of ["D:\\Apps\\tool.bat", "steam://run/730", "https://x.y", "notepad.exe", "\\\\s\\a.exe", "steam://rungameid/"]) {
      expect(addAlias({}, "x", target).ok, target).toBe(false);
    }
    expect(addAlias({}, "  ", "steam://rungameid/1").ok).toBe(false);
    const full = Object.fromEntries(Array.from({ length: 50 }, (_, i) => [`a${i}`, "steam://rungameid/1"]));
    expect(addAlias(full, "extra", "steam://rungameid/1").ok).toBe(false);
    expect(addAlias(full, "a1", "steam://rungameid/2").ok).toBe(true);
    expect(removeAlias({ a: "x", b: "y" }, "a")).toEqual({ b: "y" });
  });

  it("maps risks to classes and trims the audit list", () => {
    expect(riskClass("forbidden")).toBe("risk risk-forbidden");
    expect(recentActions(AUDIT)).toHaveLength(20);
    expect(recentActions(AUDIT, 3).map((entry) => entry.id)).toEqual([100, 99, 98]);
  });
});

describe("control settings", () => {
  it("accepts the defaults and valid values", () => {
    for (const [key, value] of [
      ["control.disabled_actions", []],
      ["control.disabled_actions", ["power.shutdown"]],
      ["control.file_roots", ["D:\\Games"]],
      ["control.app_aliases", { игра: "steam://rungameid/730" }],
      ["control.emergency_key", "Ctrl+Alt+Pause"],
    ] as const) {
      expect(validateSetting(key, value).ok, `${key}`).toBe(true);
    }
  });

  it("rejects paths outside a drive, network paths and bad alias targets", () => {
    for (const [key, value] of [
      ["control.disabled_actions", "power.shutdown"],
      ["control.disabled_actions", ["a", "a"]],
      ["control.file_roots", ["relative"]],
      ["control.file_roots", ["\\\\server\\share"]],
      ["control.file_roots", ["D:\\a", "d:\\A"]],
      ["control.app_aliases", { x: "cmd /c calc" }],
      ["control.app_aliases", { x: "steam://rungameid/abc" }],
      ["control.app_aliases", ["x"]],
      ["control.emergency_key", ""],
    ] as const) {
      expect(validateSetting(key, value).ok, `${key} ${JSON.stringify(value)}`).toBe(false);
    }
  });

  it("falls back to defaults for stored garbage", () => {
    const settings = resolveSettings({ "control.file_roots": ["\\\\evil\\share"], "control.app_aliases": "x" });
    expect(settings["control.file_roots"]).toEqual([]);
    expect(settings["control.app_aliases"]).toEqual({});
    expect(settings["control.emergency_key"]).toBe("Ctrl+Alt+Pause");
  });

  it("exposes the path predicates", () => {
    expect(validDrivePath("E:\\x")).toBe(true);
    expect(validDrivePath("E:/x")).toBe(true);
    expect(validDrivePath("x")).toBe(false);
    expect(validAliasTarget("steam://rungameid/12")).toBe(true);
    expect(validAliasTarget(5)).toBe(false);
  });
});

describe("command bar", () => {
  const base = { t, text: "", reply: null, error: null, busy: false, onText: vi.fn(), onSubmit: vi.fn(), onEmergencyStop: vi.fn() };

  it("renders the input, the reply and the emergency button", () => {
    const html = renderToStaticMarkup(createElement(CommandBarView, { ...base, text: "который час", reply: "Сейчас 03:00." }));
    expect(html).toContain("который час");
    expect(html).toContain("Сейчас 03:00.");
    expect(html).toContain("Аварийный стоп");
  });

  it("does not send an empty command and submits through the form", () => {
    const empty = mount(createElement(CommandBarView, base));
    expect(empty.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled).toBe(true);
    const onSubmit = vi.fn();
    const filled = mount(createElement(CommandBarView, { ...base, text: "x", onSubmit }));
    const send = filled.querySelector<HTMLButtonElement>('button[type="submit"]');
    expect(send?.disabled).toBe(false);
    click(send as HTMLButtonElement);
    expect(onSubmit).toHaveBeenCalledOnce();
  });
});
