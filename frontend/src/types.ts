import type {
  GeneratedCreateJobRequestField,
  GeneratedAgentUpdateReleaseStatus,
  GeneratedBackupRequestStatus,
  GeneratedDataSourceReadinessStatus,
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
  GeneratedTopologyDriftAction,
  GeneratedTopologyDriftPolicy,
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
} from "./generated/protocolContracts";

export type JobStatus = GeneratedJobStatus;
export type JobTargetStatus = GeneratedJobTargetStatus;
export type JobCommandType = GeneratedJobCommandType;
export type AgentUpdateReleaseStatus = GeneratedAgentUpdateReleaseStatus;
export type BackupRequestStatus = GeneratedBackupRequestStatus;
export type DataSourceReadinessStatus = GeneratedDataSourceReadinessStatus;
export type FleetAlertNotificationDeliveryProcessStatus =
  GeneratedFleetAlertNotificationDeliveryProcessStatus;
export type FleetAlertNotificationDeliveryStatus = GeneratedFleetAlertNotificationDeliveryStatus;
export type MigrationLinkStatus = GeneratedMigrationLinkStatus;
export type RestorePlanStatus = GeneratedRestorePlanStatus;
export type ServerJobStatus = GeneratedServerJobStatus;
export type ServerJobType = GeneratedServerJobType;
export type TopologyDriftAction = GeneratedTopologyDriftAction;
export type TopologyDriftPolicy = GeneratedTopologyDriftPolicy;
export type TopologyEdgeHealthStatus = GeneratedTopologyEdgeHealthStatus;
export type TopologyNeighborState = GeneratedTopologyNeighborState;
export type TopologyNodeStatus = GeneratedTopologyNodeStatus;
export type TopologyObservationState = GeneratedTopologyObservationState;
export type TopologyProbeState = GeneratedTopologyProbeState;
export type TopologyRuntimeState = GeneratedTopologyRuntimeState;
export type TunnelEndpointStatus = GeneratedTunnelEndpointStatus;
export type TunnelPlanStatus = GeneratedTunnelPlanStatus;
export type WebhookRuleDeliveryHistoryStatus = GeneratedWebhookRuleDeliveryHistoryStatus;
export type WebhookRuleDeliveryProcessStatus = GeneratedWebhookRuleDeliveryProcessStatus;
export type WebhookRuleDeliveryStatus = GeneratedWebhookRuleDeliveryStatus;

export type FleetSummary = {
  total: number;
  online: number;
  offline: number;
  never: number;
  stale: number;
  warnings: number;
  running_jobs: number;
};

export type DashboardWindow =
  | "15m"
  | "1h"
  | "6h"
  | "24h"
  | "7d"
  | "14d"
  | "30d"
  | "all";

export type DashboardGroupBy =
  | "labels"
  | "tags"
  | "countries"
  | "providers"
  | "clients"
  | "status"
  | "date";

export type DashboardScopeKind =
  | "all"
  | "tag"
  | "country"
  | "provider"
  | "client";

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
  view: string;
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
  worker_schedule_command_secs: number | null;
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
  stale: number;
  warnings: number;
  running_jobs: number;
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
  stale: number;
  warnings: number;
  running_jobs: number;
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
  status: FleetAlertNotificationDeliveryStatus;
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

export type FleetAlertPolicyRecord = {
  id: string;
  name: string;
  scope_kind: "global" | "provider" | "tag" | "client" | string;
  scope_value: string | null;
  memory_available_warning_ratio: number | null;
  memory_available_critical_ratio: number | null;
  disk_available_warning_ratio: number | null;
  disk_available_critical_ratio: number | null;
  cpu_load_warning: number | null;
  cpu_load_critical: number | null;
  priority: number;
  enabled: boolean;
  notes: string | null;
  actor_id: string | null;
  created_at: string;
  updated_at: string;
};

export type FleetAlertPolicyRequest = {
  id?: string;
  name: string;
  scope_kind: string;
  scope_value?: string | null;
  memory_available_warning_ratio?: number | null;
  memory_available_critical_ratio?: number | null;
  disk_available_warning_ratio?: number | null;
  disk_available_critical_ratio?: number | null;
  cpu_load_warning?: number | null;
  cpu_load_critical?: number | null;
  priority?: number;
  enabled?: boolean;
  notes?: string | null;
  confirmed: boolean;
};

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
  last_attempt_at: string | null;
  cooldown_until_unix: number;
  actor_id: string | null;
  created_at: string;
  delivered_at: string | null;
};

export type FleetAlertNotificationDispatchRequest = {
  limit?: number;
  client_id?: string | null;
  severity?: string | null;
  category?: string | null;
  operator_state?: string | null;
  include_muted?: boolean;
  dry_run?: boolean;
  confirmed: boolean;
};

export type FleetAlertNotificationProcessRequest = {
  limit?: number;
  status?: FleetAlertNotificationDeliveryProcessStatus | null;
  delivery_kind?: string | null;
  dry_run?: boolean;
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
  event_kind?: string;
  event_id?: string | null;
  limit?: number;
  dry_run?: boolean;
  confirmed: boolean;
};

export type WebhookRuleProcessRequest = {
  limit?: number;
  status?: WebhookRuleDeliveryProcessStatus | null;
  dry_run?: boolean;
  confirmed: boolean;
};

export type WebhookDeliveryRotationRequest = {
  older_than?: string | null;
  older_than_days?: number | null;
  status?: WebhookRuleDeliveryHistoryStatus | null;
  rule_id?: string | null;
  confirmed: boolean;
};

export type WebhookDeliveryRotationResponse = {
  matched_count: number;
  deleted_count: number;
  confirmation_required: boolean;
  older_than: string | null;
  status: string | null;
  rule_id: string | null;
};

export type AgentView = {
  id: string;
  display_name: string;
  status: string;
  tags: string[];
  registration_ip?: string | null;
  last_ip?: string | null;
  last_seen_at?: string | null;
  internal_build_number?: number;
  process_incarnation_id?: string | null;
  stale_since?: string | null;
  stale_reason?: string | null;
  capabilities: AgentCapabilitySnapshot;
};

export type DeleteAgentRequest = {
  confirmed: boolean;
  reason?: string | null;
};

export type DeleteAgentResponse = {
  client_id: string;
  deleted: boolean;
  deleted_at: string;
};

export type AgentCapabilitySnapshot = {
  privilege_mode: "unknown" | "root" | "unprivileged";
  effective_uid?: number | null;
  command_timeout_secs: number;
  can_attempt_privileged_ops: boolean;
  can_manage_runtime_tunnels: boolean;
  can_apply_process_limits: boolean;
  unprivileged_hint?: string | null;
};

export type GatewaySessionRecord = {
  id: string;
  gateway_id: string;
  client_id: string;
  status: string;
  noise_public_key_hex: string | null;
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
  cpu_load_1_avg: number;
  cpu_load_1_max: number;
  memory_total_bytes_max: number;
  memory_available_bytes_avg: number;
  memory_available_bytes_min: number;
  disk_total_bytes_max: number;
  disk_available_bytes_avg: number;
  disk_available_bytes_min: number;
  network_rx_bytes_max: number;
  network_tx_bytes_max: number;
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

export type TelemetryTunnelRecord = {
  client_id: string;
  observed_at: string;
  interface: string;
  kind: string;
  ownership_mode: string;
  mutation_policy: string;
  promotion_required: boolean;
  plan_correlation: string;
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
  auto_ospf_enabled?: boolean | null;
  auto_ospf_status?: string | null;
  auto_ospf_reason?: string | null;
  auto_ospf_current_cost?: number | null;
  auto_ospf_recommended_cost?: number | null;
  auto_ospf_updated_unix?: number | null;
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
      seq: number;
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
export type WsTerminalOutputEvent = Extract<
  WsEvent,
  { type: "terminal_output_recorded" }
>;

export type OperatorView = {
  id: string;
  username: string;
  role: string;
  scopes: string[];
  preferences: OperatorPreferences;
  totp_enabled: boolean;
};

export type OperatorPreferences = {
  vps_name_display_mode: "name" | "name_id_suffix";
  timezone: string | null;
  language: "en";
  show_country_flags: boolean;
  fleet_tag_visibility_overrides: Record<string, boolean>;
  sidebar_subpanel_default: "active" | "all";
  dashboard_curve_exclusions: string[];
  dashboard_resource_top_limit: number;
  dashboard_network_top_limit: number;
  bulk_output_compare_mode: JobOutputCompareMode;
  gateway_server_public_key_hex: string | null;
  gateway_endpoints: string;
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

export type AuthResponse = {
  token_type: "Bearer";
  access_token: string;
  refresh_token: string;
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
  privileged: boolean;
  status: JobStatus;
  target_count: number;
  payload_hash: string;
  created_at: string;
  completed_at: string | null;
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
  preview_hash: string;
  matched_count: number;
  matched_bytes: number;
};

export type CommandTemplateRecord = {
  id: string;
  name: string;
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
  operation: JobOperation;
  selector_expression: string;
  target_client_ids: string[];
  cron_expr: string;
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
export type BandwidthTier = "10m" | "100m" | "1000m";
export type TunnelEndpointSide = "left" | "right";
export type TunnelConfigBackend = "ifupdown" | "netplan" | "systemd_networkd";
export type RuntimeTunnelManager =
  | "agent_iproute2_managed"
  | "external_observed"
  | "external_managed_adapter";

export type RuntimeTunnelCommand = {
  argv: string[];
  timeout_secs?: number;
  max_output_bytes?: number;
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

export type RuntimeTunnelControl = {
  manager: RuntimeTunnelManager;
  startup?: RuntimeTunnelCommand | null;
  stop?: RuntimeTunnelCommand | null;
  cleanup?: RuntimeTunnelCommand | null;
  restart?: RuntimeTunnelCommand | null;
  status?: RuntimeTunnelCommand | null;
  traffic_limit_apply?: RuntimeTunnelCommand | null;
  traffic_limit?: RuntimeTunnelTrafficLimit;
  fou?: RuntimeTunnelFouOptions;
};

export type TunnelPlanInput = {
  name: string;
  interface_name: string;
  kind: TunnelKind;
  runtime_control?: RuntimeTunnelControl;
  runtime_topology?: RuntimeTunnelTopologyIntent;
  left_client_id: string;
  right_client_id: string;
  left_underlay: string;
  right_underlay: string;
  address_pool_cidr: string;
  reserved_addresses: string[];
  ipv4_tunnel?: TunnelAddressPair | null;
  ipv6_address_pool_cidr?: string | null;
  ipv6_tunnel?: TunnelAddressPair | null;
  latency_primary_family?: TunnelAddressFamily;
  bandwidth: BandwidthTier;
  latency_ms: number;
  packet_loss_ratio: number;
  preference: number;
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
  recommended_ospf_cost: number;
  ifupdown_file: string;
  bird2_file: string;
  ifupdown_snippet: string;
  bird2_interface_snippet: string;
  touched_files: string[];
  validation_steps: string[];
  rollback_notes: string[];
  conflicts: string[];
  mutates_host: boolean;
};

export type TunnelPlanRecord = {
  id: string;
  name: string;
  kind: TunnelKind;
  enabled: boolean;
  left_client_id: string;
  right_client_id: string;
  left_status: TunnelEndpointStatus;
  right_status: TunnelEndpointStatus;
  recommended_ospf_cost: number;
  status: TunnelPlanStatus;
  last_apply_job_id: string | null;
  last_rollback_job_id: string | null;
  input: TunnelPlanInput;
  plan: TunnelPlan;
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  deleted_by: string | null;
  deleted_reason: string | null;
};

export type PromoteTelemetryTunnelRequest = {
  client_id: string;
  interface: string;
  peer_client_id: string;
  local_underlay: string;
  peer_underlay: string;
  address_pool_cidr: string;
  ipv4_tunnel?: TunnelAddressPair | null;
  ipv6_address_pool_cidr?: string | null;
  ipv6_tunnel?: TunnelAddressPair | null;
  latency_primary_family?: TunnelAddressFamily;
  side?: TunnelEndpointSide;
  name?: string | null;
  topology_version?: string | null;
  bandwidth?: BandwidthTier | null;
  latency_ms?: number | null;
  packet_loss_ratio?: number | null;
  preference?: number | null;
};

export type TopologyGraphNode = {
  client_id: string;
  display_name: string;
  status: TopologyNodeStatus;
  tags: string[];
  tunnel_count: number;
  applied_tunnel_count: number;
  degraded_tunnel_count: number;
  latest_observed_at: string | null;
};

export type TopologyGraphEdge = {
  plan_id: string;
  plan_name: string;
  interface_name: string;
  kind: TunnelKind;
  left_client_id: string;
  right_client_id: string;
  left_status: TunnelEndpointStatus;
  right_status: TunnelEndpointStatus;
  status: TunnelPlanStatus;
  enabled: boolean;
  health: TopologyEdgeHealthStatus;
  convergence_blocked: boolean;
  offline_client_ids: string[];
  server_drift_reasons: string[];
  topology_drift_policy: TopologyDriftPolicy;
  topology_drift_action: TopologyDriftAction;
  neighbor_state: TopologyNeighborState;
  probe_state: TopologyObservationState;
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
  import_candidate_count: number;
  bandwidth: BandwidthTier;
  recommended_ospf_cost: number;
  cost_delta: number | null;
  latency_avg_ms: number | null;
  latency_series_ms: number[];
  packet_loss_avg_ratio: number | null;
  throughput_avg_mbps: number | null;
  throughput_max_mbps: number | null;
  sample_count: number;
  degraded_count: number;
  latest_observed_at: string | null;
  last_apply_job_id: string | null;
  last_rollback_job_id: string | null;
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
};

export type JobTargetRecord = {
  job_id: string;
  client_id: string;
  status: JobTargetStatus;
  message?: string | null;
  exit_code: number | null;
  started_at: string | null;
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
  job_id: string;
  client_id: string;
  seq: number;
  kind: string;
  role: string | null;
  plan_name: string | null;
  interface_name: string | null;
  peer_client_id: string | null;
  target: string | null;
  healthy: boolean | null;
  latency_avg_ms: number | null;
  packet_loss_ratio: number | null;
  throughput_mbps: number | null;
  bytes: number | null;
  metadata: JsonValue;
  observed_at: string;
};

export type NetworkObservationTrendRecord = {
  kind: string;
  plan_name: string | null;
  interface_name: string | null;
  client_id: string;
  peer_client_id: string | null;
  sample_count: number;
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
  plan_id: string;
  plan_name: string;
  interface_name: string;
  left_client_id: string;
  right_client_id: string;
  configured_bandwidth: BandwidthTier;
  effective_bandwidth: BandwidthTier;
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
};

export type NetworkOspfUpdateEvidenceRecord = {
  configured_bandwidth: BandwidthTier;
  effective_bandwidth: BandwidthTier;
  latency_avg_ms: number | null;
  packet_loss_avg_ratio: number | null;
  throughput_avg_mbps: number | null;
  throughput_max_mbps: number | null;
  sample_count: number;
  degraded_count: number;
  latest_observed_at: string | null;
  reason: string;
};

export type NetworkOspfUpdatePlanRecord = {
  plan_id: string;
  plan_name: string;
  interface_name: string;
  left_client_id: string;
  right_client_id: string;
  bird2_file: string;
  current_ospf_cost: number;
  recommended_ospf_cost: number;
  cost_delta: number;
  status: string;
  confidence: string;
  requires_approval: boolean;
  privilege_required: boolean;
  mutation_mode: string;
  approval_scope: string[];
  evidence: NetworkOspfUpdateEvidenceRecord;
  proposed_left_bird2_interface_snippet: string;
  proposed_right_bird2_interface_snippet: string;
  change_summary: string;
};

export type PrivilegeAssertion = {
  nonce_hex: string;
  issued_unix: number;
  expires_unix: number;
  assertion_hex: string;
};


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
  | {
      type: "terminal_input";
      session_id: string;
      input_seq: number;
      data_base64: string;
    }
  | { type: "terminal_poll"; session_id: string; replay_from_seq?: number }
  | { type: "terminal_resize"; session_id: string; cols: number; rows: number }
  | { type: "terminal_close"; session_id: string; reason?: string }
  | { type: "file_pull"; path: string }
  | { type: "config_read" }
  | {
      type: "hot_config";
      apply_mode: "full_override";
      toml: string;
      preserve_redacted?: boolean | null;
      base_config_sha256_hex?: string | null;
    }
  | { type: "data_source_config_patch"; apply_mode: "incremental_patch"; toml: string }
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
  | { type: "file_read_text"; path: string; max_bytes?: number; follow_symlinks?: boolean }
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
  | { type: "file_download"; path: string; max_bytes?: number; follow_symlinks?: boolean }
  | { type: "file_archive_tar"; path: string; max_bytes?: number; follow_symlinks?: boolean }
  | { type: "user_sessions" }
  | { type: "process_list"; limit: number }
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
      type: "backup";
      paths: string[];
      include_config: boolean;
      recipient_public_key_hex?: string | null;
    }
  | {
      type: "network_apply";
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      config_backend: TunnelConfigBackend;
      config_sha256_hex: string;
      ifupdown_sha256_hex: string;
      bird2_sha256_hex: string;
    }
  | {
      type: "network_ospf_cost_update";
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      current_ospf_cost: number;
      recommended_ospf_cost: number;
      bird2_sha256_hex: string;
    }
  | {
      type: "network_rollback";
      plan: TunnelPlan;
      side: TunnelEndpointSide;
    }
  | {
      type: "network_status";
      plan: TunnelPlan;
      side: TunnelEndpointSide;
    }
  | { type: "network_interfaces" }
  | {
      type: "network_probe";
      plan: TunnelPlan;
      side: TunnelEndpointSide;
      count: number;
      interval_ms: number;
    }
  | {
      type: "network_speed_test";
      plan: TunnelPlan;
      server_side: TunnelEndpointSide;
      duration_secs: number;
      max_bytes: number;
      rate_limit_kbps: number;
      port: number;
      connect_timeout_ms: number;
    }
  | {
      type: "restore";
      source_backup_request_id: string;
      paths: string[];
      include_config: boolean;
      destination_root: string | null;
      archive_base64?: string | null;
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
type _FrontendOperationTypesMissingFromGenerated = AssertNever<Exclude<JobOperation["type"], GeneratedJobOperationType>>;
type _GeneratedOperationTypesMissingFromFrontend = AssertNever<Exclude<GeneratedJobOperationType, JobOperation["type"]>>;

export type CreateJobRequest = {
  job_id?: string;
  selector_expression: string;
  target_client_ids: string[];
  destructive: boolean;
  confirmed: boolean;
  command: string;
  argv: string[];
  operation?: JobOperation;
  timeout_secs: number;
  force_unprivileged?: boolean;
  privileged: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

type _FrontendCreateJobRequestExtraKeys = AssertNever<Exclude<keyof CreateJobRequest, GeneratedCreateJobRequestField>>;
type _GeneratedCreateJobRequestMissingKeys = AssertNever<Exclude<GeneratedCreateJobRequestField, keyof CreateJobRequest>>;

export type CreateJobResponse = {
  job_id: string;
  target_count: number;
  status: JobStatus;
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
  privilege_assertion?: PrivilegeAssertion | null;
};

export type UpdateScheduleRequest = CreateScheduleRequest;

export type UpdateScheduleTargetsRequest = {
  selector_expression: string;
  target_client_ids: string[];
  privilege_assertion?: PrivilegeAssertion | null;
};

export type SchedulePrivilegeMutationRequest = {
  privilege_assertion?: PrivilegeAssertion | null;
};

export type DeferScheduleRequest = {
  deferred_until: string;
  reason?: string | null;
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
  recipient_public_key_hex: string | null;
  retention_days: number;
  keep_last: number;
  rotation_generation: string | null;
  cron_expr: string;
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
  recipient_public_key_hex?: string | null;
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
};

export type CreateTunnelPlanRequest = TunnelPlanInput;

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
  policies: BackupPolicyPrunePolicyRecord[];
};

export type BackupArtifactRecord = {
  id: string;
  client_id: string;
  object_key: string;
  sha256_hex: string;
  encrypted: boolean;
  size_bytes: number;
  created_at: string;
};

export type CreateBackupRequest = {
  client_id: string;
  paths: string[];
  include_config: boolean;
  recipient_public_key_hex?: string | null;
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

export type PrepareBackupArtifactRestoreRequest = {
  private_key_hex: string;
  artifact_base64?: string | null;
};

export type PreparedBackupArtifactRestoreRecord = {
  archive_base64: string;
  archive_sha256_hex: string;
  archive_size_bytes: number;
  artifact_client_id: string;
  file_count: number;
  archive_format: string;
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
};

export type JobTargetSelection = {
  selector_expression: string;
};

export type JsonValue =
  | JsonValue[]
  | boolean
  | null
  | number
  | string
  | { [key: string]: JsonValue };

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
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type TagMutationResponse = {
  tag: string;
  action: string;
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

export type DataSourcePresetRecord = {
  id: string;
  domain: string;
  name: string;
  scope: string;
  built_in: boolean;
  is_default: boolean;
  owner_client_id: string | null;
  description: string | null;
  definition: JsonValue;
  assigned_client_count: number;
  created_at: string;
  updated_at: string;
};

export type DataSourcePresetAssignmentRecord = {
  client_id: string;
  domain: string;
  preset_id: string;
  preset_name: string;
  preset_scope: string;
  assigned_at: string;
};

export type DataSourceStatusRecord = {
  client_id: string;
  display_name: string;
  client_status: string;
  domain: string;
  module: string;
  preset_id: string;
  preset_name: string;
  preset_scope: string;
  source_kind: string;
  status: DataSourceReadinessStatus;
  status_reason: string;
  evidence: JsonValue;
  assigned_at: string;
};

export type CreateDataSourcePresetRequest = {
  domain: string;
  name: string;
  scope: string;
  owner_client_id: string | null;
  description: string | null;
  definition: JsonValue;
};

export type CloneDataSourcePresetRequest = {
  name: string;
  scope: string;
  owner_client_id: string | null;
  description: string | null;
};

export type DataSourcePresetDiffRequest = {
  description: string | null;
  definition: JsonValue;
  keep_description?: boolean;
};

export type DataSourcePresetDiffResponse = {
  preset_id: string;
  domain: string;
  preset_name: string;
  current_description: string | null;
  candidate_description: string | null;
  current_definition: JsonValue;
  candidate_definition: JsonValue;
  description_changed: boolean;
  definition_changed: boolean;
  changed_keys: string[];
  affected_client_count: number;
};

export type DataSourcePresetTestRequest = {
  definition: JsonValue;
};

export type DataSourcePresetTestResponse = {
  preset_id: string;
  domain: string;
  preset_name: string;
  affected_client_count: number;
  valid: boolean;
  renderable: boolean;
  error: string | null;
  sections: JsonValue;
  toml: string;
  unsupported_domains: string[];
  render_notes: string[];
  generated_at: string;
};

export type UpdateDataSourcePresetRequest = {
  description: string | null;
  definition: JsonValue;
  confirmed: boolean;
  keep_description?: boolean;
};

export type UpdateDataSourcePresetResponse = {
  preset: DataSourcePresetRecord;
  diff: DataSourcePresetDiffResponse;
  affected_client_count: number;
  confirmation_required: boolean;
};

export type AssignDataSourcePresetRequest = {
  domain: string;
  preset_id: string;
  selector_expression: string;
  confirmed: boolean;
};

export type AssignDataSourcePresetResponse = {
  preset: DataSourcePresetRecord;
  target_count: number;
  confirmation_required: boolean;
  assignments: DataSourcePresetAssignmentRecord[];
};

export type DataSourceHotConfigResponse = {
  client_id: string;
  sections: JsonValue;
  toml: string;
  assignments: DataSourcePresetAssignmentRecord[];
  unsupported_domains: string[];
  render_notes: string[];
  generated_at: string;
};

export type HotConfigRuleTemplateRecord = {
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

export type UpsertHotConfigRuleTemplateRequest = {
  id?: string | null;
  name: string;
  category: string;
  domain: string;
  description: string;
  field_schema: JsonValue;
  raw_generator_body: string;
  docs_metadata: JsonValue;
};

export type HotConfigRuleTemplateRenderRequest = {
  values: JsonValue;
};

export type HotConfigRuleTemplateRenderResponse = {
  template_id: string;
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
};

export type ActiveView =
  | "Dashboard"
  | "Fleet"
  | "Config"
  | "Tags"
  | "Jobs"
  | "Schedules"
  | "Audit"
  | "Topology"
  | "Backups"
  | "Access"
  | "System";
