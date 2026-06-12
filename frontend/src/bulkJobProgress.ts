import type { AgentView, JobOutputRecord, JobTargetRecord } from "./types";

export type BulkFailureReason = {
  reason: string;
  target: string;
};

export type BulkJobProgress = {
  accepted: number;
  completed: number;
  doing: number;
  expected: number;
  failed: number;
  failureReasons?: BulkFailureReason[];
  jobId: string;
  retrieved: number;
  unavailable: number;
};

export const DEFAULT_BULK_PROGRESS_POLL_INTERVAL_MS = 500;
export const DEFAULT_BULK_PROGRESS_TIMEOUT_MS = 90_000;
export const BULK_PROGRESS_TIMEOUT_MARGIN_MS = 35_000;

export function bulkProgressTimeoutMs(timeoutSecs: number | undefined): number {
  if (!Number.isFinite(timeoutSecs ?? NaN)) {
    return DEFAULT_BULK_PROGRESS_TIMEOUT_MS;
  }
  return Math.max(DEFAULT_BULK_PROGRESS_TIMEOUT_MS, Math.ceil(Math.max(1, timeoutSecs ?? 1)) * 1000 + BULK_PROGRESS_TIMEOUT_MARGIN_MS);
}

export function buildBulkJobProgress({
  acceptedTargets,
  jobId,
  outputs = [],
  targetRecords,
  targets,
}: {
  acceptedTargets: number;
  jobId: string;
  outputs?: JobOutputRecord[];
  targetRecords: JobTargetRecord[];
  targets: AgentView[];
}): BulkJobProgress {
  const targetRecordByClient = new Map(targetRecords.map((target) => [target.client_id, target]));
  const outputClientIds = new Set(outputs.filter((output) => output.done).map((output) => output.client_id));
  let completed = 0;
  let failed = 0;
  const failureReasons: BulkFailureReason[] = [];
  let retrieved = 0;
  let unavailable = 0;

  for (const target of targets) {
    const targetRecord = targetRecordByClient.get(target.id);
    if (targetPreflightUnavailable(target)) {
      unavailable += 1;
      continue;
    }
    if (!targetRecord) {
      continue;
    }
    if (targetRecordTerminal(targetRecord.status) || outputClientIds.has(target.id)) {
      retrieved += 1;
    }
    if (targetRecordSucceeded(targetRecord.status)) {
      completed += 1;
    } else if (targetRecordFailed(targetRecord.status)) {
      failed += 1;
      failureReasons.push(targetFailureReason(target, targetRecord));
    }
  }

  const accepted = acceptedDispatchTargetCount(acceptedTargets, targets);
  const doing = Math.max(0, accepted - completed - failed);
  return {
    accepted,
    completed,
    doing,
    expected: targets.length,
    failed,
    failureReasons,
    jobId,
    retrieved,
    unavailable,
  };
}

export function acceptedDispatchTargetCount(acceptedTargets: number, targets: AgentView[]): number {
  const dispatchableTargets = targets.filter((target) => !targetPreflightUnavailable(target)).length;
  return Math.min(acceptedTargets, dispatchableTargets);
}

export async function waitForBulkJobTargets(
  jobId: string,
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>,
  options: {
    acceptedTargets: number;
    intervalMs?: number;
    onProgress?: (progress: BulkJobProgress) => void;
    targets: AgentView[];
    timeoutMs?: number;
  },
): Promise<{ progress: BulkJobProgress; targets: JobTargetRecord[] }> {
  let lastTargets: JobTargetRecord[] = [];
  let progress = buildBulkJobProgress({
    acceptedTargets: options.acceptedTargets,
    jobId,
    targetRecords: lastTargets,
    targets: options.targets,
  });
  const deadline = Date.now() + (options.timeoutMs ?? DEFAULT_BULK_PROGRESS_TIMEOUT_MS);
  const intervalMs = options.intervalMs ?? DEFAULT_BULK_PROGRESS_POLL_INTERVAL_MS;
  while (Date.now() <= deadline) {
    try {
      lastTargets = await onLoadTargets(jobId);
    } catch {
      // Keep polling. A transient target-history fetch failure should not hide the run.
    }
    progress = buildBulkJobProgress({
      acceptedTargets: options.acceptedTargets,
      jobId,
      targetRecords: lastTargets,
      targets: options.targets,
    });
    options.onProgress?.(progress);
    if (progress.completed + progress.failed + progress.unavailable >= progress.expected) {
      return { progress, targets: lastTargets };
    }
    await new Promise((resolve) => window.setTimeout(resolve, intervalMs));
  }
  return { progress, targets: lastTargets };
}

export function bulkProgressLabel(progress: BulkJobProgress): string {
  return [
    `active ${progress.accepted}/${progress.expected}`,
    `doing ${progress.doing}`,
    `retrieved ${progress.retrieved}`,
    `done ${progress.completed}`,
    progress.unavailable > 0 ? `unavailable ${progress.unavailable}` : "",
    progress.failed > 0 ? `failed ${progress.failed}` : "",
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
  if (progress.unavailable > 0 && progress.failed > 0 && progress.completed > 0) {
    return `partial success: ${progress.completed} done, ${progress.failed} failed, ${progress.unavailable} unavailable`;
  }
  if (progress.unavailable > 0 && progress.completed > 0 && progress.failed === 0) {
    return `partial success: ${progress.completed} done, ${progress.unavailable} unavailable`;
  }
  if (progress.failed > 0 && progress.completed > 0) {
    return `partial success: ${progress.completed} done, ${progress.failed} failed`;
  }
  if (progress.failed > 0 && progress.completed === 0 && progress.unavailable > 0) {
    return `failed on ${vpsCountLabel(progress.failed)}, ${progress.unavailable} unavailable`;
  }
  if (progress.failed > 0 && progress.completed === 0) {
    return `failed on ${vpsCountLabel(progress.failed)}`;
  }
  if (progress.completed === progress.expected && progress.expected > 0) {
    return `completed on ${vpsCountLabel(progress.completed)}`;
  }
  if (progress.unavailable === progress.expected && progress.expected > 0) {
    return `${progress.unavailable} unavailable`;
  }
  return bulkProgressLabel(progress);
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

export function targetRecordSucceeded(status: string | undefined): boolean {
  return ["completed", "degraded_unprivileged", "done", "ok", "skipped", "succeeded", "unchanged"].includes((status ?? "").toLowerCase());
}

export function targetRecordFailed(status: string | undefined): boolean {
  return [
    "agent_timed_out",
    "canceled",
    "control_timed_out",
    "dispatch_failed",
    "failed",
    "rejected",
    "rejected_authorization_required",
    "rejected_by_agent",
    "timed_out",
  ].includes((status ?? "").toLowerCase());
}

export function targetRecordTerminal(status: string | undefined): boolean {
  const normalized = (status ?? "").toLowerCase();
  return targetRecordSucceeded(normalized) || targetRecordFailed(normalized);
}

function targetFailureReason(target: AgentView, targetRecord: JobTargetRecord): BulkFailureReason {
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
