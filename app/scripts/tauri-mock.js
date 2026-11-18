// Stand-in for the Tauri backend, used ONLY to take screenshots of the dev-server page in a plain browser.
// The values below are illustrative sample data for the picture, not measurements.
(() => {
  const callbacks = new Map();
  const listeners = new Map();
  let nextId = 1;
  const settings = { "setup.completed": true };
  const GIB = 1024 ** 3;
  const snapshot = () => ({
    mode: "window",
    gpu_source: "nvml",
    metrics: {
      timestamp_ms: Date.now(),
      cpu: { usage_percent: 14.2, per_core_percent: [22, 8, 31, 12, 5, 18, 9, 14, 40, 6, 11, 17], frequency_mhz: 3900, temperature_c: 54 },
      gpu: {
        name: "NVIDIA GeForce GTX 1660",
        utilization_percent: 9,
        temperature_c: window.__gpuTemp ?? 47,
        vram_used_bytes: 1.4 * GIB,
        vram_total_bytes: 6 * GIB,
        graphics_clock_mhz: 1350,
        memory_clock_mhz: 6800,
        power_watts: 38,
      },
      ram: { used_bytes: 9.6 * GIB, total_bytes: 32 * GIB, available_bytes: 22.4 * GIB },
      storage: {
        volumes: [{ mount: "C:", is_system: true, total_bytes: 476 * GIB, free_bytes: 212 * GIB, read_bytes_per_sec: 120000, write_bytes_per_sec: 48000 }],
        health: [{ name: "Samsung SSD 970", status: "Healthy" }],
      },
      network: { link_up: true, internet_reachable: true, down_bytes_per_sec: 1250000, up_bytes_per_sec: 180000, rtt_ms: 14, rtt_host: "1.1.1.1:443" },
      hung_windows: [],
    },
  });
  const handlers = {
    settings_get: ({ key }) => (key in settings ? settings[key] : null),
    settings_set: () => null,
    core_info: () => ({ version: "0.1.0", uptime_s: 3600, features: [] }),
    audit_list: () => [
      { id: 3, ts: "2026-11-18 21:40:12", action: "settings.set", args: {}, risk: "safe", ok: true, detail: "visual.quality" },
      { id: 2, ts: "2026-11-18 21:32:07", action: "info.time", args: {}, risk: "safe", ok: true, detail: "21:32" },
      { id: 1, ts: "2026-11-18 21:20:55", action: "app.open", args: {}, risk: "confirm", ok: false, detail: "refused by user" },
    ],
    voice_devices: () => [
      { name: "Microphone (USB Audio)", is_default: true },
      { name: "Headset Mic", is_default: false },
    ],
    voice_status: () => ({ state: "IDLE", mic: { active: true, device: "Microphone (USB Audio)", peak: 0.042, no_signal: false, error: null } }),
    system_snapshot: () => snapshot(),
    system_alerts: () => [],
    system_top_processes: () => [],
    tts_voices: () => ({
      engines: [
        { engine: "piper", available: true, detail: "ready" },
        { engine: "kokoro", available: false, detail: "not installed" },
        { engine: "elevenlabs", available: false, detail: "no key" },
      ],
      voices: [{ engine: "piper", id: "ru_RU-dmitri-medium", lang: "ru", installed: true, selected: true }],
    }),
    audio_outputs: () => [{ name: "Speakers (Realtek)", is_default: true }],
    secret_exists: () => false,
    control_actions: () => ({
      default_roots: ["C:\\Users\\user\\Documents"],
      actions: [
        { name: "info.time", risk: "safe", description_ru: "Сказать время", description_en: "Tell the time", enabled: true },
        { name: "app.open", risk: "confirm", description_ru: "Открыть приложение", description_en: "Open an application", enabled: true },
        { name: "power.shutdown", risk: "confirm", description_ru: "Выключить компьютер", description_en: "Shut the computer down", enabled: false },
        { name: "process.kill", risk: "forbidden", description_ru: "Завершить процесс", description_en: "Kill a process", enabled: false },
      ],
    }),
    ai_status: () => ({ enabled: true, running: true, version: "0.12.3", model: "qwen3:8b", models: [{ name: "qwen3:8b", size_bytes: 5 * GIB }], loaded: [] }),
    memory_facts: () => [{ id: 1, ts: "2026-11-17 10:02:11", kind: "user", key: "name", value: "Иван" }],
    memory_history: () => [{ id: 1, ts: "2026-11-18 21:32:07", text: "который час", lang: "ru", route: "skill", action: "info.time", ok: true }],
    setup_status: () => ({
      installed_layout: true,
      models_dir: "C:\\Users\\user\\AppData\\Local\\Jarvis\\models",
      ollama_url: "http://127.0.0.1:11434",
      default_model: "qwen3:8b",
      items: [
        { id: "vosk_model", installed: true, optional: false, approx_mb: 45, running: false },
        { id: "piper", installed: true, optional: false, approx_mb: 20, running: false },
        { id: "whisper", installed: false, optional: false, approx_mb: 480, running: false },
        { id: "kokoro", installed: false, optional: true, approx_mb: 330, running: false },
      ],
    }),
    "plugin:event|listen": ({ event, handler }) => {
      const id = nextId++;
      if (!listeners.has(event)) listeners.set(event, []);
      listeners.get(event).push({ id, handler });
      return id;
    },
    "plugin:event|unlisten": ({ event, eventId }) => {
      listeners.set(
        event,
        (listeners.get(event) || []).filter((item) => item.id !== eventId),
      );
      return null;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    transformCallback(cb) {
      const id = nextId++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    convertFileSrc: (path) => path,
    invoke: async (command, args) => {
      const handler = handlers[command];
      return handler ? handler(args || {}) : null;
    },
  };
  window.__emit = (event, payload) => {
    for (const item of listeners.get(event) || []) callbacks.get(item.handler)?.({ event, id: item.id, payload });
  };
})();
