import {
  alertEventArgvTemplateHashHex,
  buildPrivilegeAssertion,
  canonicalSchedulePrivilegeIntent,
  operationPayloadHashHex,
  type PrivilegeMaterial,
} from "./privilege";
import type { ScheduleRecord } from "./types";

export async function buildScheduleTargetUpdatePrivilegeAssertion({
  privilegeMaterial,
  schedule,
  selectorExpression,
  targetClientIds,
}: {
  privilegeMaterial: PrivilegeMaterial;
  schedule: ScheduleRecord;
  selectorExpression: string;
  targetClientIds: string[];
}) {
  const operationHash =
    schedule.operation_payload_hash?.trim() ||
    (schedule.trigger_kind === "event"
      ? await alertEventArgvTemplateHashHex(schedule.event_argv_template)
      : schedule.operation
        ? await operationPayloadHashHex(schedule.operation)
        : "");
  if (!operationHash) {
    throw new Error(
      `${schedule.name}: saved operation evidence is unavailable`,
    );
  }
  return buildPrivilegeAssertion({
    intent: canonicalSchedulePrivilegeIntent({
      action: "schedule.targets.update",
      scheduleId: schedule.id,
      definitionRevision: schedule.definition_revision,
      name: schedule.name,
      commandType: schedule.command_type,
      operationPayloadHash: operationHash,
      selectorExpression,
      resolvedTargets: targetClientIds,
      triggerKind: schedule.trigger_kind,
      runOn: schedule.run_on,
      cronExpr: schedule.cron_expr,
      timezone: schedule.timezone,
      eventExpression: schedule.event_expression,
      enabled: schedule.enabled,
      catchUpPolicy: schedule.catch_up_policy,
      catchUpLimit: schedule.catch_up_limit,
      retryDelaySecs: schedule.retry_delay_secs,
      maxFailures: schedule.max_failures,
      maxTimeoutSecs: schedule.max_timeout_secs,
      deferredUntil: schedule.deferred_until,
      deleted: false,
    }),
    privilegeMaterial,
  });
}
