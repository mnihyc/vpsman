import { expect, test } from "@playwright/test";

import { buildBulkJobProgress, bulkProgressLabel } from "../src/bulkJobProgress";
import type { AgentView, JobTargetRecord, JobTargetStatus } from "../src/types";

const TARGET: AgentView = {
  display_name: "vps-a",
  id: "client-a",
  status: "online",
} as AgentView;

function runningTarget(startedAt: string, deadlineAt: string): JobTargetRecord {
  return {
    job_id: "job-a",
    client_id: "client-a",
    status: "running" as JobTargetStatus,
    message: null,
    exit_code: null,
    started_at: startedAt,
    deadline_at: deadlineAt,
    completed_at: null,
  };
}

test("bulk progress does not mark grace before job timeout", () => {
  const progress = buildBulkJobProgress({
    jobId: "job-a",
    nowMs: 1_700_000_020_000,
    targetRecords: [runningTarget("1700000000", "1700000120")],
    targets: [TARGET],
    maxTimeoutSecs: 60,
  });

  expect(progress.control_grace).toBe(0);
  expect(progress.deadline_overdue).toBe(0);
  expect(bulkProgressLabel(progress)).not.toContain("control grace");
  expect(bulkProgressLabel(progress)).not.toContain("deadline overdue");
});

test("bulk progress labels backend control grace after job timeout", () => {
  const progress = buildBulkJobProgress({
    jobId: "job-a",
    nowMs: 1_700_000_070_000,
    targetRecords: [runningTarget("1700000000", "1700000120")],
    targets: [TARGET],
    maxTimeoutSecs: 60,
  });

  expect(progress.control_grace).toBe(1);
  expect(progress.deadline_overdue).toBe(0);
  expect(bulkProgressLabel(progress)).toContain("control grace 1");
});

test("bulk progress labels backend deadline overdue active targets", () => {
  const progress = buildBulkJobProgress({
    jobId: "job-a",
    nowMs: 1_700_000_121_000,
    targetRecords: [runningTarget("1700000000", "1700000120")],
    targets: [TARGET],
    maxTimeoutSecs: 60,
  });

  expect(progress.control_grace).toBe(0);
  expect(progress.deadline_overdue).toBe(1);
  expect(bulkProgressLabel(progress)).toContain("deadline overdue 1");
});
