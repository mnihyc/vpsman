import type {
  ConfigurationBehavior,
  ConfigurationPresetRecord,
  ConfigurationSourceView,
  NetworkAdapterDefinitionRecord,
} from "../../src/types";

const createdAt = "2026-06-02T10:00:00Z";

const preset = (
  id: string,
  behavior: ConfigurationBehavior,
  name: string,
  definition: Record<string, unknown>,
  options: {
    description?: string;
    effectiveVpsCount?: number;
    isDefault?: boolean;
    kind?: "system" | "custom";
    overrideVpsCount?: number;
  } = {},
): ConfigurationPresetRecord => ({
  behavior,
  created_at: createdAt,
  definition,
  description: options.description ?? null,
  effective_vps_count: options.effectiveVpsCount ?? 0,
  id,
  is_default: options.isDefault ?? false,
  kind: options.kind ?? "system",
  name,
  override_vps_count: options.overrideVpsCount ?? 0,
  updated_at: createdAt,
});

export const configurationPresets: ConfigurationPresetRecord[] = [
  preset(
    "00000000-0000-4000-8000-000000000001",
    "host_metrics",
    "Linux host metrics",
    {
      source: "linux_procfs",
      proc_root: "/proc",
      sys_class_net_dir: "/sys/class/net",
      hostname_file: "/etc/hostname",
      os_release_file: "/etc/os-release",
    },
    {
      description: "Collect host metrics from standard Linux paths.",
      effectiveVpsCount: 3,
      isDefault: true,
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000011",
    "host_metrics",
    "Host-mounted Linux metrics",
    {
      source: "linux_procfs",
      proc_root: "/host/proc",
      sys_class_net_dir: "/host/sys/class/net",
      hostname_file: "/host/etc/hostname",
      os_release_file: "/host/etc/os-release",
    },
    {
      description: "Collect metrics from host files mounted beneath /host.",
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000002",
    "tunnel_traffic",
    "Interface traffic counters",
    { source: "interface_counters" },
    {
      description: "Use Linux interface counters for tunnel traffic.",
      effectiveVpsCount: 2,
      isDefault: true,
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000003",
    "latency_probe",
    "Linux latency probe",
    { source: "linux_ping_preset" },
    {
      description: "Use the agent's bounded Linux ping candidates.",
      effectiveVpsCount: 3,
      isDefault: true,
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000004",
    "ospf_update_command",
    "Unconfigured OSPF updater",
    {
      contract_version: 2,
      status_command: null,
      update_command: null,
    },
    {
      description:
        "Do not run OSPF commands until an operator assigns a configured preset.",
      effectiveVpsCount: 1,
      isDefault: true,
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000005",
    "process_inventory",
    "Linux process inventory",
    { source: "linux_procfs", proc_root: "/proc" },
    {
      description: "Read process inventory from /proc.",
      effectiveVpsCount: 3,
      isDefault: true,
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000006",
    "user_sessions",
    "Linux user sessions",
    { source: "linux_w_who_preset" },
    {
      description: "Use the agent's bounded Linux w/who candidates.",
      effectiveVpsCount: 3,
      isDefault: true,
    },
  ),
  preset(
    "00000000-0000-4000-8000-000000000007",
    "command_execution",
    "Standard command execution",
    {
      shell_script_argv: ["/bin/sh", "-lc"],
      working_directory: null,
      environment_policy: "inherit",
      environment_keep: [],
      environment_set: {},
      pty_policy: "native_pty",
      process_cleanup: "process_group",
    },
    {
      description: "Use the standard Linux shell and inherited environment.",
      effectiveVpsCount: 3,
      isDefault: true,
    },
  ),
  preset(
    "11111111-1111-4111-8111-111111111111",
    "tunnel_traffic",
    "Edge vnStat",
    { source: "vnstat", vnstat_argv: ["/usr/bin/vnstat"] },
    {
      description: "Use vnStat on edge images where it is installed.",
      effectiveVpsCount: 1,
      kind: "custom",
      overrideVpsCount: 1,
    },
  ),
  preset(
    "22222222-2222-4222-8222-222222222222",
    "process_inventory",
    "Host-mounted processes",
    { source: "linux_procfs", proc_root: "/host/proc" },
    {
      description: "Read processes from a host procfs mount.",
      kind: "custom",
    },
  ),
  preset(
    "66666666-6666-4666-8666-666666666666",
    "ospf_update_command",
    "FRR OSPF updater",
    {
      contract_version: 2,
      status_command: {
        argv: [
          "/opt/operator/frr-ospf-cost",
          "status",
          "--plan-id",
          "{plan_id}",
          "--interface",
          "{interface}",
          "--side",
          "{endpoint_side}",
        ],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
      update_command: {
        argv: [
          "/opt/operator/frr-ospf-cost",
          "apply",
          "--plan-id",
          "{plan_id}",
          "--interface",
          "{interface}",
          "--side",
          "{endpoint_side}",
          "--cost",
          "{desired_cost}",
        ],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
    },
    {
      description: "Read and update OSPF cost through the operator-owned FRR adapter.",
      effectiveVpsCount: 2,
      kind: "custom",
      overrideVpsCount: 2,
    },
  ),
];

const defaultPresetByBehavior = new Map(
  configurationPresets
    .filter((record) => record.is_default)
    .map((record) => [record.behavior, record]),
);

const behaviors = [
  "host_metrics",
  "tunnel_traffic",
  "latency_probe",
  "ospf_update_command",
  "process_inventory",
  "user_sessions",
  "command_execution",
] as const;

export const configurationSources: ConfigurationSourceView[] = [
  ...["agent-sfo-01", "agent-fra-02", "agent-nyc-03"].flatMap((clientId) =>
    behaviors.map((behavior) => {
      const inherited = defaultPresetByBehavior.get(behavior)!;
      const customTraffic =
        clientId === "agent-sfo-01" && behavior === "tunnel_traffic"
          ? configurationPresets.find(
              (record) =>
                record.id === "11111111-1111-4111-8111-111111111111",
            )!
          : null;
      const customOspf =
        clientId !== "agent-nyc-03" && behavior === "ospf_update_command"
          ? configurationPresets.find(
              (record) =>
                record.id === "66666666-6666-4666-8666-666666666666",
            )!
          : null;
      const staleLatency =
        clientId === "agent-fra-02" && behavior === "latency_probe";
      const notObserved = clientId === "agent-nyc-03";
      const effective = customTraffic ?? customOspf ?? inherited;
      const hasOverride = customTraffic !== null || customOspf !== null;
      return {
        behavior,
        client_id: clientId,
        effective_preset_id: effective.id,
        effective_preset_kind: effective.kind,
        effective_preset_name: effective.name,
        override_updated_at: hasOverride ? "2026-06-02T10:03:00Z" : null,
        readiness:
          behavior === "ospf_update_command" && customOspf === null
            ? {
                evidence: { command_configured: false },
                reason:
                  "This VPS's effective OSPF updater preset is unconfigured; assign a configured preset or use a tunnel-plan endpoint override.",
                state: "unconfigured",
              }
            : notObserved
          ? {
              evidence: {},
              reason: "The stale VPS has not reported this behavior yet.",
              state: "not_observed",
            }
          : {
              evidence: { observed: true },
              reason: "Agent evidence matches the effective preset.",
              state: "ready",
            },
        runtime_sync: staleLatency
          ? {
              reason: "The latest runtime acknowledgement is older than desired.",
              state: "stale",
            }
          : notObserved
            ? {
                reason: "No runtime acknowledgement has been observed.",
                state: "unknown",
              }
            : {
                reason: "The effective configuration is applied.",
                state: "applied",
              },
        selection_origin: hasOverride
          ? "explicit_override"
          : "system_default",
      };
    }),
  ),
];

export const networkAdapterDefinitions: NetworkAdapterDefinitionRecord[] = [
  {
    adapter_kind: "runtime_tunnel",
    created_at: createdAt,
    definition: {
      cleanup_command: {
        argv: ["/opt/operator/tunnel-adapter", "cleanup", "{interface}"],
        max_output_bytes: 16384,
        max_timeout_secs: 20,
      },
      contract_version: 1,
      manager: "custom_adapter",
      startup_command: {
        argv: ["/opt/operator/tunnel-adapter", "start", "{interface}"],
        max_output_bytes: 16384,
        max_timeout_secs: 20,
      },
      status_command: {
        argv: ["/opt/operator/tunnel-adapter", "status", "{interface}"],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
    },
    description: "Operator-owned tunnel lifecycle integration.",
    id: "33333333-3333-4333-8333-333333333333",
    name: "Tunnel lifecycle v1",
    updated_at: createdAt,
  },
  {
    adapter_kind: "routing_cost",
    created_at: createdAt,
    definition: {
      contract_version: 2,
      status_command: {
        argv: [
          "/opt/operator/routing-cost",
          "status",
          "--plan-id",
          "{plan_id}",
          "--interface",
          "{interface}",
          "--side",
          "{endpoint_side}",
        ],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
      update_command: {
        argv: [
          "/opt/operator/routing-cost",
          "apply",
          "--plan-id",
          "{plan_id}",
          "--interface",
          "{interface}",
          "--side",
          "{endpoint_side}",
          "--cost",
          "{desired_cost}",
        ],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
    },
    description: "Routing-cost integration for the SFO endpoint.",
    id: "44444444-4444-4444-8444-444444444444",
    name: "SFO routing cost",
    updated_at: createdAt,
  },
  {
    adapter_kind: "routing_cost",
    created_at: createdAt,
    definition: {
      contract_version: 2,
      status_command: {
        argv: [
          "/opt/operator/routing-cost",
          "status",
          "--plan-id",
          "{plan_id}",
          "--interface",
          "{interface}",
          "--side",
          "{endpoint_side}",
        ],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
      update_command: {
        argv: [
          "/opt/operator/routing-cost",
          "apply",
          "--plan-id",
          "{plan_id}",
          "--interface",
          "{interface}",
          "--side",
          "{endpoint_side}",
          "--cost",
          "{desired_cost}",
        ],
        max_output_bytes: 16384,
        max_timeout_secs: 10,
      },
    },
    description: "Routing-cost integration for the FRA endpoint.",
    id: "55555555-5555-4555-8555-555555555555",
    name: "FRA routing cost",
    updated_at: createdAt,
  },
];
