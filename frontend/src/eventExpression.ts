import {
  parseSearchExpression,
  type SearchExpression,
} from "./searchExpression";
import {
  ALERT_EVENT_CATEGORIES,
  ALERT_EVENT_IMMUTABLE_FIELDS,
  ALERT_EVENT_SEVERITIES,
} from "./generated/protocolContracts";

export const DURABLE_EVENT_EXPRESSION_SUGGESTIONS: string[] = [
  "alert.triggered",
  "alert.resolved",
  "alert.severity:critical",
  "alert.category:agent_status",
  "alert.category:backup",
  "alert.category:network",
  "alert.category:resource",
  "alert.category:traffic",
  "job.created",
  "job.status:completed",
  "job.status:failed",
  "job.status.become_failed",
  "job.type:shell",
  "job.target.status:online",
  "vps.status.online",
  "vps.status.become_offline",
  "vps.tag_changed",
  "schedule.due",
  "schedule.job_finished",
  "schedule.failed",
  "telemetry.rollup",
  "telemetry.network_rate",
  "telemetry.tunnel",
  "server.on_start",
];

export const WEBHOOK_EXPRESSION_SUGGESTIONS: string[] = [
  ...DURABLE_EVENT_EXPRESSION_SUGGESTIONS,
  "interval.30sec",
  "interval.1min",
  "interval.5min",
  "interval.1h",
];

export const SCHEDULE_ALERT_EVENT_EXPRESSION_SUGGESTIONS: string[] = [
  "alert.triggered",
  "alert.resolved",
  "alert.severity:critical",
  "alert.severity:warning",
  "alert.category:agent_status",
  "alert.category:backup",
  "alert.category:capability_degraded",
  "alert.category:job",
  "alert.category:network",
  "alert.category:resource",
  "alert.category:traffic",
  "alert.record_kind = condition",
  "alert.record_kind = event",
];

const SCHEDULE_ALERT_IMMUTABLE_FIELDS = new Set<string>(
  ALERT_EVENT_IMMUTABLE_FIELDS,
);
const SCHEDULE_ALERT_CATEGORIES = new Set<string>(ALERT_EVENT_CATEGORIES);
const SCHEDULE_ALERT_SEVERITIES = new Set<string>(ALERT_EVENT_SEVERITIES);

export type ScheduleEventExpressionExample = {
  detail: string;
  expression: string;
  label: string;
};

export const SCHEDULE_EVENT_EXPRESSION_EXAMPLES: readonly ScheduleEventExpressionExample[] =
  [
    {
      label: "Traffic alert triggered",
      expression: "alert.triggered && alert.category:traffic",
      detail: "Apply a rate limit when a traffic-policy episode triggers.",
    },
    {
      label: "Traffic alert resolved",
      expression: "alert.resolved && alert.category:traffic",
      detail: "Loosen a rate limit when that traffic condition recovers.",
    },
    {
      label: "Any critical alert",
      expression: "alert.triggered && alert.severity:critical",
      detail: "Dispatch once for every new critical alert lifecycle edge.",
    },
    {
      label: "Agent alert triggered",
      expression: "alert.triggered && alert.category:agent_status",
      detail: "React only after the agent-status policy suppresses flapping.",
    },
    {
      label: "Backup incident resolved",
      expression: "alert.resolved && alert.category:backup",
      detail: "Run recovery work after a reviewed backup incident resolves.",
    },
    {
      label: "Network alert resolved",
      expression: "alert.resolved && alert.category:network",
      detail:
        "Restore normal network controls after policy-confirmed recovery.",
    },
  ];

export function scheduleEventExpressionValidationMessage(
  input: string,
): string | null {
  const trimmed = input.trim();
  if (!trimmed) {
    return "Enter an event expression";
  }
  if (new TextEncoder().encode(trimmed).length > 4096) {
    return "Event expression is too long";
  }
  const parsed = parseSearchExpression(trimmed);
  if (parsed.error) {
    return parsed.error;
  }
  if (!parsed.expression) {
    return "Enter an event expression";
  }
  if (expressionContainsUnsupportedSchedulePredicate(parsed.expression)) {
    return "Schedules accept alert lifecycle fields only; raw VPS, job, telemetry, server, and interval predicates are not eligible";
  }
  if (!everyOrBranchHasAlertLifecycleAnchor(parsed.expression)) {
    return "Every OR branch must include non-negated alert.triggered or alert.resolved";
  }
  return null;
}

function expressionContainsUnsupportedSchedulePredicate(
  expression: SearchExpression,
): boolean {
  switch (expression.type) {
    case "predicate": {
      const predicate = expression.predicate;
      if (predicate.type === "bare" || predicate.type === "untagged") {
        return true;
      }
      if (predicate.type === "event") {
        const event = predicate.value.toLowerCase();
        if (event === "alert.triggered" || event === "alert.resolved") {
          return false;
        }
        if (event.startsWith("alert.category:")) {
          return !SCHEDULE_ALERT_CATEGORIES.has(
            event.slice("alert.category:".length),
          );
        }
        if (event.startsWith("alert.severity:")) {
          return !SCHEDULE_ALERT_SEVERITIES.has(
            event.slice("alert.severity:".length),
          );
        }
        return true;
      }
      return !scheduleAlertFieldAllowed(predicate.field);
    }
    case "not":
      return expressionContainsUnsupportedSchedulePredicate(
        expression.expression,
      );
    case "and":
    case "or":
      return (
        expressionContainsUnsupportedSchedulePredicate(expression.left) ||
        expressionContainsUnsupportedSchedulePredicate(expression.right)
      );
  }
}

function scheduleAlertFieldAllowed(field: string): boolean {
  return SCHEDULE_ALERT_IMMUTABLE_FIELDS.has(field.toLowerCase());
}

function everyOrBranchHasAlertLifecycleAnchor(
  expression: SearchExpression,
): boolean {
  switch (expression.type) {
    case "predicate":
      return (
        expression.predicate.type === "event" &&
        ["alert.triggered", "alert.resolved"].includes(
          expression.predicate.value.toLowerCase(),
        )
      );
    case "not":
      return false;
    case "and":
      return (
        everyOrBranchHasAlertLifecycleAnchor(expression.left) ||
        everyOrBranchHasAlertLifecycleAnchor(expression.right)
      );
    case "or":
      return (
        everyOrBranchHasAlertLifecycleAnchor(expression.left) &&
        everyOrBranchHasAlertLifecycleAnchor(expression.right)
      );
  }
}
