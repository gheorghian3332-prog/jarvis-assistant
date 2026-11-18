import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// A plain function (not vi.fn) so a failing invoke is not recorded as a mock result.
type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let behavior: Invoke = () => Promise.resolve(undefined);
const calls: Array<[string, Record<string, unknown> | undefined]> = [];

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push([command, args]);
    return behavior(command, args);
  },
}));

import {
  audioOutputs,
  chatSend,
  confirmResponse,
  controlActions,
  emergencyStop,
  call,
  ELEVEN_KEY,
  isTauri,
  listenEvent,
  secretClear,
  secretExists,
  secretSet,
  settingsSet,
  toCommandError,
  ttsSpeak,
  ttsStop,
  ttsVoices,
} from "./api";

describe("command errors", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    calls.length = 0;
    behavior = () => Promise.resolve(undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("passes results through", async () => {
    behavior = () => Promise.resolve({ version: "0.1.0", uptime_s: 1, features: [] });
    await expect(call("core_info")).resolves.toMatchObject({ version: "0.1.0" });
    expect(calls).toEqual([["core_info", undefined]]);
  });

  it("normalizes a structured backend error", async () => {
    behavior = () => Promise.reject({ code: "VALIDATION", message: "bad key" });
    const failure = await settingsSet("x", 1).then(
      () => null,
      (error: unknown) => error,
    );
    expect(failure).toEqual({ code: "VALIDATION", message: "bad key" });
    expect(calls).toEqual([["settings_set", { key: "x", value: 1 }]]);
  });

  it("maps unknown codes and plain strings", () => {
    expect(toCommandError({ code: "WEIRD", message: "m" })).toEqual({ code: "UNKNOWN", message: "m" });
    expect(toCommandError("boom")).toEqual({ code: "UNKNOWN", message: "boom" });
    expect(toCommandError(null).code).toBe("UNKNOWN");
  });
});

describe("outside Tauri", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("detects the missing backend without throwing", async () => {
    vi.stubGlobal("window", {});
    expect(isTauri()).toBe(false);
    await expect(call("core_info")).rejects.toMatchObject({ code: "UNAVAILABLE" });
    expect(calls).toEqual([]);
    const stop = await listenEvent("assistant://state", () => undefined);
    expect(stop()).toBeUndefined();
  });

  it("detects Tauri", () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    expect(isTauri()).toBe(true);
  });
});

describe("speech commands", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    calls.length = 0;
    behavior = () => Promise.resolve(undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("calls the contract commands with their arguments", async () => {
    await ttsSpeak("Привет");
    await ttsStop();
    await ttsVoices();
    await audioOutputs();
    expect(calls).toEqual([
      ["tts_speak", { text: "Привет" }],
      ["tts_stop", undefined],
      ["tts_voices", undefined],
      ["audio_outputs", undefined],
    ]);
  });

  it("sends a secret once and only asks whether it exists afterwards", async () => {
    behavior = (command) => Promise.resolve(command === "secret_exists" ? true : undefined);
    await secretSet(ELEVEN_KEY, "not-a-real-key");
    await expect(secretExists(ELEVEN_KEY)).resolves.toBe(true);
    await secretClear(ELEVEN_KEY);
    expect(calls).toEqual([
      ["secret_set", { name: "elevenlabs_api_key", value: "not-a-real-key" }],
      ["secret_exists", { name: "elevenlabs_api_key" }],
      ["secret_clear", { name: "elevenlabs_api_key" }],
    ]);
  });

  it("reports a refusal from the backend as a command error", async () => {
    behavior = () => Promise.reject({ code: "VALIDATION", message: "unknown secret name" });
    await expect(secretSet(ELEVEN_KEY, "x")).rejects.toEqual({
      code: "VALIDATION",
      message: "unknown secret name",
    });
  });
});

describe("control commands", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    calls.length = 0;
    behavior = () => Promise.resolve(undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("calls the control commands (the argument action_id travels as actionId)", async () => {
    await chatSend("который час");
    await confirmResponse("act-1", false);
    await emergencyStop();
    await controlActions();
    expect(calls).toEqual([
      ["chat_send", { text: "который час" }],
      ["confirm_response", { actionId: "act-1", approved: false }],
      ["emergency_stop", undefined],
      ["control_actions", undefined],
    ]);
  });

  it("reports an unknown confirmation as NOT_FOUND", async () => {
    behavior = () => Promise.reject({ code: "NOT_FOUND", message: "no pending confirmation" });
    await expect(confirmResponse("act-9", true)).rejects.toMatchObject({ code: "NOT_FOUND" });
  });
});
