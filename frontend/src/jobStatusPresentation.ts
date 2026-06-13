import {
  AGENT_UPDATE_RELEASE_STATUS_CLASS_BY_STATUS,
  BACKUP_REQUEST_STATUS_CLASS_BY_STATUS,
  DATA_SOURCE_READINESS_STATUS_CLASS_BY_STATUS,
  FILE_TRANSFER_SESSION_STATUS_CLASS_BY_STATUS,
  FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS,
  FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CLASS_BY_STATUS,
  JOB_STATUS_CLASS_BY_STATUS,
  JOB_TARGET_STATUSES,
  JOB_TARGET_STATUS_CLASS_BY_STATUS,
  MIGRATION_LINK_STATUS_CLASS_BY_STATUS,
  RESTORE_PLAN_STATUS_CLASS_BY_STATUS,
  SERVER_JOB_STATUS_CLASS_BY_STATUS,
  TERMINAL_SESSION_STATE_CLASS_BY_STATE,
  TERMINAL_SESSION_STATUS_CLASS_BY_STATUS,
  TOPOLOGY_EDGE_HEALTH_STATUS_CLASS_BY_STATUS,
  TOPOLOGY_NEIGHBOR_STATE_CLASS_BY_STATE,
  TOPOLOGY_NODE_STATUS_CLASS_BY_STATUS,
  TOPOLOGY_OBSERVATION_STATE_CLASS_BY_STATE,
  TOPOLOGY_PROBE_STATE_CLASS_BY_STATE,
  TOPOLOGY_RUNTIME_STATE_CLASS_BY_STATE,
  TUNNEL_ENDPOINT_STATUS_CLASS_BY_STATUS,
  TUNNEL_PLAN_STATUS_CLASS_BY_STATUS,
  WEBHOOK_RULE_DELIVERY_HISTORY_STATUS_CLASS_BY_STATUS,
  WEBHOOK_RULE_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS,
  WEBHOOK_RULE_DELIVERY_STATUS_CLASS_BY_STATUS,
} from "./generated/protocolContracts";
import type {
  GeneratedAgentUpdateReleaseStatus,
  GeneratedBackupRequestStatus,
  GeneratedDataSourceReadinessStatus,
  GeneratedFileTransferSessionStatus,
  GeneratedFleetAlertNotificationDeliveryProcessStatus,
  GeneratedFleetAlertNotificationDeliveryStatus,
  GeneratedJobStatus,
  GeneratedJobStatusClass,
  GeneratedJobTargetStatus,
  GeneratedJobTargetStatusClass,
  GeneratedMigrationLinkStatus,
  GeneratedRestorePlanStatus,
  GeneratedServerJobStatus,
  GeneratedTerminalSessionState,
  GeneratedTerminalSessionStatus,
  GeneratedTopologyEdgeHealthStatus,
  GeneratedTopologyNeighborState,
  GeneratedTopologyNodeStatus,
  GeneratedTopologyObservationState,
  GeneratedTopologyProbeState,
  GeneratedTopologyRuntimeState,
  GeneratedTunnelEndpointStatus,
  GeneratedTunnelPlanStatus,
  GeneratedWebhookRuleDeliveryHistoryStatus,
  GeneratedWebhookRuleDeliveryProcessStatus,
  GeneratedWebhookRuleDeliveryStatus,
  GeneratedWorkflowStatusClass,
} from "./generated/protocolContracts";
import type { JobOutputComparisonStatus } from "./types";

const JOB_TARGET_STATUS_SET = new Set<GeneratedJobTargetStatus>(JOB_TARGET_STATUSES);

export function isJobTargetStatus(status: string): status is GeneratedJobTargetStatus {
  return JOB_TARGET_STATUS_SET.has(status as GeneratedJobTargetStatus);
}

export function jobStatusBadgeClass(status: GeneratedJobStatus): string {
  return jobStatusClassBadge(JOB_STATUS_CLASS_BY_STATUS[status]);
}

export function jobTargetStatusBadgeClass(status: GeneratedJobTargetStatus): string {
  return jobTargetStatusClassBadge(JOB_TARGET_STATUS_CLASS_BY_STATUS[status]);
}

export function jobOutputComparisonStatusBadgeClass(status: JobOutputComparisonStatus): string {
  return status === "unknown" ? "warn" : jobTargetStatusBadgeClass(status);
}

export function terminalSessionStateBadgeClass(status: GeneratedTerminalSessionState): string {
  return workflowStatusClassBadge(TERMINAL_SESSION_STATE_CLASS_BY_STATE[status]);
}

export function terminalSessionStatusBadgeClass(status: GeneratedTerminalSessionStatus): string {
  return workflowStatusClassBadge(TERMINAL_SESSION_STATUS_CLASS_BY_STATUS[status]);
}

export function fileTransferSessionStatusBadgeClass(status: GeneratedFileTransferSessionStatus): string {
  return workflowStatusClassBadge(FILE_TRANSFER_SESSION_STATUS_CLASS_BY_STATUS[status]);
}

export function backupRequestStatusBadgeClass(status: GeneratedBackupRequestStatus): string {
  return workflowStatusClassBadge(BACKUP_REQUEST_STATUS_CLASS_BY_STATUS[status]);
}

export function restorePlanStatusBadgeClass(status: GeneratedRestorePlanStatus): string {
  return workflowStatusClassBadge(RESTORE_PLAN_STATUS_CLASS_BY_STATUS[status]);
}

export function migrationLinkStatusBadgeClass(status: GeneratedMigrationLinkStatus): string {
  return workflowStatusClassBadge(MIGRATION_LINK_STATUS_CLASS_BY_STATUS[status]);
}

export function tunnelPlanStatusBadgeClass(status: GeneratedTunnelPlanStatus): string {
  return workflowStatusClassBadge(TUNNEL_PLAN_STATUS_CLASS_BY_STATUS[status]);
}

export function tunnelEndpointStatusBadgeClass(status: GeneratedTunnelEndpointStatus): string {
  return workflowStatusClassBadge(TUNNEL_ENDPOINT_STATUS_CLASS_BY_STATUS[status]);
}

export function agentUpdateReleaseStatusBadgeClass(status: GeneratedAgentUpdateReleaseStatus): string {
  return workflowStatusClassBadge(AGENT_UPDATE_RELEASE_STATUS_CLASS_BY_STATUS[status]);
}

export function serverJobStatusBadgeClass(status: GeneratedServerJobStatus): string {
  return workflowStatusClassBadge(SERVER_JOB_STATUS_CLASS_BY_STATUS[status]);
}

export function fleetAlertNotificationDeliveryStatusBadgeClass(
  status: GeneratedFleetAlertNotificationDeliveryStatus,
): string {
  return workflowStatusClassBadge(FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CLASS_BY_STATUS[status]);
}

export function fleetAlertNotificationDeliveryProcessStatusBadgeClass(
  status: GeneratedFleetAlertNotificationDeliveryProcessStatus,
): string {
  return workflowStatusClassBadge(FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS[status]);
}

export function webhookRuleDeliveryStatusBadgeClass(status: GeneratedWebhookRuleDeliveryStatus): string {
  return workflowStatusClassBadge(WEBHOOK_RULE_DELIVERY_STATUS_CLASS_BY_STATUS[status]);
}

export function webhookRuleDeliveryHistoryStatusBadgeClass(status: GeneratedWebhookRuleDeliveryHistoryStatus): string {
  return workflowStatusClassBadge(WEBHOOK_RULE_DELIVERY_HISTORY_STATUS_CLASS_BY_STATUS[status]);
}

export function webhookRuleDeliveryProcessStatusBadgeClass(status: GeneratedWebhookRuleDeliveryProcessStatus): string {
  return workflowStatusClassBadge(WEBHOOK_RULE_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS[status]);
}

export function dataSourceReadinessStatusBadgeClass(status: GeneratedDataSourceReadinessStatus): string {
  return workflowStatusClassBadge(DATA_SOURCE_READINESS_STATUS_CLASS_BY_STATUS[status]);
}

export function topologyNodeStatusBadgeClass(status: GeneratedTopologyNodeStatus): string {
  return workflowStatusClassBadge(TOPOLOGY_NODE_STATUS_CLASS_BY_STATUS[status]);
}

export function topologyEdgeHealthStatusBadgeClass(status: GeneratedTopologyEdgeHealthStatus): string {
  return workflowStatusClassBadge(TOPOLOGY_EDGE_HEALTH_STATUS_CLASS_BY_STATUS[status]);
}

export function topologyNeighborStateBadgeClass(status: GeneratedTopologyNeighborState): string {
  return workflowStatusClassBadge(TOPOLOGY_NEIGHBOR_STATE_CLASS_BY_STATE[status]);
}

export function topologyProbeStateBadgeClass(status: GeneratedTopologyProbeState): string {
  return workflowStatusClassBadge(TOPOLOGY_PROBE_STATE_CLASS_BY_STATE[status]);
}

export function topologyRuntimeStateBadgeClass(status: GeneratedTopologyRuntimeState): string {
  return workflowStatusClassBadge(TOPOLOGY_RUNTIME_STATE_CLASS_BY_STATE[status]);
}

export function topologyObservationStateBadgeClass(status: GeneratedTopologyObservationState): string {
  return workflowStatusClassBadge(TOPOLOGY_OBSERVATION_STATE_CLASS_BY_STATE[status]);
}

function jobStatusClassBadge(statusClass: GeneratedJobStatusClass): string {
  switch (statusClass) {
    case "in_progress":
      return "info";
    case "successful":
      return "ok";
    case "partial_success":
    case "unsuccessful":
      return "warn";
    case "skipped":
      return "neutral";
  }
}

function jobTargetStatusClassBadge(statusClass: GeneratedJobTargetStatusClass): string {
  switch (statusClass) {
    case "in_progress":
      return "info";
    case "successful":
      return "ok";
    case "skipped":
      return "neutral";
    case "unsuccessful":
      return "warn";
  }
}

function workflowStatusClassBadge(statusClass: GeneratedWorkflowStatusClass): string {
  switch (statusClass) {
    case "in_progress":
      return "info";
    case "successful":
      return "ok";
    case "warning":
      return "warn";
    case "neutral":
      return "neutral";
  }
}
