import { useCallback, useEffect, useState } from "react";
import type { AlertPayload, CapturePayload } from "./api";
import type { MessageKey } from "./i18n";
import type { Language } from "./settings";
import { alertText, toastMillis } from "./system";
import { useTauriEvent } from "./useTauriEvent";

type Translate = (key: MessageKey) => string;

/** A red dot and "Analysing the screen" for as long as the backend captures or analyses the screen. */
export function CaptureIndicatorView({ t, active }: { t: Translate; active: boolean }) {
  if (!active) return null;
  return (
    <span className="capture-indicator" role="status" aria-live="polite">
      <span className="capture-dot" aria-hidden="true" />
      {t("captureActive")}
    </span>
  );
}

export function CaptureIndicator({ t }: { t: Translate }) {
  const [active, setActive] = useState(false);
  const onCapture = useCallback((payload: CapturePayload) => setActive(payload.active === true), []);
  useTauriEvent("vision://capture", onCapture);
  return <CaptureIndicatorView t={t} active={active} />;
}

export function AlertToastView({ alert, language }: { alert: AlertPayload | null; language: Language }) {
  if (!alert) return null;
  return (
    <span className={`alert-toast alert-${alert.severity}`} role="alert">
      {alertText(alert, language)}
    </span>
  );
}

/** The newest alert in the header for a few seconds. */
export function AlertToast({ language }: { language: Language }) {
  const [alert, setAlert] = useState<AlertPayload | null>(null);
  const onAlert = useCallback((payload: AlertPayload) => setAlert(payload), []);
  useTauriEvent("alert://raised", onAlert);
  useEffect(() => {
    if (!alert) return undefined;
    const timer = window.setTimeout(() => setAlert(null), toastMillis(alert));
    return () => window.clearTimeout(timer);
  }, [alert]);
  return <AlertToastView alert={alert} language={language} />;
}
