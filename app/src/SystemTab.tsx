import { useCallback, useEffect, useRef, useState } from "react";
import {
  isTauri,
  settingsGet,
  settingsSet,
  systemAlerts,
  systemSnapshot,
  systemTopProcesses,
  type AlertPayload,
  type CommandError,
  type ProcessUsage,
  type SystemSnapshot,
} from "./api";
import { formatBytes } from "./format";
import type { MessageKey } from "./i18n";
import type { Language } from "./settings";
import {
  alertText,
  freePercent,
  formatMegahertz,
  formatPercentOrDash,
  formatRate,
  formatTemperature,
  minuteToTime,
  pushAlert,
  resolveAlerts,
  THRESHOLD_RANGE,
  THRESHOLD_RULES,
  timeToMinute,
  validateAlerts,
  validRttHost,
  withEnabled,
  withThreshold,
  type AlertConfig,
  type RuleName,
  type SwitchName,
  type ThresholdName,
} from "./system";
import { useTauriEvent } from "./useTauriEvent";

type Translate = (key: MessageKey) => string;

/** Live values refresh once a second while the tab is open (and only then). */
const POLL_MS = 1000;

const RULE_LABEL: Record<RuleName, MessageKey> = {
  cpu_load: "sysRuleCpuLoad",
  gpu_temp: "sysRuleGpuTemp",
  cpu_temp: "sysRuleCpuTemp",
  ram_load: "sysRuleRamLoad",
  disk_space: "sysRuleDisk",
  internet_down: "sysRuleInternet",
  window_hung: "sysRuleHung",
  app_crash: "sysRuleCrash",
};

/** A bar without any CSS transition: it changes every second and animating it costs CPU. */
function Bar({ percent }: { percent: number }) {
  const width = Math.min(100, Math.max(0, Number.isFinite(percent) ? percent : 0));
  return (
    <div className="bar sys-bar">
      <div className="bar-fill" style={{ width: `${width}%` }} />
    </div>
  );
}

function Row({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className={warn ? "sys-row warn" : "sys-row"}>
      <span>{label}</span>
      <span className="mono">{value}</span>
    </div>
  );
}

/**
 * Polls `system_snapshot`; a slow answer is never overlapped by the next request. Polling stops while `enabled` is false
 * and while the window is hidden or minimised, so nothing runs when nobody can see the numbers.
 */
export function useSystemSnapshot(intervalMs: number = POLL_MS, enabled = true): SystemSnapshot | null {
  const [snapshot, setSnapshot] = useState<SystemSnapshot | null>(null);
  useEffect(() => {
    if (!isTauri() || !enabled) return undefined;
    let cancelled = false;
    let busy = false;
    let timer: number | undefined;
    const tick = () => {
      if (busy || document.hidden) return;
      busy = true;
      systemSnapshot()
        .then((value) => {
          if (!cancelled) setSnapshot(value);
        })
        .catch(() => undefined)
        .finally(() => {
          busy = false;
        });
    };
    const start = () => {
      if (timer !== undefined) return;
      tick();
      timer = window.setInterval(tick, intervalMs);
    };
    const stop = () => {
      if (timer === undefined) return;
      window.clearInterval(timer);
      timer = undefined;
    };
    const onVisibility = () => (document.hidden ? stop() : start());
    document.addEventListener("visibilitychange", onVisibility);
    if (!document.hidden) start();
    return () => {
      cancelled = true;
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [intervalMs, enabled]);
  return snapshot;
}

function CpuBlock({ t, snapshot }: { t: Translate; snapshot: SystemSnapshot }) {
  const cpu = snapshot.metrics.cpu;
  return (
    <section className="sys-block">
      <h4>CPU</h4>
      <Row label="CPU" value={formatPercentOrDash(cpu.usage_percent)} />
      <Bar percent={cpu.usage_percent ?? 0} />
      <Row label={t("sysFrequency")} value={formatMegahertz(cpu.frequency_mhz)} />
      <Row label={t("sysGpuTemp")} value={formatTemperature(cpu.temperature_c)} />
      {cpu.temperature_c === null && <p className="muted small">{t("sysTempUnavailable")}</p>}
      <div className="cores" aria-label={t("sysCpuCores")}>
        {cpu.per_core_percent.map((percent, index) => (
          <div className="core" key={index} title={`${index}: ${Math.round(percent)}%`}>
            <div className="core-fill" style={{ height: `${Math.min(100, Math.max(0, percent))}%` }} />
          </div>
        ))}
      </div>
    </section>
  );
}

function GpuBlock({ t, snapshot }: { t: Translate; snapshot: SystemSnapshot }) {
  const gpu = snapshot.metrics.gpu;
  if (!gpu) {
    return (
      <section className="sys-block">
        <h4>{t("sysGpu")}</h4>
        <p className="warn-text">{t("sysGpuUnavailable")}</p>
      </section>
    );
  }
  const vram =
    gpu.vram_used_bytes !== null && gpu.vram_total_bytes !== null
      ? `${formatBytes(gpu.vram_used_bytes)} / ${formatBytes(gpu.vram_total_bytes)}`
      : "—";
  const vramPercent =
    gpu.vram_used_bytes !== null && gpu.vram_total_bytes ? (gpu.vram_used_bytes / gpu.vram_total_bytes) * 100 : 0;
  return (
    <section className="sys-block">
      <h4>{gpu.name ?? t("sysGpu")}</h4>
      <Row label={t("sysGpuTemp")} value={formatTemperature(gpu.temperature_c)} />
      {gpu.temperature_c === null && <p className="warn-text">{t("sysTempUnavailable")}</p>}
      <Row label={t("sysGpuLoad")} value={formatPercentOrDash(gpu.utilization_percent)} />
      <Bar percent={gpu.utilization_percent ?? 0} />
      <Row label={t("sysVram")} value={vram} />
      <Bar percent={vramPercent} />
      <Row label={t("sysFrequency")} value={formatMegahertz(gpu.graphics_clock_mhz)} />
      {gpu.power_watts !== null && <Row label={t("sysPower")} value={`${Math.round(gpu.power_watts)} W`} />}
      <p className="muted small">
        {snapshot.gpu_source === "nvml" ? t("sysGpuSourceNvml") : t("sysGpuSourceD3dkmt")}
      </p>
    </section>
  );
}

function RamBlock({ t, snapshot }: { t: Translate; snapshot: SystemSnapshot }) {
  const ram = snapshot.metrics.ram;
  const percent = ram.total_bytes > 0 ? (ram.used_bytes / ram.total_bytes) * 100 : 0;
  return (
    <section className="sys-block">
      <h4>{t("sysRam")}</h4>
      <Row label={t("sysRam")} value={`${formatBytes(ram.used_bytes)} / ${formatBytes(ram.total_bytes)}`} />
      <Bar percent={percent} />
    </section>
  );
}

function DiskBlock({ t, snapshot }: { t: Translate; snapshot: SystemSnapshot }) {
  const { volumes, health } = snapshot.metrics.storage;
  return (
    <section className="sys-block">
      <h4>{t("sysDisks")}</h4>
      {volumes.map((volume) => {
        const free = freePercent(volume.total_bytes, volume.free_bytes);
        return (
          <div key={volume.mount} className="sys-disk">
            <Row
              label={volume.mount}
              value={`${t("sysFree")} ${formatBytes(volume.free_bytes)} / ${formatBytes(volume.total_bytes)}`}
              warn={volume.is_system && free < 5}
            />
            <Bar percent={100 - free} />
            <p className="muted small">
              {`${t("sysRead")} ${formatRate(volume.read_bytes_per_sec)} · ${t("sysWrite")} ${formatRate(volume.write_bytes_per_sec)}`}
            </p>
          </div>
        );
      })}
      {health && health.length > 0 && (
        <p className="muted small">
          {`${t("sysHealth")}: ${health.map((drive) => `${drive.name} — ${drive.status}`).join("; ")}`}
        </p>
      )}
    </section>
  );
}

function NetworkBlock({ t, snapshot }: { t: Translate; snapshot: SystemSnapshot }) {
  const net = snapshot.metrics.network;
  let status = "—";
  if (net.link_up === false) status = t("sysLinkDown");
  else if (net.internet_reachable === true) status = t("sysOnline");
  else if (net.internet_reachable === false) status = t("sysOffline");
  const rtt = net.rtt_ms === null ? "—" : `${Math.round(net.rtt_ms)} ms${net.rtt_host ? ` (${net.rtt_host})` : ""}`;
  return (
    <section className="sys-block">
      <h4>{t("sysNetwork")}</h4>
      <Row label={t("sysLink")} value={status} warn={net.internet_reachable === false || net.link_up === false} />
      <Row label={t("sysDown")} value={formatRate(net.down_bytes_per_sec)} />
      <Row label={t("sysUp")} value={formatRate(net.up_bytes_per_sec)} />
      <Row label={t("sysRtt")} value={rtt} />
    </section>
  );
}

function HungBlock({ t, snapshot }: { t: Translate; snapshot: SystemSnapshot }) {
  const hung = snapshot.metrics.hung_windows;
  return (
    <section className="sys-block">
      <h4>{t("sysHung")}</h4>
      {hung.length === 0 ? (
        <p className="muted">{t("sysNoHung")}</p>
      ) : (
        <ul className="plain">
          {hung.map((window) => (
            <li key={window.hwnd} className="warn-text">
              {window.title || window.process || String(window.pid)}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function TopProcesses({ t }: { t: Translate }) {
  const [list, setList] = useState<ProcessUsage[] | null>(null);
  const [busy, setBusy] = useState(false);
  const load = (by: "cpu" | "ram") => {
    setBusy(true);
    systemTopProcesses(by)
      .then(setList)
      .catch(() => setList([]))
      .finally(() => setBusy(false));
  };
  return (
    <section className="sys-block wide">
      <h4>{t("sysTop")}</h4>
      <div className="row">
        <button type="button" disabled={busy} onClick={() => load("cpu")}>
          {t("sysTopCpu")}
        </button>
        <button type="button" disabled={busy} onClick={() => load("ram")}>
          {t("sysTopRam")}
        </button>
      </div>
      <p className="muted small">{t("sysTopHint")}</p>
      {list && (
        <table className="sys-table">
          <tbody>
            {list.map((process) => (
              <tr key={process.exe}>
                <td>{process.display_name}</td>
                <td className="mono">{`×${process.instances}`}</td>
                <td className="mono">{`${process.memory_mb} MB`}</td>
                <td className="mono">{process.cpu_percent === null ? "" : `${Math.round(process.cpu_percent)}%`}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function AlertFeed({ t, language }: { t: Translate; language: Language }) {
  const [feed, setFeed] = useState<AlertPayload[]>([]);
  useEffect(() => {
    if (!isTauri()) return;
    systemAlerts()
      .then((list) => setFeed((current) => list.reduceRight((acc, item) => pushAlert(acc, item), current)))
      .catch(() => undefined);
  }, []);
  const onAlert = useCallback((alert: AlertPayload) => setFeed((current) => pushAlert(current, alert)), []);
  useTauriEvent("alert://raised", onAlert);
  return (
    <section className="sys-block wide">
      <h4>{t("sysAlerts")}</h4>
      {feed.length === 0 && <p className="muted">{t("sysNoAlerts")}</p>}
      <ul className="plain alerts">
        {feed.map((alert) => (
          <li key={`${alert.id}-${alert.at_ms}`} className={`alert alert-${alert.severity}`}>
            <span className="mono ts">{new Date(alert.at_ms).toLocaleTimeString()}</span>
            <span>{alertText(alert, language)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** A whole number that is committed when the field loses focus (or on Enter). */
function NumberField({
  value,
  min,
  max,
  label,
  onCommit,
}: {
  value: number;
  min: number;
  max: number;
  label: string;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const number = Number(text);
    if (text.trim() !== "" && Number.isFinite(number) && number >= min && number <= max) {
      if (number !== value) onCommit(number);
    } else {
      setText(String(value));
    }
  };
  return (
    <input
      type="number"
      aria-label={label}
      min={min}
      max={max}
      value={text}
      onChange={(event) => setText(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit();
      }}
    />
  );
}

function TimeField({ minute, label, onCommit }: { minute: number; label: string; onCommit: (minute: number) => void }) {
  const [text, setText] = useState(minuteToTime(minute));
  useEffect(() => setText(minuteToTime(minute)), [minute]);
  const commit = () => {
    const parsed = timeToMinute(text);
    if (parsed === null) setText(minuteToTime(minute));
    else if (parsed !== minute) onCommit(parsed);
  };
  return (
    <input
      type="text"
      className="time"
      aria-label={label}
      value={text}
      maxLength={5}
      onChange={(event) => setText(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit();
      }}
    />
  );
}

function Rules({ t }: { t: Translate }) {
  const [config, setConfig] = useState<AlertConfig>(() => resolveAlerts(null));
  const [host, setHost] = useState("1.1.1.1:443");
  const [storedHost, setStoredHost] = useState("1.1.1.1:443");
  const [note, setNote] = useState<string | null>(null);
  const configRef = useRef(config);
  configRef.current = config;

  useEffect(() => {
    if (!isTauri()) return;
    settingsGet("sysintel.alerts")
      .then((value) => setConfig(resolveAlerts(value)))
      .catch(() => undefined);
    settingsGet("sysintel.rtt_host")
      .then((value) => {
        if (typeof value === "string") {
          setHost(value);
          setStoredHost(value);
        }
      })
      .catch(() => undefined);
  }, []);

  const save = (next: AlertConfig) => {
    const checked = validateAlerts(next);
    if (!checked.ok) {
      setNote(checked.error);
      return;
    }
    const previous = configRef.current;
    setConfig(next);
    if (!isTauri()) return;
    settingsSet("sysintel.alerts", next)
      .then(() => setNote(t("sysSaved")))
      .catch((failure: CommandError) => {
        setConfig(previous);
        setNote(failure.message);
      });
  };
  const commitHost = () => {
    const text = host.trim();
    if (text === storedHost) return;
    if (!validRttHost(text)) {
      setHost(storedHost);
      setNote("host:port, 1.1.1.1:443");
      return;
    }
    if (!isTauri()) return;
    settingsSet("sysintel.rtt_host", text)
      .then(() => {
        setStoredHost(text);
        setNote(t("sysSaved"));
      })
      .catch((failure: CommandError) => {
        setHost(storedHost);
        setNote(failure.message);
      });
  };

  const switches: SwitchName[] = ["disk_space", "internet_down", "window_hung", "app_crash"];
  return (
    <section className="sys-block wide">
      <h4>{t("sysRules")}</h4>
      <table className="sys-table rules">
        <tbody>
          {THRESHOLD_RULES.map((name: ThresholdName) => {
            const rule = config[name];
            const range = THRESHOLD_RANGE[name];
            return (
              <tr key={name}>
                <td>
                  <label>
                    <input
                      type="checkbox"
                      checked={rule.enabled}
                      onChange={(event) => save(withEnabled(config, name, event.target.checked))}
                    />
                    {t(RULE_LABEL[name])}
                  </label>
                </td>
                <td>
                  <NumberField
                    label={`${t(RULE_LABEL[name])}: ${t("sysThreshold")}`}
                    value={rule.threshold}
                    min={range.min}
                    max={range.max}
                    onCommit={(value) => save(withThreshold(config, name, { threshold: value }))}
                  />
                  {range.unit}
                </td>
                <td>
                  <NumberField
                    label={`${t(RULE_LABEL[name])}: ${t("sysDuration")}`}
                    value={rule.duration_s}
                    min={0}
                    max={3600}
                    onCommit={(value) => save(withThreshold(config, name, { duration_s: value }))}
                  />
                  {t("sysDuration")}
                </td>
              </tr>
            );
          })}
          {switches.map((name) => (
            <tr key={name}>
              <td colSpan={3}>
                <label>
                  <input
                    type="checkbox"
                    checked={config[name].enabled}
                    onChange={(event) => save(withEnabled(config, name, event.target.checked))}
                  />
                  {t(RULE_LABEL[name])}
                </label>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="sys-row">
        <label>
          <input
            type="checkbox"
            checked={config.quiet_hours.enabled}
            onChange={(event) => save({ ...config, quiet_hours: { ...config.quiet_hours, enabled: event.target.checked } })}
          />
          {t("sysQuiet")}
        </label>
        <span>
          {t("sysQuietFrom")}{" "}
          <TimeField
            label={t("sysQuietFrom")}
            minute={config.quiet_hours.start_minute}
            onCommit={(minute) => save({ ...config, quiet_hours: { ...config.quiet_hours, start_minute: minute } })}
          />{" "}
          {t("sysQuietTo")}{" "}
          <TimeField
            label={t("sysQuietTo")}
            minute={config.quiet_hours.end_minute}
            onCommit={(minute) => save({ ...config, quiet_hours: { ...config.quiet_hours, end_minute: minute } })}
          />
        </span>
      </div>
      <label className="field">
        <span>{t("sysRttHost")}</span>
        <input
          type="text"
          value={host}
          onChange={(event) => setHost(event.target.value)}
          onBlur={commitHost}
          onKeyDown={(event) => {
            if (event.key === "Enter") commitHost();
          }}
        />
      </label>
      {note && <p className="muted small">{note}</p>}
    </section>
  );
}

export function SystemTab({ t, language }: { t: Translate; language: Language }) {
  const snapshot = useSystemSnapshot();
  return (
    <div className="system">
      <p className="muted small">{t("sysWindowOnly")}</p>
      {snapshot ? (
        <div className="sys-grid">
          <CpuBlock t={t} snapshot={snapshot} />
          <GpuBlock t={t} snapshot={snapshot} />
          <RamBlock t={t} snapshot={snapshot} />
          <DiskBlock t={t} snapshot={snapshot} />
          <NetworkBlock t={t} snapshot={snapshot} />
          <HungBlock t={t} snapshot={snapshot} />
        </div>
      ) : (
        <p className="muted">{t("waiting")}</p>
      )}
      <TopProcesses t={t} />
      <AlertFeed t={t} language={language} />
      <Rules t={t} />
    </div>
  );
}
