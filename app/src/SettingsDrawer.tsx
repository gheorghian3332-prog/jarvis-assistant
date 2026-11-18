import { memo, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { auditList, isTauri, type AssistantState, type AuditEntry, type CommandError } from "./api";
import { AiTab } from "./AiTab";
import { STATES } from "./core/states";
import { Icon, type IconName } from "./icons";
import type { MessageKey } from "./i18n";
import { MemoryTab } from "./MemoryTab";
import { PrivacyTab } from "./PrivacyTab";
import { SetupTab } from "./SetupTab";
import {
  FPS_LIMITS,
  LANGUAGES,
  MAX_CUSTOM_CHARS,
  PERSONALITY_MODES,
  QUALITY_SETTINGS,
  START_MODES,
  type AppSettings,
  type GeneralSettings,
  type Language,
  type PersonalityMode,
  type PersonalitySettings,
  type SettingKey,
} from "./settings";
import { SystemTab } from "./SystemTab";
import { ToolsTab } from "./ToolsTab";
import { VoiceTab } from "./VoiceTab";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

export const TABS: readonly { id: TabId; icon: IconName }[] = [
  { id: "GENERAL", icon: "general" },
  { id: "VOICE", icon: "voice" },
  { id: "AI", icon: "ai" },
  { id: "SYSTEM", icon: "system" },
  { id: "VISUAL", icon: "visual" },
  { id: "PRIVACY", icon: "privacy" },
  { id: "MEMORY", icon: "memory" },
  { id: "TOOLS", icon: "tools" },
  { id: "SETUP", icon: "setup" },
];

export type TabId =
  | "GENERAL"
  | "VOICE"
  | "AI"
  | "SYSTEM"
  | "VISUAL"
  | "PRIVACY"
  | "MEMORY"
  | "TOOLS"
  | "SETUP";

const NAV_LABEL: Record<TabId, MessageKey> = {
  GENERAL: "navGENERAL",
  VOICE: "navVOICE",
  AI: "navAI",
  SYSTEM: "navSYSTEM",
  VISUAL: "navVISUAL",
  PRIVACY: "navPRIVACY",
  MEMORY: "navMEMORY",
  TOOLS: "navTOOLS",
  SETUP: "navSETUP",
};

const NAV_HINT: Record<TabId, MessageKey> = {
  GENERAL: "hintGENERAL",
  VOICE: "hintVOICE",
  AI: "hintAI",
  SYSTEM: "hintSYSTEM",
  VISUAL: "hintVISUAL",
  PRIVACY: "hintPRIVACY",
  MEMORY: "hintMEMORY",
  TOOLS: "hintTOOLS",
  SETUP: "hintSETUP",
};

function AuditLog({ t, refreshKey }: { t: Translate; refreshKey: number }) {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return undefined;
    let cancelled = false;
    auditList(20)
      .then((list) => {
        if (!cancelled) {
          setEntries(list);
          setError(null);
        }
      })
      .catch((failure: CommandError) => {
        if (!cancelled) setError(failure.message);
      });
    return () => {
      cancelled = true;
    };
  }, [refreshKey]);
  return (
    <section className="audit group">
      <h3 className="group-title">{t("audit")}</h3>
      {error && <p className="error">{`${t("loadFailed")}: ${error}`}</p>}
      {!error && entries.length === 0 && <p className="empty">{t("auditEmpty")}</p>}
      <ul>
        {entries.map((entry) => (
          <li key={entry.id} className={entry.ok ? "" : "failed"}>
            <span className="mono ts">{entry.ts}</span>
            <span className="action">{entry.action}</span>
            <span className="muted detail">{entry.detail}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

const PERSONALITY_LABEL: Record<PersonalityMode, MessageKey> = {
  formal: "personalityFormal",
  classic: "personalityClassic",
  friendly: "personalityFriendly",
  custom: "personalityCustom",
};

/** The user's own style instructions (up to 2000 characters); saved when the field loses focus. */
function CustomInstructions({ t, stored, onCommit }: { t: Translate; stored: string; onCommit: (text: string) => void }) {
  const [text, setText] = useState(stored);
  useEffect(() => setText(stored), [stored]);
  const length = [...text].length;
  return (
    <label className="field column">
      <span>{`${t("personalityCustomLabel")} · ${length}/${MAX_CUSTOM_CHARS}`}</span>
      <textarea
        rows={4}
        value={text}
        maxLength={MAX_CUSTOM_CHARS}
        onChange={(event) => setText(event.target.value)}
        onBlur={() => {
          if (text !== stored) onCommit(text);
        }}
      />
    </label>
  );
}

function General({
  t,
  values,
  onChange,
  error,
}: {
  t: Translate;
  values: GeneralSettings & PersonalitySettings;
  onChange: OnChange;
  error: string | null;
}) {
  const modeLabel: Record<GeneralSettings["general.start_mode"], MessageKey> = {
    tray: "modeTray",
    compact: "modeCompact",
    full: "modeFull",
  };
  return (
    <div className="general">
      <section className="group">
        <h3 className="group-title">{t("groupStart")}</h3>
        <label className="field">
          <span>{t("autostart")}</span>
          <input
            type="checkbox"
            checked={values["general.autostart"]}
            onChange={(event) => onChange("general.autostart", event.target.checked)}
          />
        </label>
        <label className="field">
          <span>{t("startMode")}</span>
          <select
            value={values["general.start_mode"]}
            onChange={(event) =>
              onChange("general.start_mode", event.target.value as GeneralSettings["general.start_mode"])
            }
          >
            {START_MODES.map((mode) => (
              <option key={mode} value={mode}>
                {t(modeLabel[mode])}
              </option>
            ))}
          </select>
        </label>
      </section>
      <section className="group">
        <h3 className="group-title">{t("groupAppearance")}</h3>
        <label className="field">
          <span>{t("language")}</span>
          <select
            value={values["general.language"]}
            onChange={(event) => onChange("general.language", event.target.value as Language)}
          >
            {LANGUAGES.map((language) => (
              <option key={language} value={language}>
                {language.toUpperCase()}
              </option>
            ))}
          </select>
        </label>
      </section>
      <section className="group">
        <h3 className="group-title">{t("groupCharacter")}</h3>
        <label className="field">
          <span>{t("personality")}</span>
          <select
            value={values["personality.mode"]}
            onChange={(event) => onChange("personality.mode", event.target.value as PersonalityMode)}
          >
            {PERSONALITY_MODES.map((mode) => (
              <option key={mode} value={mode}>
                {t(PERSONALITY_LABEL[mode])}
              </option>
            ))}
          </select>
        </label>
        {values["personality.mode"] === "custom" && (
          <CustomInstructions t={t} stored={values["personality.custom"]} onCommit={(text) => onChange("personality.custom", text)} />
        )}
      </section>
      {error && <p className="error">{error}</p>}
    </div>
  );
}

export interface PreviewControls {
  active: AssistantState | null;
  onState: (state: AssistantState | null) => void;
  level: number;
  onLevel: (level: number) => void;
}

const QUALITY_LABEL: Record<AppSettings["visual.quality"], MessageKey> = {
  auto: "qualityAuto",
  low: "qualityLow",
  medium: "qualityMedium",
  high: "qualityHigh",
};

function Visual({
  t,
  values,
  onChange,
  error,
  debug,
  onDebug,
  preview,
}: {
  t: Translate;
  values: AppSettings;
  onChange: OnChange;
  error: string | null;
  debug: boolean;
  onDebug: (value: boolean) => void;
  preview: PreviewControls;
}) {
  return (
    <div className="visual">
      <section className="group">
        <h3 className="group-title">{t("quality")}</h3>
        <label className="field">
          <span>{t("quality")}</span>
          <select
            value={values["visual.quality"]}
            onChange={(event) => onChange("visual.quality", event.target.value as AppSettings["visual.quality"])}
          >
            {QUALITY_SETTINGS.map((quality) => (
              <option key={quality} value={quality}>
                {t(QUALITY_LABEL[quality])}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>{t("fpsLimit")}</span>
          <select
            value={values["visual.fps_limit"]}
            onChange={(event) =>
              onChange("visual.fps_limit", Number(event.target.value) as AppSettings["visual.fps_limit"])
            }
          >
            {FPS_LIMITS.map((limit) => (
              <option key={limit} value={limit}>
                {limit}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>{t("reducedMotion")}</span>
          <input
            type="checkbox"
            checked={values["visual.reduced_motion"]}
            onChange={(event) => onChange("visual.reduced_motion", event.target.checked)}
          />
        </label>
        <label className="field">
          <span>{t("debugOverlay")}</span>
          <input type="checkbox" checked={debug} onChange={(event) => onDebug(event.target.checked)} />
        </label>
      </section>
      <section className="group">
        <h3 className="group-title preview-title">{t("statePreview")}</h3>
        <div className="preview-states">
          {STATES.map((name) => (
            <button
              key={name}
              type="button"
              className={preview.active === name ? "chip active" : "chip"}
              aria-pressed={preview.active === name}
              onClick={() => preview.onState(name)}
            >
              {name}
            </button>
          ))}
          <button type="button" className="chip" disabled={preview.active === null} onClick={() => preview.onState(null)}>
            {t("previewReset")}
          </button>
        </div>
        <label className="field">
          <span>{t("levelPreview")}</span>
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={preview.level}
            onChange={(event) => preview.onLevel(Number(event.target.value))}
          />
        </label>
      </section>
      {error && <p className="error">{error}</p>}
    </div>
  );
}

export interface SettingsDrawerProps {
  t: Translate;
  language: Language;
  values: AppSettings;
  onChange: OnChange;
  error: string | null;
  auditKey: number;
  debug: boolean;
  onDebug: (value: boolean) => void;
  preview: PreviewControls;
  tab: TabId;
  onTab: (tab: TabId) => void;
  onClose: () => void;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The settings overlay: vertical icon navigation on the left, one section at a time on the right. */
export const SettingsDrawer = memo(function SettingsDrawer({
  t,
  language,
  values,
  onChange,
  error,
  auditKey,
  debug,
  onDebug,
  preview,
  tab,
  onTab: setTab,
  onClose,
}: SettingsDrawerProps) {
  const drawerRef = useRef<HTMLElement | null>(null);
  const navRefs = useRef(new Map<TabId, HTMLButtonElement>());

  // Focus goes to the active section button on open and back to the opener on close.
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    navRefs.current.get(tab)?.focus();
    return () => opener?.focus();
    // Only on mount: switching sections must not steal focus again.
  }, []);

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const nodes = Array.from(drawerRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    if (nodes.length === 0) return;
    const first = nodes[0];
    const last = nodes[nodes.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  const onNavKey = (event: KeyboardEvent<HTMLElement>) => {
    const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
    if (!keys.includes(event.key)) return;
    event.preventDefault();
    const index = TABS.findIndex((item) => item.id === tab);
    let next = index;
    if (event.key === "ArrowDown") next = (index + 1) % TABS.length;
    if (event.key === "ArrowUp") next = (index - 1 + TABS.length) % TABS.length;
    if (event.key === "Home") next = 0;
    if (event.key === "End") next = TABS.length - 1;
    const id = TABS[next]?.id ?? tab;
    setTab(id);
    navRefs.current.get(id)?.focus();
  };

  return (
    <div className="drawer-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <aside
        className="drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        ref={drawerRef}
        onKeyDown={onKeyDown}
      >
        <header className="drawer-head">
          <h2 id="settings-title">{t("settings")}</h2>
          <button type="button" className="icon-button" aria-label={t("hudCloseSettings")} title={t("hudCloseSettings")} onClick={onClose}>
            <Icon name="close" />
          </button>
        </header>
        <div className="drawer-body">
          <nav className="side-nav" role="tablist" aria-orientation="vertical" aria-label={t("settings")} onKeyDown={onNavKey}>
            {TABS.map((item) => (
              <button
                key={item.id}
                type="button"
                role="tab"
                id={`tab-${item.id}`}
                aria-selected={item.id === tab}
                aria-controls="settings-panel"
                tabIndex={item.id === tab ? 0 : -1}
                className={item.id === tab ? "nav-item active" : "nav-item"}
                title={t(NAV_LABEL[item.id])}
                ref={(node) => {
                  if (node) navRefs.current.set(item.id, node);
                  else navRefs.current.delete(item.id);
                }}
                onClick={() => setTab(item.id)}
              >
                <Icon name={item.icon} />
                <span className="nav-label">{t(NAV_LABEL[item.id])}</span>
              </button>
            ))}
          </nav>
          <div className="drawer-content" role="tabpanel" id="settings-panel" aria-labelledby={`tab-${tab}`}>
            <div className="content-head">
              <h3 className="content-title">{t(NAV_LABEL[tab])}</h3>
              <p className="content-hint">{t(NAV_HINT[tab])}</p>
            </div>
            {tab === "GENERAL" && (
              <>
                <General t={t} values={values} onChange={onChange} error={error} />
                <AuditLog t={t} refreshKey={auditKey} />
              </>
            )}
            {tab === "VOICE" && <VoiceTab t={t} values={values} onChange={onChange} error={error} />}
            {tab === "AI" && <AiTab t={t} values={values} onChange={onChange} />}
            {tab === "PRIVACY" && <PrivacyTab t={t} values={values} onChange={onChange} />}
            {tab === "SYSTEM" && <SystemTab t={t} language={language} />}
            {tab === "MEMORY" && <MemoryTab t={t} />}
            {tab === "SETUP" && <SetupTab t={t} values={values} onChange={onChange} />}
            {tab === "TOOLS" && <ToolsTab t={t} values={values} onChange={onChange} error={error} />}
            {tab === "VISUAL" && (
              <Visual t={t} values={values} onChange={onChange} error={error} debug={debug} onDebug={onDebug} preview={preview} />
            )}
          </div>
        </div>
      </aside>
    </div>
  );
});
