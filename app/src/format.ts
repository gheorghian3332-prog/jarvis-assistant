import type { MetricsPayload } from "./api";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(1)} GB`;
  return `${Math.round(bytes / MIB)} MB`;
}

export function formatPercent(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return `${clampPercent(value).toFixed(1)}%`;
}

export interface MetricsView {
  cpuText: string;
  cpuPercent: number;
  ramText: string;
  ramPercent: number;
}

export function formatMetrics(metrics: MetricsPayload): MetricsView {
  const ramPercent =
    metrics.ram_total_bytes > 0 ? (metrics.ram_used_bytes / metrics.ram_total_bytes) * 100 : 0;
  return {
    cpuText: formatPercent(metrics.cpu_percent),
    cpuPercent: clampPercent(metrics.cpu_percent),
    ramText: `${formatBytes(metrics.ram_used_bytes)} / ${formatBytes(metrics.ram_total_bytes)}`,
    ramPercent: clampPercent(ramPercent),
  };
}

export function formatUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(hours)}:${pad(minutes)}:${pad(rest)}`;
}
