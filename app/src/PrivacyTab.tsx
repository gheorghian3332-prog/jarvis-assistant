import { useState } from "react";
import { isTauri, memoryClearAll, memoryClearHistory } from "./api";
import type { MessageKey } from "./i18n";
import { MAX_HISTORY_DAYS, type AppSettings, type SettingKey } from "./settings";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

export interface PrivacyViewProps {
  t: Translate;
  values: AppSettings;
  /** `true` after the first press of "Delete all memory": the next press confirms. */
  armed: boolean;
  notice: string | null;
  onChange: OnChange;
  onClearHistory: () => void;
  onDeleteAll: () => void;
}

export function PrivacyView({ t, values, armed, notice, onChange, onClearHistory, onDeleteAll }: PrivacyViewProps) {
  const [days, setDays] = useState(String(values["privacy.history_days"]));
  const commitDays = () => {
    const number = Number(days);
    if (Number.isInteger(number) && number >= 1 && number <= MAX_HISTORY_DAYS) {
      if (number !== values["privacy.history_days"]) onChange("privacy.history_days", number);
    } else {
      setDays(String(values["privacy.history_days"]));
    }
  };
  const [model, setModel] = useState(values["vision.vlm_model"]);
  const commitModel = () => {
    const name = model.trim();
    if (name !== values["vision.vlm_model"]) onChange("vision.vlm_model", name);
  };
  return (
    <div className="privacy">
      <h3>{t("privacyTitle")}</h3>
      <label className="field">
        <span>{t("privacyHistory")}</span>
        <input
          type="checkbox"
          checked={values["privacy.history_enabled"]}
          onChange={(event) => onChange("privacy.history_enabled", event.target.checked)}
        />
      </label>
      <label className="field">
        <span>{t("privacyDays")}</span>
        <input
          type="number"
          min={1}
          max={MAX_HISTORY_DAYS}
          value={days}
          onChange={(event) => setDays(event.target.value)}
          onBlur={commitDays}
          onKeyDown={(event) => {
            if (event.key === "Enter") commitDays();
          }}
        />
      </label>
      <h3>{t("visionTitle")}</h3>
      <label className="field">
        <span>{t("visionEnabled")}</span>
        <input
          type="checkbox"
          checked={values["vision.enabled"]}
          onChange={(event) => onChange("vision.enabled", event.target.checked)}
        />
      </label>
      <label className="field">
        <span>{t("visionAllowCloud")}</span>
        <input
          type="checkbox"
          checked={values["vision.allow_cloud"]}
          onChange={(event) => onChange("vision.allow_cloud", event.target.checked)}
        />
      </label>
      <label className="field">
        <span>{t("visionModel")}</span>
        <input
          type="text"
          value={model}
          onChange={(event) => setModel(event.target.value)}
          onBlur={commitModel}
          onKeyDown={(event) => {
            if (event.key === "Enter") commitModel();
          }}
        />
      </label>
      <div className="row">
        <button type="button" onClick={onClearHistory}>
          {t("privacyClearHistory")}
        </button>
        <button type="button" className={armed ? "danger" : undefined} onClick={onDeleteAll}>
          {armed ? t("privacyConfirmAll") : t("privacyDeleteAll")}
        </button>
      </div>
      {notice && <p className="muted">{notice}</p>}
    </div>
  );
}

export function PrivacyTab({ t, values, onChange }: { t: Translate; values: AppSettings; onChange: OnChange }) {
  const [armed, setArmed] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const fail = (error: unknown) =>
    setNotice(typeof error === "object" && error !== null && "message" in error ? String((error as { message: unknown }).message) : String(error));
  return (
    <PrivacyView
      t={t}
      values={values}
      armed={armed}
      notice={notice}
      onChange={onChange}
      onClearHistory={() => {
        if (!isTauri()) return;
        memoryClearHistory()
          .then(() => setNotice(t("privacyDone")))
          .catch(fail);
      }}
      onDeleteAll={() => {
        if (!armed) {
          setArmed(true);
          return;
        }
        setArmed(false);
        if (!isTauri()) return;
        memoryClearAll(true)
          .then(() => setNotice(t("privacyDone")))
          .catch(fail);
      }}
    />
  );
}
