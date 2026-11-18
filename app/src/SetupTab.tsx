import { memo, useCallback, useEffect, useState } from "react";
import {
  aiStatus,
  isTauri,
  setupInstall,
  setupOpenOllamaPage,
  setupPullModel,
  setupStatus,
  ttsSpeak,
  voiceDevices,
  type AiStatus,
  type CommandError,
  type InputDevice,
  type SetupProgress,
  type SetupStatus,
} from "./api";
import type { MessageKey } from "./i18n";
import type { AppSettings, SettingKey } from "./settings";
import { useTauriEvent } from "./useTauriEvent";
import { MicIndicator, useMicStatus } from "./VoiceTab";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

const ITEM_LABEL: Record<string, MessageKey> = {
  vosk_model: "setupItem_vosk_model",
  piper: "setupItem_piper",
  piper_voice: "setupItem_piper_voice",
  stt_env: "setupItem_stt_env",
  whisper: "setupItem_whisper",
  kokoro: "setupItem_kokoro",
  kokoro_env: "setupItem_kokoro_env",
};

/** Percent of a download, or null when the total is unknown. */
export function percent(progress: SetupProgress | undefined): number | null {
  if (!progress || progress.total <= 0) return null;
  return Math.min(100, Math.round((progress.done / progress.total) * 100));
}

/** The first-run wizard; every step can be repeated at any time. */
export const SetupTab = memo(function SetupTab({
  t,
  values,
  onChange,
}: {
  t: Translate;
  values: AppSettings;
  onChange: OnChange;
}) {
  const mic = useMicStatus();
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [devices, setDevices] = useState<InputDevice[]>([]);
  const [progress, setProgress] = useState<Record<string, SetupProgress>>({});
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    if (!isTauri()) return;
    setupStatus()
      .then(setStatus)
      .catch((failure: CommandError) => setError(failure.message));
    aiStatus()
      .then(setAi)
      .catch(() => setAi(null));
  }, []);

  useEffect(() => {
    refresh();
    if (!isTauri()) return;
    voiceDevices()
      .then(setDevices)
      .catch(() => undefined);
  }, [refresh]);

  const onProgress = useCallback(
    (payload: SetupProgress) => {
      setProgress((current) => ({ ...current, [payload.item]: payload }));
      if (payload.state !== "running") refresh();
    },
    [refresh],
  );
  useTauriEvent("setup://progress", onProgress);

  const run = (action: () => Promise<void>) => {
    setError(null);
    action().catch((failure: CommandError) => setError(failure.message));
  };

  const device = values["voice.input_device"];
  const model = status?.default_model ?? "qwen3:4b";
  const hasModel = ai?.models.some((entry) => entry.name === model) ?? false;
  const pull = progress["ollama_model"];

  return (
    <div className="setup">
      <p className="muted">{t("setupIntro")}</p>
      <h4>{t("setupModels")}</h4>
      <p className="muted">{t("setupVerified")}</p>
      {status && (
        <p className="muted mono">
          {t("setupModelsDir")}: {status.models_dir}
        </p>
      )}
      <ul className="setup-items">
        {status?.items.map((item) => {
          const current = progress[item.id];
          const busy = item.running || current?.state === "running";
          const value = percent(current);
          return (
            <li key={item.id} data-item={item.id} data-installed={item.installed}>
              <span>
                {t(ITEM_LABEL[item.id] ?? "setupModels")}
                <small className="muted block">
                  ~{item.approx_mb} MB{item.optional ? ` · ${t("setupOptional")}` : ""}
                </small>
              </span>
              <span className="mono">{item.installed ? t("setupInstalled") : t("setupMissing")}</span>
              {!item.installed && (
                <button type="button" disabled={busy} onClick={() => run(() => setupInstall(item.id))}>
                  {busy ? t("setupRunning") : t("setupDownload")}
                </button>
              )}
              {busy && value !== null && <progress max={100} value={value} />}
              {current && current.state !== "done" && (
                <small className={current.state === "error" ? "error block" : "muted block"}>{current.message}</small>
              )}
            </li>
          );
        })}
      </ul>
      <h4>{t("setupOllama")}</h4>
      {ai?.running ? (
        <p>
          {t("setupOllamaReady")}
          {ai.version ? ` (${ai.version})` : ""}
        </p>
      ) : (
        <>
          <p className="muted">{t("setupOllamaMissing")}</p>
          <button type="button" onClick={() => run(setupOpenOllamaPage)}>
            {t("setupOllamaPage")}
          </button>
        </>
      )}
      {ai?.running && !hasModel && (
        <button type="button" disabled={pull?.state === "running"} onClick={() => run(() => setupPullModel(model))}>
          {`${t("setupPullModel")} ${model}`}
        </button>
      )}
      {pull && pull.state !== "done" && (
        <p className={pull.state === "error" ? "error" : "muted"}>
          {pull.message}
          {percent(pull) !== null ? ` ${percent(pull)}%` : ""}
        </p>
      )}
      <h4>{t("setupMic")}</h4>
      <p className="muted">{t("setupMicHint")}</p>
      <MicIndicator t={t} mic={mic} />
      <label className="field">
        <span>{t("microphone")}</span>
        <select
          value={device ?? ""}
          onChange={(event) => onChange("voice.input_device", event.target.value === "" ? null : event.target.value)}
        >
          <option value="">{t("micDefault")}</option>
          {devices.map((entry) => (
            <option key={entry.name} value={entry.name}>
              {entry.name}
              {entry.is_default ? " ★" : ""}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>{t("voiceEnabled")}</span>
        <input
          type="checkbox"
          checked={values["voice.enabled"]}
          onChange={(event) => onChange("voice.enabled", event.target.checked)}
        />
      </label>
      <button type="button" onClick={() => run(() => ttsSpeak(t("setupTestPhrase")))}>
        {t("setupTestVoice")}
      </button>
      <div className="setup-actions">
        <button type="button" onClick={refresh}>
          {t("setupRefresh")}
        </button>
        <button
          type="button"
          className={values["setup.completed"] ? "" : "primary"}
          onClick={() => onChange("setup.completed", true)}
        >
          {values["setup.completed"] ? t("setupDone") : t("setupFinish")}
        </button>
      </div>
      <p className="muted">{t("setupStandby")}</p>
      {error && <p className="error">{error}</p>}
    </div>
  );
});
