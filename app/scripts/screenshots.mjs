// Screenshots of the vite dev server page (npm run dev) in headless Edge/Chrome, with a mocked Tauri backend.
// Usage: node scripts/screenshots.mjs [outDir]   (the dev server must be running on :5173)
// ONLY=idle,confirm limits the scenarios; the mock data is sample data for the pictures only.
import { chromium } from "playwright-core";
import { mkdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const out = resolve(process.argv[2] ?? "../docs/screenshots");
mkdirSync(out, { recursive: true });
const mock = readFileSync(new URL("./tauri-mock.js", import.meta.url), "utf8");
const BASE = "http://localhost:5173/";
const SIZES = [
  { name: "1280x800", width: 1280, height: 800 },
  { name: "420x640", width: 420, height: 640 },
];

const feed = async (page) => {
  const emit = (event, payload) => page.evaluate(([e, p]) => window.__emit(e, p), [event, payload]);
  await emit("system://metrics", { cpu_percent: 14.2, ram_used_bytes: 9.6 * 1024 ** 3, ram_total_bytes: 32 * 1024 ** 3 });
  await emit("voice://transcript", { text: "Джарвис, сколько времени?", final: true, lang: "ru" });
  await emit("assistant://message", { text: "Сейчас двадцать один тридцать два. Хотите, поставлю напоминание?", lang: "ru" });
  await emit("action://result", { action_id: "a1", ok: true, detail: "info.time выполнено" });
  await emit("voice://transcript", { text: "Покажи загрузку видеокарты", final: true, lang: "ru" });
  await emit("assistant://message", { text: "Видеокарта загружена на девять процентов, температура сорок семь градусов.", lang: "ru" });
  return emit;
};

const scenarios = [
  { id: "idle", query: "state=IDLE" },
  {
    id: "listening",
    query: "state=LISTENING&level=0.55",
    after: async (emit) => emit("voice://status", { active: true, device: "Microphone (USB Audio)", peak: 0.31, no_signal: false, error: null }),
  },
  { id: "speaking", query: "state=SPEAKING&level=0.6" },
  {
    id: "warning",
    query: "state=WARNING",
    settle: 900, // the alert toast only stays for a few seconds
    after: async (emit, page) => {
      await page.evaluate(() => (window.__gpuTemp = 86));
      await emit("alert://raised", {
        id: "gpu_temp",
        kind: "gpu_temp",
        severity: "warning",
        message_ru: "Температура видеокарты 86 °C",
        message_en: "GPU temperature 86 °C",
        speak: false,
        at_ms: Date.now(),
      });
    },
  },
  { id: "settings-general", query: "state=IDLE&settings=GENERAL" },
  { id: "settings-voice", query: "state=IDLE&settings=VOICE" },
  { id: "settings-visual", query: "state=SPEAKING&level=0.4&settings=VISUAL" },
  { id: "settings-system", query: "state=IDLE&settings=SYSTEM" },
  ...["AI", "PRIVACY", "MEMORY", "TOOLS", "SETUP"].map((tab) => ({ id: `settings-${tab.toLowerCase()}`, query: `state=IDLE&settings=${tab}`, extra: true })),
  {
    id: "confirm",
    query: "state=PROCESSING",
    after: async (emit) =>
      emit("confirm://request", { action_id: "c1", action: "power.shutdown", summary: "Выключить компьютер через 60 секунд", risk: "confirm" }),
  },
];

const only = process.env.ONLY?.split(",");
const browser = await chromium.launch({
  channel: "msedge",
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist", "--enable-webgl"],
});
for (const size of SIZES) {
  for (const scenario of scenarios) {
    if (only ? !only.includes(scenario.id) : scenario.extra && !process.env.ALL) continue;
    const context = await browser.newContext({ viewport: { width: size.width, height: size.height }, deviceScaleFactor: 1 });
    const page = await context.newPage();
    page.on("pageerror", (error) => console.log("PAGEERROR", scenario.id, error.message));
    await page.addInitScript(mock);
    await page.goto(`${BASE}?${scenario.query}&quality=medium`);
    await page.waitForTimeout(scenario.settle ? 2400 : 900);
    const emit = await feed(page);
    if (scenario.after) await scenario.after(emit, page);
    await page.waitForTimeout(scenario.settle ?? 2600);
    const file = `${out}/${size.name}-${scenario.id}.png`;
    await page.screenshot({ path: file });
    console.log(file);
    await context.close();
  }
}
await browser.close();
