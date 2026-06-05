import type { Page } from "@playwright/test";
import { dataSourceAssignments, dataSourcePresets } from "./dataSourcePresetFixtures";
import { fileTransferSourceArtifacts, fileTransfers, terminalSessions } from "./jobSessionFixtures";
import { installTransferJobApiMock } from "./transferJobMock";

export { buildEncryptedBackupArtifactFixture, sha256Hex } from "./backupArtifactFixture";

const statusOutput = (value: unknown) => Buffer.from(JSON.stringify(value)).toString("base64");

const summary = {
  connected: 2,
  running_jobs: 3,
  total: 3,
  warnings: 1,
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
    status: "connected",
    tags: ["country:US", "provider:alpha"],
  },
  {
    capabilities: rootCapabilities,
    display_name: "core-fra-02",
    id: "agent-fra-02",
    status: "connected",
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
    severity: "critical",
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
    severity: "warning",
    status: "stale",
    target_id: "agent-nyc-03",
    target_kind: "agent",
    title: "Agent is not connected",
  },
  {
    category: "source_readiness",
    client_id: "agent-sfo-01",
    detail: "Backup object store: backup object-store preset is selected, but no server object store is configured",
    evidence: { domain: "backup_object_store" },
    id: "fleet-alert-source-agent-sfo-01-backup",
    observed_at: "2026-06-02T10:00:00Z",
    severity: "warning",
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
    client_status: "connected",
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
    status_reason: "latest traffic samples are available from the selected preset",
  },
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-fra-02",
    client_status: "connected",
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
    status_reason: "latest interface counters are available from the selected preset",
  },
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-sfo-01",
    client_status: "connected",
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
    status_reason: "backup object-store preset is selected, but no server object store is configured",
  },
  {
    assigned_at: "2026-06-02T10:00:00Z",
    client_id: "agent-sfo-01",
    client_status: "connected",
    display_name: "edge-sfo-01",
    domain: "update_artifact_source",
    evidence: {
      continuous_status: false,
      external_release_count: 1,
      hosted_release_count: 0,
      release_count: 1,
      server_object_store_configured: false,
      server_object_store_kind: null,
      workflow: "agent_update_releases",
    },
    module: "Update artifact source",
    preset_id: "00000000-0000-4000-8000-00000000000a",
    preset_name: "builtin:local_filesystem_or_https",
    preset_scope: "built_in",
    source_kind: "local_filesystem_or_https",
    status: "metadata_only",
    status_reason: "signed HTTPS update release metadata exists; hosted artifact storage is optional",
  },
];

const enrollmentTokens = [
  {
    assigned_client_id: null,
    allowed_client_id: null,
    created_at: "2026-05-31T10:00:00Z",
    created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
    default_tags: ["edge"],
    expected_old_public_key_sha256_hex: null,
    expires_at: "2026-05-31T10:30:00Z",
    id: "abababab-cdcd-4efe-8aaa-bbbbbbbbbbbb",
    preserve_existing_assignments: true,
    purpose: "provision",
    requires_existing_client: false,
    token_prefix: "vpsm12345678",
    used_at: null,
    used_by_client_id: null,
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
  active_rebuild_reenrollment_token_count: 0,
  clients: agents.map((agent, index) => ({
    client_id: agent.id,
    current_key_revoked: agent.id === "agent-nyc-03",
    current_public_key_sha256_hex: (index + 1).toString(16).repeat(64),
    display_name: agent.display_name,
    latest_revocation_reason: agent.id === "agent-nyc-03" ? "fixture rebuild" : null,
    latest_revoked_at: agent.id === "agent-nyc-03" ? "2026-05-31T10:01:00Z" : null,
    status: agent.status,
  })),
  current_key_revoked_count: 1,
  discovery_trusted_server_key_count: 1,
  enrolled_client_count: agents.length,
  gateway_server_public_key_configured: true,
  rebuild_reenrollment_token_count: 0,
  revocation_count: clientKeyRevocations.length,
  server_ed25519_public_key_configured: true,
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
    proof_command_id: null,
    proof_expires_unix: null,
    proof_scope: "client:agent-sfo-01",
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
      touched_files: ["/etc/network/interfaces.d/vpsman-tunnels", "/etc/bird/vpsman-ospf.conf"],
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
      validation_steps: ["confirm the external tunnel is present before routing apply"],
      rollback_notes: ["remove only the matching vpsman-managed Bird2 interface block"],
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

const agentUpdateRollouts = [
  {
    activation_policy: "manual_staging_only",
    actor_id: null,
    artifact_sha256_hex: "d".repeat(64),
    artifact_signature_provided: true,
    artifact_signing_key_sha256_hex: "e".repeat(64),
    canary_count: 0,
    completed_count: 1,
    created_at: "2026-05-31T10:09:55Z",
    failed_count: 0,
    heartbeat_timeout_secs: null,
    automation_paused: false,
    automation_pause_reason: null,
    automation_health_gate: "heartbeat_verified",
    automation_lease_owner: null,
    automation_lease_expires_at: null,
    automation_status: "ready_activate_canary",
    automation_next_action: "operator_activate_batch",
    automation_blocker: "privileged rollout dispatch requires fresh per-target proof",
    automation_targets: ["agent-sfo-01"],
    automation_updated_at: "2026-05-31T10:10:01Z",
    activation_delegations: [
      {
        action: "agent_update_activate",
        created_at: "2026-05-31T10:09:59Z",
        dispatch_job_id: null,
        dispatched_count: 0,
        dispatching_count: 0,
        expired_count: 0,
        failed_count: 0,
        payload_hash: "a".repeat(64),
        proof_expires_unix_max: 1780308600,
        proof_expires_unix_min: 1780308600,
        ready_count: 1,
        restart_agent: true,
        rollout_id: "12121212-3434-4567-8abc-defdefdefdef",
        staged_sha256_hex: "d".repeat(64),
        target_count: 1,
        updated_at: "2026-05-31T10:09:59Z",
      },
    ],
    id: "12121212-3434-4567-8abc-defdefdefdef",
    job_id: "66666666-aaaa-4bbb-8ccc-dddddddddddd",
    pending_count: 0,
    rollback_delegations: [],
    status: "staged",
    target_count: 1,
    targets: [
      {
        client_id: "agent-sfo-01",
        exit_code: 0,
        status: "completed",
        updated_at: "2026-05-31T10:10:00Z",
      },
    ],
    updated_at: "2026-05-31T10:10:00Z",
  },
];

const agentUpdateRolloutPolicies = [
  {
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    automation_health_gate: "heartbeat_verified",
    canary_count: 1,
    channel: "stable",
    created_at: "2026-05-31T10:08:00Z",
    enabled: true,
    id: "34343434-5656-4789-8abc-defdefdefdef",
    name: "stable-default",
    notes: "fixture policy preset for smoke coverage",
    priority: 0,
    scope_kind: "global",
    scope_value: null,
    updated_at: "2026-05-31T10:08:00Z",
  },
];

const agentUpdateReleases = [
  {
    actor_id: null,
    artifact_sha256_hex: "d".repeat(64),
    artifact_signature_provided: true,
    artifact_signature_sha256_hex: "a".repeat(64),
    artifact_signing_key_sha256_hex: "e".repeat(64),
    artifact_object_key: null,
    artifact_download_path: null,
    artifact_url_sha256_hex: "f".repeat(64),
    channel: "stable",
    created_at: "2026-05-31T10:08:55Z",
    id: "23232323-3434-4567-8abc-defdefdefdef",
    name: "vpsman-agent",
    notes: "signed smoke metadata",
    size_bytes: 1024,
    status: "published_metadata_only",
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
      status: "connected",
      tags: ["provider:alpha", "pool:west"],
      tunnel_count: 1,
    },
    {
      applied_tunnel_count: 1,
      client_id: "agent-fra-02",
      degraded_tunnel_count: 0,
      display_name: "core-fra-02",
      latest_observed_at: "2026-05-31T10:09:00Z",
      status: "connected",
      tags: ["bgp", "bird2", "pool:europe"],
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
    change_summary: "Change Bird2 OSPF cost on tunab from 14 to 22 for both tunnel endpoints",
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
    proof_required: true,
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

export async function installConsoleApiMock(page: Page) {
  await page.addInitScript(
    ({
      agentsFixture,
      agentUpdateRolloutPoliciesFixture,
      agentUpdateRolloutsFixture,
      agentUpdateReleasesFixture,
      artifactsFixture,
      backupsFixture,
      dataSourceAssignmentsFixture,
      dataSourcePresetsFixture,
      dataSourceStatusFixture,
      clientKeyRevocationsFixture,
      enrollmentTokensFixture,
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
      processSupervisorInventoryFixture,
      summaryFixture,
      tagsFixture,
      terminalSessionsFixture,
      topologyGraphFixture,
      tunnelPlansFixture,
    }) => {
      const originalFetch = window.fetch.bind(window);
      const requests = {
        backupArtifactHandoffs: [] as unknown[],
        backupArtifactRestorePreparations: [] as unknown[],
        agentUpdateRolloutPolicies: [] as unknown[],
        bulkResolve: [] as unknown[],
        dataSourcePresetAssignments: [] as unknown[],
        dataSourcePresets: [] as unknown[],
        enrollmentTokens: [] as unknown[],
        clientKeyRevocations: [] as unknown[],
        fleetAlertNotificationDispatches: [] as unknown[],
        fleetAlertNotificationProcesses: [] as unknown[],
        fleetAlertNotificationChannels: [] as unknown[],
        fleetAlertPolicies: [] as unknown[],
        fleetAlertStates: [] as unknown[],
        fileTransferHandoffs: [] as unknown[],
        fileTransferSourceUploads: [] as unknown[],
        historyRetentionPolicies: [] as unknown[],
        historyRetentionPrunes: [] as unknown[],
        jobs: [] as unknown[],
        migrationLinks: [] as unknown[],
        restorePlans: [] as unknown[],
        tunnelPlanAdapterPromotions: [] as unknown[],
        tunnelPlans: [] as unknown[],
      };
      Object.defineProperty(window, "__vpsmanTestRequests", {
        configurable: true,
        value: requests,
      });
      const jsonResponse = (body: unknown, status = 200) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            headers: { "Content-Type": "application/json" },
            status,
          }),
        );
      const emptyArrayResponse = () => jsonResponse([]);

      const readJsonBody = async (input: RequestInfo | URL, init?: RequestInit) => {
        const body = init?.body;
        if (typeof body === "string") {
          return JSON.parse(body) as unknown;
        }
        if (input instanceof Request) {
          return input.clone().json() as Promise<unknown>;
        }
        return null;
      };
      const artifactBodyForTransfer = (clientId: string, sessionId: string) =>
        `server-side transfer handoff ${clientId} ${sessionId}`;
      const sha256HexForText = async (value: string) => {
        const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
        return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
      };
      const bytesToBase64 = (bytes: Uint8Array) => {
        let binary = "";
        for (const byte of bytes) {
          binary += String.fromCharCode(byte);
        }
        return btoa(binary);
      };
      const resolveBulkTargets = (body: unknown) => {
        const request = body as { clients?: string[]; tags?: string[] } | null;
        const selected = new Map<string, (typeof agentsFixture)[number]>();
        const selectedClients = new Set(request?.clients ?? []);
        const selectedTags = new Set(request?.tags ?? []);
        for (const agent of agentsFixture) {
          if (selectedClients.has(agent.id) || agent.tags.some((tag) => selectedTags.has(tag))) {
            selected.set(agent.id, agent);
          }
        }
        const targets = [...selected.values()].sort((left, right) => left.id.localeCompare(right.id));
        return targets.length > 0 ? targets : [agentsFixture[0]];
      };

      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        const pathname = new URL(url, window.location.href).pathname;
        const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
        if (pathname === "/api/v1/fleet/summary") {
          return jsonResponse(summaryFixture);
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
          const request = body as { action?: string; alert_id?: string; muted_for_secs?: number | null; reason?: string | null };
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
        if (pathname === "/api/v1/fleet-alert-notification-channels" && method === "GET") {
          return jsonResponse(fleetAlertNotificationChannelsFixture);
        }
        if (pathname === "/api/v1/fleet-alert-notification-channels" && method === "POST") {
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
        if (pathname === "/api/v1/fleet-alert-notifications" && method === "GET") {
          return jsonResponse(fleetAlertNotificationsFixture);
        }
        if (pathname === "/api/v1/fleet-alert-notifications/dispatch" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.fleetAlertNotificationDispatches.push(body);
          return jsonResponse(fleetAlertNotificationsFixture);
        }
        if (pathname === "/api/v1/fleet-alert-notifications/process" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.fleetAlertNotificationProcesses.push(body);
          return jsonResponse(
            fleetAlertNotificationsFixture.map((delivery: Record<string, unknown>) => ({
              ...delivery,
              status: (body as { dry_run?: boolean } | null)?.dry_run ? delivery.status : "sent",
              updated_at: "2026-06-02T10:03:00Z",
            })),
          );
        }
        if (pathname === "/api/v1/agents") {
          return jsonResponse(agentsFixture);
        }
        if (pathname === "/api/v1/gateway-sessions" && method === "GET") return emptyArrayResponse();
        if (pathname === "/api/v1/auth/me" && method === "GET") return jsonResponse({ id: "99999999-aaaa-4bbb-8ccc-000000000001", role: "admin", scopes: ["*"], totp_enabled: false, username: "console-admin" });
        if (pathname === "/api/v1/operators" && method === "GET") {
          return jsonResponse([
            { id: "99999999-aaaa-4bbb-8ccc-000000000001", role: "admin", scopes: ["*"], totp_enabled: false, username: "console-admin" },
            { id: "99999999-aaaa-4bbb-8ccc-000000000002", role: "operator", scopes: ["fleet:read", "jobs:write"], totp_enabled: true, username: "noc-operator" },
          ]);
        }
        if (pathname === "/api/v1/operator-sessions" && method === "GET") return jsonResponse([{ id: "88888888-aaaa-4bbb-8ccc-000000000001", operator_id: "99999999-aaaa-4bbb-8ccc-000000000001", operator_role: "admin", operator_username: "console-admin", current: true, created_at: "2026-01-01T00:00:00Z", expires_at: "2026-01-01T00:15:00Z", refresh_expires_at: "2026-01-15T00:00:00Z", revoked: false, revoked_at: null }]);
        if (pathname === "/api/v1/enrollment-tokens" && method === "GET") return jsonResponse(enrollmentTokensFixture);
        if (pathname === "/api/v1/client-key-revocations" && method === "GET") return jsonResponse(clientKeyRevocationsFixture);
        if (pathname === "/api/v1/key-lifecycle/report" && method === "GET") return jsonResponse(keyLifecycleReportFixture);
        if (pathname === "/api/v1/auth/proof-rotations" && method === "GET") return emptyArrayResponse();
        if (pathname.startsWith("/api/v1/clients/") && pathname.endsWith("/key-revocations") && method === "POST") {
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
        if (pathname === "/api/v1/enrollment-tokens" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.enrollmentTokens.push(body);
          const purpose = (body as { purpose?: string }).purpose ?? "provision";
          const assignedClientId =
            purpose === "rebuild_reenrollment"
              ? (body as { allowed_client_id?: string | null }).allowed_client_id ?? "agent-sfo-01"
              : "11111111-2222-4333-8444-555555555555";
          return jsonResponse({
            ...(body as Record<string, unknown>),
            assigned_client_id: assignedClientId,
            allowed_client_id: assignedClientId,
            created_at: "2026-05-31T10:05:00Z",
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            expected_old_public_key_sha256_hex: purpose === "rebuild_reenrollment" ? "f".repeat(64) : null,
            expires_at: "2026-05-31T10:35:00Z",
            id: "bcbcbcbc-dede-4faf-8bbb-cccccccccccc",
            requires_existing_client: purpose === "rebuild_reenrollment",
            token: purpose === "rebuild_reenrollment" ? "vpsm_rebuild_token_secret" : "vpsm_provision_token_secret",
            token_prefix: purpose === "rebuild_reenrollment" ? "vpsm_rebuild" : "vpsm_provision",
            used_at: null,
            used_by_client_id: null,
          });
        }
        if (pathname === "/api/v1/telemetry/rollups" && method === "GET") return emptyArrayResponse();
        if (pathname === "/api/v1/telemetry/network-rates" && method === "GET") return jsonResponse([{ client_id: "agent-fra-02", interface: "eth0", bucket_start: "2026-05-31T10:00:00Z", bucket_secs: 300, sample_count: 2, rx_bytes_delta: 65536, tx_bytes_delta: 131072, rx_bps_avg: 8738, tx_bps_avg: 17476, first_observed_at: "2026-05-31T10:01:00Z", latest_observed_at: "2026-05-31T10:02:00Z", updated_at: "2026-05-31T10:02:05Z" }]);
        if (pathname === "/api/v1/telemetry/tunnels" && method === "GET") return jsonResponse([{ client_id: "agent-fra-02", observed_at: "2026-05-31T10:02:00Z", interface: "tun0", kind: "tun_tap", ownership_mode: "runtime_observed", source: "sysfs_proc_net_dev", operstate: "up", mtu: 1500, link_type: 65534, address: "00:00:00:00:00:00", rx_bytes: 65536, tx_bytes: 131072 }]);
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
        if (pathname === "/api/v1/data-source-assignments" && method === "GET") {
          return jsonResponse(dataSourceAssignmentsFixture);
        }
        if (pathname === "/api/v1/data-source-status" && method === "GET") {
          return jsonResponse(dataSourceStatusFixture);
        }
        if (pathname === "/api/v1/data-source-assignments" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.dataSourcePresetAssignments.push(body);
          const request = body as { preset_id?: string };
          const preset =
            dataSourcePresetsFixture.find((record: { id: string }) => record.id === request.preset_id) ??
            dataSourcePresetsFixture[0];
          return jsonResponse({
            assignments: dataSourceAssignmentsFixture,
            confirmation_required: false,
            preset,
            target_count: 1,
          });
        }
        if (pathname === "/api/v1/jobs" && method === "GET") {
          return jsonResponse(jobsFixture);
        }
        if (pathname === "/api/v1/agent-update-rollouts" && method === "GET") {
          return jsonResponse(agentUpdateRolloutsFixture);
        }
        if (pathname === "/api/v1/agent-update-rollout-policies" && method === "GET") {
          return jsonResponse(agentUpdateRolloutPoliciesFixture);
        }
        if (pathname === "/api/v1/agent-update-rollout-policies" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.agentUpdateRolloutPolicies.push(body);
          const request = body as {
            automation_health_gate?: string | null;
            canary_count?: number | null;
            channel?: string | null;
            enabled?: boolean;
            name?: string;
            notes?: string | null;
            priority?: number;
            scope_kind?: string;
            scope_value?: string | null;
          };
          return jsonResponse({
            actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
            automation_health_gate: request.automation_health_gate ?? null,
            canary_count: request.canary_count ?? null,
            channel: request.channel ?? null,
            created_at: "2026-06-02T10:04:00Z",
            enabled: request.enabled ?? true,
            id: "45454545-5656-4789-8abc-defdefdefdef",
            name: request.name ?? "policy",
            notes: request.notes ?? null,
            priority: request.priority ?? 0,
            scope_kind: request.scope_kind ?? "global",
            scope_value: request.scope_value ?? null,
            updated_at: "2026-06-02T10:04:00Z",
          });
        }
        if (pathname === "/api/v1/agent-update-releases" && method === "GET") {
          return jsonResponse(agentUpdateReleasesFixture);
        }
        if (pathname === "/api/v1/process-supervisor/inventory" && method === "GET") {
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
          const request = body as { name?: string; sha256_hex?: string; size_bytes?: number };
          return jsonResponse({
            id: "73737373-2222-4333-8444-555555555555",
            name: request.name ?? "source.bin",
            object_key: `file-transfer-sources/${request.sha256_hex}.bin`,
            sha256_hex: request.sha256_hex,
            size_bytes: request.size_bytes,
            created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
            created_at: "2026-05-31T10:12:00Z",
            download_path: "/api/v1/file-transfer-sources/73737373-2222-4333-8444-555555555555/artifact",
          });
        }
        if (
          pathname === "/api/v1/file-transfer-sources/62626262-2222-4333-8444-555555555555/artifact" &&
          method === "GET"
        ) {
          return Promise.resolve(
            new Response("stored source artifact", {
              headers: { "Content-Type": "application/octet-stream" },
            }),
          );
        }
        const handoffMatch = pathname.match(/^\/api\/v1\/file-transfers\/([^/]+)\/([^/]+)\/handoff$/);
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
            object_key: `file-transfers/${Array.from(new TextEncoder().encode(clientId), (byte) =>
              byte.toString(16).padStart(2, "0"),
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
                "Content-Length": String(new TextEncoder().encode(artifactBody).byteLength),
                "Content-Type": "application/octet-stream",
                "x-vpsman-artifact-sha256": await sha256HexForText(artifactBody),
              },
            }),
          );
        }
        if (pathname === "/api/v1/terminal-sessions" && method === "GET") {
          return jsonResponse(terminalSessionsFixture);
        }
        if (
          pathname === "/api/v1/terminal-sessions/agent-sfo-01/61616161-2222-4333-8444-555555555555/replay" &&
          method === "GET"
        ) {
          return jsonResponse({
            session_id: "61616161-2222-4333-8444-555555555555",
            client_id: "agent-sfo-01",
            from_seq: Number(new URL(url, window.location.href).searchParams.get("from_seq") ?? "1"),
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
        if (pathname === "/api/v1/network/observation-trends" && method === "GET") {
          return jsonResponse(networkTrendsFixture);
        }
        if (pathname === "/api/v1/network/ospf-recommendations" && method === "GET") {
          return jsonResponse(ospfRecommendationsFixture);
        }
        if (pathname === "/api/v1/network/ospf-update-plans" && method === "GET") {
          return jsonResponse(ospfUpdatePlansFixture);
        }
        if (pathname === "/api/v1/network/topology-graph" && method === "GET") {
          return jsonResponse(topologyGraphFixture);
        }
        const outputMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs$/);
        if (outputMatch && method === "GET") {
          return jsonResponse((jobOutputsFixture as Record<string, unknown[]>)[outputMatch[1]] ?? []);
        }
        const jobMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)$/);
        if (jobMatch && method === "GET") {
          return jsonResponse(
            (jobsFixture as Array<{ id: string }>).find((job) => job.id === jobMatch[1]) ?? {
              id: jobMatch[1],
              status: "completed",
            },
          );
        }
        if (pathname === "/api/v1/tags") {
          return jsonResponse(tagsFixture);
        }
        if (pathname === "/api/v1/backups" && method === "GET") {
          return jsonResponse(backupsFixture);
        }
        if (pathname === "/api/v1/backup-policies" && method === "GET") {
          return jsonResponse([]);
        }
        if (pathname === "/api/v1/backup-artifacts" && method === "GET") {
          return jsonResponse(artifactsFixture);
        }
        const backupArtifactHandoffMatch = pathname.match(
          /^\/api\/v1\/backups\/([^/]+)\/artifact-handoff$/,
        );
        if (backupArtifactHandoffMatch && method === "POST") {
          const body = (await readJsonBody(input, init)) as { job_id?: string | null };
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
            source_job_id: body.job_id ?? "99999999-2222-4333-8444-555555555555",
          });
        }
        const backupArtifactPrepareRestoreMatch = pathname.match(
          /^\/api\/v1\/backups\/([^/]+)\/artifact\/prepare-restore$/,
        );
        if (backupArtifactPrepareRestoreMatch && method === "POST") {
          const body = (await readJsonBody(input, init)) as {
            artifact_base64?: string | null;
            private_key_hex?: string | null;
          };
          requests.backupArtifactRestorePreparations.push(body);
          const fileBody = "edge-sfo-01\n";
          const archive = {
            client_id: "agent-sfo-01",
            created_unix: 1_780_000_000,
            files: [
              {
                data_base64: btoa(fileBody),
                mode: 0o644,
                path: "/etc/hostname",
                sha256_hex: await sha256HexForText(fileBody),
                size_bytes: new TextEncoder().encode(fileBody).byteLength,
                source: "selected_path",
              },
            ],
            format: "vpsman.backup_archive.v1",
          };
          const archiveText = JSON.stringify(archive);
          const archiveBytes = new TextEncoder().encode(archiveText);
          return jsonResponse({
            archive_base64: bytesToBase64(archiveBytes),
            archive_sha256_hex: await sha256HexForText(archiveText),
            archive_size_bytes: archiveBytes.byteLength,
            artifact_client_id: "agent-sfo-01",
            archive_format: "vpsman.backup_archive.v1",
            file_count: 1,
          });
        }
        if (pathname === "/api/v1/restore-plans" && method === "GET") {
          return emptyArrayResponse();
        }
        if (pathname === "/api/v1/migration-links" && method === "GET") {
          return emptyArrayResponse();
        }
        if (pathname === "/api/v1/tunnel-plans" && method === "GET") {
          return jsonResponse(tunnelPlansFixture);
        }
        if (pathname === "/api/v1/tunnel-plans" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.tunnelPlans.push(body);
          return jsonResponse(tunnelPlansFixture[0]);
        }
        if (pathname === "/api/v1/tunnel-plans/promote-adapter" && method === "POST") {
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
            destination_root: "/restore",
            id: "cccccccc-dddd-4eee-8fff-000000000000",
            include_config: false,
            note: null,
            paths: ["/etc/hostname"],
            payload_hash: "c".repeat(64),
            proof_command_id: null,
            proof_expires_unix: null,
            proof_scope: "client:agent-fra-02",
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
            destination_root: "/restore",
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
        if (pathname === "/api/v1/history/retention-policies" && method === "GET") {
          return jsonResponse(historyRetentionPoliciesFixture);
        }
        if (pathname === "/api/v1/history/retention-policies" && method === "POST") {
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
        if (pathname === "/api/v1/history/retention-prune" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.historyRetentionPrunes.push(body);
          const request = body as { domain?: string | null; dry_run?: boolean; metadata_only?: boolean | null } | null;
          const domains = historyRetentionPoliciesFixture.filter(
            (policy: { domain: string }) => !request?.domain || policy.domain === request.domain,
          );
          return jsonResponse({
            dry_run: Boolean(request?.dry_run),
            metadata_only_requested: request?.metadata_only ?? null,
            domains: domains.map((policy: { domain: string; enabled: boolean; retention_days: number; metadata_only: boolean }) => ({
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
            })),
          });
        }
        if (pathname === "/api/v1/history/export" && method === "GET") {
          const requestedDomains =
            new URL(url, window.location.href).searchParams.get("domains") ??
            historyRetentionPoliciesFixture.map((policy: { domain: string }) => policy.domain).join(",");
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
            limit: Number(new URL(url, window.location.href).searchParams.get("limit") ?? "25"),
          });
        }
        if (pathname === "/api/v1/bulk/resolve" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.bulkResolve.push(body);
          const targets = resolveBulkTargets(body);
          return jsonResponse({
            confirmation_required: false,
            destructive: false,
            target_count: targets.length,
            targets,
          });
        }
        if (pathname === "/api/v1/jobs" && method === "POST") {
          const body = await readJsonBody(input, init);
          requests.jobs.push(body);
          return jsonResponse({
            accepted_targets: 1,
            job_id: "11111111-2222-4333-8444-555555555555",
            status: "accepted",
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
      agentsFixture: agents,
      agentUpdateRolloutPoliciesFixture: agentUpdateRolloutPolicies,
      agentUpdateRolloutsFixture: agentUpdateRollouts,
      agentUpdateReleasesFixture: agentUpdateReleases,
      artifactsFixture: backupArtifacts,
      backupsFixture: backupRequests,
      dataSourceAssignmentsFixture: dataSourceAssignments,
      dataSourcePresetsFixture: dataSourcePresets,
      dataSourceStatusFixture: dataSourceStatus,
      clientKeyRevocationsFixture: clientKeyRevocations,
      enrollmentTokensFixture: enrollmentTokens,
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
      processSupervisorInventoryFixture: processSupervisorInventory,
      summaryFixture: summary,
      tagsFixture: tags,
      terminalSessionsFixture: terminalSessions,
      topologyGraphFixture: topologyGraph,
      tunnelPlansFixture: tunnelPlans,
    },
  );
  await installTransferJobApiMock(page);
}
