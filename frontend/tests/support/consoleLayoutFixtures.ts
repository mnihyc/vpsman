import type { Page } from "@playwright/test";
import {
  dataSourceAssignments,
  dataSourcePresets,
} from "./dataSourcePresetFixtures";
import {
  fileTransferSourceArtifacts,
  fileTransfers,
  terminalSessions,
} from "./jobSessionFixtures";
import { installTransferJobApiMock } from "./transferJobMock";
import {
  JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
} from "../../src/generated/protocolContracts";

type FixtureJobOutput = {
  client_id: string;
  created_at?: string;
  data_base64?: string;
  done?: boolean;
  exit_code?: number | null;
  job_id?: string;
  seq?: number;
  stream: string;
};

export { sha256Hex } from "./backupArtifactFixture";

const statusOutput = (value: unknown) =>
  Buffer.from(JSON.stringify(value)).toString("base64");

const systemSeries = (
  metric: string,
  label: string,
  unit: string,
  values: number[],
) => ({
  label,
  metric,
  points: values.map((value, index) => ({
    avg_value: value,
    bucket_start: `2026-06-05T20:${String(20 + index * 10).padStart(2, "0")}:00Z`,
    latest_value: value,
    max_value: value,
    sample_count: 1,
  })),
  unit,
});

const summary = {
  never: 0,
  offline: 1,
  online: 2,
  running_jobs: 3,
  stale: 0,
  total: 3,
  warnings: 1,
};

const dashboardOverview = {
  available_filters: {
    countries: [
      {
        count: 2,
        kind: "country",
        label: "country:US",
        query: "country:US",
        value: "US",
      },
      {
        count: 1,
        kind: "country",
        label: "country:DE",
        query: "country:DE",
        value: "DE",
      },
    ],
    group_by_options: [
      {
        description: "Provider, country, and custom tags together",
        label: "Labels",
        value: "labels",
      },
      {
        description: "Non-provider and non-country tags",
        label: "Custom tags",
        value: "tags",
      },
      {
        description: "country:* tag distribution",
        label: "Countries",
        value: "countries",
      },
      {
        description: "provider:* tag distribution",
        label: "Providers",
        value: "providers",
      },
      {
        description: "One group per VPS in the selected scope",
        label: "VPS clients",
        value: "clients",
      },
      {
        description: "Online, offline, and stale client states",
        label: "Status",
        value: "status",
      },
      {
        description: "Time buckets across the selected range",
        label: "Date buckets",
        value: "date",
      },
    ],
    providers: [
      {
        count: 1,
        kind: "provider",
        label: "provider:alpha",
        query: "provider:alpha",
        value: "alpha",
      },
    ],
    tags: [
      { count: 1, kind: "tag", label: "bgp", query: "tag:bgp", value: "bgp" },
      {
        count: 1,
        kind: "tag",
        label: "bird2",
        query: "tag:bird2",
        value: "bird2",
      },
    ],
    windows: [
      { label: "15 minutes", seconds: 900, value: "15m" },
      { label: "1 hour", seconds: 3600, value: "1h" },
      { label: "6 hours", seconds: 21600, value: "6h" },
      { label: "24 hours", seconds: 86400, value: "24h" },
      { label: "7 days", seconds: 604800, value: "7d" },
      { label: "14 days", seconds: 1209600, value: "14d" },
      { label: "30 days", seconds: 2592000, value: "30d" },
      { label: "All", seconds: 0, value: "all" },
    ],
  },
  drilldowns: [
    {
      label: "Open fleet instances",
      query: null,
      subpage: "instances",
      view: "Fleet",
    },
    {
      label: "Review active alerts",
      query: null,
      subpage: "alerts",
      view: "Fleet",
    },
    {
      label: "Inspect topology evidence",
      query: null,
      subpage: "evidence",
      view: "Topology",
    },
  ],
  generated_at: "2026-06-05T20:44:58Z",
  group_by: "labels",
  label_clusters: [
    {
      online: 1,
      drilldown: {
        label: "Open matching VPS",
        query: "country:US",
        subpage: "instances",
        view: "Fleet",
      },
      kind: "country",
      label: "country:US",
      query: "country:US",
      running_jobs: 1,
      rx_bps: 4200,
      stale: 1,
      total: 2,
      tx_bps: 6400,
      warnings: 2,
    },
    {
      online: 1,
      drilldown: {
        label: "Open matching VPS",
        query: "country:DE",
        subpage: "instances",
        view: "Fleet",
      },
      kind: "country",
      label: "country:DE",
      query: "country:DE",
      running_jobs: 2,
      rx_bps: 8738,
      stale: 0,
      total: 1,
      tx_bps: 17476,
      warnings: 1,
    },
    {
      online: 1,
      drilldown: {
        label: "Open matching VPS",
        query: "provider:alpha",
        subpage: "instances",
        view: "Fleet",
      },
      kind: "provider",
      label: "provider:alpha",
      query: "provider:alpha",
      running_jobs: 1,
      rx_bps: 4200,
      stale: 0,
      total: 1,
      tx_bps: 6400,
      warnings: 1,
    },
    {
      online: 2,
      drilldown: {
        label: "Open matching VPS",
        query: null,
        subpage: "instances",
        view: "Fleet",
      },
      kind: "all",
      label: "All VPS",
      query: null,
      running_jobs: 3,
      rx_bps: 12938,
      stale: 1,
      total: 3,
      tx_bps: 23876,
      warnings: 3,
    },
  ],
  network: {
    points: [
      {
        bucket_start: "2026-06-05T20:15:00Z",
        rx_bps: 5800,
        tx_bps: 7800,
      },
      {
        bucket_start: "2026-06-05T20:25:00Z",
        rx_bps: 9200,
        tx_bps: 14800,
      },
      {
        bucket_start: "2026-06-05T20:35:00Z",
        rx_bps: 12938,
        tx_bps: 23876,
      },
    ],
    rx_bps: 12938,
    traffic_points: [
      {
        bucket_start: "2026-06-05T20:15:00Z",
        rx_bytes: 160_000_000,
        tx_bytes: 280_000_000,
      },
      {
        bucket_start: "2026-06-05T20:25:00Z",
        rx_bytes: 260_000_000,
        tx_bytes: 410_000_000,
      },
      {
        bucket_start: "2026-06-05T20:35:00Z",
        rx_bytes: 348_000_000,
        tx_bytes: 724_000_000,
      },
    ],
    top_clients: [
      {
        client_id: "agent-fra-02",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-fra-02",
          subpage: "instances",
          view: "Fleet",
        },
        interfaces: ["eth0", "tun0"],
        label: "core-fra-02",
        rx_bps: 8738,
        tx_bps: 17476,
      },
      {
        client_id: "agent-sfo-01",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-sfo-01",
          subpage: "instances",
          view: "Fleet",
        },
        interfaces: ["eth0"],
        label: "edge-sfo-01",
        rx_bps: 4200,
        tx_bps: 6400,
      },
    ],
    traffic_series: [
      {
        client_id: "agent-fra-02",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-fra-02",
          subpage: "instances",
          view: "Fleet",
        },
        interfaces: ["eth0", "tun0"],
        label: "core-fra-02",
        points: [
          {
            bucket_start: "2026-06-05T20:15:00Z",
            rx_bytes: 110_000_000,
            tx_bytes: 190_000_000,
          },
          {
            bucket_start: "2026-06-05T20:25:00Z",
            rx_bytes: 180_000_000,
            tx_bytes: 310_000_000,
          },
          {
            bucket_start: "2026-06-05T20:35:00Z",
            rx_bytes: 258_000_000,
            tx_bytes: 524_000_000,
          },
        ],
        rx_bytes: 548_000_000,
        tx_bytes: 1_024_000_000,
      },
      {
        client_id: "agent-sfo-01",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-sfo-01",
          subpage: "instances",
          view: "Fleet",
        },
        interfaces: ["eth0"],
        label: "edge-sfo-01",
        points: [
          {
            bucket_start: "2026-06-05T20:15:00Z",
            rx_bytes: 50_000_000,
            tx_bytes: 90_000_000,
          },
          {
            bucket_start: "2026-06-05T20:25:00Z",
            rx_bytes: 80_000_000,
            tx_bytes: 100_000_000,
          },
          {
            bucket_start: "2026-06-05T20:35:00Z",
            rx_bytes: 90_000_000,
            tx_bytes: 200_000_000,
          },
        ],
        rx_bytes: 220_000_000,
        tx_bytes: 390_000_000,
      },
    ],
    traffic_top_clients: [
      {
        client_id: "agent-fra-02",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-fra-02",
          subpage: "instances",
          view: "Fleet",
        },
        interfaces: ["eth0", "tun0"],
        label: "core-fra-02",
        rx_bytes: 548_000_000,
        tx_bytes: 1_024_000_000,
      },
      {
        client_id: "agent-sfo-01",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-sfo-01",
          subpage: "instances",
          view: "Fleet",
        },
        interfaces: ["eth0"],
        label: "edge-sfo-01",
        rx_bytes: 220_000_000,
        tx_bytes: 390_000_000,
      },
    ],
    tx_bps: 23876,
  },
  operations: {
    active_alerts: 3,
    backup_completed: 1,
    backup_failed: 0,
    backup_pending: 1,
    critical_alerts: 1,
    degraded_agents: [
      {
        client_id: "agent-nyc-03",
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-nyc-03",
          subpage: "instances",
          view: "Fleet",
        },
        label: "backup-nyc-03",
        status: "stale",
        tags: ["country:US"],
      },
    ],
    recent_alerts: [
      {
        category: "network",
        client_id: "agent-fra-02",
        client_label: "core-fra-02",
        drilldown: {
          label: "Open core-fra-02",
          query: "id:agent-fra-02",
          subpage: "alerts",
          view: "Fleet",
        },
        id: "fleet-alert-network-agent-fra-02-tun0",
        observed_at: "2026-06-05T20:35:00Z",
        severity: "critical",
        title: "Tunnel adapter status failed",
      },
      {
        category: "agent_status",
        client_id: "agent-nyc-03",
        client_label: "backup-nyc-03",
        drilldown: {
          label: "Open backup-nyc-03",
          query: "id:agent-nyc-03",
          subpage: "alerts",
          view: "Fleet",
        },
        id: "fleet-alert-agent-agent-nyc-03-stale",
        observed_at: "2026-06-05T20:25:00Z",
        severity: "warning",
        title: "Agent is not online",
      },
    ],
    running_jobs: 3,
    stale_agents: 1,
    warning_alerts: 2,
  },
  resources: {
    cpu_load_avg: 0.74,
    cpu_load_max: 1.91,
    disk_free_ratio: 0.58,
    memory_used_ratio: 0.63,
    sampled_clients: 2,
  },
  resource_curve: {
    excluded_clients: 0,
    metric: "cpu_load",
    sampled_clients: 3,
    series: [
      {
        client_id: "agent-fra-02",
        critical_threshold: 4,
        current: 1.42,
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-fra-02",
          subpage: "instances",
          view: "Fleet",
        },
        label: "core-fra-02",
        peak: 1.91,
        points: [
          { bucket_start: "2026-06-05T20:15:00Z", value: 0.92 },
          { bucket_start: "2026-06-05T20:25:00Z", value: 1.18 },
          { bucket_start: "2026-06-05T20:35:00Z", value: 1.42 },
        ],
        threshold_direction: "above",
        warning_threshold: 2,
      },
      {
        client_id: "agent-sfo-01",
        critical_threshold: 4,
        current: 0.71,
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-sfo-01",
          subpage: "instances",
          view: "Fleet",
        },
        label: "edge-sfo-01",
        peak: 1.08,
        points: [
          { bucket_start: "2026-06-05T20:15:00Z", value: 0.61 },
          { bucket_start: "2026-06-05T20:25:00Z", value: 0.88 },
          { bucket_start: "2026-06-05T20:35:00Z", value: 0.71 },
        ],
        threshold_direction: "above",
        warning_threshold: 2,
      },
      {
        client_id: "agent-nyc-03",
        critical_threshold: 4,
        current: 0.34,
        drilldown: {
          label: "Open VPS details",
          query: "id:agent-nyc-03",
          subpage: "instances",
          view: "Fleet",
        },
        label: "backup-nyc-03",
        peak: 0.65,
        points: [
          { bucket_start: "2026-06-05T20:15:00Z", value: 0.24 },
          { bucket_start: "2026-06-05T20:25:00Z", value: 0.55 },
          { bucket_start: "2026-06-05T20:35:00Z", value: 0.34 },
        ],
        threshold_direction: "above",
        warning_threshold: 2,
      },
    ],
    top_limit: 8,
  },
  scope: {
    kind: "all",
    label: "All VPS",
    matched_clients: 3,
    query: null,
    value: null,
  },
  summary: {
    online: 2,
    running_jobs: 3,
    stale: 1,
    total: 3,
    warnings: 3,
  },
  time_range: {
    end_at: "2026-06-05T20:44:58Z",
    end_unix: 1780692298,
    mode: "window",
    start_at: "2026-06-04T20:44:58Z",
    start_unix: 1780605898,
    window: "24h",
  },
  window: "24h",
};

const systemDashboard = {
  bucket_secs: 60,
  capacity: {
    agent_offline_secs: 300,
    api_db_pool: 32,
    dispatch_ack_secs: 30,
    dispatcher_batch: 128,
    dispatcher_in_flight: 64,
    event_post_secs: 15,
    internal_http_read_secs: 15,
    worker_db_pool: 8,
    worker_schedule_command_secs: 30,
  },
  current: {
    cancellations: {
      acked: 1,
      awaiting_ack: 0,
      requested: 1,
      sent: 1,
    },
    db_pool: {
      idle_connections: 18,
      in_use_connections: 6,
      max_connections: 32,
      open_connections: 24,
    },
    dispatch: {
      active_jobs: 2,
      queued_jobs: 1,
      queue_depth: 4,
      retried_targets: 2,
      running_jobs: 1,
      total_dispatch_attempts: 42,
    },
    gateway_events: {
      active_queues: 3,
      critical_failures: 0,
      current_queue_depth: 0,
      delivered_events: 928,
      critical_failures_by_reason: {
        expired: 0,
        global_queue_full: 0,
        target_queue_full: 0,
      },
      dropped_by_kind: {
        command_output: 0,
        lifecycle: 0,
        other: 0,
        telemetry: 1,
        terminal_output: 0,
      },
      dropped_by_reason: {
        coalesced: 1,
        expired: 0,
        global_queue_full: 0,
        target_queue_full: 0,
      },
      dropped_events: 1,
      expired_events: 0,
      oldest_event_age_secs: null,
      queued_events: 0,
      retained_output_truncated_events: 0,
      rejected_agent_connections: 0,
      retry_attempts: 2,
      status: "live",
      telemetry_dropped_events: 1,
    },
    targets: {
      active: 3,
      agent_timeout_last_24h: 1,
      canceled_last_24h: 1,
      control_timeout_last_24h: 1,
      deadline_expired_active: 0,
      dispatching: 1,
      queued: 1,
      running: 2,
    },
  },
  generated_at: "2026-06-05T20:44:58Z",
  notes: ["50-VPS capacity profile active"],
  series: [
    systemSeries("db_pool.in_use_connections", "DB in-use connections", "connections", [4, 5, 6]),
    systemSeries("db_pool.open_connections", "DB open connections", "connections", [20, 22, 24]),
    systemSeries("db_pool.idle_connections", "DB idle connections", "connections", [16, 17, 18]),
    systemSeries("db_pool.max_connections", "DB max connections", "connections", [32, 32, 32]),
    systemSeries("dispatch.queue_depth", "Dispatch queue depth", "targets", [1, 2, 4]),
    systemSeries("targets.dispatching", "Dispatching targets", "targets", [0, 1, 1]),
    systemSeries("targets.running", "Running targets", "targets", [1, 2, 2]),
    systemSeries("dispatch.retried_targets", "Retried targets", "targets", [0, 1, 2]),
    systemSeries("targets.deadline_expired_active", "Expired active targets", "targets", [0, 0, 0]),
    systemSeries("targets.control_timeout_last_24h", "Control timeouts", "targets", [0, 1, 1]),
    systemSeries("targets.agent_timeout_last_24h", "Agent timeouts", "targets", [0, 0, 1]),
    systemSeries("targets.canceled_last_24h", "Canceled targets", "targets", [0, 1, 1]),
    systemSeries("gateway_events.queued_events", "Gateway queued events", "events", [2, 1, 0]),
    systemSeries("gateway_events.delivered_events", "Gateway delivered events", "events", [900, 918, 928]),
    systemSeries("gateway_events.retry_attempts", "Gateway retry attempts", "attempts", [0, 1, 2]),
    systemSeries("gateway_events.active_queues", "Gateway active queues", "queues", [2, 3, 3]),
    systemSeries("gateway_events.current_queue_depth", "Gateway queue depth", "events", [2, 1, 0]),
    systemSeries("gateway_events.oldest_event_age_secs", "Gateway oldest event age", "seconds", [0, 2, 0]),
    systemSeries("gateway_events.dropped_events", "Gateway dropped events", "events", [0, 0, 1]),
    systemSeries("gateway_events.telemetry_dropped_events", "Gateway telemetry drops", "events", [0, 0, 1]),
    systemSeries("gateway_events.expired_events", "Gateway expired events", "events", [0, 0, 0]),
    systemSeries("gateway_events.critical_failures", "Gateway critical failures", "events", [0, 0, 0]),
    systemSeries("gateway_events.dropped_by_kind.telemetry", "Gateway telemetry drops by kind", "events", [0, 0, 1]),
    systemSeries("gateway_events.dropped_by_reason.coalesced", "Gateway coalesced telemetry", "events", [0, 0, 1]),
    systemSeries("gateway_events.dropped_by_reason.target_queue_full", "Gateway target queue full drops", "events", [0, 0, 0]),
    systemSeries("gateway_events.retained_output_truncated_events", "Gateway retained output truncations", "events", [0, 0, 0]),
    systemSeries("gateway_events.rejected_agent_connections", "Gateway rejected agent connections", "connections", [0, 0, 0]),
    systemSeries("cancellations.requested", "Cancel requested", "targets", [0, 1, 1]),
    systemSeries("cancellations.sent", "Cancel sent", "targets", [0, 1, 1]),
    systemSeries("cancellations.acked", "Cancel acked", "targets", [0, 1, 1]),
    systemSeries("cancellations.awaiting_ack", "Cancel awaiting ack", "targets", [0, 0, 0]),
  ],
  window: "24h",
};

const suiteConfigToml = `version = 1

[api]
bind = "127.0.0.1:8080"
gateway_control_url = "unix:/var/lib/vpsman/gateway-control.sock"
job_output_artifact_min_bytes = 32768
require_registered_agent_updates = false

[gateway]
bind = "0.0.0.0:9443"
control_bind = "unix:/var/lib/vpsman/gateway-control.sock"
api_url = "http://api:8080"
gateway_id = "compose-gateway"
reconnect_grace_secs = 60

[worker]
tick_secs = 30
worker_lease_secs = 60
agent_offline_timeout_secs = 300
schedule_command_timeout_secs = 30

[capacity]
api_db_pool = 32
worker_db_pool = 8
dispatcher_batch = 128
dispatcher_in_flight = 64

[storage]
backup_object_store_dir = "/var/lib/vpsman/objects/backups"

[timeout]
dispatch_ack_secs = 30
event_post_secs = 15
internal_http_read_secs = 15
agent_offline_secs = 300

[secrets]
internal_token_file = "/run/secrets/vpsman_internal_token"
gateway_private_key_file = "/run/secrets/vpsman_gateway_private_key_hex"
privilege_verifier_key_file = "/run/secrets/vpsman_privilege_verifier_key_hex"
`;

const suiteConfigRedacted = {
  api: {
    bind: "127.0.0.1:8080",
    gateway_control_url: "unix:/var/lib/vpsman/gateway-control.sock",
    job_output_artifact_min_bytes: 32768,
    require_registered_agent_updates: false,
  },
  capacity: {
    api_db_pool: 32,
    dispatcher_batch: 128,
    dispatcher_in_flight: 64,
    worker_db_pool: 8,
  },
  gateway: {
    api_url: "http://api:8080",
    bind: "0.0.0.0:9443",
    control_bind: "unix:/var/lib/vpsman/gateway-control.sock",
    gateway_id: "compose-gateway",
    reconnect_grace_secs: 60,
  },
  secrets: {
    gateway_private_key_file: "/run/secrets/vpsman_gateway_private_key_hex",
    internal_token_file: "/run/secrets/vpsman_internal_token",
    privilege_verifier_key_file: "/run/secrets/vpsman_privilege_verifier_key_hex",
  },
  version: 1,
};

const suiteConfigValidation = {
  hot_reload_fields: [
    "capacity.dispatcher_batch",
    "capacity.dispatcher_in_flight",
    "timeout.dispatch_ack_secs",
    "timeout.event_post_secs",
    "timeout.internal_http_read_secs",
    "gateway.reconnect_grace_secs",
    "timeout.gateway_reconnect_grace_secs",
    "api.job_output_artifact_min_bytes",
    "api.require_registered_agent_updates",
    "worker.schedule_command_timeout_secs",
    "worker.tick_secs",
    "worker.worker_lease_secs",
    "worker.agent_offline_timeout_secs",
    "worker.notification_*",
    "worker.webhook_rule_*",
    "worker.backup_policy_prune_*",
    "worker.require_registered_agent_updates",
    "timeout.worker_schedule_command_secs",
    "timeout.agent_offline_secs",
    "api.alert_*",
  ],
  restart_required_fields: [
    "api.bind",
    "api.gateway_control_url",
    "gateway.bind",
    "gateway.control_bind",
    "gateway.api_url",
    "gateway.gateway_id",
    "gateway.expect_client_public_key_hex",
    "database.postgres_url",
    "database.migrations_dir",
    "secrets.*",
    "storage.backup_object_store_dir",
    "storage.object_endpoint",
    "storage.object_bucket",
    "storage.object_region",
    "storage.object_create_bucket",
    "capacity.api_db_pool",
    "capacity.worker_db_pool",
    "worker.once",
    "worker.worker_id",
    "timeout.internal_http_connect_secs",
    "timeout.internal_http_write_secs",
  ],
  valid: true,
  version: 1,
};

const operatorPreferences = {
  bulk_output_compare_mode: "binary",
  dashboard_curve_exclusions: [],
  dashboard_network_top_limit: 8,
  dashboard_resource_top_limit: 8,
  gateway_endpoints: "",
  gateway_server_public_key_hex: null,
  language: "en",
  show_country_flags: true,
  sidebar_subpanel_default: "active",
  timezone: null,
  vps_name_display_mode: "name_id_suffix",
};

const rootCapabilities = {
  can_apply_process_limits: true,
  can_attempt_privileged_ops: true,
  can_manage_runtime_tunnels: true,
  effective_uid: 0,
  privilege_mode: "root",
  unprivileged_hint: null,
};

const unprivilegedCapabilities = {
  can_apply_process_limits: false,
  can_attempt_privileged_ops: true,
  can_manage_runtime_tunnels: false,
  effective_uid: 1000,
  privilege_mode: "unprivileged",
  unprivileged_hint:
    "agent is not running as root; root-only network, update, restore, and limit operations may report ineffective or require forced best-effort mode",
};

const agents = [
  {
    capabilities: rootCapabilities,
    display_name: "edge-sfo-01",
    id: "agent-sfo-01",
    status: "online",
    tags: ["country:US", "provider:alpha", "role:edge"],
  },
  {
    capabilities: rootCapabilities,
    display_name: "core-fra-02",
    id: "agent-fra-02",
    status: "online",
    tags: ["bgp", "bird2", "country:DE"],
  },
  {
    capabilities: unprivilegedCapabilities,
    display_name: "backup-nyc-03",
    id: "agent-nyc-03",
    status: "stale",
    tags: ["country:US"],
  },
];

const fleetAlerts = [
  {
    category: "network",
    client_id: "agent-fra-02",
    detail: "adapter exited",
    evidence: { interface: "tun0" },
    id: "fleet-alert-network-agent-fra-02-tun0",
    observed_at: "2026-05-31T10:02:00Z",
    operator_state: "open",
    severity: "critical",
    muted_until_unix: null,
    escalation_level: 0,
    state_actor_id: null,
    state_reason: null,
    state_updated_at: null,
    status: "tunnel_adapter_degraded",
    target_id: "agent-fra-02:tun0",
    target_kind: "tunnel",
    title: "Tunnel adapter status failed",
  },
  {
    category: "agent_status",
    client_id: "agent-nyc-03",
    detail: "backup-nyc-03 currently reports stale",
    evidence: { privilege_mode: "unprivileged" },
    id: "fleet-alert-agent-agent-nyc-03-stale",
    observed_at: "2026-05-31T10:02:00Z",
    operator_state: "open",
    severity: "warning",
    muted_until_unix: null,
    escalation_level: 0,
    state_actor_id: null,
    state_reason: null,
    state_updated_at: null,
    status: "stale",
    target_id: "agent-nyc-03",
    target_kind: "agent",
    title: "Agent is not online",
  },
  {
    category: "source_readiness",
    client_id: "agent-sfo-01",
    detail:
      "Backup object store: backup object-store preset is selected, but no server object store is configured",
    evidence: { domain: "backup_object_store" },
    id: "fleet-alert-source-agent-sfo-01-backup",
    observed_at: "2026-06-02T10:00:00Z",
    operator_state: "acknowledged",
    severity: "warning",
    muted_until_unix: null,
    escalation_level: 0,
    state_actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    state_reason: "fixture acknowledgement",
    state_updated_at: "2026-06-02T10:00:10Z",
    status: "selected_no_store",
    target_id: "agent-sfo-01:backup_object_store",
    target_kind: "data_source",
    title: "Selected data source needs attention",
  },
];

const fleetAlertStates = [
  {
    action: "acknowledge",
    alert_id: "fleet-alert-source-agent-sfo-01-backup",
    created_at: "2026-06-02T10:00:10Z",
    created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
    expires_at: null,
    id: "fafafafa-1111-4111-8111-111111111111",
    reason: "fixture acknowledgement",
    updated_at: "2026-06-02T10:00:10Z",
  },
];

const fleetAlertPolicies = [
  {
    cpu_load_critical: null,
    cpu_load_warning: null,
    created_at: "2026-06-02T10:00:00Z",
    created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
    disk_available_critical_ratio: null,
    disk_available_warning_ratio: null,
    enabled: true,
    id: "fbfbfbfb-1111-4111-8111-111111111111",
    memory_available_critical_ratio: 0.1,
    memory_available_warning_ratio: 0.2,
    name: "edge-resource-policy",
    priority: 0,
    scope_kind: "tag",
    scope_value: "edge",
    updated_at: "2026-06-02T10:00:00Z",
  },
];

const fleetAlertNotificationChannels = [
  {
    categories: ["agent_status", "network"],
    cooldown_secs: 3600,
    created_at: "2026-06-02T10:00:00Z",
    created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
    delivery_kind: "audit_log",
    enabled: true,
    id: "fcfcfcfc-1111-4111-8111-111111111111",
    min_severity: "warning",
    name: "edge-audit-channel",
    operator_states: ["open", "escalated"],
    scope_kind: "tag",
    scope_value: "edge",
    target: "audit:fleet",
    updated_at: "2026-06-02T10:00:00Z",
  },
];

const fleetAlertNotifications = [
  {
    alert_category: "network",
    alert_id: "fleet-alert-network-agent-fra-02-tun0",
    attempt_count: 1,
    channel_id: "fcfcfcfc-1111-4111-8111-111111111111",
    channel_name: "edge-audit-channel",
    created_at: "2026-06-02T10:01:00Z",
    delivery_kind: "audit_log",
    error: null,
    id: "fdfdfdfd-1111-4111-8111-111111111111",
    last_attempt_at: "2026-06-02T10:01:05Z",
    status: "queued",
    target: "audit:fleet",
    updated_at: "2026-06-02T10:01:05Z",
  },
];

const webhookRules = [
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    body_template:
      "{rule.name} {event.kind} count={matched_vps.length} {matched_vps.0.display_name}",
    cooldown_secs: 300,
    created_at: "2026-06-02T10:00:00Z",
    enabled: true,
    expression: "interval.30sec && tag:edge",
    id: "fefefefe-1111-4111-8111-111111111111",
    name: "edge-interval-webhook",
    notes: "Routes interval checks for edge fleet capacity reviews.",
    target: "https://hooks.example/vpsman/edge-capacity",
    updated_at: "2026-06-02T10:00:00Z",
  },
];

const webhookDeliveries = [
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    attempt_count: 1,
    cooldown_until_unix: 0,
    created_at: "2026-06-02T10:01:00Z",
    dedupe_key: "edge-interval-webhook:interval.30sec:q2-edge-capacity",
    delivered_at: "2026-06-02T10:01:04Z",
    error: null,
    event_id: "q2-edge-capacity",
    event_kind: "interval.30sec",
    id: "abababab-1111-4111-8111-111111111111",
    last_attempt_at: "2026-06-02T10:01:04Z",
    matched_vps: [
      {
        capabilities: rootCapabilities,
        display_name: "edge-sfo-01",
        id: "agent-sfo-01",
        status: "online",
        tags: ["country:US", "provider:alpha"],
      },
    ],
    message: "edge-interval-webhook interval.30sec count=1 edge-sfo-01",
    next_attempt_at: null,
    payload: {
      event_kind: "interval.30sec",
      matched_count: 1,
      rule_name: "edge-interval-webhook",
    },
    rule_id: "fefefefe-1111-4111-8111-111111111111",
    rule_name: "edge-interval-webhook",
    status: "delivered",
    target: "https://hooks.example/vpsman/edge-capacity",
  },
];

const historyRetentionPolicies = [
  {
    built_in_default: true,
    domain: "audit_logs",
    enabled: true,
    export_enabled: true,
    metadata_only: true,
    notes: "fixture audit retention",
    prune_limit: 1000,
    retention_days: 365,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "job_outputs",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture job output retention",
    prune_limit: 500,
    retention_days: 30,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "backup_artifacts",
    enabled: true,
    export_enabled: true,
    metadata_only: true,
    notes: "fixture backup metadata retention",
    prune_limit: 100,
    retention_days: 180,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
];

const tags = [
  {
    clients: [agents[0], agents[1]],
    name: "edge",
  },
];

const processSupervisorInventory = [
  {
    client_id: "agent-sfo-01",
    cgroup_cpu_weight: 39,
    cgroup_memory_current_bytes: 1048576,
    cgroup_pids_current: 2,
    cgroup_process_count: 2,
    cgroup_status: "available",
    last_exit_code: 7,
    last_exit_unix: 1780423260,
    last_restart_unix: 1780423261,
    limit_effectiveness_status: "degraded_desired_only",
    name: "ospf-worker",
    observed_at: "2026-06-02T10:01:30Z",
    pid: 4242,
    process_exit_code: null,
    restart_attempts: 1,
    source_command_type: "process_status",
    source_job_id: "41414141-2222-4333-8444-555555555555",
    started_unix: 1780423261,
    stderr_log: "/var/lib/vpsman/supervisor/logs/ospf-worker.stderr.log",
    stdout_log: "/var/lib/vpsman/supervisor/logs/ospf-worker.stdout.log",
    status: "running",
  },
];

const dataSourceStatus = [
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-sfo-01",
    client_status: "online",
    display_name: "edge-sfo-01",
    domain: "runtime_traffic_accounting_source",
    evidence: {
      interface: "eth0",
      sample_count: 3,
      source: "vnstat",
      traffic_status: "ok",
    },
    module: "Traffic",
    preset_id: "11111111-1111-4111-8111-111111111111",
    preset_name: "shared:vnstat-json",
    preset_scope: "shared",
    source_kind: "vnstat",
    status: "ok",
    status_reason:
      "latest traffic samples are available from the selected preset",
  },
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-fra-02",
    client_status: "online",
    display_name: "core-fra-02",
    domain: "runtime_traffic_accounting_source",
    evidence: {
      interface: "tun0",
      sample_count: 1,
      source: "telemetry_reported_tunnel",
      traffic_status: "ok",
    },
    module: "Traffic",
    preset_id: "00000000-0000-4000-8000-000000000002",
    preset_name: "builtin:interface_counters",
    preset_scope: "built_in",
    source_kind: "interface_counters",
    status: "ok",
    status_reason:
      "latest interface counters are available from the selected preset",
  },
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-sfo-01",
    client_status: "online",
    display_name: "edge-sfo-01",
    domain: "backup_object_store",
    evidence: {
      artifact_count: 2,
      continuous_status: false,
      server_object_store_configured: false,
      server_object_store_kind: null,
      workflow: "backup_artifacts",
    },
    module: "Backup object store",
    preset_id: "00000000-0000-4000-8000-000000000009",
    preset_name: "builtin:local_filesystem",
    preset_scope: "built_in",
    source_kind: "local_filesystem",
    status: "selected_no_store",
    status_reason:
      "backup object-store preset is selected, but no server object store is configured",
  },
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-sfo-01",
    client_status: "online",
    display_name: "edge-sfo-01",
    domain: "update_artifact_source",
    evidence: {
      continuous_status: false,
      external_release_count: 1,
      release_count: 1,
      workflow: "agent_update_releases",
    },
    module: "Update artifact source",
    preset_id: "00000000-0000-4000-8000-00000000000a",
    preset_name: "builtin:external_https_sha256",
    preset_scope: "built_in",
    source_kind: "external_https",
    status: "ready",
    status_reason:
      "external HTTPS update release metadata exists; agents download update artifacts outside the API",
  },
];

const hotConfigRuleTemplates = [
  {
    actor_id: null,
    built_in: true,
    category: "Data sources",
    created_at: "2026-06-02T10:00:00Z",
    description:
      "Selects the runtime traffic accounting source for selected VPSs.",
    docs_metadata: {
      examples: ['runtime_traffic_accounting_source = "vnstat"'],
      notes: [
        "Generates an incremental config patch only for the traffic accounting source.",
      ],
    },
    domain: "runtime_traffic_accounting_source",
    field_schema: {
      properties: {
        source: {
          enum: ["vnstat", "interface_counters"],
          type: "string",
        },
      },
      required: ["source"],
      type: "object",
    },
    id: "91919191-1111-4111-8111-919191919191",
    name: "Traffic source",
    raw_generator_body: "runtime_traffic_accounting_source = {{source}}",
    updated_at: "2026-06-02T10:00:00Z",
  },
  {
    actor_id: null,
    built_in: true,
    category: "Update",
    created_at: "2026-06-02T10:00:00Z",
    description:
      "Enables autonomous agent updates from the official GitHub release manifest.",
    docs_metadata: {
      predefined: true,
      notes: ["Generates an incremental config patch for the updater section."],
    },
    domain: "agent_update",
    field_schema: {
      properties: {
        unmanaged_version_url: {
          default: "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
          type: "string",
        },
        unmanaged_interval_secs: { default: 86400, type: "integer" },
        unmanaged_jitter_secs: { default: 86400, type: "integer" },
        unmanaged_activate: { default: true, type: "boolean" },
        unmanaged_restart_agent: { default: true, type: "boolean" },
      },
      type: "object",
    },
    id: "55555555-5555-4555-8555-555555555555",
    name: "Autonomous updater enabled",
    raw_generator_body:
      "[update]\nunmanaged_enabled = true\nunmanaged_version_url = {{unmanaged_version_url}}\nunmanaged_interval_secs = {{unmanaged_interval_secs}}\nunmanaged_jitter_secs = {{unmanaged_jitter_secs}}\nunmanaged_activate = {{unmanaged_activate}}\nunmanaged_restart_agent = {{unmanaged_restart_agent}}\n",
    updated_at: "2026-06-02T10:00:00Z",
  },
  {
    actor_id: null,
    built_in: true,
    category: "Update",
    created_at: "2026-06-02T10:00:00Z",
    description:
      "Disables autonomous agent updates while keeping updater defaults explicit.",
    docs_metadata: {
      predefined: true,
      notes: ["Generates an incremental config patch for the updater section."],
    },
    domain: "agent_update",
    field_schema: {
      properties: {
        unmanaged_version_url: {
          default: "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
          type: "string",
        },
        unmanaged_interval_secs: { default: 86400, type: "integer" },
        unmanaged_jitter_secs: { default: 86400, type: "integer" },
        unmanaged_activate: { default: true, type: "boolean" },
        unmanaged_restart_agent: { default: true, type: "boolean" },
      },
      type: "object",
    },
    id: "66666666-6666-4666-8666-666666666666",
    name: "Autonomous updater disabled",
    raw_generator_body:
      "[update]\nunmanaged_enabled = false\nunmanaged_version_url = {{unmanaged_version_url}}\nunmanaged_interval_secs = {{unmanaged_interval_secs}}\nunmanaged_jitter_secs = {{unmanaged_jitter_secs}}\nunmanaged_activate = {{unmanaged_activate}}\nunmanaged_restart_agent = {{unmanaged_restart_agent}}\n",
    updated_at: "2026-06-02T10:00:00Z",
  },
];

const clientKeyRevocations = [
  {
    client_id: "agent-nyc-03",
    created_at: "2026-05-31T10:01:00Z",
    id: "cdcdcdcd-eeee-4faf-8bbb-dddddddddddd",
    public_key_sha256_hex: "c".repeat(64),
    reason: "fixture rebuild",
    revoked_by: "99999999-aaaa-4bbb-8ccc-000000000001",
  },
];

const keyLifecycleReport = {
  clients: agents.map((agent, index) => ({
    client_id: agent.id,
    current_key_revoked: agent.id === "agent-nyc-03",
    current_public_key_sha256_hex: (index + 1).toString(16).repeat(64),
    display_name: agent.display_name,
    latest_revocation_reason:
      agent.id === "agent-nyc-03" ? "fixture rebuild" : null,
    latest_revoked_at:
      agent.id === "agent-nyc-03" ? "2026-05-31T10:01:00Z" : null,
    status: agent.status,
  })),
  current_key_revoked_count: 1,
  direct_identity_client_count: agents.length,
  revocation_count: clientKeyRevocations.length,
};

export const backupId = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

const backupRequests = [
  {
    actor_id: null,
    artifact_id: "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
    client_id: "agent-sfo-01",
    created_at: "2026-05-31T10:00:00Z",
    id: backupId,
    include_config: false,
    note: "fixture backup",
    paths: ["/etc/hostname"],
    payload_hash: "a".repeat(64),
    command_scope: "client:agent-sfo-01",
    status: "artifact_metadata_recorded",
  },
];

const backupArtifacts = [
  {
    client_id: "agent-sfo-01",
    created_at: "2026-05-31T10:01:00Z",
    encrypted: true,
    id: "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
    object_key: `backups/agent-sfo-01/${backupId}.json`,
    sha256_hex: "b".repeat(64),
    size_bytes: 512,
  },
];

export const tunnelPlans = [
  {
    created_at: "2026-05-31T10:03:00Z",
    id: "dddddddd-eeee-4fff-8000-111111111111",
    kind: "gre",
    enabled: true,
    last_apply_job_id: "33333333-aaaa-4bbb-8ccc-dddddddddddd",
    last_rollback_job_id: null,
    left_client_id: "agent-sfo-01",
    left_status: "applied",
    name: "sfo-fra-gre",
    recommended_ospf_cost: 14,
    right_client_id: "agent-fra-02",
    right_status: "applied",
    status: "applied",
    updated_at: "2026-05-31T10:09:00Z",
    deleted_at: null,
    deleted_by: null,
    deleted_reason: null,
    input: {
      name: "sfo-fra-gre",
      interface_name: "tunab",
      kind: "gre",
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_underlay: "198.51.100.10",
      right_underlay: "203.0.113.20",
      address_pool_cidr: "10.255.0.0/30",
      reserved_addresses: [],
      bandwidth: "100m",
      latency_ms: 14,
      packet_loss_ratio: 0,
      preference: 1,
    },
    plan: {
      name: "sfo-fra-gre",
      interface_name: "tunab",
      kind: "gre",
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_underlay: "198.51.100.10",
      right_underlay: "203.0.113.20",
      left_tunnel_address: "10.255.0.0",
      right_tunnel_address: "10.255.0.1",
      tunnel_prefix_len: 31,
      bandwidth: "100m",
      recommended_ospf_cost: 14,
      ifupdown_file: "/etc/network/interfaces.d/vpsman-tunnels",
      bird2_file: "/etc/bird/vpsman-ospf.conf",
      ifupdown_snippet: [
        "# vpsman tunnel sfo-fra-gre: generated plan only",
        "auto tunab",
        "iface tunab inet static",
        "    address 10.255.0.0",
        "    netmask 255.255.255.254",
        "    pointopoint 10.255.0.1",
        "    pre-up ip tunnel add $IFACE mode gre remote 203.0.113.20 local 198.51.100.10 ttl 255",
        "    up ip link set $IFACE up",
        "    post-down ip tunnel del $IFACE || true",
      ].join("\n"),
      bird2_interface_snippet: [
        "# vpsman GRE tunnel sfo-fra-gre: agent-sfo-01 -> agent-fra-02",
        'interface "tunab" {',
        "  type ptp;",
        "  cost 14;",
        "};",
      ].join("\n"),
      touched_files: [
        "/etc/network/interfaces.d/vpsman-tunnels",
        "/etc/bird/vpsman-ospf.conf",
      ],
      validation_steps: ["review generated snippets before apply"],
      rollback_notes: ["remove only the vpsman-managed blocks"],
      conflicts: [],
      mutates_host: false,
    },
  },
  {
    created_at: "2026-05-31T10:04:00Z",
    id: "eeeeeeee-ffff-4000-8111-222222222222",
    kind: "openvpn",
    enabled: true,
    last_apply_job_id: null,
    last_rollback_job_id: null,
    left_client_id: "agent-sfo-01",
    left_status: "planned",
    name: "external-openvpn-observed",
    recommended_ospf_cost: 18,
    right_client_id: "agent-fra-02",
    right_status: "planned",
    status: "planned",
    updated_at: "2026-05-31T10:04:00Z",
    deleted_at: null,
    deleted_by: null,
    deleted_reason: null,
    input: {
      name: "external-openvpn-observed",
      interface_name: "ovpn42",
      kind: "openvpn",
      runtime_control: { manager: "external_observed" },
      runtime_topology: {
        desired_interfaces: ["ovpn42"],
        version: "telemetry-import:ovpn42",
      },
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_underlay: "198.51.100.10",
      right_underlay: "203.0.113.20",
      address_pool_cidr: "10.44.0.0/30",
      reserved_addresses: [],
      bandwidth: "100m",
      latency_ms: 18,
      packet_loss_ratio: 0,
      preference: 1,
    },
    plan: {
      name: "external-openvpn-observed",
      interface_name: "ovpn42",
      kind: "openvpn",
      runtime_control: { manager: "external_observed" },
      runtime_topology: {
        desired_interfaces: ["ovpn42"],
        version: "telemetry-import:ovpn42",
      },
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_underlay: "198.51.100.10",
      right_underlay: "203.0.113.20",
      left_tunnel_address: "10.44.0.0",
      right_tunnel_address: "10.44.0.1",
      tunnel_prefix_len: 31,
      bandwidth: "100m",
      recommended_ospf_cost: 18,
      ifupdown_file: "",
      bird2_file: "/etc/bird/vpsman-ospf.conf",
      ifupdown_snippet: "# vpsman external observed runtime tunnel",
      bird2_interface_snippet: [
        "# vpsman OpenVPN tunnel external-openvpn-observed: agent-sfo-01 -> agent-fra-02",
        'interface "ovpn42" {',
        "  type ptp;",
        "  cost 18;",
        "};",
      ].join("\n"),
      touched_files: ["/etc/bird/vpsman-ospf.conf"],
      validation_steps: [
        "confirm the external tunnel is present before routing apply",
      ],
      rollback_notes: [
        "remove only the matching vpsman-managed Bird2 interface block",
      ],
      conflicts: [],
      mutates_host: false,
    },
  },
];

const networkProbeJobId = "99999999-aaaa-4bbb-8ccc-dddddddddddd";
const networkStatusJobId = "88888888-aaaa-4bbb-8ccc-dddddddddddd";
const networkSpeedJobId = "77777777-aaaa-4bbb-8ccc-dddddddddddd";

const networkJobs = [
  {
    actor_id: null,
    command_type: "agent_update",
    completed_at: "2026-05-31T10:10:00Z",
    created_at: "2026-05-31T10:09:55Z",
    id: "66666666-aaaa-4bbb-8ccc-dddddddddddd",
    payload_hash: "6".repeat(64),
    privileged: true,
    status: "completed",
    target_count: 1,
  },
  {
    actor_id: null,
    command_type: "network_speed_test",
    completed_at: "2026-05-31T10:09:00Z",
    created_at: "2026-05-31T10:08:55Z",
    id: networkSpeedJobId,
    payload_hash: "7".repeat(64),
    privileged: true,
    status: "completed",
    target_count: 2,
  },
  {
    actor_id: null,
    command_type: "network_probe",
    completed_at: "2026-05-31T10:08:00Z",
    created_at: "2026-05-31T10:07:55Z",
    id: networkProbeJobId,
    payload_hash: "9".repeat(64),
    privileged: true,
    status: "completed",
    target_count: 1,
  },
  {
    actor_id: null,
    command_type: "network_status",
    completed_at: "2026-05-31T10:07:00Z",
    created_at: "2026-05-31T10:06:55Z",
    id: networkStatusJobId,
    payload_hash: "8".repeat(64),
    privileged: true,
    status: "completed",
    target_count: 1,
  },
];

const commandTemplates = [
  {
    actor_id: null,
    built_in: true,
    command_type: "shell_argv",
    created_at: "builtin",
    defaults: { confirmed: false, destructive: false, force_unprivileged: false, timeout_secs: 30 },
    display_group: "shell",
    id: "00000000-0000-4100-8000-000000000001",
    name: "Default shell command",
    operation: { argv: ["/usr/bin/uptime"], pty: false, type: "shell" },
    scope_kind: "global",
    scope_value: null,
    updated_at: "builtin",
  },
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    built_in: false,
    command_type: "shell_argv",
    created_at: "2026-05-31T10:04:00Z",
    defaults: { timeout_secs: 30 },
    display_group: "shell",
    id: "46464646-5656-4789-8abc-defdefdefdef",
    name: "edge-health-check",
    operation: { argv: ["uptime"], pty: false, type: "shell" },
    scope_kind: "tag",
    scope_value: "provider:alpha",
    updated_at: "2026-05-31T10:04:00Z",
  },
];

const schedules = [
  {
    catch_up_limit: 1,
    catch_up_policy: "run_once",
    command_type: "shell_argv",
    created_at: "2026-05-31T09:00:00Z",
    cron_expr: "0 * * * *",
    enabled: true,
    failure_count: 0,
    id: "51515151-6161-4717-8abc-defdefdefdef",
    last_error: null,
    last_run_at: "2026-05-31T10:00:00Z",
    max_failures: 3,
    name: "edge-health-hourly",
    next_run_at: "2026-05-31T11:00:00Z",
    next_runs: [
      "2026-05-31T11:00:00Z",
      "2026-05-31T12:00:00Z",
      "2026-05-31T13:00:00Z",
      "2026-05-31T14:00:00Z",
      "2026-05-31T15:00:00Z",
    ],
    operation: { argv: ["uptime"], pty: false, type: "shell" },
    retry_delay_secs: 300,
    selector_expression: "id:agent-sfo-01 || provider:alpha",
    target_client_ids: ["agent-sfo-01", "agent-fra-02"],
    timezone: "UTC",
  },
];

const agentUpdateReleases = [
  {
    actor_id: null,
    artifact_sha256_hex: "d".repeat(64),
    artifact_url_sha256_hex: "f".repeat(64),
    channel: "stable",
    created_at: "2026-05-31T10:08:55Z",
    id: "23232323-3434-4567-8abc-defdefdefdef",
    name: "vpsman-agent",
    notes: "external smoke metadata",
    rollback_artifact_sha256_hex: null,
    rollback_artifact_url_sha256_hex: null,
    rollback_size_bytes: null,
    size_bytes: 1024,
    status: "published_external",
    version: "0.1.0",
  },
];

const networkJobOutputs = {
  [networkSpeedJobId]: [
    {
      client_id: "agent-sfo-01",
      created_at: "2026-05-31T10:09:00Z",
      data_base64: statusOutput({
        bytes: 4194304,
        client_id: "agent-sfo-01",
        duration_secs: 3,
        elapsed_ms: 3300,
        interface: "tunab",
        max_bytes: 16777216,
        peer_client_id: "agent-fra-02",
        plan: "sfo-fra-gre",
        port: 5201,
        probe: "tcp_throughput",
        rate_limit_kbps: 100000,
        role: "server",
        server_address: "10.255.0.0",
        server_side: "left",
        success: true,
        throughput_mbps: 10.1,
        type: "network_speed_test",
      }),
      done: true,
      exit_code: 0,
      job_id: networkSpeedJobId,
      seq: 0,
      stream: "status",
    },
    {
      client_id: "agent-fra-02",
      created_at: "2026-05-31T10:09:00Z",
      data_base64: statusOutput({
        bytes: 4194304,
        client_id: "agent-fra-02",
        duration_secs: 3,
        elapsed_ms: 3300,
        interface: "tunab",
        max_bytes: 16777216,
        peer_client_id: "agent-sfo-01",
        plan: "sfo-fra-gre",
        port: 5201,
        probe: "tcp_throughput",
        rate_limit_kbps: 100000,
        role: "client",
        server_address: "10.255.0.0",
        server_side: "left",
        success: true,
        throughput_mbps: 10.1,
        type: "network_speed_test",
      }),
      done: true,
      exit_code: 0,
      job_id: networkSpeedJobId,
      seq: 1,
      stream: "status",
    },
  ],
  [networkProbeJobId]: [
    {
      client_id: "agent-sfo-01",
      created_at: "2026-05-31T10:08:00Z",
      data_base64: statusOutput({
        client_id: "agent-sfo-01",
        count: 4,
        interface: "tunab",
        interval_ms: 700,
        parsed: {
          healthy: true,
          latency_avg_ms: 12.4,
          latency_max_ms: 14.8,
          latency_min_ms: 10.9,
          packet_loss_ratio: 0.0025,
          received: 4,
          transmitted: 4,
        },
        peer_client_id: "agent-fra-02",
        plan: "sfo-fra-gre",
        probe: "icmp_ping",
        side: "left",
        target: "10.255.0.1",
        type: "network_probe",
      }),
      done: true,
      exit_code: 0,
      job_id: networkProbeJobId,
      seq: 0,
      stream: "status",
    },
  ],
  [networkStatusJobId]: [
    {
      client_id: "agent-sfo-01",
      created_at: "2026-05-31T10:07:00Z",
      data_base64: statusOutput({
        applied: true,
        client_id: "agent-sfo-01",
        interface: "tunab",
        malformed: false,
        peer_client_id: "agent-fra-02",
        plan: "sfo-fra-gre",
        runtime: {
          bird2: { healthy: true },
          interface: { exists: true, operstate: "up" },
        },
        side: "left",
        type: "network_status",
      }),
      done: true,
      exit_code: 0,
      job_id: networkStatusJobId,
      seq: 0,
      stream: "status",
    },
  ],
};

const networkObservations = [
  {
    bytes: 4194304,
    client_id: "agent-fra-02",
    healthy: true,
    id: "70707070-aaaa-4bbb-8ccc-dddddddddddd",
    interface_name: "tunab",
    job_id: networkSpeedJobId,
    kind: "network_speed_test",
    latency_avg_ms: null,
    metadata: {},
    observed_at: "2026-05-31T10:09:00Z",
    packet_loss_ratio: null,
    peer_client_id: "agent-sfo-01",
    plan_name: "sfo-fra-gre",
    role: "client",
    seq: 1,
    target: "10.255.0.0:5201",
    throughput_mbps: 10.1,
  },
  {
    bytes: null,
    client_id: "agent-sfo-01",
    healthy: true,
    id: "90909090-aaaa-4bbb-8ccc-dddddddddddd",
    interface_name: "tunab",
    job_id: networkProbeJobId,
    kind: "network_probe",
    latency_avg_ms: 12.4,
    metadata: {},
    observed_at: "2026-05-31T10:08:00Z",
    packet_loss_ratio: 0.0025,
    peer_client_id: "agent-fra-02",
    plan_name: "sfo-fra-gre",
    role: null,
    seq: 0,
    target: "10.255.0.1",
    throughput_mbps: null,
  },
  {
    bytes: null,
    client_id: "agent-fra-02",
    healthy: false,
    id: "91919191-aaaa-4bbb-8ccc-dddddddddddd",
    interface_name: "ovpn42",
    job_id: networkStatusJobId,
    kind: "network_status",
    latency_avg_ms: null,
    metadata: {
      applied: false,
      runtime: {
        summary: {
          adapter_state: "unhealthy",
          drift: false,
          healthy: false,
          manager: "external_managed_adapter",
          reasons: ["adapter_status_failed"],
          status: "adapter_unhealthy",
        },
      },
    },
    observed_at: "2026-05-31T10:07:30Z",
    packet_loss_ratio: null,
    peer_client_id: "agent-sfo-01",
    plan_name: "external-openvpn",
    role: null,
    seq: 0,
    target: null,
    throughput_mbps: null,
  },
];

const networkTrends = [
  {
    bytes_total: 4194304,
    client_id: "agent-fra-02",
    degraded_count: 0,
    healthy_count: 2,
    interface_name: "tunab",
    kind: "network_speed_test",
    latency_avg_ms: null,
    latency_max_ms: null,
    latency_min_ms: null,
    latest_observed_at: "2026-05-31T10:09:00Z",
    packet_loss_avg_ratio: null,
    peer_client_id: "agent-sfo-01",
    plan_name: "sfo-fra-gre",
    sample_count: 2,
    throughput_avg_mbps: 10.1,
    throughput_max_mbps: 11.8,
  },
  {
    bytes_total: 0,
    client_id: "agent-sfo-01",
    degraded_count: 0,
    healthy_count: 3,
    interface_name: "tunab",
    kind: "network_probe",
    latency_avg_ms: 12.4,
    latency_max_ms: 14.8,
    latency_min_ms: 10.9,
    latest_observed_at: "2026-05-31T10:08:00Z",
    packet_loss_avg_ratio: 0.0025,
    peer_client_id: "agent-fra-02",
    plan_name: "sfo-fra-gre",
    sample_count: 3,
    throughput_avg_mbps: null,
    throughput_max_mbps: null,
  },
];

const topologyGraph = {
  edges: [
    {
      bandwidth: "100m",
      cost_delta: 8,
      degraded_count: 0,
      enabled: true,
      health: "healthy",
      interface_name: "tunab",
      kind: "gre",
      last_apply_job_id: "33333333-aaaa-4bbb-8ccc-dddddddddddd",
      last_rollback_job_id: null,
      latency_avg_ms: 12.4,
      left_client_id: "agent-sfo-01",
      left_status: "applied",
      left_tunnel_address: "10.255.0.0",
      packet_loss_avg_ratio: 0.0025,
      plan_id: tunnelPlans[0].id,
      plan_name: "sfo-fra-gre",
      recommended_ospf_cost: 22,
      right_client_id: "agent-fra-02",
      right_status: "applied",
      right_tunnel_address: "10.255.0.1",
      sample_count: 5,
      status: "applied",
      throughput_avg_mbps: 10.1,
      throughput_max_mbps: 11.8,
      latest_observed_at: "2026-05-31T10:09:00Z",
    },
  ],
  generated_at: "2026-05-31T10:10:00Z",
  nodes: [
    {
      applied_tunnel_count: 1,
      client_id: "agent-sfo-01",
      degraded_tunnel_count: 0,
      display_name: "edge-sfo-01",
      latest_observed_at: "2026-05-31T10:09:00Z",
      status: "online",
      tags: ["provider:alpha", "country:US"],
      tunnel_count: 1,
    },
    {
      applied_tunnel_count: 1,
      client_id: "agent-fra-02",
      degraded_tunnel_count: 0,
      display_name: "core-fra-02",
      latest_observed_at: "2026-05-31T10:09:00Z",
      status: "online",
      tags: ["bgp", "bird2", "country:DE"],
      tunnel_count: 1,
    },
  ],
};

const ospfRecommendations = [
  {
    configured_bandwidth: "100m",
    confidence: "measured",
    cost_delta: 8,
    degraded_count: 0,
    effective_bandwidth: "10m",
    interface_name: "tunab",
    latest_observed_at: "2026-05-31T10:09:00Z",
    latency_avg_ms: 12.4,
    left_client_id: "agent-sfo-01",
    packet_loss_avg_ratio: 0.0025,
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    plan_ospf_cost: 14,
    reason: "derived from persisted probe/speed-test trends",
    recommended_ospf_cost: 22,
    right_client_id: "agent-fra-02",
    sample_count: 5,
    throughput_avg_mbps: 10.1,
    throughput_max_mbps: 11.8,
  },
];

export const ospfUpdatePlans = [
  {
    approval_scope: ["client:agent-sfo-01", "client:agent-fra-02"],
    bird2_file: "/etc/bird/vpsman-ospf.conf",
    change_summary:
      "Change Bird2 OSPF cost on tunab from 14 to 22 for both tunnel endpoints",
    confidence: "measured",
    cost_delta: 8,
    current_ospf_cost: 14,
    evidence: {
      configured_bandwidth: "100m",
      degraded_count: 0,
      effective_bandwidth: "10m",
      latest_observed_at: "2026-05-31T10:09:00Z",
      latency_avg_ms: 12.4,
      packet_loss_avg_ratio: 0.0025,
      reason: "derived from persisted probe/speed-test trends",
      sample_count: 5,
      throughput_avg_mbps: 10.1,
      throughput_max_mbps: 11.8,
    },
    interface_name: "tunab",
    left_client_id: "agent-sfo-01",
    mutation_mode: "reviewed_plan_only",
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    privilege_required: true,
    proposed_left_bird2_interface_snippet: [
      "# vpsman GRE tunnel sfo-fra-gre: agent-sfo-01 -> agent-fra-02",
      'interface "tunab" {',
      "  type ptp;",
      "  cost 22;",
      "};",
    ].join("\n"),
    proposed_right_bird2_interface_snippet: [
      "# vpsman GRE tunnel sfo-fra-gre: agent-fra-02 -> agent-sfo-01",
      'interface "tunab" {',
      "  type ptp;",
      "  cost 22;",
      "};",
    ].join("\n"),
    recommended_ospf_cost: 22,
    requires_approval: true,
    right_client_id: "agent-fra-02",
    status: "review_required",
  },
];

export async function installConsoleApiMock(
  page: Page,
  options: { agentListOverride?: typeof agents } = {},
) {
  await page.addInitScript(
    ({
      agentListOverrideFixture,
      agentsFixture,
      agentUpdateReleasesFixture,
      artifactsFixture,
      backupsFixture,
      dashboardOverviewFixture,
      systemDashboardFixture,
      dataSourceAssignmentsFixture,
      dataSourcePresetsFixture,
      dataSourceStatusFixture,
      hotConfigRuleTemplatesFixture,
      jobCommandTypeByOperationTypeFixture,
      commandTemplatesFixture,
      clientKeyRevocationsFixture,
      keyLifecycleReportFixture,
      fleetAlertNotificationChannelsFixture,
      fleetAlertNotificationsFixture,
      fleetAlertPoliciesFixture,
      fleetAlertStatesFixture,
      fleetAlertsFixture,
      fileTransferSourceArtifactsFixture,
      fileTransfersFixture,
      historyRetentionPoliciesFixture,
      jobOutputsFixture,
      jobsFixture,
      networkObservationsFixture,
      ospfRecommendationsFixture,
      ospfUpdatePlansFixture,
      networkTrendsFixture,
      operatorPreferencesFixture,
      processSupervisorInventoryFixture,
      schedulesFixture,
      summaryFixture,
      suiteConfigRedactedFixture,
      suiteConfigTomlFixture,
      suiteConfigValidationFixture,
      tagsFixture,
      terminalSessionsFixture,
      topologyGraphFixture,
      tunnelPlansFixture,
      webhookDeliveriesFixture,
      webhookRulesFixture,
    }) => {
      const originalFetch = window.fetch.bind(window);
      const targetCountsFromStatuses = (statuses: string[]) => {
        const counts = {
          agent_lost: 0,
          agent_timeout: 0,
          canceled: 0,
          completed: 0,
          control_timeout: 0,
          dispatching: 0,
          failed: 0,
          queued: 0,
          rejected: 0,
          running: 0,
          skipped: 0,
          total: statuses.length,
        };
        for (const status of statuses) {
          if (status in counts && status !== "total") {
            counts[status as keyof Omit<typeof counts, "total">] += 1;
          }
        }
        return counts;
      };
      const queuedTargetCounts = (total: number) =>
        targetCountsFromStatuses(Array.from({ length: total }, () => "queued"));
      const currentOperatorPreferences = { ...operatorPreferencesFixture };
      let currentSuiteConfigToml = suiteConfigTomlFixture;
      const deletedAgentIds = new Set<string>();
      const dashboardAgents = () =>
        (agentListOverrideFixture ?? agentsFixture).filter(
          (agent) => !deletedAgentIds.has(agent.id),
        );
      const visibleAgents = () =>
        agentsFixture.filter((agent) => !deletedAgentIds.has(agent.id));
      const visibleTunnelPlans = () =>
        tunnelPlansFixture.filter(
          (plan) =>
            !deletedAgentIds.has(plan.left_client_id) &&
            !deletedAgentIds.has(plan.right_client_id),
        );
      const requests = {
        backupArtifactHandoffs: [] as unknown[],
        backupPolicies: [] as unknown[],
        agentDeletes: [] as unknown[],
        artifactCleanupJobs: [] as unknown[],
        artifactCleanupPreviews: [] as unknown[],
        bulkTagMutations: [] as unknown[],
        bulkResolve: [] as unknown[],
        dataSourceHotConfigs: [] as unknown[],
        dataSourcePresetAssignments: [] as unknown[],
        dataSourcePresets: [] as unknown[],
        hotConfigRuleTemplates: [] as unknown[],
        agentIdentities: [] as unknown[],
        clientKeyRevocations: [] as unknown[],
        fleetAlertNotificationDispatches: [] as unknown[],
        fleetAlertNotificationProcesses: [] as unknown[],
        fleetAlertNotificationChannels: [] as unknown[],
        fleetAlertPolicies: [] as unknown[],
        fleetAlertStates: [] as unknown[],
        fileBrowserJobs: [] as unknown[],
        fileTransferHandoffs: [] as unknown[],
        fileTransferSourceUploads: [] as unknown[],
        historyRetentionPolicies: [] as unknown[],
        historyRetentionPrunes: [] as unknown[],
        jobs: [] as unknown[],
        jobOutputComparisons: [] as unknown[],
        commandTemplates: [] as unknown[],
        migrationLinks: [] as unknown[],
        operatorActions: [] as unknown[],
        operatorPreferences: [] as unknown[],
        restorePlans: [] as unknown[],
        scheduleActions: [] as unknown[],
        schedules: [] as unknown[],
        suiteConfigs: [] as unknown[],
        tunnelPlanAdapterPromotions: [] as unknown[],
        tunnelPlanAllocations: [] as unknown[],
        tunnelPlanEnabledMutations: [] as unknown[],
        tunnelPlans: [] as unknown[],
        webhookDeliveryRotations: [] as unknown[],
        webhookRuleDispatches: [] as unknown[],
        webhookRuleDryRuns: [] as unknown[],
        webhookRuleProcesses: [] as unknown[],
        webhookRules: [] as unknown[],
      };
      Object.defineProperty(window, "__vpsmanTestRequests", {
        configurable: true,
        value: requests,
      });
      const operatorRecords = [
        {
          created_at: "2026-01-01T00:00:00Z",
          deleted_at: null as string | null,
          disabled_at: null as string | null,
          id: "99999999-aaaa-4bbb-8ccc-000000000001",
          role: "admin",
          scopes: ["*"],
          session_refresh_ttl_secs: 31_536_000,
          status: "active",
          totp_enabled: false,
          username: "console-admin",
        },
        {
          created_at: "2026-01-02T00:00:00Z",
          deleted_at: null as string | null,
          disabled_at: null as string | null,
          id: "99999999-aaaa-4bbb-8ccc-000000000002",
          role: "operator",
          scopes: ["fleet:read", "jobs:write"],
          session_refresh_ttl_secs: 7_776_000,
          status: "active",
          totp_enabled: true,
          username: "noc-operator",
        },
      ];
      const operatorView = (record: (typeof operatorRecords)[number]) => ({
        ...record,
        preferences: currentOperatorPreferences,
      });
      const findOperator = (operatorId: string) =>
        operatorRecords.find((operator) => operator.id === operatorId) ??
        operatorRecords[0];
      const createdJobTargets = new Map<
        string,
        Array<{
          client_id: string;
          completed_at: string | null;
          exit_code: number | null;
          message: string | null;
          started_at: string | null;
          status: string;
        }>
      >();
      const createdJobOutputs = new Map<string, FixtureJobOutput[]>();
      const serverJobsFixture: Array<Record<string, unknown>> = [];
      const commandTypeForOperation = (
        operation: Record<string, unknown> | undefined,
      ): string | null => {
        if (!operation) {
          return null;
        }
        const operationType = typeof operation.type === "string" ? operation.type : "shell";
        if (operationType === "shell") {
          return operation.pty ? "shell_pty" : "shell_argv";
        }
        const commandType = (jobCommandTypeByOperationTypeFixture as Record<string, string>)[operationType];
        if (!commandType) {
          throw new Error(`unknown mock job operation type: ${operationType}`);
        }
        return commandType;
      };
      const scheduleTargetIdsFromSelector = (selector: unknown): string[] => {
        const expression = typeof selector === "string" ? selector : "";
        if (!expression.trim() || expression.trim() === "id:*") {
          return visibleAgents().map((agent) => agent.id);
        }
        const ids = new Set<string>();
        for (const agent of visibleAgents()) {
          const tags = Array.isArray(agent.tags) ? agent.tags : [];
          const matchesId = expression.includes(`id:${agent.id}`);
          const matchesTag = tags.some((tag) =>
            expression.includes(`tag:${tag}`) || expression.includes(tag),
          );
          if (matchesId || matchesTag) {
            ids.add(agent.id);
          }
        }
        return Array.from(ids);
      };
      const normalizeScheduleRecord = (schedule: Record<string, unknown>) => ({
        catch_up_limit: schedule.catch_up_limit ?? 1,
        catch_up_policy: schedule.catch_up_policy ?? "run_once",
        command_type:
          schedule.command_type ??
          commandTypeForOperation(
            schedule.operation as Record<string, unknown> | undefined,
          ) ??
          "shell_argv",
        created_at: schedule.created_at ?? "2026-06-02T10:00:00Z",
        cron_expr: schedule.cron_expr ?? "0 * * * *",
        deferred_until: schedule.deferred_until ?? null,
        deleted_at: schedule.deleted_at ?? null,
        enabled: schedule.enabled ?? true,
        failure_count: schedule.failure_count ?? 0,
        id: schedule.id ?? "52525252-6161-4717-8abc-defdefdefdef",
        last_error: schedule.last_error ?? null,
        last_run_at: schedule.last_run_at ?? null,
        max_failures: schedule.max_failures ?? 3,
        name: schedule.name ?? "scheduled-job",
        next_run_at: schedule.next_run_at ?? "2026-06-02T11:00:00Z",
        next_runs: schedule.next_runs ?? [
          "2026-06-02T11:00:00Z",
          "2026-06-02T12:00:00Z",
          "2026-06-02T13:00:00Z",
          "2026-06-02T14:00:00Z",
          "2026-06-02T15:00:00Z",
        ],
        operation: schedule.operation ?? {
          argv: ["uptime"],
          pty: false,
          type: "shell",
        },
        retry_delay_secs: schedule.retry_delay_secs ?? 300,
        selector_expression: schedule.selector_expression ?? "id:*",
        target_client_ids: Array.isArray(schedule.target_client_ids)
          ? schedule.target_client_ids
          : scheduleTargetIdsFromSelector(schedule.selector_expression ?? "id:*"),
        timezone: schedule.timezone ?? "UTC",
        updated_at:
          schedule.updated_at ?? schedule.created_at ?? "2026-06-02T10:00:00Z",
      });
      const currentSchedules = (
        schedulesFixture as Array<Record<string, unknown>>
      ).map((schedule) => normalizeScheduleRecord(schedule));
      const findSchedule = (encodedScheduleId: string) => {
        const scheduleId = decodeURIComponent(encodedScheduleId);
        return (
          currentSchedules.find((schedule) => schedule.id === scheduleId) ??
          null
        );
      };
      const jsonResponse = (body: unknown, status = 200) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            headers: { "Content-Type": "application/json" },
            status,
          }),
        );
      const tarResponse = (label: string) =>
        Promise.resolve(
          new Response(new TextEncoder().encode(label), {
            headers: { "Content-Type": "application/x-tar" },
            status: 200,
          }),
        );
      const emptyArrayResponse = () => jsonResponse([]);
      const buildWebhookDelivery = (
        request: Record<string, unknown>,
        status: string,
      ) => {
        const expression =
          typeof request.expression === "string" ? request.expression : "";
        const matchedAgents = visibleAgents().filter((agent) => {
          const tags = Array.isArray(agent.tags) ? agent.tags : [];
          return tags.some((tag) => expression.includes(tag));
        });
        const selectedAgents =
          matchedAgents.length > 0 ? matchedAgents : visibleAgents().slice(0, 2);
        const ruleName =
          typeof request.name === "string" && request.name.trim()
            ? request.name.trim()
            : webhookRulesFixture[0]?.name ?? "webhook-rule";
        const eventKind =
          typeof request.event_kind === "string" && request.event_kind.trim()
            ? request.event_kind.trim()
            : "interval.30sec";
        const eventId =
          typeof request.event_id === "string" && request.event_id.trim()
            ? request.event_id.trim()
            : "fixture-preview";
        const target =
          typeof request.target === "string" && request.target.trim()
            ? request.target.trim()
            : webhookRulesFixture[0]?.target ?? "https://hooks.example/vpsman";
        return {
          actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
          attempt_count: status === "queued" || status === "matched_dry_run" ? 0 : 1,
          cooldown_until_unix: 0,
          created_at: "2026-06-02T10:04:00Z",
          dedupe_key: `${ruleName}:${eventKind}:${eventId}`,
          delivered_at: status === "delivered" ? "2026-06-02T10:04:05Z" : null,
          error: null,
          event_id: eventId,
          event_kind: eventKind,
          id: "acacacac-1111-4111-8111-111111111111",
          last_attempt_at:
            status === "queued" || status === "matched_dry_run"
              ? null
              : "2026-06-02T10:04:05Z",
          matched_vps: selectedAgents,
          message: `${ruleName} ${eventKind} count=${selectedAgents.length}`,
          next_attempt_at: status === "queued" ? "2026-06-02T10:09:00Z" : null,
          payload: {
            event_kind: eventKind,
            matched_count: selectedAgents.length,
            rule_name: ruleName,
          },
          rule_id:
            typeof request.id === "string"
              ? request.id
              : webhookRulesFixture[0]?.id ??
                "fefefefe-1111-4111-8111-111111111111",
          rule_name: ruleName,
          status,
          target,
        };
      };

      const readJsonBody = async (
        input: RequestInfo | URL,
        init?: RequestInit,
      ) => {
        const body = init?.body;
        if (typeof body === "string") {
          return JSON.parse(body) as unknown;
        }
        if (input instanceof Request) {
          return input.clone().json() as Promise<unknown>;
        }
        return null;
      };
      const asFixtureRecord = (value: unknown): Record<string, unknown> | null =>
        value && !Array.isArray(value) && typeof value === "object"
          ? (value as Record<string, unknown>)
          : null;
      const fixtureSchemaDefault = (fieldSchema: unknown, field: string) => {
        const schema = asFixtureRecord(fieldSchema) ?? {};
        const fields =
          asFixtureRecord(schema.fields) ??
          asFixtureRecord(schema.properties) ??
          {};
        const spec = asFixtureRecord(fields[field]);
        return spec && "default" in spec ? spec.default : undefined;
      };
      const tomlLiteralFixture = (value: unknown): string => {
        if (typeof value === "string") {
          return JSON.stringify(value);
        }
        if (typeof value === "boolean") {
          return value ? "true" : "false";
        }
        if (typeof value === "number" && Number.isFinite(value)) {
          return String(value);
        }
        if (Array.isArray(value)) {
          return `[${value.map((item) => tomlLiteralFixture(item)).join(", ")}]`;
        }
        if (value === null || value === undefined) {
          return '""';
        }
        return JSON.stringify(value);
      };
      const renderRuleTemplateBodyFixture = (
        rawGeneratorBody: string,
        values: Record<string, unknown>,
        fieldSchema: unknown,
      ) =>
        rawGeneratorBody.replace(
          /\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}/g,
          (_match, field: string) =>
            tomlLiteralFixture(
              field in values
                ? values[field]
                : fixtureSchemaDefault(fieldSchema, field),
            ),
        );
      const affectedSectionsForTomlFixture = (
        toml: string,
        fallback: string,
      ) => {
        const sections = Array.from(
          toml.matchAll(/^\s*\[([^\]]+)\]/gm),
          (match) => match[1].trim(),
        ).filter(Boolean);
        return sections.length > 0 ? sections : [fallback];
      };
      const artifactBodyForTransfer = (clientId: string, sessionId: string) =>
        `server-side transfer handoff ${clientId} ${sessionId}`;
      const sha256HexForText = async (value: string) => {
        const digest = await crypto.subtle.digest(
          "SHA-256",
          new TextEncoder().encode(value),
        );
        return Array.from(new Uint8Array(digest), (byte) =>
          byte.toString(16).padStart(2, "0"),
        ).join("");
      };
      const valueMatches = (
        value: string,
        pattern: string,
        contains: boolean,
      ) => {
        const normalizedValue = value.toLocaleLowerCase();
        const normalizedPattern = pattern.toLocaleLowerCase();
        if (
          normalizedPattern.includes("*") ||
          normalizedPattern.includes("?")
        ) {
          const regex = new RegExp(
            `^${normalizedPattern
              .replace(/[.+^${}()|[\]\\]/g, "\\$&")
              .replace(/\*/g, ".*")
              .replace(/\?/g, ".")}$`,
          );
          return regex.test(normalizedValue);
        }
        return contains
          ? normalizedValue.includes(normalizedPattern)
          : normalizedValue === normalizedPattern;
      };
      type SelectorToken =
        | { kind: "and" | "left" | "or" | "right" }
        | { kind: "term"; raw: string };
      type SelectorExpr =
        | { type: "term"; raw: string }
        | { type: "and"; left: SelectorExpr; right: SelectorExpr }
        | { type: "or"; left: SelectorExpr; right: SelectorExpr };
      const tokenizeSelectorExpression = (
        expression: string,
      ): SelectorToken[] => {
        const tokens: SelectorToken[] = [];
        let index = 0;
        while (index < expression.length) {
          const char = expression[index];
          if (/\s/.test(char)) {
            index += 1;
            continue;
          }
          if (char === "(" || char === ")") {
            tokens.push({ kind: char === "(" ? "left" : "right" });
            index += 1;
            continue;
          }
          if (char === "&" || char === "|") {
            if (expression[index + 1] !== char) {
              throw new Error("Use && or || for boolean operators");
            }
            tokens.push({ kind: char === "&" ? "and" : "or" });
            index += 2;
            continue;
          }
          const start = index;
          while (
            index < expression.length &&
            !/[\s()&|]/.test(expression[index])
          ) {
            index += 1;
          }
          const raw = expression.slice(start, index);
          const lower = raw.toLocaleLowerCase();
          if (lower === "and" || lower === "or") {
            tokens.push({ kind: lower === "and" ? "and" : "or" });
          } else {
            tokens.push({ kind: "term", raw });
          }
        }
        return tokens;
      };
      const parseSelectorExpression = (
        expression: string,
      ): SelectorExpr | null => {
        const tokens = tokenizeSelectorExpression(expression);
        if (tokens.length === 0) {
          return null;
        }
        let position = 0;
        const peek = () => tokens[position];
        const consume = () => tokens[position++];
        const startsPrimary = () => {
          const token = peek();
          return token?.kind === "term" || token?.kind === "left";
        };
        const parsePrimary = (): SelectorExpr => {
          const token = consume();
          if (!token) {
            throw new Error("Expression is incomplete");
          }
          if (token.kind === "term") {
            return { type: "term", raw: token.raw };
          }
          if (token.kind === "left") {
            const nested = parseOr();
            if (consume()?.kind !== "right") {
              throw new Error("Missing closing parenthesis");
            }
            return nested;
          }
          throw new Error("Operator is missing an operand");
        };
        const parseAnd = (): SelectorExpr => {
          let current = parsePrimary();
          while (peek()?.kind === "and" || startsPrimary()) {
            if (peek()?.kind === "and") {
              consume();
            }
            current = { type: "and", left: current, right: parsePrimary() };
          }
          return current;
        };
        const parseOr = (): SelectorExpr => {
          let current = parseAnd();
          while (peek()?.kind === "or") {
            consume();
            current = { type: "or", left: current, right: parseAnd() };
          }
          return current;
        };
        const parsed = parseOr();
        if (position < tokens.length) {
          throw new Error("Unexpected token after expression");
        }
        return parsed;
      };
      const termMatchesAgent = (
        agent: (typeof agentsFixture)[number],
        term: string,
      ) => {
        const separator = term.indexOf(":");
        if (separator > 0) {
          const namespace = term.slice(0, separator).toLocaleLowerCase();
          const value = term.slice(separator + 1);
          if (!value) {
            return false;
          }
          if (namespace === "id") {
            return valueMatches(agent.id, value, false);
          }
          if (namespace === "name") {
            return valueMatches(agent.display_name, value, false);
          }
          if (namespace === "tag") {
            return agent.tags.some((tag) => valueMatches(tag, value, false));
          }
          if (namespace === "provider") {
            return agent.tags.some((tag) =>
              valueMatches(tag, `provider:${value}`, false),
            );
          }
          if (namespace === "country" || namespace === "region") {
            return agent.tags.some((tag) =>
              valueMatches(tag, `country:${value}`, false),
            );
          }
          if (namespace === "status") {
            return valueMatches(agent.status, value, false);
          }
          return false;
        }
        return (
          valueMatches(agent.id, term, true) ||
          valueMatches(agent.display_name, term, true)
        );
      };
      const evaluateSelectorExpression = (
        agent: (typeof agentsFixture)[number],
        expression: SelectorExpr | null,
      ): boolean => {
        if (!expression) {
          return true;
        }
        if (expression.type === "and") {
          return (
            evaluateSelectorExpression(agent, expression.left) &&
            evaluateSelectorExpression(agent, expression.right)
          );
        }
        if (expression.type === "or") {
          return (
            evaluateSelectorExpression(agent, expression.left) ||
            evaluateSelectorExpression(agent, expression.right)
          );
        }
        return termMatchesAgent(agent, expression.raw);
      };
      const expressionMatchesAgent = (
        agent: (typeof agentsFixture)[number],
        expression: string,
      ) =>
        evaluateSelectorExpression(agent, parseSelectorExpression(expression));
      const resolveBulkTargets = (body: unknown) => {
        const request = body as { selector_expression?: string } | null;
        const expression = request?.selector_expression?.trim() ?? "";
        if (!expression) {
          return [];
        }
        return visibleAgents()
          .filter((agent) => expressionMatchesAgent(agent, expression))
          .sort((left, right) => left.id.localeCompare(right.id));
      };
      const jobTargetsFor = (jobId: string) => {
        const createdTargets = createdJobTargets.get(jobId);
        if (createdTargets) {
          return createdTargets.map((target) => ({ ...target, job_id: jobId }));
        }
        const job = (
          jobsFixture as Array<{
            id: string;
            status: string;
            target_count: number;
            completed_at: string | null;
          }>
        ).find((candidate) => candidate.id === jobId) ?? {
          completed_at: "2026-05-31T10:09:00Z",
          id: jobId,
          status: "completed",
          target_count: 1,
        };
        const outputs =
          createdJobOutputs.get(jobId) ??
          (
            jobOutputsFixture as Record<
              string,
              Array<{
                client_id: string;
                exit_code?: number | null;
                stream: string;
              }>
            >
          )[jobId] ?? [];
        const outputClientIds = Array.from(
          new Set(outputs.map((output) => output.client_id)),
        );
        const fallbackClientIds = visibleAgents()
          .slice(0, Math.max(1, job.target_count))
          .map((agent) => agent.id);
        const clientIds =
          outputClientIds.length > 0 ? outputClientIds : fallbackClientIds;
        return clientIds.map((clientId) => {
          const statusOutput = outputs.find(
            (output) =>
              output.client_id === clientId && output.stream === "status",
          );
          return {
            client_id: clientId,
            completed_at: job.completed_at,
            exit_code:
              statusOutput?.exit_code ??
              (job.status === "completed" ? 0 : null),
            job_id: jobId,
            started_at: "2026-05-31T10:08:55Z",
            status: job.status,
          };
        });
      };
      const outputComparisonFor = async (jobId: string, mode: string) => {
        const comparisonMode = mode === "text" ? "text" : "binary";
        const targets = jobTargetsFor(jobId);
        const outputs =
          createdJobOutputs.get(jobId) ??
          (
            jobOutputsFixture as Record<
              string,
              Array<{
                client_id: string;
                data_base64?: string;
                stream: string;
              }>
            >
          )[jobId] ?? [];
        const rows = [] as Array<{
          byte_count: number;
          client_id: string;
          exit_code: number | null;
          group_id: string;
          job_id: string;
          matches_largest_group: boolean;
          output_compare_basis: string;
          output_digest_hex: string;
          preview: string;
          status: string;
          stream_count: number;
        }>;
        const grouped = new Map<string, typeof rows>();
        for (const target of targets) {
          const chunks = outputs.filter(
            (output) => output.client_id === target.client_id,
          );
          const decoded = chunks
            .map((chunk) => (chunk.data_base64 ? atob(chunk.data_base64) : ""))
            .join("");
          const normalized =
            comparisonMode === "text"
              ? decoded.replace(/\r\n/g, "\n").replace(/\r/g, "\n").trimEnd()
              : decoded;
          const streamKey = chunks
            .map((chunk) => `${chunk.stream}:${chunk.data_base64 ?? ""}`)
            .join("|");
          const signature = comparisonMode === "text" ? normalized : streamKey;
          const digest = await sha256HexForText(signature);
          const groupKey = `${target.status}:${target.exit_code ?? "-"}:${digest}`;
          const row = {
            byte_count: decoded.length,
            client_id: target.client_id,
            exit_code: target.exit_code,
            group_id: "",
            job_id: jobId,
            matches_largest_group: false,
            output_compare_basis: comparisonMode,
            output_digest_hex: digest,
            preview: normalized || "No retained output",
            status: target.status,
            stream_count: chunks.length,
          };
          const groupRows = grouped.get(groupKey) ?? [];
          groupRows.push(row);
          grouped.set(groupKey, groupRows);
        }
        const ordered = Array.from(grouped.values()).sort(
          (left, right) =>
            right.length - left.length ||
            left[0].client_id.localeCompare(right[0].client_id),
        );
        const largest = ordered[0]?.length ?? 0;
        const groups = ordered.map((groupRows, index) => {
          const groupId = `g${index + 1}`;
          for (const row of groupRows) {
            row.group_id = groupId;
            row.matches_largest_group =
              largest > 0 && groupRows.length === largest;
            rows.push(row);
          }
          return {
            byte_count: groupRows.reduce(
              (total, row) => total + row.byte_count,
              0,
            ),
            client_ids: groupRows.map((row) => row.client_id),
            exit_code: groupRows[0].exit_code,
            group_id: groupId,
            output_compare_basis: groupRows[0].output_compare_basis,
            output_digest_hex: groupRows[0].output_digest_hex,
            preview: groupRows[0].preview,
            representative_client_id: groupRows[0].client_id,
            status: groupRows[0].status,
            stream_count: groupRows.reduce(
              (total, row) => total + row.stream_count,
              0,
            ),
            target_count: groupRows.length,
          };
        });
        return {
          compared_at: "2026-05-31T10:09:30Z",
          compared_targets: rows.length,
          group_count: groups.length,
          groups,
          job_id: jobId,
          mode: comparisonMode,
          rows,
          total_targets: targets.length,
        };
      };
      const requestJsonBody = async (
        input: RequestInfo | URL,
        init?: RequestInit,
      ) => {
        let rawBody = init?.body;
        if (rawBody === undefined && input instanceof Request) {
          rawBody = await input.clone().text();
        }
        if (typeof rawBody === "string") {
          return rawBody.trim() ? JSON.parse(rawBody) : {};
        }
        if (rawBody instanceof Blob) {
          const text = await rawBody.text();
          return text.trim() ? JSON.parse(text) : {};
        }
        if (rawBody instanceof URLSearchParams) {
          return Object.fromEntries(rawBody.entries());
        }
        if (rawBody instanceof FormData) {
          return Object.fromEntries(rawBody.entries());
        }
        return {};
      };

      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        const pathname = new URL(url, window.location.href).pathname;
        const method = (
          init?.method ?? (input instanceof Request ? input.method : "GET")
        ).toUpperCase();
        if (pathname === "/api/v1/dashboard/overview") {
          const params = new URL(url, window.location.href).searchParams;
          const requestedWindow = params.get("window") ?? "24h";
          const requestedGroupBy = params.get("group_by") ?? "labels";
          const requestedResourceMetric =
            params.get("resource_metric") ??
            dashboardOverviewFixture.resource_curve.metric;
          const scopeKind = params.get("scope_kind") ?? "all";
          const scopeValue = params.get("scope_value");
          const startAt = params.get("start_at");
          const endAt = params.get("end_at");
          return jsonResponse({
            ...dashboardOverviewFixture,
            group_by: requestedGroupBy,
            resource_curve: {
              ...dashboardOverviewFixture.resource_curve,
              metric: requestedResourceMetric,
            },
            scope: {
              kind: scopeKind,
              label:
                scopeKind === "all"
                  ? "All VPS"
                  : scopeKind === "provider"
                    ? `provider:${scopeValue}`
                    : scopeKind === "country"
                      ? `country:${scopeValue}`
                      : scopeValue,
              matched_clients: scopeKind === "all" ? 3 : 1,
              query: scopeValue ? `${scopeKind}:${scopeValue}` : null,
              value: scopeValue,
            },
            time_range: {
              ...dashboardOverviewFixture.time_range,
              end_at: endAt ?? dashboardOverviewFixture.time_range.end_at,
              mode: startAt
                ? "custom"
                : requestedWindow === "all"
                  ? "all"
                  : "window",
              start_at: startAt ?? dashboardOverviewFixture.time_range.start_at,
              window: startAt ? null : requestedWindow,
            },
            window: requestedWindow,
          });
        }
        if (pathname === "/api/v1/system/dashboard") {
          return jsonResponse(systemDashboardFixture);
        }
        if (pathname === "/api/v1/admin/suite-config") {
          if (method === "GET") {
            return jsonResponse({
              exists: true,
              hot_reload_note: "API dispatcher limits, gateway-control read timeout, alert thresholds, job-output artifact threshold, update-registration enforcement, gateway runtime timing, and worker tick/schedule/notification/webhook/retention controls are applied by running services after this file changes.",
              path: "config/vpsman.toml",
              redacted: suiteConfigRedactedFixture,
              restart_required_note: "Bind addresses, gateway/API URLs and identities, database URL/migration path/pool sizes, secret refs, object-store clients and local object directories, worker identity/once mode, and connect/write timeout changes require service restart.",
              toml: currentSuiteConfigToml,
              validation: suiteConfigValidationFixture,
            });
          }
          if (method === "PUT") {
            const body = (await requestJsonBody(input, init)) as { toml?: string };
            requests.suiteConfigs.push(body);
            currentSuiteConfigToml = body.toml ?? currentSuiteConfigToml;
            return jsonResponse({
              changed_keys: ["capacity.api_db_pool"],
              path: "config/vpsman.toml",
              validation: suiteConfigValidationFixture,
            });
          }
        }
        if (pathname === "/api/v1/admin/suite-config/validate") {
          const body = (await requestJsonBody(input, init)) as { toml?: string };
          const draftToml = body.toml ?? currentSuiteConfigToml;
          return jsonResponse({
            changed_keys: ["capacity.api_db_pool"],
            exists: true,
            old_redacted: suiteConfigRedactedFixture,
            path: "config/vpsman.toml",
            redacted: {
              ...suiteConfigRedactedFixture,
              capacity: {
                ...(suiteConfigRedactedFixture.capacity),
                api_db_pool: draftToml.includes("api_db_pool = 40") ? 40 : 32,
              },
            },
            validation: suiteConfigValidationFixture,
          });
        }
        if (pathname === "/api/v1/fleet/summary") {
          const currentAgents = visibleAgents();
          return jsonResponse({
            ...summaryFixture,
            online: currentAgents.filter((agent) => agent.status === "online")
              .length,
            total: currentAgents.length,
          });
        }
        if (pathname === "/api/v1/fleet-alerts" && method === "GET") {
          return jsonResponse(fleetAlertsFixture);
        }
        if (pathname === "/api/v1/fleet-alert-states" && method === "GET") {
          return jsonResponse(fleetAlertStatesFixture);
        }
        if (pathname === "/api/v1/fleet-alert-states" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.fleetAlertStates.push(body);
          const request = body as {
            action?: string;
            alert_id?: string;
            muted_for_secs?: number | null;
            reason?: string | null;
          };
          return jsonResponse({
            action: request.action ?? "acknowledge",
            alert_id: request.alert_id ?? fleetAlertsFixture[0].id,
            created_at: "2026-06-02T10:02:00Z",
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            expires_at: request.muted_for_secs ? "2026-06-02T14:02:00Z" : null,
            id: "edededed-1111-4111-8111-111111111111",
            reason: request.reason ?? null,
            updated_at: "2026-06-02T10:02:00Z",
          });
        }
        if (pathname === "/api/v1/fleet-alert-policies" && method === "GET") {
          return jsonResponse(fleetAlertPoliciesFixture);
        }
        if (pathname === "/api/v1/fleet-alert-policies" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.fleetAlertPolicies.push(body);
          return jsonResponse({
            ...(body as Record<string, unknown>),
            created_at: "2026-06-02T10:02:00Z",
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            id: "eeeeeeee-1111-4111-8111-111111111111",
            updated_at: "2026-06-02T10:02:00Z",
          });
        }
        if (
          pathname === "/api/v1/fleet-alert-notification-channels" &&
          method === "GET"
        ) {
          return jsonResponse(fleetAlertNotificationChannelsFixture);
        }
        if (
          pathname === "/api/v1/fleet-alert-notification-channels" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.fleetAlertNotificationChannels.push(body);
          return jsonResponse({
            ...(body as Record<string, unknown>),
            created_at: "2026-06-02T10:02:00Z",
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            id: "efefefef-1111-4111-8111-111111111111",
            updated_at: "2026-06-02T10:02:00Z",
          });
        }
        if (
          pathname === "/api/v1/fleet-alert-notifications" &&
          method === "GET"
        ) {
          return jsonResponse(fleetAlertNotificationsFixture);
        }
        if (
          pathname === "/api/v1/fleet-alert-notifications/dispatch" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.fleetAlertNotificationDispatches.push(body);
          return jsonResponse(fleetAlertNotificationsFixture);
        }
        if (
          pathname === "/api/v1/fleet-alert-notifications/process" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.fleetAlertNotificationProcesses.push(body);
          return jsonResponse(
            fleetAlertNotificationsFixture.map(
              (delivery: Record<string, unknown>) => ({
                ...delivery,
                status: (body as { dry_run?: boolean } | null)?.dry_run
                  ? delivery.status
                  : "sent",
                updated_at: "2026-06-02T10:03:00Z",
              }),
            ),
          );
        }
        if (pathname === "/api/v1/webhook-rules" && method === "GET") {
          return jsonResponse(webhookRulesFixture);
        }
        if (pathname === "/api/v1/webhook-rules" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.webhookRules.push(body);
          return jsonResponse({
            ...(body as Record<string, unknown>),
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            created_at: "2026-06-02T10:04:00Z",
            id: "adadadad-1111-4111-8111-111111111111",
            updated_at: "2026-06-02T10:04:00Z",
          });
        }
        if (
          pathname === "/api/v1/webhook-rules/dry-run" &&
          method === "POST"
        ) {
          const body = (await readJsonBody(input, init)) as Record<
            string,
            unknown
          >;
          requests.webhookRuleDryRuns.push(body);
          const delivery = buildWebhookDelivery(body, "matched_dry_run");
          return jsonResponse({
            delivery,
            matched_vps: delivery.matched_vps,
            payload_context: delivery.payload,
            rendered_message: delivery.message,
            validation_errors: [],
          });
        }
        if (
          pathname === "/api/v1/webhook-rules/dispatch" &&
          method === "POST"
        ) {
          const body = (await readJsonBody(input, init)) as Record<
            string,
            unknown
          >;
          requests.webhookRuleDispatches.push(body);
          return jsonResponse(
            webhookRulesFixture.map((rule: Record<string, unknown>) =>
              buildWebhookDelivery(
                {
                  ...rule,
                  event_id: body.event_id,
                  event_kind: body.event_kind,
                },
                body.dry_run ? "matched_dry_run" : "queued",
              ),
            ),
          );
        }
        const webhookRuleMatch = pathname.match(
          /^\/api\/v1\/webhook-rules\/([^/]+)$/,
        );
        if (webhookRuleMatch && method === "DELETE") {
          const ruleId = decodeURIComponent(webhookRuleMatch[1]);
          requests.webhookRules.push({ delete: ruleId });
          return jsonResponse({ deleted: true, id: ruleId });
        }
        if (pathname === "/api/v1/webhook-deliveries" && method === "GET") {
          return jsonResponse(webhookDeliveriesFixture);
        }
        if (
          pathname === "/api/v1/webhook-deliveries/process" &&
          method === "POST"
        ) {
          const body = (await readJsonBody(input, init)) as {
            dry_run?: boolean;
          } | null;
          requests.webhookRuleProcesses.push(body);
          return jsonResponse(
            webhookDeliveriesFixture.map((delivery: Record<string, unknown>) => ({
              ...delivery,
              status: body?.dry_run ? delivery.status : "delivered",
            })),
          );
        }
        if (
          pathname === "/api/v1/webhook-deliveries/rotate" &&
          method === "POST"
        ) {
          const body = (await readJsonBody(input, init)) as {
            confirmed?: boolean;
            rule_id?: string | null;
            status?: string | null;
          } | null;
          requests.webhookDeliveryRotations.push(body);
          const matchedCount = webhookDeliveriesFixture.filter(
            (delivery: Record<string, unknown>) =>
              (!body?.rule_id || delivery.rule_id === body.rule_id) &&
              (!body?.status || delivery.status === body.status),
          ).length;
          return jsonResponse({
            deleted_count: body?.confirmed ? matchedCount : 0,
            matched_count: matchedCount,
            rule_id: body?.rule_id ?? null,
            status: body?.status ?? null,
          });
        }
        const deleteAgentMatch = pathname.match(
          /^\/api\/v1\/agents\/([^/]+)\/delete$/,
        );
        if (deleteAgentMatch && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.agentDeletes.push(body);
          const clientId = decodeURIComponent(deleteAgentMatch[1]);
          deletedAgentIds.add(clientId);
          return jsonResponse({
            client_id: clientId,
            deleted: true,
            deleted_at: "2026-06-02T10:07:00Z",
          });
        }
        if (pathname === "/api/v1/agents") {
          return jsonResponse(dashboardAgents());
        }
        if (pathname === "/api/v1/gateway-sessions" && method === "GET")
          return emptyArrayResponse();
        if (pathname === "/api/v1/auth/me" && method === "GET")
          return jsonResponse(operatorView(operatorRecords[0]));
        if (pathname === "/api/v1/auth/preferences" && method === "PUT") {
          const body = await readJsonBody(input, init);
          requests.operatorPreferences.push(body);
          Object.assign(currentOperatorPreferences, body);
          return jsonResponse(operatorView(operatorRecords[0]));
        }
        if (pathname === "/api/v1/operators" && method === "POST") {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.operatorActions.push({ action: "create", body });
          const record = {
            created_at: "2026-01-03T00:00:00Z",
            deleted_at: null as string | null,
            disabled_at: null as string | null,
            id: `99999999-aaaa-4bbb-8ccc-${String(operatorRecords.length + 1).padStart(12, "0")}`,
            role: String(body.role ?? "operator"),
            scopes: Array.isArray(body.scopes)
              ? body.scopes.map(String)
              : [],
            session_refresh_ttl_secs:
              typeof body.session_refresh_ttl_secs === "number"
                ? body.session_refresh_ttl_secs
                : 31_536_000,
            status: "active",
            totp_enabled: false,
            username: String(body.username ?? "new-operator"),
          };
          operatorRecords.push(record);
          return jsonResponse(operatorView(record));
        }
        if (pathname === "/api/v1/operators" && method === "GET") {
          return jsonResponse(operatorRecords.map(operatorView));
        }
        const operatorMutationMatch = pathname.match(
          /^\/api\/v1\/operators\/([^/]+)(?:\/([^/]+))?$/,
        );
        if (operatorMutationMatch && (method === "PUT" || method === "POST")) {
          const operatorId = decodeURIComponent(operatorMutationMatch[1]);
          const action = operatorMutationMatch[2] ?? "update";
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.operatorActions.push({ action, body, operator_id: operatorId });
          const record = findOperator(operatorId);
          if (method === "PUT") {
            record.role = String(body.role ?? record.role);
            record.scopes = Array.isArray(body.scopes)
              ? body.scopes.map(String)
              : record.scopes;
            record.session_refresh_ttl_secs =
              typeof body.session_refresh_ttl_secs === "number"
                ? body.session_refresh_ttl_secs
                : record.session_refresh_ttl_secs;
          } else if (action === "enable") {
            record.status = "active";
            record.disabled_at = null;
          } else if (action === "disable") {
            record.status = "disabled";
            record.disabled_at = "2026-01-03T00:10:00Z";
          } else if (action === "delete") {
            record.status = "deleted";
            record.deleted_at = "2026-01-03T00:10:00Z";
          } else if (action === "totp-clear") {
            record.totp_enabled = false;
          }
          return jsonResponse(operatorView(record));
        }
        if (pathname === "/api/v1/operator-sessions" && method === "GET")
          return jsonResponse([
            {
              id: "88888888-aaaa-4bbb-8ccc-000000000001",
              operator_id: "99999999-aaaa-4bbb-8ccc-000000000001",
              operator_role: "admin",
              operator_username: "console-admin",
              current: true,
              created_at: "2026-01-01T00:00:00Z",
              expires_at: "2026-01-01T00:15:00Z",
              refresh_expires_at: "2026-01-15T00:00:00Z",
              revoked: false,
              revoked_at: null,
            },
            {
              id: "88888888-aaaa-4bbb-8ccc-000000000002",
              operator_id: "99999999-aaaa-4bbb-8ccc-000000000001",
              operator_role: "admin",
              operator_username: "console-admin",
              current: false,
              created_at: "2026-01-01T01:00:00Z",
              expires_at: "2026-01-01T01:15:00Z",
              refresh_expires_at: "2026-01-15T01:00:00Z",
              revoked: false,
              revoked_at: null,
            },
          ]);
        if (pathname === "/api/v1/operator-auth-events" && method === "GET")
          return jsonResponse([
            {
              id: "77777777-aaaa-4bbb-8ccc-000000000001",
              operator_id: "99999999-aaaa-4bbb-8ccc-000000000001",
              username: "console-admin",
              result: "success",
              reason: null,
              remote_ip: "127.0.0.1",
              user_agent: "Playwright",
              session_id: "88888888-aaaa-4bbb-8ccc-000000000001",
              created_at: "2026-01-01T00:00:00Z",
            },
            {
              id: "77777777-aaaa-4bbb-8ccc-000000000002",
              operator_id: null,
              username: "unknown-user",
              result: "failure",
              reason: "invalid_credentials",
              remote_ip: "127.0.0.1",
              user_agent: "Playwright",
              session_id: null,
              created_at: "2026-01-01T00:01:00Z",
            },
          ]);
        if (pathname === "/api/v1/client-key-revocations" && method === "GET")
          return jsonResponse(clientKeyRevocationsFixture);
        if (pathname === "/api/v1/key-lifecycle/report" && method === "GET")
          return jsonResponse(keyLifecycleReportFixture);
        if (
          pathname.startsWith("/api/v1/clients/") &&
          pathname.endsWith("/key-revocations") &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.clientKeyRevocations.push(body);
          return jsonResponse({
            client_id: decodeURIComponent(pathname.split("/")[4] ?? ""),
            created_at: "2026-06-02T10:06:00Z",
            id: "edededed-1111-4111-8111-111111111111",
            public_key_sha256_hex: "d".repeat(64),
            reason: (body as { reason?: string | null }).reason ?? null,
            revoked_by: "99999999-aaaa-4bbb-8ccc-000000000001",
          });
        }
        if (pathname === "/api/v1/agent-identities" && method === "POST") {
          const body = (await readJsonBody(input, init)) as {
            client_id?: string;
            client_public_key_hex?: string;
            display_name?: string | null;
            tags?: string[];
          };
          requests.agentIdentities.push(body);
          return jsonResponse({
            client_id: body.client_id ?? "agent-new-direct-01",
            current_public_key_sha256_hex: "e".repeat(64),
            display_name:
              body.display_name || body.client_id || "agent-new-direct-01",
            status: "offline",
            tags: body.tags ?? [],
          });
        }
        if (pathname === "/api/v1/telemetry/rollups" && method === "GET")
          return emptyArrayResponse();
        if (pathname === "/api/v1/telemetry/network-rates" && method === "GET")
          return jsonResponse([
            {
              client_id: "agent-fra-02",
              interface: "eth0",
              bucket_start: "2026-05-31T10:00:00Z",
              bucket_secs: 300,
              sample_count: 2,
              rx_bytes_avg: 45875200,
              tx_bytes_avg: 62914560,
              rx_bytes_delta: 65536,
              tx_bytes_delta: 131072,
              rx_bps_avg: 8738,
              tx_bps_avg: 17476,
              updated_at: "2026-05-31T10:02:05Z",
            },
            {
              client_id: "agent-fra-02",
              interface: "tunab",
              bucket_start: "2026-05-31T10:00:00Z",
              bucket_secs: 300,
              sample_count: 2,
              rx_bytes_avg: 18350080,
              tx_bytes_avg: 22544384,
              rx_bytes_delta: 0,
              tx_bytes_delta: 0,
              rx_bps_avg: 3125000,
              tx_bps_avg: 2760000,
              updated_at: "2026-05-31T10:02:05Z",
            },
            {
              client_id: "agent-fra-02",
              interface: "ovpn42",
              bucket_start: "2026-05-31T09:55:00Z",
              bucket_secs: 300,
              sample_count: 1,
              rx_bytes_avg: 7864320,
              tx_bytes_avg: 7340032,
              rx_bytes_delta: 0,
              tx_bytes_delta: 0,
              rx_bps_avg: 980000,
              tx_bps_avg: 860000,
              updated_at: "2026-05-31T10:00:10Z",
            },
            {
              client_id: "agent-sfo-01",
              interface: "eth0",
              bucket_start: "2026-05-31T10:00:00Z",
              bucket_secs: 300,
              sample_count: 3,
              rx_bytes_avg: 73400320,
              tx_bytes_avg: 68157440,
              rx_bytes_delta: 393216,
              tx_bytes_delta: 458752,
              rx_bps_avg: 19200000,
              tx_bps_avg: 18400000,
              updated_at: "2026-05-31T10:02:06Z",
            },
          ]);
        if (pathname === "/api/v1/telemetry/tunnels" && method === "GET")
          return jsonResponse([
            {
              client_id: "agent-fra-02",
              observed_at: "2026-05-31T10:02:00Z",
              interface: "tunab",
              kind: "gre",
              ownership_mode: "agent_iproute2_managed",
              mutation_policy: "managed_desired",
              promotion_required: false,
              plan_correlation: "matched_saved_plan",
              plan_id: "dddddddd-eeee-4fff-8000-111111111111",
              plan_name: "sfo-fra-gre",
              plan_runtime_manager: "agent_iproute2_managed",
              endpoint_side: "right",
              peer_client_id: "agent-sfo-01",
              source: "sysfs_proc_net_dev",
              operstate: "up",
              mtu: 1500,
              link_type: 65534,
              address: "00:00:00:00:00:00",
              rx_bytes: 18350080,
              tx_bytes: 22544384,
              traffic_source: "interface_counters",
              traffic_status: "ok",
              traffic_reason: null,
              traffic_checked_unix: 1780202520,
              adapter_health: null,
              latency_monitoring_enabled: true,
              latency_status: "down",
              latency_reason: "latency_probe_missing_healthy_sample:3/3",
              latency_primary_family: "ipv4",
              latency_target: "10.255.0.0",
              latency_checked_unix: 1780202520,
              latency_avg_ms: null,
              packet_loss_ratio: 1,
              latency_healthy_windows: 0,
              latency_missed_windows: 3,
              auto_ospf_enabled: true,
              auto_ospf_status: "report_only",
              auto_ospf_reason:
                "latency_probe_unhealthy_ospf_handles_dead_adjacency",
              auto_ospf_current_cost: 14,
              auto_ospf_recommended_cost: 80,
              auto_ospf_updated_unix: null,
            },
            {
              client_id: "agent-sfo-01",
              observed_at: "2026-05-31T10:02:00Z",
              interface: "tunab",
              kind: "gre",
              ownership_mode: "agent_iproute2_managed",
              mutation_policy: "managed_desired",
              promotion_required: false,
              plan_correlation: "matched_saved_plan",
              plan_id: "dddddddd-eeee-4fff-8000-111111111111",
              plan_name: "sfo-fra-gre",
              plan_runtime_manager: "agent_iproute2_managed",
              endpoint_side: "left",
              peer_client_id: "agent-fra-02",
              source: "approved_runtime_status_telemetry",
              operstate: "up",
              mtu: 1476,
              link_type: 778,
              address: "00:00:00:00:00:00",
              rx_bytes: 22544384,
              tx_bytes: 18350080,
              traffic_source: "interface_counters",
              traffic_status: "ok",
              traffic_reason: null,
              traffic_checked_unix: 1780202520,
              adapter_health: null,
              latency_monitoring_enabled: true,
              latency_status: "healthy",
              latency_reason: "probe_ok",
              latency_primary_family: "ipv4",
              latency_target: "10.255.0.1",
              latency_checked_unix: 1780202520,
              latency_avg_ms: 18.4,
              packet_loss_ratio: 0,
              latency_healthy_windows: 5,
              latency_missed_windows: 0,
              auto_ospf_enabled: true,
              auto_ospf_status: "updated",
              auto_ospf_reason: "external_cost_program_succeeded",
              auto_ospf_current_cost: 14,
              auto_ospf_recommended_cost: 21,
              auto_ospf_updated_unix: 1780202520,
            },
            {
              client_id: "agent-fra-02",
              observed_at: "2026-05-31T10:00:00Z",
              interface: "ovpn42",
              kind: "openvpn",
              ownership_mode: "external_observed",
              mutation_policy: "observe_only_saved_plan",
              promotion_required: false,
              plan_correlation: "matched_saved_plan",
              plan_id: "eeeeeeee-ffff-4000-8111-222222222222",
              plan_name: "external-openvpn-observed",
              plan_runtime_manager: "external_observed",
              endpoint_side: "right",
              peer_client_id: "agent-sfo-01",
              source: "approved_runtime_status_telemetry",
              operstate: "unknown",
              mtu: 1500,
              link_type: null,
              address: null,
              rx_bytes: 7864320,
              tx_bytes: 7340032,
              traffic_source: "interface_counters",
              traffic_status: "ok",
              traffic_reason: null,
              traffic_checked_unix: 1780202400,
              adapter_health: null,
              latency_monitoring_enabled: true,
              latency_status: "missed",
              latency_reason: "latency_probe_missing_healthy_sample:1/3",
              latency_primary_family: "ipv4",
              latency_target: "10.255.10.0",
              latency_checked_unix: 1780202400,
              latency_avg_ms: null,
              packet_loss_ratio: 1,
              latency_healthy_windows: 0,
              latency_missed_windows: 1,
              auto_ospf_enabled: false,
              auto_ospf_status: "monitoring_only",
              auto_ospf_reason: "external_cost_program_unconfigured",
              auto_ospf_current_cost: 18,
              auto_ospf_recommended_cost: 18,
              auto_ospf_updated_unix: null,
            },
          ]);
        if (pathname === "/api/v1/data-source-presets" && method === "GET") {
          return jsonResponse(dataSourcePresetsFixture);
        }
        if (pathname === "/api/v1/data-source-presets" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.dataSourcePresets.push(body);
          return jsonResponse({
            ...(body as Record<string, unknown>),
            assigned_client_count: 0,
            built_in: false,
            created_at: "2026-06-02T10:03:00Z",
            id: "33333333-3333-4333-8333-333333333333",
            is_default: false,
            updated_at: "2026-06-02T10:03:00Z",
          });
        }
        if (
          pathname === "/api/v1/data-source-assignments" &&
          method === "GET"
        ) {
          return jsonResponse(dataSourceAssignmentsFixture);
        }
        if (pathname === "/api/v1/data-source-status" && method === "GET") {
          return jsonResponse(dataSourceStatusFixture);
        }
        if (
          pathname === "/api/v1/hot-config/rule-templates" &&
          method === "GET"
        ) {
          return jsonResponse(hotConfigRuleTemplatesFixture);
        }
        if (
          pathname === "/api/v1/hot-config/rule-templates" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.hotConfigRuleTemplates.push(body);
          const request = body as {
            category?: string;
            description?: string;
            docs_metadata?: Record<string, unknown>;
            domain?: string;
            field_schema?: Record<string, unknown>;
            id?: string | null;
            name?: string;
            raw_generator_body?: string;
          };
          return jsonResponse({
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            built_in: false,
            category: request.category ?? "Custom",
            created_at: "2026-06-02T10:05:00Z",
            description: request.description ?? "",
            docs_metadata: request.docs_metadata ?? {},
            domain: request.domain ?? "custom",
            field_schema: request.field_schema ?? { type: "object" },
            id: request.id ?? "92929292-2222-4222-8222-929292929292",
            name: request.name ?? "Custom rule",
            raw_generator_body: request.raw_generator_body ?? "",
            updated_at: "2026-06-02T10:05:00Z",
          });
        }
        if (
          pathname.startsWith("/api/v1/hot-config/rule-templates/") &&
          pathname.endsWith("/render") &&
          method === "POST"
        ) {
          const templateId =
            pathname.split("/").at(-2) ?? hotConfigRuleTemplatesFixture[0].id;
          const template =
            hotConfigRuleTemplatesFixture.find(
              (record: { id: string }) => record.id === templateId,
            ) ?? hotConfigRuleTemplatesFixture[0];
          const body = await readJsonBody(input, init);
          const values =
            asFixtureRecord(asFixtureRecord(body)?.values) ?? {};
          const toml = renderRuleTemplateBodyFixture(
            template.raw_generator_body,
            values,
            template.field_schema,
          );
          return jsonResponse({
            affected_sections: affectedSectionsForTomlFixture(
              toml,
              template.domain,
            ),
            docs_metadata: template.docs_metadata,
            generated_at: "2026-06-02T10:06:00Z",
            name: template.name,
            patch: {},
            template_id: template.id,
            toml,
          });
        }
        if (
          pathname.startsWith("/api/v1/hot-config/rule-templates/") &&
          method === "DELETE"
        ) {
          return new Response(null, { status: 204 });
        }
        if (
          pathname === "/api/v1/data-source-assignments" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.dataSourcePresetAssignments.push(body);
          const request = body as {
            preset_id?: string;
            selector_expression?: string;
            target_client_ids?: string[];
            confirmed?: boolean;
          };
          const preset =
            dataSourcePresetsFixture.find(
              (record: { id: string }) => record.id === request.preset_id,
            ) ?? dataSourcePresetsFixture[0];
          const targetCount = Array.isArray(request.target_client_ids)
            ? request.target_client_ids.length
            : request.selector_expression
              ? visibleAgents().filter((agent) =>
                  expressionMatchesAgent(
                    agent,
                    request.selector_expression ?? "",
                  ),
                ).length
              : 0;
          return jsonResponse({
            assignments: dataSourceAssignmentsFixture,
            confirmation_required: !request.confirmed,
            preset,
            target_count: targetCount,
          });
        }
        if (
          pathname === "/api/v1/data-source-hot-config" &&
          method === "GET"
        ) {
          const clientId =
            new URL(url, window.location.href).searchParams.get("client_id") ??
            "agent-sfo-01";
          requests.dataSourceHotConfigs.push({ client_id: clientId });
          return jsonResponse({
            assignments: dataSourceAssignmentsFixture.filter(
              (assignment) => assignment.client_id === clientId,
            ),
            client_id: clientId,
            generated_at: "2026-06-02T10:07:00Z",
            render_notes: [],
            sections: {
              data_sources: {
                client_id: clientId,
              },
            },
            toml: `[data_sources]\nclient_id = "${clientId}"\n`,
            unsupported_domains: [],
          });
        }
        if (pathname === "/api/v1/jobs" && method === "GET") {
          return jsonResponse(jobsFixture);
        }
        if (pathname === "/api/v1/server-jobs" && method === "GET") {
          return jsonResponse(serverJobsFixture);
        }
        if (
          pathname === "/api/v1/server-jobs/artifact-cleanup/preview" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          const request = body as { expression?: string };
          requests.artifactCleanupPreviews.push(body);
          const matchedBytes = (
            fileTransferSourceArtifactsFixture as Array<{ size_bytes?: number }>
          ).reduce((sum, artifact) => sum + (artifact.size_bytes ?? 0), 0);
          return jsonResponse({
            expression: request.expression ?? "",
            matched_count: (
              fileTransferSourceArtifactsFixture as Array<unknown>
            ).length,
            matched_bytes: matchedBytes,
            preview_hash:
              "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          });
        }
        if (
          pathname === "/api/v1/server-jobs/artifact-cleanup" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          const request = body as {
            expression?: string;
            preview_hash?: string;
          };
          requests.artifactCleanupJobs.push(body);
          const matchedBytes = (
            fileTransferSourceArtifactsFixture as Array<{ size_bytes?: number }>
          ).reduce((sum, artifact) => sum + (artifact.size_bytes ?? 0), 0);
          const job = {
            canceled_at: null,
            completed_at: null,
            created_at: "2026-06-02T10:15:00Z",
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            deleted_bytes: 0,
            deleted_count: 0,
            error: null,
            expression: request.expression ?? "",
            id: "81818181-2222-4333-8444-555555555555",
            job_type: "artifact_cleanup",
            matched_bytes: matchedBytes,
            matched_count: (
              fileTransferSourceArtifactsFixture as Array<unknown>
            ).length,
            metadata: {},
            preview_hash: request.preview_hash ?? null,
            started_at: null,
            status: "queued",
          };
          serverJobsFixture.unshift(job);
          return jsonResponse(job, 201);
        }
        const serverJobCancelMatch = pathname.match(
          /^\/api\/v1\/server-jobs\/([^/]+)\/cancel$/,
        );
        if (serverJobCancelMatch && method === "POST") {
          const jobId = decodeURIComponent(serverJobCancelMatch[1]);
          const job = serverJobsFixture.find((record) => record.id === jobId);
          if (job) {
            job.status = "canceled";
            job.canceled_at = "2026-06-02T10:16:00Z";
            return jsonResponse(job);
          }
          return jsonResponse({ error: "server job not found" }, 404);
        }
        if (pathname === "/api/v1/command-templates" && method === "GET") {
          return jsonResponse(commandTemplatesFixture);
        }
        if (pathname === "/api/v1/command-templates" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.commandTemplates.push(body);
          const request = body as {
            display_group?: string | null;
            defaults?: Record<string, unknown> | null;
            name?: string;
            operation?: Record<string, unknown>;
            scope_kind?: string;
            scope_value?: string | null;
          };
          return jsonResponse({
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            built_in: false,
            command_type: commandTypeForOperation(request.operation) ?? "shell_argv",
            created_at: "2026-06-02T10:04:00Z",
            defaults: request.defaults ?? {},
            display_group: request.display_group ?? "shell",
            id: "47474747-5656-4789-8abc-defdefdefdef",
            name: request.name ?? "saved-template",
            operation: request.operation ?? {
              argv: ["uptime"],
              pty: false,
              type: "shell",
            },
            scope_kind: request.scope_kind ?? "global",
            scope_value: request.scope_value ?? null,
            updated_at: "2026-06-02T10:04:00Z",
          });
        }
        const commandTemplateMatch = pathname.match(/^\/api\/v1\/command-templates\/([^/]+)$/);
        if (commandTemplateMatch && method === "DELETE") {
          const templateId = decodeURIComponent(commandTemplateMatch[1]);
          const template = commandTemplatesFixture.find(
            (record: { id: string }) => record.id === templateId,
          );
          if (!template) {
            return jsonResponse({ error: "command_template_not_found" }, 404);
          }
          if ((template as { built_in?: boolean }).built_in) {
            return jsonResponse({ error: "command_template_builtin_immutable" }, 409);
          }
          return jsonResponse(template);
        }
        if (pathname === "/api/v1/agent-update-releases" && method === "GET") {
          return jsonResponse(agentUpdateReleasesFixture);
        }
        if (
          pathname === "/api/v1/process-supervisor/inventory" &&
          method === "GET"
        ) {
          return jsonResponse(processSupervisorInventoryFixture);
        }
        if (pathname === "/api/v1/file-transfers" && method === "GET") {
          return jsonResponse(fileTransfersFixture);
        }
        if (pathname === "/api/v1/file-transfer-sources" && method === "GET") {
          return jsonResponse(fileTransferSourceArtifactsFixture);
        }
        if (pathname === "/api/v1/file-transfer-sources" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.fileTransferSourceUploads.push(body);
          const request = body as {
            name?: string;
            sha256_hex?: string;
            size_bytes?: number;
          };
          return jsonResponse({
            id: "73737373-2222-4333-8444-555555555555",
            name: request.name ?? "source.bin",
            object_key: `file-transfer-sources/${request.sha256_hex}.bin`,
            sha256_hex: request.sha256_hex,
            size_bytes: request.size_bytes,
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            created_at: "2026-05-31T10:12:00Z",
            download_path:
              "/api/v1/file-transfer-sources/73737373-2222-4333-8444-555555555555/artifact",
          });
        }
        if (
          pathname ===
            "/api/v1/file-transfer-sources/62626262-2222-4333-8444-555555555555/artifact" &&
          method === "GET"
        ) {
          return Promise.resolve(
            new Response("stored source artifact", {
              headers: { "Content-Type": "application/octet-stream" },
            }),
          );
        }
        const handoffMatch = pathname.match(
          /^\/api\/v1\/file-transfers\/([^/]+)\/([^/]+)\/handoff$/,
        );
        if (handoffMatch && method === "POST") {
          const clientId = decodeURIComponent(handoffMatch[1]);
          const sessionId = decodeURIComponent(handoffMatch[2]);
          const transfer = fileTransfersFixture.find(
            (record: { client_id: string; session_id: string }) =>
              record.client_id === clientId && record.session_id === sessionId,
          );
          if (!transfer) {
            return jsonResponse({ error: "unknown file transfer" }, 404);
          }
          requests.fileTransferHandoffs.push({
            body: await readJsonBody(input, init),
            client_id: clientId,
            session_id: sessionId,
          });
          const artifactBody = artifactBodyForTransfer(clientId, sessionId);
          const artifactSha256Hex = await sha256HexForText(artifactBody);
          const sizeBytes = new TextEncoder().encode(artifactBody).byteLength;
          const chunkSize = transfer.chunk_size_bytes ?? 65536;
          return jsonResponse({
            client_id: clientId,
            session_id: sessionId,
            object_key: `file-transfers/${Array.from(
              new TextEncoder().encode(clientId),
              (byte) => byte.toString(16).padStart(2, "0"),
            ).join("")}/${sessionId}/${artifactSha256Hex}.bin`,
            sha256_hex: artifactSha256Hex,
            size_bytes: sizeBytes,
            chunk_count: Math.max(1, Math.ceil(sizeBytes / chunkSize)),
            source: "job_outputs",
            download_path: `/api/v1/file-transfers/${encodeURIComponent(clientId)}/${encodeURIComponent(
              sessionId,
            )}/handoff/artifact`,
          });
        }
        const handoffArtifactMatch = pathname.match(
          /^\/api\/v1\/file-transfers\/([^/]+)\/([^/]+)\/handoff\/artifact$/,
        );
        if (handoffArtifactMatch && method === "GET") {
          const clientId = decodeURIComponent(handoffArtifactMatch[1]);
          const sessionId = decodeURIComponent(handoffArtifactMatch[2]);
          const transfer = fileTransfersFixture.find(
            (record: { client_id: string; session_id: string }) =>
              record.client_id === clientId && record.session_id === sessionId,
          );
          if (!transfer) {
            return jsonResponse({ error: "unknown transfer artifact" }, 404);
          }
          const artifactBody = artifactBodyForTransfer(clientId, sessionId);
          return Promise.resolve(
            new Response(artifactBody, {
              headers: {
                "Content-Length": String(
                  new TextEncoder().encode(artifactBody).byteLength,
                ),
                "Content-Type": "application/octet-stream",
                "x-vpsman-artifact-sha256":
                  await sha256HexForText(artifactBody),
              },
            }),
          );
        }
        if (pathname === "/api/v1/terminal-sessions" && method === "GET") {
          return jsonResponse(terminalSessionsFixture);
        }
        if (
          pathname ===
            "/api/v1/terminal-sessions/agent-sfo-01/61616161-2222-4333-8444-555555555555/replay" &&
          method === "GET"
        ) {
          return jsonResponse({
            session_id: "61616161-2222-4333-8444-555555555555",
            client_id: "agent-sfo-01",
            from_seq: Number(
              new URL(url, window.location.href).searchParams.get("from_seq") ??
                "1",
            ),
            available_first_seq: 1,
            next_seq: 4,
            chunk_count: 2,
            byte_count: 30,
            truncated: false,
            source: "job_outputs",
            chunks: [
              {
                terminal_seq: 1,
                job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
                job_output_seq: 0,
                data_base64: btoa("durable replay line 1\n"),
                size_bytes: 22,
                sha256_hex: "8".repeat(64),
                storage: "inline",
                artifact_object_key: null,
                created_at: "2026-05-31T10:12:00Z",
              },
              {
                terminal_seq: 2,
                job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
                job_output_seq: 1,
                data_base64: btoa("prompt$ "),
                size_bytes: 8,
                sha256_hex: "9".repeat(64),
                storage: "inline",
                artifact_object_key: null,
                created_at: "2026-05-31T10:12:00Z",
              },
            ],
          });
        }
        if (pathname === "/api/v1/network/observations" && method === "GET") {
          return jsonResponse(networkObservationsFixture);
        }
        if (
          pathname === "/api/v1/network/observation-trends" &&
          method === "GET"
        ) {
          return jsonResponse(networkTrendsFixture);
        }
        if (
          pathname === "/api/v1/network/ospf-recommendations" &&
          method === "GET"
        ) {
          return jsonResponse(ospfRecommendationsFixture);
        }
        if (
          pathname === "/api/v1/network/ospf-update-plans" &&
          method === "GET"
        ) {
          return jsonResponse(ospfUpdatePlansFixture);
        }
        if (pathname === "/api/v1/network/topology-graph" && method === "GET") {
          return jsonResponse(topologyGraphFixture);
        }
        const targetStatusDownloadMatch = pathname.match(
          /^\/api\/v1\/jobs\/([^/]+)\/targets\/download$/,
        );
        if (targetStatusDownloadMatch && method === "GET") {
          return tarResponse("target status archive");
        }
        const targetMatch = pathname.match(
          /^\/api\/v1\/jobs\/([^/]+)\/targets$/,
        );
        if (targetMatch && method === "GET") {
          return jsonResponse(jobTargetsFor(targetMatch[1]));
        }
        const comparisonMatch = pathname.match(
          /^\/api\/v1\/jobs\/([^/]+)\/output-comparison$/,
        );
        if (comparisonMatch && method === "GET") {
          const params = new URL(url, window.location.href).searchParams;
          const mode =
            params.get("mode") ??
            currentOperatorPreferences.bulk_output_compare_mode;
          requests.jobOutputComparisons.push({
            job_id: comparisonMatch[1],
            mode,
          });
          return jsonResponse(
            await outputComparisonFor(comparisonMatch[1], mode),
          );
        }
        const outputMatch = pathname.match(
          /^\/api\/v1\/jobs\/([^/]+)\/outputs$/,
        );
        if (outputMatch && method === "GET") {
          return jsonResponse(
            createdJobOutputs.get(outputMatch[1]) ??
              (jobOutputsFixture as Record<string, unknown[]>)[outputMatch[1]] ??
              [],
          );
        }
        const jobMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)$/);
        if (jobMatch && method === "GET") {
          return jsonResponse(
            (jobsFixture as Array<{ id: string }>).find(
              (job) => job.id === jobMatch[1],
            ) ?? {
              id: jobMatch[1],
              status: "completed",
            },
          );
        }
        if (pathname === "/api/v1/tags") {
          return jsonResponse(tagsFixture);
        }
        if (pathname === "/api/v1/tags/bulk" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.bulkTagMutations.push(body);
          const request = body as {
            action?: "add" | "remove";
            confirmed?: boolean;
            tag?: string;
            target_client_ids?: string[];
          };
          const targetIds = Array.isArray(request.target_client_ids)
            ? request.target_client_ids
            : [];
          const affected = visibleAgents().filter((agent) =>
            targetIds.includes(agent.id),
          );
          const changedCount = affected.filter((agent) =>
            request.action === "remove"
              ? agent.tags.includes(request.tag ?? "")
              : !agent.tags.includes(request.tag ?? ""),
          ).length;
          return jsonResponse({
            action: request.action ?? "add",
            affected,
            changed_count: changedCount,
            confirmation_required: !request.confirmed,
            schedule_impacts: [],
            skipped_count: affected.length - changedCount,
            tag: request.tag ?? "",
            target_count: affected.length,
          });
        }
        if (pathname === "/api/v1/backups" && method === "GET") {
          return jsonResponse(backupsFixture);
        }
        if (pathname === "/api/v1/schedules" && method === "GET") {
          return jsonResponse(
            currentSchedules.filter((schedule) => !schedule.deleted_at),
          );
        }
        if (pathname === "/api/v1/schedules" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.schedules.push(body);
          const request = body as {
            catch_up_limit?: number;
            catch_up_policy?: string;
            cron_expr?: string;
            enabled?: boolean;
            max_failures?: number;
            name?: string;
            operation?: Record<string, unknown>;
            retry_delay_secs?: number;
            selector_expression?: string;
            target_client_ids?: string[];
            timezone?: string;
          };
          const cronExpr = request.cron_expr ?? "0 * * * *";
          const schedule = normalizeScheduleRecord({
            catch_up_limit: request.catch_up_limit ?? 1,
            catch_up_policy: request.catch_up_policy ?? "run_once",
            command_type: commandTypeForOperation(request.operation) ?? "shell_argv",
            created_at: "2026-06-02T10:04:00Z",
            cron_expr: cronExpr,
            deferred_until: null,
            deleted_at: null,
            enabled: request.enabled ?? true,
            failure_count: 0,
            id: "52525252-6161-4717-8abc-defdefdefdef",
            last_error: null,
            last_run_at: null,
            max_failures: request.max_failures ?? 3,
            name: request.name ?? "scheduled-job",
            next_run_at: "2026-06-02T11:04:00Z",
            next_runs: [
              "2026-06-02T11:04:00Z",
              "2026-06-02T12:04:00Z",
              "2026-06-02T13:04:00Z",
              "2026-06-02T14:04:00Z",
              "2026-06-02T15:04:00Z",
            ],
            operation: request.operation ?? {
              argv: ["uptime"],
              pty: false,
              type: "shell",
            },
            retry_delay_secs: request.retry_delay_secs ?? 300,
            selector_expression: request.selector_expression ?? "id:*",
            target_client_ids: request.target_client_ids ?? scheduleTargetIdsFromSelector(request.selector_expression ?? "id:*"),
            timezone: request.timezone ?? "UTC",
            updated_at: "2026-06-02T10:04:00Z",
          });
          currentSchedules.push(schedule);
          return jsonResponse(schedule);
        }
        const scheduleMatch = pathname.match(/^\/api\/v1\/schedules\/([^/]+)$/);
        if (scheduleMatch && method === "PUT") {
          const body = await readJsonBody(input, init);
          requests.scheduleActions.push({ body, method, path: pathname });
          const schedule = findSchedule(scheduleMatch[1]);
          if (!schedule) {
            return jsonResponse({ error: "schedule_not_found" }, 404);
          }
          const request = body as {
            catch_up_limit?: number;
            catch_up_policy?: string;
            cron_expr?: string;
            enabled?: boolean;
            max_failures?: number;
            name?: string;
            operation?: Record<string, unknown>;
            retry_delay_secs?: number;
            selector_expression?: string;
            target_client_ids?: string[];
            timezone?: string;
          };
          Object.assign(schedule, {
            catch_up_limit: request.catch_up_limit ?? schedule.catch_up_limit,
            catch_up_policy:
              request.catch_up_policy ?? schedule.catch_up_policy,
            command_type:
              commandTypeForOperation(request.operation) ??
              schedule.command_type,
            cron_expr: request.cron_expr ?? schedule.cron_expr,
            enabled: request.enabled ?? schedule.enabled,
            max_failures: request.max_failures ?? schedule.max_failures,
            name: request.name ?? schedule.name,
            operation: request.operation ?? schedule.operation,
            retry_delay_secs:
              request.retry_delay_secs ?? schedule.retry_delay_secs,
            selector_expression:
              request.selector_expression ?? schedule.selector_expression,
            target_client_ids:
              request.target_client_ids ?? schedule.target_client_ids,
            timezone: request.timezone ?? schedule.timezone,
            updated_at: "2026-06-02T10:05:00Z",
          });
          return jsonResponse(schedule);
        }
        if (scheduleMatch && method === "DELETE") {
          const body = await readJsonBody(input, init);
          requests.scheduleActions.push({ body, method, path: pathname });
          const schedule = findSchedule(scheduleMatch[1]);
          if (!schedule) {
            return jsonResponse({ error: "schedule_not_found" }, 404);
          }
          schedule.deleted_at = "2026-06-02T10:08:00Z";
          schedule.enabled = false;
          schedule.updated_at = "2026-06-02T10:08:00Z";
          return jsonResponse(schedule);
        }
        const scheduleTargetsMatch = pathname.match(/^\/api\/v1\/schedules\/([^/]+)\/targets$/);
        if (scheduleTargetsMatch && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.scheduleActions.push({ body, method, path: pathname });
          const schedule = findSchedule(scheduleTargetsMatch[1]);
          if (!schedule) {
            return jsonResponse({ error: "schedule_not_found" }, 404);
          }
          const request = body as {
            selector_expression?: string;
            target_client_ids?: string[];
          };
          schedule.selector_expression = request.selector_expression ?? schedule.selector_expression;
          schedule.target_client_ids = request.target_client_ids ?? schedule.target_client_ids;
          schedule.updated_at = "2026-06-02T10:06:30Z";
          return jsonResponse(schedule);
        }
        const scheduleActionMatch = pathname.match(
          /^\/api\/v1\/schedules\/([^/]+)\/(enable|disable|defer|apply-now)$/,
        );
        if (scheduleActionMatch && method === "POST") {
          const body = await readJsonBody(input, init);
          const [, encodedScheduleId, action] = scheduleActionMatch;
          requests.scheduleActions.push({ body, method, path: pathname });
          const schedule = findSchedule(encodedScheduleId);
          if (!schedule) {
            return jsonResponse({ error: "schedule_not_found" }, 404);
          }
          if (action === "enable") {
            schedule.enabled = true;
            schedule.updated_at = "2026-06-02T10:06:00Z";
            return jsonResponse(schedule);
          }
          if (action === "disable") {
            schedule.enabled = false;
            schedule.updated_at = "2026-06-02T10:06:00Z";
            return jsonResponse(schedule);
          }
          if (action === "defer") {
            schedule.deferred_until =
              (body as { deferred_until?: string } | null)?.deferred_until ??
              "2026-06-03T12:00:00Z";
            schedule.updated_at = "2026-06-02T10:07:00Z";
            return jsonResponse(schedule);
          }
          {
            const fixedTargetIds = Array.isArray(schedule.target_client_ids)
              ? schedule.target_client_ids
              : scheduleTargetIdsFromSelector(schedule.selector_expression);
            const selectedTargets = visibleAgents().filter((agent) => fixedTargetIds.includes(agent.id));
            return jsonResponse({
              target_count: fixedTargetIds.length,
              target_counts: queuedTargetCounts(fixedTargetIds.length),
              job_id: "abababab-2323-4545-8989-cdcdcdcdcdcd",
              schedule_id: schedule.id,
              status: selectedTargets.length === 0 ? "skipped" : "running",
            });
          }
        }
        if (pathname === "/api/v1/backup-policies" && method === "GET") {
          return jsonResponse([]);
        }
        if (pathname === "/api/v1/backup-policies" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.backupPolicies.push(body);
          const request = body as {
            catch_up_limit?: number;
            catch_up_policy?: string;
            cron_expr?: string;
            enabled?: boolean;
            include_config?: boolean;
            keep_last?: number | null;
            max_failures?: number;
            name?: string;
            paths?: string[];
            recipient_public_key_hex?: string | null;
            retry_delay_secs?: number;
            retention_days?: number | null;
            rotation_generation?: string | null;
            selector_expression?: string;
            target_client_ids?: string[];
            timezone?: string;
          };
          return jsonResponse({
            catch_up_limit: request.catch_up_limit ?? 1,
            catch_up_policy: request.catch_up_policy ?? "skip_missed",
            created_at: "2026-06-02T10:11:00Z",
            cron_expr: request.cron_expr ?? "0 3 * * *",
            enabled: request.enabled ?? true,
            failure_count: 0,
            include_config: request.include_config ?? true,
            keep_last: request.keep_last ?? 7,
            last_error: null,
            last_run_at: null,
            max_failures: request.max_failures ?? 3,
            name: request.name ?? "backup-policy",
            next_run_at: "2026-06-03T03:00:00Z",
            next_runs: ["2026-06-03T03:00:00Z"],
            paths: request.paths ?? [],
            recipient_public_key_hex: request.recipient_public_key_hex ?? null,
            retry_delay_secs: request.retry_delay_secs ?? 300,
            retention_days: request.retention_days ?? 30,
            rotation_generation: request.rotation_generation ?? null,
            schedule_id: "62626262-6161-4717-8abc-defdefdefdef",
            selector_expression: request.selector_expression ?? "id:*",
            target_client_ids: request.target_client_ids ?? [],
            timezone: request.timezone ?? "UTC",
            updated_at: "2026-06-02T10:11:00Z",
          });
        }
        if (pathname === "/api/v1/backup-artifacts" && method === "GET") {
          return jsonResponse(artifactsFixture);
        }
        const backupArtifactHandoffMatch = pathname.match(
          /^\/api\/v1\/backups\/([^/]+)\/artifact-handoff$/,
        );
        if (backupArtifactHandoffMatch && method === "POST") {
          const body = (await readJsonBody(input, init)) as {
            job_id?: string | null;
          };
          requests.backupArtifactHandoffs.push(body);
          return jsonResponse({
            artifact: {
              client_id: "agent-sfo-01",
              created_at: "1700009999",
              encrypted: true,
              id: "dddddddd-eeee-4fff-8000-111111111111",
              object_key: `backups/agent-sfo-01/${backupArtifactHandoffMatch[1]}.json`,
              sha256_hex: "1".repeat(64),
              size_bytes: 321,
            },
            source: "retained_job_outputs",
            source_chunk_count: 2,
            source_job_id:
              body.job_id ?? "99999999-2222-4333-8444-555555555555",
          });
        }
        if (pathname === "/api/v1/restore-plans" && method === "GET") {
          return emptyArrayResponse();
        }
        if (pathname === "/api/v1/migration-links" && method === "GET") {
          return emptyArrayResponse();
        }
        if (pathname === "/api/v1/tunnel-plans" && method === "GET") {
          return jsonResponse(visibleTunnelPlans());
        }
        const tunnelPlanEnabledMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)\/(enable|disable)$/,
        );
        if (tunnelPlanEnabledMatch && method === "POST") {
          const planId = decodeURIComponent(tunnelPlanEnabledMatch[1]);
          const enabled = tunnelPlanEnabledMatch[2] === "enable";
          requests.tunnelPlanEnabledMutations.push({ enabled, plan_id: planId });
          const plan = tunnelPlansFixture.find((record) => record.id === planId);
          if (plan) {
            plan.enabled = enabled;
            plan.updated_at = "2026-06-02T10:08:00Z";
            return jsonResponse(plan);
          }
          return jsonResponse({ code: "tunnel_plan_not_found" }, 400);
        }
        if (
          pathname === "/api/v1/tunnel-plans/allocate" &&
          method === "POST"
        ) {
          const body = (await readJsonBody(input, init)) as {
            include_ipv4?: boolean;
            include_ipv6?: boolean;
          };
          requests.tunnelPlanAllocations.push(body);
          return jsonResponse({
            ipv4_tunnel:
              body.include_ipv4 === false
                ? null
                : {
                    left: "10.255.50.0",
                    right: "10.255.50.1",
                    prefix_len: 31,
                  },
            ipv6_tunnel: body.include_ipv6
              ? {
                  left: "fd00:255:50::0",
                  right: "fd00:255:50::1",
                  prefix_len: 127,
                }
              : null,
            latency_primary_family:
              body.include_ipv4 === false && body.include_ipv6
                ? "ipv6"
                : "ipv4",
            conflicts: [],
          });
        }
        if (pathname === "/api/v1/tunnel-plans" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.tunnelPlans.push(body);
          return jsonResponse(tunnelPlansFixture[0]);
        }
        if (
          pathname === "/api/v1/tunnel-plans/promote-adapter" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.tunnelPlanAdapterPromotions.push(body);
          return jsonResponse(tunnelPlansFixture[1]);
        }
        if (pathname === "/api/v1/restore-plans" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.restorePlans.push(body);
          return jsonResponse({
            actor_id: null,
            created_at: "2026-05-31T10:02:00Z",
            destination_root: body.destination_root,
            id: "cccccccc-dddd-4eee-8fff-000000000000",
            include_config: false,
            note: null,
            paths: ["/etc/hostname"],
            payload_hash: "c".repeat(64),
            command_scope: "client:agent-fra-02",
            source_backup_request_id: backupsFixture[0].id,
            source_client_id: "agent-sfo-01",
            status: "planned_metadata_only",
            target_client_id: "agent-fra-02",
          });
        }
        if (pathname === "/api/v1/migration-links" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.migrationLinks.push(body);
          return jsonResponse({
            actor_id: null,
            created_at: "2026-05-31T10:03:00Z",
            destination_root: body.destination_root ?? "/restore",
            id: "dddddddd-eeee-4fff-8aaa-000000000000",
            include_config: false,
            note: null,
            paths: ["/etc/hostname"],
            restore_plan_id: "cccccccc-dddd-4eee-8fff-000000000000",
            source_backup_request_id: backupsFixture[0].id,
            source_client_id: "agent-sfo-01",
            status: "linked_metadata_only",
            target_client_id: "agent-fra-02",
          });
        }
        if (pathname === "/api/v1/audit") {
          return emptyArrayResponse();
        }
        if (
          pathname === "/api/v1/history/retention-policies" &&
          method === "GET"
        ) {
          return jsonResponse(historyRetentionPoliciesFixture);
        }
        if (
          pathname === "/api/v1/history/retention-policies" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.historyRetentionPolicies.push(body);
          return jsonResponse({
            ...historyRetentionPoliciesFixture[0],
            ...(body as Record<string, unknown>),
            built_in_default: false,
            updated_at: "2026-06-02T10:05:00Z",
            updated_by: "99999999-aaaa-4bbb-8ccc-000000000001",
          });
        }
        if (
          pathname === "/api/v1/history/retention-prune" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.historyRetentionPrunes.push(body);
          const request = body as {
            domain?: string | null;
            dry_run?: boolean;
            metadata_only?: boolean | null;
          } | null;
          const domains = historyRetentionPoliciesFixture.filter(
            (policy: { domain: string }) =>
              !request?.domain || policy.domain === request.domain,
          );
          return jsonResponse({
            dry_run: Boolean(request?.dry_run),
            metadata_only_requested: request?.metadata_only ?? null,
            domains: domains.map(
              (policy: {
                domain: string;
                enabled: boolean;
                retention_days: number;
                metadata_only: boolean;
              }) => ({
                cutoff_unix: 1780000000,
                domain: policy.domain,
                enabled: policy.enabled,
                matched_rows: 0,
                metadata_only: request?.metadata_only ?? policy.metadata_only,
                object_delete_attempted: false,
                object_delete_errors: [],
                object_keys: [],
                pruned_rows: 0,
                retention_days: policy.retention_days,
                status: request?.dry_run ? "dry_run" : "pruned",
              }),
            ),
          });
        }
        if (pathname === "/api/v1/history/export" && method === "GET") {
          const requestedDomains =
            new URL(url, window.location.href).searchParams.get("domains") ??
            historyRetentionPoliciesFixture
              .map((policy: { domain: string }) => policy.domain)
              .join(",");
          const domains = requestedDomains
            .split(",")
            .map((entry) => entry.trim())
            .filter((entry) => entry.length > 0);
          return jsonResponse({
            data: {
              audit_logs: [],
              backup_artifacts: artifactsFixture,
              job_outputs: [],
            },
            domains,
            generated_at: "2026-06-02T10:06:00Z",
            limit: Number(
              new URL(url, window.location.href).searchParams.get("limit") ??
                "25",
            ),
          });
        }
        if (pathname === "/api/v1/bulk/resolve" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.bulkResolve.push(body);
          const targets = resolveBulkTargets(body);
          return jsonResponse({
            target_count: targets.length,
            targets,
          });
        }
        if (pathname === "/api/v1/jobs" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.jobs.push(body);
          const targets = resolveBulkTargets(body);
          const commandType =
            (body as { command?: string } | null)?.command ?? "job";
          const targetRecords = targets.map((agent) => ({
            client_id: agent.id,
            completed_at: "2026-05-31T10:09:00Z",
            exit_code:
              agent.status === "stale"
                ? 2
                : agent.status === "offline"
                  ? null
                  : 0,
            message:
              agent.status === "stale"
                ? `stale: agent rejected ${commandType} command_version 3`
                : agent.status === "offline"
                  ? "agent offline"
                  : "completed",
            started_at:
              agent.status === "offline" ? null : "2026-05-31T10:08:55Z",
            status:
              agent.status === "stale"
                ? "failed"
                : agent.status === "offline"
                  ? "control_timeout"
                  : "completed",
          }));
          const jobId = "11111111-2222-4333-8444-555555555555";
          createdJobTargets.set(jobId, targetRecords);
          if (commandType === "config_read") {
            createdJobOutputs.set(
              jobId,
              targetRecords.map((target, index) => ({
                client_id: target.client_id,
                created_at: "2026-05-31T10:09:00Z",
                data_base64: btoa(
                  JSON.stringify({
                    base_config_sha256_hex: "b".repeat(64),
                    toml:
                      'client_id = "' +
                      target.client_id +
                      '"\n\n[update]\nunmanaged_enabled = false\nunmanaged_version_url = "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"\nunmanaged_interval_secs = 86400\nunmanaged_jitter_secs = 86400\nunmanaged_activate = true\nunmanaged_restart_agent = true\n',
                    type: "config_read",
                  }),
                ),
                done: true,
                exit_code: 0,
                job_id: jobId,
                seq: index,
                stream: "status",
              })),
            );
          }
          return jsonResponse({
            target_count: targets.length,
            target_counts: targetCountsFromStatuses(
              targetRecords.map((target) => target.status),
            ),
            job_id: jobId,
            status: "running",
          });
        }
        return originalFetch(input, init);
      };

      class TestWebSocket extends EventTarget {
        static CONNECTING = 0;
        static OPEN = 1;
        static CLOSING = 2;
        static CLOSED = 3;

        readyState = TestWebSocket.OPEN;
        url: string;

        constructor(url: string) {
          super();
          this.url = url;
          window.setTimeout(() => this.dispatchEvent(new Event("open")), 0);
        }

        close() {
          this.readyState = TestWebSocket.CLOSED;
          this.dispatchEvent(new CloseEvent("close"));
        }

        send() {
          return;
        }
      }

      Object.defineProperty(window, "WebSocket", {
        configurable: true,
        value: TestWebSocket,
      });
    },
    {
      agentListOverrideFixture: options.agentListOverride ?? null,
      agentsFixture: agents,
      agentUpdateReleasesFixture: agentUpdateReleases,
      artifactsFixture: backupArtifacts,
      backupsFixture: backupRequests,
      dashboardOverviewFixture: dashboardOverview,
      systemDashboardFixture: systemDashboard,
      dataSourceAssignmentsFixture: dataSourceAssignments,
      dataSourcePresetsFixture: dataSourcePresets,
      dataSourceStatusFixture: dataSourceStatus,
      hotConfigRuleTemplatesFixture: hotConfigRuleTemplates,
      jobCommandTypeByOperationTypeFixture: JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
      commandTemplatesFixture: commandTemplates,
      clientKeyRevocationsFixture: clientKeyRevocations,
      keyLifecycleReportFixture: keyLifecycleReport,
      fleetAlertNotificationChannelsFixture: fleetAlertNotificationChannels,
      fleetAlertNotificationsFixture: fleetAlertNotifications,
      fleetAlertPoliciesFixture: fleetAlertPolicies,
      fleetAlertStatesFixture: fleetAlertStates,
      fleetAlertsFixture: fleetAlerts,
      fileTransferSourceArtifactsFixture: fileTransferSourceArtifacts,
      fileTransfersFixture: fileTransfers,
      historyRetentionPoliciesFixture: historyRetentionPolicies,
      jobOutputsFixture: networkJobOutputs,
      jobsFixture: networkJobs,
      networkObservationsFixture: networkObservations,
      ospfRecommendationsFixture: ospfRecommendations,
      ospfUpdatePlansFixture: ospfUpdatePlans,
      networkTrendsFixture: networkTrends,
      operatorPreferencesFixture: operatorPreferences,
      processSupervisorInventoryFixture: processSupervisorInventory,
      schedulesFixture: schedules,
      summaryFixture: summary,
      suiteConfigRedactedFixture: suiteConfigRedacted,
      suiteConfigTomlFixture: suiteConfigToml,
      suiteConfigValidationFixture: suiteConfigValidation,
      tagsFixture: tags,
      terminalSessionsFixture: terminalSessions,
      topologyGraphFixture: topologyGraph,
      tunnelPlansFixture: tunnelPlans,
      webhookDeliveriesFixture: webhookDeliveries,
      webhookRulesFixture: webhookRules,
    },
  );
  await installTransferJobApiMock(page);
}
