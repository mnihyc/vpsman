import {
  JOB_STATUS_CLASS_BY_STATUS,
  JOB_TARGET_STATUSES,
  JOB_TARGET_STATUS_CLASS_BY_STATUS,
} from "./generated/protocolContracts";
import type {
  GeneratedJobStatus,
  GeneratedJobStatusClass,
  GeneratedJobTargetStatus,
  GeneratedJobTargetStatusClass,
} from "./generated/protocolContracts";
import type { JobOutputComparisonStatus } from "./types";

const JOB_TARGET_STATUS_SET = new Set<GeneratedJobTargetStatus>(JOB_TARGET_STATUSES);

export function isJobTargetStatus(status: string): status is GeneratedJobTargetStatus {
  return JOB_TARGET_STATUS_SET.has(status as GeneratedJobTargetStatus);
}

export function jobStatusBadgeClass(status: GeneratedJobStatus): string {
  return jobStatusClassBadge(JOB_STATUS_CLASS_BY_STATUS[status]);
}

export function jobTargetStatusBadgeClass(status: GeneratedJobTargetStatus): string {
  return jobTargetStatusClassBadge(JOB_TARGET_STATUS_CLASS_BY_STATUS[status]);
}

export function jobOutputComparisonStatusBadgeClass(status: JobOutputComparisonStatus): string {
  return status === "unknown" ? "warn" : jobTargetStatusBadgeClass(status);
}

function jobStatusClassBadge(statusClass: GeneratedJobStatusClass): string {
  switch (statusClass) {
    case "in_progress":
      return "info";
    case "successful":
      return "ok";
    case "partial_success":
    case "unsuccessful":
      return "warn";
    case "skipped":
      return "neutral";
  }
}

function jobTargetStatusClassBadge(statusClass: GeneratedJobTargetStatusClass): string {
  switch (statusClass) {
    case "in_progress":
      return "info";
    case "successful":
      return "ok";
    case "skipped":
      return "neutral";
    case "unsuccessful":
      return "warn";
  }
}
