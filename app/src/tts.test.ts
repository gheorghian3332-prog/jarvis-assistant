import { describe, expect, it } from "vitest";
import type { TtsVoices } from "./api";
import { keyToSave, testPhrase, voiceOptions } from "./tts";

const report: TtsVoices = {
  engines: [],
  voices: [
    { engine: "piper", id: "ru_RU-dmitri-medium", lang: "ru", installed: true, selected: true },
    { engine: "piper", id: "ru_RU-ruslan-medium", lang: "ru", installed: false, selected: false },
    { engine: "piper", id: "en_GB-alan-medium", lang: "en", installed: false, selected: false },
    { engine: "kokoro", id: "bm_george", lang: "en", installed: true, selected: true },
    { engine: "kokoro", id: "bm_lewis", lang: "en", installed: true, selected: false },
  ],
};

describe("voiceOptions", () => {
  it("lists the voices of one engine and language", () => {
    expect(voiceOptions(report, "kokoro", "en", "bm_george").map((o) => o.id)).toEqual(["bm_george", "bm_lewis"]);
    expect(voiceOptions(report, "piper", "ru", "ru_RU-dmitri-medium")).toEqual([
      { id: "ru_RU-dmitri-medium", installed: true },
      { id: "ru_RU-ruslan-medium", installed: false },
    ]);
  });

  it("always contains the stored voice, marked as not installed when unknown", () => {
    expect(voiceOptions(report, "kokoro", "en", "bm_daniel")).toContainEqual({ id: "bm_daniel", installed: false });
    expect(voiceOptions(null, "piper", "ru", "ru_RU-dmitri-medium")).toEqual([
      { id: "ru_RU-dmitri-medium", installed: false },
    ]);
    expect(voiceOptions(null, "piper", "ru", "")).toEqual([]);
  });
});

describe("test phrase and key field", () => {
  it("speaks in the selected language", () => {
    const phrases = { ru: "Привет", en: "Hello" };
    expect(testPhrase("ru", phrases)).toBe("Привет");
    expect(testPhrase("en", phrases)).toBe("Hello");
  });

  it("does not send an empty key and trims the rest", () => {
    expect(keyToSave("")).toBeNull();
    expect(keyToSave("   ")).toBeNull();
    expect(keyToSave("  abc123 ")).toBe("abc123");
  });
});
