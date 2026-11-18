import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import {
  isTauri,
  setWindowMode,
  settingsGet,
  settingsSet,
  type AssistantState,
  type CommandError,
} from "./api";
import { NeuralCore } from "./core/NeuralCore";
import { sanitizeLevel } from "./core/level";
import { parsePreviewParams } from "./core/preview";
import { CommandBar } from "./CommandBar";
import { ConfirmDialog } from "./ConfirmDialog";
import { CoreRings, Header, MiniMetrics, EventFeed, Subtitles, SystemColumn, useMedia } from "./Hud";
import { translate, type MessageKey } from "./i18n";
import { SettingsDrawer, TABS, type PreviewControls, type TabId } from "./SettingsDrawer";
import {
  SETTING_DEFAULTS,
  SETTING_KEYS,
  resolveSettings,
  validateSetting,
  type AppSettings,
  type SettingKey,
} from "./settings";
import { useTauriEvent } from "./useTauriEvent";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

export function App() {
  const params = useMemo(() => parsePreviewParams(window.location.search), []);
  const [state, setState] = useState<AssistantState>(isTauri() ? "IDLE" : "OFFLINE");
  const [preview, setPreview] = useState<AssistantState | null>(params.state);
  const [previewLevel, setPreviewLevel] = useState(params.level);
  const [debug, setDebug] = useState(params.debug);
  const [values, setValues] = useState<AppSettings>(() =>
    !isTauri() && params.quality ? { ...SETTING_DEFAULTS, "visual.quality": params.quality } : SETTING_DEFAULTS,
  );
  const [error, setError] = useState<string | null>(null);
  const [auditKey, setAuditKey] = useState(0);
  const [loaded, setLoaded] = useState(!isTauri());
  const levelRef: RefObject<number> = useRef(params.level);
  const compact = useMedia("(max-width: 699px)");
  const wide = useMedia("(min-width: 1000px)");
  const [settingsOpen, setSettingsOpen] = useState(() => params.settings !== null);
  const [settingsTab, setSettingsTab] = useState<TabId>(() => TABS.find((item) => item.id === params.settings)?.id ?? "GENERAL");
  const autoOpened = useRef(false);
  const wasCompact = useRef(compact);
  const language = values["general.language"];
  const t = useCallback<Translate>((key) => translate(language, key), [language]);

  const onState = useCallback((payload: { state: AssistantState }) => {
    setState(payload.state);
    setPreview(null);
  }, []);
  const onLevel = useCallback((payload: { rms: number }) => {
    levelRef.current = sanitizeLevel(payload.rms);
  }, []);
  useTauriEvent("assistant://state", onState);
  useTauriEvent("assistant://level", onLevel);

  useEffect(() => {
    if (!isTauri()) return undefined;
    let cancelled = false;
    Promise.all(SETTING_KEYS.map(async (key) => [key, await settingsGet(key)] as const))
      .then((pairs) => {
        if (!cancelled) {
          setValues(resolveSettings(Object.fromEntries(pairs)));
          setLoaded(true);
        }
      })
      .catch((failure: CommandError) => {
        if (!cancelled) setError(failure.message);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const valuesRef = useRef(values);
  useEffect(() => {
    valuesRef.current = values;
  }, [values]);

  const onChange = useCallback<OnChange>((key, value) => {
    const checked = validateSetting(key, value);
    if (!checked.ok) {
      setError(checked.error);
      return;
    }
    const previous = valuesRef.current[key];
    setValues((current) => ({ ...current, [key]: value }));
    if (!isTauri()) {
      setError(null);
      return;
    }
    settingsSet(key, value)
      .then(() => {
        setError(null);
        setAuditKey((current) => current + 1);
      })
      .catch((failure: CommandError) => {
        setValues((current) => ({ ...current, [key]: previous }));
        setError(`${translate(valuesRef.current["general.language"], "saveFailed")}: ${failure.message}`);
      });
  }, []);

  const onPreviewLevel = useCallback((level: number) => {
    const clamped = sanitizeLevel(level);
    levelRef.current = clamped;
    setPreviewLevel(clamped);
  }, []);
  const previewControls = useMemo<PreviewControls>(
    () => ({ active: preview, onState: setPreview, level: previewLevel, onLevel: onPreviewLevel }),
    [preview, previewLevel, onPreviewLevel],
  );

  // First run: the wizard opens by itself once the stored settings are known.
  useEffect(() => {
    if (loaded && !autoOpened.current) {
      autoOpened.current = true;
      if (!values["setup.completed"]) {
        setSettingsTab("SETUP");
        setSettingsOpen(true);
      }
    }
  }, [loaded, values]);

  // The compact window has no settings: going back to it closes the overlay.
  useEffect(() => {
    if (compact && !wasCompact.current && isTauri()) setSettingsOpen(false);
    wasCompact.current = compact;
  }, [compact]);

  const openSettings = useCallback(() => {
    setSettingsOpen((open) => !open);
    // The compact window only has a button: it switches to the full window, where the settings live.
    if (compact && isTauri()) void setWindowMode("full").catch(() => undefined);
  }, [compact]);
  const closeSettings = useCallback(() => setSettingsOpen(false), []);

  const shown = preview ?? state;
  const reduced = values["visual.reduced_motion"];
  return (
    <div className={reduced ? "app reduce-motion" : "app"} data-state={shown} data-mode={compact ? "compact" : "full"}>
      <Header t={t} language={language} state={shown} settingsOpen={settingsOpen} onSettings={openSettings} />
      <main className="main">
        {wide && (
          <aside className="col col-left">
            <SystemColumn t={t} />
          </aside>
        )}
        <section className="stage">
          <div className="core-slot">
            <NeuralCore
              state={shown}
              levelRef={levelRef}
              quality={values["visual.quality"]}
              fpsLimit={values["visual.fps_limit"]}
              reducedMotion={values["visual.reduced_motion"]}
              compact={compact}
              debug={debug}
              unavailableText={t("webglUnavailable")}
            />
            <CoreRings />
          </div>
          <Subtitles t={t} state={shown} preview={preview !== null} />
        </section>
        {wide && (
          <aside className="col col-right">
            <EventFeed t={t} language={language} />
          </aside>
        )}
      </main>
      <footer className="dock">
        {isTauri() && <CommandBar t={t} />}
        {isTauri() && !wide && <MiniMetrics t={t} />}
      </footer>
      {settingsOpen && (
        <SettingsDrawer
          t={t}
          language={language}
          values={values}
          onChange={onChange}
          error={error}
          auditKey={auditKey}
          debug={debug}
          onDebug={setDebug}
          preview={previewControls}
          tab={settingsTab}
          onTab={setSettingsTab}
          onClose={closeSettings}
        />
      )}
      {isTauri() && <ConfirmDialog t={t} />}
    </div>
  );
}
