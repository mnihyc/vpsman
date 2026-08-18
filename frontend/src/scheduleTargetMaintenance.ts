import {
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
  const privilegeOperation =
    schedule.trigger_kind === "event"
      ? {
          type: "shell" as const,
          argv: schedule.event_argv_template ?? ["/bin/true"],
          pty: false,
        }
      : schedule.operation;
  const operationHash =
    schedule.operation_payload_hash?.trim() ||
    (privilegeOperation
      ? await operationPayloadHashHex(privilegeOperation)
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
      cronExpr: schedule.cron_expr,
      timezone: schedule.timezone,
      eventExpression: schedule.event_expression,
      enabled: schedule.enabled,
      catchUpPolicy: schedule.catch_up_policy,
      catchUpLimit: schedule.catch_up_limit,
      retryDelaySecs: schedule.retry_delay_secs,
      maxFailures: schedule.max_failures,
      deferredUntil: schedule.deferred_until,
      deleted: false,
    }),
    privilegeMaterial,
  });
}
