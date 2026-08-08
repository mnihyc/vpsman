import type {
  GeneratedCreateJobRequestField,
  GeneratedAgentUpdateReleaseStatus,
  GeneratedBackupRequestStatus,
  GeneratedFleetAlertNotificationDeliveryProcessStatus,
  GeneratedFleetAlertNotificationDeliveryStatus,
  GeneratedMigrationLinkStatus,
  GeneratedJobCommandType,
  GeneratedJobOperationType,
  GeneratedJobStatus,
  GeneratedJobTargetStatus,
  GeneratedRestorePlanStatus,
  GeneratedServerJobStatus,
  GeneratedServerJobType,
  GeneratedTopologyEdgeHealthStatus,
  GeneratedTopologyNeighborState,
  GeneratedTopologyNodeStatus,
  GeneratedTopologyObservationState,
  GeneratedTopologyProbeState,
  GeneratedTopologyRuntimeState,
  GeneratedWebhookRuleDeliveryHistoryStatus,
  GeneratedWebhookRuleDeliveryProcessStatus,
  GeneratedWebhookRuleDeliveryStatus,
  PolicyAlertRecord as GeneratedPolicyAlertRecord,
  PolicyDryRunRequest as GeneratedPolicyDryRunRequest,
  PolicyDryRunResponse as GeneratedPolicyDryRunResponse,
  PolicyDryRunRulePreview as GeneratedPolicyDryRunRulePreview,
  PolicyGroupRecord as GeneratedPolicyGroupRecord,
  PolicyGroupRequest as GeneratedPolicyGroupRequest,
  PolicyRuleRecord as GeneratedPolicyRuleRecord,
  PolicyRuleRequest as GeneratedPolicyRuleRequest,
  PolicyRuleStateRecord as GeneratedPolicyRuleStateRecord,
  TrafficAccountingRecord as GeneratedTrafficAccountingRecord,
  TrafficAccountingSelectorBreakdown as GeneratedTrafficAccountingSelectorBreakdown,
  VpsRuleChangePreview as GeneratedVpsRuleChangePreview,
  VpsRuleValueRecord as GeneratedVpsRuleValueRecord,
  VpsRulesBulkUnsetRequest as GeneratedVpsRulesBulkUnsetRequest,
  VpsRulesBulkUpsertRequest as GeneratedVpsRulesBulkUpsertRequest,
  VpsRulesDryRunRequest as GeneratedVpsRulesDryRunRequest,
  VpsRulesDryRunResponse as GeneratedVpsRulesDryRunResponse,
} from "./generated/protocolContracts";

export type JobStatus = GeneratedJobStatus;
export type JobTargetStatus = GeneratedJobTargetStatus;
export type JobCommandType = GeneratedJobCommandType;
export type AgentUpdateReleaseStatus = GeneratedAgentUpdateReleaseStatus;
export type BackupRequestStatus = GeneratedBackupRequestStatus;
export type FleetAlertNotificationDeliveryProcessStatus =
  GeneratedFleetAlertNotificationDeliveryProcessStatus;
export type FleetAlertNotificationDeliveryStatus =
  GeneratedFleetAlertNotificationDeliveryStatus;
export type MigrationLinkStatus = GeneratedMigrationLinkStatus;
export type RestorePlanStatus = GeneratedRestorePlanStatus;
export type ServerJobStatus = GeneratedServerJobStatus;
export type ServerJobType = GeneratedServerJobType;
export type TopologyEdgeHealthStatus = GeneratedTopologyEdgeHealthStatus;
export type TopologyNeighborState = GeneratedTopologyNeighborState;
export type TopologyNodeStatus = GeneratedTopologyNodeStatus;
export type TopologyObservationState = GeneratedTopologyObservationState;
export type TopologyProbeState = GeneratedTopologyProbeState;
export type TopologyRuntimeState = GeneratedTopologyRuntimeState;
export type TunnelEndpointRuntimeState =
  "disabled" | "unknown" | "stale" | "healthy" | "degraded";
export type TunnelEndpointReachabilityState =
  "unknown" | "reachable" | "probe_failed" | "stale" | "not_configured";
export type TunnelConnectionAssessment =
  "automatic" | "connected" | "disconnected";
export type WebhookRuleDeliveryHistoryStatus =
  GeneratedWebhookRuleDeliveryHistoryStatus;
export type WebhookRuleDeliveryProcessStatus =
  GeneratedWebhookRuleDeliveryProcessStatus;
export type WebhookRuleDeliveryStatus = GeneratedWebhookRuleDeliveryStatus;

export type FleetSummary = {
  total: number;
  online: number;
  offline: number;
  never: number;
  revoked: number;
  unknown: number;
  stale: number;
  warnings: number;
  running_jobs: number;
};

export type DashboardWindow =
  "15m" | "1h" | "8h" | "1d" | "7d" | "30d" | "90d" | "180d" | "1y" | "all";

export type DashboardGroupBy =
  "labels" | "tags" | "countries" | "providers" | "clients" | "status" | "date";

export type DashboardScopeKind =
  "all" | "tag" | "country" | "provider" | "client";

export type DashboardResourceMetric = "cpu_load" | "memory_used" | "disk_free";

export type DashboardNetworkViewMode = "speed" | "traffic";

export type DashboardTrafficSort = "total" | "rx" | "tx";

export type DashboardPointDensity = "compact" | "balanced" | "dense";

export type DashboardRefreshIntervalSecs = 5 | 30 | 60;

export type DashboardPreferences = {
  groupBy: DashboardGroupBy;
  networkView: DashboardNetworkViewMode;
  pointDensity: DashboardPointDensity;
  refreshIntervalSecs: DashboardRefreshIntervalSecs;
  resourceMetric: DashboardResourceMetric;
  scopeKind: DashboardScopeKind;
  scopeValue: string;
  startAt: string;
  endAt: string;
  trafficSort: DashboardTrafficSort;
  window: DashboardWindow;
};

export type DashboardDrilldownRecord = {
  label: string;
  view: ActiveView;
  subpage: string;
  query: string | null;
};

export type DashboardOverviewRecord = {
  window: DashboardWindow;
  generated_at: string;
  group_by: DashboardGroupBy;
  scope: DashboardScopeRecord;
  time_range: DashboardTimeRangeRecord;
  available_filters: DashboardAvailableFiltersRecord;
  summary: DashboardSummaryRecord;
  operations: DashboardOperationsRecord;
  resources: DashboardResourcesRecord;
  resource_curve: DashboardResourceCurveRecord;
  network: DashboardNetworkRecord;
  label_clusters: DashboardLabelClusterRecord[];
  drilldowns: DashboardDrilldownRecord[];
};

export type SystemDashboardDbPoolRecord = {
  max_connections: number;
  open_connections: number;
  idle_connections: number;
  in_use_connections: number;
};

export type SystemDashboardDispatchRecord = {
  active_jobs: number;
  queued_jobs: number;
  running_jobs: number;
  queue_depth: number;
  total_dispatch_attempts: number;
  retried_targets: number;
};

export type SystemDashboardTargetsRecord = {
  queued: number;
  dispatching: number;
  running: number;
  active: number;
  deadline_expired_active: number;
  control_timeout_last_24h: number;
  agent_timeout_last_24h: number;
  agent_lost_last_24h: number;
  canceled_last_24h: number;
};

export type SystemDashboardCancellationsRecord = {
  requested: number;
  sent: number;
  acked: number;
  awaiting_ack: number;
};

export type GatewayForwardEventKindCounters = {
  telemetry: number;
  command_output: number;
  lifecycle: number;
  terminal_output: number;
  other: number;
};

export type GatewayForwardDropReasonCounters = {
  global_queue_full: number;
  target_queue_full: number;
  expired: number;
  coalesced: number;
  protocol_conflict: number;
};

export type GatewayForwardCriticalFailureCounters = {
  global_queue_full: number;
  target_queue_full: number;
  expired: number;
};

export type SystemDashboardGatewayEventsRecord = {
  queued_events: number | null;
  delivered_events: number | null;
  retry_attempts: number | null;
  active_queues: number | null;
  current_queue_depth: number | null;
  oldest_event_age_secs: number | null;
  dropped_events: number | null;
  telemetry_dropped_events: number | null;
  expired_events: number | null;
  critical_failures: number | null;
  dropped_by_kind: GatewayForwardEventKindCounters;
  dropped_by_reason: GatewayForwardDropReasonCounters;
  critical_failures_by_reason: GatewayForwardCriticalFailureCounters;
  retained_output_truncated_events: number | null;
  rejected_agent_connections: number | null;
  status: "live" | "unavailable" | string;
};

export type SystemDashboardRecord = {
  generated_at: string;
  window: string;
  bucket_secs: number;
  current: SystemDashboardSnapshotRecord;
  capacity: SystemDashboardCapacityRecord;
  series: SystemMetricSeriesRecord[];
  notes: string[];
};

export type SystemDashboardSnapshotRecord = {
  db_pool: SystemDashboardDbPoolRecord;
  dispatch: SystemDashboardDispatchRecord;
  targets: SystemDashboardTargetsRecord;
  cancellations: SystemDashboardCancellationsRecord;
  gateway_events: SystemDashboardGatewayEventsRecord;
};

export type SystemDashboardCapacityRecord = {
  api_db_pool: number | null;
  worker_db_pool: number | null;
  dispatcher_batch: number | null;
  dispatcher_in_flight: number | null;
  dispatch_ack_secs: number | null;
  event_post_secs: number | null;
  internal_http_read_secs: number | null;
  control_deadline_grace_secs: number | null;
  max_job_timeout_secs: number | null;
  worker_schedule_job_max_timeout_secs: number | null;
  agent_offline_secs: number | null;
};

export type SystemMetricSeriesRecord = {
  metric: string;
  label: string;
  unit: string;
  points: SystemMetricPointRecord[];
};

export type SystemMetricPointRecord = {
  bucket_start: string;
  avg_value: number;
  max_value: number;
  latest_value: number;
  sample_count: number;
};

export type DashboardScopeRecord = {
  kind: DashboardScopeKind;
  value: string | null;
  label: string;
  query: string | null;
  matched_clients: number;
};

export type DashboardTimeRangeRecord = {
  mode: "window" | "custom" | string;
  window: DashboardWindow | null;
  start_unix: number;
  end_unix: number;
  start_at: string;
  end_at: string;
};

export type DashboardAvailableFiltersRecord = {
  windows: DashboardWindowOptionRecord[];
  group_by_options: DashboardGroupByOptionRecord[];
  providers: DashboardFilterOptionRecord[];
  countries: DashboardFilterOptionRecord[];
  tags: DashboardFilterOptionRecord[];
};

export type DashboardWindowOptionRecord = {
  value: DashboardWindow;
  label: string;
  seconds: number;
};

export type DashboardGroupByOptionRecord = {
  value: DashboardGroupBy;
  label: string;
  description: string;
};

export type DashboardFilterOptionRecord = {
  kind: DashboardScopeKind;
  value: string;
  label: string;
  query: string;
  count: number;
};

export type DashboardSummaryRecord = {
  total: number;
  online: number;
  offline: number;
  revoked: number;
  stale: number;
  warnings: number;
  warnings_truncated: boolean;
  running_jobs: number;
  running_jobs_truncated: boolean;
};

export type DashboardOperationsRecord = {
  active_alerts: number;
  critical_alerts: number;
  warning_alerts: number;
  stale_agents: number;
  running_jobs: number;
  backup_pending: number;
  backup_completed: number;
  backup_failed: number;
  alerts_truncated: boolean;
  running_jobs_truncated: boolean;
  backups_truncated: boolean;
  recent_alerts: DashboardAlertSummaryRecord[];
  degraded_agents: DashboardAgentSummaryRecord[];
};

export type DashboardResourcesRecord = {
  sampled_clients: number;
  cpu_load_avg: number | null;
  cpu_load_max: number | null;
  memory_used_ratio: number | null;
  disk_free_ratio: number | null;
};

export type DashboardResourceCurveRecord = {
  metric: DashboardResourceMetric;
  sampled_clients: number;
  excluded_clients: number;
  top_limit: number;
  latest_sample_at: string | null;
  series: DashboardResourceSeriesRecord[];
};

export type DashboardResourceSeriesRecord = {
  client_id: string;
  label: string;
  current: number | null;
  peak: number | null;
  warning_threshold: number | null;
  critical_threshold: number | null;
  threshold_direction: "above" | "below" | string;
  points: DashboardResourcePointRecord[];
  drilldown: DashboardDrilldownRecord;
};

export type DashboardResourcePointRecord = {
  bucket_start: string;
  value: number | null;
};

export type DashboardNetworkRecord = {
  rx_bps: number;
  tx_bps: number;
  points: DashboardNetworkPointRecord[];
  traffic_points: DashboardTrafficPointRecord[];
  top_clients: DashboardNetworkClientRecord[];
  traffic_top_clients: DashboardTrafficClientRecord[];
  traffic_series: DashboardTrafficSeriesRecord[];
};

export type DashboardNetworkPointRecord = {
  bucket_start: string;
  rx_bps: number;
  tx_bps: number;
};

export type DashboardNetworkClientRecord = {
  client_id: string;
  label: string;
  rx_bps: number;
  tx_bps: number;
  interfaces: string[];
  drilldown: DashboardDrilldownRecord;
};

export type DashboardTrafficClientRecord = {
  client_id: string;
  label: string;
  rx_bytes: number;
  tx_bytes: number;
  interfaces: string[];
  drilldown: DashboardDrilldownRecord;
};

export type DashboardTrafficPointRecord = {
  bucket_start: string;
  rx_bytes: number;
  tx_bytes: number;
};

export type DashboardTrafficSeriesRecord = DashboardTrafficClientRecord & {
  points: DashboardTrafficPointRecord[];
};

export type DashboardLabelClusterRecord = {
  label: string;
  kind: string;
  query: string | null;
  total: number;
  online: number;
  offline: number;
  revoked: number;
  stale: number;
  warnings: number;
  running_jobs: number;
  counts_truncated: boolean;
  rx_bps: number;
  tx_bps: number;
  drilldown: DashboardDrilldownRecord;
};

export type DashboardAlertSummaryRecord = {
  id: string;
  severity: string;
  category: string;
  title: string;
  client_id: string | null;
  client_label: string | null;
  observed_at: string;
  drilldown: DashboardDrilldownRecord;
};

export type DashboardAgentSummaryRecord = {
  client_id: string;
  label: string;
  status: AgentView["status"];
  tags: string[];
  drilldown: DashboardDrilldownRecord;
};

export type FleetAlertRecord = {
  id: string;
  severity: "critical" | "warning" | "info" | string;
  category: string;
  target_kind: string;
  target_id: string;
  client_id: string | null;
  title: string;
  detail: string;
  status: string;
  evidence: JsonValue;
  observed_at: string;
  operator_state: "open" | "acknowledged" | "muted" | "escalated" | string;
  muted_until_unix: number | null;
  escalation_level: number;
  state_reason: string | null;
  state_actor_id: string | null;
  state_updated_at: string | null;
};

export type FleetAlertStateRecord = {
  alert_id: string;
  state: "open" | "acknowledged" | "muted" | "escalated" | string;
  muted_until_unix: number | null;
  escalation_level: number;
  reason: string | null;
  actor_id: string | null;
  created_at: string;
  updated_at: string;
};

export type FleetAlertStateRequest = {
  alert_id: string;
  action: "acknowledge" | "mute" | "escalate" | "clear" | string;
  muted_for_secs?: number | null;
  reason?: string | null;
  confirmed: boolean;
};

export type VpsRuleValueRecord = GeneratedVpsRuleValueRecord;
export type VpsRuleChangePreview = GeneratedVpsRuleChangePreview;
export type VpsRulesDryRunRequest = GeneratedVpsRulesDryRunRequest;
export type VpsRulesDryRunResponse = GeneratedVpsRulesDryRunResponse;
export type VpsRulesBulkUpsertRequest = GeneratedVpsRulesBulkUpsertRequest;
export type VpsRulesBulkUnsetRequest = GeneratedVpsRulesBulkUnsetRequest;
export type TrafficAccountingSelectorBreakdown =
  GeneratedTrafficAccountingSelectorBreakdown;
export type TrafficAccountingRecord = GeneratedTrafficAccountingRecord;
export type PolicyRuleRecord = GeneratedPolicyRuleRecord;
export type PolicyGroupRecord = GeneratedPolicyGroupRecord;
export type FleetAlertPolicyRecord = GeneratedPolicyGroupRecord;
export type PolicyRuleRequest = GeneratedPolicyRuleRequest;
export type PolicyGroupRequest = GeneratedPolicyGroupRequest;
export type FleetAlertPolicyRequest = GeneratedPolicyGroupRequest;
export type PolicyDryRunRequest = GeneratedPolicyDryRunRequest;
export type PolicyDryRunRulePreview = GeneratedPolicyDryRunRulePreview;
export type PolicyDryRunResponse = GeneratedPolicyDryRunResponse;
export type PolicyRuleStateRecord = GeneratedPolicyRuleStateRecord;
export type PolicyAlertRecord = GeneratedPolicyAlertRecord;

export type FleetAlertNotificationChannelRecord = {
  id: string;
  name: string;
  scope_kind: "global" | "provider" | "tag" | "client" | string;
  scope_value: string | null;
  min_severity: "critical" | "warning" | "info" | string;
  categories: string[];
  operator_states: string[];
  delivery_kind: string;
  target: string;
  cooldown_secs: number;
  enabled: boolean;
  configuration_error: string | null;
  notes: string | null;
  actor_id: string | null;
  created_at: string;
  updated_at: string;
};

export type FleetAlertNotificationChannelRequest = {
  id?: string;
  name: string;
  scope_kind: string;
  scope_value?: string | null;
  min_severity?: string | null;
  categories?: string[];
  operator_states?: string[];
  delivery_kind: string;
  target: string;
  cooldown_secs?: number | null;
  enabled?: boolean;
  notes?: string | null;
  confirmed: boolean;
};

export type FleetAlertNotificationDeliveryRecord = {
  id: string;
  channel_id: string;
  channel_name: string;
  alert_id: string;
  alert_severity: string;
  alert_category: string;
  status: FleetAlertNotificationDeliveryStatus;
  delivery_kind: string;
  target: string;
  dedupe_key: string;
  payload: JsonValue;
  error: string | null;
  attempt_count: number;
  next_attempt_at: string | null;
  last_attempt_at: string | null;
  cooldown_until_unix: number;
  actor_id: string | null;
  created_at: string;
  delivered_at: string | null;
  review_preview_hash?: string | null;
};

export type FleetAlertNotificationDispatchRequest = {
  limit?: number;
  client_id?: string | null;
  severity?: string | null;
  category?: string | null;
  operator_state?: string | null;
  include_muted?: boolean;
  dry_run?: boolean;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type FleetAlertNotificationProcessRequest = {
  limit?: number;
  status?: FleetAlertNotificationDeliveryProcessStatus | null;
  delivery_kind?: string | null;
  dry_run?: boolean;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type WebhookRuleRecord = {
  id: string;
  name: string;
  enabled: boolean;
  expression: string;
  target: string;
  body_template: string;
  cooldown_secs: number;
  signing_secret_set: boolean;
  notes: string | null;
  actor_id: string | null;
  created_at: string;
  updated_at: string;
};

export type WebhookRuleRequest = {
  id?: string;
  name: string;
  enabled?: boolean;
  expression: string;
  target: string;
  body_template?: string;
  signing_secret?: string | null;
  clear_signing_secret?: boolean;
  cooldown_secs?: number | null;
  notes?: string | null;
  confirmed: boolean;
};

export type WebhookRuleDeliveryRecord = {
  id: string;
  rule_id: string;
  rule_name: string;
  event_kind: string;
  event_id: string;
  status: WebhookRuleDeliveryStatus;
  target: string;
  dedupe_key: string;
  payload: JsonValue;
  matched_vps: AgentView[];
  message: string;
  error: string | null;
  cooldown_until_unix: number;
  attempt_count: number;
  next_attempt_at: string | null;
  last_attempt_at: string | null;
  actor_id: string | null;
  created_at: string;
  delivered_at: string | null;
  review_preview_hash?: string | null;
};

export type WebhookRuleDryRunRecord = {
  rendered_message: string;
  matched_vps: AgentView[];
  payload_context: JsonValue;
  validation_errors: string[];
  delivery: WebhookRuleDeliveryRecord | null;
};

export type WebhookRuleDryRunRequest = {
  name?: string;
  enabled?: boolean;
  expression: string;
  target?: string;
  event_kind?: string;
  event_id?: string | null;
  body_template?: string;
  cooldown_secs?: number | null;
  notes?: string | null;
};

export type WebhookRuleDispatchRequest = {
  rule_id?: string | null;
  event_kind?: string;
  event_id?: string | null;
  limit?: number;
  dry_run?: boolean;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type WebhookRuleProcessRequest = {
  limit?: number;
  status?: WebhookRuleDeliveryProcessStatus | null;
  dry_run?: boolean;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type WebhookDeliveryRotationRequest = {
  older_than?: string | null;
  older_than_days?: number | null;
  status?: WebhookRuleDeliveryHistoryStatus | null;
  rule_id?: string | null;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type WebhookDeliveryRotationResponse = {
  matched_count: number;
  deleted_count: number;
  confirmation_required: boolean;
  older_than: string | null;
  status: string | null;
  rule_id: string | null;
  preview_hash: string;
};

export type AgentView = {
  id: string;
  display_name: string;
  status: string;
  tags: string[];
  registration_ip?: string | null;
  last_ip?: string | null;
  last_seen_at?: string | null;
  arch?: string | null;
  internal_build_number?: number;
  process_incarnation_id?: string | null;
  stale_since?: string | null;
  stale_reason?: string | null;
  capabilities: AgentCapabilitySnapshot;
};

export type DeleteAgentRequest = {
  confirmed: boolean;
  reason?: string | null;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type UpdateAgentAliasRequest = {
  display_name: string;
  confirmed: boolean;
};

export type DeleteAgentResponse = {
  client_id: string;
  deleted: boolean;
  deleted_at: string;
  post_commit: LifecycleOutcomeRecord[];
  runtime_sync: RuntimeConfigDispatchRecord[];
};

export type DeleteAgentBatchTarget = {
  client_id: string;
  request: DeleteAgentRequest;
};

export type DeleteAgentBatchOutcome = {
  client_id: string;
  response: DeleteAgentResponse | null;
  error: string | null;
};

export type AgentCapabilitySnapshot = {
  privilege_mode: "unknown" | "root" | "unprivileged";
  effective_uid?: number | null;
  max_job_timeout_secs: number;
  can_attempt_privileged_ops: boolean;
  can_manage_runtime_tunnels: boolean;
  builtin_tunnel_drivers?: AgentBuiltinTunnelDriverCapabilities;
  can_apply_process_limits: boolean;
  port_forwarding?: PortForwardCapability;
  unprivileged_hint?: string | null;
};

export type AgentBuiltinTunnelDriverCapability = {
  available: boolean;
  version?: string | null;
  unavailable_reason?: string | null;
};

export type AgentBuiltinTunnelDriverCapabilities = {
  iproute2: AgentBuiltinTunnelDriverCapability;
  wireguard: AgentBuiltinTunnelDriverCapability;
  openvpn: AgentBuiltinTunnelDriverCapability;
};

export type PortForwardCapabilityStatus =
  | "supported"
  | "nft_missing"
  | "insufficient_privilege"
  | "inet_nat_unsupported"
  | "probe_failed"
  | "unknown";

export type PortForwardCapability = {
  status: PortForwardCapabilityStatus;
  nft_version?: string | null;
  reason?: string | null;
};

export type PortForwardProtocol = "tcp" | "udp" | "both";

export type PortRange = {
  start: number;
  end: number;
};

export type PortForwardMapping = {
  incoming: PortRange;
  target: PortRange;
};

export type PortForwardRuleRecord = {
  id: string;
  client_id: string;
  name: string;
  protocol: PortForwardProtocol;
  target_ip: string;
  mappings: PortForwardMapping[];
  masquerade: boolean;
  enabled: boolean;
  revision: number;
  desired_status: "enabled" | "disabled" | "removal_pending";
  runtime_status:
    | "absent"
    | "applied"
    | "applied_warning"
    | "pending"
    | "drifted"
    | "unsupported"
    | "failed"
    | "unknown"
    | "disabled"
    | "removal_pending";
  nat_matches: number;
  desired_hash?: string | null;
  agent_desired_hash?: string | null;
  observed_hash?: string | null;
  nft_version?: string | null;
  forwarding_enabled?: boolean | null;
  runtime_observed_unix?: number | null;
  runtime_error_code?: string | null;
  runtime_error?: string | null;
  created_at: string;
  updated_at: string;
  deleted_at?: string | null;
  removal_confirmed_at?: string | null;
  forgotten_at?: string | null;
};

export type PortForwardRuleCorruptRecord = {
  id: string;
  client_id: string;
  name: string;
  enabled: boolean;
  revision: number;
  created_at: string;
  updated_at: string;
  deleted_at?: string | null;
  removal_confirmed_at?: string | null;
  forgotten_at?: string | null;
  configuration_error: string;
};

export type PortForwardRuleListItem =
  PortForwardRuleRecord | PortForwardRuleCorruptRecord;

export type PortForwardRuleInput = {
  name: string;
  protocol: PortForwardProtocol;
  target_ip: string;
  mappings: PortForwardMapping[];
  masquerade: boolean;
  enabled: boolean;
  confirmed: boolean;
};

export type CreatePortForwardRuleRequest = PortForwardRuleInput & {
  client_id: string;
};

export type UpdatePortForwardRuleRequest = PortForwardRuleInput & {
  expected_revision: number;
};

export type PortForwardMutationRequest = {
  expected_revision: number;
  confirmed: boolean;
  reason?: string | null;
};

export type PortForwardSyncRecord = {
  status: string;
  job_id?: string | null;
  error?: string | null;
};

export type PortForwardMutationResponse = {
  rule: PortForwardRuleListItem;
  sync: PortForwardSyncRecord;
};

export type PortForwardBulkAction = "enable" | "disable" | "reapply" | "delete";

export type PortForwardBulkResponse = {
  rules: PortForwardRuleRecord[];
  sync: Array<{ client_id: string; sync: PortForwardSyncRecord }>;
};

export type ResolveHostnameResponse = {
  hostname: string;
  candidates: Array<{ address: string; family: "ipv4" | "ipv6" }>;
};

export type GatewaySessionRecord = {
  id: string;
  gateway_id: string;
  client_id: string;
  status: string;
  noise_public_key_hex: string | null;
  remote_ip: string | null;
  agent_version: string;
  started_at: string;
  last_seen_at: string;
  ended_at: string | null;
  end_reason: string | null;
};

export type TelemetryRollupRecord = {
  client_id: string;
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  cpu_usage_avg: number | null;
  cpu_usage_sample_count: number;
  cpu_cores_max: number;
  cpu_load_1_avg: number;
  cpu_load_1_max: number;
  cpu_load_5_avg: number;
  cpu_load_5_max: number;
  cpu_load_15_avg: number;
  cpu_load_15_max: number;
  memory_total_bytes_max: number;
  memory_available_bytes_avg: number;
  memory_available_bytes_min: number;
  memory_used_ratio_avg: number;
  memory_used_ratio_max: number;
  swap_sample_count: number;
  swap_total_bytes_max: number | null;
  swap_available_bytes_avg: number | null;
  swap_available_bytes_min: number | null;
  swap_used_ratio_avg: number | null;
  swap_used_ratio_max: number | null;
  disk_total_bytes_max: number;
  disk_available_bytes_avg: number;
  disk_available_bytes_min: number;
  disk_used_ratio_avg: number;
  disk_used_ratio_max: number;
  network_rx_bytes_max: number;
  network_tx_bytes_max: number;
  connections_sample_count: number;
  tcp_sockets_latest: number | null;
  udp_sockets_latest: number | null;
  connections_observed_at: string | null;
  latest_observed_at: string;
  updated_at: string;
};

export type TelemetryNetworkRateRecord = {
  client_id: string;
  interface: string;
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  rx_bytes_avg: number;
  tx_bytes_avg: number;
  rx_bytes_delta: number;
  tx_bytes_delta: number;
  rx_bps_avg: number;
  tx_bps_avg: number;
  updated_at: string;
};

export type CurrentPingView = {
  target_id: string;
  target_name: string;
  enabled: boolean;
  generation: number;
  state: string;
  status: string | null;
  latency_avg_ms: number | null;
  loss_ratio: number | null;
  reason: string | null;
  checked_at: string | null;
};

export type MonitoringCardView = {
  client: AgentView;
  billing: BillingPlanView | null;
  system_information: SystemInformationView | null;
  port_speed: PortSpeedView | null;
  resources: TelemetryRollupRecord | null;
  resource_history: TelemetryRollupRecord[];
  network: TelemetryNetworkRateRecord[];
  network_history: TelemetryNetworkRateRecord[];
  traffic: TrafficAccountingRecord;
  primary_ping: CurrentPingView | null;
  primary_ping_history: PingRollupView[];
};

export type PortSpeedView = {
  bps: number;
  display: string;
};

export type BillingPlanView = {
  disabled: boolean;
  price: string | null;
  currency: string | null;
  currency_display: string | null;
  period: "month" | "quarter" | "half_year" | "year" | string | null;
  period_code: "m" | "q" | "hy" | "y" | string | null;
  cycle: string | null;
  display: string;
};

export type SystemInformationView = {
  os_name: string | null;
  architecture: string | null;
  cpu_model: string | null;
  kernel_release: string | null;
  virtualization: string | null;
  reported_at: string | null;
  uptime_secs: number | null;
  uptime_observed_at: string | null;
};

export type MonitoringCardsPageView = {
  items: MonitoringCardView[];
  offset: number;
  limit: number;
  total: number;
  next_offset: number | null;
};

export type MonitoringRangeView = {
  window: string;
  source: "raw" | "minute" | string;
  start_unix: number;
  end_unix: number;
  step_secs: number;
  points: number;
};

export type ClientMonitoringView = {
  client: AgentView;
  system_information: SystemInformationView | null;
  range: MonitoringRangeView;
  resources: TelemetryRollupRecord[];
  network: TelemetryNetworkRateRecord[];
  traffic: TrafficAccountingRecord;
  traffic_history: TrafficHistoryPointView[];
  ping_targets: CurrentPingView[];
  ping: PingRollupView[];
  primary_ping: CurrentPingView | null;
};

export type TrafficHistoryPointView = {
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  reset_count: number;
  rx_bytes: number | null;
  tx_bytes: number | null;
  total_bytes: number | null;
};

export type PublicMonitoringShareView = {
  id: string;
  name: string;
  target_count: number;
  visibility: MonitoringShareVisibilityView;
  expires_at: string;
};

export type PublicMonitoringShareBootstrapView = {
  share: PublicMonitoringShareView;
  visitor_id: string;
};

export type PublicResourceMetricView = {
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  cpu_usage_avg: number | null;
  cpu_cores: number;
  load_1: number;
  load_5: number;
  load_15: number;
  memory_total_bytes: number;
  memory_available_bytes: number;
  memory_used_ratio_avg: number;
  swap_sample_count: number;
  swap_total_bytes?: number;
  swap_available_bytes?: number;
  swap_used_ratio_avg?: number;
  disk_total_bytes: number;
  disk_available_bytes: number;
  disk_used_ratio_avg: number;
  tcp_sockets: number | null;
  udp_sockets: number | null;
  connections_observed_at: string | null;
  observed_at: string;
};

export type PublicNetworkMetricView = {
  rx_bps: number | null;
  tx_bps: number | null;
  observed_at: string | null;
};

export type PublicNetworkPointView = {
  bucket_start: string;
  bucket_secs: number;
  rx_bps: number;
  tx_bps: number;
};

export type PublicTrafficMetricView = {
  configured: boolean;
  cycle_start?: string;
  cycle_end?: string;
  rx_bytes?: number;
  tx_bytes?: number;
  total_bytes?: number;
  quota_rx_bytes?: number;
  quota_tx_bytes?: number;
  quota_total_bytes?: number;
  cycle_percent?: number;
  state: string;
  observed_at?: string;
  port_speed?: PortSpeedView;
};

export type PublicBillingPlanView = {
  disabled: boolean;
  display: string;
  cycle?: string;
};

export type PublicSystemInformationView = {
  os_name?: string;
  architecture?: string;
  cpu_model?: string;
  kernel_release?: string;
  virtualization?: string;
  reported_at?: string;
  uptime_secs?: number;
  uptime_observed_at?: string;
};

export type PublicPingMetricView = {
  target_name: string;
  state: string;
  status: string | null;
  latency_avg_ms: number | null;
  loss_ratio: number | null;
  checked_at: string | null;
};

export type PublicPingPointView = {
  target_name: string;
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  latency_avg_ms: number | null;
  loss_ratio: number;
  status: string;
  checked_at: string;
};

export type PublicMonitoringCardView = {
  client_key: string;
  display_name: string;
  status: string;
  tags?: string[];
  billing?: PublicBillingPlanView;
  system_information?: PublicSystemInformationView;
  resources?: PublicResourceMetricView;
  resource_history?: PublicResourceMetricView[];
  network?: PublicNetworkMetricView;
  network_history?: PublicNetworkPointView[];
  traffic?: PublicTrafficMetricView;
  primary_ping?: PublicPingMetricView;
  primary_ping_history?: PublicPingPointView[];
};

export type PublicMonitoringRangeView = {
  window: string;
  source: string;
  start_unix: number;
  end_unix: number;
  step_secs: number;
  points: number;
};

export type PublicTrafficHistoryPointView = {
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  reset_count: number;
  rx_bytes: number | null;
  tx_bytes: number | null;
  total_bytes: number | null;
};

export type PublicMonitoringDetailView = {
  client_key: string;
  range: PublicMonitoringRangeView;
  resources?: PublicResourceMetricView[];
  network?: PublicNetworkPointView[];
  traffic?: PublicTrafficHistoryPointView[];
  ping_targets?: PublicPingMetricView[];
  ping?: PublicPingPointView[];
};

export type PublicMonitoringDataView = {
  share: PublicMonitoringShareView;
  cards: PublicMonitoringCardView[];
  offset: number;
  total: number;
  next_offset: number | null;
  detail?: PublicMonitoringDetailView;
};

export type PingTargetView = {
  id: string;
  name: string;
  host: string;
  probe_kind: string;
  port: number | null;
  enabled: boolean;
  selector_expression: string;
  generation: number;
  assigned_count: number;
  target_client_ids: string[];
  primary_count: number;
  runtime_sync: {
    state: string;
    reason: string;
  };
  target_update_available: boolean;
  created_at: string;
  updated_at: string;
};

export type PingTargetAssignmentView = {
  target_id: string;
  client: AgentView;
  is_primary: boolean;
  assigned_at: string;
};

export type PingTargetDetailView = {
  target: PingTargetView;
  assignments: PingTargetAssignmentView[];
};

export type PingTargetMutationRequest = {
  name: string;
  host: string;
  probe_kind: string;
  port?: number | null;
  enabled?: boolean;
  selector_expression?: string;
  target_client_ids?: string[];
  confirmed?: boolean;
};

export type PingTargetMutationResponse = {
  target: PingTargetDetailView;
  runtime_sync: RuntimeConfigDispatchRecord[];
};

export type BulkUpdatePingTargetsRequest = {
  target_ids: string[];
  preview_hash?: string | null;
  confirmed?: boolean;
};

export type PingTargetAssignmentChangeView = {
  target_id: string;
  target_name: string;
  selector_expression: string;
  added_client_ids: string[];
  removed_client_ids: string[];
  unchanged_count: number;
};

export type BulkUpdatePingTargetsResponse = {
  preview_hash: string;
  applied: boolean;
  changes: PingTargetAssignmentChangeView[];
  runtime_sync: RuntimeConfigDispatchRecord[];
};

export type MakePrimaryPingTargetRequest = {
  client_ids: string[];
};

export type DeletePingTargetRequest = {
  confirmed?: boolean;
};

export type DeletePingTargetResponse = {
  runtime_sync: RuntimeConfigDispatchRecord[];
};

export type BulkPingTargetLifecycleRequest = {
  target_ids: string[];
  action: "enable" | "disable" | "delete";
  confirmed?: boolean;
};

export type BulkPingTargetLifecycleResponse = {
  action: "enable" | "disable" | "delete";
  affected_target_ids: string[];
  runtime_sync: RuntimeConfigDispatchRecord[];
};

export type PingRollupView = {
  client_id: string;
  target_id: string;
  target_name: string;
  is_primary: boolean;
  generation: number;
  bucket_start: string;
  bucket_secs: number;
  sample_count: number;
  success_count: number;
  latency_avg_ms: number | null;
  latency_min_ms: number | null;
  latency_max_ms: number | null;
  loss_ratio_avg: number;
  loss_ratio_max: number;
  latest_status: string;
  latest_reason: string | null;
  latest_checked_at: string;
};

export type MonitoringShareListQuery = {
  status?: string | null;
  limit?: number | null;
  offset?: number | null;
};

export type MonitoringShareVisibilityView = {
  identity_context: boolean;
  billing: boolean;
  system_information: boolean;
  resources: boolean;
  network: boolean;
  traffic: boolean;
  ping: boolean;
  detail_history: boolean;
};

export type MonitoringShareVisibilityRequest = {
  identity_context?: boolean;
  billing?: boolean;
  system_information?: boolean;
  resources?: boolean;
  network?: boolean;
  traffic?: boolean;
  ping?: boolean;
  detail_history?: boolean;
};

export type MonitoringShareView = {
  id: string;
  name: string;
  selector_expression: string;
  target_count: number;
  target_client_ids: string[];
  target_update_available: boolean;
  visibility: MonitoringShareVisibilityView;
  status: string;
  expires_at: string;
  revoked_at: string | null;
  created_by: string | null;
  created_at: string;
  updated_at: string;
  visitor_count: number;
  first_visited_at: string | null;
  last_visited_at: string | null;
};

export type MonitoringShareUrlResponse = {
  fragment_path: string;
};

export type CreateMonitoringShareRequest = {
  name: string;
  selector_expression?: string;
  target_client_ids?: string[];
  visibility: MonitoringShareVisibilityRequest;
  expires_in_secs: number;
  confirmed?: boolean;
};

export type CreateMonitoringShareResponse = {
  share: MonitoringShareView;
  fragment_path: string;
};

export type ExtendMonitoringSharesRequest = {
  share_ids: string[];
  extend_by_secs: number;
};

export type RevokeMonitoringSharesRequest = {
  share_ids: string[];
};

export type BulkUpdateMonitoringShareTargetsRequest = {
  share_ids: string[];
  preview_hash?: string;
  confirmed?: boolean;
};

export type MonitoringShareTargetChangeView = {
  share_id: string;
  share_name: string;
  selector_expression: string;
  added_client_ids: string[];
  removed_client_ids: string[];
  unchanged_count: number;
};

export type BulkUpdateMonitoringShareTargetsResponse = {
  preview_hash: string;
  applied: boolean;
  changes: MonitoringShareTargetChangeView[];
};

export type MonitoringSharesMutationResponse = {
  shares: MonitoringShareView[];
};

export type TelemetryTunnelRecord = {
  client_id: string;
  observed_at: string;
  interface: string;
  kind: string;
  ownership_mode: string;
  mutation_policy: string;
  plan_id: string | null;
  plan_name: string | null;
  plan_runtime_manager: RuntimeTunnelManager | null;
  endpoint_side: TunnelEndpointSide | null;
  peer_client_id: string | null;
  source: string;
  operstate: string | null;
  mtu: number | null;
  link_type: number | null;
  address: string | null;
  rx_bytes: number;
  tx_bytes: number;
  traffic_source?: string | null;
  traffic_status?: string | null;
  traffic_reason?: string | null;
  traffic_checked_unix?: number | null;
  adapter_health: TelemetryTunnelAdapterHealth | null;
  latency_monitoring_enabled?: boolean | null;
  latency_status?: string | null;
  latency_reason?: string | null;
  latency_primary_family?: TunnelAddressFamily | null;
  latency_target?: string | null;
  latency_checked_unix?: number | null;
  latency_avg_ms?: number | null;
  packet_loss_ratio?: number | null;
  latency_healthy_windows?: number | null;
  latency_missed_windows?: number | null;
};

export type TelemetryTunnelAdapterHealth = {
  status: string;
  checked_unix: number;
  configured: boolean;
  success: boolean;
  exit_code: number | null;
  reason: string | null;
  duration_ms: number;
  command_sha256_hex: string | null;
  timed_out: boolean;
  output_truncated: boolean;
  stdout_sha256_hex: string | null;
  stderr_sha256_hex: string | null;
};

export type WsEvent =
  | { type: "hello"; service: string; stream: string }
  | { type: "fleet_snapshot"; summary: FleetSummary; agents: AgentView[] }
  | { type: "agent_updated"; client_id: string; gateway_id: string }
  | {
      type: "telemetry_updated";
      client_id: string;
      observed_unix: number;
      gateway_id: string;
    }
  | {
      type: "job_rejected";
      job_id: string;
      status: JobStatus;
    }
  | {
      type: "job_output_recorded";
      job_id: string;
      client_id: string;
      seq: number;
      done: boolean;
    }
  | {
      type: "terminal_output_recorded";
      job_id: string;
      client_id: string;
      session_id: string;
      terminal_seq: number | null;
      done: boolean;
    }
  | {
      type: "job_finished";
      job_id: string;
      status: JobStatus;
    }
  | {
      type: "backup_artifact_recorded";
      backup_request_id: string;
      client_id: string;
      artifact_id: string;
    };

export type WsJobOutputEvent = Extract<
  WsEvent,
  { type: "job_output_recorded" }
>;
export type OperatorView = {
  id: string;
  username: string;
  status: "active" | "disabled" | "deleted" | string;
  role: string;
  scopes: string[];
  preferences: OperatorPreferences;
  totp_enabled: boolean;
  session_refresh_ttl_secs: number;
  created_at: string;
  disabled_at: string | null;
  deleted_at: string | null;
};

export type OperatorPreferences = {
  vps_name_display_mode: "name" | "name_id_suffix";
  timezone: string | null;
  language: "en";
  show_country_flags: boolean;
  fleet_tag_visibility_overrides: Record<string, boolean>;
  gateway_endpoints: string;
  gateway_server_public_key_hex: string | null;
  agent_install_mode: "root" | "user" | "staged";
  sidebar_subpanel_default: "active" | "all";
  review_prompt_mode: "inline" | "overlay";
  dashboard_curve_exclusions: string[];
  dashboard_resource_top_limit: number;
  dashboard_network_top_limit: number;
  bulk_output_compare_mode: JobOutputCompareMode;
};

export type OperatorSessionRecord = {
  id: string;
  operator_id: string;
  operator_username: string;
  operator_role: string;
  current: boolean;
  created_at: string;
  expires_at: string;
  refresh_expires_at: string;
  revoked: boolean;
  revoked_at: string | null;
};

export type OperatorAuthEventRecord = {
  id: string;
  operator_id: string | null;
  username: string;
  result: "success" | "failure" | "throttled" | string;
  reason: string | null;
  remote_ip: string | null;
  user_agent: string | null;
  session_id: string | null;
  created_at: string;
};

export type AuthResponse = {
  token_type: "Bearer";
  access_token: string;
  refresh_token: string;
  session_id: string;
  expires_in_secs: number;
  refresh_expires_in_secs: number;
  operator: OperatorView;
};

export type TotpSetupResponse = {
  operator_id: string;
  secret_base32: string;
  otpauth_uri: string;
  algorithm: "SHA1";
  digits: number;
  period_secs: number;
};

export type JobHistoryRecord = {
  id: string;
  actor_id: string | null;
  command_type: string;
  source_schedule_id: string | null;
  privileged: boolean;
  status: JobStatus;
  target_count: number;
  payload_hash: string;
  max_timeout_secs: number;
  created_at: string;
  completed_at: string | null;
};

export type JobApprovalStatus = "pending" | "approved" | "rejected";

export type JobApprovalRecord = {
  id: string;
  status: JobApprovalStatus;
  job_id: string;
  command_type: string;
  selector_expression: string;
  target_client_ids: string[];
  target_count: number;
  privileged: boolean;
  destructive: boolean;
  force_unprivileged: boolean;
  max_timeout_secs: number;
  payload_hash: string;
  request_fingerprint: string;
  requester_id: string | null;
  requester_username: string;
  requester_role: string;
  requested_at: string;
  request_reason: string | null;
  risk: string;
  decision_by: string | null;
  decision_username: string | null;
  decision_reason: string | null;
  decided_at: string | null;
};

export type ServerJobRecord = {
  id: string;
  job_type: ServerJobType;
  status: ServerJobStatus;
  expression: string | null;
  preview_hash: string | null;
  matched_count: number;
  matched_bytes: number;
  deleted_count: number;
  deleted_bytes: number;
  error: string | null;
  created_by: string | null;
  metadata: JsonValue;
  created_at: string;
  started_at: string | null;
  completed_at: string | null;
  canceled_at: string | null;
};

export type ArtifactCleanupPreviewRecord = {
  expression: string;
  domains: string[];
  preview_hash: string;
  matched_count: number;
  matched_bytes: number;
  oldest_created_at?: string | null;
  newest_created_at?: string | null;
  retained_count?: number | null;
  reference_protected_count?: number | null;
  representative_objects?: Array<{
    id?: string | null;
    domain: string;
    object_key: string;
    size_bytes: number;
    status: string;
    created_at?: string | null;
    reference_protected?: boolean | null;
    reason?: string | null;
  }> | null;
  full_list_download_url?: string | null;
};

export type CommandTemplateRecord = {
  id: string;
  name: string;
  built_in: boolean;
  scope_kind: "global" | "provider" | "tag" | "client" | string;
  scope_value: string | null;
  command_type: JobCommandType;
  display_group: string | null;
  operation: JobOperation;
  defaults: JsonValue;
  actor_id: string | null;
  created_at: string;
  updated_at: string;
};

export type UpsertCommandTemplateRequest = {
  name: string;
  scope_kind: string;
  scope_value?: string | null;
  display_group?: string | null;
  operation: JobOperation;
  defaults?: JsonValue;
  confirmed: boolean;
};

export type DeleteCommandTemplateRequest = {
  confirmed: boolean;
  reviewed_name: string;
};

export type JobOutputCompareMode = "binary" | "text";

export type JobOutputComparisonRecord = {
  job_id: string;
  mode: JobOutputCompareMode;
  compared_at: string;
  total_targets: number;
  compared_targets: number;
  group_count: number;
  groups: JobOutputComparisonGroupRecord[];
  rows: JobOutputComparisonRowRecord[];
};

export type JobOutputComparisonStatus = JobTargetStatus | "unknown";

export type JobOutputComparisonGroupRecord = {
  group_id: string;
  status: JobOutputComparisonStatus;
  exit_code: number | null;
  output_digest_hex: string;
  output_compare_basis: "binary" | "text" | "binary_metadata" | string;
  target_count: number;
  stream_count: number;
  byte_count: number;
  representative_client_id: string;
  client_ids: string[];
  preview: string;
};

export type JobOutputComparisonRowRecord = {
  job_id: string;
  client_id: string;
  group_id: string;
  status: JobOutputComparisonStatus;
  exit_code: number | null;
  output_digest_hex: string;
  output_compare_basis: "binary" | "text" | "binary_metadata" | string;
  stream_count: number;
  byte_count: number;
  matches_largest_group: boolean;
  preview: string;
};

export type ScheduleRecord = {
  id: string;
  name: string;
  enabled: boolean;
  command_type: string;
  operation: JobOperation | null;
  operation_error?: string | null;
  operation_payload_hash?: string;
  selector_expression: string;
  target_client_ids: string[];
  cron_expr: string;
  cadence_error: string | null;
  timezone: "UTC" | string;
  next_runs: string[];
  catch_up_policy: string;
  catch_up_limit: number;
  retry_delay_secs: number;
  max_failures: number;
  failure_count: number;
  last_error: string | null;
  next_run_at: string;
  last_run_at: string | null;
  deferred_until: string | null;
  deleted_at: string | null;
  created_at: string;
  updated_at: string;
};

export type TunnelKind =
  | "gre"
  | "ipip"
  | "sit"
  | "fou"
  | "openvpn"
  | "wireguard"
  | "tun_tap"
  | "custom";
export type TunnelEndpointSide = "left" | "right";
export type RuntimeTunnelManager =
  "agent_builtin" | "external_observed" | "custom_adapter";

export type RuntimeTunnelCommand = {
  argv: string[];
  max_timeout_secs?: number;
  max_output_bytes?: number;
};

export type RoutingCostAdapterCommands = {
  source?: "plan_override" | "configuration_preset";
  template_id: string;
  template_name: string;
  definition_hash: string;
  status: RuntimeTunnelCommand;
  update: RuntimeTunnelCommand;
};

export type RuntimeTunnelAdapterCommands = {
  template_id: string;
  template_name: string;
  definition_hash: string;
  startup?: RuntimeTunnelCommand | null;
  stop?: RuntimeTunnelCommand | null;
  cleanup?: RuntimeTunnelCommand | null;
  restart?: RuntimeTunnelCommand | null;
  status: RuntimeTunnelCommand;
  traffic_limit_apply?: RuntimeTunnelCommand | null;
};

export type RuntimeTunnelTrafficLimit = {
  ingress_kbps?: number | null;
  egress_kbps?: number | null;
  burst_kb?: number | null;
};

export type RuntimeTunnelFouOptions = {
  port: number;
  peer_port: number;
  ipproto: number;
};

export type RuntimeTunnelWireguardEndpointMode = "left" | "right" | "both";

export type RuntimeTunnelWireguardOptions = {
  endpoint_mode: RuntimeTunnelWireguardEndpointMode;
  left_listen_port: number;
  right_listen_port: number;
  left_keepalive_secs: number;
  right_keepalive_secs: number;
};

export type RuntimeTunnelOpenvpnTransport = "udp" | "tcp";

export type RuntimeTunnelOpenvpnOptions = {
  transport: RuntimeTunnelOpenvpnTransport;
  listener_side: TunnelEndpointSide;
  port: number;
};

export type RuntimeTunnelControl = {
  manager: RuntimeTunnelManager;
  left_adapter_template_id?: string | null;
  right_adapter_template_id?: string | null;
  traffic_limit?: RuntimeTunnelTrafficLimit;
  fou?: RuntimeTunnelFouOptions;
  wireguard?: RuntimeTunnelWireguardOptions;
  openvpn?: RuntimeTunnelOpenvpnOptions;
};

export type TunnelWireguardPublicEvidence = {
  public_key_base64: string;
};

export type TunnelOpenvpnPublicEvidence = {
  certificate_sha256_fingerprint: string;
};

export type TunnelBuiltinCredentialEvidence =
  | {
      kind: "wireguard";
      generation: number;
      left: TunnelWireguardPublicEvidence;
      right: TunnelWireguardPublicEvidence;
    }
  | {
      kind: "openvpn";
      generation: number;
      left: TunnelOpenvpnPublicEvidence;
      right: TunnelOpenvpnPublicEvidence;
    };

export type OspfControlMode = "reviewed" | "automatic";

export type OspfCostPolicy = {
  latency_weight: number;
  loss_weight: number;
  bandwidth_weight: number;
  preference_bias: number;
  min_cost: number;
  max_cost: number;
};

export type TunnelOspfConfig = {
  mode: OspfControlMode;
  planned_latency_ms: number;
  planned_packet_loss_ratio: number;
  preference: number;
  policy: OspfCostPolicy;
  min_cost_delta: number;
  healthy_windows: number;
  left_adapter_template_id?: string | null;
  right_adapter_template_id?: string | null;
};

export type TunnelPlanInput = {
  name: string;
  interface_name: string;
  kind: TunnelKind;
  runtime_control?: RuntimeTunnelControl;
  runtime_topology?: RuntimeTunnelTopologyIntent;
  left_client_id: string;
  right_client_id: string;
  left_remote_underlay: string;
  left_local_underlay?: string | null;
  right_remote_underlay: string;
  right_local_underlay?: string | null;
  address_pool_cidr: string;
  reserved_addresses: string[];
  ipv4_tunnel?: TunnelAddressPair | null;
  ipv6_address_pool_cidr?: string | null;
  ipv6_tunnel?: TunnelAddressPair | null;
  latency_primary_family?: TunnelAddressFamily;
  bandwidth_mbps: number;
  left_mtu?: number | null;
  right_mtu?: number | null;
  ospf?: TunnelOspfConfig | null;
};

export type TunnelAddressFamily = "ipv4" | "ipv6";

export type TunnelAddressPair = {
  left: string;
  right: string;
  prefix_len: number;
};

export type RuntimeTunnelRoute = {
  destination_cidr: string;
  via?: string | null;
  interface_name?: string | null;
  metric?: number | null;
};

export type RuntimeTunnelTopologyIntent = {
  version?: string | null;
  desired_interfaces?: string[];
  stale_interfaces?: string[];
  routes?: RuntimeTunnelRoute[];
  stale_routes?: RuntimeTunnelRoute[];
};

export type TunnelPlan = TunnelPlanInput & {
  left_tunnel_address: string;
  right_tunnel_address: string;
  tunnel_prefix_len: number;
  ipv4_tunnel?: TunnelAddressPair | null;
  ipv6_tunnel?: TunnelAddressPair | null;
  latency_primary_family?: TunnelAddressFamily;
  runtime_control?: RuntimeTunnelControl;
  ospf?: TunnelOspfConfig | null;
  recommended_ospf_cost: number | null;
  conflicts: string[];
};

export type TunnelPlanExport = TunnelPlan;

export type TunnelPlanRecord = {
  id: string;
  name: string;
  kind: TunnelKind;
  enabled: boolean;
  revision: number;
  left_client_id: string;
  right_client_id: string;
  recommended_ospf_cost: number | null;
  ospf_status: string;
  left_ospf_status: string;
  right_ospf_status: string;
  desired_ospf_cost: number | null;
  left_current_ospf_cost: number | null;
  right_current_ospf_cost: number | null;
  left_ospf_job_id: string | null;
  right_ospf_job_id: string | null;
  connection_assessment: TunnelConnectionAssessment;
  connection_assessment_note: string | null;
  connection_assessed_at: string | null;
  connection_assessed_by: string | null;
  left_runtime_config: TunnelPlanEndpointRuntimeConfig;
  right_runtime_config: TunnelPlanEndpointRuntimeConfig;
  input: TunnelPlanInput;
  plan: TunnelPlan;
  builtin_credentials: TunnelBuiltinCredentialEvidence | null;
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  deleted_by: string | null;
  deleted_reason: string | null;
};

export type TunnelPlanCorruptRecord = {
  id: string;
  name: string;
  kind: string;
  enabled: boolean;
  revision: number;
  left_client_id: string;
  right_client_id: string;
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  configuration_error: string;
};

export type TunnelPlanListItem = TunnelPlanRecord | TunnelPlanCorruptRecord;

export type TunnelPlanEndpointRuntimeConfig = {
  client_id: string;
  desired: "present" | "absent" | string;
  status:
    | "queued"
    | "pending"
    | "failed"
    | "applied"
    | "removed"
    | "not_applied"
    | "removal_required"
    | "not_dispatched"
    | "stale_pending"
    | string;
  job_id: string | null;
  error: string | null;
  updated_at: string | null;
};

export type RuntimeConfigDispatchRecord = {
  client_id: string;
  status: "queued" | "not_queued" | "queue_failed" | string;
  job_id: string | null;
  error: string | null;
};

export type LifecycleOutcomeRecord = {
  operation: string;
  status: "completed" | "failed" | string;
  error: string | null;
};

export type TunnelPlanMutationResponse = {
  plan: TunnelPlanRecord;
  sync: RuntimeConfigDispatchRecord[];
};

export type TopologyGraphNode = {
  client_id: string;
  display_name: string;
  status: TopologyNodeStatus;
  tags: string[];
  tunnel_count: number;
  healthy_tunnel_count: number;
  degraded_tunnel_count: number;
  latest_observed_at: string | null;
};

export type TopologyGraphEdge = {
  plan_id: string;
  topology_identity_hash: string;
  plan_name: string;
  interface_name: string;
  kind: TunnelKind;
  left_client_id: string;
  right_client_id: string;
  enabled: boolean;
  health: TopologyEdgeHealthStatus;
  left_runtime_state: TunnelEndpointRuntimeState;
  right_runtime_state: TunnelEndpointRuntimeState;
  left_runtime_reason: string | null;
  right_runtime_reason: string | null;
  left_reachability_state: TunnelEndpointReachabilityState;
  right_reachability_state: TunnelEndpointReachabilityState;
  left_reachability_reason: string | null;
  right_reachability_reason: string | null;
  left_reachability_source: "automatic" | "manual" | string | null;
  right_reachability_source: "automatic" | "manual" | string | null;
  left_reachability_observed_at: string | null;
  right_reachability_observed_at: string | null;
  left_observed_at: string | null;
  right_observed_at: string | null;
  unavailable_client_ids: string[];
  availability_reasons: string[];
  neighbor_state: TopologyNeighborState;
  reachability_state: TopologyObservationState;
  runtime_state: TopologyRuntimeState;
  runtime_reasons: string[];
  adapter_state: TopologyRuntimeState;
  routing_state: TopologyRuntimeState;
  kernel_link_probe_state: TopologyProbeState;
  kernel_neighbor_probe_state: TopologyProbeState;
  kernel_route_probe_state: TopologyProbeState;
  kernel_namespace_covered: boolean;
  desired_missing_count: number;
  stale_present_count: number;
  bandwidth_mbps: number;
  recommended_ospf_cost: number | null;
  cost_delta: number | null;
  latency_avg_ms: number | null;
  latest_latency_avg_ms: number | null;
  latency_series_ms: number[];
  packet_loss_avg_ratio: number | null;
  throughput_avg_mbps: number | null;
  latest_speed_mbps: number | null;
  throughput_max_mbps: number | null;
  sample_count: number;
  degraded_count: number;
  latest_observed_at: string | null;
  left_tunnel_address: string;
  right_tunnel_address: string;
  ipv4_tunnel?: TunnelAddressPair | null;
  ipv6_tunnel?: TunnelAddressPair | null;
  latency_primary_family?: TunnelAddressFamily;
};

export type TopologyGraph = {
  nodes: TopologyGraphNode[];
  edges: TopologyGraphEdge[];
  generated_at: string;
  start_unix: number;
  end_unix: number;
};

export type JobTargetRecord = {
  job_id: string;
  client_id: string;
  status: JobTargetStatus;
  message?: string | null;
  exit_code: number | null;
  started_at: string | null;
  deadline_at?: string | null;
  completed_at: string | null;
  process_incarnation_id?: string | null;
};

export type JobOutputRecord = {
  job_id: string;
  client_id: string;
  seq: number;
  stream: string;
  data_base64: string;
  storage?: string;
  artifact_object_key?: string | null;
  artifact_sha256_hex?: string | null;
  artifact_size_bytes?: number | null;
  exit_code: number | null;
  done: boolean;
  received_at?: string | null;
  created_at: string;
};

export type JobOutputListPageRecord = {
  items: JobOutputRecord[];
  limit: number;
  next_cursor: string | null;
  has_more: boolean;
};

export type RestoreRollbackFile = {
  archive_path: string;
  destination_path: string;
  rollback_path: string | null;
  restored_size_bytes: number;
  restored_sha256_hex: string;
};

export type ProcessSupervisorInventoryRecord = {
  client_id: string;
  name: string;
  status: string;
  pid: number | null;
  process_exit_code: number | null;
  source_job_id: string;
  source_command_type: string;
  stdout_log: string | null;
  stderr_log: string | null;
  started_unix: number | null;
  restart_attempts: number | null;
  last_exit_code: number | null;
  last_exit_unix: number | null;
  last_restart_unix: number | null;
  limit_effectiveness_status: string | null;
  cgroup_status: string | null;
  cgroup_process_count: number | null;
  cgroup_cpu_weight: number | null;
  cgroup_memory_current_bytes: number | null;
  cgroup_pids_current: number | null;
  observed_at: string;
};

export type HostProcessRecord = {
  pid: number;
  ppid: number;
  uid: number;
  state: string;
  name: string;
  command: string;
  rss_kib: number;
};

export type HostProcessAttemptRecord = {
  job_id: string;
  status: string;
  message: string | null;
  completed_at: string | null;
};

export type HostProcessInventoryRecord = {
  client_id: string;
  source_job_id: string | null;
  source: string | null;
  truncated: boolean;
  observed_at: string | null;
  processes: HostProcessRecord[];
  last_attempt: HostProcessAttemptRecord | null;
};

export type HostServiceProvider = "systemd" | "openrc" | "sysv";
export type HostServiceAction =
  "start" | "stop" | "restart" | "enable" | "disable";

export type HostServiceCapabilityRecord = {
  status: "supported" | "ambiguous" | "probe_failed" | "unsupported";
  provider: HostServiceProvider | null;
  provider_version: string | null;
  can_inventory: boolean;
  can_start_stop_restart: boolean;
  can_enable_disable: boolean;
  can_read_logs: boolean;
  enable_backend: string | null;
  reason: string | null;
};

export type HostServiceRecord = {
  name: string;
  description: string;
  load_state: string;
  active_state: string;
  sub_state: string;
  enabled_state: string;
  state_reason: string | null;
};

export type HostServiceInventoryRecord = {
  client_id: string;
  source_job_id: string | null;
  observed_at: string | null;
  capability: HostServiceCapabilityRecord | null;
  truncated: boolean;
  services: HostServiceRecord[];
  last_attempt: HostProcessAttemptRecord | null;
};

export type HostStorageProvider = "lsblk_json" | "lsblk_pairs";

export type HostStorageCapabilityRecord = {
  status: "supported" | "probe_failed" | "unsupported";
  provider: HostStorageProvider | null;
  provider_version: string | null;
  available_columns: string[];
  can_report_filesystem_usage: boolean;
  reason: string | null;
};

export type HostBlockDeviceRecord = {
  name: string;
  path: string;
  kernel_name: string | null;
  parent_path: string | null;
  device_type: string;
  size_bytes: number;
  filesystem_type: string | null;
  filesystem_version: string | null;
  label: string | null;
  uuid: string | null;
  mount_points: string[];
  filesystem_available_bytes: number | null;
  filesystem_used_percent: number | null;
  read_only: boolean;
  removable: boolean;
  model: string | null;
  serial: string | null;
  transport: string | null;
  major_minor: string | null;
};

export type HostMountRecord = {
  mount_id: number;
  parent_id: number;
  major_minor: string;
  root: string;
  target: string;
  filesystem_type: string;
  source: string;
  options: string[];
  read_only: boolean;
  pseudo: boolean;
};

export type HostStorageInventoryRecord = {
  client_id: string;
  source_job_id: string | null;
  observed_at: string | null;
  capability: HostStorageCapabilityRecord | null;
  include_pseudo_mounts: boolean;
  devices_truncated: boolean;
  mounts_truncated: boolean;
  devices: HostBlockDeviceRecord[];
  mounts: HostMountRecord[];
  last_attempt: HostProcessAttemptRecord | null;
};

export type HostServiceLogSnapshot = {
  type: "service_logs";
  provider: HostServiceProvider;
  service: string;
  truncated: boolean;
  lines: string[];
};

export type HostPackageProvider = "apt" | "dnf" | "yum" | "pacman";

export type HostPackageCapabilityRecord = {
  status: "supported" | "ambiguous" | "probe_failed" | "unsupported";
  provider: HostPackageProvider | null;
  distro_id: string;
  distro_version: string | null;
  can_plan_cached: boolean;
  can_refresh_metadata: boolean;
  can_apply: boolean;
  reason: string | null;
};

export type HostPackageUpdateRecord = {
  name: string;
  architecture: string | null;
  current_version: string | null;
  candidate_version: string;
  repository: string | null;
};

export type HostPackageUpdatePlanRecord = {
  client_id: string;
  source_job_id: string | null;
  observed_at: string | null;
  capability: HostPackageCapabilityRecord | null;
  metadata_refresh_requested: boolean;
  metadata_refreshed: boolean;
  plan_hash: string | null;
  truncated: boolean;
  packages: HostPackageUpdateRecord[];
  reboot_required_before: boolean | null;
  last_attempt: HostProcessAttemptRecord | null;
  evidence_error: string | null;
};

export type HostPackageUpdateApplyResult = {
  type: "package_update_apply";
  provider: HostPackageProvider;
  accepted_plan_hash: string;
  applied_package_count: number;
  remaining_packages: HostPackageUpdateRecord[];
  completed: boolean;
  reboot_required_after: boolean | null;
};

export type AgentUpdateReleaseRecord = {
  id: string;
  actor_id: string | null;
  name: string;
  version: string;
  channel: string;
  status: AgentUpdateReleaseStatus;
  artifact_sha256_hex: string;
  artifact_url_sha256_hex: string | null;
  rollback_artifact_sha256_hex: string | null;
  rollback_artifact_url_sha256_hex: string | null;
  rollback_size_bytes: number | null;
  size_bytes: number | null;
  notes: string | null;
  created_at: string;
};

export type CreateAgentUpdateReleaseRequest = {
  name: string;
  version: string;
  channel: string;
  artifact_url: string;
  artifact_sha256_hex: string;
  rollback_artifact_sha256_hex?: string | null;
  rollback_artifact_url?: string | null;
  rollback_size_bytes?: number | null;
  size_bytes?: number | null;
  notes?: string | null;
  confirmed: boolean;
};

export type NetworkObservationRecord = {
  id: string;
  job_id: string | null;
  client_id: string;
  seq: number | null;
  kind: string;
  source: "automatic" | "manual" | string;
  role: string | null;
  plan_id: string | null;
  topology_identity_hash: string | null;
  plan_name: string | null;
  interface_name: string | null;
  peer_client_id: string | null;
  target: string | null;
  endpoint_side: "left" | "right" | string | null;
  address_family: "ipv4" | "ipv6" | string | null;
  stale_after_secs: number | null;
  healthy: boolean | null;
  transmitted: number | null;
  received: number | null;
  latency_min_ms: number | null;
  latency_avg_ms: number | null;
  latency_max_ms: number | null;
  latency_mdev_ms: number | null;
  packet_loss_ratio: number | null;
  reason: string | null;
  throughput_mbps: number | null;
  bytes: number | null;
  metadata: JsonValue;
  observed_at: string;
  received_at: string;
};

export type NetworkObservationTrendRecord = {
  kind: string;
  plan_id: string | null;
  topology_identity_hash: string | null;
  plan_name: string | null;
  interface_name: string | null;
  client_id: string;
  peer_client_id: string | null;
  sample_count: number;
  automatic_count: number;
  manual_count: number;
  healthy_count: number;
  degraded_count: number;
  latency_avg_ms: number | null;
  latency_min_ms: number | null;
  latency_max_ms: number | null;
  packet_loss_avg_ratio: number | null;
  throughput_avg_mbps: number | null;
  throughput_max_mbps: number | null;
  bytes_total: number;
  latest_observed_at: string;
};

export type NetworkOspfRecommendationRecord = {
  recommendation_id: string;
  plan_id: string;
  plan_name: string;
  interface_name: string;
  left_client_id: string;
  right_client_id: string;
  configured_bandwidth_mbps: number;
  effective_bandwidth_mbps: number;
  plan_ospf_cost: number;
  recommended_ospf_cost: number;
  cost_delta: number;
  latency_avg_ms: number | null;
  packet_loss_avg_ratio: number | null;
  throughput_avg_mbps: number | null;
  throughput_max_mbps: number | null;
  sample_count: number;
  degraded_count: number;
  latest_observed_at: string | null;
  confidence: string;
  reason: string;
  evidence_summary: string;
};

export type NetworkOspfUpdateEvidenceRecord = {
  configured_bandwidth_mbps: number;
  effective_bandwidth_mbps: number;
  latency_avg_ms: number | null;
  packet_loss_avg_ratio: number | null;
  throughput_avg_mbps: number | null;
  throughput_max_mbps: number | null;
  sample_count: number;
  degraded_count: number;
  healthy_probe_streak: number;
  required_healthy_probe_streak: number;
  latest_observed_at: string | null;
  reason: string;
};

export type NetworkOspfUpdatePlanRecord = {
  recommendation_id: string;
  plan_id: string;
  plan_revision: number;
  plan_name: string;
  interface_name: string;
  left_client_id: string;
  right_client_id: string;
  control_mode: OspfControlMode;
  left_updater_source:
    "plan_override" | "configuration_preset" | "unconfigured";
  right_updater_source:
    "plan_override" | "configuration_preset" | "unconfigured";
  left_adapter_template_id: string | null;
  right_adapter_template_id: string | null;
  left_adapter_template_name: string | null;
  right_adapter_template_name: string | null;
  left_adapter_definition_hash: string | null;
  right_adapter_definition_hash: string | null;
  left_current_ospf_cost: number | null;
  right_current_ospf_cost: number | null;
  left_ospf_status: string;
  right_ospf_status: string;
  recommended_ospf_cost: number;
  maximum_cost_delta: number;
  status: string;
  confidence: string;
  requires_approval: boolean;
  privilege_required: boolean;
  mutation_mode: string;
  approval_scope: string[];
  evidence: NetworkOspfUpdateEvidenceRecord;
  change_summary: string;
  evidence_summary: string;
};

export type PrivilegeAssertion = {
  nonce_hex: string;
  issued_unix: number;
  expires_unix: number;
  assertion_hex: string;
};

export type BackupMissingPathPolicy = "fail" | "skip";

export type JobOperation =
  | { type: "shell"; argv: string[]; pty: boolean }
  | { type: "shell_script"; script: string }
  | {
      type: "terminal_open";
      session_id: string;
      argv: string[];
      cwd: string | null;
      user?: string | null;
      user_policy?: "fail" | "fallback";
      cols: number;
      rows: number;
      replay_from_seq?: number;
      idle_timeout_secs: number;
      flow_window_bytes: number;
    }
  | { type: "file_pull"; path: string; follow_symlinks: boolean }
  | { type: "config_read" }
  | {
      type: "runtime_config_sync";
      desired_version: number;
      reason: string;
      config: JsonValue;
    }
  | {
      type: "agent_update";
      artifact_url: string;
      sha256_hex: string;
    }
  | {
      type: "agent_update_activate";
      staged_sha256_hex: string;
      restart_agent?: boolean;
    }
  | { type: "agent_update_rollback"; rollback_sha256_hex?: string }
  | {
      type: "agent_update_check";
      version_url?: string;
      activate?: boolean;
      restart_agent?: boolean;
    }
  | { type: "agent_stop" }
  | { type: "agent_restart" }
  | {
      type: "file_push";
      path: string;
      mode: number;
      size_bytes: number;
      sha256_hex: string;
      data_base64: string;
      existing_policy?: FileExistingPolicy;
      owner?: string | null;
      group?: string | null;
      uid?: number | null;
      gid?: number | null;
      ownership_policy?: FileOwnershipPolicy;
    }
  | {
      type: "file_push_chunked";
      path: string;
      mode: number;
      size_bytes: number;
      sha256_hex: string;
      chunks: Array<{
        offset: number;
        size_bytes: number;
        sha256_hex: string;
        data_base64: string;
      }>;
      existing_policy?: FileExistingPolicy;
      owner?: string | null;
      group?: string | null;
      uid?: number | null;
      gid?: number | null;
      ownership_policy?: FileOwnershipPolicy;
    }
  | {
      type: "file_transfer_start";
      session_id: string;
      path: string;
      mode: number;
      size_bytes: number;
      sha256_hex: string;
      chunk_size_bytes: number;
      rate_limit_kbps: number;
      existing_policy?: FileExistingPolicy;
      resume_token_hash: string;
    }
  | {
      type: "file_transfer_chunk";
      session_id: string;
      offset: number;
      chunk: {
        offset: number;
        size_bytes: number;
        sha256_hex: string;
        data_base64: string;
      };
      resume_token_hash: string;
    }
  | {
      type: "file_transfer_commit";
      session_id: string;
      resume_token_hash: string;
    }
  | {
      type: "file_transfer_abort";
      session_id: string;
      resume_token_hash: string;
    }
  | {
      type: "file_transfer_download_start";
      session_id: string;
      path: string;
      chunk_size_bytes: number;
      rate_limit_kbps: number;
      follow_symlinks: boolean;
      resume_token_hash: string;
    }
  | {
      type: "file_transfer_download_chunk";
      session_id: string;
      offset: number;
      max_bytes: number;
      resume_token_hash: string;
    }
  | { type: "file_stat"; path: string }
  | {
      type: "file_list_dir";
      path: string;
      offset?: number;
      limit?: number;
      show_hidden?: boolean;
    }
  | {
      type: "file_read_text";
      path: string;
      max_bytes?: number;
      follow_symlinks?: boolean;
    }
  | {
      type: "file_write_text";
      path: string;
      mode: number;
      size_bytes: number;
      sha256_hex: string;
      content_base64: string;
      expected_sha256_hex?: string | null;
      create?: boolean;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_mkdir";
      path: string;
      mode: number;
      recursive?: boolean;
      follow_symlinks?: boolean;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_rename";
      path: string;
      new_path: string;
      overwrite?: boolean;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_delete";
      path: string;
      recursive?: boolean;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_chmod";
      path: string;
      mode: number;
      recursive?: boolean;
      follow_symlinks?: boolean;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_chown";
      path: string;
      owner?: string | null;
      group?: string | null;
      uid?: number | null;
      gid?: number | null;
      recursive?: boolean;
      ownership_policy?: FileOwnershipPolicy;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_copy";
      path: string;
      new_path: string;
      overwrite?: boolean;
      recursive?: boolean;
      follow_symlinks?: boolean;
      policy?: FileActionPolicy;
    }
  | {
      type: "file_download";
      path: string;
      max_bytes?: number;
      follow_symlinks?: boolean;
    }
  | {
      type: "file_archive_tar";
      path: string;
      max_bytes?: number;
      follow_symlinks?: boolean;
    }
  | { type: "user_sessions" }
  | { type: "process_list"; limit: number }
  | {
      type: "storage_inventory";
      include_pseudo_mounts: boolean;
      limit: number;
    }
  | {
      type: "process_start";
      name: string;
      argv: string[];
      cwd: string | null;
      env: Record<string, string>;
      policy?: {
        restart?: "never" | "on_failure" | "always";
        restart_max_retries?: number;
        restart_backoff_secs?: number;
        graceful_stop_secs?: number;
      };
      limits?: {
        memory_max_bytes?: number;
        pids_max?: number;
        open_files_max?: number;
        cpu_shares?: number;
        no_new_privileges?: boolean;
      };
    }
  | { type: "process_stop"; name: string }
  | { type: "process_restart"; name: string }
  | { type: "process_status"; name: string | null }
  | { type: "process_logs"; name: string; max_bytes: number }
  | {
      type: "service_inventory";
      expected_provider?: HostServiceProvider | null;
      limit: number;
    }
  | {
      type: "service_action";
      provider: HostServiceProvider;
      service: string;
      action: HostServiceAction;
      expected_active_state: string;
      expected_enabled_state: string;
    }
  | {
      type: "service_logs";
      provider: HostServiceProvider;
      service: string;
      max_lines: number;
    }
  | {
      type: "package_update_plan";
      expected_provider?: HostPackageProvider | null;
      refresh_metadata: boolean;
    }
  | {
      type: "package_update_apply";
      provider: HostPackageProvider;
      plan_hash: string;
    }
  | {
      type: "backup";
      paths: string[];
      include_config: boolean;
      follow_symlinks: boolean;
      missing_path_policy: BackupMissingPathPolicy;
    }
  | {
      type: "network_status";
      plan_id: string;
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      runtime_adapter?: RuntimeTunnelAdapterCommands | null;
    }
  | { type: "network_interfaces" }
  | {
      type: "network_traffic_import_vnstat";
      interfaces: string[];
      start_unix: number;
    }
  | {
      type: "network_probe";
      plan_id: string;
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      count: number;
      interval_ms: number;
    }
  | {
      type: "network_speed_test";
      plan_id: string;
      plan: TunnelPlan;
      server_side: TunnelEndpointSide;
      duration_secs: number;
      max_bytes: number;
      rate_limit_kbps: number;
      port: number;
      connect_timeout_ms: number;
    }
  | {
      type: "network_routing_status";
      plan_id: string;
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      adapter: RoutingCostAdapterCommands;
    }
  | {
      type: "network_routing_apply";
      plan_id: string;
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      adapter: RoutingCostAdapterCommands;
      expected_current_cost: number | null;
      desired_cost: number;
    }
  | {
      type: "restore";
      source_backup_request_id: string;
      archive_transfer_session_id: string;
      paths: string[];
      include_config: boolean;
      destination_root: string | null;
      archive_path?: string | null;
      archive_size_bytes?: number | null;
      archive_sha256_hex?: string | null;
      dry_run?: boolean;
      post_restore_argv?: string[];
    }
  | {
      type: "restore_rollback";
      source_restore_job_id: string;
      restored_files: RestoreRollbackFile[];
    };

export type FileActionPolicy = "fail" | "ensure" | "ignore";
export type FileExistingPolicy = "skip" | "replace";
export type FileOwnershipPolicy = "fail" | "ignore";

type AssertNever<T extends never> = T;
type _FrontendOperationTypesMissingFromGenerated = AssertNever<
  Exclude<JobOperation["type"], GeneratedJobOperationType>
>;
type _GeneratedOperationTypesMissingFromFrontend = AssertNever<
  Exclude<GeneratedJobOperationType, JobOperation["type"]>
>;

export type CreateJobRequest = {
  job_id: string;
  selector_expression: string;
  target_client_ids: string[];
  destructive: boolean;
  confirmed: boolean;
  command: string;
  argv: string[];
  operation?: JobOperation;
  max_timeout_secs?: number;
  force_unprivileged?: boolean;
  privileged: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
  rollout?: JobRolloutPolicy | null;
};

export type JobRolloutPolicy = {
  canary_client_ids: string[];
  batch_size: number;
  max_failures: number;
  pause_after_canary: boolean;
  batch_delay_secs: number;
};

export type JobRolloutTargetRecord = {
  client_id: string;
  batch_index: number;
  status: JobTargetStatus;
  message: string | null;
};

export type JobRolloutRecord = {
  job_id: string;
  status: "running" | "paused" | "completed" | "aborted";
  canary_client_ids: string[];
  batch_size: number;
  max_failures: number;
  pause_after_canary: boolean;
  batch_delay_secs: number;
  current_batch: number;
  total_batches: number;
  failure_baseline: number;
  pause_reason: string | null;
  next_batch_at: string;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  targets: JobRolloutTargetRecord[];
};

export type UpdateJobRolloutRequest = {
  confirmed?: boolean;
  reason?: string | null;
};

export type CancelJobResponse = {
  job_id: string;
  status: string | null;
  requested_targets: number;
  pending_canceled: number;
  cancel_acks: Array<{
    client_id: string;
    accepted: boolean;
    acked: boolean;
    applied: boolean;
    message: string;
  }>;
};

export type CreateJobApprovalRequest = {
  approval_id?: string;
  job: CreateJobRequest;
  reason?: string | null;
  risk?: string | null;
};

export type DecideJobApprovalRequest = {
  confirmed: boolean;
  reason?: string | null;
};

type _FrontendCreateJobRequestExtraKeys = AssertNever<
  Exclude<keyof CreateJobRequest, GeneratedCreateJobRequestField>
>;
type _GeneratedCreateJobRequestMissingKeys = AssertNever<
  Exclude<GeneratedCreateJobRequestField, keyof CreateJobRequest>
>;

export type CreateJobResponse = {
  job_id: string;
  target_count: number;
  status: JobStatus;
  max_timeout_secs: number;
  max_job_timeout_secs: number;
  control_deadline_extra_secs: number;
  error?: string | null;
  message?: string | null;
  recovery?: string | null;
  target_counts: {
    total: number;
    queued: number;
    dispatching: number;
    running: number;
    completed: number;
    skipped: number;
    rejected: number;
    failed: number;
    agent_lost: number;
    agent_timeout: number;
    control_timeout: number;
    canceled: number;
  };
};

export type JobApprovalDecisionResponse = {
  approval: JobApprovalRecord;
  job: CreateJobResponse | null;
};

export type CreateScheduleRequest = {
  name: string;
  operation: JobOperation;
  selector_expression: string;
  target_client_ids: string[];
  cron_expr: string;
  timezone: "UTC";
  enabled: boolean;
  catch_up_policy: string;
  catch_up_limit: number;
  retry_delay_secs: number;
  max_failures: number;
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type UpdateScheduleRequest = CreateScheduleRequest & {
  expected_selector_expression: string;
  expected_target_client_ids: string[];
};

export type UpdateScheduleTargetsRequest = {
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type SchedulePrivilegeMutationRequest = {
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type DeferScheduleRequest = {
  deferred_until: string;
  reason?: string | null;
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type BackupPolicyRecord = {
  schedule_id: string;
  name: string;
  enabled: boolean;
  selector_expression: string;
  target_client_ids: string[];
  paths: string[];
  include_config: boolean;
  follow_symlinks: boolean;
  missing_path_policy: BackupMissingPathPolicy;
  retention_days: number;
  keep_last: number;
  rotation_generation: string | null;
  cron_expr: string;
  cadence_error: string | null;
  timezone: "UTC" | string;
  next_runs: string[];
  catch_up_policy: string;
  catch_up_limit: number;
  retry_delay_secs: number;
  max_failures: number;
  failure_count: number;
  last_error: string | null;
  next_run_at: string;
  last_run_at: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateBackupPolicyRequest = {
  name: string;
  selector_expression: string;
  target_client_ids: string[];
  paths: string[];
  include_config: boolean;
  follow_symlinks: boolean;
  missing_path_policy: BackupMissingPathPolicy;
  retention_days?: number | null;
  keep_last?: number | null;
  rotation_generation?: string | null;
  cron_expr: string;
  timezone: "UTC";
  enabled: boolean;
  catch_up_policy: string;
  catch_up_limit: number;
  retry_delay_secs: number;
  max_failures: number;
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type UpdateBackupPolicyRequest = Omit<
  CreateBackupPolicyRequest,
  "retention_days" | "keep_last" | "rotation_generation"
> & {
  retention_days: number;
  keep_last: number;
  rotation_generation: string | null;
  expected_selector_expression: string;
  expected_target_client_ids: string[];
};

export type CreateTunnelPlanRequest = TunnelPlanInput & {
  enabled: boolean;
  confirmed: boolean;
};

export type UpdateTunnelPlanRequest = CreateTunnelPlanRequest & {
  expected_revision: number;
};

export type UpdateTunnelConnectionAssessmentRequest = {
  expected_revision: number;
  assessment: TunnelConnectionAssessment;
  note: string | null;
};

export type TunnelPlanRevisionTarget = {
  plan_id: string;
  expected_revision: number;
};

export type UpdateTunnelPlanOspfCostRequest = {
  plan_revision: number;
  recommendation_id: string;
  left_adapter_definition_hash: string;
  right_adapter_definition_hash: string;
  left_current_ospf_cost: number | null;
  right_current_ospf_cost: number | null;
  desired_ospf_cost: number;
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type TunnelPlanOspfJobsResponse = {
  plan: TunnelPlanRecord;
  jobs: CreateJobResponse[];
  dispatch: TunnelPlanOspfDispatchRecord[];
};

export type TunnelPlanOspfDispatchRecord = {
  endpoint_side: "left" | "right";
  client_id: string;
  job_id: string;
  status: "queued" | "not_queued" | "queue_failed" | string;
  error: string | null;
};

export type AllocateTunnelEndpointsRequest = {
  ipv4_pool_cidr?: string | null;
  ipv6_pool_cidr?: string | null;
  reserved_addresses?: string[];
  include_ipv4?: boolean;
  include_ipv6?: boolean;
};

export type AllocateTunnelEndpointsResponse = {
  ipv4_tunnel: TunnelAddressPair | null;
  ipv6_tunnel: TunnelAddressPair | null;
  latency_primary_family: TunnelAddressFamily;
  conflicts: string[];
};

export type BackupRequestRecord = {
  id: string;
  actor_id: string | null;
  client_id: string;
  paths: string[];
  include_config: boolean;
  follow_symlinks: boolean;
  missing_path_policy: BackupMissingPathPolicy;
  status: BackupRequestStatus;
  payload_hash: string;
  command_scope: string;
  artifact_id: string | null;
  source_job_id: string | null;
  source_schedule_id: string | null;
  note: string | null;
  created_at: string;
};

export type BackupPolicyPruneRequest = {
  schedule_id?: string | null;
  dry_run: boolean;
  metadata_only?: boolean | null;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type BackupPolicyPrunePolicyRecord = {
  schedule_id: string;
  name: string;
  enabled: boolean;
  retention_days: number;
  keep_last: number;
  cutoff_unix: number;
  matched_rows: number;
  pruned_rows: number;
  object_keys: string[];
  object_delete_attempted: boolean;
  object_delete_errors: string[];
  metadata_only: boolean;
  status: string;
};

export type BackupPolicyPruneResponse = {
  dry_run: boolean;
  metadata_only_requested: boolean | null;
  preview_hash: string;
  policies: BackupPolicyPrunePolicyRecord[];
};

export type BackupArtifactRecord = {
  id: string;
  client_id: string;
  object_key: string;
  sha256_hex: string;
  size_bytes: number;
  status: string;
  content_available: boolean;
  created_at: string;
};

export type CreateBackupRequest = {
  client_id: string;
  paths: string[];
  include_config: boolean;
  follow_symlinks: boolean;
  missing_path_policy: BackupMissingPathPolicy;
  confirmed: boolean;
  note: string | null;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type UploadBackupArtifactRequest = {
  object_key: string;
  artifact_base64: string;
  confirmed: boolean;
};

export type BackupArtifactUploadSessionRecord = {
  upload_id: string;
  backup_request_id: string;
  client_id: string;
  object_key: string;
  expected_sha256_hex: string;
  expected_size_bytes: number;
  received_bytes: number;
  next_offset_bytes: number;
  chunk_count: number;
  max_chunk_bytes: number;
  status: string;
  created_unix: number;
  updated_unix: number;
  expires_unix: number;
};

export type BackupArtifactHandoffRequest = {
  confirmed: boolean;
  job_id: string | null;
};

export type BackupArtifactHandoffRecord = {
  artifact: BackupArtifactRecord;
  source_job_id: string;
  source_chunk_count: number;
  source: string;
};

export type RestorePlanRecord = {
  id: string;
  actor_id: string | null;
  source_backup_request_id: string;
  source_client_id: string;
  target_client_id: string;
  paths: string[];
  include_config: boolean;
  destination_root: string | null;
  status: RestorePlanStatus;
  payload_hash: string;
  command_scope: string;
  note: string | null;
  created_at: string;
};

export type MigrationLinkRecord = {
  id: string;
  actor_id: string | null;
  restore_plan_id: string;
  source_backup_request_id: string;
  source_client_id: string;
  target_client_id: string;
  paths: string[];
  include_config: boolean;
  destination_root: string | null;
  status: MigrationLinkStatus;
  note: string | null;
  created_at: string;
};

export type CreateRestorePlanRequest = {
  source_backup_request_id: string;
  target_client_id: string;
  paths: string[];
  include_config: boolean;
  destination_root: string | null;
  confirmed: boolean;
  note: string | null;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type CreateMigrationLinkRequest = {
  restore_plan_id: string;
  confirmed: boolean;
  note: string | null;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type CreateMigrationRunRequest = {
  link: CreateMigrationLinkRequest;
  job: CreateJobRequest;
};

export type CreateMigrationRunResponse = {
  migration_link: MigrationLinkRecord;
  restore_job: CreateJobResponse;
};

export type JobTargetSelection = {
  selector_expression: string;
};

export type JsonValue =
  JsonValue[] | boolean | null | number | string | { [key: string]: JsonValue };

export type AuditLogRecord = {
  id: string;
  actor_id: string | null;
  action: string;
  target: string;
  command_hash: string | null;
  metadata: JsonValue;
  created_at: string;
};

export type HistoryRetentionPolicyRecord = {
  domain: string;
  retention_days: number;
  prune_limit: number;
  enabled: boolean;
  metadata_only: boolean;
  export_enabled: boolean;
  notes: string | null;
  updated_by: string | null;
  updated_at: string;
  built_in_default: boolean;
};

export type HistoryRetentionPolicyRequest = {
  domain: string;
  retention_days?: number | null;
  prune_limit?: number | null;
  enabled?: boolean | null;
  metadata_only?: boolean | null;
  export_enabled?: boolean | null;
  notes?: string | null;
  clear_notes?: boolean;
  confirmed: boolean;
};

export type HistoryRetentionPruneRequest = {
  domain?: string | null;
  dry_run?: boolean;
  metadata_only?: boolean | null;
  preview_hash?: string | null;
  confirmed: boolean;
};

export type HistoryRetentionPruneDomainRecord = {
  domain: string;
  enabled: boolean;
  retention_days: number;
  cutoff_unix: number;
  matched_rows: number;
  pruned_rows: number;
  object_keys: string[];
  object_delete_attempted: boolean;
  object_delete_errors: string[];
  metadata_only: boolean;
  status: string;
};

export type HistoryRetentionPruneResponse = {
  dry_run: boolean;
  metadata_only_requested: boolean | null;
  preview_hash: string;
  domains: HistoryRetentionPruneDomainRecord[];
};

export type HistoryExportRecord = {
  generated_at: string;
  limit: number;
  domains: string[];
  data: JsonValue;
};

export type TagView = {
  name: string;
  display_order: number;
  clients: AgentView[];
};

export type BulkTagMutationRequest = {
  action: "add" | "remove";
  tag: string;
  selector_expression: string;
  target_client_ids: string[];
  confirmed: boolean;
  preview_hash?: string | null;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type TagMutationResponse = {
  tag: string;
  action: string;
  preview_hash: string;
  target_count: number;
  changed_count: number;
  skipped_count: number;
  affected: AgentView[];
  schedule_impacts: ScheduleImpactRecord[];
  confirmation_required: boolean;
};

export type ScheduleImpactRecord = {
  schedule_id: string;
  name: string;
  command_type: string;
  selector_expression: string;
  before_target_count: number;
  after_target_count: number;
  added_target_count: number;
  removed_target_count: number;
  unchanged_target_count: number;
  added_targets: AgentView[];
  removed_targets: AgentView[];
  summary: string;
};

export type ConfigurationBehavior =
  | "host_metrics"
  | "latency_probe"
  | "ospf_update_command"
  | "process_inventory"
  | "user_sessions"
  | "command_execution";

export type NetworkAdapterKind = "runtime_tunnel" | "routing_cost";

export type NetworkAdapterDefinitionRecord = {
  id: string;
  adapter_kind: NetworkAdapterKind;
  name: string;
  description: string | null;
  definition: JsonValue;
  created_at: string;
  updated_at: string;
};

export type UpsertNetworkAdapterDefinitionRequest = {
  adapter_kind: NetworkAdapterKind;
  name: string;
  description?: string | null;
  definition: JsonValue;
};

export type ConfigurationPresetRecord = {
  id: string;
  behavior: ConfigurationBehavior;
  name: string;
  kind: "system" | "custom";
  is_default: boolean;
  description: string | null;
  definition: JsonValue;
  effective_vps_count: number;
  override_vps_count: number;
  created_at: string;
  updated_at: string;
};

export type ConfigurationSourceSyncState =
  "applied" | "queued" | "failed" | "stale" | "unknown";

export type ConfigurationSourceView = {
  client_id: string;
  behavior: ConfigurationBehavior;
  effective_preset_id: string;
  effective_preset_name: string;
  effective_preset_kind: "system" | "custom";
  selection_origin: "system_default" | "explicit_override";
  override_updated_at: string | null;
  runtime_sync: {
    state: ConfigurationSourceSyncState;
    reason: string;
  };
  readiness: {
    state: string;
    reason: string;
    evidence: JsonValue;
  };
};

export type CreateConfigurationPresetRequest = {
  behavior: ConfigurationBehavior;
  name: string;
  description?: string | null;
  definition: JsonValue;
};

export type CloneConfigurationPresetRequest = {
  name: string;
  description?: string | null;
};

export type PreviewConfigurationPresetRequest = {
  description?: string | null;
  definition: JsonValue;
};

export type ConfigurationPresetPreview = {
  preset_id: string;
  behavior: ConfigurationBehavior;
  name: string;
  current_description: string | null;
  candidate_description: string | null;
  current_definition: JsonValue;
  candidate_definition: JsonValue;
  changed_keys: string[];
  affected_client_ids: string[];
  affected_client_count: number;
  sections: JsonValue;
  toml: string;
  preview_hash: string;
};

export type UpdateConfigurationPresetRequest = {
  description?: string | null;
  definition: JsonValue;
  preview_hash: string;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type UpdateConfigurationPresetResponse = {
  preset: ConfigurationPresetRecord;
  preview: ConfigurationPresetPreview;
  sync: RuntimeConfigDispatchRecord[];
};

export type ConfigurationSourceOverrideAction = "set" | "reset";

export type ConfigurationSourceOverrideRequest = {
  action: ConfigurationSourceOverrideAction;
  behavior: ConfigurationBehavior;
  preset_id?: string | null;
  selector_expression: string;
  target_client_ids: string[];
};

export type ConfigurationSourceOverridePreview = {
  action: ConfigurationSourceOverrideAction;
  behavior: ConfigurationBehavior;
  preset: ConfigurationPresetRecord | null;
  selector_expression: string;
  target_count: number;
  targets: Array<{
    client_id: string;
    before_preset_id: string;
    before_preset_name: string;
    before_origin: "system_default" | "explicit_override";
    after_preset_id: string;
    after_preset_name: string;
    after_origin: "system_default" | "explicit_override";
  }>;
  preview_hash: string;
};

export type ApplyConfigurationSourceOverrideRequest =
  ConfigurationSourceOverrideRequest & {
    preview_hash: string;
    privilege_assertion: PrivilegeAssertion;
  };

export type ApplyConfigurationSourceOverrideResponse =
  ConfigurationSourceOverridePreview & {
    sync: RuntimeConfigDispatchRecord[];
  };

export type EffectiveAgentConfigResponse = {
  client_id: string;
  sections: JsonValue;
  toml: string;
  sources: ConfigurationSourceView[];
  generated_at: string;
};

export type RuntimeConfigPatchRequest = {
  selector_expression: string;
  target_client_ids: string[];
  toml: string;
  reason?: string | null;
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type RuntimeConfigPatchResponse = {
  target_count: number;
  overrides: Array<{
    client_id: string;
    toml: string;
    reason: string;
    updated_at: string;
    updated_by: string | null;
  }>;
  sync_job_ids: string[];
  sync: RuntimeConfigDispatchRecord[];
};

export type RuntimeConfigApplyStateRecord = {
  client_id: string;
  applied_version?: number | null;
  applied_content_hash?: string | null;
  applied_job_id?: string | null;
  applied_at?: string | null;
  pending_version?: number | null;
  pending_content_hash?: string | null;
  pending_job_id?: string | null;
  pending_reason?: string | null;
  pending_status?: "queued" | "failed" | string | null;
  pending_error?: string | null;
  pending_updated_at?: string | null;
  updated_at: string;
};

export type RuntimeConfigPatchGeneratorRecord = {
  id: string;
  name: string;
  category: string;
  domain: string;
  description: string;
  field_schema: JsonValue;
  raw_generator_body: string;
  docs_metadata: JsonValue;
  built_in: boolean;
  actor_id: string | null;
  created_at: string;
  updated_at: string;
};

export type UpsertRuntimeConfigPatchGeneratorRequest = {
  id?: string | null;
  name: string;
  category: string;
  domain: string;
  description: string;
  field_schema: JsonValue;
  raw_generator_body: string;
  docs_metadata: JsonValue;
  confirmed: boolean;
};

export type DeleteRuntimeConfigPatchGeneratorRequest = {
  confirmed: boolean;
  reviewed_name: string;
};

export type RuntimeConfigPatchGeneratorRenderRequest = {
  values: JsonValue;
};

export type RuntimeConfigPatchGeneratorRenderResponse = {
  generator_id: string;
  name: string;
  toml: string;
  patch: JsonValue;
  affected_sections: string[];
  docs_metadata: JsonValue;
  generated_at: string;
};

export type BulkResolveResponse = {
  targets: AgentView[];
  target_count: number;
};

export type SuiteConfigValidationRecord = {
  valid: boolean;
  version: number;
  restart_required_fields: string[];
  hot_reload_fields: string[];
};

export type SuiteConfigResponse = {
  path: string;
  exists: boolean;
  effective_require_registered_agent_updates: boolean;
  toml: string;
  redacted: JsonValue;
  validation: SuiteConfigValidationRecord;
  hot_reload_note: string;
  restart_required_note: string;
};

export type SuiteConfigValidateResponse = {
  path: string;
  exists: boolean;
  changed_keys: string[];
  redacted: JsonValue;
  old_redacted: JsonValue;
  validation: SuiteConfigValidationRecord;
};

export type SuiteConfigUpdateResponse = {
  path: string;
  changed_keys: string[];
  validation: SuiteConfigValidationRecord;
  audit_status: string;
};

export type ActiveView =
  | "Home"
  | "Fleet"
  | "Remote Operations"
  | "Jobs"
  | "Automation"
  | "Network"
  | "Backups"
  | "Config"
  | "Observability"
  | "Audit"
  | "Access"
  | "System";
