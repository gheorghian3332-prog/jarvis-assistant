import { useCallback, useEffect, useState } from "react";
import {
  confirmResponse,
  type ActionResultPayload,
  type CommandError,
  type ConfirmRequest,
} from "./api";
import { CONFIRM_SECONDS, fractionLeft, secondsLeft } from "./confirm";
import { Icon } from "./icons";
import type { MessageKey } from "./i18n";
import { useTauriEvent } from "./useTauriEvent";

type Translate = (key: MessageKey) => string;

export interface ConfirmViewProps {
  t: Translate;
  request: ConfirmRequest;
  seconds: number;
  fraction: number;
  error: string | null;
  onAnswer: (approved: boolean) => void;
}

/** The modal itself: no timers, no backend calls (those live in `ConfirmDialog`). */
export function ConfirmView({ t, request, seconds, fraction, error, onAnswer }: ConfirmViewProps) {
  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="confirm-title">
        <h2 id="confirm-title" className="modal-title">
          <Icon name="stop" size={20} />
          {t("confirmTitle")}
        </h2>
        <p className="confirm-summary">{request.summary}</p>
        <p className="muted mono confirm-meta">
          {request.action} · {request.risk}
        </p>
        <div className="bar" aria-hidden="true">
          <div className="bar-fill" style={{ width: `${Math.round(fraction * 100)}%` }} />
        </div>
        <p className="muted confirm-left">
          {t("confirmLeft")}: <span className="mono confirm-seconds">{seconds}</span>
        </p>
        {error && <p className="error">{error}</p>}
        <div className="modal-actions">
          <button type="button" className="primary" onClick={() => onAnswer(true)}>
            {t("confirmRun")}
          </button>
          <button type="button" autoFocus onClick={() => onAnswer(false)}>
            {t("confirmCancel")}
          </button>
        </div>
      </div>
    </div>
  );
}

interface Active {
  request: ConfirmRequest;
  deadline: number;
}

/** Shows the confirmation request from the backend and sends the answer. */
export function ConfirmDialog({ t }: { t: Translate }) {
  const [active, setActive] = useState<Active | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [error, setError] = useState<string | null>(null);

  const onRequest = useCallback((request: ConfirmRequest) => {
    setError(null);
    setNow(Date.now());
    setActive({ request, deadline: Date.now() + CONFIRM_SECONDS * 1000 });
  }, []);
  const onResult = useCallback((result: ActionResultPayload) => {
    // Answered elsewhere (voice), timed out or stopped: the dialog goes away.
    setActive((current) => (current?.request.action_id === result.action_id ? null : current));
  }, []);
  useTauriEvent("confirm://request", onRequest);
  useTauriEvent("action://result", onResult);

  const deadline = active?.deadline ?? null;
  useEffect(() => {
    if (deadline === null) return undefined;
    const timer = window.setInterval(() => {
      const current = Date.now();
      setNow(current);
      if (current >= deadline) setActive(null);
    }, 250);
    return () => window.clearInterval(timer);
  }, [deadline]);

  if (!active) return null;
  const answer = (approved: boolean) => {
    confirmResponse(active.request.action_id, approved)
      .then(() => setActive(null))
      .catch((failure: CommandError) => {
        // Already answered by voice or expired: nothing is left to decide.
        if (failure.code === "NOT_FOUND") setActive(null);
        else setError(failure.message);
      });
  };
  return (
    <ConfirmView
      t={t}
      request={active.request}
      seconds={secondsLeft(active.deadline, now)}
      fraction={fractionLeft(active.deadline, now)}
      error={error}
      onAnswer={answer}
    />
  );
}
