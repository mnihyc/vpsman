import { expect, test } from "@playwright/test";
import { presentFleetAlert } from "../src/alertPresentation";
import {
  applyResolvedPolicyAlertHistory,
  applyFleetAlertStates,
  policyAlertEpisodeId,
  reconcileFleetAlertEventReviewItems,
  withoutResolvedPolicyAlertEpisodes,
} from "../src/hooks/useFleetData";
import type {
  FleetAlertRecord,
  FleetAlertStateRecord,
  PolicyAlertRecord,
} from "../src/types";

const baseAlert: FleetAlertRecord = {
  category: "resource",
  client_id: "fixture-client-01",
  detail: "Fixture condition evidence",
  escalation_level: 0,
  evidence: {},
  id: "policy-alert:fixture-alert-01",
  lifecycle: {
    last_confirmed_at: "2026-08-18T10:01:00Z",
    resolution_actor_id: null,
    resolution_note: null,
    resolution_reason: null,
    resolved_at: null,
    state: "triggered",
    trigger_generation: 1,
    triggered_at: "2026-08-18T10:00:00Z",
  },
  muted_until_unix: null,
  observed_at: "2026-08-18T10:01:00Z",
  operator_state: "open",
  record_kind: "condition",
  severity: "warning",
  state_actor_id: null,
  state_reason: null,
  state_revision: 0,
  state_updated_at: null,
  status: "policy_condition",
  target_id: "fixture-policy-rule-01",
  target_kind: "policy_rule",
  title: "Fixture policy condition",
};

function withLifecycle(
  lifecycle: Partial<FleetAlertRecord["lifecycle"]>,
): FleetAlertRecord {
  return {
    ...baseAlert,
    lifecycle: { ...baseAlert.lifecycle, ...lifecycle },
  };
}

function alertWithState(state: FleetAlertStateRecord): FleetAlertRecord {
  return {
    ...baseAlert,
    escalation_level: state.escalation_level,
    muted_until_unix: state.muted_until_unix,
    operator_state: state.state,
    state_actor_id: state.actor_id,
    state_reason: state.reason,
    state_revision: state.revision,
    state_updated_at: state.updated_at,
  };
}

test("delayed bulk state responses cannot downgrade embedded alert revisions", () => {
  const delayed: FleetAlertStateRecord = {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    alert_id: baseAlert.id,
    created_at: "2026-08-18T10:01:00Z",
    escalation_level: 0,
    muted_until_unix: null,
    reason: "delayed acknowledgement",
    revision: 11,
    state: "acknowledged",
    updated_at: "2026-08-18T10:01:00Z",
  };
  const newer: FleetAlertStateRecord = {
    ...delayed,
    escalation_level: 2,
    reason: "newer escalation",
    revision: 12,
    state: "escalated",
    updated_at: "2026-08-18T10:02:00Z",
  };
  const newerAlert = alertWithState(newer);

  expect(applyFleetAlertStates([newerAlert], [delayed])).toEqual([newerAlert]);

  expect(applyFleetAlertStates([alertWithState(delayed)], [newer])).toEqual([
    newerAlert,
  ]);
});

test("current occurrence sync resets an over-bound retained tail to a contiguous head", () => {
  const retained = Array.from({ length: 5_000 }, (_, index) => ({
    ...baseAlert,
    id: `occurrence:${index}`,
    record_kind: "event" as const,
  }));
  const head = [
    { ...retained[0], id: "occurrence:newest" },
    ...retained.slice(0, 199),
  ];

  const overflow = reconcileFleetAlertEventReviewItems(head, retained);
  expect(overflow.resetTail).toBe(true);
  expect(overflow.items).toHaveLength(200);
  expect(overflow.items.map((alert) => alert.id)).toEqual(
    head.map((alert) => alert.id),
  );

  const repeated = reconcileFleetAlertEventReviewItems(head, overflow.items);
  expect(repeated.resetTail).toBe(false);
  expect(repeated.items).toHaveLength(200);
});

test("maps only canonical policy alert public identities to episode rows", () => {
  expect(policyAlertEpisodeId("policy-alert:episode-01")).toBe("episode-01");
  expect(policyAlertEpisodeId("policy-alert:")).toBeNull();
  expect(policyAlertEpisodeId("operational-alert:episode-01")).toBeNull();

  const policyAlert: PolicyAlertRecord = {
    actual_value: 0.91,
    category: "resource",
    client_id: "fixture-client-01",
    created_at: "2026-08-18T10:00:00Z",
    detail: "Fixture occurrence evidence",
    id: "episode-01",
    last_confirmed_at: "2026-08-18T10:01:00Z",
    lifecycle_state: "persisting",
    observed_at: "2026-08-18T10:01:00Z",
    payload: {},
    policy_group_id: "group-01",
    policy_rule_id: "rule-01",
    resolution_reason: null,
    resolved_at: null,
    severity: "warning",
    threshold_value: 0.9,
    title: "Fixture occurrence",
    trigger_generation: 1,
  };
  const resolved: FleetAlertRecord = {
    ...baseAlert,
    id: "policy-alert:episode-01",
    lifecycle: {
      ...baseAlert.lifecycle,
      last_confirmed_at: "2026-08-18T10:02:00Z",
      resolution_reason: "operator_resolved",
      resolved_at: "2026-08-18T10:03:00Z",
      state: "resolved",
    },
    observed_at: "2026-08-18T10:02:00Z",
    record_kind: "event",
  };
  expect(withoutResolvedPolicyAlertEpisodes([policyAlert], [resolved])).toEqual(
    [],
  );
  expect(applyResolvedPolicyAlertHistory([policyAlert], [resolved])).toEqual([
    expect.objectContaining({
      id: "episode-01",
      last_confirmed_at: "2026-08-18T10:02:00Z",
      lifecycle_state: "resolved",
      observed_at: "2026-08-18T10:02:00Z",
      resolution_reason: "operator_resolved",
      resolved_at: "2026-08-18T10:03:00Z",
    }),
  ]);
});

test("presents lifecycle and operator triage as independent state machines", () => {
  expect(presentFleetAlert(baseAlert)).toMatchObject({
    actionable: true,
    active: true,
    current: true,
    lifecycleLabel: "Triggered",
    malformed: false,
    recordKindLabel: "Condition",
    resolvableIncident: false,
  });

  const unknown = withLifecycle({ state: "unknown" });
  expect(presentFleetAlert(unknown)).toMatchObject({
    actionable: false,
    active: false,
    current: true,
    lifecycleLabel: "Unknown",
    malformed: false,
    operatorLabel: "Open",
    resolvableIncident: false,
  });

  const recovered = withLifecycle({
    resolution_reason: "condition_recovered",
    resolved_at: "2026-08-18T10:02:00Z",
    state: "resolved",
  });
  expect(presentFleetAlert(recovered)).toMatchObject({
    active: false,
    current: false,
    lifecycleLabel: "Resolved",
    malformed: false,
    resolvableIncident: false,
  });

  const hysteresisRecovered = withLifecycle({
    resolution_reason: "recovery_expression_matched",
    resolved_at: "2026-08-18T10:02:00Z",
    state: "resolved",
  });
  expect(presentFleetAlert(hysteresisRecovered)).toMatchObject({
    current: false,
    malformed: false,
  });

  const occurrence: FleetAlertRecord = {
    ...baseAlert,
    id: "operational-alert:fixture-event-01",
    record_kind: "event",
    target_kind: "job",
  };
  expect(presentFleetAlert(occurrence)).toMatchObject({
    actionable: true,
    active: true,
    current: true,
    recordKindLabel: "Occurrence",
    resolvableIncident: true,
  });

  const resolved: FleetAlertRecord = {
    ...occurrence,
    lifecycle: {
      ...occurrence.lifecycle,
      resolution_actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
      resolution_note: "Replacement job completed.",
      resolution_reason: "operator_resolved",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    },
  };
  expect(presentFleetAlert(resolved)).toMatchObject({
    actionable: false,
    active: false,
    current: false,
    lifecycleLabel: "Resolved",
    malformed: false,
    resolvableIncident: false,
  });

  const elapsed: FleetAlertRecord = {
    ...occurrence,
    lifecycle: {
      ...occurrence.lifecycle,
      resolution_reason: "policy_time_elapsed",
      resolved_at: "2026-08-25T10:00:00Z",
      state: "resolved",
    },
  };
  expect(presentFleetAlert(elapsed)).toMatchObject({
    current: false,
    malformed: false,
    resolvableIncident: false,
  });

  const deletedByPolicy: FleetAlertRecord = {
    ...occurrence,
    lifecycle: {
      ...occurrence.lifecycle,
      resolution_reason: "policy_deleted",
      resolved_at: "2026-08-18T10:03:00Z",
      state: "resolved",
    },
  };
  expect(presentFleetAlert(deletedByPolicy)).toMatchObject({
    current: false,
    malformed: false,
    resolvableIncident: false,
  });
});

test("fails closed for impossible lifecycle shape, causality, or provenance", () => {
  const malformedAlerts: FleetAlertRecord[] = [
    withLifecycle({ trigger_generation: 0 }),
    withLifecycle({ triggered_at: "" }),
    withLifecycle({ last_confirmed_at: null as unknown as string }),
    withLifecycle({ last_confirmed_at: "2026-08-18T09:59:00Z" }),
    withLifecycle({
      last_confirmed_at: "2026-08-18T09:59:00Z",
      state: "unknown",
    }),
    {
      ...withLifecycle({ state: "unknown" }),
      record_kind: "event",
    },
    withLifecycle({
      resolution_reason: "condition_recovered",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "unknown",
    }),
    withLifecycle({
      resolution_reason: "condition_recovered",
      resolved_at: "2026-08-18T09:59:00Z",
      state: "resolved",
    }),
    withLifecycle({
      resolution_reason: "condition_recovered",
      resolved_at: "2026-08-18T10:00:30Z",
      state: "resolved",
    }),
    withLifecycle({
      last_confirmed_at: null as unknown as string,
      resolution_reason: "condition_recovered",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    withLifecycle({ resolved_at: "2026-08-18T10:02:00Z", state: "resolved" }),
    withLifecycle({
      resolution_actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
      resolution_note: "A condition cannot be operator-resolved.",
      resolution_reason: "operator_resolved",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    withLifecycle({
      resolution_actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
      resolution_note: " ",
      resolution_reason: "operator_resolved",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    withLifecycle({
      resolution_actor_id: "not-a-uuid",
      resolution_note: "Reviewed.",
      resolution_reason: "operator_resolved",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    withLifecycle({
      resolution_reason: "unsupported_reason",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    {
      ...withLifecycle({
        resolution_reason: "condition_recovered",
        resolved_at: "2026-08-18T10:02:00Z",
        state: "resolved",
      }),
      record_kind: "event",
    },
    withLifecycle({
      resolution_reason: "policy_time_elapsed",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    {
      ...withLifecycle({
        resolution_reason: "recovery_expression_matched",
        resolved_at: "2026-08-18T10:02:00Z",
        state: "resolved",
      }),
      record_kind: "event",
    },
    withLifecycle({
      resolution_actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
      resolution_note: "Operator-only provenance on an automatic edge.",
      resolution_reason: "condition_recovered",
      resolved_at: "2026-08-18T10:02:00Z",
      state: "resolved",
    }),
    { ...baseAlert, operator_state: "missing" },
    { ...baseAlert, record_kind: "missing" },
  ];

  for (const alert of malformedAlerts) {
    expect(presentFleetAlert(alert)).toMatchObject({
      actionable: false,
      active: false,
      current: false,
      malformed: true,
      resolvableIncident: false,
    });
  }
});
