import { memo, useCallback, useEffect, useRef, useState } from "react";
import {
  isTauri,
  setWindowMode,
  type ActionResultPayload,
  type AlertPayload,
  type AssistantMessage,
  type AssistantState,
  type MetricsPayload,
  type TranscriptPayload,
  type WindowMode,
} from "./api";
import { formatBytes, formatMetrics } from "./format";
import { Icon } from "./icons";
import type { MessageKey } from "./i18n";
import { AlertToast, CaptureIndicator } from "./Overlays";
import type { Language } from "./settings";
import { useSystemSnapshot } from "./SystemTab";
import { alertText, formatRate, formatTemperature } from "./system";
import { useTauriEvent } from "./useTauriEvent";

type Translate = (key: MessageKey) => string;

const STATE_KEY: Record<AssistantState, MessageKey> = {
  IDLE: "stateIDLE",
  LISTENING: "stateLISTENING",
  PROCESSING: "statePROCESSING",
  SPEAKING: "stateSPEAKING",
  EXECUTING: "stateEXECUTING",
  WARNING: "stateWARNING",
  ERROR: "stateERROR",
  OFFLINE: "stateOFFLINE",
};

export function stateLabel(t: Translate, state: AssistantState): string {
  return t(STATE_KEY[state]);
}

/** A boolean that follows a media query. */
export function useMedia(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const media = window.matchMedia(query);
    const update = () => setMatches(media.matches);
    media.addEventListener("change", update);
    update();
    return () => media.removeEventListener("change", update);
  }, [query]);
  return matches;
}

// ---------- header ----------

export function Header({
  t,
  language,
  state,
  settingsOpen,
  onSettings,
}: {
  t: Translate;
  language: Language;
  state: AssistantState;
  settingsOpen: boolean;
  onSettings: () => void;
}) {
  const switchMode = (mode: WindowMode) => {
    void setWindowMode(mode).catch(() => undefined);
  };
  const modes: { mode: WindowMode; key: MessageKey; icon: "compact" | "full" | "tray" }[] = [
    { mode: "compact", key: "compact", icon: "compact" },
    { mode: "full", key: "full", icon: "full" },
    { mode: "tray", key: "tray", icon: "tray" },
  ];
  return (
    <header className="header" data-tauri-drag-region>
      <span className="brand" data-tauri-drag-region>
        <span className="brand-mark" aria-hidden="true" />
        J.A.R.V.I.S.
      </span>
      <span className="state-chip" role="status" data-tauri-drag-region>
        <span className="state-dot" aria-hidden="true" />
        {stateLabel(t, state).replace("…", "")}
      </span>
      {isTauri() && <CaptureIndicator t={t} />}
      {isTauri() && <AlertToast language={language} />}
      <span className="header-spacer" data-tauri-drag-region />
      <nav className="modes" aria-label={t("hudWindowMode")}>
        {isTauri() &&
          modes.map((item) => (
            <button
              key={item.mode}
              type="button"
              className="icon-button"
              data-mode-btn={item.mode}
              title={t(item.key)}
              aria-label={t(item.key)}
              onClick={() => switchMode(item.mode)}
            >
              <Icon name={item.icon} />
            </button>
          ))}
        <button
          type="button"
          className={settingsOpen ? "icon-button active" : "icon-button"}
          title={t("hudOpenSettings")}
          aria-label={t("hudOpenSettings")}
          aria-haspopup="dialog"
          aria-expanded={settingsOpen}
          onClick={onSettings}
        >
          <Icon name="gear" />
        </button>
      </nav>
    </header>
  );
}

// ---------- core decoration ----------

/** Thin static rings around the core; they carry the state colour and never animate. */
export const CoreRings = memo(function CoreRings() {
  const ticks = Array.from({ length: 72 }, (_, index) => index);
  return (
    <svg className="core-rings" viewBox="-200 -200 400 400" aria-hidden="true" focusable="false">
      <circle className="ring-line" r="192" />
      <circle className="ring-line faint" r="164" />
      <g className="ring-ticks">
        {ticks.map((index) => {
          const angle = (index / 72) * Math.PI * 2;
          const long = index % 6 === 0;
          const r1 = 192;
          const r2 = long ? 180 : 186;
          return (
            <line
              key={index}
              className={long ? "tick long" : "tick"}
              x1={Math.cos(angle) * r1}
              y1={Math.sin(angle) * r1}
              x2={Math.cos(angle) * r2}
              y2={Math.sin(angle) * r2}
            />
          );
        })}
      </g>
      <path className="ring-arc" d="M -150 0 A 150 150 0 0 1 0 -150" />
      <path className="ring-arc" d="M 150 0 A 150 150 0 0 1 0 150" />
    </svg>
  );
});

// ---------- subtitles ----------

/** Status line of the core, what was recognised and what the assistant answered. */
export const Subtitles = memo(function Subtitles({
  t,
  state,
  preview,
}: {
  t: Translate;
  state: AssistantState;
  preview: boolean;
}) {
  const [heard, setHeard] = useState<TranscriptPayload | null>(null);
  const [reply, setReply] = useState<{ text: string; n: number } | null>(null);
  const counter = useRef(0);
  const onTranscript = useCallback((payload: TranscriptPayload) => {
    if (payload.final) setHeard(payload);
  }, []);
  const onMessage = useCallback((message: AssistantMessage) => {
    counter.current += 1;
    setReply({ text: message.text, n: counter.current });
  }, []);
  useTauriEvent("voice://transcript", onTranscript);
  useTauriEvent("assistant://message", onMessage);
  return (
    <section className="subtitles" aria-label={t("hudEvents")}>
      <p className="core-status" role="status">
        {stateLabel(t, state)}
        {preview && <span className="preview-chip">{t("previewLabel")}</span>}
      </p>
      {heard && (
        <p className="heard">
          <span className="heard-label">{t("hudYouSaid")}</span> {heard.text}
        </p>
      )}
      {reply ? (
        <p className="reply" key={reply.n} aria-live="polite">
          {reply.text}
        </p>
      ) : (
        <p className="hint">{t("hudSubtitleHint")}</p>
      )}
    </section>
  );
});

// ---------- gauges ----------

const RING_RADIUS = 26;
const RING_LENGTH = 2 * Math.PI * RING_RADIUS;

/** A circular gauge without any transition: its value changes every second or two. */
function Gauge({ label, percent, text, warn }: { label: string; percent: number | null; text: string; warn?: boolean }) {
  const value = percent === null || !Number.isFinite(percent) ? 0 : Math.min(100, Math.max(0, percent));
  return (
    <div className={warn ? "gauge warn" : "gauge"} role="img" aria-label={`${label} ${text}`}>
      <svg viewBox="0 0 64 64" aria-hidden="true" focusable="false">
        <circle className="gauge-track" cx="32" cy="32" r={RING_RADIUS} />
        <circle
          className="gauge-value"
          cx="32"
          cy="32"
          r={RING_RADIUS}
          strokeDasharray={`${(value / 100) * RING_LENGTH} ${RING_LENGTH}`}
          transform="rotate(-90 32 32)"
        />
      </svg>
      <span className="gauge-text mono">{text}</span>
      <span className="gauge-label">{label}</span>
    </div>
  );
}

function Line({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className={warn ? "hud-row warn" : "hud-row"}>
      <span>{label}</span>
      <span className="mono">{value}</span>
    </div>
  );
}

/** The 1 Hz `system://metrics` stream; only the component that calls this re-renders on a tick. */
function useMetrics(): MetricsPayload | null {
  const [metrics, setMetrics] = useState<MetricsPayload | null>(null);
  const onMetrics = useCallback((payload: MetricsPayload) => setMetrics(payload), []);
  useTauriEvent("system://metrics", onMetrics);
  return metrics;
}

/** CPU, RAM and the microphone in one line for the compact window. */
export const MiniMetrics = memo(function MiniMetrics({ t }: { t: Translate }) {
  const metrics = useMetrics();
  const view = metrics ? formatMetrics(metrics) : null;
  return (
    <div className="mini-metrics" aria-label={t("hudSystem")}>
      <span className="mini">
        <span className="mini-label">{t("cpu")}</span>
        <span className="mono">{view ? view.cpuText : "—"}</span>
        <span className="mini-bar" aria-hidden="true">
          <span style={{ width: `${view?.cpuPercent ?? 0}%` }} />
        </span>
      </span>
      <span className="mini">
        <span className="mini-label">{t("ram")}</span>
        <span className="mono">{view ? view.ramText : "—"}</span>
        <span className="mini-bar" aria-hidden="true">
          <span style={{ width: `${view?.ramPercent ?? 0}%` }} />
        </span>
      </span>
    </div>
  );
});

/** Left column of the full window: CPU and RAM from the event stream, GPU and network from a slow snapshot poll. */
export const SystemColumn = memo(function SystemColumn({ t }: { t: Translate }) {
  const metrics = useMetrics();
  const snapshot = useSystemSnapshot(SNAPSHOT_POLL_MS);
  const view = metrics ? formatMetrics(metrics) : null;
  const gpu = snapshot?.metrics.gpu ?? null;
  const net = snapshot?.metrics.network ?? null;
  const gpuHot = gpu?.temperature_c != null && gpu.temperature_c >= 83;
  let link = "—";
  if (net) {
    if (net.link_up === false) link = t("sysLinkDown");
    else if (net.internet_reachable === true) link = t("sysOnline");
    else if (net.internet_reachable === false) link = t("sysOffline");
  }
  return (
    <section className="panel hud-system" aria-label={t("hudSystem")}>
      <h2 className="panel-title">{t("hudSystem")}</h2>
      <div className="gauges">
        <Gauge label={t("cpu")} percent={view?.cpuPercent ?? null} text={view ? `${Math.round(view.cpuPercent)}` : "—"} />
        <Gauge
          label={t("hudGpu")}
          percent={gpu?.utilization_percent ?? null}
          text={gpu?.utilization_percent == null ? "—" : `${Math.round(gpu.utilization_percent)}`}
          warn={gpuHot}
        />
      </div>
      <div className="hud-rows">
        <Line label="GPU t°" value={formatTemperature(gpu?.temperature_c ?? null)} warn={gpuHot} />
        <Line label="CPU t°" value={formatTemperature(snapshot?.metrics.cpu.temperature_c ?? null)} />
        <Line label={t("ram")} value={view ? view.ramText : "—"} />
      </div>
      {view && (
        <div className="hud-bar" aria-hidden="true">
          <span style={{ width: `${view.ramPercent}%` }} />
        </div>
      )}
      <h3 className="panel-sub">{t("hudNet")}</h3>
      <div className="hud-rows">
        <Line label={t("sysLink")} value={link} warn={net?.internet_reachable === false || net?.link_up === false} />
        <Line label={t("sysDown")} value={formatRate(net?.down_bytes_per_sec ?? null)} />
        <Line label={t("sysUp")} value={formatRate(net?.up_bytes_per_sec ?? null)} />
      </div>
      {!metrics && !snapshot && <p className="muted small">{t("waiting")}</p>}
      {snapshot && gpu === null && <p className="muted small">{t("sysGpuUnavailable")}</p>}
      {snapshot && gpu?.vram_total_bytes != null && gpu.vram_used_bytes != null && (
        <p className="muted small mono">{`VRAM ${formatBytes(gpu.vram_used_bytes)} / ${formatBytes(gpu.vram_total_bytes)}`}</p>
      )}
    </section>
  );
});

/** GPU and network change slowly enough for a 2 s poll; the SYSTEM section polls once a second on its own. */
const SNAPSHOT_POLL_MS = 2000;

// ---------- event feed ----------

type FeedKind = "you" | "assistant" | "alert" | "action";

interface FeedEntry {
  id: number;
  kind: FeedKind;
  text: string;
  at: number;
  tone?: "ok" | "warning" | "critical" | "info" | "bad";
}

const FEED_LIMIT = 40;

export const EventFeed = memo(function EventFeed({ t, language }: { t: Translate; language: Language }) {
  const [entries, setEntries] = useState<FeedEntry[]>([]);
  const counter = useRef(0);
  const push = useCallback((entry: Omit<FeedEntry, "id" | "at">) => {
    counter.current += 1;
    const next: FeedEntry = { ...entry, id: counter.current, at: Date.now() };
    setEntries((current) => [next, ...current].slice(0, FEED_LIMIT));
  }, []);
  const onTranscript = useCallback(
    (payload: TranscriptPayload) => {
      if (payload.final && payload.text.trim()) push({ kind: "you", text: payload.text });
    },
    [push],
  );
  const onMessage = useCallback((message: AssistantMessage) => push({ kind: "assistant", text: message.text }), [push]);
  const onAlert = useCallback(
    (alert: AlertPayload) => push({ kind: "alert", text: alertText(alert, language), tone: alert.severity }),
    [push, language],
  );
  const onAction = useCallback(
    (result: ActionResultPayload) => push({ kind: "action", text: result.detail, tone: result.ok ? "ok" : "bad" }),
    [push],
  );
  useTauriEvent("voice://transcript", onTranscript);
  useTauriEvent("assistant://message", onMessage);
  useTauriEvent("alert://raised", onAlert);
  useTauriEvent("action://result", onAction);
  const kindLabel: Record<FeedKind, MessageKey> = {
    you: "hudYou",
    assistant: "hudAssistant",
    alert: "hudAlert",
    action: "hudAction",
  };
  return (
    <section className="panel hud-feed" aria-label={t("hudEvents")}>
      <h2 className="panel-title">{t("hudEvents")}</h2>
      {entries.length === 0 ? (
        <p className="feed-empty">{t("hudEventsEmpty")}</p>
      ) : (
        <ol className="feed">
          {entries.map((entry) => (
            <li key={entry.id} className={`feed-item kind-${entry.kind}${entry.tone ? ` tone-${entry.tone}` : ""}`}>
              <span className="feed-meta">
                <span className="feed-kind">{t(kindLabel[entry.kind])}</span>
                <time className="mono">{new Date(entry.at).toLocaleTimeString([], { hour12: false })}</time>
              </span>
              <span className="feed-text">{entry.text}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
});
