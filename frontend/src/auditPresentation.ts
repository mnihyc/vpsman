import type { AuditLogRecord, JsonValue } from "./types";
import { isJsonObject, shortId } from "./utils";

export type AuditOutcomeTone =
  "critical" | "warning" | "ok" | "info" | "neutral";

export type AuditEvidenceKind =
  | "Gateway session"
  | "Job"
  | "Operator session"
  | "Schedule"
  | "Terminal session";

export type AuditEvidenceReference = {
  detail: string;
  kind: AuditEvidenceKind;
  label: string;
  value: string;
};

export type AuditPresentation = {
  actionDetail: string;
  actionLabel: string;
  actorDetail: string;
  actorLabel: string;
  evidenceDetail: string;
  evidenceLabel: string;
  evidenceReferences: AuditEvidenceReference[];
  executionPrivilege: string | null;
  gatewaySessionId: string | null;
  operatorSessionId: string | null;
  originLabel: string;
  outcomeLabel: string;
  outcomeTone: AuditOutcomeTone;
  privilege: string | null;
  sourceIp: string | null;
  targetDetail: string;
  targetLabel: string;
  terminalSessionId: string | null;
  userAgent: string | null;
};

const ACTION_LABELS: Record<string, string> = {
  "agent_identity.upserted": "Agent identity saved",
  "fleet.alert_policy_deleted": "Fleet alert policy deleted",
  "fleet.alert_policy_upserted": "Fleet alert policy saved",
  "fleet.vps_rules_updated": "Fleet VPS rules updated",
  "job.dispatch_requested": "Job dispatch requested",
  "job.target_result": "Job target result",
  "operator_auth.login_failure": "Operator login failed",
  "operator_auth.login_success": "Operator login succeeded",
  "operator_auth.login_throttled": "Operator login throttled",
  "operator_auth.lockout_created": "Operator login lockout created",
  "operator_session.logged_out": "Operator session logged out",
  "operator_session.revoked": "Operator session revoked",
  "privilege.unlock": "Privilege unlock",
  "suite_config.update_failed": "Suite configuration update failed",
  "suite_config.update_requested": "Suite configuration update requested",
  "suite_config.updated": "Suite configuration updated",
  "terminal.close": "Terminal closed",
  "terminal.input": "Terminal input",
  "terminal.open": "Terminal opened",
};

const ACRONYMS: Record<string, string> = {
  api: "API",
  cpu: "CPU",
  id: "ID",
  ip: "IP",
  ospf: "OSPF",
  ssh: "SSH",
  tls: "TLS",
  totp: "TOTP",
  uuid: "UUID",
  vps: "VPS",
};

const TARGET_PREFIX_LABELS: Record<string, string> = {
  agent_update_release: "Agent update release",
  backup_artifact: "Backup artifact",
  backup_policy: "Backup policy",
  backup_request: "Backup request",
  command_template: "Command template",
  fleet_alert: "Fleet alert",
  fleet_alert_notification_channel: "Alert notification channel",
  fleet_alert_policy: "Fleet alert policy",
  history_retention: "History retention",
  job_approval: "Job approval",
  migration_link: "Migration link",
  monitoring_share: "Monitoring share",
  monitoring_shares: "Monitoring shares",
  ping_target: "Ping target",
  ping_targets: "Ping targets",
  port_forward_rule: "Port-forward rule",
  restore_plan: "Restore plan",
  runtime_config_patch_generator: "Runtime-config patch generator",
  schedule: "Schedule",
  tunnel_plan: "Tunnel plan",
  webhook_delivery: "Webhook delivery",
  webhook_rule: "Webhook rule",
  "webhook-rule-failure": "Webhook failure",
};

const OUTCOME_TONES: Record<string, AuditOutcomeTone> = Object.fromEntries([
  ...[
    "agent_lost",
    "agent_timeout",
    "canceled",
    "cancelled",
    "control_timeout",
    "denied",
    "error",
    "execution_canceled",
    "execution_failed",
    "failure",
    "failed",
    "permanently_failed",
    "rejected",
  ].map((value) => [value, "critical"] as const),
  ...[
    "aborted",
    "canceled_disabled",
    "degraded",
    "disconnected_timeout",
    "expired",
    "idle_timeout",
    "ignored",
    "lifecycle_disconnected",
    "locked",
    "missing",
    "partial",
    "partial_success",
    "skipped",
    "stale",
    "throttled",
    "unavailable",
    "warning",
  ].map((value) => [value, "warning"] as const),
  ...[
    "accepted",
    "applied",
    "approved",
    "closed",
    "completed",
    "deleted",
    "delivered",
    "disabled",
    "enabled",
    "exited",
    "linked_metadata_only",
    "ok",
    "online",
    "published_external",
    "recorded",
    "resized",
    "revoked",
    "succeeded",
    "success",
    "transitioned",
  ].map((value) => [value, "ok"] as const),
  ...[
    "attached",
    "dispatching",
    "opened",
    "opening",
    "pending",
    "queued",
    "requested",
    "requested_metadata_only",
    "running",
    "streaming",
  ].map((value) => [value, "info"] as const),
]);

export function presentAudit(audit: AuditLogRecord): AuditPresentation {
  const actionLabel = auditActionLabel(audit.action);
  const actor = auditActor(audit);
  const target = auditTarget(audit);
  const outcome = auditOutcome(audit);
  const sessions = auditSessions(audit);
  const evidenceReferences = auditEvidenceReferences(audit, sessions);

  return {
    actionDetail: auditActionDetail(audit),
    actionLabel,
    actorDetail: actor.detail,
    actorLabel: actor.label,
    evidenceDetail:
      evidenceReferences.length === 0
        ? "No related job, schedule, or session ID"
        : `${evidenceReferences.length} related record${evidenceReferences.length === 1 ? "" : "s"}: ${evidenceReferences
            .map((reference) => reference.kind)
            .join(", ")}`,
    evidenceLabel:
      evidenceReferences.length === 0
        ? "Standalone event"
        : evidenceReferences
            .slice(0, 2)
            .map((reference) => reference.label)
            .join(" · "),
    evidenceReferences,
    executionPrivilege: auditExecutionPrivilege(audit),
    gatewaySessionId: sessions.gateway,
    operatorSessionId: sessions.operator,
    originLabel: auditOriginLabel(audit),
    outcomeLabel: outcome.label,
    outcomeTone: outcome.tone,
    privilege: firstDirectMetadataText(audit.metadata, ["privilege_scope"]),
    sourceIp: firstDirectMetadataText(audit.metadata, [
      "remote_ip",
      "source_ip",
    ]),
    targetDetail: target.detail,
    targetLabel: target.label,
    terminalSessionId: sessions.terminal,
    userAgent: firstDirectMetadataText(audit.metadata, ["user_agent"]),
  };
}

function auditOriginLabel(audit: AuditLogRecord): string {
  const component = firstDirectMetadataText(audit.metadata, ["component"]);
  const explicitOrigin = firstDirectMetadataText(audit.metadata, [
    "origin_kind",
  ]);
  if (explicitOrigin) {
    const origin = readableCode(explicitOrigin);
    return component && component.toLowerCase() !== explicitOrigin.toLowerCase()
      ? `${origin} · ${readableCode(component)}`
      : origin;
  }
  return component
    ? `Origin not recorded · ${readableCode(component)}`
    : "Origin not recorded";
}

export function auditActionLabel(action: string): string {
  return ACTION_LABELS[action] ?? readableCode(action);
}

export function auditMissingFieldLabel(
  field: "gateway" | "operator" | "privilege" | "request" | "terminal",
): string {
  const labels = {
    gateway: "Gateway session not recorded",
    operator: "Operator session not recorded",
    privilege: "Privilege scope not recorded",
    request: "Not recorded for this event",
    terminal: "Terminal session not recorded",
  } as const;
  return labels[field];
}

export function auditEvidenceSearchText(audit: AuditLogRecord): string {
  const presentation = presentAudit(audit);
  return presentation.evidenceReferences
    .flatMap((reference) => [
      reference.kind,
      reference.label,
      reference.detail,
      reference.value,
    ])
    .join(" ");
}

export function auditClientIds(audit: AuditLogRecord): string[] {
  const clientIds = new Set<string>();
  if (audit.target.startsWith("client:")) {
    const targetClientId = audit.target.slice("client:".length).trim();
    if (targetClientId) clientIds.add(targetClientId);
  }
  if (!isJsonObject(audit.metadata)) return [...clientIds];

  for (const key of [
    "client_id",
    "left_client_id",
    "peer_client_id",
    "right_client_id",
    "source_client_id",
    "target_client_id",
  ]) {
    const value = directMetadataValue(audit.metadata, key);
    if (typeof value === "string" && value.trim()) {
      clientIds.add(value.trim());
    }
  }
  for (const key of ["client_ids", "target_client_ids"]) {
    const value = directMetadataValue(audit.metadata, key);
    if (!Array.isArray(value)) continue;
    for (const entry of value) {
      if (typeof entry === "string" && entry.trim()) {
        clientIds.add(entry.trim());
      }
    }
  }
  return [...clientIds];
}

export function auditSessionSearchText(audit: AuditLogRecord): string {
  const presentation = presentAudit(audit);
  return [
    ...new Set(
      [
        presentation.operatorSessionId,
        presentation.terminalSessionId,
        presentation.gatewaySessionId,
        ...presentation.evidenceReferences
          .filter((reference) => reference.kind.includes("session"))
          .map((reference) => reference.value),
      ].filter((value): value is string => Boolean(value)),
    ),
  ].join(" ");
}

function auditActionDetail(audit: AuditLogRecord): string {
  const commandType = firstDirectMetadataText(audit.metadata, ["command_type"]);
  const targetCount = directMetadataCount(audit.metadata, "target_count");
  const changedKeys = directMetadataValue(audit.metadata, "changed_keys");
  const reason = firstDirectMetadataText(audit.metadata, [
    "reason",
    "write_error",
    "error",
  ]);
  const details = [
    commandType ? readableCode(commandType) : null,
    targetCount !== null
      ? `${targetCount} target${targetCount === 1 ? "" : "s"}`
      : null,
    Array.isArray(changedKeys)
      ? `${changedKeys.length} setting${changedKeys.length === 1 ? "" : "s"}`
      : null,
    reason ? readableCode(reason) : null,
  ].filter((value): value is string => Boolean(value));
  if (details.length > 0) {
    return details.join(" · ");
  }
  if (audit.action.startsWith("operator_auth.")) return "Authentication";
  if (audit.action.startsWith("operator_session.")) return "Operator access";
  if (audit.action.startsWith("terminal.")) return "Terminal session";
  if (audit.action.startsWith("job.")) return "Job execution";
  if (audit.action.startsWith("suite_config.")) return "Server configuration";
  if (audit.action.startsWith("fleet.")) return "Fleet configuration";
  if (audit.action.startsWith("agent_identity.")) return "VPS identity";
  if (audit.action.includes("privilege")) return "Privileged access";
  return "Audit event";
}

function auditActor(audit: AuditLogRecord): { detail: string; label: string } {
  const username = firstDirectMetadataText(audit.metadata, [
    "operator_username",
  ]);
  const attemptedUsername = firstDirectMetadataText(audit.metadata, [
    "attempted_username",
  ]);
  const role = firstDirectMetadataText(audit.metadata, ["operator_role"]);
  const origin = firstDirectMetadataText(audit.metadata, ["origin_kind"]);
  const visitorId = firstDirectMetadataText(audit.metadata, ["visitor_id"]);
  if (!audit.actor_id && origin === "public_share" && visitorId) {
    return {
      detail: `Visitor ${shortId(visitorId)}`,
      label: "Public visitor",
    };
  }
  if (!audit.actor_id && audit.action.startsWith("operator_auth.")) {
    return {
      detail: attemptedUsername
        ? `Attempted username ${attemptedUsername}`
        : "No authenticated operator",
      label: "Unauthenticated request",
    };
  }
  if (!audit.actor_id) {
    const component = firstDirectMetadataText(audit.metadata, ["component"]);
    return {
      detail: component ? readableCode(component) : "Automated system event",
      label: "Control plane",
    };
  }
  return {
    detail: role ? readableCode(role) : "Authenticated operator",
    label: username ?? `Operator ${shortId(audit.actor_id)}`,
  };
}

function auditTarget(audit: AuditLogRecord): { detail: string; label: string } {
  const target = audit.target;
  const clientId = firstDirectMetadataText(audit.metadata, ["client_id"]);
  const targetCount = directMetadataCount(audit.metadata, "target_count");
  if (target === "access/privilege-vault") {
    return { detail: "Privileged access control", label: "Privilege vault" };
  }
  if (target === "auth:login") {
    return { detail: "Operator authentication", label: "Authentication" };
  }
  if (target === "suite_config") {
    return { detail: "Control-plane settings", label: "Suite configuration" };
  }
  if (target === "vps_rules") {
    return { detail: "Fleet-scoped configuration", label: "Fleet VPS rules" };
  }
  if (target.startsWith("api:/api/v1/jobs")) {
    return {
      detail:
        targetCount !== null
          ? `${targetCount} resolved target${targetCount === 1 ? "" : "s"}`
          : "Job submission endpoint",
      label: "Jobs API",
    };
  }
  if (target.startsWith("client:")) {
    const id = target.slice("client:".length);
    return { detail: `Client ID ${id}`, label: `VPS ${id}` };
  }
  if (target.startsWith("job:")) {
    const id = target.slice("job:".length);
    return { detail: id, label: `Job ${shortId(id)}` };
  }
  if (target.startsWith("operator-session:")) {
    const id = target.slice("operator-session:".length);
    return { detail: id, label: `Operator session ${shortId(id)}` };
  }
  if (target.startsWith("terminal:")) {
    const terminal = firstDirectMetadataText(audit.metadata, [
      "terminal_session_id",
    ]);
    return {
      detail: terminal ?? target.slice("terminal:".length),
      label: clientId ? `Terminal on VPS ${clientId}` : "Terminal session",
    };
  }
  if (target.startsWith("operator:")) {
    const id = target.slice("operator:".length);
    return {
      detail: `Operator ID ${shortId(id)}`,
      label:
        firstDirectMetadataText(audit.metadata, ["target_operator_username"]) ??
        `Operator ${shortId(id)}`,
    };
  }
  const separator = target.indexOf(":");
  if (separator > 0) {
    const prefix = target.slice(0, separator);
    const id = target.slice(separator + 1);
    const resource = TARGET_PREFIX_LABELS[prefix];
    if (resource && id) {
      return {
        detail: `${resource} ID ${id}`,
        label: `${resource} ${resourceIdentityLabel(id)}`,
      };
    }
  }
  return {
    detail: clientId ? `VPS ${clientId}` : target,
    label: readableCode(target),
  };
}

function resourceIdentityLabel(value: string): string {
  if (/^[0-9a-f]{8}-[0-9a-f-]{27,}$/i.test(value) || value.length > 24) {
    return shortId(value);
  }
  return readableCode(value);
}

function auditOutcome(audit: AuditLogRecord): {
  label: string;
  tone: AuditOutcomeTone;
} {
  const raw = firstDirectMetadataText(
    audit.metadata,
    audit.action === "job.target_result" ? ["result", "status"] : ["result"],
  );
  if (!raw) {
    return { label: "Outcome not recorded", tone: "neutral" };
  }
  const lower = raw.toLowerCase();
  return {
    label: readableCode(raw),
    tone: OUTCOME_TONES[lower] ?? "neutral",
  };
}

function auditExecutionPrivilege(audit: AuditLogRecord): string | null {
  const privileged = directMetadataBoolean(audit.metadata, "privileged");
  const forceUnprivileged = directMetadataBoolean(
    audit.metadata,
    "force_unprivileged",
  );
  if (privileged === null && forceUnprivileged === null) {
    return null;
  }
  return [
    privileged === null
      ? null
      : privileged
        ? "Privileged requested"
        : "Unprivileged requested",
    forceUnprivileged === null
      ? null
      : forceUnprivileged
        ? "Force-unprivileged enabled"
        : "Force-unprivileged disabled",
  ]
    .filter((value): value is string => Boolean(value))
    .join(" · ");
}

function auditSessions(audit: AuditLogRecord): {
  gateway: string | null;
  operator: string | null;
  terminal: string | null;
} {
  const directOperator = firstDirectMetadataText(audit.metadata, [
    "operator_session_id",
  ]);
  const gateway = firstDirectMetadataText(audit.metadata, [
    "gateway_session_id",
  ]);
  const terminal = firstDirectMetadataText(audit.metadata, [
    "terminal_session_id",
  ]);
  return {
    gateway,
    operator: directOperator,
    terminal,
  };
}

function auditEvidenceReferences(
  audit: AuditLogRecord,
  sessions: ReturnType<typeof auditSessions>,
): AuditEvidenceReference[] {
  const references: AuditEvidenceReference[] = [];
  const push = (kind: AuditEvidenceKind, value: string | null) => {
    if (
      !value ||
      references.some(
        (reference) => reference.kind === kind && reference.value === value,
      )
    ) {
      return;
    }
    references.push({
      detail: `${kind} ${value}`,
      kind,
      label: `${kind} ${shortId(value)}`,
      value,
    });
  };
  for (const key of [
    "job_id",
    "activation_job_id",
    "last_job_id",
    "left_job_id",
    "right_job_id",
    "rollback_job_id",
    "source_job_id",
  ]) {
    push("Job", firstDirectMetadataText(audit.metadata, [key]));
  }
  for (const key of ["agent_lost_job_ids", "skipped_unstarted_job_ids"]) {
    for (const jobId of directMetadataTextArray(audit.metadata, key)) {
      push("Job", jobId);
    }
  }
  push("Terminal session", sessions.terminal);
  push(
    "Operator session",
    firstDirectMetadataText(audit.metadata, ["revoked_operator_session_id"]),
  );
  push("Operator session", sessions.operator);
  push("Gateway session", sessions.gateway);
  push("Schedule", firstDirectMetadataText(audit.metadata, ["schedule_id"]));
  push(
    "Schedule",
    firstDirectMetadataText(audit.metadata, ["source_schedule_id"]),
  );
  return references;
}

function firstDirectMetadataText(
  metadata: JsonValue,
  keys: string[],
): string | null {
  for (const key of keys) {
    const text = directMetadataText(metadata, key);
    if (text) return text;
  }
  return null;
}

function directMetadataText(metadata: JsonValue, key: string): string | null {
  const value = directMetadataValue(metadata, key);
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed || null;
}

function directMetadataCount(metadata: JsonValue, key: string): number | null {
  const value = directMetadataValue(metadata, key);
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : null;
}

function directMetadataBoolean(
  metadata: JsonValue,
  key: string,
): boolean | null {
  const value = directMetadataValue(metadata, key);
  return typeof value === "boolean" ? value : null;
}

function directMetadataTextArray(metadata: JsonValue, key: string): string[] {
  const value = directMetadataValue(metadata, key);
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    if (typeof entry !== "string") return [];
    const trimmed = entry.trim();
    return trimmed ? [trimmed] : [];
  });
}

function directMetadataValue(
  metadata: JsonValue,
  key: string,
): JsonValue | undefined {
  if (!isJsonObject(metadata)) return undefined;
  if (Object.prototype.hasOwnProperty.call(metadata, key)) {
    return metadata[key];
  }
  if (
    metadata.component === "monitoring-controller" &&
    isJsonObject(metadata.details) &&
    Object.prototype.hasOwnProperty.call(metadata.details, key)
  ) {
    return metadata.details[key];
  }
  return undefined;
}

function readableCode(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .split(/[._:/-]+|\s+/)
    .filter(Boolean)
    .map((part, index) => {
      const lower = part.toLowerCase();
      if (ACRONYMS[lower]) return ACRONYMS[lower];
      return index === 0
        ? `${part.charAt(0).toUpperCase()}${part.slice(1).toLowerCase()}`
        : part.toLowerCase();
    })
    .join(" ");
}
