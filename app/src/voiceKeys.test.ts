import { describe, expect, it } from "vitest";
import { shortcutFromKeyEvent } from "./voiceKeys";

const base = { ctrlKey: false, altKey: false, shiftKey: false, metaKey: false };

describe("shortcutFromKeyEvent", () => {
  it("keeps function keys as they are", () => {
    expect(shortcutFromKeyEvent({ ...base, code: "F8" })).toBe("F8");
  });

  it("strips the Key and Digit prefixes", () => {
    expect(shortcutFromKeyEvent({ ...base, code: "KeyK" })).toBe("K");
    expect(shortcutFromKeyEvent({ ...base, code: "Digit5" })).toBe("5");
  });

  it("adds modifiers in a fixed order", () => {
    expect(shortcutFromKeyEvent({ ...base, code: "KeyJ", altKey: true, ctrlKey: true })).toBe("Ctrl+Alt+J");
    expect(shortcutFromKeyEvent({ ...base, code: "F9", shiftKey: true, metaKey: true })).toBe("Shift+Super+F9");
  });

  it("ignores a lone modifier", () => {
    expect(shortcutFromKeyEvent({ ...base, code: "ControlLeft", ctrlKey: true })).toBeNull();
    expect(shortcutFromKeyEvent({ ...base, code: "" })).toBeNull();
  });
});
