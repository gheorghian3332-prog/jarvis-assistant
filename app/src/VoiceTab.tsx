import { memo, useEffect, useState } from "react";
import {
  isTauri,
  listenEvent,
  ptt,
  voiceDevices,
  voiceStatus,
  type InputDevice,
  type MicStatus,
  type TranscriptPayload,
} from "./api";
import type { MessageKey } from "./i18n";
import { parseWakeNames, validWakeNames, type AppSettings, type SettingKey } from "./settings";
import { TtsSection } from "./TtsSection";
import { shortcutFromKeyEvent } from "./voiceKeys";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

/** Microphone indicator: closed, open, or a warning when the device delivers nothing but zeros. */
export function MicIndicator({ t, mic }: { t: Translate; mic: MicStatus | null }) {
  let label = t("micClosed");
  let tone = "mic-off";
  if (mic?.error) {
    label = `${t("micError")}: ${mic.error}`;
    tone = "mic-warn";
  } else if (mic?.active) {
    label = mic.no_signal ? t("micNoSignal") : t("micOpen");
    tone = mic.no_signal ? "mic-warn" : "mic-on";
  }
  return (
    <div className={`mic ${tone}`} role="status">
      <span className="mic-dot" aria-hidden="true" />
      <span>{label}</span>
      {mic?.active && mic.peak !== null && !mic.error && (
        <small className="muted mono">
          {mic.device} · {t("micPeak")} {mic.peak.toFixed(3)}
        </small>
      )}
    </div>
  );
}

/** Keeps the microphone status current: one read when mounted, then `voice://status` events. */
export function useMicStatus(): MicStatus | null {
  const [mic, setMic] = useState<MicStatus | null>(null);
  useEffect(() => {
    if (!isTauri()) return undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    voiceStatus()
      .then((status) => {
        if (!disposed) setMic((current) => current ?? status.mic);
      })
      .catch(() => undefined);
    void listenEvent<MicStatus>("voice://status", (status) => setMic(status)).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  return mic;
}

/** The last recognized phrase and its language, from `voice://transcript` events. */
function useLastTranscript(): TranscriptPayload | null {
  const [last, setLast] = useState<TranscriptPayload | null>(null);
  useEffect(() => {
    if (!isTauri()) return undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenEvent<TranscriptPayload>("voice://transcript", (payload) => {
      if (payload.final) setLast(payload);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  return last;
}

export const VoiceTab = memo(function VoiceTab({
  t,
  values,
  onChange,
  error,
}: {
  t: Translate;
  values: AppSettings;
  onChange: OnChange;
  error: string | null;
}) {
  const mic = useMicStatus();
  const transcript = useLastTranscript();
  const [devices, setDevices] = useState<InputDevice[]>([]);
  const [namesText, setNamesText] = useState(values["voice.wake_names"].join(", "));
  const [namesError, setNamesError] = useState(false);
  const stored = values["voice.wake_names"].join(", ");

  useEffect(() => {
    setNamesText(stored);
    setNamesError(false);
  }, [stored]);

  useEffect(() => {
    if (!isTauri()) return undefined;
    let cancelled = false;
    voiceDevices()
      .then((list) => {
        if (!cancelled) setDevices(list);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  const commitNames = () => {
    const names = parseWakeNames(namesText);
    if (!validWakeNames(names)) {
      setNamesError(true);
      return;
    }
    setNamesError(false);
    if (names.join(",") !== values["voice.wake_names"].join(",")) onChange("voice.wake_names", names);
  };

  const device = values["voice.input_device"];
  const known = device === null || devices.some((item) => item.name === device);

  return (
    <div className="voice">
      <MicIndicator t={t} mic={mic} />
      <p className="transcript" aria-live="polite">
        <span className="muted">{t("lastTranscript")}: </span>
        {transcript ? (
          <>
            <span className="mono">{transcript.text}</span>
            <small className="muted"> ({transcript.lang || "?"})</small>
          </>
        ) : (
          <span className="muted">{t("noTranscript")}</span>
        )}
      </p>
      <label className="field">
        <span>
          {t("voiceEnabled")}
          <small className="muted block">{t("voiceEnabledHint")}</small>
        </span>
        <input
          type="checkbox"
          checked={values["voice.enabled"]}
          onChange={(event) => onChange("voice.enabled", event.target.checked)}
        />
      </label>
      <label className="field">
        <span>{t("microphone")}</span>
        <select
          value={device ?? ""}
          onChange={(event) => onChange("voice.input_device", event.target.value === "" ? null : event.target.value)}
        >
          <option value="">{t("micDefault")}</option>
          {!known && device !== null && <option value={device}>{device}</option>}
          {devices.map((item) => (
            <option key={item.name} value={item.name}>
              {item.name}
              {item.is_default ? " ★" : ""}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>
          {t("sensitivity")} <span className="mono">{values["voice.sensitivity"].toFixed(2)}</span>
        </span>
        <input
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={values["voice.sensitivity"]}
          onChange={(event) => onChange("voice.sensitivity", Number(event.target.value))}
        />
      </label>
      <label className="field">
        <span>
          {t("wakeNames")}
          <small className="muted block">{t("wakeNamesHint")}</small>
        </span>
        <input
          type="text"
          value={namesText}
          maxLength={120}
          aria-invalid={namesError}
          onChange={(event) => setNamesText(event.target.value)}
          onBlur={commitNames}
          onKeyDown={(event) => {
            if (event.key === "Enter") commitNames();
          }}
        />
      </label>
      {namesError && <p className="error">{t("invalidNames")}</p>}
      <label className="field">
        <span>
          {t("pttKey")}
          <small className="muted block">{t("pttKeyHint")}</small>
        </span>
        <input
          type="text"
          readOnly
          className="mono key-capture"
          value={values["voice.ptt_key"]}
          onKeyDown={(event) => {
            event.preventDefault();
            const shortcut = shortcutFromKeyEvent(event.nativeEvent);
            if (shortcut === null) return;
            onChange("voice.ptt_key", shortcut);
          }}
        />
      </label>
      <button
        type="button"
        className="ptt-button"
        disabled={!isTauri()}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          void ptt(true).catch(() => undefined);
        }}
        onPointerUp={() => void ptt(false).catch(() => undefined)}
        onPointerCancel={() => void ptt(false).catch(() => undefined)}
      >
        {t("pttHold")}
      </button>
      <TtsSection t={t} values={values} onChange={onChange} />
      {error && <p className="error">{error}</p>}
    </div>
  );
});
