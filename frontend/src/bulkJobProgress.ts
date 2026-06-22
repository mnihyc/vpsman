import {
  JOB_TARGET_STATUSES,
  JOB_TARGET_STATUS_CLASS_BY_STATUS,
  JOB_TARGET_TERMINAL_STATUSES,
} from "./generated/protocolContracts";
import type { AgentView, JobOutputRecord, JobTargetRecord } from "./types";
import type {
  GeneratedJobTargetStatus,
  GeneratedJobTargetStatusClass,
} from "./generated/protocolContracts";

export type BulkFailureReason = {
  reason: string;
  target: string;
};

export type BulkJobProgress = {
  agent_timeout: number;
  agent_lost: number;
  canceled: number;
  completed: number;
  control_timeout: number;
  control_grace: number;
  deadline_overdue: number;
  dispatching: number;
  failed: number;
  failureReasons?: BulkFailureReason[];
  in_progress: number;
  jobId: string;
  queued: number;
  rejected: number;
  retrieved: number;
  running: number;
  skipped: number;
  successful: number;
  terminal: number;
  total: number;
  unavailable: number;
  unsuccessful: number;
};

export const DEFAULT_BULK_PROGRESS_POLL_INTERVAL_MS = 500;

export function buildBulkJobProgress({
  jobId,
  nowMs = Date.now(),
  outputs = [],
  targetCount,
  targetRecords,
  targets,
  timeoutSecs,
}: {
  jobId: string;
  nowMs?: number;
  outputs?: JobOutputRecord[];
  targetCount?: number;
  targetRecords: JobTargetRecord[];
  targets: AgentView[];
  timeoutSecs?: number;
}): BulkJobProgress {
  const targetRecordByClient = new Map(targetRecords.map((target) => [target.client_id, target]));
  const targetByClient = new Map(targets.map((target) => [target.id, target]));
  const outputClientIds = new Set(outputs.filter((output) => output.done).map((output) => output.client_id));
  const total = Math.max(0, targetCount ?? targets.length, targets.length, targetRecords.length);
  const jobTimeoutMs = Number.isFinite(timeoutSecs ?? NaN)
    ? Math.ceil(Math.max(1, timeoutSecs ?? 1)) * 1000
    : null;
  let agent_timeout = 0;
  let agent_lost = 0;
  let canceled = 0;
  let completed = 0;
  let control_timeout = 0;
  let control_grace = 0;
  let deadline_overdue = 0;
  let dispatching = 0;
  let failed = 0;
  const failureReasons: BulkFailureReason[] = [];
  let queued = 0;
  let rejected = 0;
  let retrieved = 0;
  let running = 0;
  let skipped = 0;
  let unavailable = 0;

  for (const target of targets) {
    if (targetPreflightUnavailable(target)) {
      unavailable += 1;
    }
  }

  for (const targetRecord of targetRecords) {
    const target = targetByClient.get(targetRecord.client_id) ?? {
      display_name: targetRecord.client_id,
      id: targetRecord.client_id,
      status: "unknown",
    };
    const statusClass = targetRecordStatusClass(targetRecord.status);
    switch (targetRecord.status) {
      case "queued":
        queued += 1;
        break;
      case "dispatching":
        dispatching += 1;
        break;
      case "running":
        running += 1;
        break;
      case "completed":
        completed += 1;
        break;
      case "skipped":
        skipped += 1;
        break;
      case "rejected":
        rejected += 1;
        break;
      case "failed":
        failed += 1;
        break;
      case "agent_lost":
        agent_lost += 1;
        break;
      case "agent_timeout":
        agent_timeout += 1;
        break;
      case "control_timeout":
        control_timeout += 1;
        break;
      case "canceled":
        canceled += 1;
        break;
      default:
        failed += 1;
        break;
    }
    if (statusClass === "unsuccessful") {
      failureReasons.push(targetFailureReason(target, targetRecord));
    }
    if (targetRecordTerminal(targetRecord.status) || outputClientIds.has(targetRecord.client_id)) {
      retrieved += 1;
    }
    if (
      jobTimeoutMs !== null &&
      (targetRecord.status === "dispatching" || targetRecord.status === "running")
    ) {
      const startedAtMs = parseBackendTimestampMs(targetRecord.started_at);
      const deadlineAtMs = parseBackendTimestampMs(targetRecord.deadline_at);
      if (startedAtMs !== null && deadlineAtMs !== null && nowMs >= startedAtMs + jobTimeoutMs) {
        if (nowMs >= deadlineAtMs) {
          deadline_overdue += 1;
        } else {
          control_grace += 1;
        }
      }
    }
  }

  for (const clientId of outputClientIds) {
    if (!targetRecordByClient.has(clientId)) {
      retrieved += 1;
    }
  }

  queued += Math.max(0, total - targetRecords.length);
  const in_progress = queued + dispatching + running;
  const successful = completed;
  const unsuccessful = rejected + failed + agent_lost + agent_timeout + control_timeout + canceled;
  const terminal = successful + skipped + unsuccessful;
  return {
    agent_timeout,
    agent_lost,
    canceled,
    completed,
    control_timeout,
    control_grace,
    deadline_overdue,
    dispatching,
    failed,
    failureReasons,
    in_progress,
    jobId,
    queued,
    rejected,
    retrieved,
    running,
    skipped,
    successful,
    terminal,
    total,
    unavailable,
    unsuccessful,
  };
}

export function createJobTargetCount(job: { target_count: number; target_counts: { total: number } }): number {
  return Math.max(0, job.target_counts.total);
}

export function targetRecordCompleted(status: GeneratedJobTargetStatus | undefined): boolean {
  return targetRecordStatusClass(status) === "successful";
}

export function targetRecordSkipped(status: GeneratedJobTargetStatus | undefined): boolean {
  return targetRecordStatusClass(status) === "skipped";
}

export function targetRecordFailed(status: GeneratedJobTargetStatus | undefined): boolean {
  return targetRecordStatusClass(status) === "unsuccessful";
}

export function targetRecordTerminal(status: GeneratedJobTargetStatus | undefined): boolean {
  return status !== undefined && TARGET_TERMINAL_STATUS_SET.has(status);
}

function targetRecordStatusClass(status: GeneratedJobTargetStatus | undefined): GeneratedJobTargetStatusClass | undefined {
  return status !== undefined && TARGET_STATUS_SET.has(status) ? JOB_TARGET_STATUS_CLASS_BY_STATUS[status] : undefined;
}

const TARGET_STATUS_SET = new Set<GeneratedJobTargetStatus>(JOB_TARGET_STATUSES);
const TARGET_TERMINAL_STATUS_SET = new Set<GeneratedJobTargetStatus>(JOB_TARGET_TERMINAL_STATUSES);

export async function waitForBulkJobTargets(
  jobId: string,
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>,
  options: {
    intervalMs?: number;
    onProgress?: (progress: BulkJobProgress) => void;
    targetCount?: number;
    targets: AgentView[];
    timeoutSecs?: number;
  },
): Promise<{ progress: BulkJobProgress; targets: JobTargetRecord[] }> {
  let lastTargets: JobTargetRecord[] = [];
  let progress = buildBulkJobProgress({
    jobId,
    targetCount: options.targetCount,
    targetRecords: lastTargets,
    targets: options.targets,
    timeoutSecs: options.timeoutSecs,
  });
  const intervalMs = options.intervalMs ?? DEFAULT_BULK_PROGRESS_POLL_INTERVAL_MS;
  for (;;) {
    try {
      lastTargets = await onLoadTargets(jobId);
    } catch {
      // Keep polling. A transient target-history fetch failure should not hide the run.
    }
    progress = buildBulkJobProgress({
      jobId,
      targetCount: options.targetCount,
      targetRecords: lastTargets,
      targets: options.targets,
      timeoutSecs: options.timeoutSecs,
    });
    options.onProgress?.(progress);
    if (progress.total === 0 || progress.terminal >= progress.total) {
      return { progress, targets: lastTargets };
    }
    await new Promise((resolve) => window.setTimeout(resolve, intervalMs));
  }
}

export function bulkProgressLabel(progress: BulkJobProgress): string {
  return [
    `targets ${progress.terminal}/${progress.total}`,
    `in progress ${progress.in_progress}`,
    `retrieved ${progress.retrieved}`,
    `completed ${progress.completed}`,
    progress.skipped > 0 ? `skipped ${progress.skipped}` : "",
    progress.unavailable > 0 ? `unavailable ${progress.unavailable}` : "",
    progress.unsuccessful > 0 ? `unsuccessful ${progress.unsuccessful}` : "",
    progress.rejected > 0 ? `rejected ${progress.rejected}` : "",
    progress.failed > 0 ? `failed ${progress.failed}` : "",
    progress.control_grace > 0 ? `control grace ${progress.control_grace}` : "",
    progress.deadline_overdue > 0 ? `deadline overdue ${progress.deadline_overdue}` : "",
    progress.agent_lost > 0 ? `agent_lost ${progress.agent_lost}` : "",
    progress.agent_timeout > 0 ? `agent_timeout ${progress.agent_timeout}` : "",
    progress.control_timeout > 0 ? `control_timeout ${progress.control_timeout}` : "",
    progress.canceled > 0 ? `canceled ${progress.canceled}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

export function targetPreflightUnavailable(target: AgentView): boolean {
  return ["deleted", "disconnected", "never", "offline", "revoked"].includes(target.status);
}

export function targetAvailabilityCounts(targets: AgentView[]): { online: number; stale: number; unavailable: number } {
  return {
    online: targets.filter((target) => target.status === "online").length,
    stale: targets.filter((target) => target.status === "stale").length,
    unavailable: targets.filter(targetPreflightUnavailable).length,
  };
}

export function formatTargetAvailabilitySummary(targets: AgentView[]): string {
  const counts = targetAvailabilityCounts(targets);
  if (counts.stale === 0 && counts.unavailable === 0) {
    return `${targets.length} resolved`;
  }
  const parts = [`${counts.online} online`];
  if (counts.stale > 0) {
    parts.push(`${counts.stale} stale`);
  }
  if (counts.unavailable > 0) {
    parts.push(`${counts.unavailable} unavailable`);
  }
  return `${targets.length} resolved (${parts.join(", ")})`;
}

export function bulkOutcomeSummary(progress: BulkJobProgress): string {
  if (progress.completed > 0 && (progress.unsuccessful > 0 || progress.skipped > 0)) {
    return [
      `partial success: ${progress.completed} completed`,
      progress.unsuccessful > 0 ? `${progress.unsuccessful} unsuccessful` : "",
      progress.skipped > 0 ? `${progress.skipped} skipped` : "",
    ]
      .filter(Boolean)
      .join(", ");
  }
  if (progress.skipped > 0 && progress.unsuccessful === 0 && progress.terminal >= progress.total) {
    return `partial success: ${progress.skipped} skipped`;
  }
  if (progress.unsuccessful > 0 && progress.skipped > 0) {
    return `unsuccessful on ${vpsCountLabel(progress.unsuccessful)}, ${progress.skipped} skipped`;
  }
  if (progress.unsuccessful > 0) {
    return `unsuccessful on ${vpsCountLabel(progress.unsuccessful)}`;
  }
  if (progress.completed === progress.total && progress.total > 0 && progress.in_progress === 0) {
    return `completed on ${vpsCountLabel(progress.completed)}`;
  }
  return bulkProgressLabel(progress);
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function parseBackendTimestampMs(value: string | null | undefined): number | null {
  const trimmed = value?.trim();
  if (!trimmed) {
    return null;
  }
  if (/^\d+$/.test(trimmed)) {
    const parsed = Number.parseInt(trimmed, 10);
    if (!Number.isFinite(parsed)) {
      return null;
    }
    return trimmed.length <= 10 ? parsed * 1000 : parsed;
  }
  const parsed = Date.parse(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

function targetFailureReason(target: Pick<AgentView, "display_name" | "id" | "status">, targetRecord: JobTargetRecord): BulkFailureReason {
  const message = targetRecord.message?.trim();
  const rawReason = message || targetRecord.status || "failed";
  const reason =
    target.status === "stale" && !rawReason.toLowerCase().includes("stale")
      ? `stale: ${rawReason}`
      : rawReason;
  return {
    reason,
    target: target.display_name || target.id,
  };
}
