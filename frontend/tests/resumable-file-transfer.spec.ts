import { expect, test } from "@playwright/test";

import {
  buildBulkJobProgress,
  bulkProgressLabel,
  waitForBulkJobSet,
} from "../src/bulkJobProgress";
import type {
  AgentView,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetStatus,
} from "../src/types";

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

test("bulk progress prefers retained stderr over a generic failed status", () => {
  const target = {
    ...runningTarget("1700000000", "1700000120"),
    completed_at: "1700000010",
    message: "failed",
    status: "failed" as JobTargetStatus,
  };
  const output: JobOutputRecord = {
    client_id: "client-a",
    created_at: "1700000010",
    data_base64: btoa("ping: connect: Network is unreachable"),
    done: true,
    exit_code: 2,
    job_id: "job-a",
    seq: 1,
    stream: "stderr",
  };

  const progress = buildBulkJobProgress({
    jobId: "job-a",
    outputs: [output],
    targetRecords: [target],
    targets: [TARGET],
  });

  expect(progress.failureReasons).toEqual([
    {
      reason: "ping: connect: Network is unreachable",
      target: "vps-a",
    },
  ]);
});

test("bulk progress aggregates target-specific jobs into one operation", async () => {
  const targets = [
    TARGET,
    { ...TARGET, display_name: "vps-b", id: "client-b" },
  ];
  const records: Record<string, JobTargetRecord[]> = {
    "job-a": [
      {
        ...runningTarget("1700000000", "1700000120"),
        completed_at: "1700000010",
        status: "completed" as JobTargetStatus,
      },
    ],
    "job-b": [
      {
        ...runningTarget("1700000000", "1700000120"),
        client_id: "client-b",
        completed_at: "1700000011",
        job_id: "job-b",
        status: "completed" as JobTargetStatus,
      },
    ],
  };

  const result = await waitForBulkJobSet(
    ["job-a", "job-b"],
    async (jobId) => records[jobId] ?? [],
    {
      intervalMs: 0,
      operationId: "operation-a",
      targetCount: 2,
      targets,
    },
  );

  expect(result.progress.jobId).toBe("operation-a");
  expect(result.progress.jobIds).toEqual(["job-a", "job-b"]);
  expect(result.progress.completed).toBe(2);
  expect(result.progress.terminal).toBe(2);
  expect(result.progress.in_progress).toBe(0);
});
