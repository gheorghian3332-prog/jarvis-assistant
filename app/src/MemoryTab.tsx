import { useCallback, useEffect, useState } from "react";
import {
  isTauri,
  memoryFactDelete,
  memoryFacts,
  memoryHistory,
  memoryHistoryDelete,
  type HistoryRow,
  type MemoryFact,
} from "./api";
import type { MessageKey } from "./i18n";

type Translate = (key: MessageKey) => string;

export interface MemoryViewProps {
  t: Translate;
  query: string;
  facts: MemoryFact[];
  history: HistoryRow[];
  error: string | null;
  onQuery: (query: string) => void;
  onDeleteFact: (id: number) => void;
  onDeleteHistory: (id: number) => void;
}

/** The memory panel: remembered facts and the command history, searchable, any row deletable. */
export function MemoryView({ t, query, facts, history, error, onQuery, onDeleteFact, onDeleteHistory }: MemoryViewProps) {
  return (
    <div className="memory">
      <h3>{t("memoryTitle")}</h3>
      <label className="field">
        <span>{t("memorySearch")}</span>
        <input type="text" value={query} onChange={(event) => onQuery(event.target.value)} />
      </label>
      {error && <p className="error">{error}</p>}
      <h3 className="section-title">{t("memoryFacts")}</h3>
      {facts.length === 0 && <p className="muted">{t("memoryEmpty")}</p>}
      <ul className="plain">
        {facts.map((fact) => (
          <li key={fact.id}>
            <span>
              <span className="mono">{fact.kind}</span> <strong>{fact.key}</strong>: {fact.value}
            </span>
            <button type="button" aria-label={`${t("memoryDelete")} #${fact.id}`} onClick={() => onDeleteFact(fact.id)}>
              {t("memoryDelete")}
            </button>
          </li>
        ))}
      </ul>
      <h3 className="section-title">{t("memoryHistory")}</h3>
      {history.length === 0 && <p className="muted">{t("memoryEmpty")}</p>}
      <ul className="plain">
        {history.map((row) => (
          <li key={row.id} className={row.ok ? "" : "failed"}>
            <span>
              <span className="mono ts">{row.ts}</span> <span className="mono">{row.route}</span> {row.text}
              {row.action && <small className="muted"> → {row.action}</small>}
            </span>
            <button type="button" aria-label={`${t("memoryDelete")} #${row.id}`} onClick={() => onDeleteHistory(row.id)}>
              {t("memoryDelete")}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function MemoryTab({ t }: { t: Translate }) {
  const [query, setQuery] = useState("");
  const [facts, setFacts] = useState<MemoryFact[]>([]);
  const [history, setHistory] = useState<HistoryRow[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    if (!isTauri()) return;
    const search = query.trim() === "" ? null : query.trim();
    Promise.all([memoryFacts(search, 200), memoryHistory(search, 200)])
      .then(([factRows, historyRows]) => {
        setFacts(factRows);
        setHistory(historyRows);
        setError(null);
      })
      .catch((failure: { message?: string }) => setError(failure.message ?? String(failure)));
  }, [query]);

  useEffect(load, [load]);

  return (
    <MemoryView
      t={t}
      query={query}
      facts={facts}
      history={history}
      error={error}
      onQuery={setQuery}
      onDeleteFact={(id) => {
        memoryFactDelete(id)
          .then(load)
          .catch((failure: { message?: string }) => setError(failure.message ?? String(failure)));
      }}
      onDeleteHistory={(id) => {
        memoryHistoryDelete(id)
          .then(load)
          .catch((failure: { message?: string }) => setError(failure.message ?? String(failure)));
      }}
    />
  );
}
