import { useMemo, useState, type FormEvent } from "react";
import { GitBranch, Power, PowerOff, RefreshCcw, Route, Save, Wand2 } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
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
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../privilege";
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
  PromoteTelemetryTunnelRequest,
  RuntimeTunnelManager,
  TelemetryTunnelRecord,
  TunnelAddressFamily,
  TunnelAddressPair,
  TopologyGraph,
  TunnelKind,
  TunnelPlanRecord,
} from "../types";
import type { PromoteTunnelPlanToAdapterRequest } from "../typesTopology";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatTime,
  runPanelAction,
  shortId,
} from "../utils";
import { TopologyApplyControls } from "./topology/TopologyApplyControls";
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
  onCreateJob,
  onAllocateTunnelEndpoints,
  onCreateTunnelPlan,
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
  onPromoteTunnelPlanToAdapter,
  onRefresh,
  onSetTunnelPlanEnabled,
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
  topologyGraph: TopologyGraph;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onAllocateTunnelEndpoints: (request: AllocateTunnelEndpointsRequest) => Promise<AllocateTunnelEndpointsResponse>;
  onCreateTunnelPlan: (request: CreateTunnelPlanRequest) => Promise<void>;
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
  onPromoteTunnelPlanToAdapter: (request: PromoteTunnelPlanToAdapterRequest) => Promise<void>;
  onRefresh: () => Promise<void>;
  onSetTunnelPlanEnabled: (planIds: string[], enabled: boolean) => Promise<void>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [form, setForm] = useState<CreateTunnelPlanRequest>({
    name: "",
    interface_name: "tun0",
    kind: "gre",
    runtime_control: { manager: "agent_iproute2_managed", traffic_limit: {} },
    runtime_topology: {},
    left_client_id: "",
    right_client_id: "",
    left_underlay: "",
    right_underlay: "",
    address_pool_cidr: "10.255.0.0/30",
    reserved_addresses: [],
    ipv4_tunnel: null,
    ipv6_address_pool_cidr: "",
    ipv6_tunnel: null,
    latency_primary_family: "ipv4",
    bandwidth: "100m",
    latency_ms: 20,
    packet_loss_ratio: 0,
    preference: 1,
  });
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
  const [topologyVersion, setTopologyVersion] = useState("");
  const [topologyDesiredText, setTopologyDesiredText] = useState("");
  const [topologyStaleText, setTopologyStaleText] = useState("");
  const [topologyRoutesText, setTopologyRoutesText] = useState("");
  const [topologyStaleRoutesText, setTopologyStaleRoutesText] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [automationBulkPending, setAutomationBulkPending] = useState(false);
  const [automationBulkStatus, setAutomationBulkStatus] = useState<string | null>(null);
  const [tunnelPlanTogglePending, setTunnelPlanTogglePending] = useState(false);
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

  async function submitPlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!ready) {
        throw new Error("Tunnel plan is incomplete");
      }
      await onCreateTunnelPlan({
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
          version: topologyVersion,
          desiredText: topologyDesiredText,
          staleText: topologyStaleText,
          routesText: topologyRoutesText,
          staleRoutesText: topologyStaleRoutesText,
        }),
      });
    });
  }

  async function allocateEndpoints() {
    await runPanelAction(setPending, setActionError, async () => {
      const includeIpv4 = Boolean(form.address_pool_cidr.trim());
      const includeIpv6 = Boolean((form.ipv6_address_pool_cidr ?? "").trim());
      if (!includeIpv4 && !includeIpv6) {
        throw new Error("Enter an IPv4 or IPv6 pool before generating endpoints");
      }
      const allocation = await onAllocateTunnelEndpoints({
        ipv4_pool_cidr: includeIpv4 ? form.address_pool_cidr.trim() : null,
        ipv6_pool_cidr: includeIpv6 ? (form.ipv6_address_pool_cidr ?? "").trim() : null,
        reserved_addresses: splitReserved(reservedText),
        include_ipv4: includeIpv4,
        include_ipv6: includeIpv6,
      });
      setForm((current) => ({
        ...current,
        ipv4_tunnel: allocation.ipv4_tunnel,
        ipv6_tunnel: allocation.ipv6_tunnel,
        latency_primary_family: allocation.latency_primary_family,
      }));
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
      />
      )}

      {topologySubpage === "plans" && (
      <section className="fleetPanel scheduleComposer topologyPlanComposer">
        <div className="sectionHeader">
          <div>
            <h2>Create tunnel plan</h2>
            <span>Observe-plan only; no host mutation</span>
          </div>
          <GitBranch size={20} />
        </div>
        <form className="dispatchForm" onSubmit={submitPlan}>
          <div className="operationNote formSectionNote">
            <strong>Plan identity</strong>
            <span>Name the intended tunnel and choose the link type before selecting endpoints.</span>
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
          </div>
          <div className="dispatchControls">
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
          </div>
          <div className="operationNote formSectionNote">
            <strong>Endpoints and address plan</strong>
            <span>Pair exactly two VPSs, provide underlays, then enter or generate explicit tunnel endpoints.</span>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Left VPS"
                excludeIds={form.right_client_id ? [form.right_client_id] : []}
                onChange={(value) => setField("left_client_id", value)}
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
                onChange={(value) => setField("right_client_id", value)}
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
              <span>IPv4 allocation pool</span>
              <input
                value={form.address_pool_cidr}
                onChange={(event) => setField("address_pool_cidr", event.target.value)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>IPv6 allocation pool</span>
              <input
                value={form.ipv6_address_pool_cidr ?? ""}
                onChange={(event) => setField("ipv6_address_pool_cidr", event.target.value)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>Reserved</span>
              <input value={reservedText} onChange={(event) => setReservedText(event.target.value)} />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left IPv4</span>
              <input
                value={form.ipv4_tunnel?.left ?? ""}
                onChange={(event) => setAddressPair("ipv4_tunnel", "left", event.target.value, 31)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>Right IPv4</span>
              <input
                value={form.ipv4_tunnel?.right ?? ""}
                onChange={(event) => setAddressPair("ipv4_tunnel", "right", event.target.value, 31)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>IPv4 prefix</span>
              <input
                max={32}
                min={0}
                onChange={(event) => setAddressPrefix("ipv4_tunnel", Number(event.target.value), 31)}
                type="number"
                value={form.ipv4_tunnel?.prefix_len ?? 31}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left IPv6</span>
              <input
                value={form.ipv6_tunnel?.left ?? ""}
                onChange={(event) => setAddressPair("ipv6_tunnel", "left", event.target.value, 127)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>Right IPv6</span>
              <input
                value={form.ipv6_tunnel?.right ?? ""}
                onChange={(event) => setAddressPair("ipv6_tunnel", "right", event.target.value, 127)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>IPv6 prefix</span>
              <input
                max={128}
                min={0}
                onChange={(event) => setAddressPrefix("ipv6_tunnel", Number(event.target.value), 127)}
                type="number"
                value={form.ipv6_tunnel?.prefix_len ?? 127}
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
              Generate endpoints
            </button>
          </div>
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
          <div className="operationNote formSectionNote">
            <strong>Runtime ownership and traffic limits</strong>
            <span>Choose whether the agent owns the tunnel, only observes it, or delegates commands to an adapter.</span>
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
            <label>
              <span>Topology version</span>
              <input value={topologyVersion} onChange={(event) => setTopologyVersion(event.target.value)} />
            </label>
          </div>
          <label className="checkLine">
            <input
              checked={trafficLimitEnabled}
              onChange={(event) => setTrafficLimitEnabled(event.target.checked)}
              type="checkbox"
            />
            <span>Enable traffic shaping</span>
          </label>
          <div className="dispatchControls">
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
          <div className="operationNote formSectionNote">
            <strong>Topology evidence</strong>
            <span>Optional desired/stale interface and route evidence helps later promotion and drift review.</span>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Desired interfaces</span>
              <input value={topologyDesiredText} onChange={(event) => setTopologyDesiredText(event.target.value)} />
            </label>
            <label>
              <span>Stale interfaces</span>
              <input value={topologyStaleText} onChange={(event) => setTopologyStaleText(event.target.value)} />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Routes</span>
              <textarea value={topologyRoutesText} onChange={(event) => setTopologyRoutesText(event.target.value)} />
            </label>
            <label>
              <span>Stale routes</span>
              <textarea value={topologyStaleRoutesText} onChange={(event) => setTopologyStaleRoutesText(event.target.value)} />
            </label>
          </div>
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
        onPromoteTunnelPlanToAdapter={onPromoteTunnelPlanToAdapter}
        telemetryTunnels={telemetryTunnels}
        tunnelPlans={tunnelPlans}
      />
      )}

      {topologySubpage === "apply" && tunnelPlans.length > 0 && (
        <TopologyApplyControls
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
            onCreateJob={onCreateJob}
            onLoadTargets={onLoadTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            ospfUpdatePlans={ospfUpdatePlans}
            privilegeMaterial={privilegeMaterial}
            setPrivilegeMaterial={setPrivilegeMaterial}
            tunnelPlans={tunnelPlans}
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

  function setAddressPair(
    key: "ipv4_tunnel" | "ipv6_tunnel",
    side: "left" | "right",
    value: string,
    fallbackPrefix: number,
  ) {
    setForm((current) => {
      const currentPair = current[key] ?? { left: "", right: "", prefix_len: fallbackPrefix };
      const nextPair = { ...currentPair, [side]: value.trim() };
      return { ...current, [key]: normalizePair(nextPair) };
    });
  }

  function setAddressPrefix(key: "ipv4_tunnel" | "ipv6_tunnel", value: number, fallbackPrefix: number) {
    setForm((current) => {
      const currentPair = current[key] ?? { left: "", right: "", prefix_len: fallbackPrefix };
      const nextPair = { ...currentPair, prefix_len: Number.isFinite(value) ? Math.trunc(value) : fallbackPrefix };
      return { ...current, [key]: normalizePair(nextPair) };
    });
  }

  async function setTunnelPlanEnabledForRows(rows: TunnelPlanRecord[], enabled: boolean) {
    const targets = rows.filter((plan) => plan.enabled !== enabled);
    if (targets.length === 0) {
      return;
    }
    await runPanelAction(setTunnelPlanTogglePending, setActionError, async () => {
      await onSetTunnelPlanEnabled(targets.map((plan) => plan.id), enabled);
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
      let submitted = 0;
      for (const target of targets) {
        const toml = buildMonitoringConfigPatchToml(target.clientId, tunnelPlans, enabled);
        const operation = {
          type: "data_source_config_patch" as const,
          apply_mode: "incremental_patch" as const,
          toml,
        };
        const selectorExpression = selectorExpressionForClientIds([target.clientId]);
        const timeoutSecs = 120;
        const builtPrivilege = await buildPrivilegeForJobOperation({
          clientIds: [target.clientId],
          commandType: "data_source_config_patch",
          operation,
          privilegeMaterial,
          selectorExpression,
          timeoutSecs,
        });
        await onCreateJob({
          argv: [],
          command: "data_source_config_patch",
          confirmed: true,
          destructive: true,
          force_unprivileged: false,
          operation,
          privileged: true,
          privilege_assertion: builtPrivilege.privilegeAssertion,
          selector_expression: selectorExpression,
          target_client_ids: [target.clientId],
          timeout_secs: timeoutSecs,
        });
        submitted += 1;
      }
      setAutomationBulkStatus(`${enabled ? "Enabled" : "Disabled"} monitoring on ${submitted} VPSs`);
    });
  }
}

function splitReserved(value: string): string[] {
  return value
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
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

function buildMonitoringConfigPatchToml(clientId: string, tunnelPlans: TunnelPlanRecord[], enabled: boolean): string {
  const telemetryPlans = tunnelPlans.flatMap((record) => {
    if (!record.enabled) {
      return [];
    }
    const entries = [];
    if (record.plan.left_client_id === clientId) {
      entries.push({
        plan_id: record.id,
        endpoint_side: "left",
        plan: record.plan,
        latency_monitoring_enabled: enabled,
        auto_ospf_enabled: enabled,
      });
    }
    if (record.plan.right_client_id === clientId) {
      entries.push({
        plan_id: record.id,
        endpoint_side: "right",
        plan: record.plan,
        latency_monitoring_enabled: enabled,
        auto_ospf_enabled: enabled,
      });
    }
    return entries;
  });
  if (telemetryPlans.length === 0) {
    throw new Error("Selected VPS has no saved tunnel endpoints");
  }
  if (telemetryPlans.length > 16) {
    throw new Error(`${clientId} has ${telemetryPlans.length} tunnel endpoints; agent telemetry config supports 16`);
  }
  return tomlDocument({
    network: {
      runtime_status_telemetry_enabled: true,
      latency_monitoring_enabled: enabled,
      auto_ospf_enabled: enabled,
      runtime_status_telemetry_plans: telemetryPlans,
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
