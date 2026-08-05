import type { DashboardResourceMetric } from "./types";

export type NetworkObservationMetric = "latency" | "loss" | "throughput";

export const INTERFACE_RATE_DEFINITION =
  "Interval-average rate from non-negative deltas of cumulative interface byte counters between adjacent telemetry buckets; stored as bits per second and displayed as decimal bytes per second, never as an instantaneous line-speed sample.";

export function formatByteRateFromBitsPerSecond(
  value: number | null | undefined,
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "No data";
  }
  const units = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s", "PB/s"];
  let scaled = Math.max(0, value) / 8;
  let unit = 0;
  while (scaled >= 1_000 && unit < units.length - 1) {
    scaled /= 1_000;
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

export function formatByteCount(value: number): string {
  if (!Number.isFinite(value)) return "No data";
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  let scaled = Math.max(0, value);
  let unit = 0;
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024;
    unit += 1;
  }
  return `${scaled >= 10 || unit === 0 ? Math.round(scaled) : scaled.toFixed(1)} ${units[unit]}`;
}

export function resourceMetricDefinition(metric: DashboardResourceMetric): string {
  switch (metric) {
    case "cpu_load":
      return "Each chart point averages retained 60-second Linux 1-minute load rollups in the displayed interval; load is scheduler demand, not CPU utilization.";
    case "memory_used":
      return "Each chart point averages retained 60-second used-memory ratios computed independently from each MemTotal and MemAvailable snapshot; peak is the largest snapshot ratio.";
    case "disk_free":
      return "Each chart point derives free space from retained per-snapshot aggregate-filesystem used ratios; lowest corresponds to the largest used ratio.";
  }
}

export function networkObservationMetricDefinition(
  metric: NetworkObservationMetric,
): string {
  switch (metric) {
    case "latency":
      return "Each point is the mean RTT reported by one bounded ICMP probe run.";
    case "loss":
      return "Each point is the lost-to-transmitted packet ratio from one bounded ICMP probe run.";
    case "throughput":
      return "Each point is average TCP throughput for one capped test: bytes transferred x 8 / actual elapsed seconds; it is not an instantaneous interface rate.";
  }
}
