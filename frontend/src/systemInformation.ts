import type { SystemInformationView, TelemetryRollupRecord } from "./types";
import { formatVirtualizationLabel } from "./utils";

export type SystemInformationFact = {
  key: "architecture" | "cpu" | "kernel" | "os" | "virtualization";
  label: string;
  value: string;
};

export function systemInformationFacts(
  information: SystemInformationView | null | undefined,
): SystemInformationFact[] {
  if (!information) return [];
  return [
    fact("os", "OS", information.os_name),
    fact("cpu", "CPU", information.cpu_model),
    fact("kernel", "Kernel", information.kernel_release),
    fact("architecture", "Architecture", information.architecture),
    fact(
      "virtualization",
      "Virtualization",
      information.virtualization
        ? formatVirtualizationLabel(information.virtualization)
        : null,
    ),
  ].filter((value): value is SystemInformationFact => value !== null);
}

export function formatSwapUsage(
  rollup: TelemetryRollupRecord | null | undefined,
  formatBytes: (value: number) => string,
): string | null {
  const total = rollup?.swap_total_bytes_max;
  if (!rollup || rollup.swap_sample_count <= 0 || total == null || total < 0) {
    return null;
  }
  if (total === 0) return "None";
  const ratio = rollup.swap_used_ratio_avg;
  return typeof ratio === "number" && Number.isFinite(ratio)
    ? `${Math.round(ratio * 100)}% (${formatBytes(total)})`
    : `${formatBytes(total)} capacity; usage unavailable`;
}

function fact(
  key: SystemInformationFact["key"],
  label: string,
  value: string | null | undefined,
): SystemInformationFact | null {
  const trimmed = value?.trim();
  return trimmed ? { key, label, value: trimmed } : null;
}
