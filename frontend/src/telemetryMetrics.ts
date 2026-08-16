import type { DashboardResourceMetric } from "./types";

export type NetworkObservationMetric = "latency" | "loss" | "throughput";
export type ByteUnitDisplayMode = "decimal" | "binary";

export const INTERFACE_RATE_DEFINITION =
  "Interval-average rate from non-negative deltas of cumulative interface byte counters between adjacent telemetry buckets; stored as bits per second and displayed as bytes per second using the operator's selected unit system, never as an instantaneous line-speed sample.";

export function formatByteRateFromBitsPerSecond(
  value: number | null | undefined,
  mode: ByteUnitDisplayMode = "decimal",
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "No data";
  }
  const units =
    mode === "binary"
      ? ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"]
      : ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
  const base = mode === "binary" ? 1024 : 1_000;
  let scaled = Math.max(0, value) / 8;
  let unit = 0;
  while (scaled >= base && unit < units.length - 1) {
    scaled /= base;
    unit += 1;
  }
  if (scaled === 0) {
    return `0 ${units[unit]}`;
  }
  const display =
    unit === 0
      ? scaled >= 10
        ? Math.round(scaled).toString()
        : scaled.toFixed(1)
      : scaled.toFixed(1);
  return `${display} ${units[unit]}`;
}

export function formatByteCount(
  value: number | null | undefined,
  mode: ByteUnitDisplayMode = "decimal",
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "No data";
  }
  const units =
    mode === "binary"
      ? ["B", "KiB", "MiB", "GiB", "TiB"]
      : ["B", "KB", "MB", "GB", "TB"];
  const base = mode === "binary" ? 1024 : 1_000;
  let scaled = Math.max(0, value);
  let unit = 0;
  while (scaled >= base && unit < units.length - 1) {
    scaled /= base;
    unit += 1;
  }
  return `${scaled >= 10 || unit === 0 ? Math.round(scaled) : scaled.toFixed(1)} ${units[unit]}`;
}

export function formatUptime(value: number | null | undefined): string {
  if (
    value === null ||
    value === undefined ||
    !Number.isFinite(value) ||
    value < 0
  ) {
    return "-";
  }
  const seconds = Math.floor(value);
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return `${seconds}s`;
}

export function resourceMetricDefinition(
  metric: DashboardResourceMetric,
): string {
  switch (metric) {
    case "cpu_load":
      return "Each chart point averages available Linux 1-minute load evidence in the displayed interval; source resolution follows the selected history range. Load is scheduler demand, not CPU utilization.";
    case "memory_used":
      return "Each chart point averages available used-memory ratio evidence computed from MemTotal and MemAvailable snapshots in the displayed interval; peak is the largest represented snapshot ratio.";
    case "disk_free":
      return "Each chart point derives free space from available aggregate-filesystem used-ratio evidence in the displayed interval; lowest corresponds to the largest represented used ratio.";
  }
}

export function networkObservationMetricDefinition(
  metric: NetworkObservationMetric,
): string {
  switch (metric) {
    case "latency":
      return "Each point is the mean RTT from one exact bounded ICMP probe run or from the source runs represented by one retained evidence bucket.";
    case "loss":
      return "Each point is the lost-to-transmitted packet ratio from one exact bounded ICMP probe run or the average ratio across source runs represented by one retained evidence bucket.";
    case "throughput":
      return "Each point is average TCP throughput for one capped test: bytes transferred x 8 / actual elapsed seconds; it is not an instantaneous interface rate.";
  }
}
