import type { FleetAlertRecord, PolicyAlertRecord } from "./types";

export type AlertLifecycleState =
  | "triggered"
  | "persisting"
  | "unknown"
  | "resolved";
export type AlertRecordKind = "condition" | "event";
export type AlertOperatorState =
  | "open"
  | "acknowledged"
  | "muted"
  | "escalated";

export type FleetAlertPresentation = {
  actionable: boolean;
  active: boolean;
  current: boolean;
  lifecycleLabel: string;
  lifecycleState: AlertLifecycleState | null;
  lifecycleTone: "warning" | "info" | "ok" | "neutral";
  malformed: boolean;
  operatorLabel: string;
  operatorState: AlertOperatorState | null;
  recordKind: AlertRecordKind | null;
  recordKindLabel: string;
  resolvableIncident: boolean;
};

const ALERT_LIFECYCLE_STATES = new Set<AlertLifecycleState>([
  "triggered",
  "persisting",
  "unknown",
  "resolved",
]);
const ALERT_RECORD_KINDS = new Set<AlertRecordKind>(["condition", "event"]);
const ALERT_OPERATOR_STATES = new Set<AlertOperatorState>([
  "open",
  "acknowledged",
  "muted",
  "escalated",
]);
const AUTOMATIC_RESOLUTION_REASONS = new Set([
  "condition_recovered",
  "source_scope_exited",
  "policy_scope_exited",
  "policy_scope_changed",
  "policy_disabled",
  "policy_changed",
  "policy_deleted",
]);
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function presentFleetAlert(
  alert: FleetAlertRecord,
): FleetAlertPresentation {
  const recordKind = knownToken(alert.record_kind, ALERT_RECORD_KINDS);
  const lifecycleState = validLifecycleState(alert.lifecycle, recordKind);
  const operatorState = knownToken(alert.operator_state, ALERT_OPERATOR_STATES);
  const malformed =
    recordKind === null || lifecycleState === null || operatorState === null;
  const active =
    !malformed &&
    (lifecycleState === "triggered" || lifecycleState === "persisting");
  const current = !malformed && lifecycleState !== "resolved";
  const operatorActionable =
    operatorState === "open" || operatorState === "escalated";

  return {
    actionable: active && operatorActionable,
    active,
    current,
    lifecycleLabel: alertLifecycleLabel(lifecycleState),
    lifecycleState,
    lifecycleTone: alertLifecycleTone(lifecycleState),
    malformed,
    operatorLabel: alertOperatorStateLabel(operatorState),
    operatorState,
    recordKind,
    recordKindLabel: alertRecordKindLabel(recordKind),
    resolvableIncident:
      !malformed &&
      recordKind === "event" &&
      (lifecycleState === "triggered" || lifecycleState === "persisting"),
  };
}

export function isActiveFleetAlert(alert: FleetAlertRecord): boolean {
  return presentFleetAlert(alert).active;
}

export function isActionableFleetAlert(alert: FleetAlertRecord): boolean {
  return presentFleetAlert(alert).actionable;
}

export function isCurrentFleetAlert(alert: FleetAlertRecord): boolean {
  return presentFleetAlert(alert).current;
}

export function isActivePolicyAlert(alert: PolicyAlertRecord): boolean {
  return (
    alert.lifecycle_state === "triggered" ||
    alert.lifecycle_state === "persisting"
  );
}

export function isCurrentPolicyAlert(alert: PolicyAlertRecord): boolean {
  return (
    isActivePolicyAlert(alert) ||
    (alert.lifecycle_state === "unknown" && alert.last_confirmed_at !== null)
  );
}

export function isResolvedPolicyAlert(alert: PolicyAlertRecord): boolean {
  return alert.lifecycle_state === "resolved";
}

export function alertLifecycleLabel(
  lifecycleState: AlertLifecycleState | string | null | undefined,
): string {
  switch (lifecycleState) {
    case "triggered":
      return "Triggered";
    case "persisting":
      return "Persisting";
    case "unknown":
      return "Unknown";
    case "resolved":
      return "Resolved";
    default:
      return "Lifecycle unavailable";
  }
}

export function alertLifecycleTone(
  lifecycleState: AlertLifecycleState | string | null | undefined,
): "warning" | "info" | "ok" | "neutral" {
  switch (lifecycleState) {
    case "triggered":
      return "warning";
    case "persisting":
      return "info";
    case "resolved":
      return "ok";
    default:
      return "neutral";
  }
}

export function alertOperatorStateLabel(
  operatorState: AlertOperatorState | string | null | undefined,
): string {
  switch (operatorState) {
    case "open":
      return "Open";
    case "acknowledged":
      return "Acknowledged";
    case "muted":
      return "Muted";
    case "escalated":
      return "Escalated";
    default:
      return "Triage unavailable";
  }
}

export function alertRecordKindLabel(
  recordKind: AlertRecordKind | string | null | undefined,
): string {
  switch (recordKind) {
    case "condition":
      return "Condition";
    case "event":
      return "Occurrence";
    default:
      return "Record kind unavailable";
  }
}

export function alertCategoryLabel(category: unknown): string {
  switch (category) {
    case "agent_status":
      return "Agent status";
    case "backup":
      return "Backup";
    case "agent_update":
      return "Agent update";
    case "capability_degraded":
      return "Capability degraded";
    case "job":
      return "Job";
    case "network":
      return "Network";
    case "resource":
      return "Resource";
    case "traffic":
      return "Traffic";
    case "update":
      return "Update";
    default:
      return typeof category === "string" && category.trim()
        ? readableAlertToken(category)
        : "Category unavailable";
  }
}

export function readableAlertToken(value: string): string {
  const label = value
    .split(/[_:\-.]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
  return label || "Unavailable";
}

function knownToken<T extends string>(
  value: unknown,
  accepted: Set<T>,
): T | null {
  return typeof value === "string" && accepted.has(value as T)
    ? (value as T)
    : null;
}

function validLifecycleState(
  value: unknown,
  recordKind: AlertRecordKind | null,
): AlertLifecycleState | null {
  if (!isRecord(value)) {
    return null;
  }
  const state = knownToken(value.state, ALERT_LIFECYCLE_STATES);
  if (
    state === null ||
    !Number.isInteger(value.trigger_generation) ||
    (value.trigger_generation as number) <= 0 ||
    !validTimestamp(value.triggered_at) ||
    !nullableTimestamp(value.last_confirmed_at) ||
    !nullableTimestamp(value.resolved_at) ||
    !nullableString(value.resolution_reason) ||
    !nullableString(value.resolution_note) ||
    !nullableString(value.resolution_actor_id)
  ) {
    return null;
  }

  const hasNoResolution =
    value.resolved_at === null &&
    value.resolution_reason === null &&
    value.resolution_note === null &&
    value.resolution_actor_id === null;
  if (state === "triggered" || state === "persisting") {
    return validTimestamp(value.last_confirmed_at) &&
      atOrAfter(value.last_confirmed_at, value.triggered_at) &&
      hasNoResolution
      ? state
      : null;
  }
  if (state === "unknown") {
    return recordKind === "condition" &&
      hasNoResolution &&
      (value.last_confirmed_at === null ||
        atOrAfter(value.last_confirmed_at, value.triggered_at))
      ? state
      : null;
  }
  if (
    !validTimestamp(value.last_confirmed_at) ||
    !validTimestamp(value.resolved_at) ||
    typeof value.resolution_reason !== "string" ||
    !value.resolution_reason.trim()
  ) {
    return null;
  }
  if (
    !atOrAfter(value.last_confirmed_at, value.triggered_at) ||
    !atOrAfter(value.resolved_at, value.triggered_at) ||
    !atOrAfter(value.resolved_at, value.last_confirmed_at)
  ) {
    return null;
  }
  if (value.resolution_reason === "operator_resolved") {
    if (
      recordKind !== "event" ||
      typeof value.resolution_note !== "string" ||
      !value.resolution_note.trim() ||
      typeof value.resolution_actor_id !== "string" ||
      !UUID_PATTERN.test(value.resolution_actor_id)
    ) {
      return null;
    }
  } else if (
    recordKind !== "condition" ||
    !AUTOMATIC_RESOLUTION_REASONS.has(value.resolution_reason) ||
    value.resolution_note !== null ||
    value.resolution_actor_id !== null
  ) {
    return null;
  }
  return state;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function nullableTimestamp(value: unknown): value is string | null {
  return value === null || validTimestamp(value);
}

function validTimestamp(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    Number.isFinite(Date.parse(value))
  );
}

function atOrAfter(value: string, lowerBound: string): boolean {
  return Date.parse(value) >= Date.parse(lowerBound);
}
