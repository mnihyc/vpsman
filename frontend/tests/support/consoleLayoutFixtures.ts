import type { Page } from "@playwright/test";
import {
  configurationPresets,
  configurationSources,
  networkAdapterDefinitions,
} from "./configurationSourceFixtures";
import {
  fileTransferSourceArtifacts,
  fileTransfers,
  terminalSessions,
} from "./jobSessionFixtures";
import { installTransferJobApiMock } from "./transferJobMock";
import { JOB_COMMAND_TYPE_BY_OPERATION_TYPE } from "../../src/generated/protocolContracts";
import type {
  AuditLogRecord,
  BackupPolicyRecord,
  ConfigurationBehavior,
  ConfigurationPresetRecord,
  FleetAlertNotificationChannelRecord,
  HostPackageUpdatePlanRecord,
  HostServiceInventoryRecord,
  HostStorageInventoryRecord,
  JobRolloutRecord,
  NetworkAdapterDefinitionRecord,
  OperatorAuthEventRecord,
  ScheduleRecord,
  TagMutationResponse,
  VpsRuleValueRecord,
} from "../../src/types";

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
  offline: 0,
  online: 0,
  revoked: 0,
  running_jobs: 3,
  stale: 1,
  total: 3,
  unknown: 2,
  warnings: 3,
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
        label: "routing",
        query: "tag:routing",
        value: "routing",
      },
    ],
    windows: [
      { label: "Realtime · last 15 minutes", seconds: 900, value: "15m" },
      { label: "1 hour", seconds: 3600, value: "1h" },
      { label: "8 hours", seconds: 28800, value: "8h" },
      { label: "1 day", seconds: 86400, value: "1d" },
      { label: "7 days", seconds: 604800, value: "7d" },
      { label: "30 days", seconds: 2592000, value: "30d" },
      { label: "90 days", seconds: 7776000, value: "90d" },
      { label: "180 days", seconds: 15552000, value: "180d" },
      { label: "1 year", seconds: 31536000, value: "1y" },
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
      label: "Inspect network evidence",
      query: null,
      subpage: "evidence",
      view: "Network",
    },
  ],
  generated_at: "2026-06-05T20:44:58Z",
  group_by: "labels",
  label_clusters: [
    {
      counts_truncated: false,
      offline: 0,
      online: 1,
      revoked: 0,
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
      counts_truncated: false,
      offline: 0,
      online: 1,
      revoked: 0,
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
      counts_truncated: false,
      offline: 0,
      online: 1,
      revoked: 0,
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
      counts_truncated: false,
      offline: 0,
      online: 2,
      revoked: 0,
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
    alerts_truncated: false,
    backup_completed: 1,
    backup_failed: 0,
    backup_pending: 1,
    backups_truncated: false,
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
    running_jobs_truncated: false,
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
    latest_sample_at: "2026-06-05T20:35:00Z",
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
    offline: 0,
    online: 2,
    revoked: 0,
    running_jobs: 3,
    running_jobs_truncated: false,
    stale: 1,
    total: 3,
    warnings: 3,
    warnings_truncated: false,
  },
  time_range: {
    end_at: "2026-06-05T20:44:58Z",
    end_unix: 1780692298,
    mode: "window",
    start_at: "2026-06-04T20:44:58Z",
    start_unix: 1780605898,
    window: "1d",
  },
  window: "1d",
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
    worker_schedule_job_max_timeout_secs: 30,
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
      agent_lost_last_24h: 1,
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
    systemSeries(
      "db_pool.in_use_connections",
      "DB in-use connections",
      "connections",
      [4, 5, 6],
    ),
    systemSeries(
      "db_pool.open_connections",
      "DB open connections",
      "connections",
      [20, 22, 24],
    ),
    systemSeries(
      "db_pool.idle_connections",
      "DB idle connections",
      "connections",
      [16, 17, 18],
    ),
    systemSeries(
      "db_pool.max_connections",
      "DB max connections",
      "connections",
      [32, 32, 32],
    ),
    systemSeries(
      "dispatch.queue_depth",
      "Dispatch queue depth",
      "targets",
      [1, 2, 4],
    ),
    systemSeries(
      "targets.dispatching",
      "Dispatching targets",
      "targets",
      [0, 1, 1],
    ),
    systemSeries("targets.running", "Running targets", "targets", [1, 2, 2]),
    systemSeries(
      "dispatch.retried_targets",
      "Retried targets",
      "targets",
      [0, 1, 2],
    ),
    systemSeries(
      "targets.deadline_expired_active",
      "Expired active targets",
      "targets",
      [0, 0, 0],
    ),
    systemSeries(
      "targets.control_timeout_last_24h",
      "Control timeouts",
      "targets",
      [0, 1, 1],
    ),
    systemSeries(
      "targets.agent_timeout_last_24h",
      "Agent timeouts",
      "targets",
      [0, 0, 1],
    ),
    systemSeries(
      "targets.agent_lost_last_24h",
      "Agent lost",
      "targets",
      [0, 0, 1],
    ),
    systemSeries(
      "targets.canceled_last_24h",
      "Canceled targets",
      "targets",
      [0, 1, 1],
    ),
    systemSeries(
      "gateway_events.queued_events",
      "Gateway queued events",
      "events",
      [2, 1, 0],
    ),
    systemSeries(
      "gateway_events.delivered_events",
      "Gateway delivered events",
      "events",
      [900, 918, 928],
    ),
    systemSeries(
      "gateway_events.retry_attempts",
      "Gateway retry attempts",
      "attempts",
      [0, 1, 2],
    ),
    systemSeries(
      "gateway_events.active_queues",
      "Gateway active queues",
      "queues",
      [2, 3, 3],
    ),
    systemSeries(
      "gateway_events.current_queue_depth",
      "Gateway queue depth",
      "events",
      [2, 1, 0],
    ),
    systemSeries(
      "gateway_events.oldest_event_age_secs",
      "Gateway oldest event age",
      "seconds",
      [0, 2, 0],
    ),
    systemSeries(
      "gateway_events.dropped_events",
      "Gateway dropped events",
      "events",
      [0, 0, 1],
    ),
    systemSeries(
      "gateway_events.telemetry_dropped_events",
      "Gateway telemetry drops",
      "events",
      [0, 0, 1],
    ),
    systemSeries(
      "gateway_events.expired_events",
      "Gateway expired events",
      "events",
      [0, 0, 0],
    ),
    systemSeries(
      "gateway_events.critical_failures",
      "Gateway critical failures",
      "events",
      [0, 0, 0],
    ),
    systemSeries(
      "gateway_events.dropped_by_kind.telemetry",
      "Gateway telemetry drops by kind",
      "events",
      [0, 0, 1],
    ),
    systemSeries(
      "gateway_events.dropped_by_reason.coalesced",
      "Gateway coalesced telemetry",
      "events",
      [0, 0, 1],
    ),
    systemSeries(
      "gateway_events.dropped_by_reason.target_queue_full",
      "Gateway target queue full drops",
      "events",
      [0, 0, 0],
    ),
    systemSeries(
      "gateway_events.retained_output_truncated_events",
      "Gateway retained output truncations",
      "events",
      [0, 0, 0],
    ),
    systemSeries(
      "gateway_events.rejected_agent_connections",
      "Gateway rejected agent connections",
      "connections",
      [0, 0, 0],
    ),
    systemSeries(
      "cancellations.requested",
      "Cancel requested",
      "targets",
      [0, 1, 1],
    ),
    systemSeries("cancellations.sent", "Cancel sent", "targets", [0, 1, 1]),
    systemSeries("cancellations.acked", "Cancel acked", "targets", [0, 1, 1]),
    systemSeries(
      "cancellations.awaiting_ack",
      "Cancel awaiting ack",
      "targets",
      [0, 0, 0],
    ),
  ],
  window: "1d",
};

const suiteConfigToml = `version = 1

[api]
bind = "127.0.0.1:8080"
gateway_control_url = "unix:/var/lib/vpsman/gateway-control.sock"
job_output_artifact_min_bytes = 32768
require_registered_agent_updates = false
alert_cpu_load_warning = 2.0
alert_cpu_load_critical = 4.0

[gateway]
bind = "0.0.0.0:9443"
control_bind = "unix:/var/lib/vpsman/gateway-control.sock"
api_url = "http://api:8080"
gateway_id = "compose-gateway"
reconnect_grace_secs = 60

[network]
tunnel_ipv4_allocation_pool_cidr = ""
tunnel_ipv6_allocation_pool_cidr = ""

[worker]
tick_secs = 30
worker_lease_secs = 60
agent_offline_timeout_secs = 300
schedule_job_max_timeout_secs = 30

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
    alert_cpu_load_warning: 2,
    alert_cpu_load_critical: 4,
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
  network: {
    tunnel_ipv4_allocation_pool_cidr: "",
    tunnel_ipv6_allocation_pool_cidr: "",
  },
  secrets: {
    gateway_private_key_file: "/run/secrets/vpsman_gateway_private_key_hex",
    internal_token_file: "/run/secrets/vpsman_internal_token",
    privilege_verifier_key_file:
      "/run/secrets/vpsman_privilege_verifier_key_hex",
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
    "worker.schedule_job_max_timeout_secs",
    "worker.tick_secs",
    "worker.worker_lease_secs",
    "worker.agent_offline_timeout_secs",
    "worker.notification_*",
    "worker.webhook_rule_*",
    "worker.backup_policy_prune_*",
    "worker.require_registered_agent_updates",
    "timeout.worker_schedule_job_max_timeout_secs",
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
  gateway_endpoints: "primary=gw.example.com:9443=10",
  gateway_server_public_key_hex:
    "1111111111111111111111111111111111111111111111111111111111111111",
  language: "en",
  review_prompt_mode: "inline",
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
  port_forwarding: {
    nft_version: "nftables v1.1.3",
    reason: null,
    status: "supported",
  },
  unprivileged_hint: null,
};

const unprivilegedCapabilities = {
  can_apply_process_limits: false,
  can_attempt_privileged_ops: true,
  can_manage_runtime_tunnels: false,
  effective_uid: 1000,
  privilege_mode: "unprivileged",
  port_forwarding: {
    nft_version: "nftables v1.0.9",
    reason: "Agent lacks CAP_NET_ADMIN in the host network namespace",
    status: "insufficient_privilege",
  },
  unprivileged_hint:
    "agent is not running as root; root-only network, update, restore, and limit operations may report ineffective or require forced best-effort mode",
};

const agents = [
  {
    capabilities: rootCapabilities,
    display_name: "edge-sfo-01",
    id: "agent-sfo-01",
    last_ip: "198.51.100.10",
    registration_ip: "198.51.100.9",
    status: "online",
    tags: ["country:US", "provider:alpha", "role:edge"],
  },
  {
    capabilities: rootCapabilities,
    display_name: "core-fra-02",
    id: "agent-fra-02",
    last_ip: "203.0.113.20",
    registration_ip: "203.0.113.19",
    status: "online",
    tags: ["bgp", "routing", "country:DE"],
  },
  {
    capabilities: unprivilegedCapabilities,
    display_name: "backup-nyc-03",
    id: "agent-nyc-03",
    last_ip: null,
    registration_ip: "192.0.2.30",
    status: "stale",
    tags: ["country:US"],
  },
];

const portForwardRules = [
  {
    agent_desired_hash: "a".repeat(64),
    client_id: "agent-sfo-01",
    created_at: "2026-06-02T09:00:00Z",
    deleted_at: null,
    desired_hash: "a".repeat(64),
    desired_status: "enabled",
    enabled: true,
    forwarding_enabled: true,
    forgotten_at: null,
    id: "4f000000-0000-4000-8000-000000000001",
    mappings: [
      { incoming: { end: 80, start: 80 }, target: { end: 8080, start: 8080 } },
      {
        incoming: { end: 443, start: 443 },
        target: { end: 8443, start: 8443 },
      },
    ],
    masquerade: true,
    name: "Public web ingress",
    nat_matches: 12_482,
    nft_version: "nftables v1.1.3",
    observed_hash: "b".repeat(64),
    protocol: "both",
    removal_confirmed_at: null,
    revision: 3,
    runtime_error: null,
    runtime_error_code: null,
    runtime_observed_unix: 1_780_386_360,
    runtime_status: "applied",
    target_ip: "10.20.0.15",
    updated_at: "2026-06-02T10:00:00Z",
  },
  {
    agent_desired_hash: "a".repeat(64),
    client_id: "agent-fra-02",
    created_at: "2026-06-02T09:10:00Z",
    deleted_at: null,
    desired_hash: "a".repeat(64),
    desired_status: "enabled",
    enabled: true,
    forwarding_enabled: false,
    forgotten_at: null,
    id: "4f000000-0000-4000-8000-000000000002",
    mappings: [
      {
        incoming: { end: 10_010, start: 10_000 },
        target: { end: 20_010, start: 20_000 },
      },
    ],
    masquerade: false,
    name: "IPv6 service range",
    nat_matches: 392,
    nft_version: "nftables v1.1.3",
    observed_hash: "c".repeat(64),
    protocol: "tcp",
    removal_confirmed_at: null,
    revision: 2,
    runtime_error: null,
    runtime_error_code: null,
    runtime_observed_unix: 1_780_386_300,
    runtime_status: "applied_warning",
    target_ip: "2001:db8:20::15",
    updated_at: "2026-06-02T09:58:00Z",
  },
  {
    agent_desired_hash: null,
    client_id: "agent-nyc-03",
    created_at: "2026-06-02T09:20:00Z",
    deleted_at: null,
    desired_hash: null,
    desired_status: "disabled",
    enabled: false,
    forwarding_enabled: null,
    forgotten_at: null,
    id: "4f000000-0000-4000-8000-000000000003",
    mappings: [
      { incoming: { end: 2222, start: 2222 }, target: { end: 22, start: 22 } },
    ],
    masquerade: true,
    name: "Staged SSH alternate",
    nat_matches: 0,
    nft_version: "nftables v1.0.9",
    observed_hash: null,
    protocol: "tcp",
    removal_confirmed_at: null,
    revision: 1,
    runtime_error: null,
    runtime_error_code: null,
    runtime_observed_unix: null,
    runtime_status: "disabled",
    target_ip: "10.30.0.8",
    updated_at: "2026-06-02T09:20:00Z",
  },
  {
    agent_desired_hash: "d".repeat(64),
    client_id: "agent-fra-02",
    created_at: "2026-06-01T14:00:00Z",
    deleted_at: "2026-06-02T09:55:00Z",
    desired_hash: "a".repeat(64),
    desired_status: "removal_pending",
    enabled: false,
    forwarding_enabled: true,
    forgotten_at: null,
    id: "4f000000-0000-4000-8000-000000000004",
    mappings: [
      { incoming: { end: 53, start: 53 }, target: { end: 53, start: 53 } },
    ],
    masquerade: true,
    name: "Retired DNS relay",
    nat_matches: 81,
    nft_version: "nftables v1.1.3",
    observed_hash: "e".repeat(64),
    protocol: "udp",
    removal_confirmed_at: null,
    revision: 5,
    runtime_error: null,
    runtime_error_code: null,
    runtime_observed_unix: 1_780_386_240,
    runtime_status: "removal_pending",
    target_ip: "10.20.0.53",
    updated_at: "2026-06-02T09:55:00Z",
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
    category: "backup",
    client_id: "agent-sfo-01",
    detail: "backup request fixture-backup-01 is execution_failed",
    evidence: { include_config: true, paths: ["/etc"] },
    id: "fleet-alert-backup-agent-sfo-01",
    observed_at: "2026-06-02T10:00:00Z",
    operator_state: "acknowledged",
    severity: "warning",
    muted_until_unix: null,
    escalation_level: 0,
    state_actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    state_reason: "fixture acknowledgement",
    state_updated_at: "2026-06-02T10:00:10Z",
    status: "execution_failed",
    target_id: "fixture-backup-01",
    target_kind: "backup_request",
    title: "Backup request failed",
  },
  {
    category: "traffic",
    client_id: "agent-sfo-01",
    detail: "traffic.cycle.total >= traffic.quota.total * 0.8",
    evidence: {
      policy: { name: "edge-resource-policy" },
      rule: {
        condition_expression:
          "traffic.cycle.total >= traffic.quota.total * 0.8",
      },
      traffic: { cycle_percent: 80.33, reset_day: 14 },
    },
    escalation_level: 0,
    id: "policy-alert:policy-alert-fixture-01",
    muted_until_unix: null,
    observed_at: "2026-06-23T07:31:00Z",
    operator_state: "open",
    severity: "warning",
    state_actor_id: null,
    state_reason: null,
    state_updated_at: null,
    status: "policy_reached",
    target_id: "agent-sfo-01:policy-alert-fixture-01",
    target_kind: "policy_alert",
    title: "Traffic quota threshold reached",
  },
];

const fleetAlertStates = [
  {
    action: "acknowledge",
    alert_id: "fleet-alert-backup-agent-sfo-01",
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
    active_critical_count: 0,
    active_warning_count: 1,
    created_at: "2026-06-02T10:00:00Z",
    created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
    enabled: true,
    id: "fbfbfbfb-1111-4111-8111-111111111111",
    enabled_rule_count: 1,
    incomplete_vps_count: 0,
    last_evaluated_at: "2026-06-02T10:01:00Z",
    matched_vps_count: 1,
    name: "edge-resource-policy",
    notes: "Fixture policy group for edge traffic and resource alerts.",
    rule_count: 1,
    rules: [
      {
        created_at: "2026-06-02T10:00:00Z",
        enabled: true,
        group_id: "fbfbfbfb-1111-4111-8111-111111111111",
        id: "fbfbfbfb-2222-4111-8111-111111111111",
        condition_expression:
          "traffic.cycle.total >= traffic.quota.total * 0.8",
        name: "80% total quota",
        rule_version: 1,
        severity: "warning",
        sort_order: 0,
        traffic_selector: null,
        updated_at: "2026-06-02T10:00:00Z",
        window_secs: 0,
      },
    ],
    selector_expression: "tag:edge",
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: "99999999-aaaa-4bbb-8ccc-000000000001",
  },
];

const vpsRuleValues = [
  {
    client_id: "agent-sfo-01",
    key: "traffic.reset_day",
    parsed_display: "14 UTC",
    source_id: null,
    source_kind: "operator",
    state: "ok",
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: "fixture-admin",
    validation_errors: [],
    value_json: 14,
    value_raw: "14",
  },
  {
    client_id: "agent-sfo-01",
    key: "traffic.quota.total",
    parsed_display: "3000000000000 bytes",
    source_id: null,
    source_kind: "operator",
    state: "ok",
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: "fixture-admin",
    validation_errors: [],
    value_json: 3000000000000,
    value_raw: "3TB",
  },
  {
    client_id: "agent-sfo-01",
    key: "traffic.selectors",
    parsed_display: "2 selectors",
    source_id: null,
    source_kind: "operator",
    state: "ok",
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: "fixture-admin",
    validation_errors: [],
    value_json: {
      selectors: [
        {
          canonical: "eth0+tx",
          direction: "tx",
          interface: "eth0",
          source: "host",
        },
        {
          canonical: "ens3",
          direction: "total",
          interface: "ens3",
          source: "host",
        },
      ],
    },
    value_raw: "eth0+tx,ens3",
  },
];

const trafficAccounting = [
  {
    client_id: "agent-sfo-01",
    cycle_end: "2026-07-14T00:00:00Z",
    cycle_percent: 80.33,
    cycle_start: "2026-06-14T00:00:00Z",
    counter_epochs_seen: 2,
    incomplete_reasons: [],
    last_sample_at: "2026-06-23T07:31:00Z",
    latest_rx_bytes: 510000000000,
    latest_total_bytes: 2410000000000,
    latest_tx_bytes: 1900000000000,
    quota_rx_bytes: null,
    quota_total_bytes: 3000000000000,
    quota_tx_bytes: null,
    reset_day: 14,
    rx_bytes: 510000000000,
    selector_breakdown: [
      {
        cycle_rx_bytes: 0,
        cycle_total_bytes: 1900000000000,
        cycle_tx_bytes: 1900000000000,
        direction: "tx",
        incomplete_reasons: [],
        interface: "eth0",
        latest_rx_bytes: 11200000000000,
        latest_tx_bytes: 2400000000000,
        sample_age_secs: 60,
        source: "host",
        state: "ok",
      },
      {
        cycle_rx_bytes: 300000000000,
        cycle_total_bytes: 420000000000,
        cycle_tx_bytes: 120000000000,
        direction: "total",
        incomplete_reasons: [],
        interface: "ens3",
        latest_rx_bytes: 4100000000000,
        latest_tx_bytes: 3200000000000,
        sample_age_secs: 60,
        source: "host",
        state: "ok",
      },
    ],
    selector_hash: "fixture-selector-hash",
    selectors: ["eth0+tx", "ens3"],
    state: "ok",
    total_bytes: 2410000000000,
    tx_bytes: 1900000000000,
    updated_at: "2026-06-23T07:31:30Z",
  },
];

const policyAlerts = [
  {
    actual_value: 2410000000000,
    category: "traffic",
    client_id: "agent-sfo-01",
    created_at: "2026-06-23T07:31:30Z",
    detail: "traffic.cycle.total reached traffic.quota.total * 0.8",
    id: "policy-alert-fixture-01",
    observed_at: "2026-06-23T07:31:00Z",
    payload: {
      alert: { category: "traffic", severity: "warning" },
      traffic: { cycle_percent: 80.33, reset_day: 14 },
    },
    policy_group_id: "fbfbfbfb-1111-4111-8111-111111111111",
    policy_rule_id: "fbfbfbfb-2222-4111-8111-111111111111",
    severity: "warning",
    threshold_value: 2400000000000,
    title: "Traffic quota threshold reached",
    trigger_generation: 1,
  },
];

const policyDryRunFixture = {
  incomplete_vps_count: 0,
  invalid_rule_count: 0,
  matched_vps: ["agent-sfo-01"],
  matched_vps_count: 1,
  preview_hash:
    "2222222222222222222222222222222222222222222222222222222222222222",
  rule_previews: [
    {
      false_count: 0,
      incomplete_count: 0,
      category: "traffic",
      condition_expression: "traffic.cycle.total >= traffic.quota.total * 0.8",
      rule_name: "80% total quota",
      severity: "warning",
      true_count: 1,
    },
  ],
  validation_errors: [],
};

const fleetAlertNotificationChannels = [
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    categories: ["agent_status", "network"],
    configuration_error: null,
    cooldown_secs: 3600,
    created_at: "2026-06-02T10:00:00Z",
    delivery_kind: "webhook",
    enabled: true,
    id: "fcfcfcfc-1111-4111-8111-111111111111",
    min_severity: "warning",
    name: "edge-webhook-channel",
    notes: null,
    operator_states: ["open", "escalated"],
    scope_kind: "tag",
    scope_value: "edge",
    target: "https://hooks.example/vpsman/fleet",
    updated_at: "2026-06-02T10:00:00Z",
  },
] satisfies FleetAlertNotificationChannelRecord[];

const fleetAlertNotifications = [
  {
    alert_category: "network",
    alert_id: "fleet-alert-network-agent-fra-02-tun0",
    attempt_count: 1,
    channel_id: "fcfcfcfc-1111-4111-8111-111111111111",
    channel_name: "edge-webhook-channel",
    created_at: "2026-06-02T10:01:00Z",
    delivery_kind: "webhook",
    error: null,
    id: "fdfdfdfd-1111-4111-8111-111111111111",
    last_attempt_at: "2026-06-02T10:01:05Z",
    next_attempt_at: "2026-06-02T10:06:05Z",
    review_preview_hash:
      "1111111111111111111111111111111111111111111111111111111111111111",
    status: "failed",
    target: "https://hooks.example/vpsman/fleet",
    updated_at: "2026-06-02T10:01:05Z",
  },
  {
    alert_category: "resource",
    alert_id: "fleet-alert-resource-agent-sfo-01-cpu",
    alert_severity: "warning",
    attempt_count: 0,
    channel_id: "fcfcfcfc-1111-4111-8111-111111111111",
    channel_name: "edge-webhook-channel",
    cooldown_until_unix: 0,
    created_at: "2026-06-02T10:02:00Z",
    dedupe_key: "edge-webhook-channel:fleet-alert-resource-agent-sfo-01-cpu",
    delivery_kind: "webhook",
    delivered_at: null,
    error: null,
    id: "fdfdfdfd-2222-4222-8222-222222222222",
    last_attempt_at: null,
    next_attempt_at: null,
    payload: { alert_id: "fleet-alert-resource-agent-sfo-01-cpu" },
    review_preview_hash: null,
    status: "queued",
    target: "https://hooks.example/vpsman/fleet",
    updated_at: "2026-06-02T10:02:00Z",
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
    signing_secret_set: true,
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
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    attempt_count: 1,
    cooldown_until_unix: 0,
    created_at: "2026-06-02T10:02:00Z",
    dedupe_key: "edge-interval-webhook:interval.30sec:q2-edge-failed",
    delivered_at: null,
    error: "fixture receiver returned 503",
    event_id: "q2-edge-failed",
    event_kind: "interval.30sec",
    id: "abababab-2222-4222-8222-222222222222",
    last_attempt_at: "2026-06-02T10:02:04Z",
    matched_vps: [],
    message: "edge-interval-webhook failed test",
    next_attempt_at: "2026-06-02T10:07:04Z",
    payload: { event_kind: "interval.30sec", matched_count: 0 },
    review_preview_hash: null,
    rule_id: "fefefefe-1111-4111-8111-111111111111",
    rule_name: "edge-interval-webhook",
    status: "failed",
    target: "https://hooks.example/vpsman/edge-capacity",
  },
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    attempt_count: 0,
    cooldown_until_unix: 0,
    created_at: "2026-06-02T10:03:00Z",
    dedupe_key: "edge-interval-webhook:interval.30sec:q2-edge-queued",
    delivered_at: null,
    error: null,
    event_id: "q2-edge-queued",
    event_kind: "interval.30sec",
    id: "abababab-3333-4333-8333-333333333333",
    last_attempt_at: null,
    matched_vps: [],
    message: "edge-interval-webhook queued test",
    next_attempt_at: null,
    payload: { event_kind: "interval.30sec", matched_count: 0 },
    review_preview_hash: null,
    rule_id: "fefefefe-1111-4111-8111-111111111111",
    rule_name: "edge-interval-webhook",
    status: "queued",
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
    domain: "system_metric_rollups",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture system metric retention",
    prune_limit: 2000,
    retention_days: 3650,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "telemetry_rollups",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture telemetry rollup retention",
    prune_limit: 2000,
    retention_days: 3650,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "telemetry_network_rates",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture network-rate telemetry retention",
    prune_limit: 2000,
    retention_days: 3650,
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
  {
    built_in_default: true,
    domain: "network_observations",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture network observation retention",
    prune_limit: 5000,
    retention_days: 180,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "topology_history",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture topology retention",
    prune_limit: 2000,
    retention_days: 180,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "client_status_history",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture client lifecycle retention",
    prune_limit: 2000,
    retention_days: 365,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
  {
    built_in_default: true,
    domain: "gateway_sessions",
    enabled: true,
    export_enabled: true,
    metadata_only: false,
    notes: "fixture gateway session retention",
    prune_limit: 2000,
    retention_days: 365,
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: null,
  },
];

const auditLogs = [
  {
    action: "job.dispatch_requested",
    actor_id: null,
    command_hash: "a".repeat(64),
    created_at: "2026-05-31T11:00:04Z",
    id: "audit-job-dispatch-scheduled-0001",
    metadata: {
      command_type: "scheduled_shell_argv",
      component: "system scheduler",
      job_id: "77777777-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      origin_kind: "control_plane",
      privileged: false,
      result: "requested",
      target_client_ids: ["agent-sfo-01", "agent-fra-02"],
      source_schedule_id: "51515151-6161-4717-8abc-defdefdefdef",
      target_count: 2,
    },
    target: "api:/api/v1/jobs",
  },
  {
    action: "job.dispatch_requested",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: "7".repeat(64),
    created_at: "2026-05-31T10:08:55Z",
    id: "audit-job-dispatch-network-0001",
    metadata: {
      command_type: "network_speed_test",
      component: "job-submission-controller",
      job_id: "77777777-aaaa-4bbb-8ccc-dddddddddddd",
      operator_role: "admin",
      operator_username: "console-admin",
      origin_kind: "operator_request",
      privileged: true,
      result: "requested",
      target_client_ids: ["agent-sfo-01", "agent-fra-02"],
      operator_session_id: "88888888-aaaa-4bbb-8ccc-000000000001",
      target_count: 2,
    },
    target: "api:/api/v1/jobs",
  },
  {
    action: "job.dispatch_requested",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: "7".repeat(64),
    created_at: "2026-05-31T09:08:55Z",
    id: "audit-job-dispatch-repeated-payload-0001",
    metadata: {
      command_type: "network_speed_test",
      component: "job-submission-controller",
      job_id: "11111111-2222-4333-8444-555555555555",
      operator_role: "admin",
      operator_username: "console-admin",
      origin_kind: "operator_request",
      privileged: true,
      result: "requested",
      target_client_ids: ["agent-sfo-01"],
      target_count: 1,
    },
    target: "api:/api/v1/jobs",
  },
  {
    action: "terminal.open",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: "b".repeat(64),
    created_at: "2026-05-31T10:11:50Z",
    id: "audit-terminal-open-0001",
    metadata: {
      accepted: true,
      client_id: "agent-sfo-01",
      operator_session_id: "88888888-aaaa-4bbb-8ccc-000000000001",
      operator_username: "console-admin",
      origin_kind: "operator_request",
      component: "terminal-controller",
      result: "accepted",
      status: "accepted",
      terminal_session_id: "61616161-2222-4333-8444-555555555555",
    },
    target: "terminal:agent-sfo-01/61616161-2222-4333-8444-555555555555",
  },
  {
    action: "terminal.input",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: "c".repeat(64),
    created_at: "2026-05-31T10:12:00Z",
    id: "audit-terminal-input-0001",
    metadata: {
      accepted: true,
      client_id: "agent-sfo-01",
      input_seq: 2,
      job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
      operator_session_id: "88888888-aaaa-4bbb-8ccc-000000000001",
      operator_username: "console-admin",
      origin_kind: "operator_request",
      component: "terminal-controller",
      result: "accepted",
      status: "accepted",
      terminal_session_id: "61616161-2222-4333-8444-555555555555",
    },
    target: "terminal:agent-sfo-01/61616161-2222-4333-8444-555555555555",
  },
  {
    action: "terminal.close",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: "d".repeat(64),
    created_at: "2026-05-31T10:12:30Z",
    id: "audit-terminal-close-0001",
    metadata: {
      accepted: true,
      client_id: "agent-fra-02",
      close_reason: "operator",
      job_id: "71717171-aaaa-4bbb-8ccc-dddddddddddd",
      operator_session_id: "88888888-aaaa-4bbb-8ccc-000000000002",
      operator_username: "console-admin",
      origin_kind: "operator_request",
      component: "terminal-controller",
      result: "accepted",
      status: "closed",
      terminal_session_id: "71717171-2222-4333-8444-555555555555",
    },
    target: "terminal:agent-fra-02/71717171-2222-4333-8444-555555555555",
  },
  {
    action: "privilege.unlock",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: null,
    created_at: "2026-06-02T10:12:00Z",
    id: "audit-privilege-unlock-0001",
    metadata: {
      component: "privilege-verifier",
      operator_role: "admin",
      origin_kind: "authentication",
      remote_ip: "127.0.0.1",
      result: "succeeded",
      operator_session_id: "88888888-aaaa-4bbb-8ccc-000000000001",
      operator_username: "console-admin",
      privilege_scope: "privilege.unlock",
      user_agent: "Playwright (test automation)",
    },
    target: "access/privilege-vault",
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

function hostProcessInventory(clientId: string) {
  return {
    client_id: clientId,
    last_attempt: {
      completed_at: "2026-06-02T10:03:00Z",
      job_id: "51515151-2222-4333-8444-555555555555",
      message: "completed",
      status: "completed",
    },
    observed_at: "2026-06-02T10:03:00Z",
    processes: [
      {
        command: "/usr/sbin/sshd -D -o AuthorizedKeysFile=.ssh/authorized_keys",
        name: "sshd",
        pid: 812,
        ppid: 1,
        rss_kib: 18_432,
        state: "S",
        uid: 0,
      },
      {
        command:
          "/usr/bin/node /srv/dashboard/server.js --listen 127.0.0.1:3000",
        name: "node",
        pid: 4_242,
        ppid: 1,
        rss_kib: 131_072,
        state: "R",
        uid: 1000,
      },
    ],
    source: "/proc",
    source_job_id: "51515151-2222-4333-8444-555555555555",
    truncated: false,
  };
}

export function hostServiceInventory(
  clientId: string,
): HostServiceInventoryRecord {
  return {
    capability: {
      can_enable_disable: true,
      can_inventory: true,
      can_read_logs: true,
      can_start_stop_restart: true,
      enable_backend: "systemctl",
      provider: "systemd" as const,
      provider_version: null,
      reason: null,
      status: "supported" as const,
    },
    client_id: clientId,
    last_attempt: {
      completed_at: "2026-06-02T10:04:00Z",
      job_id: "52525252-2222-4333-8444-555555555555",
      message: "completed",
      status: "completed",
    },
    observed_at: "2026-06-02T10:04:00Z",
    services: [
      {
        active_state: "active",
        description: "OpenSSH server daemon",
        enabled_state: "enabled",
        load_state: "loaded",
        name: "sshd.service",
        state_reason: null,
        sub_state: "running",
      },
      {
        active_state: "failed",
        description: "Example background worker",
        enabled_state: "disabled",
        load_state: "loaded",
        name: "example-worker.service",
        state_reason: "Result: exit-code",
        sub_state: "failed",
      },
      {
        active_state: "inactive",
        description: "One-shot maintenance task",
        enabled_state: "static",
        load_state: "loaded",
        name: "maintenance.service",
        state_reason: null,
        sub_state: "dead",
      },
    ],
    source_job_id: "52525252-2222-4333-8444-555555555555",
    truncated: false,
  };
}

export function hostStorageInventory(
  clientId: string,
): HostStorageInventoryRecord {
  return {
    capability: {
      available_columns: [
        "NAME",
        "KNAME",
        "PKNAME",
        "TYPE",
        "SIZE",
        "FSTYPE",
        "FSVER",
        "LABEL",
        "UUID",
        "MOUNTPOINT",
        "FSAVAIL",
        "FSUSE%",
        "RO",
        "RM",
        "MODEL",
        "SERIAL",
        "TRAN",
        "MAJ:MIN",
      ],
      can_report_filesystem_usage: true,
      provider: "lsblk_json",
      provider_version: "lsblk from util-linux 2.39.3",
      reason: null,
      status: "supported",
    },
    client_id: clientId,
    devices: [
      {
        device_type: "disk",
        filesystem_available_bytes: null,
        filesystem_type: null,
        filesystem_used_percent: null,
        filesystem_version: null,
        kernel_name: "vda",
        label: null,
        major_minor: "252:0",
        model: "Cloud Block Device",
        mount_points: [],
        name: "vda",
        parent_path: null,
        path: "/dev/vda",
        read_only: false,
        removable: false,
        serial: "cloud-root-001",
        size_bytes: 107_374_182_400,
        transport: "virtio",
        uuid: null,
      },
      {
        device_type: "part",
        filesystem_available_bytes: 30_064_771_072,
        filesystem_type: "ext4",
        filesystem_used_percent: 72,
        filesystem_version: "1.0",
        kernel_name: "vda1",
        label: "rootfs",
        major_minor: "252:1",
        model: null,
        mount_points: ["/"],
        name: "vda1",
        parent_path: "/dev/vda",
        path: "/dev/vda1",
        read_only: false,
        removable: false,
        serial: null,
        size_bytes: 107_363_696_640,
        transport: null,
        uuid: "11111111-2222-4333-8444-555555555555",
      },
      {
        device_type: "disk",
        filesystem_available_bytes: 48_318_382_080,
        filesystem_type: "xfs",
        filesystem_used_percent: 91,
        filesystem_version: "5",
        kernel_name: "vdb",
        label: "archive",
        major_minor: "252:16",
        model: "Cloud Archive Volume",
        mount_points: ["/srv/archive"],
        name: "vdb",
        parent_path: null,
        path: "/dev/vdb",
        read_only: true,
        removable: false,
        serial: "cloud-archive-001",
        size_bytes: 536_870_912_000,
        transport: "virtio",
        uuid: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      },
    ],
    devices_truncated: false,
    include_pseudo_mounts: false,
    last_attempt: {
      completed_at: "2026-06-02T10:05:00Z",
      job_id: "53535353-2222-4333-8444-555555555555",
      message: "completed",
      status: "completed",
    },
    mounts: [
      {
        filesystem_type: "ext4",
        major_minor: "252:1",
        mount_id: 36,
        options: ["errors=remount-ro", "relatime", "rw"],
        parent_id: 25,
        pseudo: false,
        read_only: false,
        root: "/",
        source: "/dev/vda1",
        target: "/",
      },
      {
        filesystem_type: "xfs",
        major_minor: "252:16",
        mount_id: 37,
        options: ["attr2", "inode64", "ro"],
        parent_id: 25,
        pseudo: false,
        read_only: true,
        root: "/",
        source: "/dev/vdb",
        target: "/srv/archive",
      },
    ],
    mounts_truncated: false,
    observed_at: "2026-06-02T10:05:00Z",
    source_job_id: "53535353-2222-4333-8444-555555555555",
  };
}

export function hostPackageUpdatePlans(): HostPackageUpdatePlanRecord[] {
  return [
    {
      capability: {
        can_apply: true,
        can_plan_cached: true,
        can_refresh_metadata: true,
        distro_id: "ubuntu",
        distro_version: "22.04",
        provider: "apt",
        reason: null,
        status: "supported",
      },
      client_id: "agent-sfo-01",
      evidence_error: null,
      last_attempt: {
        completed_at: "2026-06-02T10:05:00Z",
        job_id: "53535353-2222-4333-8444-555555555555",
        message: "completed",
        status: "completed",
      },
      metadata_refresh_requested: true,
      metadata_refreshed: true,
      observed_at: "2026-06-02T10:05:00Z",
      packages: [
        {
          architecture: "amd64",
          candidate_version: "1.1.1f-1ubuntu2.22",
          current_version: "1.1.1f-1ubuntu2.21",
          name: "openssl",
          repository: "Ubuntu:focal-updates",
        },
        {
          architecture: "amd64",
          candidate_version: "252.22-1ubuntu1.1",
          current_version: "252.22-1ubuntu1",
          name: "systemd",
          repository: "Ubuntu:jammy-updates",
        },
      ],
      plan_hash: "a".repeat(64),
      reboot_required_before: false,
      source_job_id: "53535353-2222-4333-8444-555555555555",
      truncated: false,
    },
    {
      capability: {
        can_apply: true,
        can_plan_cached: true,
        can_refresh_metadata: false,
        distro_id: "arch",
        distro_version: null,
        provider: "pacman",
        reason:
          "Pacman metadata refresh is unsupported as a separate action because Arch requires it to be followed immediately by a full system upgrade; cached planning and application remain available",
        status: "supported",
      },
      client_id: "agent-fra-02",
      evidence_error: null,
      last_attempt: {
        completed_at: "2026-06-02T09:55:00Z",
        job_id: "54545454-2222-4333-8444-555555555555",
        message: "completed",
        status: "completed",
      },
      metadata_refresh_requested: false,
      metadata_refreshed: false,
      observed_at: "2026-06-02T09:55:00Z",
      packages: [
        {
          architecture: null,
          candidate_version: "6.10.9.arch1-1",
          current_version: "6.10.8.arch1-1",
          name: "linux",
          repository: null,
        },
      ],
      plan_hash: "b".repeat(64),
      reboot_required_before: null,
      source_job_id: "54545454-2222-4333-8444-555555555555",
      truncated: false,
    },
    {
      capability: null,
      client_id: "agent-nyc-03",
      evidence_error: null,
      last_attempt: null,
      metadata_refresh_requested: false,
      metadata_refreshed: false,
      observed_at: null,
      packages: [],
      plan_hash: null,
      reboot_required_before: null,
      source_job_id: null,
      truncated: false,
    },
  ];
}

const runtimeConfigApplyStates = [
  {
    client_id: "agent-sfo-01",
    applied_version: 1780423200,
    applied_content_hash:
      "9f0d2c5fbbf493d83e4e944d7f7d0bb4a6acb54a5a4248b4f66f2d2b98a4a100",
    applied_job_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1",
    applied_at: "2026-06-02T10:02:00Z",
    pending_version: null,
    pending_content_hash: null,
    pending_job_id: null,
    pending_reason: null,
    pending_status: null,
    pending_error: null,
    pending_updated_at: null,
    updated_at: "2026-06-02T10:02:00Z",
  },
  {
    client_id: "agent-fra-02",
    applied_version: 1780423000,
    applied_content_hash:
      "22b2df7fd6f49bb6d518866d77a6f22b6f8e61d2db3e2b2c6f7a12e4d20a0200",
    applied_job_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2",
    applied_at: "2026-06-02T09:58:00Z",
    pending_version: 1780423260,
    pending_content_hash:
      "f0d3b8b3c0e9017c0d5ed95a3a9f83a03b2d48f606b78ab836d6ed0f2ff1f201",
    pending_job_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2",
    pending_reason: "Enable tunnel monitoring defaults",
    pending_status: "queued",
    pending_error: null,
    pending_updated_at: "2026-06-02T10:01:00Z",
    updated_at: "2026-06-02T10:01:00Z",
  },
  {
    client_id: "agent-tyo-03",
    applied_version: null,
    applied_content_hash: null,
    applied_job_id: null,
    applied_at: null,
    pending_version: 1780423265,
    pending_content_hash:
      "f83e776f558e99a2a21c872fb85ebf2b7498055acbc57986602b8a87bdeff303",
    pending_job_id: "cccccccc-cccc-4ccc-8ccc-ccccccccccc3",
    pending_reason: "Enable tunnel monitoring defaults",
    pending_status: "failed",
    pending_error: "target agent lacks root runtime network capability",
    pending_updated_at: "2026-06-02T10:02:30Z",
    updated_at: "2026-06-02T10:02:30Z",
  },
];

const runtimeConfigPatchGenerators = [
  {
    actor_id: null,
    built_in: true,
    category: "Source config",
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
          default:
            "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
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
          default:
            "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
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
  suggested_client_id: "v-1",
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
    follow_symlinks: false,
    note: "fixture backup",
    paths: ["/etc/hostname"],
    payload_hash: "a".repeat(64),
    command_scope: "client:agent-sfo-01",
    status: "artifact_metadata_recorded",
    source_job_id: "77777777-aaaa-4bbb-8ccc-dddddddddddd",
    source_schedule_id: null,
  },
];

const backupArtifacts = [
  {
    client_id: "agent-sfo-01",
    created_at: "2026-05-31T10:01:00Z",
    id: "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
    object_key: `backups/agent-sfo-01/${backupId}.tar`,
    sha256_hex: "b".repeat(64),
    size_bytes: 512,
    status: "active",
    content_available: true,
  },
];

function tunnelRuntimeConfigFixture(clientId: string, enabled: boolean) {
  return {
    client_id: clientId,
    desired: enabled ? "present" : "absent",
    error: null,
    job_id: "4f200000-0000-4000-8000-000000000001",
    status: enabled ? "applied" : "removed",
    updated_at: "2026-05-31T10:09:00Z",
  };
}

export const tunnelPlans = [
  {
    created_at: "2026-05-31T10:03:00Z",
    id: "dddddddd-eeee-4fff-8000-111111111111",
    kind: "gre",
    enabled: true,
    revision: 3,
    left_client_id: "agent-sfo-01",
    left_ospf_status: "verified",
    left_current_ospf_cost: 14,
    left_ospf_job_id: null,
    name: "sfo-fra-gre",
    recommended_ospf_cost: 22,
    right_client_id: "agent-fra-02",
    right_ospf_status: "verified",
    right_current_ospf_cost: 14,
    right_ospf_job_id: null,
    connection_assessment: "automatic",
    connection_assessment_note: null,
    connection_assessed_at: null,
    connection_assessed_by: null,
    left_runtime_config: tunnelRuntimeConfigFixture("agent-sfo-01", true),
    right_runtime_config: tunnelRuntimeConfigFixture("agent-fra-02", true),
    ospf_status: "review_required",
    desired_ospf_cost: 22,
    updated_at: "2026-05-31T10:09:00Z",
    deleted_at: null,
    deleted_by: null,
    deleted_reason: null,
    builtin_credentials: null,
    input: {
      name: "sfo-fra-gre",
      interface_name: "tunab",
      kind: "gre",
      runtime_topology: {
        desired_interfaces: ["tunab"],
        version: "declared:v1",
      },
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_remote_underlay: "203.0.113.20",
      left_local_underlay: "10.0.0.10",
      right_remote_underlay: "198.51.100.10",
      right_local_underlay: "10.0.1.20",
      address_pool_cidr: "10.255.0.0/30",
      reserved_addresses: [],
      ipv4_tunnel: {
        left: "10.255.0.0",
        right: "10.255.0.1",
        prefix_len: 31,
      },
      ipv6_address_pool_cidr: null,
      ipv6_tunnel: null,
      latency_primary_family: "ipv4",
      bandwidth_mbps: 100,
      left_mtu: 1476,
      right_mtu: 1476,
      ospf: {
        mode: "reviewed",
        planned_latency_ms: 14,
        planned_packet_loss_ratio: 0,
        preference: 1,
        policy: {
          latency_weight: 1,
          loss_weight: 400,
          bandwidth_weight: 10,
          preference_bias: 1,
          min_cost: 5,
          max_cost: 65535,
        },
        min_cost_delta: 5,
        healthy_windows: 2,
        left_adapter_template_id: "44444444-4444-4444-8444-444444444444",
        right_adapter_template_id: "55555555-5555-4555-8555-555555555555",
      },
    },
    plan: {
      name: "sfo-fra-gre",
      interface_name: "tunab",
      kind: "gre",
      runtime_topology: {
        desired_interfaces: ["tunab"],
        version: "declared:v1",
      },
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_remote_underlay: "203.0.113.20",
      left_local_underlay: "10.0.0.10",
      right_remote_underlay: "198.51.100.10",
      right_local_underlay: "10.0.1.20",
      left_tunnel_address: "10.255.0.0",
      right_tunnel_address: "10.255.0.1",
      tunnel_prefix_len: 31,
      ipv4_tunnel: {
        left: "10.255.0.0",
        right: "10.255.0.1",
        prefix_len: 31,
      },
      ipv6_tunnel: null,
      latency_primary_family: "ipv4",
      bandwidth_mbps: 100,
      left_mtu: 1476,
      right_mtu: 1476,
      ospf: {
        mode: "reviewed",
        planned_latency_ms: 14,
        planned_packet_loss_ratio: 0,
        preference: 1,
        policy: {
          latency_weight: 1,
          loss_weight: 400,
          bandwidth_weight: 10,
          preference_bias: 1,
          min_cost: 5,
          max_cost: 65535,
        },
        min_cost_delta: 5,
        healthy_windows: 2,
        left_adapter_template_id: "44444444-4444-4444-8444-444444444444",
        right_adapter_template_id: "55555555-5555-4555-8555-555555555555",
      },
      recommended_ospf_cost: 22,
      conflicts: [],
    },
  },
  {
    created_at: "2026-05-31T10:04:00Z",
    id: "eeeeeeee-ffff-4000-8111-222222222222",
    kind: "openvpn",
    enabled: true,
    revision: 1,
    left_client_id: "agent-sfo-01",
    left_ospf_status: "disabled",
    left_current_ospf_cost: null,
    left_ospf_job_id: null,
    name: "external-openvpn-observed",
    recommended_ospf_cost: null,
    right_client_id: "agent-fra-02",
    right_ospf_status: "disabled",
    right_current_ospf_cost: null,
    right_ospf_job_id: null,
    connection_assessment: "connected",
    connection_assessment_note:
      "Application traffic verified; provider blocks ICMP",
    connection_assessed_at: "2026-05-31T10:05:00Z",
    connection_assessed_by: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    left_runtime_config: tunnelRuntimeConfigFixture("agent-sfo-01", true),
    right_runtime_config: tunnelRuntimeConfigFixture("agent-fra-02", true),
    ospf_status: "disabled",
    desired_ospf_cost: null,
    updated_at: "2026-05-31T10:04:00Z",
    deleted_at: null,
    deleted_by: null,
    deleted_reason: null,
    builtin_credentials: null,
    input: {
      name: "external-openvpn-observed",
      interface_name: "ovpn42",
      kind: "openvpn",
      runtime_control: { manager: "external_observed" },
      runtime_topology: {},
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_remote_underlay: "203.0.113.20",
      left_local_underlay: null,
      right_remote_underlay: "198.51.100.10",
      right_local_underlay: null,
      address_pool_cidr: "10.44.0.0/30",
      reserved_addresses: [],
      ipv4_tunnel: {
        left: "10.44.0.0",
        right: "10.44.0.1",
        prefix_len: 31,
      },
      ipv6_address_pool_cidr: null,
      ipv6_tunnel: null,
      latency_primary_family: "ipv4",
      bandwidth_mbps: 100,
      ospf: null,
    },
    plan: {
      name: "external-openvpn-observed",
      interface_name: "ovpn42",
      kind: "openvpn",
      runtime_control: { manager: "external_observed" },
      runtime_topology: {},
      left_client_id: "agent-sfo-01",
      right_client_id: "agent-fra-02",
      left_remote_underlay: "203.0.113.20",
      left_local_underlay: null,
      right_remote_underlay: "198.51.100.10",
      right_local_underlay: null,
      left_tunnel_address: "10.44.0.0",
      right_tunnel_address: "10.44.0.1",
      tunnel_prefix_len: 31,
      ipv4_tunnel: {
        left: "10.44.0.0",
        right: "10.44.0.1",
        prefix_len: 31,
      },
      ipv6_tunnel: null,
      latency_primary_family: "ipv4",
      bandwidth_mbps: 100,
      ospf: null,
      recommended_ospf_cost: null,
      conflicts: [],
    },
  },
];

const networkProbeJobId = "99999999-aaaa-4bbb-8ccc-dddddddddddd";
const networkStatusJobId = "88888888-aaaa-4bbb-8ccc-dddddddddddd";
const externalNetworkStatusJobId = "88888888-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const networkSpeedJobId = "77777777-aaaa-4bbb-8ccc-dddddddddddd";
const rolloutJobId = "55555555-aaaa-4bbb-8ccc-dddddddddddd";

const networkJobs = [
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_type: "shell_argv",
    completed_at: null,
    created_at: "2026-06-02T10:00:00Z",
    id: rolloutJobId,
    max_timeout_secs: 60,
    payload_hash: "5".repeat(64),
    privileged: true,
    source_schedule_id: null,
    status: "running",
    target_count: 3,
  },
  {
    actor_id: null,
    command_type: "scheduled_shell_argv",
    completed_at: "2026-05-31T11:00:09Z",
    created_at: "2026-05-31T11:00:04Z",
    id: "77777777-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    max_timeout_secs: 30,
    payload_hash: "a".repeat(64),
    privileged: false,
    source_schedule_id: "51515151-6161-4717-8abc-defdefdefdef",
    status: "completed",
    target_count: 2,
  },
  {
    actor_id: null,
    command_type: "agent_update",
    completed_at: "2026-05-31T10:10:00Z",
    created_at: "2026-05-31T10:09:55Z",
    id: "66666666-aaaa-4bbb-8ccc-dddddddddddd",
    payload_hash: "6".repeat(64),
    privileged: true,
    source_schedule_id: null,
    status: "completed",
    target_count: 1,
  },
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_type: "network_speed_test",
    completed_at: "2026-05-31T10:09:00Z",
    created_at: "2026-05-31T10:08:55Z",
    id: networkSpeedJobId,
    payload_hash: "7".repeat(64),
    privileged: true,
    source_schedule_id: null,
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
    source_schedule_id: null,
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
    source_schedule_id: null,
    status: "completed",
    target_count: 1,
  },
  {
    actor_id: null,
    command_type: "network_status",
    completed_at: "2026-05-31T10:07:30Z",
    created_at: "2026-05-31T10:07:25Z",
    id: externalNetworkStatusJobId,
    payload_hash: "4".repeat(64),
    privileged: true,
    source_schedule_id: null,
    status: "completed",
    target_count: 1,
  },
];

const jobRollouts: JobRolloutRecord[] = [
  {
    batch_delay_secs: 30,
    batch_size: 1,
    canary_client_ids: ["agent-sfo-01"],
    completed_at: null,
    created_at: "2026-06-02T10:00:00Z",
    current_batch: 1,
    failure_baseline: 0,
    job_id: rolloutJobId,
    max_failures: 0,
    next_batch_at: "2026-06-02T10:01:00Z",
    pause_after_canary: true,
    pause_reason: "canary_review",
    status: "paused",
    targets: [
      {
        batch_index: 0,
        client_id: "agent-sfo-01",
        message: "completed",
        status: "completed",
      },
      {
        batch_index: 1,
        client_id: "agent-fra-02",
        message: null,
        status: "queued",
      },
      {
        batch_index: 2,
        client_id: "agent-nyc-03",
        message: null,
        status: "queued",
      },
    ],
    total_batches: 3,
    updated_at: "2026-06-02T10:01:00Z",
  },
];

const jobApprovals = [
  {
    id: "abababab-1111-4222-8333-444444444444",
    status: "pending",
    job_id: "abababab-2222-4333-8444-555555555555",
    command_type: "shell_argv",
    selector_expression: "tag:provider:alpha && status:online",
    target_client_ids: ["agent-sfo-01", "agent-nyc-03"],
    target_count: 2,
    privileged: true,
    destructive: true,
    force_unprivileged: false,
    max_timeout_secs: 60,
    payload_hash: "c".repeat(64),
    request_fingerprint: "d".repeat(64),
    requester_id: "99999999-aaaa-4bbb-8ccc-000000000002",
    requester_username: "noc-operator",
    requester_role: "operator",
    requested_at: "2026-06-02T10:12:00Z",
    request_reason: "Restart app service during maintenance window",
    risk: "destructive",
    decision_by: null,
    decision_username: null,
    decision_reason: null,
    decided_at: null,
  },
];

const commandTemplates = [
  {
    actor_id: null,
    built_in: true,
    command_type: "shell_argv",
    created_at: "builtin",
    defaults: {
      confirmed: false,
      destructive: false,
      force_unprivileged: false,
      max_timeout_secs: 30,
    },
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
    defaults: { max_timeout_secs: 30 },
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
    cadence_error: null,
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
        scope: "declared_plan_only",
        runtime: {
          manager: "agent_builtin",
          interface: { exists: true, operstate: "up" },
          desired_interfaces: [
            { interface: "tunab", exists: true, operstate: "up" },
          ],
          declared_stale_interfaces: [],
          adapter: null,
          summary: { applied: true, reason: "declared_runtime_matches" },
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
  [externalNetworkStatusJobId]: [
    {
      client_id: "agent-fra-02",
      created_at: "2026-05-31T10:07:30Z",
      data_base64: statusOutput({
        applied: false,
        client_id: "agent-fra-02",
        interface: "ovpn42",
        malformed: false,
        peer_client_id: "agent-sfo-01",
        plan: "external-openvpn-observed",
        scope: "declared_plan_only",
        runtime: {
          manager: "external_observed",
          interface: { exists: true, operstate: "up" },
          desired_interfaces: [],
          declared_stale_interfaces: [],
          adapter: { configured: false, reason: "external_observed" },
          summary: {
            adapter_state: "observed_only",
            drift: false,
            healthy: true,
            manager: "external_observed",
            reasons: [],
            status: "observed",
          },
        },
        side: "right",
        type: "network_status",
      }),
      done: true,
      exit_code: 0,
      job_id: externalNetworkStatusJobId,
      seq: 0,
      stream: "status",
    },
  ],
};

const sfoFraTopologyIdentityHash = "a".repeat(64);
const externalTopologyIdentityHash = "b".repeat(64);

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
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    role: "client",
    seq: 1,
    target: "10.255.0.0:5201",
    topology_identity_hash: sfoFraTopologyIdentityHash,
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
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    role: null,
    seq: 0,
    target: "10.255.0.1",
    topology_identity_hash: sfoFraTopologyIdentityHash,
    throughput_mbps: null,
  },
  {
    bytes: null,
    client_id: "agent-fra-02",
    healthy: true,
    id: "91919191-aaaa-4bbb-8ccc-dddddddddddd",
    interface_name: "ovpn42",
    job_id: externalNetworkStatusJobId,
    kind: "network_status",
    latency_avg_ms: null,
    metadata: {
      applied: false,
      runtime: {
        summary: {
          adapter_state: "observed_only",
          drift: false,
          healthy: true,
          manager: "external_observed",
          reasons: [],
          status: "observed",
        },
      },
    },
    observed_at: "2026-05-31T10:07:30Z",
    packet_loss_ratio: null,
    peer_client_id: "agent-sfo-01",
    plan_id: tunnelPlans[1].id,
    plan_name: "external-openvpn-observed",
    role: null,
    seq: 0,
    target: null,
    topology_identity_hash: externalTopologyIdentityHash,
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
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    sample_count: 2,
    topology_identity_hash: sfoFraTopologyIdentityHash,
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
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    sample_count: 3,
    topology_identity_hash: sfoFraTopologyIdentityHash,
    throughput_avg_mbps: null,
    throughput_max_mbps: null,
  },
];

const topologyGraph = {
  edges: [
    {
      bandwidth_mbps: 100,
      cost_delta: 8,
      degraded_count: 0,
      enabled: true,
      health: "degraded",
      interface_name: "tunab",
      kind: "gre",
      latency_avg_ms: 12.4,
      latency_primary_family: "ipv4",
      latency_series_ms: [13.8, 12.9, 12.4],
      left_client_id: "agent-sfo-01",
      left_observed_at: "2026-05-31T10:02:00Z",
      left_runtime_reason: null,
      left_runtime_state: "healthy",
      left_reachability_reason: null,
      left_reachability_state: "reachable",
      left_tunnel_address: "10.255.0.0",
      ipv4_tunnel: { left: "10.255.0.0", right: "10.255.0.1", prefix_len: 31 },
      ipv6_tunnel: null,
      availability_reasons: [],
      unavailable_client_ids: [],
      adapter_state: "not_applicable",
      desired_missing_count: 0,
      kernel_link_probe_state: "success",
      kernel_namespace_covered: true,
      kernel_neighbor_probe_state: "success",
      kernel_route_probe_state: "success",
      neighbor_state: "healthy",
      packet_loss_avg_ratio: 0.0025,
      plan_id: tunnelPlans[0].id,
      plan_name: "sfo-fra-gre",
      probe_state: "healthy",
      recommended_ospf_cost: 22,
      right_client_id: "agent-fra-02",
      right_observed_at: "2026-05-31T10:02:00Z",
      right_runtime_reason: null,
      right_runtime_state: "healthy",
      right_reachability_reason: "latency_probe_missing_healthy_sample:3/3",
      right_reachability_state: "probe_failed",
      right_tunnel_address: "10.255.0.1",
      routing_state: "healthy",
      runtime_reasons: [],
      runtime_state: "healthy",
      sample_count: 5,
      stale_present_count: 0,
      throughput_avg_mbps: 10.1,
      throughput_max_mbps: 11.8,
      topology_identity_hash: sfoFraTopologyIdentityHash,
      latest_observed_at: "2026-05-31T10:09:00Z",
    },
  ],
  generated_at: "2026-05-31T10:10:00Z",
  nodes: [
    {
      healthy_tunnel_count: 0,
      client_id: "agent-sfo-01",
      degraded_tunnel_count: 1,
      display_name: "edge-sfo-01",
      latest_observed_at: "2026-05-31T10:09:00Z",
      status: "online",
      tags: ["provider:alpha", "country:US"],
      tunnel_count: 1,
    },
    {
      healthy_tunnel_count: 0,
      client_id: "agent-fra-02",
      degraded_tunnel_count: 1,
      display_name: "core-fra-02",
      latest_observed_at: "2026-05-31T10:09:00Z",
      status: "online",
      tags: ["bgp", "routing", "country:DE"],
      tunnel_count: 1,
    },
  ],
};

const ospfRecommendations = [
  {
    configured_bandwidth_mbps: 100,
    confidence: "measured",
    cost_delta: 8,
    degraded_count: 0,
    evidence_summary:
      "12.4 ms avg; 0.25% loss; 10.1 Mbps avg, 11.8 Mbps max; 5 samples; latest 2026-05-31T10:09:00Z",
    effective_bandwidth_mbps: 10,
    interface_name: "tunab",
    latest_observed_at: "2026-05-31T10:09:00Z",
    latency_avg_ms: 12.4,
    left_client_id: "agent-sfo-01",
    packet_loss_avg_ratio: 0.0025,
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    plan_ospf_cost: 14,
    reason: "derived from persisted probe/speed-test trends",
    recommendation_id: "ospf-1234abcd5678ef90",
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
    change_summary:
      "Apply OSPF cost 22 through the two resolved endpoint updaters",
    confidence: "measured",
    control_mode: "reviewed",
    evidence: {
      configured_bandwidth_mbps: 100,
      degraded_count: 0,
      effective_bandwidth_mbps: 10,
      healthy_probe_streak: 3,
      latest_observed_at: "2026-05-31T10:09:00Z",
      latency_avg_ms: 12.4,
      packet_loss_avg_ratio: 0.0025,
      reason: "derived from persisted probe/speed-test trends",
      required_healthy_probe_streak: 2,
      sample_count: 5,
      throughput_avg_mbps: 10.1,
      throughput_max_mbps: 11.8,
    },
    interface_name: "tunab",
    left_client_id: "agent-sfo-01",
    left_updater_source: "configuration_preset",
    left_adapter_template_id: "66666666-6666-4666-8666-666666666666",
    left_adapter_template_name: "FRR OSPF updater",
    left_adapter_definition_hash: "c".repeat(64),
    left_current_ospf_cost: 14,
    left_ospf_status: "verified",
    maximum_cost_delta: 8,
    mutation_mode: "server_issued_adapter_jobs",
    plan_id: tunnelPlans[0].id,
    plan_name: "sfo-fra-gre",
    plan_revision: tunnelPlans[0].revision,
    privilege_required: true,
    recommendation_id: "ospf-1234abcd5678ef90",
    recommended_ospf_cost: 22,
    requires_approval: true,
    right_client_id: "agent-fra-02",
    right_updater_source: "plan_override",
    right_adapter_template_id: "55555555-5555-4555-8555-555555555555",
    right_adapter_template_name: "FRA routing cost",
    right_adapter_definition_hash: "d".repeat(64),
    right_current_ospf_cost: 14,
    right_ospf_status: "verified",
    status: "review_required",
    evidence_summary:
      "12.4 ms avg; 0.25% loss; 10.1 Mbps avg, 11.8 Mbps max; 5 samples; latest 2026-05-31T10:09:00Z",
  },
];

export async function installConsoleApiMock(
  page: Page,
  options: {
    agentListOverride?: typeof agents;
    alertEvidenceSaturated?: boolean;
    alertStateCoverage?: boolean;
    agentDeleteDelayMs?: number;
    agentDeleteFailedClientIds?: string[];
    agentDeleteRequestFailure?: boolean;
    agentDeleteSyncJobIds?: string[];
    auditDetailOverride?: AuditLogRecord;
    auditLogsOverride?: AuditLogRecord[];
    backupPoliciesOverride?: BackupPolicyRecord[];
    backupArtifactsOverride?: typeof backupArtifacts;
    bulkTagMutationDelayMs?: number;
    bulkTagScheduleImpacts?: TagMutationResponse["schedule_impacts"];
    bulkResolveDelayMs?: number;
    bulkResolveFailure?: boolean;
    configurationSourceApplyFailure?: boolean;
    configurationSourceSyncFailure?: boolean;
    dashboardLatestSampleAtOverride?: string;
    dashboardCountsTruncated?: boolean;
    dashboardSummaryOverride?: Partial<typeof dashboardOverview.summary>;
    fileTransferSourceArtifactsOverride?: typeof fileTransferSourceArtifacts;
    fileTransfersOverride?: typeof fileTransfers;
    fleetAlertStateFailure?: boolean;
    fleetAlertNotificationChannelsOverride?: FleetAlertNotificationChannelRecord[];
    recordPagesSaturated?: boolean;
    runtimeConfigApplyFailure?: boolean;
    hostServiceInventoryOverride?: ReturnType<typeof hostServiceInventory>;
    hostStorageInventoryOverride?: ReturnType<typeof hostStorageInventory>;
    hostPackageUpdatePlansOverride?: ReturnType<typeof hostPackageUpdatePlans>;
    jobRolloutsOverride?: JobRolloutRecord[];
    ospfUpdatePlansOverride?: typeof ospfUpdatePlans;
    operatorRoleOverride?: "admin" | "operator" | "viewer";
    operatorAuthEventsOverride?: OperatorAuthEventRecord[];
    privilegeVerificationDelayMs?: number;
    privilegeVerificationFailure?: "denied" | "unavailable";
    schedulesOverride?: ScheduleRecord[];
    telemetryFailurePath?: "network-rates" | "rollups" | "tunnels";
    telemetryNetworkRateScales?: number[];
    terminalSessionsOverride?: typeof terminalSessions;
    totpSetupDelayMs?: number;
    totpSetupOperatorIdOverride?: string;
    totpSetupSwitchSession?: boolean;
    portSpeedRulesDelayMs?: number;
    portSpeedRulesOverride?: VpsRuleValueRecord[];
    vpsRulesApplyDelayMs?: number;
  } = {},
) {
  await page.addInitScript(
    ({
      agentListOverrideFixture,
      agentDeleteDelayMsFixture,
      agentDeleteFailedClientIdsFixture,
      agentDeleteRequestFailureFixture,
      agentDeleteSyncJobIdsFixture,
      agentsFixture,
      agentUpdateReleasesFixture,
      auditDetailFixture,
      auditLogsFixture,
      artifactsFixture,
      backupPoliciesFixture,
      backupsFixture,
      bulkTagMutationDelayMsFixture,
      bulkTagScheduleImpactsFixture,
      bulkResolveDelayMsFixture,
      bulkResolveFailureFixture,
      configurationSourceApplyFailureFixture,
      configurationSourceSyncFailureFixture,
      dashboardOverviewFixture,
      dashboardLatestSampleAtOverrideFixture,
      dashboardCountsTruncatedFixture,
      dashboardSummaryOverrideFixture,
      systemDashboardFixture,
      configurationPresetsFixture,
      configurationSourcesFixture,
      networkAdapterDefinitionsFixture,
      runtimeConfigApplyStatesFixture,
      runtimeConfigApplyFailureFixture,
      runtimeConfigPatchGeneratorsFixture,
      jobCommandTypeByOperationTypeFixture,
      commandTemplatesFixture,
      clientKeyRevocationsFixture,
      keyLifecycleReportFixture,
      fleetAlertNotificationChannelsFixture,
      fleetAlertNotificationsFixture,
      fleetAlertPoliciesFixture,
      fleetAlertStateFailureFixture,
      fleetAlertStatesFixture,
      fleetAlertsFixture,
      policyAlertsFixture,
      policyDryRunFixture,
      portForwardRulesFixture,
      fileTransferSourceArtifactsFixture,
      fileTransfersFixture,
      historyRetentionPoliciesFixture,
      hostProcessInventoryFixture,
      hostPackageUpdatePlansFixture,
      hostServiceInventoryFixture,
      hostStorageInventoryFixture,
      jobApprovalsFixture,
      jobRolloutsFixture,
      jobOutputsFixture,
      jobsFixture,
      networkObservationsFixture,
      ospfRecommendationsFixture,
      ospfUpdatePlansFixture,
      networkTrendsFixture,
      operatorPreferencesFixture,
      operatorAuthEventsFixture,
      operatorRoleOverrideFixture,
      privilegeVerificationDelayMsFixture,
      privilegeVerificationFailureFixture,
      processSupervisorInventoryFixture,
      schedulesFixture,
      summaryFixture,
      suiteConfigRedactedFixture,
      suiteConfigTomlFixture,
      suiteConfigValidationFixture,
      tagsFixture,
      telemetryFailurePathFixture,
      telemetryNetworkRateScalesFixture,
      terminalSessionsFixture,
      totpSetupDelayMsFixture,
      totpSetupOperatorIdOverrideFixture,
      totpSetupSwitchSessionFixture,
      topologyGraphFixture,
      trafficAccountingFixture,
      tunnelPlansFixture,
      portSpeedRulesDelayMsFixture,
      vpsRulesApplyDelayMsFixture,
      vpsRuleValuesFixture,
      webhookDeliveriesFixture,
      webhookRulesFixture,
    }) => {
      const originalFetch = window.fetch.bind(window);
      const runtimeTunnelConfig = (clientId: string, enabled: boolean) => ({
        client_id: clientId,
        desired: enabled ? "present" : "absent",
        error: null,
        job_id: enabled ? "4f100000-0000-4000-8000-000000000001" : null,
        status: enabled ? "queued" : "removed",
        updated_at: "2026-05-31T10:09:00Z",
      });
      const runtimeTunnelDispatch = (
        leftClientId: string,
        rightClientId: string,
      ) =>
        [leftClientId, rightClientId].map((clientId, index) => ({
          client_id: clientId,
          error: null,
          job_id: `4f200000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
          status: "queued",
        }));
      const setRuntimeTunnelConfig = (
        plan: Record<string, unknown>,
        enabled: boolean,
      ) => {
        const leftClientId = String(plan.left_client_id ?? "");
        const rightClientId = String(plan.right_client_id ?? "");
        plan.left_runtime_config = runtimeTunnelConfig(leftClientId, enabled);
        plan.right_runtime_config = runtimeTunnelConfig(rightClientId, enabled);
        return runtimeTunnelDispatch(leftClientId, rightClientId);
      };
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
      let mutableConfigurationPresets = configurationPresetsFixture.map(
        (record) => ({
          ...record,
          definition: structuredClone(record.definition),
        }),
      );
      let mutableConfigurationSources = configurationSourcesFixture.map(
        (record) => ({
          ...record,
          readiness: {
            ...record.readiness,
            evidence: structuredClone(record.readiness.evidence),
          },
          runtime_sync: { ...record.runtime_sync },
        }),
      );
      let mutableNetworkAdapterDefinitions =
        networkAdapterDefinitionsFixture.map((record) => ({
          ...record,
          definition: structuredClone(record.definition),
        }));
      const deletedAgentIds = new Set<string>();
      const deletedTunnelPlanIds = new Set<string>();
      let mutablePortForwardRules = portForwardRulesFixture.map((rule) => ({
        ...rule,
        mappings: rule.mappings.map((mapping) => ({
          incoming: { ...mapping.incoming },
          target: { ...mapping.target },
        })),
      }));
      let telemetryNetworkRateRequestCount = 0;
      let monitoringCardsRequestCount = 0;
      const mutableHostPackageUpdatePlans = hostPackageUpdatePlansFixture.map(
        (plan) => ({
          ...plan,
          capability: plan.capability ? { ...plan.capability } : null,
          packages: plan.packages.map((item) => ({ ...item })),
        }),
      );
      const hostStorageStorageKey = "__vpsmanTestHostStorage";
      const persistedHostStorage = window.sessionStorage.getItem(
        hostStorageStorageKey,
      );
      if (persistedHostStorage) {
        Object.assign(
          hostStorageInventoryFixture,
          JSON.parse(persistedHostStorage),
        );
      }
      const persistHostStorage = () =>
        window.sessionStorage.setItem(
          hostStorageStorageKey,
          JSON.stringify(hostStorageInventoryFixture),
        );
      const rolloutStorageKey = "__vpsmanTestJobRollouts";
      const persistedJobRollouts =
        window.sessionStorage.getItem(rolloutStorageKey);
      let mutableJobRollouts = persistedJobRollouts
        ? (JSON.parse(persistedJobRollouts) as typeof jobRolloutsFixture)
        : jobRolloutsFixture.map((rollout) => ({
            ...rollout,
            canary_client_ids: [...rollout.canary_client_ids],
            targets: rollout.targets.map((target) => ({ ...target })),
          }));
      const persistJobRollouts = () =>
        window.sessionStorage.setItem(
          rolloutStorageKey,
          JSON.stringify(mutableJobRollouts),
        );
      const createdRolloutJobStorageKey = "__vpsmanTestCreatedRolloutJob";
      const persistedCreatedRolloutJob = window.sessionStorage.getItem(
        createdRolloutJobStorageKey,
      );
      if (persistedCreatedRolloutJob) {
        const createdJob = JSON.parse(
          persistedCreatedRolloutJob,
        ) as (typeof jobsFixture)[number];
        if (
          !(jobsFixture as Array<{ id: string }>).some(
            (job) => job.id === createdJob.id,
          )
        ) {
          jobsFixture.unshift(createdJob);
        }
      }
      const backendAgents = () =>
        agentsFixture.filter((agent) => !deletedAgentIds.has(agent.id));
      const dashboardAgents = () =>
        (agentListOverrideFixture ?? agentsFixture).filter(
          (agent) => !deletedAgentIds.has(agent.id),
        );
      const visibleAgents = () => dashboardAgents();
      const visibleTunnelPlans = () =>
        tunnelPlansFixture.filter(
          (plan) =>
            !deletedTunnelPlanIds.has(plan.id) &&
            !deletedAgentIds.has(plan.left_client_id) &&
            !deletedAgentIds.has(plan.right_client_id),
        );
      const visibleTopologyGraph = () => {
        const visiblePlanIds = new Set(
          visibleTunnelPlans().map((plan) => plan.id),
        );
        const edges = topologyGraphFixture.edges.filter((edge) =>
          visiblePlanIds.has(edge.plan_id),
        );
        const visibleNodeIds = new Set(
          edges.flatMap((edge) => [edge.left_client_id, edge.right_client_id]),
        );
        return {
          ...topologyGraphFixture,
          edges,
          nodes: topologyGraphFixture.nodes.filter((node) =>
            visibleNodeIds.has(node.client_id),
          ),
        };
      };
      const requests = {
        backupArtifactHandoffs: [] as unknown[],
        backupPolicies: [] as unknown[],
        backupPolicyUpdates: [] as unknown[],
        backupPolicyPrunes: [] as unknown[],
        agentDeletes: [] as unknown[],
        artifactCleanupJobs: [] as unknown[],
        artifactCleanupPreviews: [] as unknown[],
        bulkTagMutations: [] as unknown[],
        tagDeletes: [] as unknown[],
        bulkResolve: [] as unknown[],
        runtimeConfigPatches: [] as unknown[],
        configurationPresetMutations: [] as unknown[],
        configurationSourceOverrides: [] as unknown[],
        effectiveConfigurationReads: [] as unknown[],
        networkAdapterMutations: [] as unknown[],
        runtimeConfigPatchGenerators: [] as unknown[],
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
        jobApprovals: [] as unknown[],
        jobApprovalDecisions: [] as unknown[],
        jobRolloutActions: [] as unknown[],
        jobOutputComparisons: [] as unknown[],
        commandTemplates: [] as unknown[],
        migrationLinks: [] as unknown[],
        operatorActions: [] as unknown[],
        operatorPreferences: [] as unknown[],
        privilegeVerifications: [] as unknown[],
        portForwardRules: [] as unknown[],
        restorePlans: [] as unknown[],
        scheduleActions: [] as unknown[],
        schedules: [] as unknown[],
        suiteConfigs: [] as unknown[],
        suiteConfigReads: 0,
        terminalControlAcks: [] as unknown[],
        terminalControls: [] as unknown[],
        totpSetups: [] as unknown[],
        tunnelPlanAllocations: [] as unknown[],
        tunnelPlanEnabledMutations: [] as unknown[],
        tunnelPlanConnectionAssessments: [] as unknown[],
        tunnelPlanDeletes: [] as unknown[],
        tunnelPlanOspfCostUpdates: [] as unknown[],
        tunnelPlanOspfStatusChecks: [] as unknown[],
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
      const fixtureTotpSecret = "JBSWY3DPEHPK3PXP";
      const decodeFixtureBase32 = (value: string) => {
        const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let bits = 0;
        let buffer = 0;
        const bytes: number[] = [];
        for (const character of value.replace(/=+$/u, "").toUpperCase()) {
          const digit = alphabet.indexOf(character);
          if (digit < 0) {
            throw new Error("Invalid fixture TOTP secret");
          }
          buffer = (buffer << 5) | digit;
          bits += 5;
          if (bits >= 8) {
            bits -= 8;
            bytes.push((buffer >>> bits) & 0xff);
          }
        }
        return new Uint8Array(bytes);
      };
      const fixtureTotpCode = async () => {
        let counter = Math.floor(Date.now() / 30_000);
        const counterBytes = new Uint8Array(8);
        for (let index = counterBytes.length - 1; index >= 0; index -= 1) {
          counterBytes[index] = counter & 0xff;
          counter = Math.floor(counter / 256);
        }
        const key = await crypto.subtle.importKey(
          "raw",
          decodeFixtureBase32(fixtureTotpSecret),
          { hash: "SHA-1", name: "HMAC" },
          false,
          ["sign"],
        );
        const digest = new Uint8Array(
          await crypto.subtle.sign("HMAC", key, counterBytes),
        );
        const offset = digest[digest.length - 1] & 0x0f;
        const binary =
          ((digest[offset] & 0x7f) << 24) |
          ((digest[offset + 1] & 0xff) << 16) |
          ((digest[offset + 2] & 0xff) << 8) |
          (digest[offset + 3] & 0xff);
        return String(binary % 1_000_000).padStart(6, "0");
      };
      Object.defineProperty(window, "__vpsmanFixtureTotpCode", {
        configurable: true,
        value: fixtureTotpCode,
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
      const operatorSessions = [
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
          revoked_at: null as string | null,
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
          revoked_at: null as string | null,
        },
      ];
      const operatorView = (record: (typeof operatorRecords)[number]) => ({
        ...record,
        preferences: currentOperatorPreferences,
      });
      const currentOperatorRecord =
        operatorRecords.find(
          (record) => record.role === operatorRoleOverrideFixture,
        ) ?? operatorRecords[0];
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
      const currentJobApprovals = (
        jobApprovalsFixture as Array<Record<string, unknown>>
      ).map((approval) => ({ ...approval }));
      const serverJobsFixture: Array<Record<string, unknown>> = [];
      const commandTypeForOperation = (
        operation: Record<string, unknown> | undefined,
      ): string | null => {
        if (!operation) {
          return null;
        }
        const operationType =
          typeof operation.type === "string" ? operation.type : "shell";
        if (operationType === "shell") {
          return operation.pty ? "shell_pty" : "shell_argv";
        }
        const commandType = (
          jobCommandTypeByOperationTypeFixture as Record<string, string>
        )[operationType];
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
          const matchesTag = tags.some(
            (tag) =>
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
        cadence_error: schedule.cadence_error ?? null,
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
        operation:
          "operation" in schedule
            ? schedule.operation
            : {
                argv: ["uptime"],
                pty: false,
                type: "shell",
              },
        operation_error: schedule.operation_error ?? null,
        operation_payload_hash: schedule.operation_payload_hash,
        retry_delay_secs: schedule.retry_delay_secs ?? 300,
        selector_expression: schedule.selector_expression ?? "id:*",
        target_client_ids: Array.isArray(schedule.target_client_ids)
          ? schedule.target_client_ids
          : scheduleTargetIdsFromSelector(
              schedule.selector_expression ?? "id:*",
            ),
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
      const approvalDecisionResponse = (
        approval: Record<string, unknown>,
        job: Record<string, unknown> | null,
      ) => jsonResponse({ approval, job });
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
          matchedAgents.length > 0
            ? matchedAgents
            : visibleAgents().slice(0, 2);
        const ruleName =
          typeof request.name === "string" && request.name.trim()
            ? request.name.trim()
            : (webhookRulesFixture[0]?.name ?? "webhook-rule");
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
            : (webhookRulesFixture[0]?.target ??
              "https://hooks.example/vpsman");
        return {
          actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
          attempt_count:
            status === "queued" || status === "matched_dry_run" ? 0 : 1,
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
              : (webhookRulesFixture[0]?.id ??
                "fefefefe-1111-4111-8111-111111111111"),
          rule_name: ruleName,
          status,
          target,
        };
      };
      const withReviewPreviewHash = (
        delivery: Record<string, unknown>,
        reviewPreviewHash: string | null,
      ) =>
        reviewPreviewHash
          ? { ...delivery, review_preview_hash: reviewPreviewHash }
          : delivery;

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
      const asFixtureRecord = (
        value: unknown,
      ): Record<string, unknown> | null =>
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
      const renderPatchGeneratorBodyFixture = (
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
        return backendAgents()
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
          )[jobId] ??
          [];
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
          )[jobId] ??
          [];
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
      const buildVpsRulesPreview = (body: Record<string, unknown>) => {
        const operation = body.operation === "unset" ? "unset" : "upsert";
        const keys =
          operation === "unset"
            ? ((body.keys as string[] | undefined) ?? ["traffic.quota.total"])
            : Object.keys(
                (body.values as Record<string, string> | undefined) ?? {
                  "traffic.reset_day": "14",
                },
              );
        const values =
          (body.values as Record<string, string> | undefined) ?? {};
        const changes = keys.map((key) => {
          const after = operation === "unset" ? null : (values[key] ?? "14");
          const validationErrors =
            key === "billing.price" && after?.endsWith("/w")
              ? ["billing_plan_period_invalid"]
              : [];
          return {
            action: operation === "unset" ? "unset" : "set",
            after,
            before:
              vpsRuleValuesFixture.find((row) => row.key === key)?.value_raw ??
              null,
            client_id: "agent-sfo-01",
            display_name: "edge-sfo-01",
            key,
            validation: validationErrors.length > 0 ? "invalid" : "ok",
            validation_errors: validationErrors,
          };
        });
        return {
          changed_row_count: changes.length,
          changes,
          invalid_row_count: changes.filter(
            (change) => change.validation !== "ok",
          ).length,
          matched_vps_count: 1,
          preview_hash:
            "3333333333333333333333333333333333333333333333333333333333333333",
        };
      };
      const monitoringDetailFixture = (clientId: string) => {
        const client =
          agentsFixture.find((candidate) => candidate.id === clientId) ??
          agentsFixture[0]!;
        const starts = [
          "2026-06-23T07:20:00Z",
          "2026-06-23T07:25:00Z",
          "2026-06-23T07:30:00Z",
        ];
        const resources = starts.map((bucketStart, index) => ({
          bucket_secs: 60,
          bucket_start: bucketStart,
          client_id: client.id,
          connections_observed_at: bucketStart,
          connections_sample_count: 1,
          cpu_cores_max: 4,
          cpu_load_1_avg: [0.72, 1.08, 0.84][index],
          cpu_load_1_max: [0.9, 1.24, 1.02][index],
          cpu_load_5_avg: [0.65, 0.82, 0.79][index],
          cpu_load_5_max: [0.78, 0.94, 0.88][index],
          cpu_load_15_avg: [0.58, 0.68, 0.71][index],
          cpu_load_15_max: [0.66, 0.75, 0.79][index],
          cpu_usage_avg: [0.22, 0.37, 0.29][index],
          cpu_usage_sample_count: 1,
          disk_available_bytes_avg: [72, 71.8, 71.7][index] * 1_000_000_000,
          disk_available_bytes_min: [71.9, 71.7, 71.6][index] * 1_000_000_000,
          disk_total_bytes_max: 120_000_000_000,
          disk_used_ratio_avg: 1 - [72, 71.8, 71.7][index] / 120,
          disk_used_ratio_max: 1 - [71.9, 71.7, 71.6][index] / 120,
          latest_observed_at: bucketStart,
          memory_available_bytes_avg: [6.4, 6.1, 6.25][index] * 1_000_000_000,
          memory_available_bytes_min: [6.2, 5.9, 6.1][index] * 1_000_000_000,
          memory_total_bytes_max: 8_000_000_000,
          memory_used_ratio_avg: 1 - [6.4, 6.1, 6.25][index] / 8,
          memory_used_ratio_max: 1 - [6.2, 5.9, 6.1][index] / 8,
          network_rx_bytes_max: [18, 18.4, 18.9][index] * 1_000_000_000,
          network_tx_bytes_max: [8, 8.2, 8.45][index] * 1_000_000_000,
          sample_count: 1,
          swap_available_bytes_avg: null,
          swap_available_bytes_min: null,
          swap_sample_count: 0,
          swap_total_bytes_max: null,
          swap_used_ratio_avg: null,
          swap_used_ratio_max: null,
          tcp_sockets_latest: [34, 41, 38][index],
          udp_sockets_latest: [5, 6, 5][index],
          updated_at: bucketStart,
        }));
        const network = starts.map((bucketStart, index) => ({
          bucket_secs: 60,
          bucket_start: bucketStart,
          client_id: client.id,
          interface: "eth0",
          rx_bps_avg: [640_000, 1_280_000, 920_000][index],
          rx_bytes_avg: [18, 18.4, 18.9][index] * 1_000_000_000,
          rx_bytes_delta: [4_800_000, 9_600_000, 6_900_000][index],
          sample_count: 1,
          tx_bps_avg: [320_000, 510_000, 420_000][index],
          tx_bytes_avg: [8, 8.2, 8.45][index] * 1_000_000_000,
          tx_bytes_delta: [2_400_000, 3_825_000, 3_150_000][index],
          updated_at: bucketStart,
        }));
        const pingTargets = [
          {
            checked_at: starts[2],
            enabled: true,
            generation: 3,
            latency_avg_ms: 22.4,
            loss_ratio: 0,
            reason: null,
            state: "ok",
            status: "ok",
            target_id: "51515151-1111-4111-8111-111111111111",
            target_name: "Singapore gateway",
          },
          {
            checked_at: starts[2],
            enabled: true,
            generation: 2,
            latency_avg_ms: 31.8,
            loss_ratio: 0.03,
            reason: "One of three probes timed out",
            state: "degraded",
            status: "degraded",
            target_id: "52525252-2222-4222-8222-222222222222",
            target_name: "Cloudflare DNS",
          },
        ];
        const ping = pingTargets.flatMap((target, targetIndex) =>
          starts.map((bucketStart, index) => ({
            bucket_secs: 60,
            bucket_start: bucketStart,
            client_id: client.id,
            generation: target.generation,
            is_primary: targetIndex === 0,
            latency_avg_ms:
              targetIndex === 0
                ? [24.2, 21.7, 22.4][index]
                : [29.6, null, 31.8][index],
            latency_max_ms:
              targetIndex === 0
                ? [25.3, 23.1, 24.0][index]
                : [31.2, null, 34.5][index],
            latency_min_ms:
              targetIndex === 0
                ? [23.4, 20.9, 21.5][index]
                : [28.4, null, 29.7][index],
            latest_checked_at: bucketStart,
            latest_reason:
              targetIndex === 1 && index === 1
                ? "Probe timeout"
                : target.reason,
            latest_status:
              targetIndex === 1 && index === 1 ? "timeout" : target.status,
            loss_ratio_avg: targetIndex === 0 ? 0 : [0, 1, 0.03][index],
            loss_ratio_max: targetIndex === 0 ? 0 : [0, 1, 0.09][index],
            sample_count: 3,
            success_count:
              targetIndex === 1 && index === 1 ? 0 : targetIndex === 1 ? 2 : 3,
            target_id: target.target_id,
            target_name: target.target_name,
          })),
        );
        return {
          client,
          network,
          ping,
          ping_targets: pingTargets,
          primary_ping: pingTargets[0],
          range: {
            end_unix: Date.parse(starts[2]) / 1_000,
            points: 11,
            source: "minute",
            start_unix: Date.parse(starts[0]) / 1_000,
            step_secs: 60,
            window: "15m",
          },
          resources,
          system_information: null,
          traffic:
            trafficAccountingFixture.find(
              (row) => row.client_id === client.id,
            ) ?? trafficAccountingFixture[0],
          traffic_history: starts.map((bucketStart, index) => ({
            bucket_secs: 60,
            bucket_start: bucketStart,
            reset_count: 0,
            rx_bytes: [508, 509, 510][index] * 1_000_000_000,
            sample_count: 1,
            total_bytes: [2_406, 2_408, 2_410][index] * 1_000_000_000,
            tx_bytes: [1_898, 1_899, 1_900][index] * 1_000_000_000,
          })),
        };
      };

      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        const pathname = new URL(url, window.location.href).pathname;
        const method = (
          init?.method ?? (input instanceof Request ? input.method : "GET")
        ).toUpperCase();
        const trackedWindow = window as typeof window & {
          __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
        };
        trackedWindow.__vpsmanFetchRequests ??= [];
        trackedWindow.__vpsmanFetchRequests.push({ method, url });
        if (pathname === "/api/v1/dashboard/overview") {
          const params = new URL(url, window.location.href).searchParams;
          const requestedWindow = params.get("window") ?? "1d";
          const requestedGroupBy = params.get("group_by") ?? "labels";
          const requestedResourceMetric =
            params.get("resource_metric") ??
            dashboardOverviewFixture.resource_curve.metric;
          const scopeKind = params.get("scope_kind") ?? "all";
          const scopeValue = params.get("scope_value");
          const startAt = params.get("start_at");
          const endAt = params.get("end_at");
          const scopedAgents = agentsFixture.filter((agent) => {
            if (scopeKind === "all" || !scopeValue) return true;
            if (scopeKind === "client") {
              return (
                agent.id === scopeValue || agent.display_name === scopeValue
              );
            }
            const expected =
              scopeKind === "provider" || scopeKind === "country"
                ? scopeValue.startsWith(`${scopeKind}:`)
                  ? scopeValue
                  : `${scopeKind}:${scopeValue}`
                : scopeValue;
            return agent.tags.includes(expected);
          });
          const scopedClientIds = new Set(
            scopedAgents.map((agent) => agent.id),
          );
          const scopedResourceSeries =
            dashboardOverviewFixture.resource_curve.series
              .filter((series) => scopedClientIds.has(series.client_id))
              .map((series) => {
                if (requestedResourceMetric === "cpu_load") return series;

                const transform =
                  requestedResourceMetric === "memory_used"
                    ? (value: number) =>
                        0.36 + Math.min(Math.max(value, 0) / 4, 0.52)
                    : (value: number) =>
                        0.82 - Math.min(Math.max(value, 0) / 4, 0.62);
                const points = series.points.map((point) => ({
                  ...point,
                  value: transform(point.value),
                }));

                return {
                  ...series,
                  critical_threshold:
                    requestedResourceMetric === "memory_used" ? 0.8 : 0.1,
                  current: points.at(-1)?.value ?? transform(series.current),
                  peak: transform(series.peak),
                  points,
                  threshold_direction:
                    requestedResourceMetric === "memory_used"
                      ? "above"
                      : "below",
                  warning_threshold:
                    requestedResourceMetric === "memory_used" ? 0.7 : 0.2,
                };
              });
          return jsonResponse({
            ...dashboardOverviewFixture,
            group_by: requestedGroupBy,
            label_clusters: dashboardOverviewFixture.label_clusters.map(
              (cluster) => ({
                ...cluster,
                counts_truncated:
                  dashboardCountsTruncatedFixture || cluster.counts_truncated,
              }),
            ),
            operations: {
              ...dashboardOverviewFixture.operations,
              alerts_truncated:
                dashboardCountsTruncatedFixture ||
                dashboardOverviewFixture.operations.alerts_truncated,
              backups_truncated:
                dashboardCountsTruncatedFixture ||
                dashboardOverviewFixture.operations.backups_truncated,
              running_jobs_truncated:
                dashboardCountsTruncatedFixture ||
                dashboardOverviewFixture.operations.running_jobs_truncated,
            },
            resource_curve: {
              ...dashboardOverviewFixture.resource_curve,
              latest_sample_at:
                dashboardLatestSampleAtOverrideFixture ??
                dashboardOverviewFixture.resource_curve.latest_sample_at,
              metric: requestedResourceMetric,
              sampled_clients: scopedResourceSeries.length,
              series: scopedResourceSeries,
            },
            resources: {
              ...dashboardOverviewFixture.resources,
              sampled_clients: scopedResourceSeries.length,
            },
            summary: {
              ...dashboardOverviewFixture.summary,
              ...dashboardSummaryOverrideFixture,
              running_jobs_truncated:
                dashboardCountsTruncatedFixture ||
                dashboardOverviewFixture.summary.running_jobs_truncated,
              warnings_truncated:
                dashboardCountsTruncatedFixture ||
                dashboardOverviewFixture.summary.warnings_truncated,
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
              matched_clients: scopedAgents.length,
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
            requests.suiteConfigReads += 1;
            return jsonResponse({
              effective_require_registered_agent_updates: false,
              exists: true,
              hot_reload_note:
                "API dispatcher limits, gateway-control read timeout, alert thresholds, job-output artifact threshold, update-registration enforcement, gateway runtime timing, and worker tick/schedule/notification/webhook/retention controls are applied by running services after this file changes.",
              path: "config/vpsman.toml",
              redacted: suiteConfigRedactedFixture,
              restart_required_note:
                "Bind addresses, gateway/API URLs and identities, database URL/migration path/pool sizes, secret refs, object-store clients and local object directories, worker identity/once mode, and connect/write timeout changes require service restart.",
              toml: currentSuiteConfigToml,
              validation: suiteConfigValidationFixture,
            });
          }
          if (method === "PUT") {
            const body = (await requestJsonBody(input, init)) as {
              toml?: string;
            };
            requests.suiteConfigs.push(body);
            currentSuiteConfigToml = body.toml ?? currentSuiteConfigToml;
            return jsonResponse({
              audit_status: "applied_recorded",
              changed_keys: ["capacity.api_db_pool"],
              path: "config/vpsman.toml",
              validation: suiteConfigValidationFixture,
            });
          }
        }
        if (pathname === "/api/v1/admin/suite-config/validate") {
          const body = (await requestJsonBody(input, init)) as {
            toml?: string;
          };
          const draftToml = body.toml ?? currentSuiteConfigToml;
          const changedKeys = [
            draftToml.includes("api_db_pool = 40")
              ? "capacity.api_db_pool"
              : null,
            draftToml.includes(
              'tunnel_ipv4_allocation_pool_cidr = "10.250.0.0/16"',
            )
              ? "network.tunnel_ipv4_allocation_pool_cidr"
              : null,
            draftToml.includes(
              'tunnel_ipv6_allocation_pool_cidr = "fd42:250::/64"',
            )
              ? "network.tunnel_ipv6_allocation_pool_cidr"
              : null,
          ].filter((value): value is string => value !== null);
          return jsonResponse({
            changed_keys: changedKeys,
            exists: true,
            old_redacted: suiteConfigRedactedFixture,
            path: "config/vpsman.toml",
            redacted: {
              ...suiteConfigRedactedFixture,
              capacity: {
                ...suiteConfigRedactedFixture.capacity,
                api_db_pool: draftToml.includes("api_db_pool = 40") ? 40 : 32,
              },
              network: {
                ...suiteConfigRedactedFixture.network,
                tunnel_ipv4_allocation_pool_cidr: draftToml.includes(
                  'tunnel_ipv4_allocation_pool_cidr = "10.250.0.0/16"',
                )
                  ? "10.250.0.0/16"
                  : "",
                tunnel_ipv6_allocation_pool_cidr: draftToml.includes(
                  'tunnel_ipv6_allocation_pool_cidr = "fd42:250::/64"',
                )
                  ? "fd42:250::/64"
                  : "",
              },
            },
            validation: suiteConfigValidationFixture,
          });
        }
        if (pathname === "/api/v1/fleet/summary") {
          const currentAgents = visibleAgents();
          const online = currentAgents.filter(
            (agent) => agent.status === "online" && Boolean(agent.last_seen_at),
          ).length;
          const offline = currentAgents.filter((agent) =>
            ["offline", "disconnected"].includes(agent.status),
          ).length;
          const never = currentAgents.filter(
            (agent) => agent.status === "never",
          ).length;
          const stale = currentAgents.filter(
            (agent) => agent.status === "stale",
          ).length;
          const revoked = currentAgents.filter(
            (agent) => agent.status === "revoked",
          ).length;
          const unknown =
            currentAgents.length - online - offline - never - revoked - stale;
          return jsonResponse({
            ...summaryFixture,
            never,
            offline,
            online,
            revoked,
            stale,
            total: currentAgents.length,
            unknown,
            warnings: offline + never + revoked + stale + unknown,
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
          if (fleetAlertStateFailureFixture) {
            return jsonResponse(
              {
                error: "fleet_alert_state_update_failed",
                message: "Simulated fleet alert triage failure.",
              },
              500,
            );
          }
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
        if (pathname === "/api/v1/vps-rules" && method === "GET") {
          const params = new URL(url, window.location.href).searchParams;
          if (
            params.get("key") === "network.port_speed" &&
            portSpeedRulesDelayMsFixture > 0
          ) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, portSpeedRulesDelayMsFixture),
            );
          }
          return jsonResponse(vpsRuleValuesFixture);
        }
        if (pathname === "/api/v1/vps-rules/dry-run" && method === "POST") {
          const body = await readJsonBody(input, init);
          return jsonResponse(
            buildVpsRulesPreview(body as Record<string, unknown>),
          );
        }
        if (
          (pathname === "/api/v1/vps-rules/bulk-upsert" ||
            pathname === "/api/v1/vps-rules/bulk-unset") &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          if (vpsRulesApplyDelayMsFixture > 0) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, vpsRulesApplyDelayMsFixture),
            );
          }
          return jsonResponse(
            buildVpsRulesPreview(body as Record<string, unknown>),
          );
        }
        if (pathname === "/api/v1/traffic-accounting" && method === "GET") {
          return jsonResponse(trafficAccountingFixture);
        }
        if (pathname === "/api/v1/monitoring/cards" && method === "GET") {
          const params = new URL(url, window.location.href).searchParams;
          const offset = Math.max(0, Number(params.get("offset") ?? "0"));
          const limit = Math.max(1, Number(params.get("limit") ?? "1000"));
          const networkScale =
            telemetryNetworkRateScalesFixture[
              Math.min(
                monitoringCardsRequestCount,
                telemetryNetworkRateScalesFixture.length - 1,
              )
            ] ?? 1;
          monitoringCardsRequestCount += 1;
          const items = visibleAgents().map((client) => ({
            billing:
              client.id === "agent-sfo-01"
                ? {
                    currency: "CNY",
                    currency_display: "¥",
                    cycle: "14",
                    disabled: false,
                    display: "29.90 ¥/m",
                    period: "month",
                    period_code: "m",
                    price: "29.90",
                  }
                : null,
            client,
            network:
              client.id === "agent-sfo-01"
                ? [
                    {
                      bucket_secs: 60,
                      bucket_start: "2026-06-05T20:35:00Z",
                      client_id: client.id,
                      interface: "eth0",
                      rx_bps_avg: 19_200_000 * networkScale,
                      rx_bytes_avg: 71_303_168,
                      rx_bytes_delta: 480_000 * networkScale,
                      sample_count: 1,
                      tx_bps_avg: 18_400_000 * networkScale,
                      tx_bytes_avg: 68_157_440,
                      tx_bytes_delta: 458_752 * networkScale,
                      updated_at: "2026-06-05T20:35:00Z",
                    },
                  ]
                : [],
            network_history:
              client.id === "agent-sfo-01"
                ? [
                    [9_600_000, 9_200_000],
                    [14_400_000, 13_800_000],
                    [19_200_000, 18_400_000],
                  ].map(([rxBps, txBps], index) => ({
                    bucket_secs: 60,
                    bucket_start: [
                      "2026-06-05T20:33:00Z",
                      "2026-06-05T20:34:00Z",
                      "2026-06-05T20:35:00Z",
                    ][index],
                    client_id: client.id,
                    interface: "eth0",
                    rx_bps_avg: rxBps * networkScale,
                    rx_bytes_avg: 71_303_168,
                    rx_bytes_delta: 240_000 * (index + 1) * networkScale,
                    sample_count: 1,
                    tx_bps_avg: txBps * networkScale,
                    tx_bytes_avg: 68_157_440,
                    tx_bytes_delta: 230_000 * (index + 1) * networkScale,
                    updated_at: [
                      "2026-06-05T20:33:00Z",
                      "2026-06-05T20:34:00Z",
                      "2026-06-05T20:35:00Z",
                    ][index],
                  }))
                : [],
            port_speed:
              client.id === "agent-sfo-01"
                ? { bps: 1_500_000_000, display: "1.5 Gbps" }
                : null,
            primary_ping: null,
            primary_ping_history: [],
            resource_history: [],
            resources:
              client.id === "agent-sfo-01"
                ? {
                    bucket_secs: 60,
                    bucket_start: "2026-06-05T20:35:00Z",
                    client_id: client.id,
                    connections_observed_at: "2026-06-05T20:35:00Z",
                    connections_sample_count: 1,
                    cpu_cores_max: 4,
                    cpu_load_1_avg: 0.71,
                    cpu_load_1_max: 0.71,
                    cpu_load_5_avg: 0.62,
                    cpu_load_5_max: 0.62,
                    cpu_load_15_avg: 0.55,
                    cpu_load_15_max: 0.55,
                    cpu_usage_avg: 0.24,
                    cpu_usage_sample_count: 1,
                    disk_available_bytes_avg: 40_000_000_000,
                    disk_available_bytes_min: 40_000_000_000,
                    disk_total_bytes_max: 100_000_000_000,
                    disk_used_ratio_avg: 0.6,
                    disk_used_ratio_max: 0.6,
                    latest_observed_at: "2026-06-05T20:35:00Z",
                    memory_available_bytes_avg: 5_000_000_000,
                    memory_available_bytes_min: 5_000_000_000,
                    memory_total_bytes_max: 8_000_000_000,
                    memory_used_ratio_avg: 0.375,
                    memory_used_ratio_max: 0.375,
                    network_rx_bytes_max: 71_303_168,
                    network_tx_bytes_max: 68_157_440,
                    sample_count: 1,
                    swap_available_bytes_avg: 0,
                    swap_available_bytes_min: 0,
                    swap_sample_count: 0,
                    swap_total_bytes_max: 0,
                    swap_used_ratio_avg: null,
                    swap_used_ratio_max: null,
                    tcp_sockets_latest: 37,
                    udp_sockets_latest: 4,
                    updated_at: "2026-06-05T20:35:00Z",
                  }
                : null,
            system_information: null,
            traffic:
              trafficAccountingFixture.find(
                (row) => row.client_id === client.id,
              ) ?? null,
          }));
          const page = items.slice(offset, offset + limit);
          const nextOffset = offset + page.length;
          return jsonResponse({
            items: page,
            limit,
            next_offset: nextOffset < items.length ? nextOffset : null,
            offset,
            total: items.length,
          });
        }
        const clientMonitoringMatch = pathname.match(
          /^\/api\/v1\/clients\/([^/]+)\/monitoring$/,
        );
        if (clientMonitoringMatch && method === "GET") {
          return jsonResponse(
            monitoringDetailFixture(
              decodeURIComponent(clientMonitoringMatch[1]),
            ),
          );
        }
        const trafficAccountingMatch = pathname.match(
          /^\/api\/v1\/traffic-accounting\/([^/]+)$/,
        );
        if (trafficAccountingMatch && method === "GET") {
          const clientId = decodeURIComponent(trafficAccountingMatch[1]);
          return jsonResponse(
            trafficAccountingFixture.find(
              (row) => row.client_id === clientId,
            ) ?? null,
          );
        }
        if (pathname === "/api/v1/policy-alerts" && method === "GET") {
          return jsonResponse(policyAlertsFixture);
        }
        if (pathname === "/api/v1/fleet-alert-policies" && method === "GET") {
          return jsonResponse(fleetAlertPoliciesFixture);
        }
        if (
          pathname === "/api/v1/fleet-alert-policies/dry-run" &&
          method === "POST"
        ) {
          return jsonResponse(policyDryRunFixture);
        }
        if (pathname === "/api/v1/fleet-alert-policies" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.fleetAlertPolicies.push(body);
          const request = body as {
            enabled?: boolean;
            id?: string;
            name?: string;
            notes?: string | null;
            rules?: unknown[];
            selector_expression?: string;
          };
          return jsonResponse({
            active_critical_count: 0,
            active_warning_count: 0,
            created_at: "2026-06-02T10:02:00Z",
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            enabled: request.enabled ?? true,
            enabled_rule_count: request.rules?.length ?? 0,
            id: request.id ?? "eeeeeeee-1111-4111-8111-111111111111",
            incomplete_vps_count: 0,
            last_evaluated_at: "2026-06-02T10:02:00Z",
            matched_vps_count: 1,
            name: request.name ?? "saved-policy",
            notes: request.notes ?? null,
            rule_count: request.rules?.length ?? 0,
            rules: request.rules ?? [],
            selector_expression: request.selector_expression ?? "tag:edge",
            updated_at: "2026-06-02T10:02:00Z",
            updated_by: "99999999-aaaa-4bbb-8ccc-000000000001",
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
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            configuration_error: null,
            created_at: "2026-06-02T10:02:00Z",
            id: "efefefef-1111-4111-8111-111111111111",
            updated_at: "2026-06-02T10:02:00Z",
          });
        }
        const notificationChannelMatch = pathname.match(
          /^\/api\/v1\/fleet-alert-notification-channels\/([^/]+)$/,
        );
        if (notificationChannelMatch && method === "DELETE") {
          const channelId = decodeURIComponent(notificationChannelMatch[1]);
          const channelIndex = (
            fleetAlertNotificationChannelsFixture as Array<
              Record<string, unknown>
            >
          ).findIndex((channel) => channel.id === channelId);
          if (channelIndex >= 0) {
            fleetAlertNotificationChannelsFixture.splice(channelIndex, 1);
          }
          for (const delivery of fleetAlertNotificationsFixture as Array<
            Record<string, unknown>
          >) {
            if (
              delivery.channel_id === channelId &&
              ["queued", "failed", "in_progress"].includes(
                String(delivery.status),
              )
            ) {
              delivery.status = "canceled_disabled";
              delivery.error = "fleet alert notification channel deleted";
              delivery.next_attempt_at = null;
              delivery.delivered_at = null;
            }
          }
          requests.fleetAlertNotificationChannels.push({ delete: channelId });
          return jsonResponse({ deleted: true, id: channelId });
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
          return jsonResponse(
            fleetAlertNotificationsFixture.map(
              (delivery: Record<string, unknown>) => ({
                ...delivery,
                review_preview_hash: (body as { dry_run?: boolean } | null)
                  ?.dry_run
                  ? "1111111111111111111111111111111111111111111111111111111111111111"
                  : delivery.review_preview_hash,
              }),
            ),
          );
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
                review_preview_hash: (body as { dry_run?: boolean } | null)
                  ?.dry_run
                  ? "2222222222222222222222222222222222222222222222222222222222222222"
                  : delivery.review_preview_hash,
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
          const body = (await readJsonBody(input, init)) as Record<
            string,
            unknown
          >;
          requests.webhookRules.push(body);
          const existingRule = webhookRulesFixture.find(
            (rule: Record<string, unknown>) => rule.id === body.id,
          );
          const nextSecret = body.signing_secret;
          const signingSecretSet = body.clear_signing_secret
            ? false
            : typeof nextSecret === "string" && nextSecret.trim()
              ? true
              : Boolean(existingRule?.signing_secret_set);
          const {
            signing_secret: _secret,
            clear_signing_secret: _clear,
            ...redactedBody
          } = body;
          const storedRule = {
            ...redactedBody,
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            created_at: "2026-06-02T10:04:00Z",
            id:
              typeof body.id === "string"
                ? body.id
                : "adadadad-1111-4111-8111-111111111111",
            signing_secret_set: signingSecretSet,
            updated_at: "2026-06-02T10:04:00Z",
          };
          const storedIndex = webhookRulesFixture.findIndex(
            (rule: Record<string, unknown>) => rule.id === storedRule.id,
          );
          if (storedIndex >= 0) {
            webhookRulesFixture.splice(storedIndex, 1, storedRule);
          } else {
            webhookRulesFixture.push(storedRule);
          }
          return jsonResponse(storedRule);
        }
        if (pathname === "/api/v1/webhook-rules/dry-run" && method === "POST") {
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
          const matchingRules = webhookRulesFixture.filter(
            (rule: Record<string, unknown>) =>
              !body.rule_id || rule.id === body.rule_id,
          );
          return jsonResponse(
            matchingRules.map((rule: Record<string, unknown>) =>
              withReviewPreviewHash(
                buildWebhookDelivery(
                  {
                    ...rule,
                    event_id: body.event_id,
                    event_kind: body.event_kind,
                  },
                  body.dry_run ? "matched_dry_run" : "queued",
                ),
                body.dry_run
                  ? "3333333333333333333333333333333333333333333333333333333333333333"
                  : null,
              ),
            ),
          );
        }
        const webhookRuleMatch = pathname.match(
          /^\/api\/v1\/webhook-rules\/([^/]+)$/,
        );
        if (webhookRuleMatch && method === "DELETE") {
          const ruleId = decodeURIComponent(webhookRuleMatch[1]);
          const ruleIndex = (
            webhookRulesFixture as Array<Record<string, unknown>>
          ).findIndex((rule) => rule.id === ruleId);
          if (ruleIndex >= 0) {
            webhookRulesFixture.splice(ruleIndex, 1);
          }
          for (const delivery of webhookDeliveriesFixture as Array<
            Record<string, unknown>
          >) {
            if (
              delivery.rule_id === ruleId &&
              ["queued", "failed", "in_progress"].includes(
                String(delivery.status),
              )
            ) {
              delivery.status = "canceled_disabled";
              delivery.error = "webhook rule deleted";
              delivery.next_attempt_at = null;
              delivery.delivered_at = null;
            }
          }
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
            webhookDeliveriesFixture.map(
              (delivery: Record<string, unknown>) => ({
                ...delivery,
                review_preview_hash: body?.dry_run
                  ? "4444444444444444444444444444444444444444444444444444444444444444"
                  : delivery.review_preview_hash,
                status: body?.dry_run ? delivery.status : "delivered",
              }),
            ),
          );
        }
        if (
          pathname === "/api/v1/webhook-deliveries/rotate" &&
          method === "POST"
        ) {
          const body = (await readJsonBody(input, init)) as {
            confirmed?: boolean;
            older_than?: string | null;
            rule_id?: string | null;
            status?: string | null;
            preview_hash?: string | null;
          } | null;
          requests.webhookDeliveryRotations.push(body);
          const matchedCount = webhookDeliveriesFixture.filter(
            (delivery: Record<string, unknown>) =>
              (!body?.rule_id || delivery.rule_id === body.rule_id) &&
              (!body?.status || delivery.status === body.status),
          ).length;
          return jsonResponse({
            confirmation_required: !body?.confirmed,
            deleted_count: body?.confirmed ? matchedCount : 0,
            matched_count: matchedCount,
            older_than: body?.older_than ?? "2025-12-31T00:00:00.000Z",
            preview_hash: body?.preview_hash ?? "9".repeat(64),
            rule_id: body?.rule_id ?? null,
            status: body?.status ?? null,
          });
        }
        const deleteAgentMatch = pathname.match(
          /^\/api\/v1\/agents\/([^/]+)\/delete$/,
        );
        if (deleteAgentMatch && method === "POST") {
          if (agentDeleteDelayMsFixture > 0) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, agentDeleteDelayMsFixture),
            );
          }
          const body = await readJsonBody(input, init);
          requests.agentDeletes.push(body);
          const clientId = decodeURIComponent(deleteAgentMatch[1]);
          if (agentDeleteRequestFailureFixture) {
            return jsonResponse(
              {
                error: "fixture_delete_refused",
                message:
                  "Fixture refused the VPS deletion before changing inventory.",
              },
              503,
            );
          }
          deletedAgentIds.add(clientId);
          return jsonResponse({
            client_id: clientId,
            deleted: true,
            deleted_at: "2026-06-02T10:07:00Z",
            post_commit: [
              {
                error: null,
                operation: "gateway_session_disconnect",
                status: "completed",
              },
              {
                error: null,
                operation: "job_terminal_reconciliation",
                status: "completed",
              },
            ],
            runtime_sync: [
              ...agentDeleteSyncJobIdsFixture.map((jobId, index) => ({
                client_id: index === 0 ? "agent-fra-02" : `peer-${index + 1}`,
                error: null,
                job_id: jobId,
                status: "queued",
              })),
              ...agentDeleteFailedClientIdsFixture.map((failedClientId) => ({
                client_id: failedClientId,
                error:
                  "Runtime apply job could not be queued because the server failed while creating it. Desired state remains saved; inspect API logs and retry",
                job_id: null,
                status: "queue_failed",
              })),
            ],
          });
        }
        if (pathname === "/api/v1/agents") {
          return jsonResponse(dashboardAgents());
        }
        if (pathname === "/api/v1/gateway-sessions" && method === "GET")
          return emptyArrayResponse();
        if (pathname === "/api/v1/auth/bootstrap-status" && method === "GET") {
          return jsonResponse({
            bootstrap_required: false,
          });
        }
        if (
          (pathname === "/api/v1/auth/login" ||
            pathname === "/api/v1/auth/bootstrap") &&
          method === "POST"
        ) {
          return jsonResponse({
            access_token:
              "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            expires_in_secs: 900,
            operator: operatorView(currentOperatorRecord),
            refresh_expires_in_secs: 1_209_600,
            refresh_token:
              "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            token_type: "Bearer",
          });
        }
        if (pathname === "/api/v1/auth/me" && method === "GET")
          return jsonResponse(operatorView(currentOperatorRecord));
        if (pathname === "/api/v1/auth/privilege/verify" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.privilegeVerifications.push(body);
          if (privilegeVerificationDelayMsFixture > 0) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, privilegeVerificationDelayMsFixture),
            );
          }
          if (privilegeVerificationFailureFixture === "denied") {
            return jsonResponse(
              {
                error: "privilege_verification_failed",
                message: "The privilege assertion was rejected.",
              },
              403,
            );
          }
          if (privilegeVerificationFailureFixture === "unavailable") {
            return jsonResponse(
              {
                error: "privilege_verification_unavailable",
                message: "The gateway could not verify privilege material.",
              },
              503,
            );
          }
          return jsonResponse({ verified: true });
        }
        if (pathname === "/api/v1/auth/preferences" && method === "PUT") {
          const body = await readJsonBody(input, init);
          requests.operatorPreferences.push(body);
          Object.assign(currentOperatorPreferences, body);
          return jsonResponse(operatorView(currentOperatorRecord));
        }
        if (pathname === "/api/v1/auth/totp/setup" && method === "POST") {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.totpSetups.push(body);
          const password = String(body.password ?? "");
          if (password.length < 12) {
            return jsonResponse({ error: "password_too_short" }, 400);
          }
          if (password !== "valid-password-123") {
            return jsonResponse(
              {
                error: "invalid_totp_credentials",
                message: "The current password is incorrect.",
              },
              400,
            );
          }
          if (totpSetupSwitchSessionFixture && operatorSessions.length > 1) {
            operatorSessions.forEach((session, index) => {
              session.current = index === 1;
            });
          }
          if (totpSetupDelayMsFixture > 0) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, totpSetupDelayMsFixture),
            );
          }
          return jsonResponse({
            algorithm: "SHA1",
            digits: 6,
            operator_id:
              totpSetupOperatorIdOverrideFixture ?? currentOperatorRecord.id,
            otpauth_uri: `otpauth://totp/vpsman:console-admin?secret=${fixtureTotpSecret}&issuer=vpsman`,
            period_secs: 30,
            secret_base32: fixtureTotpSecret,
          });
        }
        if (pathname === "/api/v1/auth/totp/confirm" && method === "POST") {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          if (
            String(body.password ?? "") !== "valid-password-123" ||
            String(body.code ?? "") !== (await fixtureTotpCode())
          ) {
            return jsonResponse(
              {
                error: "invalid_totp_credentials",
                message:
                  "The current password or authenticator code is incorrect.",
              },
              400,
            );
          }
          currentOperatorRecord.totp_enabled = true;
          return jsonResponse(operatorView(currentOperatorRecord));
        }
        if (pathname === "/api/v1/auth/totp/disable" && method === "POST") {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          if (
            String(body.password ?? "") !== "valid-password-123" ||
            String(body.code ?? "") !== (await fixtureTotpCode())
          ) {
            return jsonResponse(
              {
                error: "invalid_totp_credentials",
                message:
                  "The current password or authenticator code is incorrect.",
              },
              400,
            );
          }
          currentOperatorRecord.totp_enabled = false;
          return jsonResponse(operatorView(currentOperatorRecord));
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
            scopes: Array.isArray(body.scopes) ? body.scopes.map(String) : [],
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
          requests.operatorActions.push({
            action,
            body,
            operator_id: operatorId,
          });
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
          return jsonResponse(operatorSessions);
        const operatorSessionRevokeMatch = pathname.match(
          /^\/api\/v1\/operator-sessions\/([^/]+)\/revoke$/,
        );
        if (operatorSessionRevokeMatch && method === "POST") {
          const sessionId = decodeURIComponent(operatorSessionRevokeMatch[1]);
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.operatorActions.push({
            action: "session-revoke",
            body,
            session_id: sessionId,
          });
          const session = operatorSessions.find(
            (item) => item.id === sessionId,
          );
          if (!session)
            return jsonResponse({ error: "not found" }, { status: 404 });
          session.revoked = true;
          session.revoked_at = "2026-01-03T00:15:00Z";
          return jsonResponse(session);
        }
        if (pathname === "/api/v1/operator-auth-events" && method === "GET")
          return jsonResponse(
            operatorAuthEventsFixture ?? [
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
                operator_id: "99999999-aaaa-4bbb-8ccc-000000000001",
                username: "console-admin",
                result: "success",
                reason: null,
                remote_ip: "203.0.113.44",
                user_agent: "Mozilla/5.0 Chrome/124.0",
                session_id: "88888888-aaaa-4bbb-8ccc-000000000002",
                created_at: "2026-01-01T01:00:00Z",
              },
              {
                id: "77777777-aaaa-4bbb-8ccc-000000000003",
                operator_id: null,
                username: "unknown-user",
                result: "failure",
                reason: "invalid_credentials",
                remote_ip: "127.0.0.1",
                user_agent: "Playwright",
                session_id: null,
                created_at: "2026-01-01T00:01:00Z",
              },
              {
                id: "77777777-aaaa-4bbb-8ccc-000000000004",
                operator_id: null,
                username: "unknown-user",
                result: "failure",
                reason: "invalid_credentials",
                remote_ip: "127.0.0.1",
                user_agent: "Playwright",
                session_id: null,
                created_at: "2026-01-01T00:02:00Z",
              },
            ],
          );
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
            post_commit: [
              {
                error: null,
                operation: "gateway_session_disconnect",
                status: "completed",
              },
              {
                error: null,
                operation: "job_terminal_reconciliation",
                status: "completed",
              },
            ],
            revocation: {
              client_id: decodeURIComponent(pathname.split("/")[4] ?? ""),
              created_at: "2026-06-02T10:06:00Z",
              id: "edededed-1111-4111-8111-111111111111",
              public_key_sha256_hex: "d".repeat(64),
              reason: (body as { reason?: string | null }).reason ?? null,
              revoked_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            },
          });
        }
        if (pathname === "/api/v1/agent-identities" && method === "POST") {
          const body = (await readJsonBody(input, init)) as {
            client_id?: string;
            client_public_key_hex?: string;
            display_name?: string | null;
            replace_existing_key?: boolean;
            tags?: string[];
          };
          requests.agentIdentities.push(body);
          return jsonResponse({
            identity: {
              client_id: body.client_id ?? "agent-new-direct-01",
              current_public_key_sha256_hex: "e".repeat(64),
              display_name:
                body.display_name || body.client_id || "agent-new-direct-01",
              status: "offline",
              tags: body.tags ?? [],
            },
            post_commit: [
              ...(body.replace_existing_key
                ? [
                    {
                      error: null,
                      operation: "gateway_session_disconnect",
                      status: "completed",
                    },
                  ]
                : []),
              {
                error: null,
                operation: "job_terminal_reconciliation",
                status: "completed",
              },
            ],
          });
        }
        if (
          pathname === "/api/v1/telemetry/rollups" &&
          method === "GET" &&
          telemetryFailurePathFixture === "rollups"
        ) {
          return jsonResponse({ error: "telemetry_rollups_unavailable" }, 503);
        }
        if (pathname === "/api/v1/telemetry/rollups" && method === "GET")
          return emptyArrayResponse();
        if (
          pathname === "/api/v1/telemetry/network-rates" &&
          method === "GET" &&
          telemetryFailurePathFixture === "network-rates"
        ) {
          return jsonResponse(
            { error: "telemetry_network_rates_unavailable" },
            503,
          );
        }
        if (
          pathname === "/api/v1/telemetry/network-rates" &&
          method === "GET"
        ) {
          const scale =
            telemetryNetworkRateScalesFixture[
              Math.min(
                telemetryNetworkRateRequestCount,
                telemetryNetworkRateScalesFixture.length - 1,
              )
            ] ?? 1;
          telemetryNetworkRateRequestCount += 1;
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
              rx_bps_avg: 8738 * scale,
              tx_bps_avg: 17476 * scale,
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
              rx_bps_avg: 980000 * scale,
              tx_bps_avg: 860000 * scale,
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
              rx_bps_avg: 19200000 * scale,
              tx_bps_avg: 18400000 * scale,
              updated_at: "2026-05-31T10:02:06Z",
            },
          ]);
        }
        if (
          pathname === "/api/v1/telemetry/tunnels" &&
          method === "GET" &&
          telemetryFailurePathFixture === "tunnels"
        ) {
          return jsonResponse({ error: "telemetry_tunnels_unavailable" }, 503);
        }
        if (pathname === "/api/v1/telemetry/tunnels" && method === "GET")
          return jsonResponse([
            {
              client_id: "agent-fra-02",
              observed_at: "2026-05-31T10:02:00Z",
              interface: "tunab",
              kind: "gre",
              ownership_mode: "agent_builtin",
              mutation_policy: "managed_declared_plan",
              plan_id: "dddddddd-eeee-4fff-8000-111111111111",
              plan_name: "sfo-fra-gre",
              plan_runtime_manager: "agent_builtin",
              endpoint_side: "right",
              peer_client_id: "agent-sfo-01",
              source: "declared_plan_status",
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
            },
            {
              client_id: "agent-sfo-01",
              observed_at: "2026-05-31T10:02:00Z",
              interface: "tunab",
              kind: "gre",
              ownership_mode: "agent_builtin",
              mutation_policy: "managed_declared_plan",
              plan_id: "dddddddd-eeee-4fff-8000-111111111111",
              plan_name: "sfo-fra-gre",
              plan_runtime_manager: "agent_builtin",
              endpoint_side: "left",
              peer_client_id: "agent-fra-02",
              source: "declared_plan_status",
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
            },
            {
              client_id: "agent-sfo-01",
              observed_at: "2026-05-31T10:00:00Z",
              interface: "ovpn42",
              kind: "openvpn",
              ownership_mode: "external_observed",
              mutation_policy: "observe_only_declared_plan",
              plan_id: "eeeeeeee-ffff-4000-8111-222222222222",
              plan_name: "external-openvpn-observed",
              plan_runtime_manager: "external_observed",
              endpoint_side: "left",
              peer_client_id: "agent-fra-02",
              source: "declared_interface_status",
              operstate: "up",
              mtu: 1500,
              link_type: null,
              address: null,
              rx_bytes: 7340032,
              tx_bytes: 7864320,
              traffic_source: "interface_counters",
              traffic_status: "ok",
              traffic_reason: null,
              traffic_checked_unix: 1780202400,
              adapter_health: null,
              latency_monitoring_enabled: true,
              latency_status: "healthy",
              latency_reason: "probe_ok",
              latency_primary_family: "ipv4",
              latency_target: "10.44.0.1",
              latency_checked_unix: 1780202400,
              latency_avg_ms: 18.1,
              packet_loss_ratio: 0,
              latency_healthy_windows: 3,
              latency_missed_windows: 0,
            },
            {
              client_id: "agent-fra-02",
              observed_at: "2026-05-31T10:00:00Z",
              interface: "ovpn42",
              kind: "openvpn",
              ownership_mode: "external_observed",
              mutation_policy: "observe_only_declared_plan",
              plan_id: "eeeeeeee-ffff-4000-8111-222222222222",
              plan_name: "external-openvpn-observed",
              plan_runtime_manager: "external_observed",
              endpoint_side: "right",
              peer_client_id: "agent-sfo-01",
              source: "declared_interface_status",
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
              latency_target: "10.44.0.0",
              latency_checked_unix: 1780202400,
              latency_avg_ms: null,
              packet_loss_ratio: 1,
              latency_healthy_windows: 0,
              latency_missed_windows: 1,
            },
          ]);
        if (pathname === "/api/v1/configuration-presets" && method === "GET") {
          const behavior = new URL(url, window.location.href).searchParams.get(
            "behavior",
          );
          return jsonResponse(
            behavior
              ? mutableConfigurationPresets.filter(
                  (record) => record.behavior === behavior,
                )
              : mutableConfigurationPresets,
          );
        }
        if (pathname === "/api/v1/configuration-presets" && method === "POST") {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.configurationPresetMutations.push({
            action: "create",
            body,
          });
          const created: ConfigurationPresetRecord = {
            behavior: String(
              body.behavior ?? "host_metrics",
            ) as ConfigurationBehavior,
            created_at: "2026-06-02T10:08:00Z",
            definition: structuredClone(
              body.definition ?? {},
            ) as ConfigurationPresetRecord["definition"],
            description:
              typeof body.description === "string" ? body.description : null,
            effective_vps_count: 0,
            id: "34343434-3434-4434-8434-343434343434",
            is_default: false,
            kind: "custom",
            name: String(body.name ?? "Custom preset"),
            override_vps_count: 0,
            updated_at: "2026-06-02T10:08:00Z",
          };
          mutableConfigurationPresets.push(created);
          return jsonResponse(created);
        }
        const configurationPresetCloneMatch = pathname.match(
          /^\/api\/v1\/configuration-presets\/([^/]+)\/clone$/,
        );
        if (configurationPresetCloneMatch && method === "POST") {
          const presetId = decodeURIComponent(configurationPresetCloneMatch[1]);
          const source = mutableConfigurationPresets.find(
            (record) => record.id === presetId,
          );
          if (!source) {
            return jsonResponse(
              { error: "configuration_preset_not_found" },
              404,
            );
          }
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.configurationPresetMutations.push({
            action: "clone",
            body,
            preset_id: presetId,
          });
          const cloned: ConfigurationPresetRecord = {
            ...source,
            created_at: "2026-06-02T10:08:00Z",
            definition: structuredClone(source.definition),
            description:
              typeof body.description === "string" ? body.description : null,
            effective_vps_count: 0,
            id: "35353535-3535-4535-8535-353535353535",
            is_default: false,
            kind: "custom",
            name: String(body.name ?? `${source.name} copy`),
            override_vps_count: 0,
            updated_at: "2026-06-02T10:08:00Z",
          };
          mutableConfigurationPresets.push(cloned);
          return jsonResponse(cloned);
        }
        const configurationPresetPreviewMatch = pathname.match(
          /^\/api\/v1\/configuration-presets\/([^/]+)\/preview$/,
        );
        if (configurationPresetPreviewMatch && method === "POST") {
          const presetId = decodeURIComponent(
            configurationPresetPreviewMatch[1],
          );
          const preset = mutableConfigurationPresets.find(
            (record) => record.id === presetId,
          );
          if (!preset) {
            return jsonResponse(
              { error: "configuration_preset_not_found" },
              404,
            );
          }
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          const affectedClientIds = mutableConfigurationSources
            .filter((source) => source.effective_preset_id === preset.id)
            .map((source) => source.client_id)
            .filter(
              (clientId, index, values) => values.indexOf(clientId) === index,
            )
            .sort();
          return jsonResponse({
            affected_client_count: affectedClientIds.length,
            affected_client_ids: affectedClientIds,
            behavior: preset.behavior,
            candidate_definition: body.definition ?? preset.definition,
            candidate_description:
              typeof body.description === "string" ? body.description : null,
            changed_keys: ["source"],
            current_definition: preset.definition,
            current_description: preset.description,
            name: preset.name,
            preset_id: preset.id,
            preview_hash: "8".repeat(64),
            sections: { fixture: true },
            toml: "[fixture]\npreview = true\n",
          });
        }
        const configurationPresetMatch = pathname.match(
          /^\/api\/v1\/configuration-presets\/([^/]+)$/,
        );
        if (configurationPresetMatch && method === "PUT") {
          const presetId = decodeURIComponent(configurationPresetMatch[1]);
          const preset = mutableConfigurationPresets.find(
            (record) => record.id === presetId,
          );
          if (!preset) {
            return jsonResponse(
              { error: "configuration_preset_not_found" },
              404,
            );
          }
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.configurationPresetMutations.push({
            action: "update",
            body,
            preset_id: presetId,
          });
          const affectedClientIds = mutableConfigurationSources
            .filter((source) => source.effective_preset_id === preset.id)
            .map((source) => source.client_id)
            .filter(
              (clientId, index, values) => values.indexOf(clientId) === index,
            )
            .sort();
          const preview = {
            affected_client_count: affectedClientIds.length,
            affected_client_ids: affectedClientIds,
            behavior: preset.behavior,
            candidate_definition: body.definition ?? preset.definition,
            candidate_description:
              typeof body.description === "string" ? body.description : null,
            changed_keys: ["source"],
            current_definition: preset.definition,
            current_description: preset.description,
            name: preset.name,
            preset_id: preset.id,
            preview_hash: String(body.preview_hash ?? "8".repeat(64)),
            sections: { fixture: true },
            toml: "[fixture]\npreview = true\n",
          };
          preset.definition = structuredClone(
            body.definition ?? preset.definition,
          ) as ConfigurationPresetRecord["definition"];
          preset.description =
            typeof body.description === "string" ? body.description : null;
          preset.updated_at = "2026-06-02T10:09:00Z";
          return jsonResponse({
            preset,
            preview,
            sync: affectedClientIds.map((clientId, index) => ({
              client_id: clientId,
              error: null,
              job_id: `4f300000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
              status: "queued",
            })),
          });
        }
        if (configurationPresetMatch && method === "DELETE") {
          const presetId = decodeURIComponent(configurationPresetMatch[1]);
          requests.configurationPresetMutations.push({
            action: "delete",
            preset_id: presetId,
          });
          mutableConfigurationPresets = mutableConfigurationPresets.filter(
            (record) => record.id !== presetId,
          );
          return new Response(null, { status: 204 });
        }
        if (pathname === "/api/v1/configuration-sources" && method === "GET") {
          const search = new URL(url, window.location.href).searchParams;
          const clientId = search.get("client_id");
          const behavior = search.get("behavior");
          return jsonResponse(
            mutableConfigurationSources.filter(
              (record) =>
                (!clientId || record.client_id === clientId) &&
                (!behavior || record.behavior === behavior),
            ),
          );
        }
        if (
          pathname === "/api/v1/runtime-config/apply-state" &&
          method === "GET"
        ) {
          if (runtimeConfigApplyFailureFixture) {
            return jsonResponse(
              { error: "runtime_config_apply_state_unavailable" },
              503,
            );
          }
          return jsonResponse(runtimeConfigApplyStatesFixture);
        }
        if (
          pathname === "/api/v1/runtime-config/patch-generators" &&
          method === "GET"
        ) {
          return jsonResponse(runtimeConfigPatchGeneratorsFixture);
        }
        if (
          pathname === "/api/v1/runtime-config/patch-generators" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          requests.runtimeConfigPatchGenerators.push(body);
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
            name: request.name ?? "Custom generator",
            raw_generator_body: request.raw_generator_body ?? "",
            updated_at: "2026-06-02T10:05:00Z",
          });
        }
        if (
          pathname.startsWith("/api/v1/runtime-config/patch-generators/") &&
          pathname.endsWith("/render") &&
          method === "POST"
        ) {
          const generatorId =
            pathname.split("/").at(-2) ??
            runtimeConfigPatchGeneratorsFixture[0].id;
          const generator =
            runtimeConfigPatchGeneratorsFixture.find(
              (record: { id: string }) => record.id === generatorId,
            ) ?? runtimeConfigPatchGeneratorsFixture[0];
          const body = await readJsonBody(input, init);
          const values = asFixtureRecord(asFixtureRecord(body)?.values) ?? {};
          const toml = renderPatchGeneratorBodyFixture(
            generator.raw_generator_body,
            values,
            generator.field_schema,
          );
          return jsonResponse({
            affected_sections: affectedSectionsForTomlFixture(
              toml,
              generator.domain,
            ),
            docs_metadata: generator.docs_metadata,
            generated_at: "2026-06-02T10:06:00Z",
            name: generator.name,
            patch: {},
            generator_id: generator.id,
            toml,
          });
        }
        if (
          pathname.startsWith("/api/v1/runtime-config/patch-generators/") &&
          method === "DELETE"
        ) {
          return new Response(null, { status: 204 });
        }
        if (
          [
            "/api/v1/configuration-source-overrides/preview",
            "/api/v1/configuration-source-overrides/apply",
          ].includes(pathname) &&
          method === "POST"
        ) {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          const directIds = Array.isArray(body.target_client_ids)
            ? body.target_client_ids.map(String)
            : [];
          const selectorExpression =
            typeof body.selector_expression === "string"
              ? body.selector_expression
              : "";
          const selectorIds = selectorExpression
            ? visibleAgents()
                .filter((agent) =>
                  expressionMatchesAgent(agent, selectorExpression),
                )
                .map((agent) => agent.id)
            : [];
          const targetClientIds = [...new Set([...directIds, ...selectorIds])]
            .filter((clientId) =>
              visibleAgents().some((agent) => agent.id === clientId),
            )
            .sort();
          const behavior = String(body.behavior ?? "host_metrics");
          const action = body.action === "reset" ? "reset" : "set";
          const selectedPreset =
            action === "set"
              ? (mutableConfigurationPresets.find(
                  (record) => record.id === body.preset_id,
                ) ?? null)
              : (mutableConfigurationPresets.find(
                  (record) => record.behavior === behavior && record.is_default,
                ) ?? null);
          if (!selectedPreset) {
            return jsonResponse(
              { error: "configuration_preset_not_found" },
              404,
            );
          }
          const targets = targetClientIds.map((clientId) => {
            const before = mutableConfigurationSources.find(
              (record) =>
                record.client_id === clientId && record.behavior === behavior,
            );
            return {
              after_origin:
                action === "set" ? "explicit_override" : "system_default",
              after_preset_id: selectedPreset.id,
              after_preset_name: selectedPreset.name,
              before_origin: before?.selection_origin ?? "system_default",
              before_preset_id:
                before?.effective_preset_id ?? selectedPreset.id,
              before_preset_name:
                before?.effective_preset_name ?? selectedPreset.name,
              client_id: clientId,
            };
          });
          requests.configurationSourceOverrides.push({ body, pathname });
          const preview = {
            action,
            behavior,
            preset: action === "set" ? selectedPreset : null,
            preview_hash: String(body.preview_hash ?? "8".repeat(64)),
            selector_expression:
              typeof body.selector_expression === "string"
                ? body.selector_expression.trim()
                : "",
            target_count: targets.length,
            targets,
          };
          if (pathname.endsWith("/preview")) {
            return jsonResponse(preview);
          }
          if (configurationSourceApplyFailureFixture) {
            return jsonResponse(
              { error: "configuration_preview_hash_mismatch" },
              409,
            );
          }
          const targetIds = new Set(targetClientIds);
          mutableConfigurationSources = mutableConfigurationSources.map(
            (record) =>
              record.behavior === behavior && targetIds.has(record.client_id)
                ? {
                    ...record,
                    effective_preset_id: selectedPreset.id,
                    effective_preset_kind: selectedPreset.kind,
                    effective_preset_name: selectedPreset.name,
                    override_updated_at:
                      action === "set" ? "2026-06-02T10:10:00Z" : null,
                    runtime_sync: {
                      reason: "The effective configuration is queued.",
                      state: "queued",
                    },
                    selection_origin:
                      action === "set" ? "explicit_override" : "system_default",
                  }
                : record,
          );
          return jsonResponse({
            ...preview,
            sync: targetClientIds.map((clientId, index) => ({
              client_id: clientId,
              error:
                configurationSourceSyncFailureFixture && index === 0
                  ? "Runtime apply queue unavailable"
                  : null,
              job_id:
                configurationSourceSyncFailureFixture && index === 0
                  ? null
                  : `4f400000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
              status:
                configurationSourceSyncFailureFixture && index === 0
                  ? "queue_failed"
                  : "queued",
            })),
          });
        }
        if (pathname === "/api/v1/effective-agent-config" && method === "GET") {
          const clientId =
            new URL(url, window.location.href).searchParams.get("client_id") ??
            "agent-sfo-01";
          requests.effectiveConfigurationReads.push({ client_id: clientId });
          const sources = mutableConfigurationSources.filter(
            (source) => source.client_id === clientId,
          );
          return jsonResponse({
            client_id: clientId,
            generated_at: "2026-06-02T10:07:00Z",
            sections: {
              execution: { process_inventory_source: "linux_procfs" },
              telemetry: { source: "linux_procfs" },
            },
            sources,
            toml: `[telemetry]\nsource = "linux_procfs"\n\n[execution]\nprocess_inventory_source = "linux_procfs"\n`,
          });
        }
        if (
          pathname === "/api/v1/network-adapter-definitions" &&
          method === "GET"
        ) {
          const kind = new URL(url, window.location.href).searchParams.get(
            "adapter_kind",
          );
          return jsonResponse(
            kind
              ? mutableNetworkAdapterDefinitions.filter(
                  (record) => record.adapter_kind === kind,
                )
              : mutableNetworkAdapterDefinitions,
          );
        }
        if (
          pathname === "/api/v1/network-adapter-definitions" &&
          method === "POST"
        ) {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.networkAdapterMutations.push({ action: "create", body });
          const created: NetworkAdapterDefinitionRecord = {
            adapter_kind: String(
              body.adapter_kind ?? "runtime_tunnel",
            ) as NetworkAdapterDefinitionRecord["adapter_kind"],
            created_at: "2026-06-02T10:08:00Z",
            definition: structuredClone(
              body.definition ?? {},
            ) as NetworkAdapterDefinitionRecord["definition"],
            description:
              typeof body.description === "string" ? body.description : null,
            id: "36363636-3636-4636-8636-363636363636",
            name: String(body.name ?? "Custom adapter"),
            updated_at: "2026-06-02T10:08:00Z",
          };
          mutableNetworkAdapterDefinitions.push(created);
          return jsonResponse(created);
        }
        const networkAdapterMatch = pathname.match(
          /^\/api\/v1\/network-adapter-definitions\/([^/]+)$/,
        );
        if (networkAdapterMatch && method === "PUT") {
          const definitionId = decodeURIComponent(networkAdapterMatch[1]);
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          const definition = mutableNetworkAdapterDefinitions.find(
            (record) => record.id === definitionId,
          );
          if (!definition) {
            return jsonResponse({ error: "network_adapter_not_found" }, 404);
          }
          requests.networkAdapterMutations.push({
            action: "update",
            body,
            definition_id: definitionId,
          });
          Object.assign(definition, {
            adapter_kind: String(
              body.adapter_kind ?? definition.adapter_kind,
            ) as NetworkAdapterDefinitionRecord["adapter_kind"],
            definition: structuredClone(
              body.definition ?? definition.definition,
            ),
            description:
              typeof body.description === "string" ? body.description : null,
            name: String(body.name ?? definition.name),
            updated_at: "2026-06-02T10:09:00Z",
          });
          return jsonResponse(definition);
        }
        if (networkAdapterMatch && method === "DELETE") {
          const definitionId = decodeURIComponent(networkAdapterMatch[1]);
          requests.networkAdapterMutations.push({
            action: "delete",
            definition_id: definitionId,
          });
          mutableNetworkAdapterDefinitions =
            mutableNetworkAdapterDefinitions.filter(
              (record) => record.id !== definitionId,
            );
          return new Response(null, { status: 204 });
        }
        if (pathname === "/api/v1/job-approvals" && method === "GET") {
          return jsonResponse(currentJobApprovals);
        }
        if (pathname === "/api/v1/job-rollouts" && method === "GET") {
          return jsonResponse(mutableJobRollouts);
        }
        const jobRolloutActionMatch = pathname.match(
          /^\/api\/v1\/job-rollouts\/([^/]+)\/(pause|resume)$/,
        );
        if (jobRolloutActionMatch && method === "POST") {
          const jobId = decodeURIComponent(jobRolloutActionMatch[1]);
          const action = jobRolloutActionMatch[2] as "pause" | "resume";
          const body = (await readJsonBody(input, init)) as {
            confirmed?: boolean;
            reason?: string | null;
          } | null;
          requests.jobRolloutActions.push({ action, body, job_id: jobId });
          const rollout = mutableJobRollouts.find(
            (record) => record.job_id === jobId,
          );
          if (!rollout) {
            return jsonResponse({ error: "job_rollout_not_found" }, 404);
          }
          if (rollout.status === "completed" || rollout.status === "aborted") {
            return jsonResponse({ error: "job_rollout_terminal" }, 409);
          }
          if (action === "resume" && !body?.confirmed) {
            return jsonResponse(
              { error: "job_rollout_resume_requires_confirmation" },
              409,
            );
          }
          rollout.status = action === "pause" ? "paused" : "running";
          rollout.pause_reason =
            action === "pause" ? (body?.reason ?? "operator_requested") : null;
          rollout.next_batch_at = "2026-06-02T10:02:00Z";
          rollout.updated_at = "2026-06-02T10:02:00Z";
          persistJobRollouts();
          return jsonResponse(rollout);
        }
        const jobRolloutMatch = pathname.match(
          /^\/api\/v1\/job-rollouts\/([^/]+)$/,
        );
        if (jobRolloutMatch && method === "GET") {
          const jobId = decodeURIComponent(jobRolloutMatch[1]);
          const rollout = mutableJobRollouts.find(
            (record) => record.job_id === jobId,
          );
          return rollout
            ? jsonResponse(rollout)
            : jsonResponse({ error: "job_rollout_not_found" }, 404);
        }
        if (pathname === "/api/v1/job-approvals" && method === "POST") {
          const body = (await readJsonBody(input, init)) as {
            approval_id?: string;
            job?: Record<string, unknown>;
            reason?: string | null;
            risk?: string | null;
          } | null;
          requests.jobApprovals.push(body);
          const job = body?.job ?? {};
          const targetClientIds = Array.isArray(job.target_client_ids)
            ? (job.target_client_ids as string[])
            : [];
          const approval = {
            id: body?.approval_id ?? "abababab-3333-4444-8555-666666666666",
            status: "pending",
            job_id: job.job_id ?? "abababab-4444-4555-8666-777777777777",
            command_type:
              commandTypeForOperation(
                job.operation as Record<string, unknown> | undefined,
              ) ??
              job.command ??
              "shell_argv",
            selector_expression: job.selector_expression ?? "",
            target_client_ids: targetClientIds,
            target_count: targetClientIds.length,
            privileged: job.privileged ?? true,
            destructive: job.destructive ?? false,
            force_unprivileged: job.force_unprivileged ?? false,
            max_timeout_secs: job.max_timeout_secs ?? 30,
            payload_hash: "e".repeat(64),
            request_fingerprint: "f".repeat(64),
            requester_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            requester_username: "console-admin",
            requester_role: "admin",
            requested_at: "2026-06-02T10:13:00Z",
            request_reason: body?.reason ?? null,
            risk:
              body?.risk ?? (job.destructive ? "destructive" : "privileged"),
            decision_by: null,
            decision_username: null,
            decision_reason: null,
            decided_at: null,
          };
          currentJobApprovals.unshift(approval);
          return jsonResponse(approval, 201);
        }
        const jobApprovalDecisionMatch = pathname.match(
          /^\/api\/v1\/job-approvals\/([^/]+)\/(approve|reject)$/,
        );
        if (jobApprovalDecisionMatch && method === "POST") {
          const approvalId = decodeURIComponent(jobApprovalDecisionMatch[1]);
          const decision = jobApprovalDecisionMatch[2];
          const approval = currentJobApprovals.find(
            (record) => record.id === approvalId,
          );
          if (!approval) {
            return jsonResponse({ error: "job_approval_not_found" }, 404);
          }
          const body = (await readJsonBody(input, init)) as {
            reason?: string | null;
          } | null;
          requests.jobApprovalDecisions.push({
            approval_id: approvalId,
            decision,
            body,
          });
          if (decision === "reject" && !String(body?.reason ?? "").trim()) {
            return jsonResponse(
              { error: "job_approval_rejection_reason_required" },
              400,
            );
          }
          approval.status = decision === "approve" ? "approved" : "rejected";
          approval.decision_by = "99999999-aaaa-4bbb-8ccc-000000000001";
          approval.decision_username = "console-admin";
          approval.decision_reason = body?.reason ?? null;
          approval.decided_at = "2026-06-02T10:14:00Z";
          if (decision === "reject") {
            return approvalDecisionResponse(approval, null);
          }
          const jobId = String(approval.job_id);
          const targetClientIds = Array.isArray(approval.target_client_ids)
            ? (approval.target_client_ids as string[])
            : [];
          const jobRecord = {
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            command_type: approval.command_type,
            completed_at: null,
            created_at: "2026-06-02T10:14:00Z",
            id: jobId,
            max_timeout_secs: approval.max_timeout_secs,
            payload_hash: approval.payload_hash,
            privileged: approval.privileged,
            source_schedule_id: null,
            status: "running",
            target_count: targetClientIds.length,
          };
          (jobsFixture as Array<Record<string, unknown>>).unshift(jobRecord);
          createdJobTargets.set(
            jobId,
            targetClientIds.map((clientId) => ({
              client_id: clientId,
              completed_at: null,
              exit_code: null,
              message: "queued from approval",
              started_at: null,
              status: "queued",
            })),
          );
          return approvalDecisionResponse(approval, {
            job_id: jobId,
            max_job_timeout_secs: 3600,
            max_timeout_secs: approval.max_timeout_secs,
            status: "running",
            target_count: targetClientIds.length,
            target_counts: queuedTargetCounts(targetClientIds.length),
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
          const request = body as { domains?: string[]; expression?: string };
          requests.artifactCleanupPreviews.push(body);
          const matchedBytes = (
            fileTransferSourceArtifactsFixture as Array<{ size_bytes?: number }>
          ).reduce((sum, artifact) => sum + (artifact.size_bytes ?? 0), 0);
          const representativeObjects = (
            fileTransferSourceArtifactsFixture as Array<{
              created_at?: string;
              id?: string;
              object_key?: string;
              size_bytes?: number;
              status?: string;
            }>
          ).map((artifact) => ({
            created_at: artifact.created_at ?? "2026-05-31T10:10:00Z",
            domain: "file_transfer_source",
            id: artifact.id ?? "62626262-2222-4333-8444-555555555555",
            object_key:
              artifact.object_key ?? "file-transfer-sources/payload.bin",
            reason: null,
            reference_protected: false,
            size_bytes: artifact.size_bytes ?? 0,
            status: artifact.status ?? "active",
          }));
          const createdTimes = representativeObjects
            .map((artifact) => artifact.created_at)
            .sort();
          return jsonResponse({
            domains: request.domains ?? [],
            expression: request.expression ?? "",
            full_list_download_url: null,
            matched_count: (
              fileTransferSourceArtifactsFixture as Array<unknown>
            ).length,
            matched_bytes: matchedBytes,
            newest_created_at: createdTimes.at(-1) ?? null,
            oldest_created_at: createdTimes[0] ?? null,
            preview_hash:
              "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            reference_protected_count: 0,
            representative_objects: representativeObjects,
            retained_count: representativeObjects.length,
          });
        }
        if (
          pathname === "/api/v1/server-jobs/artifact-cleanup" &&
          method === "POST"
        ) {
          const body = await readJsonBody(input, init);
          const request = body as {
            domains?: string[];
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
            metadata: { domains: request.domains ?? [] },
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
            command_type:
              commandTypeForOperation(request.operation) ?? "shell_argv",
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
        const commandTemplateMatch = pathname.match(
          /^\/api\/v1\/command-templates\/([^/]+)$/,
        );
        if (commandTemplateMatch && method === "DELETE") {
          const templateId = decodeURIComponent(commandTemplateMatch[1]);
          const template = commandTemplatesFixture.find(
            (record: { id: string }) => record.id === templateId,
          );
          if (!template) {
            return jsonResponse({ error: "command_template_not_found" }, 404);
          }
          if ((template as { built_in?: boolean }).built_in) {
            return jsonResponse(
              { error: "command_template_builtin_immutable" },
              409,
            );
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
        const hostProcessMatch = pathname.match(
          /^\/api\/v1\/host-processes\/([^/]+)$/,
        );
        if (hostProcessMatch && method === "GET") {
          return jsonResponse({
            ...hostProcessInventoryFixture,
            client_id: decodeURIComponent(hostProcessMatch[1]),
          });
        }
        const hostServiceMatch = pathname.match(
          /^\/api\/v1\/host-services\/([^/]+)$/,
        );
        if (hostServiceMatch && method === "GET") {
          return jsonResponse({
            ...hostServiceInventoryFixture,
            client_id: decodeURIComponent(hostServiceMatch[1]),
          });
        }
        const hostStorageMatch = pathname.match(
          /^\/api\/v1\/host-storage\/([^/]+)$/,
        );
        if (hostStorageMatch && method === "GET") {
          return jsonResponse({
            ...hostStorageInventoryFixture,
            client_id: decodeURIComponent(hostStorageMatch[1]),
          });
        }
        if (pathname === "/api/v1/os-updates" && method === "GET") {
          const visibleClientIds = new Set(
            visibleAgents().map((agent) => agent.id),
          );
          return jsonResponse(
            mutableHostPackageUpdatePlans.filter((plan) =>
              visibleClientIds.has(plan.client_id),
            ),
          );
        }
        const hostPackageUpdateMatch = pathname.match(
          /^\/api\/v1\/os-updates\/([^/]+)$/,
        );
        if (hostPackageUpdateMatch && method === "GET") {
          const clientId = decodeURIComponent(hostPackageUpdateMatch[1]);
          const plan = mutableHostPackageUpdatePlans.find(
            (item) => item.client_id === clientId,
          );
          return plan
            ? jsonResponse(plan)
            : jsonResponse({ error: "agent_not_found" }, 404);
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
            object_key: `file-transfer-sources/73737373-2222-4333-8444-555555555555-${request.sha256_hex}.bin`,
            sha256_hex: request.sha256_hex,
            size_bytes: request.size_bytes,
            status: "active",
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
            chunk_count: 3,
            byte_count: 40,
            truncated: false,
            source: "terminal_output_chunks",
            chunks: [
              {
                terminal_seq: 1,
                job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
                data_base64: btoa("durable replay line 1\n"),
                size_bytes: 22,
                sha256_hex: "8".repeat(64),
                created_at: "2026-05-31T10:12:00Z",
              },
              {
                terminal_seq: 2,
                job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
                data_base64: btoa(
                  `prompt$ ${String.fromCharCode(0xe2)}`,
                ),
                size_bytes: 9,
                sha256_hex: "9".repeat(64),
                created_at: "2026-05-31T10:12:00Z",
              },
              {
                terminal_seq: 3,
                job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
                data_base64: btoa(
                  `${String.fromCharCode(0x82, 0xac)} ready\n`,
                ),
                size_bytes: 9,
                sha256_hex: "a".repeat(64),
                created_at: "2026-05-31T10:12:01Z",
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
          return jsonResponse(visibleTopologyGraph());
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
          const items =
            createdJobOutputs.get(outputMatch[1]) ??
            (jobOutputsFixture as Record<string, unknown[]>)[outputMatch[1]] ??
            [];
          return jsonResponse({
            items,
            limit: 1000,
            next_cursor: null,
            has_more: false,
          });
        }
        const outputStreamMatch = pathname.match(
          /^\/api\/v1\/jobs\/([^/]+)\/outputs\/([^/]+)\/download$/,
        );
        if (outputStreamMatch && method === "GET") {
          const jobId = outputStreamMatch[1];
          const clientId = decodeURIComponent(outputStreamMatch[2]);
          const stream =
            new URL(url, window.location.href).searchParams.get("stream") ??
            "combined";
          const items =
            createdJobOutputs.get(jobId) ??
            (jobOutputsFixture as Record<string, FixtureJobOutput[]>)[jobId] ??
            [];
          const payload = items
            .filter(
              (item) =>
                item.client_id === clientId &&
                (stream === "combined" || item.stream === stream),
            )
            .sort((left, right) => (left.seq ?? 0) - (right.seq ?? 0))
            .map((item) => (item.data_base64 ? atob(item.data_base64) : ""))
            .join("");
          return Promise.resolve(
            new Response(payload, {
              headers: { "Content-Type": "application/octet-stream" },
              status: 200,
            }),
          );
        }
        const jobCancelMatch = pathname.match(
          /^\/api\/v1\/jobs\/([^/]+)\/cancel$/,
        );
        if (jobCancelMatch && method === "POST") {
          const jobId = decodeURIComponent(jobCancelMatch[1]);
          const body = await readJsonBody(input, init);
          const rollout = mutableJobRollouts.find(
            (record) => record.job_id === jobId,
          );
          if (!rollout) {
            return jsonResponse({ error: "job_not_found" }, 404);
          }
          const activeTargets = rollout.targets.filter((target) =>
            ["dispatching", "running"].includes(target.status),
          );
          const queuedTargets = rollout.targets.filter(
            (target) => target.status === "queued",
          );
          for (const target of queuedTargets) {
            target.status = "canceled";
            target.message = "operator_aborted_rollout";
          }
          rollout.status = "aborted";
          rollout.pause_reason = "operator_aborted_rollout";
          rollout.completed_at = "2026-06-02T10:03:00Z";
          rollout.updated_at = "2026-06-02T10:03:00Z";
          persistJobRollouts();
          requests.jobRolloutActions.push({
            action: "abort",
            body,
            job_id: jobId,
          });
          return jsonResponse({
            cancel_acks: activeTargets.map((target) => ({
              accepted: true,
              acked: true,
              applied: true,
              client_id: target.client_id,
              message: "cancellation applied",
            })),
            job_id: jobId,
            pending_canceled: queuedTargets.length,
            requested_targets: activeTargets.length + queuedTargets.length,
            status: "canceled",
          });
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
        const tagDeleteMatch = pathname.match(/^\/api\/v1\/tags\/([^/]+)$/);
        if (tagDeleteMatch && method === "DELETE") {
          const body = await readJsonBody(input, init);
          const tagName = decodeURIComponent(tagDeleteMatch[1]);
          requests.tagDeletes.push({ body, tag: tagName });
          const confirmed = Boolean(
            (body as { confirmed?: boolean } | null)?.confirmed,
          );
          const matchedTag = tagsFixture.find((tag) => tag.name === tagName);
          const affected =
            matchedTag?.clients ??
            visibleAgents().filter((agent) => agent.tags.includes(tagName));
          return jsonResponse({
            action: "delete",
            affected,
            changed_count: confirmed ? affected.length : 0,
            confirmation_required: !confirmed,
            preview_hash:
              (body as { preview_hash?: string | null } | null)?.preview_hash ??
              "6".repeat(64),
            schedule_impacts: bulkTagScheduleImpactsFixture,
            skipped_count: 0,
            tag: tagName,
            target_count: affected.length,
          });
        }
        if (pathname === "/api/v1/tags/bulk" && method === "POST") {
          if (bulkTagMutationDelayMsFixture > 0) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, bulkTagMutationDelayMsFixture),
            );
          }
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
            preview_hash: "7".repeat(64),
            schedule_impacts: bulkTagScheduleImpactsFixture,
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
            command_type:
              commandTypeForOperation(request.operation) ?? "shell_argv",
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
            target_client_ids:
              request.target_client_ids ??
              scheduleTargetIdsFromSelector(
                request.selector_expression ?? "id:*",
              ),
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
        const scheduleTargetsMatch = pathname.match(
          /^\/api\/v1\/schedules\/([^/]+)\/targets$/,
        );
        if (scheduleTargetsMatch && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.scheduleActions.push({ body, method, path: pathname });
          const schedule = findSchedule(scheduleTargetsMatch[1]);
          if (!schedule) {
            return jsonResponse({ error: "schedule_not_found" }, 404);
          }
          schedule.target_client_ids = scheduleTargetIdsFromSelector(
            schedule.selector_expression,
          );
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
            const selectedTargets = visibleAgents().filter((agent) =>
              fixedTargetIds.includes(agent.id),
            );
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
          return jsonResponse(backupPoliciesFixture);
        }
        const backupPolicyUpdateMatch = pathname.match(
          /^\/api\/v1\/backup-policies\/([^/]+)$/,
        );
        if (
          (pathname === "/api/v1/backup-policies" && method === "POST") ||
          (backupPolicyUpdateMatch && method === "PUT")
        ) {
          const body = await readJsonBody(input, init);
          if (backupPolicyUpdateMatch) {
            requests.backupPolicyUpdates.push({
              body,
              schedule_id: decodeURIComponent(backupPolicyUpdateMatch[1]),
            });
          } else {
            requests.backupPolicies.push(body);
          }
          const request = body as {
            catch_up_limit?: number;
            catch_up_policy?: string;
            cron_expr?: string;
            enabled?: boolean;
            follow_symlinks?: boolean;
            include_config?: boolean;
            keep_last?: number | null;
            max_failures?: number;
            name?: string;
            paths?: string[];
            retry_delay_secs?: number;
            retention_days?: number | null;
            rotation_generation?: string | null;
            selector_expression?: string;
            target_client_ids?: string[];
            timezone?: string;
          };
          const updatedPolicy = {
            catch_up_limit: request.catch_up_limit ?? 1,
            catch_up_policy: request.catch_up_policy ?? "skip_missed",
            created_at: "2026-06-02T10:11:00Z",
            cron_expr: request.cron_expr ?? "0 3 * * *",
            enabled: request.enabled ?? true,
            failure_count: 0,
            follow_symlinks: request.follow_symlinks ?? false,
            include_config: request.include_config ?? true,
            keep_last: request.keep_last ?? 7,
            cadence_error: null,
            last_error: null,
            last_run_at: null,
            max_failures: request.max_failures ?? 3,
            name: request.name ?? "backup-policy",
            next_run_at: "2026-06-03T03:00:00Z",
            next_runs: ["2026-06-03T03:00:00Z"],
            paths: request.paths ?? [],
            retry_delay_secs: request.retry_delay_secs ?? 300,
            retention_days: request.retention_days ?? 30,
            rotation_generation: request.rotation_generation ?? null,
            schedule_id:
              backupPolicyUpdateMatch?.[1] ??
              "62626262-6161-4717-8abc-defdefdefdef",
            selector_expression: request.selector_expression ?? "id:*",
            target_client_ids: request.target_client_ids ?? [],
            timezone: request.timezone ?? "UTC",
            updated_at: "2026-06-02T10:11:00Z",
          } as BackupPolicyRecord;
          if (backupPolicyUpdateMatch) {
            const updatedIndex = backupPoliciesFixture.findIndex(
              (policy) => policy.schedule_id === updatedPolicy.schedule_id,
            );
            if (updatedIndex >= 0) {
              backupPoliciesFixture.splice(updatedIndex, 1, updatedPolicy);
            } else {
              backupPoliciesFixture.push(updatedPolicy);
            }
          }
          return jsonResponse(updatedPolicy);
        }
        if (pathname === "/api/v1/backup-policies/prune" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.backupPolicyPrunes.push(body);
          const request = body as {
            confirmed?: boolean;
            dry_run?: boolean;
            metadata_only?: boolean | null;
            preview_hash?: string | null;
            schedule_id?: string | null;
          } | null;
          const dryRun = Boolean(request?.dry_run);
          const metadataOnly = request?.metadata_only ?? false;
          return jsonResponse({
            dry_run: dryRun,
            metadata_only_requested: request?.metadata_only ?? null,
            preview_hash:
              request?.preview_hash ??
              "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            policies: [
              {
                cutoff_unix: 1780000000,
                enabled: true,
                keep_last: 7,
                matched_rows: 3,
                metadata_only: metadataOnly,
                name: "nightly-system",
                object_delete_attempted: !dryRun && !metadataOnly,
                object_delete_errors: [],
                object_keys: metadataOnly ? [] : ["backups/agent-sfo-01/a.tar"],
                pruned_rows: dryRun ? 0 : 3,
                retention_days: 30,
                schedule_id:
                  request?.schedule_id ??
                  "62626262-6161-4717-8abc-defdefdefdef",
                status: "ok",
              },
            ],
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
              id: "dddddddd-eeee-4fff-8000-111111111111",
              object_key: `backups/agent-sfo-01/${backupArtifactHandoffMatch[1]}.tar`,
              sha256_hex: "1".repeat(64),
              size_bytes: 321,
              status: "active",
              content_available: true,
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
        if (
          pathname === "/api/v1/network/resolve-hostname" &&
          method === "POST"
        ) {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          return jsonResponse({
            candidates: [
              { address: "10.20.0.21", family: "ipv4" },
              { address: "2001:db8:20::21", family: "ipv6" },
            ],
            hostname: String(body.hostname ?? "app.internal"),
          });
        }
        if (pathname === "/api/v1/port-forward-rules" && method === "GET") {
          return jsonResponse(
            mutablePortForwardRules.filter(
              (rule) => !(rule.deleted_at && rule.removal_confirmed_at),
            ),
          );
        }
        if (pathname === "/api/v1/port-forward-rules" && method === "POST") {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.portForwardRules.push({ action: "create", body });
          const enabled = body.enabled !== false;
          const rule = {
            ...body,
            agent_desired_hash: null,
            created_at: "2026-06-02T10:10:00Z",
            deleted_at: null,
            desired_hash: enabled ? "f".repeat(64) : null,
            desired_status: enabled ? "enabled" : "disabled",
            enabled,
            forgotten_at: null,
            forwarding_enabled: null,
            id: `4f000000-0000-4000-8000-${String(mutablePortForwardRules.length + 10).padStart(12, "0")}`,
            nat_matches: 0,
            nft_version: null,
            observed_hash: null,
            removal_confirmed_at: null,
            revision: 1,
            runtime_error: null,
            runtime_error_code: null,
            runtime_observed_unix: null,
            runtime_status: enabled ? "pending" : "disabled",
            updated_at: "2026-06-02T10:10:00Z",
          };
          mutablePortForwardRules = [rule, ...mutablePortForwardRules];
          return jsonResponse(
            {
              rule,
              sync: {
                error: null,
                job_id: enabled ? "4f100000-0000-4000-8000-000000000001" : null,
                status: enabled ? "queued" : "saved_disabled",
              },
            },
            201,
          );
        }
        if (
          pathname === "/api/v1/port-forward-rules/bulk" &&
          method === "POST"
        ) {
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.portForwardRules.push({ action: "bulk", body });
          const action = String(body.action ?? "reapply");
          const selected = new Set(
            Array.isArray(body.items)
              ? body.items.map((item) =>
                  String(asFixtureRecord(item)?.id ?? ""),
                )
              : [],
          );
          const affectedClients = new Set<string>();
          const immediateRetiredClients = new Set<string>();
          mutablePortForwardRules = mutablePortForwardRules.flatMap((rule) => {
            if (!selected.has(rule.id)) return [rule];
            const retireImmediately =
              action === "delete" &&
              !rule.enabled &&
              rule.revision === 1 &&
              !rule.deleted_at;
            if (retireImmediately) {
              immediateRetiredClients.add(rule.client_id);
            } else {
              affectedClients.add(rule.client_id);
            }
            if (action === "reapply") return [rule];
            const next = {
              ...rule,
              desired_status:
                action === "delete"
                  ? "removal_pending"
                  : action === "enable"
                    ? "enabled"
                    : "disabled",
              enabled: action === "enable",
              revision: rule.revision + 1,
              runtime_status:
                action === "delete" ? "removal_pending" : "pending",
              updated_at: "2026-06-02T10:11:00Z",
              ...(action === "delete"
                ? {
                    deleted_at: "2026-06-02T10:11:00Z",
                    removal_confirmed_at: retireImmediately
                      ? "2026-06-02T10:11:00Z"
                      : null,
                  }
                : {}),
            };
            return [next];
          });
          return jsonResponse({
            rules: mutablePortForwardRules.filter((rule) =>
              selected.has(rule.id),
            ),
            sync: [
              ...[...affectedClients].map((clientId, index) => ({
                client_id: clientId,
                sync: {
                  error: null,
                  job_id: `4f100000-0000-4000-8000-${String(index + 20).padStart(12, "0")}`,
                  status: "queued",
                },
              })),
              ...[...immediateRetiredClients]
                .filter((clientId) => !affectedClients.has(clientId))
                .map((clientId) => ({
                  client_id: clientId,
                  sync: {
                    error: null,
                    job_id: null,
                    status: "retired_disabled_draft",
                  },
                })),
            ],
          });
        }
        const portForwardRuleMatch = pathname.match(
          /^\/api\/v1\/port-forward-rules\/([^/]+)(?:\/([^/]+))?$/,
        );
        if (portForwardRuleMatch && (method === "PUT" || method === "POST")) {
          const ruleId = decodeURIComponent(portForwardRuleMatch[1]);
          const operation = portForwardRuleMatch[2] ?? "update";
          const body = asFixtureRecord(await readJsonBody(input, init)) ?? {};
          requests.portForwardRules.push({
            action: operation,
            body,
            rule_id: ruleId,
          });
          const index = mutablePortForwardRules.findIndex(
            (rule) => rule.id === ruleId,
          );
          if (index < 0)
            return jsonResponse({ code: "port_forward_rule_not_found" }, 404);
          const current = mutablePortForwardRules[index];
          if (Number(body.expected_revision) !== current.revision) {
            return jsonResponse(
              { code: "port_forward_rule_snapshot_stale" },
              409,
            );
          }
          const retireImmediately =
            operation === "delete" &&
            !current.enabled &&
            current.revision === 1 &&
            !current.deleted_at;
          const enabled =
            operation === "enable"
              ? true
              : operation === "disable" || operation === "delete"
                ? false
                : Boolean(body.enabled ?? current.enabled);
          const next = {
            ...current,
            ...(operation === "update" ? body : {}),
            desired_status:
              operation === "delete"
                ? "removal_pending"
                : enabled
                  ? "enabled"
                  : "disabled",
            enabled,
            revision:
              operation === "reapply" ? current.revision : current.revision + 1,
            runtime_status:
              operation === "delete"
                ? "removal_pending"
                : operation === "reapply"
                  ? current.runtime_status
                  : "pending",
            updated_at: "2026-06-02T10:12:00Z",
            ...(operation === "delete"
              ? {
                  deleted_at: "2026-06-02T10:12:00Z",
                  removal_confirmed_at: retireImmediately
                    ? "2026-06-02T10:12:00Z"
                    : null,
                }
              : {}),
            ...(operation === "forget"
              ? { forgotten_at: "2026-06-02T10:12:00Z" }
              : {}),
          };
          if (operation === "forget") {
            mutablePortForwardRules.splice(index, 1);
          } else {
            mutablePortForwardRules[index] = next;
          }
          return jsonResponse({
            rule: next,
            sync: {
              error: null,
              job_id:
                operation === "forget" || retireImmediately
                  ? null
                  : "4f100000-0000-4000-8000-000000000002",
              status:
                operation === "forget"
                  ? "forgotten_without_host_cleanup"
                  : retireImmediately
                    ? "retired_disabled_draft"
                    : "queued",
            },
          });
        }
        if (pathname === "/api/v1/tunnel-plans" && method === "GET") {
          return jsonResponse(visibleTunnelPlans());
        }
        const tunnelPlanUpdateMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)$/,
        );
        if (tunnelPlanUpdateMatch && method === "PUT") {
          const planId = decodeURIComponent(tunnelPlanUpdateMatch[1]);
          const body = (await readJsonBody(input, init)) as Record<
            string,
            unknown
          >;
          requests.tunnelPlans.push(body);
          const plan = tunnelPlansFixture.find(
            (record) => record.id === planId,
          );
          if (!plan) {
            return jsonResponse(
              { error: "tunnel_plan_not_found", status: 404 },
              404,
            );
          }
          if (body.expected_revision !== plan.revision) {
            return jsonResponse(
              { error: "tunnel_plan_snapshot_stale", status: 409 },
              409,
            );
          }
          const {
            confirmed: _confirmed,
            enabled,
            expected_revision: _expectedRevision,
            ...planInput
          } = body;
          const mutablePlan = plan as unknown as Record<string, unknown>;
          mutablePlan.enabled = enabled ?? false;
          mutablePlan.connection_assessment = "automatic";
          mutablePlan.connection_assessment_note = null;
          mutablePlan.connection_assessed_at = null;
          mutablePlan.connection_assessed_by = null;
          mutablePlan.input = planInput;
          mutablePlan.kind = planInput.kind;
          mutablePlan.left_client_id = planInput.left_client_id;
          mutablePlan.name = planInput.name;
          mutablePlan.plan = {
            ...planInput,
            conflicts: [],
            left_tunnel_address:
              (planInput.ipv4_tunnel as { left?: string } | undefined)?.left ??
              (planInput.ipv6_tunnel as { left?: string } | undefined)?.left,
            recommended_ospf_cost: plan.recommended_ospf_cost,
            right_tunnel_address:
              (planInput.ipv4_tunnel as { right?: string } | undefined)
                ?.right ??
              (planInput.ipv6_tunnel as { right?: string } | undefined)?.right,
            tunnel_prefix_len:
              (planInput.ipv4_tunnel as { prefix_len?: number } | undefined)
                ?.prefix_len ??
              (planInput.ipv6_tunnel as { prefix_len?: number } | undefined)
                ?.prefix_len,
          };
          mutablePlan.revision = plan.revision + 1;
          mutablePlan.right_client_id = planInput.right_client_id;
          mutablePlan.updated_at = "2026-06-02T10:08:00Z";
          const sync = setRuntimeTunnelConfig(
            mutablePlan,
            mutablePlan.enabled === true,
          );
          return jsonResponse({ plan: mutablePlan, sync });
        }
        const tunnelPlanEnabledMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)\/(enable|disable)$/,
        );
        if (tunnelPlanEnabledMatch && method === "POST") {
          const planId = decodeURIComponent(tunnelPlanEnabledMatch[1]);
          const enabled = tunnelPlanEnabledMatch[2] === "enable";
          const body = (await readJsonBody(input, init)) as {
            expected_revision?: number;
          };
          requests.tunnelPlanEnabledMutations.push({
            enabled,
            plan_id: planId,
          });
          const plan = tunnelPlansFixture.find(
            (record) => record.id === planId,
          );
          if (plan) {
            if (body.expected_revision !== plan.revision) {
              return jsonResponse(
                { error: "tunnel_plan_snapshot_stale", status: 409 },
                409,
              );
            }
            plan.enabled = enabled;
            plan.revision += 1;
            plan.connection_assessment = "automatic";
            plan.connection_assessment_note = null;
            plan.connection_assessed_at = null;
            plan.connection_assessed_by = null;
            plan.updated_at = "2026-06-02T10:08:00Z";
            const sync = setRuntimeTunnelConfig(
              plan as unknown as Record<string, unknown>,
              enabled,
            );
            return jsonResponse({ plan, sync });
          }
          return jsonResponse({ code: "tunnel_plan_not_found" }, 400);
        }
        const tunnelPlanAssessmentMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)\/connection-assessment$/,
        );
        if (tunnelPlanAssessmentMatch && method === "PUT") {
          const planId = decodeURIComponent(tunnelPlanAssessmentMatch[1]);
          const body = (await readJsonBody(input, init)) as {
            assessment?: "automatic" | "connected" | "disconnected";
            expected_revision?: number;
            note?: string | null;
          };
          requests.tunnelPlanConnectionAssessments.push({
            body,
            plan_id: planId,
          });
          const plan = tunnelPlansFixture.find(
            (record) => record.id === planId,
          );
          if (!plan) {
            return jsonResponse({ code: "tunnel_plan_not_found" }, 404);
          }
          if (body.expected_revision !== plan.revision) {
            return jsonResponse({ code: "tunnel_plan_snapshot_stale" }, 409);
          }
          plan.revision += 1;
          plan.connection_assessment = body.assessment ?? "automatic";
          plan.connection_assessment_note =
            body.assessment === "automatic" ? null : (body.note ?? null);
          plan.connection_assessed_at =
            body.assessment === "automatic" ? null : "2026-06-02T10:10:00Z";
          plan.connection_assessed_by =
            body.assessment === "automatic"
              ? null
              : "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
          return jsonResponse(plan);
        }
        const tunnelPlanDeleteMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)\/delete$/,
        );
        if (tunnelPlanDeleteMatch && method === "POST") {
          const planId = decodeURIComponent(tunnelPlanDeleteMatch[1]);
          const body = (await readJsonBody(input, init)) as {
            expected_revision?: number;
          };
          requests.tunnelPlanDeletes.push({
            expected_revision: body.expected_revision,
            plan_id: planId,
          });
          const plan = tunnelPlansFixture.find(
            (record) =>
              record.id === planId && !deletedTunnelPlanIds.has(record.id),
          );
          if (!plan) {
            return jsonResponse({ code: "tunnel_plan_not_found" }, 404);
          }
          if (body.expected_revision !== plan.revision) {
            return jsonResponse({ code: "tunnel_plan_snapshot_stale" }, 409);
          }
          const sync = runtimeTunnelDispatch(
            plan.left_client_id,
            plan.right_client_id,
          );
          plan.revision += 1;
          plan.enabled = false;
          plan.left_runtime_config = runtimeTunnelConfig(
            plan.left_client_id,
            false,
          );
          plan.right_runtime_config = runtimeTunnelConfig(
            plan.right_client_id,
            false,
          );
          plan.left_runtime_config.status = "queued";
          plan.right_runtime_config.status = "queued";
          plan.left_runtime_config.job_id = sync[0].job_id;
          plan.right_runtime_config.job_id = sync[1].job_id;
          plan.deleted_at = "2026-06-02T10:09:00Z";
          plan.deleted_by = "99999999-aaaa-4bbb-8ccc-000000000001";
          plan.deleted_reason = "operator_retired";
          plan.updated_at = "2026-06-02T10:09:00Z";
          deletedTunnelPlanIds.add(plan.id);
          return jsonResponse({ plan, sync });
        }
        const tunnelPlanOspfCostMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)\/ospf-cost$/,
        );
        const tunnelPlanOspfStatusMatch = pathname.match(
          /^\/api\/v1\/tunnel-plans\/([^/]+)\/ospf-status$/,
        );
        if (tunnelPlanOspfStatusMatch && method === "POST") {
          const planId = decodeURIComponent(tunnelPlanOspfStatusMatch[1]);
          requests.tunnelPlanOspfStatusChecks.push({ plan_id: planId });
          const plan = tunnelPlansFixture.find(
            (record) => record.id === planId,
          );
          if (plan) {
            plan.ospf_status = "pending";
            plan.left_ospf_status = "pending";
            plan.right_ospf_status = "pending";
            plan.left_ospf_job_id = "11111111-aaaa-4bbb-8ccc-111111111111";
            plan.right_ospf_job_id = "22222222-aaaa-4bbb-8ccc-222222222222";
            plan.updated_at = "2026-06-02T10:08:00Z";
            const updatePlan = ospfUpdatePlansFixture.find(
              (record) => record.plan_id === planId,
            );
            if (updatePlan) {
              updatePlan.left_ospf_status = "pending";
              updatePlan.right_ospf_status = "pending";
              updatePlan.status = "in_progress";
            }
            return jsonResponse({
              plan,
              jobs: [],
              dispatch: runtimeTunnelDispatch(
                plan.left_client_id,
                plan.right_client_id,
              ).map((outcome, index) => ({
                ...outcome,
                endpoint_side: index === 0 ? "left" : "right",
              })),
            });
          }
          return jsonResponse({ code: "tunnel_plan_not_found" }, 400);
        }
        if (tunnelPlanOspfCostMatch && method === "POST") {
          const planId = decodeURIComponent(tunnelPlanOspfCostMatch[1]);
          const body = await readJsonBody(input, init);
          requests.tunnelPlanOspfCostUpdates.push({ body, plan_id: planId });
          const plan = tunnelPlansFixture.find(
            (record) => record.id === planId,
          );
          if (plan) {
            const nextCost =
              (body as { desired_ospf_cost?: number }).desired_ospf_cost ??
              plan.plan.recommended_ospf_cost;
            plan.desired_ospf_cost = nextCost;
            plan.ospf_status = "pending";
            plan.left_ospf_status = "pending";
            plan.right_ospf_status = "pending";
            plan.left_ospf_job_id = "33333333-aaaa-4bbb-8ccc-333333333333";
            plan.right_ospf_job_id = "44444444-aaaa-4bbb-8ccc-444444444444";
            plan.updated_at = "2026-06-02T10:08:00Z";
            const updatePlan = ospfUpdatePlansFixture.find(
              (record) => record.plan_id === planId,
            );
            if (updatePlan) {
              updatePlan.left_ospf_status = "pending";
              updatePlan.right_ospf_status = "pending";
              updatePlan.status = "in_progress";
            }
            return jsonResponse({
              plan,
              jobs: [],
              dispatch: runtimeTunnelDispatch(
                plan.left_client_id,
                plan.right_client_id,
              ).map((outcome, index) => ({
                ...outcome,
                endpoint_side: index === 0 ? "left" : "right",
              })),
            });
          }
          return jsonResponse({ code: "tunnel_plan_not_found" }, 400);
        }
        if (pathname === "/api/v1/tunnel-plans/allocate" && method === "POST") {
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
          const body = (await readJsonBody(input, init)) as Record<
            string,
            unknown
          >;
          requests.tunnelPlans.push(body);
          const enabled = (body.enabled as boolean | undefined) ?? false;
          const plan = {
            ...tunnelPlansFixture[0],
            desired_ospf_cost: null,
            enabled,
            id: "bbbbbbbb-aaaa-4bbb-8ccc-eeeeeeeeeeee",
            revision: 1,
            input: body,
            kind:
              (body.kind as string | undefined) ?? tunnelPlansFixture[0].kind,
            left_client_id:
              (body.left_client_id as string | undefined) ??
              tunnelPlansFixture[0].left_client_id,
            left_current_ospf_cost: null,
            left_ospf_job_id: null,
            left_ospf_status: body.ospf ? "needs_status" : "disabled",
            name:
              (body.name as string | undefined) ?? tunnelPlansFixture[0].name,
            ospf_status: body.ospf ? "needs_status" : "disabled",
            plan: {
              ...body,
              conflicts: [],
              left_tunnel_address:
                (body.ipv4_tunnel as { left?: string } | undefined)?.left ??
                "10.255.50.0",
              recommended_ospf_cost: null,
              right_tunnel_address:
                (body.ipv4_tunnel as { right?: string } | undefined)?.right ??
                "10.255.50.1",
              tunnel_prefix_len:
                (body.ipv4_tunnel as { prefix_len?: number } | undefined)
                  ?.prefix_len ?? 31,
            },
            recommended_ospf_cost: null,
            right_client_id:
              (body.right_client_id as string | undefined) ??
              tunnelPlansFixture[0].right_client_id,
            right_current_ospf_cost: null,
            right_ospf_job_id: null,
            right_ospf_status: body.ospf ? "needs_status" : "disabled",
            updated_at: "2026-06-02T10:08:00Z",
          };
          const sync = enabled
            ? setRuntimeTunnelConfig(
                plan as unknown as Record<string, unknown>,
                true,
              )
            : [];
          if (!enabled) {
            setRuntimeTunnelConfig(
              plan as unknown as Record<string, unknown>,
              false,
            );
          }
          return jsonResponse({ plan, sync });
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
          return jsonResponse(auditLogsFixture);
        }
        if (pathname.startsWith("/api/v1/audit/")) {
          const auditId = decodeURIComponent(
            pathname.slice("/api/v1/audit/".length),
          );
          const audit =
            (auditDetailFixture?.id === auditId ? auditDetailFixture : null) ??
            auditLogsFixture.find(
              (record: { id: string }) => record.id === auditId,
            );
          return audit
            ? jsonResponse(audit)
            : jsonResponse(
                {
                  error: "audit_event_not_found",
                  message: "Audit event not found",
                  recovery: "Return to the audit event list.",
                  status: 404,
                },
                404,
              );
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
              audit_logs: auditLogsFixture,
              backup_artifacts: artifactsFixture,
              client_status_history: [],
              gateway_sessions: [],
              job_outputs: [],
              network_observations: [],
              system_metric_rollups: [],
              telemetry_network_rates: [],
              telemetry_rollups: [],
              topology_history: { graph: {}, trends: [] },
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
          if (bulkResolveDelayMsFixture > 0) {
            await new Promise((resolve) =>
              window.setTimeout(resolve, bulkResolveDelayMsFixture),
            );
          }
          if (bulkResolveFailureFixture) {
            return jsonResponse(
              {
                error: "target_resolver_unavailable",
                message: "Target inventory could not be read",
                status: 503,
              },
              503,
            );
          }
          const targets = resolveBulkTargets(body);
          return jsonResponse({
            target_count: targets.length,
            targets,
          });
        }
        if (pathname === "/api/v1/runtime-config/patch" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.runtimeConfigPatches.push(body);
          const targets = resolveBulkTargets(body);
          return jsonResponse({
            target_count: targets.length,
            overrides: targets.map((agent) => ({
              client_id: agent.id,
              reason:
                (body as { reason?: string | null }).reason ??
                "Runtime config patch",
              toml: (body as { toml?: string }).toml ?? "",
              updated_at: "2026-06-02T10:08:00Z",
              updated_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            })),
            sync_job_ids: targets.map(
              (_, index) =>
                `99999999-9999-4999-8999-${String(index + 1).padStart(12, "0")}`,
            ),
            sync: targets.map((agent, index) => ({
              client_id: agent.id,
              error: null,
              job_id: `99999999-9999-4999-8999-${String(index + 1).padStart(12, "0")}`,
              status: "queued",
            })),
          });
        }
        if (pathname === "/api/v1/jobs" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.jobs.push(body);
          const targets = resolveBulkTargets(body);
          const commandType =
            (body as { command?: string } | null)?.command ?? "job";
          const rolloutPolicy = (
            body as {
              rollout?: {
                batch_delay_secs: number;
                batch_size: number;
                canary_client_ids: string[];
                max_failures: number;
                pause_after_canary: boolean;
              } | null;
            } | null
          )?.rollout;
          const targetRecords = targets.map((agent) => ({
            client_id: agent.id,
            completed_at: "2026-05-31T10:09:00Z",
            exit_code:
              agent.status === "stale"
                ? 2
                : agent.status === "offline"
                  ? null
                  : 0,
            message: (agent.status === "stale"
              ? `stale: agent rejected ${commandType} command_version 3`
              : agent.status === "offline"
                ? "agent offline"
                : "completed") as string | null,
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
          if (commandType === "storage_inventory") {
            const operation = (
              body as {
                operation?: { include_pseudo_mounts?: boolean };
              }
            ).operation;
            hostStorageInventoryFixture.include_pseudo_mounts = Boolean(
              operation?.include_pseudo_mounts,
            );
            hostStorageInventoryFixture.source_job_id = jobId;
            hostStorageInventoryFixture.observed_at = "2026-06-02T10:06:00Z";
            hostStorageInventoryFixture.last_attempt = {
              completed_at: "2026-06-02T10:06:00Z",
              job_id: jobId,
              message: "completed",
              status: "completed",
            };
            persistHostStorage();
          }
          if (rolloutPolicy) {
            const canaryIds = new Set(rolloutPolicy.canary_client_ids);
            for (const target of targetRecords) {
              const isCanary = canaryIds.has(target.client_id);
              target.completed_at = isCanary ? "2026-05-31T10:09:00Z" : null;
              target.exit_code = isCanary ? 0 : null;
              target.message = isCanary ? "completed" : null;
              target.started_at = isCanary ? "2026-05-31T10:08:55Z" : null;
              target.status = isCanary ? "completed" : "queued";
            }
            const batchByClient = new Map<string, number>();
            for (const canaryId of rolloutPolicy.canary_client_ids) {
              batchByClient.set(canaryId, 0);
            }
            const remaining = targets
              .map((agent) => agent.id)
              .filter((clientId) => !canaryIds.has(clientId))
              .sort();
            const batchSize = Math.max(1, rolloutPolicy.batch_size);
            for (let index = 0; index < remaining.length; index += 1) {
              batchByClient.set(
                remaining[index],
                Math.floor(index / batchSize) + 1,
              );
            }
            const totalBatches =
              Math.max(0, ...Array.from(batchByClient.values())) + 1;
            const currentBatch = totalBatches > 1 ? 1 : 0;
            const nextRollout: JobRolloutRecord = {
              ...rolloutPolicy,
              canary_client_ids: [...rolloutPolicy.canary_client_ids],
              completed_at: null,
              created_at: "2026-05-31T10:08:55Z",
              current_batch: currentBatch,
              failure_baseline: 0,
              job_id: jobId,
              next_batch_at: "2026-05-31T10:09:00Z",
              pause_reason:
                rolloutPolicy.pause_after_canary && totalBatches > 1
                  ? "canary_review"
                  : null,
              status:
                rolloutPolicy.pause_after_canary && totalBatches > 1
                  ? "paused"
                  : "running",
              targets: targetRecords.map((target) => ({
                batch_index: batchByClient.get(target.client_id) ?? 0,
                client_id: target.client_id,
                message: target.message,
                status: target.status,
              })),
              total_batches: totalBatches,
              updated_at: "2026-05-31T10:09:00Z",
            };
            mutableJobRollouts = [
              nextRollout,
              ...mutableJobRollouts.filter(
                (rollout) => rollout.job_id !== jobId,
              ),
            ];
            persistJobRollouts();
            const createdJob = {
              actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
              command_type: commandType,
              completed_at: null,
              created_at: "2026-05-31T10:08:55Z",
              id: jobId,
              max_timeout_secs:
                (body as { max_timeout_secs?: number } | null)
                  ?.max_timeout_secs ?? 30,
              payload_hash: "1".repeat(64),
              privileged: true,
              source_schedule_id: null,
              status: "running",
              target_count: targets.length,
            };
            const existingIndex = (
              jobsFixture as Array<{ id: string }>
            ).findIndex((job) => job.id === jobId);
            if (existingIndex >= 0) {
              jobsFixture.splice(existingIndex, 1, createdJob);
            } else {
              jobsFixture.unshift(createdJob);
            }
            window.sessionStorage.setItem(
              createdRolloutJobStorageKey,
              JSON.stringify(createdJob),
            );
          }
          createdJobTargets.set(jobId, targetRecords);
          if (commandType === "package_update_plan") {
            const operation = (
              body as {
                operation?: { refresh_metadata?: boolean };
              }
            ).operation;
            for (const target of targetRecords) {
              if (target.status !== "completed") continue;
              const plan = mutableHostPackageUpdatePlans.find(
                (item) => item.client_id === target.client_id,
              );
              if (!plan) continue;
              plan.last_attempt = {
                completed_at: "2026-06-02T10:09:00Z",
                job_id: jobId,
                message: "completed",
                status: "completed",
              };
              plan.metadata_refresh_requested = Boolean(
                operation?.refresh_metadata,
              );
              plan.metadata_refreshed = Boolean(operation?.refresh_metadata);
              plan.observed_at = "2026-06-02T10:09:00Z";
              plan.source_job_id = jobId;
            }
          }
          if (commandType === "package_update_apply") {
            const operation = (
              body as {
                operation?: { plan_hash?: string; provider?: string };
              }
            ).operation;
            const outputs: FixtureJobOutput[] = [];
            for (const [index, target] of targetRecords.entries()) {
              if (target.status !== "completed") continue;
              const plan = mutableHostPackageUpdatePlans.find(
                (item) => item.client_id === target.client_id,
              );
              const appliedPackageCount = plan?.packages.length ?? 0;
              if (plan) {
                plan.packages = [];
                plan.plan_hash = "c".repeat(64);
                plan.observed_at = "2026-06-02T10:10:00Z";
                plan.reboot_required_before = true;
              }
              outputs.push({
                client_id: target.client_id,
                created_at: "2026-06-02T10:10:00Z",
                data_base64: btoa(
                  JSON.stringify({
                    accepted_plan_hash: operation?.plan_hash ?? "",
                    applied_package_count: appliedPackageCount,
                    completed: true,
                    provider: operation?.provider ?? "apt",
                    reboot_required_after: true,
                    remaining_packages: [],
                    type: "package_update_apply",
                  }),
                ),
                done: true,
                exit_code: 0,
                job_id: jobId,
                seq: index,
                stream: "stdout",
              });
            }
            createdJobOutputs.set(jobId, outputs);
          }
          if (commandType === "config_read") {
            createdJobOutputs.set(
              jobId,
              targetRecords.map((target, index) => ({
                client_id: target.client_id,
                created_at: "2026-05-31T10:09:00Z",
                data_base64: btoa(
                  JSON.stringify({
                    config_sha256_hex: "b".repeat(64),
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
          if (commandType === "service_logs") {
            const operation = (
              body as {
                operation?: { provider?: string; service?: string };
              }
            ).operation;
            createdJobOutputs.set(
              jobId,
              targetRecords.flatMap((target, index) => [
                {
                  client_id: target.client_id,
                  created_at: "2026-05-31T10:09:00Z",
                  data_base64: btoa(
                    JSON.stringify({
                      lines: [
                        "Jun 02 10:03:58 host sshd[812]: Server listening on 0.0.0.0 port 22.",
                        "Jun 02 10:04:01 host sshd[812]: Accepted publickey for operator.",
                      ],
                      provider: operation?.provider ?? "systemd",
                      service: operation?.service ?? "sshd.service",
                      truncated: false,
                      type: "service_logs",
                    }),
                  ),
                  done: true,
                  exit_code: 0,
                  job_id: jobId,
                  seq: index,
                  stream: "stdout",
                },
              ]),
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

      const testWebSockets: TestWebSocket[] = [];

      class TestWebSocket extends EventTarget {
        static CONNECTING = 0;
        static OPEN = 1;
        static CLOSING = 2;
        static CLOSED = 3;

        readyState = TestWebSocket.CONNECTING;
        url: string;

        constructor(url: string) {
          super();
          this.url = url;
          testWebSockets.push(this);
          window.setTimeout(() => {
            if (this.readyState !== TestWebSocket.CONNECTING) {
              return;
            }
            this.readyState = TestWebSocket.OPEN;
            this.dispatchEvent(new Event("open"));
          }, 0);
        }

        close() {
          this.readyState = TestWebSocket.CLOSED;
          this.dispatchEvent(new CloseEvent("close"));
        }

        send(data: string) {
          const url = new URL(this.url, window.location.href);
          const match = url.pathname.match(
            /^\/ws\/terminal\/([^/]+)\/([^/]+)$/,
          );
          if (!match) {
            return;
          }
          const frame = JSON.parse(data) as Record<string, unknown>;
          const clientId = decodeURIComponent(match[1]);
          const sessionId = decodeURIComponent(match[2]);
          const session = terminalSessionsFixture.find(
            (candidate: { client_id: string; session_id: string }) =>
              candidate.client_id === clientId &&
              candidate.session_id === sessionId,
          );
          const dispatch = (message: Record<string, unknown>) =>
            this.dispatchEvent(
              new MessageEvent("message", { data: JSON.stringify(message) }),
            );
          if (frame.type === "auth") {
            if (!session) {
              dispatch({
                type: "error",
                message: "terminal_session_not_found",
                recoverable: false,
              });
              return;
            }
            const fromSeq = Number(frame.from_seq ?? 1);
            dispatch({
              type: "ready",
              from_seq: fromSeq,
              available_first_seq: 1,
              next_seq: session.output_next_seq,
              replay_truncated: false,
              session: { ...session },
            });
            const chunks = [
              {
                data_base64: btoa("durable replay line 1\n"),
                terminal_seq: 1,
              },
              {
                data_base64: btoa(
                  `prompt$ ${String.fromCharCode(0xe2)}`,
                ),
                terminal_seq: 2,
              },
              {
                data_base64: btoa(
                  `${String.fromCharCode(0x82, 0xac)} ready\n`,
                ),
                terminal_seq: 3,
              },
            ];
            for (const chunk of chunks) {
              if (chunk.terminal_seq >= fromSeq) {
                dispatch({ type: "output", ...chunk });
              }
            }
            dispatch({ type: "session_state", session: { ...session } });
            return;
          }
          if (
            !session ||
            typeof frame.request_id !== "string" ||
            !["input", "resize", "close"].includes(String(frame.type))
          ) {
            return;
          }
          requests.terminalControls.push(frame);
          const terminalTestWindow = window as typeof window & {
            __vpsmanRejectNextTerminalControl?: string | null;
          };
          if (terminalTestWindow.__vpsmanRejectNextTerminalControl === frame.type) {
            terminalTestWindow.__vpsmanRejectNextTerminalControl = null;
            dispatch({
              type: "error",
              message: `terminal_${String(frame.type)}_rejected_for_test`,
              recoverable: true,
              request_id: frame.request_id,
            });
            return;
          }
          const ack: Record<string, unknown> = {
            request_id: frame.request_id,
            session_id: sessionId,
            action: frame.type,
            accepted: true,
            status: "accepted",
            message: `terminal_${String(frame.type)}_accepted`,
          };
          if (frame.type === "input") {
            session.last_input_seq += 1;
            ack.input_seq = session.last_input_seq;
            ack.written_bytes = atob(String(frame.data_base64 ?? "")).length;
          } else if (frame.type === "resize") {
            session.cols = Number(frame.cols);
            session.rows = Number(frame.rows);
            ack.cols = session.cols;
            ack.rows = session.rows;
          } else {
            session.state = "closed";
            session.last_status = "closed";
            session.last_event = "terminal_close";
            session.close_reason = String(frame.reason ?? "operator");
            ack.status = "closed";
          }
          const dispatchAck = () => {
            requests.terminalControlAcks.push(ack);
            dispatch({ type: "control_ack", ack });
            if (frame.type === "close") {
              dispatch({ type: "session_state", session: { ...session } });
            }
          };
          const ackDelayMs = Number(
            (
              window as typeof window & {
                __vpsmanTerminalControlAckDelayMs?: number;
              }
            ).__vpsmanTerminalControlAckDelayMs ?? 0,
          );
          if (ackDelayMs > 0) {
            window.setTimeout(dispatchAck, ackDelayMs);
          } else {
            dispatchAck();
          }
        }
      }

      Object.defineProperty(window, "WebSocket", {
        configurable: true,
        value: TestWebSocket,
      });
      Object.defineProperty(window, "__vpsmanTestWebSockets", {
        configurable: true,
        value: testWebSockets,
      });
    },
    {
      agentListOverrideFixture: options.agentListOverride ?? null,
      agentDeleteDelayMsFixture: options.agentDeleteDelayMs ?? 0,
      agentDeleteFailedClientIdsFixture:
        options.agentDeleteFailedClientIds ?? [],
      agentDeleteRequestFailureFixture:
        options.agentDeleteRequestFailure ?? false,
      agentDeleteSyncJobIdsFixture: options.agentDeleteSyncJobIds ?? [
        "d1000000-0000-4000-8000-000000000001",
      ],
      agentsFixture: agents,
      agentUpdateReleasesFixture: agentUpdateReleases,
      auditDetailFixture: options.auditDetailOverride ?? null,
      auditLogsFixture: options.auditLogsOverride ?? auditLogs,
      backupPoliciesFixture: options.backupPoliciesOverride ?? [],
      bulkTagMutationDelayMsFixture: options.bulkTagMutationDelayMs ?? 0,
      bulkTagScheduleImpactsFixture: options.bulkTagScheduleImpacts ?? [],
      bulkResolveDelayMsFixture: options.bulkResolveDelayMs ?? 0,
      artifactsFixture:
        options.backupArtifactsOverride ??
        (options.recordPagesSaturated
          ? [
              ...backupArtifacts,
              ...Array.from(
                { length: 1_000 - backupArtifacts.length },
                (_, index) => ({
                  ...backupArtifacts[0],
                  client_id: "agent-fra-02",
                  id: `b0000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
                  object_key: `backups/agent-fra-02/filler-${index}.tar`,
                }),
              ),
            ]
          : backupArtifacts),
      backupsFixture: options.recordPagesSaturated
        ? [
            ...backupRequests,
            ...Array.from(
              { length: 1_000 - backupRequests.length },
              (_, index) => ({
                ...backupRequests[0],
                client_id: "agent-fra-02",
                id: `a0000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
              }),
            ),
          ]
        : backupRequests,
      bulkResolveFailureFixture: options.bulkResolveFailure ?? false,
      configurationSourceApplyFailureFixture:
        options.configurationSourceApplyFailure ?? false,
      configurationSourceSyncFailureFixture:
        options.configurationSourceSyncFailure ?? false,
      dashboardOverviewFixture: dashboardOverview,
      dashboardLatestSampleAtOverrideFixture:
        options.dashboardLatestSampleAtOverride ?? null,
      dashboardCountsTruncatedFixture:
        options.dashboardCountsTruncated ?? false,
      dashboardSummaryOverrideFixture: options.dashboardSummaryOverride ?? null,
      systemDashboardFixture: systemDashboard,
      configurationPresetsFixture: configurationPresets,
      configurationSourcesFixture: configurationSources,
      networkAdapterDefinitionsFixture: networkAdapterDefinitions,
      runtimeConfigApplyStatesFixture: runtimeConfigApplyStates,
      runtimeConfigApplyFailureFixture:
        options.runtimeConfigApplyFailure ?? false,
      runtimeConfigPatchGeneratorsFixture: runtimeConfigPatchGenerators,
      jobCommandTypeByOperationTypeFixture: JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
      commandTemplatesFixture: commandTemplates,
      clientKeyRevocationsFixture: clientKeyRevocations,
      keyLifecycleReportFixture: keyLifecycleReport,
      fleetAlertNotificationChannelsFixture:
        options.fleetAlertNotificationChannelsOverride ??
        fleetAlertNotificationChannels,
      fleetAlertNotificationsFixture: options.alertEvidenceSaturated
        ? Array.from({ length: 200 }, (_, index) => ({
            ...fleetAlertNotifications[0],
            id: `fdfdfdfd-aaaa-4aaa-8aaa-${String(index).padStart(12, "0")}`,
          }))
        : fleetAlertNotifications,
      fleetAlertPoliciesFixture: fleetAlertPolicies,
      fleetAlertStateFailureFixture: options.fleetAlertStateFailure ?? false,
      fleetAlertStatesFixture: fleetAlertStates,
      fleetAlertsFixture: options.alertStateCoverage
        ? [
            {
              ...fleetAlerts[3],
              id: "fleet-alert-state-open",
              operator_state: "open",
              severity: "warning",
              title: "Open daily alert",
            },
            {
              ...fleetAlerts[3],
              escalation_level: 1,
              id: "fleet-alert-state-escalated",
              operator_state: "escalated",
              severity: "critical",
              title: "Escalated daily alert",
            },
            {
              ...fleetAlerts[3],
              id: "fleet-alert-state-muted",
              muted_until_unix: 1_900_000_000,
              operator_state: "muted",
              severity: "critical",
              title: "Muted daily alert",
            },
            {
              ...fleetAlerts[2],
              id: "fleet-alert-state-acknowledged",
              title: "Acknowledged daily alert",
            },
          ]
        : options.recordPagesSaturated
          ? [
              ...fleetAlerts,
              ...Array.from(
                { length: 200 - fleetAlerts.length },
                (_, index) => ({
                  ...fleetAlerts[0],
                  id: `fleet-alert-filler-${String(index).padStart(3, "0")}`,
                  target_id: `agent-fra-02:filler-${index}`,
                }),
              ),
            ]
          : fleetAlerts,
      policyAlertsFixture: options.alertEvidenceSaturated
        ? Array.from({ length: 200 }, (_, index) => ({
            ...policyAlerts[0],
            id: `policy-alert-saturated-${String(index).padStart(3, "0")}`,
          }))
        : policyAlerts,
      policyDryRunFixture,
      portForwardRulesFixture: portForwardRules,
      fileTransferSourceArtifactsFixture:
        options.fileTransferSourceArtifactsOverride ??
        fileTransferSourceArtifacts,
      fileTransfersFixture:
        options.fileTransfersOverride ??
        (options.recordPagesSaturated
          ? [
              ...fileTransfers,
              ...Array.from(
                { length: 200 - fileTransfers.length },
                (_, index) => ({
                  ...fileTransfers[1],
                  session_id: `70000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
                }),
              ),
            ]
          : fileTransfers),
      historyRetentionPoliciesFixture: historyRetentionPolicies,
      hostProcessInventoryFixture: hostProcessInventory("agent-sfo-01"),
      hostPackageUpdatePlansFixture:
        options.hostPackageUpdatePlansOverride ?? hostPackageUpdatePlans(),
      hostServiceInventoryFixture:
        options.hostServiceInventoryOverride ??
        hostServiceInventory("agent-sfo-01"),
      hostStorageInventoryFixture:
        options.hostStorageInventoryOverride ??
        hostStorageInventory("agent-sfo-01"),
      jobApprovalsFixture: jobApprovals,
      jobRolloutsFixture: options.jobRolloutsOverride ?? jobRollouts,
      jobOutputsFixture: networkJobOutputs,
      jobsFixture: networkJobs,
      networkObservationsFixture: networkObservations,
      ospfRecommendationsFixture: ospfRecommendations,
      ospfUpdatePlansFixture:
        options.ospfUpdatePlansOverride ?? ospfUpdatePlans,
      networkTrendsFixture: networkTrends,
      operatorPreferencesFixture: operatorPreferences,
      operatorAuthEventsFixture: options.operatorAuthEventsOverride ?? null,
      operatorRoleOverrideFixture: options.operatorRoleOverride ?? "admin",
      privilegeVerificationDelayMsFixture:
        options.privilegeVerificationDelayMs ?? 0,
      privilegeVerificationFailureFixture:
        options.privilegeVerificationFailure ?? null,
      processSupervisorInventoryFixture: processSupervisorInventory,
      schedulesFixture: options.schedulesOverride ?? schedules,
      summaryFixture: summary,
      suiteConfigRedactedFixture: suiteConfigRedacted,
      suiteConfigTomlFixture: suiteConfigToml,
      suiteConfigValidationFixture: suiteConfigValidation,
      tagsFixture: tags,
      telemetryFailurePathFixture: options.telemetryFailurePath ?? null,
      telemetryNetworkRateScalesFixture: options.telemetryNetworkRateScales ?? [
        1,
      ],
      terminalSessionsFixture:
        options.terminalSessionsOverride ?? terminalSessions,
      totpSetupDelayMsFixture: options.totpSetupDelayMs ?? 0,
      totpSetupOperatorIdOverrideFixture:
        options.totpSetupOperatorIdOverride ?? null,
      totpSetupSwitchSessionFixture: options.totpSetupSwitchSession ?? false,
      topologyGraphFixture: topologyGraph,
      trafficAccountingFixture: trafficAccounting,
      tunnelPlansFixture: tunnelPlans,
      portSpeedRulesDelayMsFixture: options.portSpeedRulesDelayMs ?? 0,
      vpsRulesApplyDelayMsFixture: options.vpsRulesApplyDelayMs ?? 0,
      vpsRuleValuesFixture: [
        ...vpsRuleValues,
        ...(options.portSpeedRulesOverride ?? []),
      ],
      webhookDeliveriesFixture: webhookDeliveries,
      webhookRulesFixture: webhookRules,
    },
  );
  await installTransferJobApiMock(page);
}
