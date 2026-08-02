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
  const operationHash =
    schedule.operation_payload_hash?.trim() ||
    (schedule.operation
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
      name: schedule.name,
      commandType: schedule.command_type,
      operationPayloadHash: operationHash,
      selectorExpression,
      resolvedTargets: targetClientIds,
      cronExpr: schedule.cron_expr,
      timezone: schedule.timezone,
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
