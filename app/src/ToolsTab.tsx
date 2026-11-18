import { useCallback, useEffect, useState } from "react";
import {
  auditList,
  controlActions,
  emergencyStop,
  isTauri,
  type ActionInfo,
  type AuditEntry,
  type CommandError,
} from "./api";
import type { MessageKey } from "./i18n";
import type { AppSettings, Language, SettingKey, Validation } from "./settings";
import { addAlias, addRoot, isEnabled, recentActions, removeAlias, removeRoot, riskClass, toggleAction } from "./tools";
import { useTauriEvent } from "./useTauriEvent";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

const RISK_LABEL: Record<ActionInfo["risk"], MessageKey> = {
  safe: "riskSafe",
  confirm: "riskConfirm",
  forbidden: "riskForbidden",
};

export interface ToolsViewProps {
  t: Translate;
  language: Language;
  actions: ActionInfo[];
  defaultRoots: string[];
  values: AppSettings;
  recent: AuditEntry[];
  error: string | null;
  onChange: OnChange;
  onEmergencyStop: () => void;
}

/** The TOOLS tab without any backend calls: the table, roots, aliases and the recent actions. */
export function ToolsView({
  t,
  language,
  actions,
  defaultRoots,
  values,
  recent,
  error,
  onChange,
  onEmergencyStop,
}: ToolsViewProps) {
  const disabled = values["control.disabled_actions"];
  const roots = values["control.file_roots"];
  const aliases = values["control.app_aliases"];
  const [rootInput, setRootInput] = useState("");
  const [aliasName, setAliasName] = useState("");
  const [aliasTarget, setAliasTarget] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const apply = <V,>(checked: Validation<V>, commit: (value: V) => void, reset: () => void) => {
    if (checked.ok) {
      setFormError(null);
      commit(checked.value);
      reset();
    } else {
      setFormError(checked.error);
    }
  };

  return (
    <div className="tools">
      <h3>{t("toolsActions")}</h3>
      <table className="actions">
        <tbody>
          {actions.map((action) => {
            const locked = action.risk === "forbidden";
            const description = language === "ru" ? action.description_ru : action.description_en;
            return (
              <tr key={action.name}>
                <td>
                  <span className="mono action-name">{action.name}</span>
                  <small className="muted block">{description}</small>
                </td>
                <td>
                  <span className={riskClass(action.risk)}>{t(RISK_LABEL[action.risk])}</span>
                </td>
                <td>
                  <input
                    type="checkbox"
                    aria-label={`${t("toolsEnabled")}: ${action.name}`}
                    title={locked ? t("forbiddenLocked") : undefined}
                    checked={isEnabled(action, disabled)}
                    disabled={locked}
                    onChange={(event) =>
                      onChange("control.disabled_actions", toggleAction(disabled, action.name, event.target.checked))
                    }
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <h3 className="section-title">{t("fileRoots")}</h3>
      {roots.length === 0 && <p className="muted">{`${t("fileRootsDefault")}: ${defaultRoots.join(", ")}`}</p>}
      <ul className="plain">
        {roots.map((root) => (
          <li key={root}>
            <span className="mono">{root}</span>
            <button type="button" onClick={() => onChange("control.file_roots", removeRoot(roots, root))}>
              {t("remove")}
            </button>
          </li>
        ))}
      </ul>
      <div className="row">
        <input
          type="text"
          value={rootInput}
          placeholder={t("folderPlaceholder")}
          aria-label={t("fileRoots")}
          onChange={(event) => setRootInput(event.target.value)}
        />
        <button
          type="button"
          onClick={() =>
            apply(addRoot(roots, rootInput), (value) => onChange("control.file_roots", value), () => setRootInput(""))
          }
        >
          {t("add")}
        </button>
      </div>

      <h3 className="section-title">{t("appAliases")}</h3>
      <ul className="plain">
        {Object.entries(aliases).map(([name, target]) => (
          <li key={name}>
            <span>
              <strong>{name}</strong> <span className="mono muted">{target}</span>
            </span>
            <button type="button" onClick={() => onChange("control.app_aliases", removeAlias(aliases, name))}>
              {t("remove")}
            </button>
          </li>
        ))}
      </ul>
      <div className="row">
        <input
          type="text"
          value={aliasName}
          placeholder={t("aliasName")}
          aria-label={t("aliasName")}
          onChange={(event) => setAliasName(event.target.value)}
        />
        <input
          type="text"
          value={aliasTarget}
          placeholder={t("aliasTarget")}
          aria-label={t("aliasTarget")}
          onChange={(event) => setAliasTarget(event.target.value)}
        />
        <button
          type="button"
          onClick={() =>
            apply(
              addAlias(aliases, aliasName, aliasTarget),
              (value) => onChange("control.app_aliases", value),
              () => {
                setAliasName("");
                setAliasTarget("");
              },
            )
          }
        >
          {t("add")}
        </button>
      </div>

      <label className="field">
        <span>{t("emergencyKey")}</span>
        <input
          type="text"
          defaultValue={values["control.emergency_key"]}
          aria-label={t("emergencyKey")}
          onBlur={(event) => onChange("control.emergency_key", event.target.value)}
        />
      </label>
      <button type="button" className="danger" onClick={onEmergencyStop}>
        {t("emergencyStop")}
      </button>

      {(formError ?? error) && <p className="error">{formError ?? error}</p>}

      <section className="audit">
        <h3>{t("recentActions")}</h3>
        {recent.length === 0 && <p className="muted">{t("auditEmpty")}</p>}
        <ul>
          {recentActions(recent).map((entry) => (
            <li key={entry.id} className={entry.ok ? "" : "failed"}>
              <span className="mono ts">{entry.ts}</span>
              <span className="action">{entry.action}</span>
              <span className="muted detail">{entry.detail}</span>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

/** Loads the action table and the audit tail, and hands them to the view. */
export function ToolsTab({
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
  const [actions, setActions] = useState<ActionInfo[]>([]);
  const [defaultRoots, setDefaultRoots] = useState<string[]>([]);
  const [recent, setRecent] = useState<AuditEntry[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!isTauri()) return undefined;
    let cancelled = false;
    controlActions()
      .then((info) => {
        if (cancelled) return;
        setActions(info.actions);
        setDefaultRoots(info.default_roots);
      })
      .catch((failure: CommandError) => {
        if (!cancelled) setLoadError(`${t("loadToolsFailed")}: ${failure.message}`);
      });
    return () => {
      cancelled = true;
    };
  }, [t]);

  useEffect(() => {
    if (!isTauri()) return undefined;
    let cancelled = false;
    auditList(20)
      .then((list) => {
        if (!cancelled) setRecent(list);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [tick, values]);

  const refresh = useCallback(() => setTick((current) => current + 1), []);
  useTauriEvent("action://result", refresh);

  return (
    <ToolsView
      t={t}
      language={values["general.language"]}
      actions={actions}
      defaultRoots={defaultRoots}
      values={values}
      recent={recent}
      error={loadError ?? error}
      onChange={onChange}
      onEmergencyStop={() => void emergencyStop().catch(() => undefined)}
    />
  );
}
