import { useState, type FormEvent, type ReactNode } from "react";
import { chatSend, emergencyStop, ptt, type CommandError, type MicStatus } from "./api";
import { Icon } from "./icons";
import type { MessageKey } from "./i18n";
import { MicIndicator, useMicStatus } from "./VoiceTab";

type Translate = (key: MessageKey) => string;

export interface CommandBarViewProps {
  t: Translate;
  text: string;
  reply: string | null;
  error: string | null;
  busy: boolean;
  onText: (text: string) => void;
  onSubmit: () => void;
  onEmergencyStop: () => void;
  /** The push-to-talk button, rendered before the field. */
  lead?: ReactNode;
  /** Microphone state line under the field. */
  meta?: ReactNode;
}

/** The text command field (works without a microphone), the last reply and the emergency button. */
export function CommandBarView({ t, text, reply, error, busy, onText, onSubmit, onEmergencyStop, lead, meta }: CommandBarViewProps) {
  const submit = (event: FormEvent) => {
    event.preventDefault();
    onSubmit();
  };
  return (
    <section className="command">
      <form className="dock-row" onSubmit={submit}>
        {lead}
        <input
          type="text"
          value={text}
          maxLength={500}
          placeholder={t("commandPlaceholder")}
          aria-label={t("commandPlaceholder")}
          onChange={(event) => onText(event.target.value)}
        />
        <button type="submit" className="primary dock-send" disabled={busy || text.trim().length === 0}>
          <Icon name="send" />
          <span className="btn-label">{t("send")}</span>
        </button>
        <button type="button" className="danger dock-stop" onClick={onEmergencyStop}>
          <Icon name="stop" />
          <span className="btn-label">{t("emergencyStop")}</span>
        </button>
      </form>
      {meta}
      {reply && <p className="reply dock-reply">{reply}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

/** Push-to-talk: hold to speak. The ring shows that the microphone is open. */
function PttButton({ t, mic }: { t: Translate; mic: MicStatus | null }) {
  const live = mic?.active === true && !mic.error;
  return (
    <button
      type="button"
      className={live ? "ptt ptt-live" : "ptt"}
      aria-label={t("pttHold")}
      title={t("pttHold")}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        void ptt(true).catch(() => undefined);
      }}
      onPointerUp={() => void ptt(false).catch(() => undefined)}
      onPointerCancel={() => void ptt(false).catch(() => undefined)}
    >
      <Icon name="mic" size={22} />
    </button>
  );
}

export function CommandBar({ t }: { t: Translate }) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const mic = useMicStatus();

  const submit = () => {
    const command = text.trim();
    if (!command) return;
    setBusy(true);
    setError(null);
    chatSend(command)
      .then(() => setText(""))
      .catch((failure: CommandError) => setError(failure.message))
      .finally(() => setBusy(false));
  };
  return (
    <CommandBarView
      t={t}
      text={text}
      reply={null}
      error={error}
      busy={busy}
      onText={setText}
      onSubmit={submit}
      onEmergencyStop={() => void emergencyStop().catch(() => undefined)}
      lead={<PttButton t={t} mic={mic} />}
      meta={<MicIndicator t={t} mic={mic} />}
    />
  );
}
