import type { DashboardResourceMetric } from "./types";

export type NetworkObservationMetric = "latency" | "loss" | "throughput";

export const INTERFACE_RATE_DEFINITION =
  "Interval-average rate from non-negative deltas of cumulative interface byte counters between adjacent telemetry buckets; never an instantaneous line-speed sample.";

export function resourceMetricDefinition(metric: DashboardResourceMetric): string {
  switch (metric) {
    case "cpu_load":
      return "Each chart point averages retained 60-second Linux 1-minute load rollups in the displayed interval; load is scheduler demand, not CPU utilization.";
    case "memory_used":
      return "Each chart point averages retained 60-second used-memory ratios computed from MemTotal and MemAvailable; peak uses the lowest available-memory sample.";
    case "disk_free":
      return "Each chart point averages retained 60-second free-space ratios across reported filesystems; lowest uses the smallest available-space sample.";
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
