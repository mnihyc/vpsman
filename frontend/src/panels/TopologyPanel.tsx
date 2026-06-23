import { useMemo, useState, type FormEvent } from "react";
import { Download, GitBranch, Power, PowerOff, RefreshCcw, Route, Save, Wand2 } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { VpsCombobox } from "../components/VpsCombobox";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  OSPF_COST_MODEL_DETAIL,
  OSPF_COST_MODEL_SUMMARY,
  addressFamilyLabel,
  buildRuntimeControl,
  buildRuntimeTopology,
  endpointSideLabel,
  latencyStatusLabel,
  mutationPolicyLabel,
  ospfStatusLabel,
  planCorrelationLabel,
  readableTelemetryToken,
  runtimeManagerLabel,
  telemetryReasonLabel,
  telemetrySourceLabel,
} from "../topologyRuntime";
import { buildPrivilegeAssertion, canonicalDbPrivilegeIntent, type PrivilegeMaterial } from "../privilege";
import { sha256Hex } from "../fileTransfer";
import { selectorExpressionForClientIds } from "../searchExpression";
import type {
  AgentView,
  BandwidthTier,
  CreateJobRequest,
  CreateJobResponse,
  CreateTunnelPlanRequest,
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
  JobHistoryRecord,
  JobOutputRecord,
  JobTargetRecord,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  OperatorPreferences,
  PromoteTelemetryTunnelRequest,
  RuntimeTunnelManager,
  RuntimeConfigApplyStateRecord,
  RuntimeConfigPatchRequest,
  RuntimeConfigPatchResponse,
  TelemetryTunnelRecord,
  TunnelAddressFamily,
  TunnelAddressPair,
  TopologyGraph,
  TunnelKind,
  TunnelPlan,
  TunnelPlanRecord,
  UpdateTunnelPlanOspfCostRequest,
} from "../types";
import type { PromoteTunnelPlanToCustomAdapterRequest } from "../typesTopology";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatTime,
  runPanelAction,
  shortId,
} from "../utils";
import { TopologyNetworkTestControls } from "./topology/TopologyNetworkTestControls";
import { TopologyEvidencePanel } from "./topology/TopologyEvidencePanel";
import { TopologyGraphPanel } from "./topology/TopologyGraphPanel";
import { TopologyOspfUpdateControls } from "./topology/TopologyOspfUpdateControls";
import { TopologyPromotionPanel } from "./topology/TopologyPromotionPanel";

const tunnelKinds: TunnelKind[] = ["gre", "ipip", "sit", "fou", "openvpn", "wireguard", "tun_tap", "custom"];
const bandwidthTiers: BandwidthTier[] = ["10m", "100m", "1000m"];
const runtimeManagers: RuntimeTunnelManager[] = [
  "agent_iproute2_managed",
  "external_observed",
  "external_managed_adapter",
];

type AutomationRow = ReturnType<typeof buildAutomationRows>[number];
type TunnelPlanSaveSnapshot = {
  draftKey: string;
  request: CreateTunnelPlanRequest;
};
type TunnelPlanToggleSnapshot = {
  enabled: boolean;
  planIds: string[];
  planNames: string[];
};

export function TopologyPanel({
  activeSubpage,
  agents,
  error,
  jobs,
  loading,
  networkObservations,
  networkTrends,
  ospfRecommendations,
  ospfUpdatePlans,
  runtimeConfigApplyStates,
  onCreateJob,
  onAllocateTunnelEndpoints,
  onCreateTunnelPlan,
  onExportTunnelPlan,
  onLoadNetworkObservations,
  onLoadNetworkTrends,
  onLoadOspfRecommendations,
  onLoadOspfUpdatePlans,
  onLoadTopologyGraph,
  onLoadOutputs,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onPromoteTelemetryTunnel,
  onPromoteTunnelPlanToCustomAdapter,
  onRefresh,
  onSubmitRuntimeConfigPatch,
  onSetTunnelPlanEnabled,
  onUpdateTunnelPlanOspfCost,
  privilegeMaterial,
  setPrivilegeMaterial,
  telemetryTunnels,
  topologyGraph,
  tunnelPlans,
}: {
  activeSubpage: string;
  agents: AgentView[];
  error: string | null;
  jobs: JobHistoryRecord[];
  loading: boolean;
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  topologyGraph: TopologyGraph;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onAllocateTunnelEndpoints: (request: AllocateTunnelEndpointsRequest) => Promise<AllocateTunnelEndpointsResponse>;
  onCreateTunnelPlan: (request: CreateTunnelPlanRequest) => Promise<void>;
  onExportTunnelPlan: (planId: string) => Promise<TunnelPlan>;
  onLoadNetworkObservations: () => Promise<void>;
  onLoadNetworkTrends: () => Promise<void>;
  onLoadOspfRecommendations: () => Promise<void>;
  onLoadOspfUpdatePlans: () => Promise<void>;
  onLoadTopologyGraph: () => Promise<void>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onPromoteTelemetryTunnel: (request: PromoteTelemetryTunnelRequest) => Promise<void>;
  onPromoteTunnelPlanToCustomAdapter: (request: PromoteTunnelPlanToCustomAdapterRequest) => Promise<void>;
  onRefresh: () => Promise<void>;
  onSubmitRuntimeConfigPatch: (request: RuntimeConfigPatchRequest) => Promise<RuntimeConfigPatchResponse>;
  onSetTunnelPlanEnabled: (planIds: string[], enabled: boolean) => Promise<void>;
  onUpdateTunnelPlanOspfCost: (planId: string, request: UpdateTunnelPlanOspfCostRequest) => Promise<void>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const [form, setForm] = useState<CreateTunnelPlanRequest>(() => initialTunnelPlanForm(preferences));
  const [reservedText, setReservedText] = useState("");
  const [runtimeStartupArgv, setRuntimeStartupArgv] = useState("");
  const [runtimeStopArgv, setRuntimeStopArgv] = useState("");
  const [runtimeCleanupArgv, setRuntimeCleanupArgv] = useState("");
  const [runtimeRestartArgv, setRuntimeRestartArgv] = useState("");
  const [runtimeStatusArgv, setRuntimeStatusArgv] = useState("");
  const [runtimeTrafficArgv, setRuntimeTrafficArgv] = useState("");
  const [trafficIngressKbps, setTrafficIngressKbps] = useState("");
  const [trafficEgressKbps, setTrafficEgressKbps] = useState("");
  const [trafficBurstKb, setTrafficBurstKb] = useState("");
  const [trafficLimitEnabled, setTrafficLimitEnabled] = useState(false);
  const [fouPort, setFouPort] = useState("5555");
  const [fouPeerPort, setFouPeerPort] = useState("5555");
  const [fouIpproto, setFouIpproto] = useState("4");
  const [topologyDesiredText, setTopologyDesiredText] = useState("");
  const [topologyStaleText, setTopologyStaleText] = useState("");
  const [topologyRoutesText, setTopologyRoutesText] = useState("");
  const [topologyStaleRoutesText, setTopologyStaleRoutesText] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [automationBulkPending, setAutomationBulkPending] = useState(false);
  const [automationBulkStatus, setAutomationBulkStatus] = useState<string | null>(null);
  const [tunnelPlanTogglePending, setTunnelPlanTogglePending] = useState(false);
  const [tunnelPlanSaveSnapshot, setTunnelPlanSaveSnapshot] = useState<TunnelPlanSaveSnapshot | null>(null);
  const [tunnelPlanToggleSnapshot, setTunnelPlanToggleSnapshot] = useState<TunnelPlanToggleSnapshot | null>(null);
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
  const automationRows = useMemo(
    () => buildAutomationRows(agents, telemetryTunnels, tunnelPlans, clientLabel),
    [agents, telemetryTunnels, tunnelPlans, agentNameById],
  );
  const tunnelPlanColumns = useMemo<ConsoleDataGridColumn<TunnelPlanRecord>[]>(
    () => [
      {
        id: "name",
        header: "Tunnel",
        size: 240,
        minSize: 180,
        sortValue: (plan) => plan.name,
        searchValue: (plan) => `${plan.name} ${plan.id} ${plan.plan.interface_name}`,
        cell: (plan) => (
          <span className="historyPrimary">
            <strong>{plan.name}</strong>
            <small>{shortId(plan.id)} · {plan.plan.interface_name}</small>
          </span>
        ),
      },
      {
        id: "endpoints",
        header: "Endpoints",
        size: 310,
        minSize: 230,
        sortValue: (plan) => `${clientLabel(plan.left_client_id)} ${clientLabel(plan.right_client_id)}`,
        searchValue: (plan) =>
          [
            clientLabel(plan.left_client_id),
            clientLabel(plan.right_client_id),
            plan.left_client_id,
            plan.right_client_id,
            plan.plan.left_underlay,
            plan.plan.right_underlay,
          ].join(" "),
        cell: (plan) => (
          <span className="historyPrimary">
            <strong>{clientLabel(plan.left_client_id)} / {clientLabel(plan.right_client_id)}</strong>
            <small>L {readableTelemetryToken(plan.left_status)} / R {readableTelemetryToken(plan.right_status)}</small>
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        size: 170,
        minSize: 105,
        sortValue: (plan) => `${plan.enabled ? "enabled" : "disabled"} ${plan.status}`,
        searchValue: (plan) => `${plan.enabled ? "enabled" : "disabled"} ${plan.status} ${plan.left_status} ${plan.right_status}`,
        cell: (plan) => (
          <span className="historyPrimary">
            <strong className={`status ${plan.enabled ? "ok" : "warn"}`}>{plan.enabled ? "enabled" : "disabled"}</strong>
            <small>{readableTelemetryToken(plan.status)}</small>
          </span>
        ),
      },
      {
        id: "ospf",
        header: "OSPF",
        size: 120,
        minSize: 95,
        sortValue: (plan) => plan.recommended_ospf_cost,
        searchValue: (plan) => plan.recommended_ospf_cost,
        cell: (plan) => (
          <span className="historyPrimary">
            <strong>{plan.recommended_ospf_cost}</strong>
            <small>{plan.plan.bandwidth}</small>
          </span>
        ),
      },
      {
        id: "runtime",
        header: "Runtime",
        size: 190,
        minSize: 150,
        sortValue: (plan) => runtimeManagerLabel(plan.plan.runtime_control?.manager),
        searchValue: (plan) => `${plan.kind} ${runtimeManagerLabel(plan.plan.runtime_control?.manager)}`,
        cell: (plan) => (
          <span className="historyPrimary">
            <strong>{plan.kind.toUpperCase()}</strong>
            <small>{runtimeManagerLabel(plan.plan.runtime_control?.manager)}</small>
          </span>
        ),
      },
      {
        id: "updated",
        header: "Updated",
        size: 170,
        minSize: 135,
        sortValue: (plan) => plan.updated_at ?? plan.created_at,
        searchValue: (plan) => `${plan.updated_at} ${plan.created_at}`,
        cell: (plan) => (
          <span className="historyPrimary">
            <strong>{formatTime(plan.updated_at ?? plan.created_at)}</strong>
            <small>created {formatTime(plan.created_at)}</small>
          </span>
        ),
      },
    ],
    [agentNameById],
  );
  const tunnelPlanActions: ConsoleDataGridAction<TunnelPlanRecord>[] = [
    {
      disabled: (rows) => tunnelPlanTogglePending || rows.every((plan) => plan.enabled),
      icon: <Power size={15} />,
      label: "Enable plan",
      onSelect: (rows) => void setTunnelPlanEnabledForRows(rows, true),
    },
    {
      disabled: (rows) => tunnelPlanTogglePending || rows.every((plan) => !plan.enabled),
      icon: <PowerOff size={15} />,
      label: "Disable plan",
      onSelect: (rows) => void setTunnelPlanEnabledForRows(rows, false),
    },
    {
      disabled: (rows) => pending || rows.length !== 1,
      icon: <Download size={15} />,
      label: "Export JSON",
      onSelect: (rows) => void exportTunnelPlanJson(rows[0]),
      separatorBefore: true,
    },
  ];
  const automationColumns = useMemo<ConsoleDataGridColumn<AutomationRow>[]>(
    () => [
      {
        id: "agent",
        header: "VPS",
        size: 250,
        minSize: 190,
        sortValue: (row) => row.label,
        searchValue: (row) => `${row.label} ${row.clientId}`,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.label}</strong>
            <small>{row.monitored} monitored · {row.endpointCount} saved endpoints</small>
          </span>
        ),
      },
      {
        id: "latency",
        header: "Latency",
        size: 250,
        minSize: 190,
        sortValue: (row) => row.urgency,
        searchValue: (row) => `${row.latency} ${row.latencyDetail} ${row.latencyTitle}`,
        cell: (row) => (
          <TelemetryCell
            detail={row.latencyDetail}
            main={row.latency}
            title={row.latencyTitle}
            tone={row.latencyTone}
          />
        ),
      },
      {
        id: "auto_ospf",
        header: "Auto OSPF",
        size: 250,
        minSize: 190,
        sortValue: (row) => row.autoOspf,
        searchValue: (row) => `${row.autoOspf} ${row.autoOspfDetail} ${row.autoOspfTitle}`,
        cell: (row) => (
          <TelemetryCell
            detail={row.autoOspfDetail}
            main={row.autoOspf}
            title={row.autoOspfTitle}
            tone={row.autoOspfTone}
          />
        ),
      },
      {
        id: "cost_report",
        header: "Cost / Report",
        size: 230,
        minSize: 180,
        sortValue: (row) => row.lastReport,
        searchValue: (row) => `${row.cost} ${row.costDetail} ${row.lastReport} ${row.reportDetail}`,
        cell: (row) => (
          <TelemetryCell
            detail={`${row.costDetail}; ${row.reportDetail}`}
            main={`${row.cost} / ${row.lastReport}`}
            title={`${row.costDetail}; ${row.reportDetail}`}
          />
        ),
      },
    ],
    [],
  );
  const automationActions: ConsoleDataGridAction<AutomationRow>[] = [
    {
      disabled: (rows) => automationBulkPending || rows.every((row) => row.endpointCount === 0),
      icon: <Power size={15} />,
      label: "Enable monitoring",
      onSelect: (rows) => void applyAutomationBulk(rows, true),
    },
    {
      disabled: (rows) => automationBulkPending || rows.every((row) => row.endpointCount === 0),
      icon: <PowerOff size={15} />,
      label: "Disable monitoring",
      onSelect: (rows) => void applyAutomationBulk(rows, false),
    },
  ];
  const runtimeManager = form.runtime_control?.manager ?? "agent_iproute2_managed";
  const topologySubpage = ["graph", "plans", "apply", "promotion", "evidence", "ospf"].includes(activeSubpage)
    ? activeSubpage
    : "graph";
  const ready =
    form.name.trim() &&
    form.interface_name.trim() &&
    form.left_client_id &&
    form.right_client_id &&
    form.left_client_id !== form.right_client_id &&
    form.left_underlay.trim() &&
    form.right_underlay.trim() &&
    hasAddressSource(form);
  const status = actionError ?? error ?? (loading ? "Loading" : `${tunnelPlans.length} plans`);
  const tunnelPlanDraftKey = useMemo(
    () =>
      JSON.stringify({
        form,
        reservedText,
        runtimeStartupArgv,
        runtimeStopArgv,
        runtimeCleanupArgv,
        runtimeRestartArgv,
        runtimeStatusArgv,
        runtimeTrafficArgv,
        trafficIngressKbps,
        trafficEgressKbps,
        trafficBurstKb,
        trafficLimitEnabled,
        fouPort,
        fouPeerPort,
        fouIpproto,
        topologyDesiredText,
        topologyStaleText,
        topologyRoutesText,
        topologyStaleRoutesText,
      }),
    [
      form,
      reservedText,
      runtimeStartupArgv,
      runtimeStopArgv,
      runtimeCleanupArgv,
      runtimeRestartArgv,
      runtimeStatusArgv,
      runtimeTrafficArgv,
      trafficIngressKbps,
      trafficEgressKbps,
      trafficBurstKb,
      trafficLimitEnabled,
      fouPort,
      fouPeerPort,
      fouIpproto,
      topologyDesiredText,
      topologyStaleText,
      topologyRoutesText,
      topologyStaleRoutesText,
    ],
  );

  async function submitPlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setActionError(null);
    if (!ready) {
      setActionError("Tunnel plan is incomplete");
      return;
    }
    setTunnelPlanSaveSnapshot({
      draftKey: tunnelPlanDraftKey,
      request: buildTunnelPlanSaveRequest(),
    });
  }

  async function executeTunnelPlanSave(snapshot: TunnelPlanSaveSnapshot) {
    await runPanelAction(setPending, setActionError, async () => {
      await onCreateTunnelPlan(snapshot.request);
      setTunnelPlanSaveSnapshot(null);
    });
  }

  function buildTunnelPlanSaveRequest(): CreateTunnelPlanRequest {
    return {
      ...form,
      reserved_addresses: splitReserved(reservedText),
      ipv4_tunnel: completePairOrNull(form.ipv4_tunnel ?? null),
      ipv6_address_pool_cidr: form.ipv6_address_pool_cidr?.trim() || null,
      ipv6_tunnel: completePairOrNull(form.ipv6_tunnel ?? null),
      runtime_control: buildRuntimeControl(runtimeManager, {
        startup: runtimeStartupArgv,
        stop: runtimeStopArgv,
        cleanup: runtimeCleanupArgv,
        restart: runtimeRestartArgv,
        status: runtimeStatusArgv,
        traffic: trafficLimitEnabled ? runtimeTrafficArgv : "",
        ingressKbps: trafficLimitEnabled ? trafficIngressKbps : "",
        egressKbps: trafficLimitEnabled ? trafficEgressKbps : "",
        burstKb: trafficLimitEnabled ? trafficBurstKb : "",
        fouPort: form.kind === "fou" ? fouPort : "",
        fouPeerPort: form.kind === "fou" ? fouPeerPort : "",
        fouIpproto: form.kind === "fou" ? fouIpproto : "",
      }),
      runtime_topology: buildRuntimeTopology({
        desiredText: topologyDesiredText,
        staleText: topologyStaleText,
        routesText: topologyRoutesText,
        staleRoutesText: topologyStaleRoutesText,
      }),
      confirmed: true,
    };
  }

  async function allocateEndpoints() {
    await runPanelAction(setPending, setActionError, async () => {
      const ipv4Pool = form.address_pool_cidr.trim();
      const ipv6Pool = (form.ipv6_address_pool_cidr ?? "").trim();
      const hasLocalPool = Boolean(ipv4Pool || ipv6Pool);
      const reservedAddresses = mergeReservedAddresses(
        splitReserved(reservedText),
        currentTunnelAddresses(form),
      );
      const allocation = await onAllocateTunnelEndpoints({
        ipv4_pool_cidr: ipv4Pool || null,
        ipv6_pool_cidr: ipv6Pool || null,
        reserved_addresses: reservedAddresses,
        include_ipv4: hasLocalPool ? Boolean(ipv4Pool) : undefined,
        include_ipv6: hasLocalPool ? Boolean(ipv6Pool) : undefined,
      });
      if (!allocation.ipv4_tunnel && !allocation.ipv6_tunnel) {
        throw new Error("No tunnel allocation pool is configured; enter endpoint CIDRs or configure an allocator pool");
      }
      setReservedText(formatReservedAddresses(reservedAddresses));
      setForm((current) => ({
        ...current,
        ipv4_tunnel: allocation.ipv4_tunnel,
        ipv6_tunnel: allocation.ipv6_tunnel,
        latency_primary_family: allocation.latency_primary_family,
      }));
    });
  }

  async function exportTunnelPlanJson(plan: TunnelPlanRecord | undefined) {
    if (!plan) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      const exported = await onExportTunnelPlan(plan.id);
      saveBlob(
        new Blob([`${JSON.stringify(exported, null, 2)}\n`], {
          type: "application/json",
        }),
        `${safeFileName(plan.name || plan.id)}.plan.json`,
      );
    });
  }

  return (
    <div className="workspaceGrid">
      {(topologySubpage === "graph" || topologySubpage === "plans") && (
      <section className="fleetPanel">
        <ConsoleDataGrid
          actions={tunnelPlanActions}
          columns={tunnelPlanColumns}
          defaultPageSize={20}
          empty={<div className="emptyState compactEmpty">{status === "Loading" ? "Loading tunnel plans" : "No tunnel plans"}</div>}
          getRowId={(plan) => plan.id}
          itemLabel="plans"
          renderExpandedRow={(plan) => (
            <TunnelPlanGridDetail plan={plan} clientLabel={clientLabel} />
          )}
          rows={tunnelPlans}
          searchPlaceholder="Search tunnel plans"
          storageKey="vpsman.grid.topology.tunnelPlans.v2"
          title="Tunnel plans"
          toolbarActions={
            <button className="secondaryAction compactAction" disabled={loading || pending} onClick={onRefresh} type="button">
              <RefreshCcw size={16} />
              <span>Refresh</span>
            </button>
          }
        />
        <OspfCostModelNote />
      </section>
      )}
      <ConfirmationPrompt
        confirmLabel="Save plan"
        detail="Save the reviewed tunnel plan as canonical topology state."
        error={actionError}
        items={[
          { label: "Name", value: tunnelPlanSaveSnapshot?.request.name ?? "-" },
          {
            label: "Status",
            value: tunnelPlanSaveSnapshot?.request.enabled ? "Enabled" : "Disabled",
          },
          { label: "Kind", value: tunnelPlanSaveSnapshot?.request.kind ?? "-" },
          {
            label: "Endpoints",
            value: tunnelPlanSaveSnapshot
              ? `${clientLabel(tunnelPlanSaveSnapshot.request.left_client_id)} / ${clientLabel(tunnelPlanSaveSnapshot.request.right_client_id)}`
              : "-",
          },
          {
            label: "Runtime",
            value: tunnelPlanSaveSnapshot
              ? runtimeManagerLabel(tunnelPlanSaveSnapshot.request.runtime_control?.manager)
              : "-",
          },
          {
            label: "Sync",
            value: tunnelPlanSaveSnapshot?.request.enabled ? "Now" : "Deferred",
          },
        ]}
        onCancel={() => setTunnelPlanSaveSnapshot(null)}
        onConfirm={() => {
          if (tunnelPlanSaveSnapshot) {
            void executeTunnelPlanSave(tunnelPlanSaveSnapshot);
          }
        }}
        open={Boolean(tunnelPlanSaveSnapshot && tunnelPlanSaveSnapshot.draftKey === tunnelPlanDraftKey)}
        pending={pending}
        title="Confirm tunnel plan save"
      />
      <ConfirmationPrompt
        confirmLabel={tunnelPlanToggleSnapshot?.enabled ? "Enable plans" : "Disable plans"}
        detail="Confirm the reviewed lifecycle change and push runtime config to the affected agents."
        error={actionError}
        items={[
          { label: "Action", value: tunnelPlanToggleSnapshot?.enabled ? "Enable" : "Disable" },
          { label: "Plans", value: tunnelPlanToggleSnapshot?.planIds.length ?? 0 },
          {
            label: "Names",
            value: tunnelPlanToggleSnapshot
              ? tunnelPlanToggleSnapshot.planNames.slice(0, 4).join(", ") +
                (tunnelPlanToggleSnapshot.planNames.length > 4 ? " ..." : "")
              : "-",
          },
        ]}
        onCancel={() => setTunnelPlanToggleSnapshot(null)}
        onConfirm={() => {
          if (tunnelPlanToggleSnapshot) {
            void executeTunnelPlanToggle(tunnelPlanToggleSnapshot);
          }
        }}
        open={Boolean(tunnelPlanToggleSnapshot)}
        pending={tunnelPlanTogglePending}
        title="Confirm tunnel plan lifecycle"
      />

      {(topologySubpage === "graph" || topologySubpage === "plans" || topologySubpage === "ospf") && (
        <section className="fleetPanel">
          <ConsoleDataGrid
            actions={automationActions}
            columns={automationColumns}
            defaultPageSize={20}
            empty={<div className="emptyState compactEmpty">No agents</div>}
            getRowId={(row) => row.clientId}
            itemLabel="VPSs"
            renderExpandedRow={(row) => <AutomationGridDetail row={row} />}
            rows={automationRows}
            searchPlaceholder="Search automation state"
            storageKey="vpsman.grid.topology.automation.v2"
            title="Latency and auto OSPF"
            toolbarActions={
              automationBulkStatus ? <span className="mutedCell">{automationBulkStatus}</span> : <Route size={18} />
            }
          />
        </section>
      )}

      {topologySubpage === "graph" && (
      <TopologyGraphPanel
        graph={topologyGraph}
        loading={loading}
        onRefresh={onLoadTopologyGraph}
        runtimeConfigApplyStates={runtimeConfigApplyStates}
      />
      )}

      {topologySubpage === "plans" && (
      <section className="fleetPanel scheduleComposer topologyPlanComposer">
        <div className="sectionHeader">
          <div>
            <h2>Create tunnel plan</h2>
          </div>
          <GitBranch size={20} />
        </div>
        <form className="dispatchForm topologyPlanForm" onSubmit={submitPlan}>
          <div
            className="operationNote formSectionNote"
            title="Name the intended tunnel and choose the link type before selecting endpoints."
          >
            <strong>Plan identity</strong>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Name</span>
              <input value={form.name} onChange={(event) => setField("name", event.target.value)} />
            </label>
            <label>
              <span>Interface</span>
              <input
                value={form.interface_name}
                onChange={(event) => setField("interface_name", event.target.value)}
              />
            </label>
            <label>
              <span>Kind</span>
              <select value={form.kind} onChange={(event) => setField("kind", event.target.value as TunnelKind)}>
                {tunnelKinds.map((kind) => (
                  <option key={kind} value={kind}>
                    {kind.toUpperCase()}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Bandwidth</span>
              <select
                value={form.bandwidth}
                onChange={(event) => setField("bandwidth", event.target.value as BandwidthTier)}
              >
                {bandwidthTiers.map((tier) => (
                  <option key={tier} value={tier}>
                    {bandwidthTierLabel(tier)}
                  </option>
                ))}
              </select>
            </label>
            <div className="topologyLifecycleField">
              <span>Status</span>
              <label className="checkLine inlineCheck topologyLifecycleToggle">
                <input
                  aria-label="Plan enabled"
                  checked={form.enabled}
                  onChange={(event) => setField("enabled", event.target.checked)}
                  type="checkbox"
                />
                <span>{form.enabled ? "Enabled" : "Disabled"}</span>
              </label>
            </div>
          </div>
          <div
            className="operationNote formSectionNote"
            title="Pair exactly two VPSs, provide underlays, then enter or generate explicit tunnel endpoints."
          >
            <strong>Endpoints</strong>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Left VPS"
                excludeIds={form.right_client_id ? [form.right_client_id] : []}
                onChange={(value) => setEndpointClient("left", value)}
                placeholder="Search left VPS"
                value={form.left_client_id}
              />
            </label>
            <label>
              <span>Right VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Right VPS"
                excludeIds={form.left_client_id ? [form.left_client_id] : []}
                onChange={(value) => setEndpointClient("right", value)}
                placeholder="Search right VPS"
                value={form.right_client_id}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left underlay</span>
              <input value={form.left_underlay} onChange={(event) => setField("left_underlay", event.target.value)} />
            </label>
            <label>
              <span>Right underlay</span>
              <input value={form.right_underlay} onChange={(event) => setField("right_underlay", event.target.value)} />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left IPv4 CIDR</span>
              <input
                value={formatEndpointCidr(form.ipv4_tunnel ?? null, "left")}
                onChange={(event) => setAddressCidr("ipv4_tunnel", "left", event.target.value, 31)}
                placeholder="IPv4 CIDR"
              />
            </label>
            <label>
              <span>Right IPv4 CIDR</span>
              <input
                value={formatEndpointCidr(form.ipv4_tunnel ?? null, "right")}
                onChange={(event) => setAddressCidr("ipv4_tunnel", "right", event.target.value, 31)}
                placeholder="IPv4 CIDR"
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left IPv6 CIDR</span>
              <input
                value={formatEndpointCidr(form.ipv6_tunnel ?? null, "left")}
                onChange={(event) => setAddressCidr("ipv6_tunnel", "left", event.target.value, 127)}
                placeholder="IPv6 CIDR"
              />
            </label>
            <label>
              <span>Right IPv6 CIDR</span>
              <input
                value={formatEndpointCidr(form.ipv6_tunnel ?? null, "right")}
                onChange={(event) => setAddressCidr("ipv6_tunnel", "right", event.target.value, 127)}
                placeholder="IPv6 CIDR"
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Latency primary</span>
              <select
                value={form.latency_primary_family ?? "ipv4"}
                onChange={(event) => setField("latency_primary_family", event.target.value as TunnelAddressFamily)}
              >
                <option value="ipv4">IPv4</option>
                <option value="ipv6">IPv6</option>
              </select>
            </label>
            <button className="secondaryAction" disabled={pending} onClick={allocateEndpoints} type="button">
              <Wand2 size={17} />
              Allocate endpoints
            </button>
          </div>
          <details
            className="operationNote formSectionNote"
            title="Uses Preferences pools unless overridden here. Reserved addresses are comma-separated; repeated allocation appends current endpoint IPs before requesting another suggestion."
          >
            <summary>Allocation overrides</summary>
            <div className="dispatchControls">
              <label>
                <span>IPv4 pool override</span>
                <input
                  value={form.address_pool_cidr}
                  onChange={(event) => setField("address_pool_cidr", event.target.value)}
                  placeholder="No default"
                />
              </label>
              <label>
                <span>IPv6 pool override</span>
                <input
                  value={form.ipv6_address_pool_cidr ?? ""}
                  onChange={(event) => setField("ipv6_address_pool_cidr", event.target.value)}
                  placeholder="No default"
                />
              </label>
              <label>
                <span>Reserved addresses</span>
                <input value={reservedText} onChange={(event) => setReservedText(event.target.value)} />
              </label>
            </div>
          </details>
          <div className="dispatchControls">
            <label>
              <span>Latency ms</span>
              <input
                min={0}
                onChange={(event) => setField("latency_ms", Number(event.target.value))}
                type="number"
                value={form.latency_ms}
              />
            </label>
            <label>
              <span>Preference</span>
              <input
                min={0.1}
                step={0.1}
                onChange={(event) => setField("preference", Number(event.target.value))}
                type="number"
                value={form.preference}
              />
            </label>
          </div>
          <div
            className="operationNote formSectionNote"
            title="Choose whether the agent owns the tunnel, observes it externally, or delegates lifecycle commands to a custom adapter."
          >
            <strong>Runtime</strong>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Runtime owner</span>
              <select
                value={runtimeManager}
                onChange={(event) =>
                  setField("runtime_control", {
                    ...(form.runtime_control ?? { traffic_limit: {} }),
                    manager: event.target.value as RuntimeTunnelManager,
                  })
                }
              >
                {runtimeManagers.map((manager) => (
                  <option key={manager} value={manager}>
                    {runtimeManagerLabel(manager)}
                  </option>
                ))}
              </select>
            </label>
            <label className="checkLine inlineCheck topologyLifecycleToggle">
              <input
                checked={trafficLimitEnabled}
                onChange={(event) => setTrafficLimitEnabled(event.target.checked)}
                type="checkbox"
              />
              <span>Traffic shaping</span>
            </label>
            <label>
              <span>Egress Kbps</span>
              <input
                disabled={!trafficLimitEnabled}
                min={64}
                onChange={(event) => setTrafficEgressKbps(event.target.value)}
                placeholder="Empty means disabled"
                type="number"
                value={trafficEgressKbps}
              />
            </label>
            <label>
              <span>Ingress Kbps</span>
              <input
                disabled={!trafficLimitEnabled}
                min={64}
                onChange={(event) => setTrafficIngressKbps(event.target.value)}
                placeholder="Empty means disabled"
                type="number"
                value={trafficIngressKbps}
              />
            </label>
            <label>
              <span>Burst KB</span>
              <input
                disabled={!trafficLimitEnabled}
                min={1}
                onChange={(event) => setTrafficBurstKb(event.target.value)}
                placeholder="Empty means disabled"
                type="number"
                value={trafficBurstKb}
              />
            </label>
          </div>
          {form.kind === "fou" && (
            <div className="dispatchControls">
              <label>
                <span>FOU port</span>
                <input min={1} max={65535} onChange={(event) => setFouPort(event.target.value)} type="number" value={fouPort} />
              </label>
              <label>
                <span>FOU peer port</span>
                <input
                  min={1}
                  max={65535}
                  onChange={(event) => setFouPeerPort(event.target.value)}
                  type="number"
                  value={fouPeerPort}
                />
              </label>
              <label>
                <span>FOU IP proto</span>
                <input min={1} max={255} onChange={(event) => setFouIpproto(event.target.value)} type="number" value={fouIpproto} />
              </label>
            </div>
          )}
          {runtimeManager === "external_managed_adapter" && (
            <>
              <div className="dispatchControls">
                <label>
                  <span>Start argv</span>
                  <textarea value={runtimeStartupArgv} onChange={(event) => setRuntimeStartupArgv(event.target.value)} />
                </label>
                <label>
                  <span>Restart argv</span>
                  <textarea value={runtimeRestartArgv} onChange={(event) => setRuntimeRestartArgv(event.target.value)} />
                </label>
              </div>
              <div className="dispatchControls">
                <label>
                  <span>Status argv</span>
                  <textarea value={runtimeStatusArgv} onChange={(event) => setRuntimeStatusArgv(event.target.value)} />
                </label>
                <label>
                  <span>Stop argv</span>
                  <textarea value={runtimeStopArgv} onChange={(event) => setRuntimeStopArgv(event.target.value)} />
                </label>
              </div>
              <label>
                <span>Cleanup argv</span>
                <textarea value={runtimeCleanupArgv} onChange={(event) => setRuntimeCleanupArgv(event.target.value)} />
              </label>
              <label>
                <span>Traffic argv</span>
                <textarea value={runtimeTrafficArgv} onChange={(event) => setRuntimeTrafficArgv(event.target.value)} />
              </label>
            </>
          )}
          <details
            className="operationNote formSectionNote topologyEvidenceDisclosure"
            title="Optional desired/stale interface and route evidence helps later promotion and drift review."
          >
            <summary>Topology evidence</summary>
            <div className="dispatchControls">
              <label>
                <span>Desired interfaces</span>
                <input value={topologyDesiredText} onChange={(event) => setTopologyDesiredText(event.target.value)} />
              </label>
              <label>
                <span>Stale interfaces</span>
                <input value={topologyStaleText} onChange={(event) => setTopologyStaleText(event.target.value)} />
              </label>
              <label>
                <span>Routes</span>
                <textarea value={topologyRoutesText} onChange={(event) => setTopologyRoutesText(event.target.value)} />
              </label>
              <label>
                <span>Stale routes</span>
                <textarea value={topologyStaleRoutesText} onChange={(event) => setTopologyStaleRoutesText(event.target.value)} />
              </label>
            </div>
          </details>
          <button className="primaryAction" disabled={pending || !ready} type="submit">
            <Save size={17} />
            Save plan
          </button>
        </form>
      </section>
      )}

      {topologySubpage === "promotion" && (
      <TopologyPromotionPanel
        agents={agents}
        onAllocateTunnelEndpoints={onAllocateTunnelEndpoints}
        onPromoteTelemetryTunnel={onPromoteTelemetryTunnel}
        onPromoteTunnelPlanToCustomAdapter={onPromoteTunnelPlanToCustomAdapter}
        telemetryTunnels={telemetryTunnels}
        tunnelPlans={tunnelPlans}
      />
      )}

      {topologySubpage === "apply" && tunnelPlans.length > 0 && (
        <TopologyNetworkTestControls
          agents={agents}
          onCreateJob={onCreateJob}
          onLoadTargets={onLoadTargets}
          onOpenJobDetails={onOpenJobDetails}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
          tunnelPlans={tunnelPlans}
        />
      )}
      {topologySubpage === "ospf" && tunnelPlans.length > 0 && (
        <>
          <section className="fleetPanel">
            <OspfCostModelNote />
          </section>
          <TopologyOspfUpdateControls
            agents={agents}
            ospfUpdatePlans={ospfUpdatePlans}
            tunnelPlans={tunnelPlans}
            onUpdateTunnelPlanOspfCost={onUpdateTunnelPlanOspfCost}
          />
        </>
      )}

      {topologySubpage === "evidence" && (
      <TopologyEvidencePanel
        clientLabel={clientLabel}
        jobs={jobs}
        observations={networkObservations}
        onLoadTrends={onLoadNetworkTrends}
        onLoadObservations={onLoadNetworkObservations}
        onLoadOspfRecommendations={onLoadOspfRecommendations}
        onLoadOspfUpdatePlans={onLoadOspfUpdatePlans}
        onLoadOutputs={onLoadOutputs}
        ospfRecommendations={ospfRecommendations}
        ospfUpdatePlans={ospfUpdatePlans}
        trends={networkTrends}
      />
      )}

      {topologySubpage === "plans" && tunnelPlans[0] && (
        <section className="fleetPanel topologyPreview">
          <div className="sectionHeader">
            <div>
              <h2>Latest plan</h2>
              <span>{runtimeManagerLabel(tunnelPlans[0].plan.runtime_control?.manager)}</span>
            </div>
            <Route size={20} />
          </div>
          <pre>{tunnelPlans[0].plan.ifupdown_snippet}</pre>
          <pre>{tunnelPlans[0].plan.bird2_interface_snippet}</pre>
        </section>
      )}
    </div>
  );

  function setField<K extends keyof CreateTunnelPlanRequest>(key: K, value: CreateTunnelPlanRequest[K]) {
    setForm((current) => ({ ...current, [key]: value }));
  }

  function setEndpointClient(side: "left" | "right", clientId: string) {
    setForm((current) => {
      const clientKey = side === "left" ? "left_client_id" : "right_client_id";
      const underlayKey = side === "left" ? "left_underlay" : "right_underlay";
      return {
        ...current,
        [clientKey]: clientId,
        [underlayKey]: autoUnderlayValue(
          current[underlayKey],
          current[clientKey],
          clientId,
          agents,
        ),
      };
    });
  }

  function setAddressCidr(
    key: "ipv4_tunnel" | "ipv6_tunnel",
    side: "left" | "right",
    value: string,
    fallbackPrefix: number,
  ) {
    setForm((current) => {
      const currentPair = current[key] ?? { left: "", right: "", prefix_len: fallbackPrefix };
      const parsed = parseEndpointCidr(value, currentPair.prefix_len);
      const nextPair = {
        ...currentPair,
        [side]: parsed.address,
        prefix_len: parsed.prefix_len,
      };
      return { ...current, [key]: normalizePair(nextPair) };
    });
  }

  function setTunnelPlanEnabledForRows(rows: TunnelPlanRecord[], enabled: boolean) {
    const targets = rows.filter((plan) => plan.enabled !== enabled);
    if (targets.length === 0) {
      return;
    }
    setActionError(null);
    setTunnelPlanToggleSnapshot({
      enabled,
      planIds: targets.map((plan) => plan.id),
      planNames: targets.map((plan) => plan.name),
    });
  }

  async function executeTunnelPlanToggle(snapshot: TunnelPlanToggleSnapshot) {
    await runPanelAction(setTunnelPlanTogglePending, setActionError, async () => {
      await onSetTunnelPlanEnabled(snapshot.planIds, snapshot.enabled);
      setTunnelPlanToggleSnapshot(null);
    });
  }

  async function applyAutomationBulk(rows: AutomationRow[], enabled: boolean) {
    if (!privilegeMaterial) {
      setAutomationBulkStatus("Privilege unlock required");
      onOpenPrivilegeUnlock();
      return;
    }
    const targets = rows.filter((row) => row.endpointCount > 0);
    if (targets.length === 0) {
      setAutomationBulkStatus("No endpoint targets selected");
      return;
    }
    await runPanelAction(setAutomationBulkPending, setAutomationBulkStatus, async () => {
      const targetClientIds = targets.map((target) => target.clientId);
      const selectorExpression = selectorExpressionForClientIds(targetClientIds);
      const toml = buildMonitoringConfigPatchToml(enabled);
      const payloadHash = await sha256Hex(new TextEncoder().encode(toml));
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "runtime_config.patch",
          target: "runtime_config",
          selectorExpression,
          resolvedTargets: targetClientIds,
          confirmed: true,
          payloadHash,
        }),
        privilegeMaterial,
      });
      const response = await onSubmitRuntimeConfigPatch({
        confirmed: true,
        reason: enabled ? "Enable tunnel monitoring defaults" : "Disable tunnel monitoring defaults",
        selector_expression: selectorExpression,
        target_client_ids: targetClientIds,
        toml,
        privilege_assertion: privilegeAssertion,
      });
      setAutomationBulkStatus(
        `${enabled ? "Enabled" : "Disabled"} monitoring defaults on ${response.target_count} VPSs; ${response.sync_job_ids.length} sync jobs`,
      );
    });
  }
}

function splitReserved(value: string): string[] {
  return value
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
}

function formatReservedAddresses(addresses: string[]): string {
  return addresses.join(", ");
}

function mergeReservedAddresses(existing: string[], additions: string[]): string[] {
  const seen = new Set<string>();
  const merged: string[] = [];
  for (const address of [...existing, ...additions]) {
    const trimmed = address.trim();
    if (!trimmed || seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    merged.push(trimmed);
  }
  return merged;
}

function currentTunnelAddresses(form: CreateTunnelPlanRequest): string[] {
  return [
    ...addressPairValues(form.ipv4_tunnel ?? null),
    ...addressPairValues(form.ipv6_tunnel ?? null),
  ];
}

function addressPairValues(pair: TunnelAddressPair | null): string[] {
  if (!pair) {
    return [];
  }
  return [pair.left, pair.right].filter(Boolean);
}

function formatEndpointCidr(pair: TunnelAddressPair | null, side: "left" | "right"): string {
  const address = side === "left" ? pair?.left : pair?.right;
  if (!pair || !address) {
    return "";
  }
  return `${address}/${pair.prefix_len}`;
}

function parseEndpointCidr(value: string, fallbackPrefix: number): { address: string; prefix_len: number } {
  const trimmed = value.trim();
  if (!trimmed) {
    return { address: "", prefix_len: fallbackPrefix };
  }
  const slashIndex = trimmed.lastIndexOf("/");
  if (slashIndex < 0) {
    return { address: trimmed, prefix_len: fallbackPrefix };
  }
  const address = trimmed.slice(0, slashIndex).trim();
  const rawPrefix = Number(trimmed.slice(slashIndex + 1).trim());
  const prefix_len = Number.isFinite(rawPrefix) ? Math.trunc(rawPrefix) : fallbackPrefix;
  return { address, prefix_len };
}

function defaultUnderlayForAgent(agents: AgentView[], clientId: string): string {
  const agent = agents.find((candidate) => candidate.id === clientId);
  return agent?.last_ip?.trim() || agent?.registration_ip?.trim() || "";
}

function autoUnderlayValue(
  currentValue: string,
  currentClientId: string,
  nextClientId: string,
  agents: AgentView[],
): string {
  const nextAuto = defaultUnderlayForAgent(agents, nextClientId);
  if (!nextAuto) {
    return currentValue;
  }
  const currentAuto = defaultUnderlayForAgent(agents, currentClientId);
  const currentTrimmed = currentValue.trim();
  if (!currentTrimmed || currentTrimmed === currentAuto) {
    return nextAuto;
  }
  return currentValue;
}

function initialTunnelPlanForm(preferences: OperatorPreferences): CreateTunnelPlanRequest {
  return {
    name: "",
    interface_name: "tun0",
    kind: "gre",
    runtime_control: { manager: "agent_iproute2_managed", traffic_limit: {} },
    runtime_topology: {},
    left_client_id: "",
    right_client_id: "",
    left_underlay: "",
    right_underlay: "",
    address_pool_cidr: preferences.tunnel_ipv4_allocation_pool_cidr,
    reserved_addresses: [],
    ipv4_tunnel: null,
    ipv6_address_pool_cidr: preferences.tunnel_ipv6_allocation_pool_cidr,
    ipv6_tunnel: null,
    latency_primary_family: "ipv4",
    bandwidth: "100m",
    latency_ms: 20,
    packet_loss_ratio: 0,
    preference: 1,
    enabled: false,
    confirmed: false,
  };
}

function saveBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name || "plan.json";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

function safeFileName(value: string): string {
  return value.trim().replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "tunnel-plan";
}

function normalizePair(pair: TunnelAddressPair): TunnelAddressPair | null {
  if (!pair.left && !pair.right) {
    return null;
  }
  return pair;
}

function hasAddressSource(form: CreateTunnelPlanRequest): boolean {
  return Boolean(completePairOrNull(form.ipv4_tunnel ?? null) || completePairOrNull(form.ipv6_tunnel ?? null));
}

function completePairOrNull(pair: TunnelAddressPair | null): TunnelAddressPair | null {
  if (!pair?.left || !pair.right) {
    return null;
  }
  return pair;
}

function buildMonitoringConfigPatchToml(enabled: boolean): string {
  return tomlDocument({
    network: {
      runtime_status_telemetry_enabled: true,
      latency_monitoring_enabled: enabled,
      auto_ospf_enabled: enabled,
    },
  });
}

function OspfCostModelNote() {
  return (
    <div className="operationNote formSectionNote compactModelNote" title={OSPF_COST_MODEL_DETAIL}>
      <strong>OSPF cost model</strong>
      <span>{OSPF_COST_MODEL_SUMMARY}</span>
    </div>
  );
}

function TunnelPlanGridDetail({
  clientLabel,
  plan,
}: {
  clientLabel: (clientId: string) => string;
  plan: TunnelPlanRecord;
}) {
  return (
    <div className="gridDetailLine">
      <strong>{plan.name}</strong>
      <span>{plan.enabled ? "enabled" : "disabled"}</span>
      <span className="monoCell">interface {plan.plan.interface_name}</span>
      <span>{plan.kind.toUpperCase()} / {runtimeManagerLabel(plan.plan.runtime_control?.manager)}</span>
      <span>{clientLabel(plan.left_client_id)} underlay {plan.plan.left_underlay}</span>
      <span>{clientLabel(plan.right_client_id)} underlay {plan.plan.right_underlay}</span>
      <span>IPv4 {formatAddressPair(plan.plan.ipv4_tunnel)}</span>
      <span>IPv6 {formatAddressPair(plan.plan.ipv6_tunnel)}</span>
      <span>primary {addressFamilyLabel(plan.plan.latency_primary_family)}</span>
      <span>loss {(plan.plan.packet_loss_ratio * 100).toFixed(1)}%</span>
      <span>preference {plan.plan.preference}</span>
      <span>L {readableTelemetryToken(plan.left_status)} / R {readableTelemetryToken(plan.right_status)}</span>
      {plan.last_apply_job_id && <span>apply {shortId(plan.last_apply_job_id)}</span>}
      {plan.last_rollback_job_id && <span>rollback {shortId(plan.last_rollback_job_id)}</span>}
    </div>
  );
}

function AutomationGridDetail({ row }: { row: AutomationRow }) {
  if (row.tunnels.length === 0) {
    return (
      <div className="gridDetailLine">
        <strong>{row.label}</strong>
        <span>{row.endpointCount} saved endpoints</span>
        <span>No latest tunnel telemetry reports</span>
      </div>
    );
  }
  return (
    <div className="networkInterfaceList">
      {row.tunnels.map((tunnel) => (
        <div className={`networkInterfaceRow ${telemetryTunnelRowClass(tunnel)}`} key={`${tunnel.client_id}:${tunnel.interface}:${tunnel.plan_id ?? tunnel.plan_name ?? ""}`}>
          <TelemetryCell
            detail={`${readableTelemetryToken(tunnel.kind)}; ${endpointSideLabel(tunnel.endpoint_side)}`}
            main={tunnel.interface}
            title={`${readableTelemetryToken(tunnel.kind)}; ${endpointSideLabel(tunnel.endpoint_side)}; ${telemetrySourceLabel(tunnel.source)}; ${planCorrelationLabel(tunnel.plan_correlation)}; ${mutationPolicyLabel(tunnel.mutation_policy)}`}
          />
          <TelemetryCell
            detail={formatAutomationLatencyShort(tunnel)}
            main={formatTunnelLatencySummary(tunnel)}
            title={formatAutomationLatencyDetail(tunnel)}
            tone={latencyTone(tunnel.latency_status)}
          />
          <TelemetryCell
            detail={formatAutomationOspfShort(tunnel)}
            main={formatTunnelOspfSummary(tunnel)}
            title={formatAutomationOspfDetail(tunnel)}
            tone={ospfTone(tunnel.auto_ospf_status)}
          />
          <TelemetryCell
            detail={`observed ${formatTime(tunnel.observed_at)}; ${tunnel.peer_client_id ?? "peer unknown"}`}
            main={formatTunnelCostSummary(tunnel)}
            title={`observed ${formatTime(tunnel.observed_at)}; peer ${tunnel.peer_client_id ?? "unknown"}`}
          />
        </div>
      ))}
    </div>
  );
}

function buildAutomationRows(
  agents: AgentView[],
  tunnels: TelemetryTunnelRecord[],
  tunnelPlans: TunnelPlanRecord[],
  clientLabel: (clientId: string) => string,
) {
  return agents.map((agent) => {
    const endpointCount = tunnelPlans.filter(
      (record) => record.enabled && (record.plan.left_client_id === agent.id || record.plan.right_client_id === agent.id),
    ).length;
    const owned = tunnels.filter((tunnel) => tunnel.client_id === agent.id && tunnel.plan_name);
    const monitoredCount = owned.filter((tunnel) => tunnel.latency_monitoring_enabled).length;
    const latencyStates = summarizeStates(
      owned.map((tunnel) => tunnel.latency_status).filter(Boolean) as string[],
      latencyStatusLabel,
    );
    const autoStates = summarizeStates(
      owned.map((tunnel) => tunnel.auto_ospf_status).filter(Boolean) as string[],
      (status) => ospfStatusLabel(status, true),
    );
    const prioritized = owned.slice().sort(compareTunnelUrgency);
    const costs = owned
      .filter((tunnel) => tunnel.auto_ospf_current_cost || tunnel.auto_ospf_recommended_cost)
      .map((tunnel) =>
        tunnel.auto_ospf_current_cost && tunnel.auto_ospf_recommended_cost
          ? `${tunnel.auto_ospf_current_cost}->${tunnel.auto_ospf_recommended_cost}`
          : String(tunnel.auto_ospf_recommended_cost ?? tunnel.auto_ospf_current_cost),
      );
    const unreportedCount = Math.max(endpointCount - owned.length, 0);
    const latestObserved = latestIso(owned.map((tunnel) => tunnel.observed_at));
    return {
      clientId: agent.id,
      endpointCount,
      label: clientLabel(agent.id),
      monitored: endpointCount === 0 ? "No tunnel endpoints" : `${monitoredCount}/${endpointCount}`,
      latency: latencyStates || "No samples",
      latencyDetail: prioritized.map(formatAutomationLatencyShort).filter(Boolean).slice(0, 2).join(" · ")
        || (endpointCount > 0 ? `${owned.length}/${endpointCount} endpoint reports` : "No saved tunnel endpoints"),
      latencyTitle: prioritized.map(formatAutomationLatencyDetail).filter(Boolean).slice(0, 4).join(" | ")
        || (endpointCount > 0 ? `${owned.length}/${endpointCount} endpoint reports` : "No saved tunnel endpoints"),
      latencyTone: automationLatencyTone(owned),
      autoOspf: autoStates || "Monitoring only",
      autoOspfDetail: prioritized.map(formatAutomationOspfShort).filter(Boolean).slice(0, 2).join(" · ")
        || "No updater report",
      autoOspfTitle: prioritized.map(formatAutomationOspfDetail).filter(Boolean).slice(0, 4).join(" | ")
        || "External updater not reporting",
      autoOspfTone: automationOspfTone(owned),
      cost: costs.slice(0, 3).join(", ") || "-",
      costDetail: costs.length > 3 ? `${costs.length - 3} more cost reports` : "current to recommended",
      tunnels: prioritized,
      urgency: prioritized[0] ? tunnelUrgency(prioritized[0]) : 0,
      lastReport: latestObserved ? formatTime(latestObserved) : "No report",
      reportDetail: unreportedCount > 0
        ? `${unreportedCount} saved endpoint${unreportedCount === 1 ? "" : "s"} without telemetry`
        : `${owned.length} latest tunnel report${owned.length === 1 ? "" : "s"}`,
    };
  });
}

function TelemetryCell({
  detail,
  main,
  title,
  tone = "neutral",
}: {
  detail: string;
  main: string;
  title?: string;
  tone?: "critical" | "neutral" | "ok" | "warn";
}) {
  return (
    <span className="telemetryStack" title={title ?? detail}>
      <strong className={`telemetryStatus ${tone}`}>{main}</strong>
      <small>{detail}</small>
    </span>
  );
}

function planStatusClass(status: string): "neutral" | "ok" | "warn" {
  if (status.includes("rolled_back") || status.includes("failed")) {
    return "warn";
  }
  if (status.includes("applied")) {
    return "ok";
  }
  return "neutral";
}

function formatAddressPair(pair: TunnelAddressPair | null | undefined): string {
  if (!pair) {
    return "disabled";
  }
  return `${pair.left} / ${pair.right}/${pair.prefix_len}`;
}

function bandwidthTierLabel(tier: BandwidthTier): string {
  if (tier === "1000m") {
    return "1000m (1000 Mbps)";
  }
  if (tier === "100m") {
    return "100m (100 Mbps)";
  }
  return "10m (10 Mbps)";
}

function telemetryTunnelRowClass(tunnel: TelemetryTunnelRecord): string {
  if (latencyTone(tunnel.latency_status) === "critical" || ospfTone(tunnel.auto_ospf_status) === "critical") {
    return "telemetryRowCritical";
  }
  if (latencyTone(tunnel.latency_status) === "warn" || ospfTone(tunnel.auto_ospf_status) === "warn") {
    return "telemetryRowWarn";
  }
  return "";
}

function latencyTone(status: string | null | undefined): "critical" | "neutral" | "ok" | "warn" {
  if (status === "down") {
    return "critical";
  }
  if (status === "healthy") {
    return "ok";
  }
  if (status === "missed" || status === "unconfigured" || status === "disabled") {
    return "warn";
  }
  return "neutral";
}

function ospfTone(status: string | null | undefined): "critical" | "neutral" | "ok" | "warn" {
  if (status === "failed") {
    return "critical";
  }
  if (status === "updated" || status === "stable" || status === "disabled") {
    return "ok";
  }
  if (status === "report_only" || status === "stabilizing" || status === "monitoring_only") {
    return "warn";
  }
  return "neutral";
}

function formatTunnelLatencySummary(tunnel: TelemetryTunnelRecord): string {
  const status = latencyStatusLabel(tunnel.latency_status);
  if (typeof tunnel.latency_avg_ms === "number") {
    return `${status} / ${tunnel.latency_avg_ms.toFixed(1)} ms`;
  }
  return status;
}

function formatTunnelOspfSummary(tunnel: TelemetryTunnelRecord): string {
  return ospfStatusLabel(tunnel.auto_ospf_status, tunnel.auto_ospf_enabled);
}

function formatTunnelCostSummary(tunnel: TelemetryTunnelRecord): string {
  if (tunnel.auto_ospf_current_cost || tunnel.auto_ospf_recommended_cost) {
    return `${tunnel.auto_ospf_current_cost ?? "?"}->${tunnel.auto_ospf_recommended_cost ?? "?"}`;
  }
  return "cost n/a";
}

function compareTunnelUrgency(left: TelemetryTunnelRecord, right: TelemetryTunnelRecord): number {
  return tunnelUrgency(right) - tunnelUrgency(left) || right.observed_at.localeCompare(left.observed_at);
}

function tunnelUrgency(tunnel: TelemetryTunnelRecord): number {
  const latency = tunnel.latency_status ?? "";
  const autoOspf = tunnel.auto_ospf_status ?? "";
  if (latency === "down" || autoOspf === "failed") {
    return 4;
  }
  if (latency === "missed" || autoOspf === "report_only") {
    return 3;
  }
  if (latency === "unconfigured" || latency === "disabled" || autoOspf === "monitoring_only") {
    return 2;
  }
  if (latency === "healthy" || autoOspf === "updated" || autoOspf === "stable") {
    return 1;
  }
  return 0;
}

function automationLatencyTone(tunnels: TelemetryTunnelRecord[]): "critical" | "neutral" | "ok" | "warn" {
  const states = tunnels.map((tunnel) => tunnel.latency_status ?? "");
  if (states.includes("down")) {
    return "critical";
  }
  if (states.some((state) => ["missed", "unconfigured", "disabled"].includes(state))) {
    return "warn";
  }
  if (states.includes("healthy")) {
    return "ok";
  }
  return "neutral";
}

function automationOspfTone(tunnels: TelemetryTunnelRecord[]): "critical" | "neutral" | "ok" | "warn" {
  const states = tunnels.map((tunnel) => tunnel.auto_ospf_status ?? "");
  if (states.includes("failed")) {
    return "critical";
  }
  if (states.some((state) => ["report_only", "stabilizing", "monitoring_only"].includes(state))) {
    return "warn";
  }
  if (states.some((state) => ["updated", "stable", "disabled"].includes(state))) {
    return "ok";
  }
  return "neutral";
}

function formatAutomationLatencyDetail(tunnel: TelemetryTunnelRecord): string {
  const status = latencyStatusLabel(tunnel.latency_status);
  const metric = typeof tunnel.latency_avg_ms === "number" ? `${tunnel.latency_avg_ms.toFixed(1)} ms` : "no avg";
  const loss = typeof tunnel.packet_loss_ratio === "number" ? `${(tunnel.packet_loss_ratio * 100).toFixed(1)}% loss` : "loss n/a";
  const checked = typeof tunnel.latency_checked_unix === "number" ? formatUnixTime(tunnel.latency_checked_unix) : formatTime(tunnel.observed_at);
  const windows = [
    typeof tunnel.latency_healthy_windows === "number" ? `ok ${tunnel.latency_healthy_windows}` : "",
    typeof tunnel.latency_missed_windows === "number" ? `miss ${tunnel.latency_missed_windows}` : "",
  ].filter(Boolean).join("/");
  const target = `${addressFamilyLabel(tunnel.latency_primary_family)} ${tunnel.latency_target ?? "target n/a"}`;
  const reason = telemetryReasonLabel(tunnel.latency_reason);
  return `${tunnel.interface}: ${status}; ${metric}; ${loss}; ${target}; ${windows || "windows n/a"}; checked ${checked}${reason ? `; ${reason}` : ""}`;
}

function formatAutomationOspfDetail(tunnel: TelemetryTunnelRecord): string {
  const status = ospfStatusLabel(tunnel.auto_ospf_status, tunnel.auto_ospf_enabled);
  const costs = tunnel.auto_ospf_current_cost || tunnel.auto_ospf_recommended_cost
    ? `${tunnel.auto_ospf_current_cost ?? "?"}->${tunnel.auto_ospf_recommended_cost ?? "?"}`
    : "cost n/a";
  const updated = typeof tunnel.auto_ospf_updated_unix === "number" ? `updated ${formatUnixTime(tunnel.auto_ospf_updated_unix)}` : "no update";
  const reason = telemetryReasonLabel(tunnel.auto_ospf_reason);
  return `${tunnel.interface}: ${status}; cost ${costs}; ${updated}${reason ? `; ${reason}` : ""}`;
}

function formatAutomationLatencyShort(tunnel: TelemetryTunnelRecord): string {
  const parts = [
    tunnel.interface,
    typeof tunnel.packet_loss_ratio === "number" && tunnel.packet_loss_ratio > 0
      ? `${(tunnel.packet_loss_ratio * 100).toFixed(1)}% loss`
      : typeof tunnel.latency_avg_ms === "number"
        ? `${tunnel.latency_avg_ms.toFixed(1)} ms`
        : latencyStatusLabel(tunnel.latency_status),
  ].filter(Boolean);
  return parts.join("; ") || "No latency detail";
}

function formatAutomationOspfShort(tunnel: TelemetryTunnelRecord): string {
  const parts = [
    tunnel.interface,
    formatTunnelCostSummary(tunnel),
    shortTelemetryReasonLabel(tunnel.auto_ospf_reason),
  ].filter((part) => part && part !== "cost n/a");
  return parts.join("; ") || "No updater detail";
}

function shortTelemetryReasonLabel(reason: string | null | undefined): string {
  if (!reason) {
    return "";
  }
  if (reason.startsWith("external_cost_program_unconfigured")) {
    return "No updater";
  }
  if (reason.startsWith("external_cost_program_succeeded")) {
    return "Applied";
  }
  if (reason.startsWith("latency_probe_unhealthy")) {
    return "Adjacency down";
  }
  if (reason.startsWith("latency_probe_missing_healthy_sample")) {
    return "Awaiting probes";
  }
  return telemetryReasonLabel(reason);
}

function latestIso(values: string[]): string | null {
  return values.reduce<string | null>((latest, value) => (!latest || value > latest ? value : latest), null);
}

function formatUnixTime(value: number): string {
  return formatTime(new Date(value * 1000).toISOString());
}

function summarizeStates(states: string[], label: (state: string) => string): string {
  const counts = new Map<string, number>();
  for (const state of states) {
    counts.set(state, (counts.get(state) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .map(([state, count]) => `${label(state)} ${count}`)
    .join(", ");
}

type TomlValue =
  | string
  | number
  | boolean
  | TomlValue[]
  | object
  | null
  | undefined;

function tomlDocument(root: Record<string, TomlValue>): string {
  const lines: string[] = [];
  for (const [section, value] of Object.entries(root)) {
    if (!isTomlObject(value)) {
      continue;
    }
    lines.push(`[${tomlKey(section)}]`);
    for (const [key, child] of Object.entries(value)) {
      if (child === null || child === undefined) {
        continue;
      }
      lines.push(`${tomlKey(key)} = ${tomlValue(child)}`);
    }
    lines.push("");
  }
  return `${lines.join("\n").trim()}\n`;
}

function tomlValue(value: TomlValue): string {
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    return Number.isFinite(value) ? String(value) : "0";
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (Array.isArray(value)) {
    return `[${value.filter((item) => item !== null && item !== undefined).map(tomlValue).join(", ")}]`;
  }
  if (isTomlObject(value)) {
    return tomlInlineTable(value);
  }
  return "\"\"";
}

function tomlInlineTable(value: object): string {
  const entries = Object.entries(value as Record<string, TomlValue | null | undefined>)
    .filter((entry): entry is [string, TomlValue] => entry[1] !== null && entry[1] !== undefined)
    .map(([key, child]) => `${tomlKey(key)} = ${tomlValue(child)}`);
  return `{ ${entries.join(", ")} }`;
}

function isTomlObject(value: TomlValue): value is object {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function tomlKey(value: string): string {
  return /^[A-Za-z0-9_-]+$/.test(value) ? value : JSON.stringify(value);
}
