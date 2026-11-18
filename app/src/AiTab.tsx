import { useCallback, useEffect, useState } from "react";
import {
  AI_CLOUD_KEY,
  aiStatus,
  aiUnload,
  isTauri,
  secretClear,
  secretExists,
  secretSet,
  type AiStatus,
} from "./api";
import type { MessageKey } from "./i18n";
import {
  MAX_KEEP_ALIVE_MINUTES,
  NUM_CTX_RANGE,
  NUM_THREAD_RANGE,
  validCloudUrl,
  type AppSettings,
  type SettingKey,
} from "./settings";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

function message(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

function megabytes(bytes: number): string {
  return `${Math.round(bytes / (1024 * 1024))} MB`;
}

export interface AiViewProps {
  t: Translate;
  values: AppSettings;
  status: AiStatus | null;
  keySaved: boolean;
  problem: string | null;
  onChange: OnChange;
  onRefresh: () => void;
  onUnload: () => void;
  onSaveKey: (key: string) => void;
  onDeleteKey: () => void;
}

/** Number input that commits on blur or Enter, so a half-typed number is not saved. */
function NumberField({
  label,
  value,
  min,
  max,
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const number = Number(text);
    if (Number.isInteger(number) && number >= min && number <= max) {
      if (number !== value) onCommit(number);
    } else {
      setText(String(value));
    }
  };
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type="number"
        min={min}
        max={max}
        value={text}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
        }}
      />
    </label>
  );
}

function TextField({
  label,
  value,
  placeholder,
  valid,
  onCommit,
}: {
  label: string;
  value: string;
  placeholder?: string;
  valid: (text: string) => boolean;
  onCommit: (text: string) => void;
}) {
  const [text, setText] = useState(value);
  const [bad, setBad] = useState(false);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    const trimmed = text.trim();
    if (!valid(trimmed)) {
      setBad(true);
      return;
    }
    setBad(false);
    if (trimmed !== value) onCommit(trimmed);
  };
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type="text"
        value={text}
        placeholder={placeholder}
        aria-invalid={bad}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
        }}
      />
    </label>
  );
}

/** The AI tab without backend calls: Ollama status, model, limits, unload and the optional cloud. */
export function AiView({
  t,
  values,
  status,
  keySaved,
  problem,
  onChange,
  onRefresh,
  onUnload,
  onSaveKey,
  onDeleteKey,
}: AiViewProps) {
  const [keyInput, setKeyInput] = useState("");
  const chosen = values["ai.model"];
  const installed = status?.models ?? [];
  const known = installed.some((model) => model.name === chosen);
  return (
    <div className="ai">
      <h3>{t("aiTitle")}</h3>
      <p className="muted">
        {t("aiServer")}: {status === null ? "…" : status.running ? `${t("aiRunning")} (${status.version ?? ""})` : t("aiStopped")}
      </p>
      <label className="field">
        <span>{t("aiEnabled")}</span>
        <input
          type="checkbox"
          checked={values["ai.enabled"]}
          onChange={(event) => onChange("ai.enabled", event.target.checked)}
        />
      </label>
      <label className="field">
        <span>{t("aiModel")}</span>
        <select value={chosen} onChange={(event) => onChange("ai.model", event.target.value)}>
          {!known && (
            <option value={chosen}>
              {status?.running ? `${chosen} (${t("aiModelNotInstalled")})` : chosen}
            </option>
          )}
          {installed.map((model) => (
            <option key={model.name} value={model.name}>
              {`${model.name} · ${megabytes(model.size_bytes)}`}
            </option>
          ))}
        </select>
      </label>
      <NumberField
        label={t("aiKeepAlive")}
        value={values["ai.keep_alive_minutes"]}
        min={0}
        max={MAX_KEEP_ALIVE_MINUTES}
        onCommit={(value) => onChange("ai.keep_alive_minutes", value)}
      />
      <small className="muted block">{t("aiKeepAliveHint")}</small>
      <NumberField
        label={t("aiNumCtx")}
        value={values["ai.num_ctx"]}
        min={NUM_CTX_RANGE.min}
        max={NUM_CTX_RANGE.max}
        onCommit={(value) => onChange("ai.num_ctx", value)}
      />
      <NumberField
        label={t("aiNumThread")}
        value={values["ai.num_thread"]}
        min={NUM_THREAD_RANGE.min}
        max={NUM_THREAD_RANGE.max}
        onCommit={(value) => onChange("ai.num_thread", value)}
      />
      <p>
        {t("aiLoaded")}:{" "}
        {status && status.loaded.length > 0
          ? status.loaded.map((model) => `${model.name} (VRAM ${megabytes(model.vram_bytes)})`).join(", ")
          : t("aiNothingLoaded")}
      </p>
      <div className="row">
        <button type="button" onClick={onUnload}>
          {t("aiUnload")}
        </button>
        <button type="button" onClick={onRefresh}>
          {t("aiRefresh")}
        </button>
      </div>

      <h3 className="section-title">{t("aiCloudTitle")}</h3>
      <label className="field">
        <span>{t("aiCloudEnabled")}</span>
        <input
          type="checkbox"
          checked={values["ai.cloud_enabled"]}
          onChange={(event) => onChange("ai.cloud_enabled", event.target.checked)}
        />
      </label>
      <small className="muted block">{t("aiCloudHint")}</small>
      <TextField
        label={t("aiCloudUrl")}
        value={values["ai.cloud_base_url"]}
        placeholder="https://api.example.com/v1"
        valid={validCloudUrl}
        onCommit={(text) => onChange("ai.cloud_base_url", text)}
      />
      <TextField
        label={t("aiCloudModel")}
        value={values["ai.cloud_model"]}
        valid={(text) => text === "" || /^[A-Za-z0-9._:/-]{1,100}$/.test(text)}
        onCommit={(text) => onChange("ai.cloud_model", text)}
      />
      <div className="field">
        <span>{t("aiCloudKey")}</span>
        <input
          type="password"
          value={keyInput}
          autoComplete="off"
          aria-label={t("aiCloudKey")}
          onChange={(event) => setKeyInput(event.target.value)}
        />
      </div>
      <div className="row">
        <button
          type="button"
          disabled={keyInput.trim() === ""}
          onClick={() => {
            // The field is emptied at once: the value lives in the credential store from now on.
            const key = keyInput.trim();
            setKeyInput("");
            onSaveKey(key);
          }}
        >
          {t("aiCloudKeySave")}
        </button>
        <button type="button" disabled={!keySaved} onClick={onDeleteKey}>
          {t("aiCloudKeyDelete")}
        </button>
        <small className="muted">{keySaved ? t("ttsKeySaved") : t("ttsKeyMissing")}</small>
      </div>
      {problem && <p className="error">{problem}</p>}
    </div>
  );
}

export function AiTab({ t, values, onChange }: { t: Translate; values: AppSettings; onChange: OnChange }) {
  const [status, setStatus] = useState<AiStatus | null>(null);
  const [keySaved, setKeySaved] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const refresh = useCallback(() => {
    if (!isTauri()) return;
    aiStatus()
      .then(setStatus)
      .catch((error: unknown) => setProblem(message(error)));
    secretExists(AI_CLOUD_KEY)
      .then(setKeySaved)
      .catch(() => undefined);
  }, []);

  useEffect(refresh, [refresh]);

  return (
    <AiView
      t={t}
      values={values}
      status={status}
      keySaved={keySaved}
      problem={problem}
      onChange={onChange}
      onRefresh={refresh}
      onUnload={() => {
        aiUnload()
          .then(() => {
            setProblem(null);
            refresh();
          })
          .catch((error: unknown) => setProblem(message(error)));
      }}
      onSaveKey={(key) => {
        secretSet(AI_CLOUD_KEY, key)
          .then(() => {
            setProblem(null);
            refresh();
          })
          .catch((error: unknown) => setProblem(message(error)));
      }}
      onDeleteKey={() => {
        secretClear(AI_CLOUD_KEY)
          .then(() => {
            setProblem(null);
            refresh();
          })
          .catch((error: unknown) => setProblem(message(error)));
      }}
    />
  );
}
