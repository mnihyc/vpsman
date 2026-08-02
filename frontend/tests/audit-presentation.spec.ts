import { expect, test } from "@playwright/test";
import {
  auditClientIds,
  auditSessionSearchText,
  presentAudit,
} from "../src/auditPresentation";
import type { AuditLogRecord, JsonValue } from "../src/types";

function auditRecord(
  metadata: JsonValue,
  overrides: Partial<AuditLogRecord> = {},
): AuditLogRecord {
  return {
    action: "audit.test",
    actor_id: null,
    command_hash: null,
    created_at: "2026-08-02T00:00:00Z",
    id: "audit-test",
    metadata,
    target: "monitoring_share:11111111-2222-4333-8444-555555555555",
    ...overrides,
  };
}

test("canonical audit presenter identifies public visitors and monitoring targets", () => {
  const presentation = presentAudit(
    auditRecord({
      component: "monitoring-share-controller",
      origin_kind: "public_share",
      remote_ip: "203.0.113.80",
      result: "succeeded",
      visitor_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    }),
  );

  expect(presentation.actorLabel).toBe("Public visitor");
  expect(presentation.actorDetail).toBe("Visitor aaaaaaaa");
  expect(presentation.sourceIp).toBe("203.0.113.80");
  expect(presentation.targetLabel).toBe("Monitoring share 11111111");
});

test("canonical audit presenter keeps execution privilege separate from scope", () => {
  const presentation = presentAudit(
    auditRecord(
      {
        force_unprivileged: false,
        privilege_scope: "job.dispatch",
        privileged: true,
        result: "requested",
      },
      { target: "ping_targets:bulk" },
    ),
  );

  expect(presentation.privilege).toBe("job.dispatch");
  expect(presentation.executionPrivilege).toBe(
    "Privileged requested · Force-unprivileged disabled",
  );
  expect(presentation.targetLabel).toBe("Ping targets Bulk");
});

test("canonical audit presenter extracts only exact linked job arrays and revoked sessions", () => {
  const revokedSessionId = "aaaaaaaa-1111-4222-8333-bbbbbbbbbbbb";
  const firstJobId = "11111111-1111-4111-8111-111111111111";
  const secondJobId = "22222222-2222-4222-8222-222222222222";
  const record = auditRecord({
    agent_lost_job_ids: [firstJobId, 7, ""],
    nested: { job_id: "33333333-3333-4333-8333-333333333333" },
    result: "ignored",
    revoked_operator_session_id: revokedSessionId,
    skipped_unstarted_job_ids: [secondJobId, firstJobId],
  });
  const presentation = presentAudit(record);

  expect(
    presentation.evidenceReferences
      .filter((reference) => reference.kind === "Job")
      .map((reference) => reference.value),
  ).toEqual([firstJobId, secondJobId]);
  expect(auditSessionSearchText(record)).toContain(revokedSessionId);
  expect(presentation.outcomeTone).toBe("warning");
});

test("canonical audit presenter assigns explicit tones to audited failure states", () => {
  const expected = {
    failure: "critical",
    ignored: "warning",
    throttled: "warning",
    unavailable: "warning",
  } as const;

  for (const [result, tone] of Object.entries(expected)) {
    expect(presentAudit(auditRecord({ result })).outcomeTone).toBe(tone);
  }
});

test("canonical audit presenter recognizes the retained public-share source IP key", () => {
  const presentation = presentAudit(
    auditRecord({
      component: "monitoring-share-controller",
      origin_kind: "public_share",
      result: "succeeded",
      source_ip: "203.0.113.81",
      visitor_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    }),
  );

  expect(presentation.sourceIp).toBe("203.0.113.81");
});

test("canonical audit presenter reads the historical monitoring details envelope", () => {
  const record = auditRecord({
    component: "monitoring-controller",
    details: {
      schedule_id: "11111111-2222-4333-8444-555555555555",
      target_client_ids: ["v-1", "v-15"],
      target_count: 2,
    },
    origin_kind: "operator_request",
    result: "succeeded",
  });
  const presentation = presentAudit(record);

  expect(presentation.actionDetail).toContain("2 targets");
  expect(auditClientIds(record)).toEqual(["v-1", "v-15"]);
  expect(
    presentation.evidenceReferences.find(
      (reference) => reference.kind === "Schedule",
    )?.value,
  ).toBe("11111111-2222-4333-8444-555555555555");
});

test("canonical audit presenter uses retained job target status as outcome only for job results", () => {
  expect(
    presentAudit(
      auditRecord(
        { status: "skipped" },
        { action: "job.target_result", target: "client:v-1" },
      ),
    ).outcomeLabel,
  ).toBe("Skipped");
  expect(
    presentAudit(auditRecord({ details: { status: "failed" } })).outcomeLabel,
  ).toBe("Outcome not recorded");
});
