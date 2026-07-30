import {
  type FormEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Activity,
  CirclePlus,
  Download,
  ExternalLink,
  Gauge,
  GitGraph,
  Network,
  Pencil,
  Power,
  PowerOff,
  RefreshCcw,
  Repeat2,
  Route,
  Search,
  Settings2,
  ShieldAlert,
  Trash2,
  X,
} from "lucide-react";
import { ActionFeedback, type ActionFeedbackTone } from "../components/ActionFeedback";
import { ApiResponseError } from "../api";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleActionMenu,
  ConsoleContextActionMenu,
  ConsoleInlineActions,
  type ConsoleMenuAction,
} from "../components/ConsoleLayout";
import { VpsCombobox } from "../components/VpsCombobox";
import { scrollIntoViewWithMotion } from "../motion";
import { tunnelEndpointRuntimeStateBadgeClass } from "../jobStatusPresentation";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildRuntimeControl,
  buildRuntimeTopology,
  calculateOspfCostPreview,
  clampTunnelBandwidthMbps,
  DEFAULT_TUNNEL_BANDWIDTH_MBPS,
  MAX_TUNNEL_BANDWIDTH_MBPS,
  MIN_TUNNEL_BANDWIDTH_MBPS,
  OSPF_COST_MODEL_DETAIL,
  readableTelemetryToken,
  runtimeManagerLabel,
} from "../topologyRuntime";
import type {
  AgentView,
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
  ConfigurationSourceView,
  CreateJobRequest,
  CreateJobResponse,
  CreateTunnelPlanRequest,
  JobHistoryRecord,
  JobOutputRecord,
  JobTargetRecord,
  NetworkObservationRecord,
  NetworkAdapterDefinitionRecord,
  NetworkAdapterKind,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  OperatorView,
  OspfCostPolicy,
  PrivilegeAssertion,
  RuntimeConfigApplyStateRecord,
  RuntimeConfigDispatchRecord,
  TunnelPlanOspfJobsResponse,
  RuntimeTunnelRoute,
  RuntimeTunnelManager,
  TelemetryTunnelRecord,
  TopologyGraph,
  TopologyGraphEdge,
  TunnelAddressPair,
  TunnelKind,
  TunnelPlan,
  TunnelPlanEndpointRuntimeConfig,
  TunnelPlanMutationResponse,
  TunnelPlanRecord,
  TunnelPlanRevisionTarget,
  TunnelConnectionAssessment,
  UpdateTunnelConnectionAssessmentRequest,
  UpdateTunnelPlanOspfCostRequest,
  UpdateTunnelPlanRequest,
  UpsertNetworkAdapterDefinitionRequest,
} from "../types";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  dispatchFailureReason,
  formatCompactTime,
  shortId,
  timestampMillis,
} from "../utils";
import type { PrivilegeMaterial } from "../privilege";
import { TopologyEvidencePanel } from "./topology/TopologyEvidencePanel";
import { TopologyGraphPanel } from "./topology/TopologyGraphPanel";
import { TopologyNetworkTestControls } from "./topology/TopologyNetworkTestControls";
import { TopologyOspfUpdateControls } from "./topology/TopologyOspfUpdateControls";
import { PortForwardingPanel } from "./topology/PortForwardingPanel";
import { NetworkAdapterDefinitionsPanel } from "./topology/NetworkAdapterDefinitionsPanel";

const AGENT_TUNNEL_KINDS: TunnelKind[] = ["gre", "ipip", "sit", "fou"];
const ALL_TUNNEL_KINDS: TunnelKind[] = [
  "gre",
  "ipip",
  "sit",
  "fou",
  "wireguard",
  "openvpn",
  "tun_tap",
  "custom",
];
const DEFAULT_OSPF_POLICY: OspfCostPolicy = {
  bandwidth_weight: 10,
  latency_weight: 1,
  loss_weight: 400,
  max_cost: 65535,
  min_cost: 5,
  preference_bias: 1,
};

export function TopologyPanel({
  activeSubpage,
  agents,
  configurationSources,
  configurationSourcesEvidenceState,
  error,
  initialAdapterKind,
  initialPlanWorkflow,
  initialTargetIntent,
  jobs,
  loading,
  networkObservations,
  networkTrends,
  onAllocateTunnelEndpoints,
  onCreateJob,
  onCreateTunnelPlan,
  onCreateNetworkAdapterDefinition,
  onDeleteNetworkAdapterDefinition,
  onDeleteTunnelPlan,
  onExportTunnelPlan,
  onInitialPlanWorkflowConsumed,
  onInitialAdapterKindConsumed,
  onInitialTargetIntentConsumed,
  onLoadRuntimeConfigApplyStates,
  onLoadConfigurationSources,
  onLoadNetworkObservations,
  onLoadNetworkTrends,
  onLoadOspfRecommendations,
  onLoadOspfUpdatePlans,
  onLoadNetworkAdapterDefinitions,
  onLoadOutputs,
  onLoadTargets,
  onLoadTopologyGraph,
  onOpenJobDetails,
  onOpenCreateTunnelPlan,
  onOpenConfigurationSources,
  onOpenPrivilegeUnlock,
  onOpenAdapterDefinitions,
  onOpenVpsDetail,
  onBulkMutatePortForwardRules,
  onCreatePortForwardRule,
  onLoadPortForwardRules,
  onMutatePortForwardRule,
  onResolvePortForwardHostname,
  onUpdatePortForwardRule,
  onRefresh,
  onRefreshTunnelPlanOspfStatus,
  onSelectSubpage,
  onSetTunnelPlanEnabled,
  onUpdateTunnelConnectionAssessment,
  onUpdateTunnelPlanOspfCost,
  onUpdateTunnelPlan,
  onUpdateNetworkAdapterDefinition,
  operator,
  ospfRecommendations,
  ospfUpdatePlans,
  portForwardError,
  portForwardLoading,
  portForwardRules,
  privilegeMaterial,
  runtimeConfigEvidenceState,
  runtimeConfigApplyStates,
  setPrivilegeMaterial,
  networkAdapterDefinitions,
  telemetryTunnels,
  topologyGraph,
  tunnelPlanCorruptions,
  tunnelPlans,
}: TopologyPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const clientNames = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, clientNames);
  const [adapterDefinitionsLoadError, setAdapterDefinitionsLoadError] = useState<
    string | null
  >(null);

  useEffect(() => {
    if (activeSubpage === "graph") {
      void Promise.all([
        onLoadTopologyGraph(),
        onLoadRuntimeConfigApplyStates(),
      ]);
    }
    if (activeSubpage === "overview") {
      void Promise.all([onLoadTopologyGraph(), onLoadOspfUpdatePlans()]);
    }
    if (activeSubpage === "evidence") {
      void Promise.all([
        onLoadNetworkObservations(),
        onLoadNetworkTrends(),
        onLoadOspfRecommendations(),
        onLoadOspfUpdatePlans(),
      ]);
    }
    if (activeSubpage === "tests") void onLoadNetworkTrends();
    if (activeSubpage === "ospf") void onLoadOspfUpdatePlans();
    if (activeSubpage === "port_forwards") void onLoadPortForwardRules();
    if (activeSubpage === "tunnel_plans") {
      setAdapterDefinitionsLoadError(null);
      void onLoadTopologyGraph();
      void onLoadConfigurationSources().catch(() => undefined);
      void onLoadNetworkAdapterDefinitions().catch((loadError) => {
        setAdapterDefinitionsLoadError(
          loadError instanceof Error
            ? loadError.message
            : "Network adapter definitions unavailable",
        );
      });
    }
  }, [
    activeSubpage,
    onLoadRuntimeConfigApplyStates,
    onLoadConfigurationSources,
    onLoadNetworkObservations,
    onLoadNetworkTrends,
    onLoadOspfRecommendations,
    onLoadOspfUpdatePlans,
    onLoadPortForwardRules,
    onLoadNetworkAdapterDefinitions,
    onLoadTopologyGraph,
  ]);

  if (activeSubpage === "port_forwards") {
    const hasNetworkWriteScope = Boolean(
      operator?.scopes.includes("*") || operator?.scopes.includes("network:write"),
    );
    return (
      <PortForwardingPanel
        agents={agents}
        canForget={operator?.role === "admin" && hasNetworkWriteScope}
        canWrite={
          (operator?.role === "operator" || operator?.role === "admin") &&
          hasNetworkWriteScope
        }
        error={portForwardError}
        loading={portForwardLoading}
        onBulkMutate={onBulkMutatePortForwardRules}
        onCreate={onCreatePortForwardRule}
        onLoad={onLoadPortForwardRules}
        onMutate={onMutatePortForwardRule}
        onResolveHostname={onResolvePortForwardHostname}
        onUpdate={onUpdatePortForwardRule}
        rules={portForwardRules}
      />
    );
  }

  if (activeSubpage === "graph") {
    return (
      <TopologyGraphPanel
        agents={agents}
        graph={topologyGraph}
        initialSelectedClientId={initialTargetIntent?.clientId ?? null}
        initialSelectionRequestId={initialTargetIntent?.requestId ?? null}
        loading={loading}
        onInitialSelectionConsumed={onInitialTargetIntentConsumed}
        onOpenVpsDetail={onOpenVpsDetail}
        onRefresh={async () => {
          await Promise.all([
            onLoadTopologyGraph(),
            onLoadRuntimeConfigApplyStates(),
          ]);
        }}
        runtimeConfigEvidenceState={runtimeConfigEvidenceState}
        runtimeConfigApplyStates={runtimeConfigApplyStates}
      />
    );
  }

  if (activeSubpage === "tests") {
    return (
      <TopologyNetworkTestControls
        agents={agents}
        loading={loading}
        networkTrends={networkTrends}
        onCreateJob={onCreateJob}
        onLoadNetworkTrends={onLoadNetworkTrends}
        onLoadOutputs={onLoadOutputs}
        onLoadTargets={onLoadTargets}
        onOpenJobDetails={onOpenJobDetails}
        onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
        onOpenTunnelPlans={() => onSelectSubpage("tunnel_plans")}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        tunnelPlans={tunnelPlans}
      />
    );
  }

  if (activeSubpage === "ospf") {
    return (
      <TopologyOspfUpdateControls
        agents={agents}
        onOpenJobDetails={onOpenJobDetails}
        onOpenAdapterDefinitions={() =>
          onOpenAdapterDefinitions("routing_cost")
        }
        onOpenConfigurationSources={onOpenConfigurationSources}
        onOpenTunnelPlans={() => onSelectSubpage("tunnel_plans")}
        onRefresh={async () => {
          await Promise.all([onRefresh(), onLoadOspfUpdatePlans()]);
        }}
        onRefreshTunnelPlanOspfStatus={onRefreshTunnelPlanOspfStatus}
        onUpdateTunnelPlanOspfCost={onUpdateTunnelPlanOspfCost}
        ospfUpdatePlans={ospfUpdatePlans}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        tunnelPlans={tunnelPlans}
      />
    );
  }

  if (activeSubpage === "evidence") {
    return (
      <TopologyEvidencePanel
        clientLabel={clientLabel}
        jobs={jobs}
        observations={networkObservations}
        onLoadObservations={onLoadNetworkObservations}
        onLoadOspfRecommendations={onLoadOspfRecommendations}
        onLoadOspfUpdatePlans={onLoadOspfUpdatePlans}
        onLoadOutputs={onLoadOutputs}
        onLoadTrends={onLoadNetworkTrends}
        onOpenGraph={() => onSelectSubpage("graph")}
        onOpenJobDetails={onOpenJobDetails}
        onOpenOspfApprovals={() => onSelectSubpage("ospf")}
        onOpenTests={() => onSelectSubpage("tests")}
        onOpenTunnelPlans={() => onSelectSubpage("tunnel_plans")}
        ospfRecommendations={ospfRecommendations}
        ospfUpdatePlans={ospfUpdatePlans}
        trends={networkTrends}
      />
    );
  }

  if (activeSubpage === "overview") {
    return (
      <NetworkOverview
        error={error}
        loading={loading}
        onCreate={onOpenCreateTunnelPlan}
        onRefresh={async () => {
          await Promise.all([onRefresh(), onLoadTopologyGraph(), onLoadOspfUpdatePlans()]);
        }}
        onSelectSubpage={onSelectSubpage}
        ospfUpdatePlans={ospfUpdatePlans}
        telemetryTunnels={telemetryTunnels}
        topologyGraph={topologyGraph}
        tunnelPlans={tunnelPlans}
      />
    );
  }

  return (
    <TunnelPlansWorkspace
      agents={agents}
      configurationSources={configurationSources}
      configurationSourcesEvidenceState={configurationSourcesEvidenceState}
      error={error ?? adapterDefinitionsLoadError}
      initialAdapterKind={initialAdapterKind}
      initialPlanWorkflow={initialPlanWorkflow}
      loading={loading}
      onAllocateTunnelEndpoints={onAllocateTunnelEndpoints}
      onCreateTunnelPlan={onCreateTunnelPlan}
      onCreateNetworkAdapterDefinition={onCreateNetworkAdapterDefinition}
      onDeleteNetworkAdapterDefinition={onDeleteNetworkAdapterDefinition}
      onDeleteTunnelPlan={onDeleteTunnelPlan}
      onExportTunnelPlan={onExportTunnelPlan}
      onInitialPlanWorkflowConsumed={onInitialPlanWorkflowConsumed}
      onInitialAdapterKindConsumed={onInitialAdapterKindConsumed}
      onOpenAdapterDefinitions={onOpenAdapterDefinitions}
      onOpenConfigurationSources={onOpenConfigurationSources}
      onRefresh={onRefresh}
      onSetTunnelPlanEnabled={onSetTunnelPlanEnabled}
      onUpdateTunnelConnectionAssessment={onUpdateTunnelConnectionAssessment}
      onUpdateTunnelPlan={onUpdateTunnelPlan}
      onUpdateNetworkAdapterDefinition={onUpdateNetworkAdapterDefinition}
      networkAdapterDefinitions={networkAdapterDefinitions}
      topologyGraph={topologyGraph}
      tunnelPlanCorruptions={tunnelPlanCorruptions}
      tunnelPlans={tunnelPlans}
    />
  );
}

function NetworkOverview({
  error,
  loading,
  onCreate,
  onRefresh,
  onSelectSubpage,
  ospfUpdatePlans,
  telemetryTunnels,
  topologyGraph,
  tunnelPlans,
}: {
  error: string | null;
  loading: boolean;
  onCreate: () => void;
  onRefresh: () => Promise<void>;
  onSelectSubpage: (subpage: string) => void;
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  topologyGraph: TopologyGraph;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const enabled = tunnelPlans.filter((plan) => plan.enabled).length;
  const attention = topologyGraph.edges.filter(
    (edge) => edge.enabled && edge.health !== "healthy",
  ).length;
  const ospfAttention = ospfUpdatePlans.filter((plan) =>
    matchesActionableOspfStatus(plan.status),
  ).length;
  const latestEvidence = telemetryTunnels.reduce<string | null>(
    (latest, tunnel) =>
      !latest || timestampMillis(tunnel.observed_at) > timestampMillis(latest)
        ? tunnel.observed_at
        : latest,
    null,
  );
  const evidenceStale =
    !latestEvidence ||
    Date.now() - timestampMillis(latestEvidence) > 15 * 60 * 1000;
  return (
    <div className="topologyPageStack">
      <section className="fleetPanel networkOverviewHeader">
        <div className="sectionHeader">
          <div>
            <h2>Network posture</h2>
            <span>Declared tunnel state, runtime evidence, and routing control</span>
          </div>
          <div className="headerActionStack">
            <button className="secondaryAction" disabled={loading} onClick={() => void onRefresh()} type="button">
              <RefreshCcw size={16} />
              Refresh
            </button>
            <button className="primaryAction" onClick={onCreate} type="button">
              <CirclePlus size={17} />
              Create plan
            </button>
          </div>
        </div>
        <ActionFeedback className="localActionFeedback" message={error} tone="danger" />
        <div
          aria-label="Network posture summary"
          className="networkMetricStrip"
        >
          <Metric label="Plans" value={tunnelPlans.length} />
          <Metric label="Enabled" value={enabled} />
          <Metric label="Runtime attention" tone={attention > 0 ? "warning" : "normal"} value={attention} />
          <Metric label="Declared observations" value={telemetryTunnels.length} />
          <Metric
            label="Latest evidence"
            title={latestEvidence ?? "No declared tunnel evidence"}
            tone={evidenceStale ? "warning" : "normal"}
            value={latestEvidence ? (evidenceStale ? "Stale" : "Current") : "None"}
          />
          <Metric label="OSPF attention" tone={ospfAttention > 0 ? "warning" : "normal"} value={ospfAttention} />
        </div>
      </section>
      <section className="fleetPanel networkOverviewActions">
        <div className="sectionHeader">
          <div>
            <h2>Workflows</h2>
            <span>Open the evidence or control surface needed for the current task</span>
          </div>
        </div>
        <div
          aria-label="Network overview workflow links"
          className="networkWorkflowList"
        >
          <WorkflowLink icon={<Network size={18} />} label="Tunnel plans" detail="Declare ownership, endpoints, addresses, and optional OSPF" onClick={() => onSelectSubpage("tunnel_plans")} />
          <WorkflowLink icon={<Repeat2 size={18} />} label="Port forwarding" detail="Manage per-VPS owned DNAT rules and inspect apply state" onClick={() => onSelectSubpage("port_forwards")} />
          <WorkflowLink icon={<GitGraph size={18} />} label="Graph" detail="Inspect declared relationships and runtime drift" onClick={() => onSelectSubpage("graph")} />
          <WorkflowLink icon={<Activity size={18} />} label="Tests" detail="Run status, probe, and bounded throughput jobs" onClick={() => onSelectSubpage("tests")} />
          <WorkflowLink icon={<Gauge size={18} />} label="OSPF" detail="Check bound adapters and control reviewed or automatic cost updates" onClick={() => onSelectSubpage("ospf")} />
          <WorkflowLink icon={<Route size={18} />} label="Evidence" detail="Review persisted observations, trends, and jobs" onClick={() => onSelectSubpage("evidence")} />
        </div>
      </section>
    </div>
  );
}

function TunnelPlansWorkspace({
  agents,
  configurationSources,
  configurationSourcesEvidenceState,
  error,
  initialAdapterKind,
  initialPlanWorkflow,
  loading,
  onAllocateTunnelEndpoints,
  onCreateTunnelPlan,
  onCreateNetworkAdapterDefinition,
  onDeleteNetworkAdapterDefinition,
  onDeleteTunnelPlan,
  onExportTunnelPlan,
  onInitialPlanWorkflowConsumed,
  onInitialAdapterKindConsumed,
  onOpenAdapterDefinitions,
  onOpenConfigurationSources,
  onRefresh,
  onSetTunnelPlanEnabled,
  onUpdateTunnelConnectionAssessment,
  onUpdateTunnelPlan,
  onUpdateNetworkAdapterDefinition,
  networkAdapterDefinitions,
  topologyGraph,
  tunnelPlanCorruptions,
  tunnelPlans,
}: {
  agents: AgentView[];
  configurationSources: ConfigurationSourceView[];
  configurationSourcesEvidenceState: "available" | "loading" | "unavailable";
  error: string | null;
  initialAdapterKind: NetworkAdapterKind | null;
  initialPlanWorkflow: "create" | null;
  loading: boolean;
  onAllocateTunnelEndpoints: (
    request: AllocateTunnelEndpointsRequest,
  ) => Promise<AllocateTunnelEndpointsResponse>;
  onCreateTunnelPlan: (request: CreateTunnelPlanRequest) => Promise<TunnelPlanMutationResponse>;
  onCreateNetworkAdapterDefinition: (
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  onDeleteNetworkAdapterDefinition: (definitionId: string) => Promise<void>;
  onDeleteTunnelPlan: (target: TunnelPlanRevisionTarget) => Promise<TunnelPlanMutationResponse>;
  onExportTunnelPlan: (planId: string) => Promise<TunnelPlan>;
  onInitialPlanWorkflowConsumed: () => void;
  onInitialAdapterKindConsumed: () => void;
  onOpenAdapterDefinitions: (domain: NetworkAdapterKind) => void;
  onOpenConfigurationSources: () => void;
  onRefresh: () => Promise<void>;
  onSetTunnelPlanEnabled: (targets: TunnelPlanRevisionTarget[], enabled: boolean) => Promise<TunnelPlanMutationResponse[]>;
  onUpdateTunnelConnectionAssessment: (planId: string, request: UpdateTunnelConnectionAssessmentRequest) => Promise<void>;
  onUpdateTunnelPlan: (planId: string, request: UpdateTunnelPlanRequest) => Promise<TunnelPlanMutationResponse>;
  onUpdateNetworkAdapterDefinition: (
    definitionId: string,
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  networkAdapterDefinitions: NetworkAdapterDefinitionRecord[];
  topologyGraph: TopologyGraph;
  tunnelPlanCorruptions: import("../types").TunnelPlanCorruptRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const [query, setQuery] = useState("");
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [createOpen, setCreateOpen] = useState(initialPlanWorkflow === "create");
  const [editingPlan, setEditingPlan] = useState<TunnelPlanRecord | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [lifecycleSnapshot, setLifecycleSnapshot] =
    useState<LifecycleSnapshot | null>(null);
  const [deleteSnapshot, setDeleteSnapshot] = useState<DeleteSnapshot | null>(null);
  const [pending, setPending] = useState(false);
  const createRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLElement | null>(null);
  const planFeedbackRef = useRef<HTMLDivElement | null>(null);
  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return tunnelPlans;
    return tunnelPlans.filter((plan) =>
      [
        plan.name,
        plan.kind,
        plan.plan.interface_name,
        plan.left_client_id,
        plan.right_client_id,
        runtimeManagerLabel(plan.plan.runtime_control?.manager),
      ]
        .join(" ")
        .toLowerCase()
        .includes(normalized),
    );
  }, [query, tunnelPlans]);

  useEffect(() => {
    if (initialPlanWorkflow !== "create") return;
    setEditingPlan(null);
    setCreateOpen(true);
    onInitialPlanWorkflowConsumed();
  }, [initialPlanWorkflow, onInitialPlanWorkflowConsumed]);

  useEffect(() => {
    if (!createOpen) return;
    window.setTimeout(() => {
      createRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
      const preferredControl = createRef.current?.querySelector<HTMLElement>(
        'input:not([type="hidden"]):not([disabled]):not([readonly]), select:not([disabled]), textarea:not([disabled])',
      );
      const closeControl = createRef.current?.querySelector<HTMLElement>(
        '[aria-label="Close tunnel plan editor"]',
      );
      (preferredControl ?? closeControl)?.focus({ preventScroll: true });
    }, 0);
  }, [createOpen, editingPlan?.id]);

  useEffect(() => {
    if (!feedback && !error) return;
    const frame = window.requestAnimationFrame(() => {
      if (planFeedbackRef.current) {
        scrollIntoViewWithMotion(planFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [error, feedback]);

  function requestLifecycle(
    ids: string[],
    enabled: boolean,
    retryCleanup = false,
    retryApply = false,
  ) {
    const targets = [...new Set(ids)]
      .map((id) => tunnelPlans.find((plan) => plan.id === id))
      .filter(
        (plan): plan is TunnelPlanRecord =>
          Boolean(plan)
            && (plan?.enabled !== enabled
              || (retryCleanup && !enabled && plan?.enabled === false)
              || (retryApply && enabled && plan?.enabled === true)),
      )
      .map((plan) => ({
        declaration: `${runtimeManagerLabel(plan.plan.runtime_control?.manager)} · ${formatTunnelKind(plan.kind)} · ${plan.plan.interface_name} · ${plan.plan.bandwidth_mbps} Mbps`,
        expected_revision: plan.revision,
        name: plan.name,
        ospfEnabled: Boolean(plan.plan.ospf),
        plan_id: plan.id,
      }));
    if (targets.length === 0) return;
    setLifecycleSnapshot({
      enabled,
      retryApply,
      retryCleanup,
      targets,
    });
  }

  async function applyLifecycle(snapshot: LifecycleSnapshot) {
    setPending(true);
    setFeedback(null);
    try {
      const responses = await onSetTunnelPlanEnabled(snapshot.targets, snapshot.enabled);
      setLifecycleSnapshot(null);
      setSelected(new Set());
      setFeedback(tunnelDispatchFeedback(
        responses.flatMap((response) => response.sync),
        snapshot.retryCleanup
          ? `Cleanup requested for ${snapshot.targets.length} tunnel plan${snapshot.targets.length === 1 ? "" : "s"}`
          : snapshot.retryApply
            ? `Runtime reapply requested for ${snapshot.targets.length} tunnel plan${snapshot.targets.length === 1 ? "" : "s"}`
          : `${snapshot.targets.length} tunnel plan${snapshot.targets.length === 1 ? "" : "s"} ${snapshot.enabled ? "enabled" : "disabled"}`,
      ));
    } catch (actionError) {
      setLifecycleSnapshot(null);
      setFeedback({
        message: actionError instanceof Error ? actionError.message : "Tunnel plan update failed",
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  async function applyDelete(snapshot: DeleteSnapshot) {
    setPending(true);
    setFeedback(null);
    try {
      const response = await onDeleteTunnelPlan(snapshot.target);
      setDeleteSnapshot(null);
      setSelected((current) => {
        const next = new Set(current);
        next.delete(snapshot.target.plan_id);
        return next;
      });
      if (editingPlan?.id === snapshot.target.plan_id) {
        setCreateOpen(false);
        setEditingPlan(null);
      }
      setExpandedId((current) => current === snapshot.target.plan_id ? null : current);
      setFeedback(tunnelRetirementFeedback(
        response.sync,
        `Deleted tunnel plan ${snapshot.plan.name}`,
      ));
    } catch (actionError) {
      setDeleteSnapshot(null);
      setFeedback({
        message: tunnelPlanDeleteError(actionError),
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  async function exportPlan(plan: TunnelPlanRecord) {
    setFeedback(null);
    try {
      const exported = await onExportTunnelPlan(plan.id);
      const blob = new Blob([JSON.stringify(exported, null, 2)], {
        type: "application/json",
      });
      const href = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = href;
      anchor.download = `${safeFilename(plan.name)}.json`;
      anchor.click();
      URL.revokeObjectURL(href);
      setFeedback({ message: `Exported ${plan.name}`, tone: "success" });
    } catch (actionError) {
      setFeedback({
        message: actionError instanceof Error ? actionError.message : "Tunnel plan export failed",
        tone: "danger",
      });
    }
  }

  const selectedRows = tunnelPlans.filter((plan) => selected.has(plan.id));
  const selectedDisabledRows = selectedRows.filter((plan) => !plan.enabled);
  const selectedEnabledRows = selectedRows.filter((plan) => plan.enabled);
  const selectedRow = selectedRows.length === 1 ? selectedRows[0] : null;
  const selectedCleanupRetryNeeded = Boolean(
    selectedRow
      && !selectedRow.enabled
      && [
        selectedRow.left_runtime_config.status,
        selectedRow.right_runtime_config.status,
      ].some((status) => !["removed", "not_dispatched"].includes(status)),
  );
  const selectedApplyRetryNeeded = Boolean(
    selectedRow
      && selectedRow.enabled
      && [
        selectedRow.left_runtime_config.status,
        selectedRow.right_runtime_config.status,
      ].some((status) =>
        ["failed", "not_applied", "not_dispatched", "stale_pending"].includes(
          status,
        ),
      ),
  );
  const editorOpen = createOpen;
  function tunnelPlanRowActions(
    plan: TunnelPlanRecord,
  ): ConsoleMenuAction[] {
    const cleanupRetryNeeded =
      !plan.enabled &&
      [plan.left_runtime_config.status, plan.right_runtime_config.status].some(
        (status) => !["removed", "not_dispatched"].includes(status),
      );
    const applyRetryNeeded =
      plan.enabled &&
      [plan.left_runtime_config.status, plan.right_runtime_config.status].some(
        (status) =>
          ["failed", "not_applied", "not_dispatched", "stale_pending"].includes(
            status,
          ),
      );
    return [
      {
        disabled: editorOpen,
        label: "Edit",
        onSelect: () => {
          setEditingPlan(plan);
          setCreateOpen(true);
        },
        title: editorOpen ? "Close the current tunnel plan editor first" : undefined,
      },
      {
        label: "Export",
        onSelect: () => void exportPlan(plan),
      },
      {
        label: plan.enabled ? "Disable" : "Enable",
        onSelect: () => requestLifecycle([plan.id], !plan.enabled),
      },
      ...(cleanupRetryNeeded || applyRetryNeeded
        ? [
            {
              label: "Retry runtime",
              onSelect: () =>
                requestLifecycle(
                  [plan.id],
                  plan.enabled,
                  cleanupRetryNeeded,
                  applyRetryNeeded,
                ),
            } satisfies ConsoleMenuAction,
          ]
        : []),
      {
        label: "Delete",
        onSelect: () =>
          setDeleteSnapshot({
            plan,
            target: {
              expected_revision: plan.revision,
              plan_id: plan.id,
            },
          }),
        title: "Delete this plan and queue runtime removal",
        tone: "danger",
      },
    ];
  }
  const lifecycleIncludesOspf = Boolean(
    lifecycleSnapshot?.targets.some((target) => target.ospfEnabled),
  );
  const runtimeEdgeByPlan = new Map(
    topologyGraph.edges.map((edge) => [edge.plan_id, edge]),
  );
  const agentNameById = clientDisplayNameMap(agents);
  return (
    <div className="topologyPageStack">
      <section className="fleetPanel tunnelPlanRegistry" ref={listRef}>
        <div className="sectionHeader">
          <div>
            <h2>Tunnel plans</h2>
            <span>
              {tunnelPlans.length} declared point-to-point tunnel
              {tunnelPlans.length === 1 ? "" : "s"}
            </span>
          </div>
          <div className="headerActionStack">
            <button className="secondaryAction" disabled={loading} onClick={() => void onRefresh()} type="button">
              <RefreshCcw size={16} />
              Refresh
            </button>
            <button
              className="primaryAction"
              disabled={editorOpen}
              onClick={() => { setEditingPlan(null); setCreateOpen(true); }}
              title={editorOpen ? "Close the current tunnel plan editor first" : "Create a tunnel plan"}
              type="button"
            >
              <CirclePlus size={17} />
              Create plan
            </button>
          </div>
        </div>
        {tunnelPlanCorruptions.length > 0 && (
          <div className="portForwardRemovalNotice" role="alert">
            <ShieldAlert size={17} />
            <div>
              <strong>
                {tunnelPlanCorruptions.length} persisted tunnel plan
                {tunnelPlanCorruptions.length === 1 ? "" : "s"} need repair
              </strong>
              {tunnelPlanCorruptions.map((plan) => (
                <div key={plan.id} title={plan.configuration_error}>
                  {plan.name} · {plan.left_client_id} / {plan.right_client_id} ·
                  revision {plan.revision}: {plan.configuration_error}
                </div>
              ))}
            </div>
          </div>
        )}
        <ActionFeedback className="localActionFeedback topologyPlanActionFeedback" message={error ?? feedback?.message} ref={planFeedbackRef} tone={error ? "danger" : feedback?.tone} />
        <div className="tunnelRegistryToolbar">
          <label className="searchControl compactSearch">
            <Search size={15} />
            <input aria-label="Search tunnel plans" onChange={(event) => setQuery(event.target.value)} placeholder="Search plans" value={query} />
          </label>
          {selectedRows.length > 0 && (
            <div className="selectionActionBar" aria-label="Selected tunnel plan actions">
              <span>{selectedRows.length} selected</span>
              <ConsoleActionMenu
                actions={[
                  {
                    disabled: !selectedRow || editorOpen,
                    label: "Edit",
                    onSelect: () => {
                      if (selectedRow) {
                        setEditingPlan(selectedRow);
                        setCreateOpen(true);
                      }
                    },
                  },
                  {
                    disabled: !selectedRow,
                    label: "Export",
                    onSelect: () => {
                      if (selectedRow) void exportPlan(selectedRow);
                    },
                  },
                  {
                    disabled: selectedDisabledRows.length === 0,
                    label: "Enable",
                    onSelect: () =>
                      requestLifecycle(
                        selectedDisabledRows.map((plan) => plan.id),
                        true,
                      ),
                    title:
                      selectedDisabledRows.length === 0
                        ? "All selected plans are already enabled"
                        : undefined,
                  },
                  {
                    disabled: selectedEnabledRows.length === 0,
                    label: "Disable",
                    onSelect: () =>
                      requestLifecycle(
                        selectedEnabledRows.map((plan) => plan.id),
                        false,
                      ),
                    title:
                      selectedEnabledRows.length === 0
                        ? "All selected plans are already disabled"
                        : undefined,
                  },
                  ...(selectedRow &&
                  (selectedCleanupRetryNeeded || selectedApplyRetryNeeded)
                    ? [
                        {
                          label: "Retry runtime",
                          onSelect: () =>
                            requestLifecycle(
                              [selectedRow.id],
                              selectedRow.enabled,
                              selectedCleanupRetryNeeded,
                              selectedApplyRetryNeeded,
                            ),
                        },
                      ]
                    : []),
                  {
                    disabled: !selectedRow,
                    label: "Delete",
                    onSelect: () => {
                      if (selectedRow) {
                        setDeleteSnapshot({
                          plan: selectedRow,
                          target: {
                            expected_revision: selectedRow.revision,
                            plan_id: selectedRow.id,
                          },
                        });
                      }
                    },
                    tone: "danger",
                    title: "Delete this plan and queue runtime removal",
                  },
                ]}
                label={`Actions for ${selectedRows.length} selected tunnel plan${selectedRows.length === 1 ? "" : "s"}`}
              />
            </div>
          )}
        </div>
        {filtered.length === 0 ? (
          <div className="emptyState compactEmptyState">
            <strong>{tunnelPlans.length === 0 ? "No tunnel plans" : "No matching plans"}</strong>
            <span>{tunnelPlans.length === 0 ? "Create an explicit plan before vpsman observes or manages a tunnel." : "Adjust the plan search."}</span>
          </div>
        ) : (
          <div className="tunnelPlanTableWrap">
            <table aria-label="Tunnel plans" className="tunnelPlanTable">
              <thead>
                <tr>
                  <th className="selectionCell">
                    <input
                      aria-label="Select visible tunnel plans"
                      checked={filtered.length > 0 && filtered.every((plan) => selected.has(plan.id))}
                      onChange={(event) => {
                        const next = new Set(selected);
                        for (const plan of filtered) {
                          if (event.target.checked) next.add(plan.id);
                          else next.delete(plan.id);
                        }
                        setSelected(next);
                      }}
                      type="checkbox"
                    />
                  </th>
                  <th>Plan</th>
                  <th>Endpoints</th>
                  <th>Runtime owner</th>
                  <th>Runtime</th>
                  <th>Connectivity</th>
                  <th>OSPF</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((plan) => {
                  const expanded = expandedId === plan.id;
                  return (
                    <TunnelPlanRows
                      expanded={expanded}
                      agentNameById={agentNameById}
                      key={plan.id}
                      onUpdateConnectionAssessment={onUpdateTunnelConnectionAssessment}
                      contextActions={tunnelPlanRowActions(plan)}
                      onSelect={(checked) => {
                        const next = new Set(selected);
                        if (checked) next.add(plan.id);
                        else next.delete(plan.id);
                        setSelected(next);
                      }}
                      onToggle={() => setExpandedId((current) => current === plan.id ? null : plan.id)}
                      plan={plan}
                      runtimeEdge={runtimeEdgeByPlan.get(plan.id)}
                      selected={selected.has(plan.id)}
                    />
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
      {createOpen && (
        <div ref={createRef}>
          <TunnelPlanComposer
            agents={agents}
            configurationSources={configurationSources}
            configurationSourcesEvidenceState={
              configurationSourcesEvidenceState
            }
            existingPlans={tunnelPlans}
            initialPlan={editingPlan}
            key={editingPlan?.id ?? "new-tunnel-plan"}
            onAllocateTunnelEndpoints={onAllocateTunnelEndpoints}
            onClose={() => { setCreateOpen(false); setEditingPlan(null); }}
            onSaveTunnelPlan={async (request) => {
              let response: TunnelPlanMutationResponse;
              if (editingPlan) {
                response = await onUpdateTunnelPlan(editingPlan.id, {
                  ...request,
                  expected_revision: editingPlan.revision,
                });
              } else {
                response = await onCreateTunnelPlan(request);
              }
              setCreateOpen(false);
              setEditingPlan(null);
              setFeedback(tunnelDispatchFeedback(
                response.sync,
                `${editingPlan ? "Updated" : "Created"} tunnel plan ${request.name}`,
              ));
              window.setTimeout(() => listRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 0);
            }}
            onOpenAdapterDefinitions={onOpenAdapterDefinitions}
            onOpenConfigurationSources={onOpenConfigurationSources}
            networkAdapterDefinitions={networkAdapterDefinitions}
          />
        </div>
      )}
      <NetworkAdapterDefinitionsPanel
        definitions={networkAdapterDefinitions}
        initialKind={initialAdapterKind}
        onCreate={onCreateNetworkAdapterDefinition}
        onDelete={onDeleteNetworkAdapterDefinition}
        onInitialKindConsumed={onInitialAdapterKindConsumed}
        onUpdate={onUpdateNetworkAdapterDefinition}
        tunnelPlans={tunnelPlans}
      />
      <ConfirmationPrompt
        confirmLabel={lifecycleSnapshot?.retryCleanup ? "Retry removal" : lifecycleSnapshot?.retryApply ? "Retry apply" : lifecycleSnapshot?.enabled ? "Enable plans" : "Disable plans"}
        detail={lifecycleSnapshot?.retryCleanup
          ? "Push the current disabled desired state to both endpoints again. The plan remains disabled; deletion remains available and retires the declaration immediately."
          : lifecycleSnapshot?.retryApply
            ? "Push the current enabled declaration to both endpoints again without changing its revision or runtime ownership."
          : lifecycleSnapshot?.enabled
          ? `Enable these declared plans and push their exact desired state to both endpoints.${lifecycleIncludesOspf ? " OSPF control resumes as unverified; existing external daemon costs remain unchanged until a verified update." : ""}`
          : `Disable these plans and push runtime config that removes their managed state from both endpoints. External observed plans are no longer observed.${lifecycleIncludesOspf ? " OSPF control stops; existing external daemon costs are not reverted." : ""}`}
        items={lifecycleSnapshot ? [
          { label: "Action", value: lifecycleSnapshot.retryCleanup ? "Retry runtime removal" : lifecycleSnapshot.retryApply ? "Retry runtime apply" : lifecycleSnapshot.enabled ? "Enable" : "Disable" },
          { label: "Plans", value: lifecycleSnapshot.targets.map((target) => `${target.name} (r${target.expected_revision})`).join(", ") },
          { label: "Declared state", value: lifecycleSnapshot.targets.map((target) => `${target.name}: ${target.declaration}`).join("; ") },
          { label: "Endpoints", value: `${lifecycleSnapshot.targets.length * 2} endpoint configurations` },
          ...(lifecycleIncludesOspf ? [{
            label: "Routing cost",
            value: lifecycleSnapshot.enabled
              ? "Resume control unverified; keep current external values until verification"
              : "Stop control; keep current external values",
          }] : []),
        ] : []}
        onCancel={() => setLifecycleSnapshot(null)}
        onConfirm={() => lifecycleSnapshot && void applyLifecycle(lifecycleSnapshot)}
        open={lifecycleSnapshot !== null}
        pending={pending}
        title={lifecycleSnapshot?.retryCleanup ? "Confirm tunnel cleanup retry" : lifecycleSnapshot?.retryApply ? "Confirm tunnel runtime reapply" : lifecycleSnapshot?.enabled ? "Confirm tunnel plan enable" : "Confirm tunnel plan disable"}
        tone={lifecycleSnapshot?.enabled ? "normal" : "danger"}
      />
      <ConfirmationPrompt
        confirmLabel="Delete plan"
        detail="Retire this declaration immediately. It is removed from desired state now; both endpoints receive runtime removal jobs, and offline agents reconcile it on reconnect. Reservations become available immediately and audit history remains."
        items={deleteSnapshot ? [
          { label: "Plan", value: `${deleteSnapshot.plan.name} (r${deleteSnapshot.target.expected_revision})` },
          { label: "Endpoints", value: `${deleteSnapshot.plan.left_client_id} / ${deleteSnapshot.plan.right_client_id}` },
          { label: "Current state", value: deleteSnapshot.plan.enabled ? "Enabled" : "Disabled" },
          { label: "Runtime effect", value: "Queue removal on both endpoints" },
          { label: "Current evidence", value: `Left ${tunnelRuntimeConfigLabel(deleteSnapshot.plan.left_runtime_config)}; right ${tunnelRuntimeConfigLabel(deleteSnapshot.plan.right_runtime_config)}` },
          ...(deleteSnapshot.plan.plan.ospf ? [{
            label: "Routing cost",
            value: "Stop control; keep daemon cost",
          }] : []),
        ] : []}
        onCancel={() => setDeleteSnapshot(null)}
        onConfirm={() => deleteSnapshot && void applyDelete(deleteSnapshot)}
        open={deleteSnapshot !== null}
        pending={pending}
        title="Confirm tunnel plan deletion"
        tone="danger"
      />
    </div>
  );
}

function TunnelPlanRows({
  agentNameById,
  contextActions,
  expanded,
  onUpdateConnectionAssessment,
  onSelect,
  onToggle,
  plan,
  runtimeEdge,
  selected,
}: {
  agentNameById: Map<string, string>;
  contextActions: ConsoleMenuAction[];
  expanded: boolean;
  onUpdateConnectionAssessment: (planId: string, request: UpdateTunnelConnectionAssessmentRequest) => Promise<void>;
  onSelect: (checked: boolean) => void;
  onToggle: () => void;
  plan: TunnelPlanRecord;
  runtimeEdge?: TopologyGraphEdge;
  selected: boolean;
}) {
  const runtime = plan.plan.runtime_control?.manager ?? "agent_iproute2_managed";
  const leftRuntimeState = tunnelEndpointDisplayState(
    plan.enabled,
    plan.left_runtime_config,
    runtimeEdge?.left_runtime_state,
  );
  const rightRuntimeState = tunnelEndpointDisplayState(
    plan.enabled,
    plan.right_runtime_config,
    runtimeEdge?.right_runtime_state,
  );
  const connectivity = tunnelConnectivityPresentation(plan, runtimeEdge);
  const leftClientName = clientDisplayNameFromMap(
    plan.left_client_id,
    agentNameById,
  );
  const rightClientName = clientDisplayNameFromMap(
    plan.right_client_id,
    agentNameById,
  );
  const [assessment, setAssessment] = useState<TunnelConnectionAssessment>(
    plan.connection_assessment ?? "automatic",
  );
  const [assessmentNote, setAssessmentNote] = useState(
    plan.connection_assessment_note ?? "",
  );
  const [assessmentPending, setAssessmentPending] = useState(false);
  const [assessmentFeedback, setAssessmentFeedback] = useState<Feedback | null>(null);
  useEffect(() => {
    setAssessment(plan.connection_assessment ?? "automatic");
    setAssessmentNote(plan.connection_assessment_note ?? "");
    setAssessmentFeedback(null);
  }, [plan.connection_assessment, plan.connection_assessment_note, plan.revision]);
  const assessmentChanged = assessment !== (plan.connection_assessment ?? "automatic")
    || (assessment === "automatic" ? "" : assessmentNote.trim())
      !== (plan.connection_assessment === "automatic" ? "" : (plan.connection_assessment_note ?? ""));
  const assessmentValid = assessment === "automatic"
    || (plan.enabled && assessmentNote.trim().length > 0 && assessmentNote.trim().length <= 500);

  async function saveConnectionAssessment(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    event.stopPropagation();
    if (!assessmentChanged || !assessmentValid) return;
    setAssessmentPending(true);
    setAssessmentFeedback(null);
    try {
      await onUpdateConnectionAssessment(plan.id, {
        assessment,
        expected_revision: plan.revision,
        note: assessment === "automatic" ? null : assessmentNote.trim(),
      });
      setAssessmentFeedback({
        message: assessment === "automatic"
          ? "Operator assessment cleared; measured evidence is authoritative"
          : `Recorded ${assessment} as an operator assessment`,
        tone: "success",
      });
    } catch (error) {
      setAssessmentFeedback({
        message: tunnelConnectionAssessmentError(error),
        tone: "danger",
      });
    } finally {
      setAssessmentPending(false);
    }
  }
  return (
    <>
      <ConsoleContextActionMenu
        actions={contextActions}
        label={`Actions for ${plan.name}`}
      >
        <tr aria-expanded={expanded} className={`${expanded ? "isExpanded" : ""} ${selected ? "isSelected" : ""}`} onClick={onToggle} tabIndex={0} onKeyDown={(event) => {
          if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            onToggle();
          }
        }}>
          <td className="selectionCell" onClick={(event) => event.stopPropagation()}>
            <input aria-label={`Select ${plan.name}`} checked={selected} onChange={(event) => onSelect(event.target.checked)} type="checkbox" />
          </td>
          <td>
            <span className="historyPrimary">
              <strong title={plan.name}>{plan.name}</strong>
              <small title={`${plan.kind} · ${plan.plan.interface_name}`}>{formatTunnelKind(plan.kind)} · {plan.plan.interface_name}</small>
            </span>
            <div
              className="topologyMobileRowActions"
              onClick={(event) => event.stopPropagation()}
            >
              <ConsoleInlineActions
                actions={contextActions}
                label={`Actions for ${plan.name}`}
              />
            </div>
          </td>
          <td>
            <span className="historyPrimary">
              <strong title={`${leftClientName}; full ID ${plan.left_client_id}`}>
                {leftClientName}
              </strong>
              <small title={`${rightClientName}; full ID ${plan.right_client_id}`}>
                {rightClientName}
              </small>
            </span>
          </td>
          <td>
            <span className="historyPrimary">
              <strong title={runtimeManagerLabel(runtime)}>{runtimeManagerLabel(runtime)}</strong>
              <small title={plan.enabled ? "Enabled" : "Disabled"}>{plan.enabled ? "Enabled" : "Disabled"}</small>
            </span>
          </td>
          <td>
            <span className="endpointStatusPair">
              <span className={`status ${tunnelEndpointRuntimeStateBadgeClass(leftRuntimeState)}`} title={tunnelEndpointStateTitle("Left", plan.left_runtime_config, runtimeEdge?.left_runtime_reason, runtimeEdge?.left_observed_at)}>L {readableTelemetryToken(leftRuntimeState)}</span>
              <span className={`status ${tunnelEndpointRuntimeStateBadgeClass(rightRuntimeState)}`} title={tunnelEndpointStateTitle("Right", plan.right_runtime_config, runtimeEdge?.right_runtime_reason, runtimeEdge?.right_observed_at)}>R {readableTelemetryToken(rightRuntimeState)}</span>
            </span>
          </td>
          <td>
            <span className="historyPrimary tunnelConnectivitySummary">
              <strong className={`status ${connectivity.statusClass}`} title={connectivity.title}>{connectivity.label}</strong>
              <small title={connectivity.detail}>{connectivity.detail}</small>
            </span>
          </td>
          <td>
            <span className="historyPrimary">
              <strong title={plan.plan.ospf ? `${plan.recommended_ospf_cost ?? "unknown"} cost` : "OSPF off"}>{plan.plan.ospf ? `${plan.recommended_ospf_cost ?? "?"} cost` : "Off"}</strong>
              <small title={plan.plan.ospf ? `${formatOspfMode(plan.plan.ospf.mode)} · ${readableTelemetryToken(plan.ospf_status)}` : "Tunnel only"}>{plan.plan.ospf ? `${formatOspfMode(plan.plan.ospf.mode)} · ${readableTelemetryToken(plan.ospf_status)}` : "Tunnel only"}</small>
            </span>
          </td>
        </tr>
      </ConsoleContextActionMenu>
      {expanded && (
        <tr className="tunnelPlanDetailRow">
          <td colSpan={7}>
            <div className="tunnelPlanDetail">
              <button aria-label={`Close details for ${plan.name}`} className="iconAction topologyDetailClose" onClick={onToggle} title="Close details" type="button"><X size={15} /></button>
              <div className="tunnelPlanFacts">
                <PlanFact label="Desired state" value={plan.enabled ? "Present on both endpoints" : "Absent from both endpoints"} />
                <PlanFact
                  label="Endpoints"
                  title={`${plan.left_client_id} / ${plan.right_client_id}`}
                  value={`${leftClientName} / ${rightClientName}`}
                />
                <PlanFact label="Apply state" value={`L ${readableTelemetryToken(leftRuntimeState)} · R ${readableTelemetryToken(rightRuntimeState)}`} />
                <PlanFact label="Connectivity" value={`${connectivity.label} · ${connectivity.detail}`} />
                <PlanFact label="Left outer path" value={formatEndpointUnderlay(plan.plan.left_local_underlay, plan.plan.left_remote_underlay)} />
                <PlanFact label="Right outer path" value={formatEndpointUnderlay(plan.plan.right_local_underlay, plan.plan.right_remote_underlay)} />
                <PlanFact label="Tunnel addresses" value={formatTunnelAddresses(plan)} />
                <PlanFact label="Bandwidth" value={`${plan.plan.bandwidth_mbps} Mbps`} />
                <PlanFact label="Runtime ownership" value={formatRuntimeBinding(plan)} />
                <PlanFact label="OSPF control" value={formatPlanOspf(plan)} />
              </div>
              <form className="tunnelConnectionAssessment" onClick={(event) => event.stopPropagation()} onSubmit={(event) => void saveConnectionAssessment(event)}>
                <div className="tunnelConnectionAssessmentHeading">
                  <strong>Operator connectivity assessment</strong>
                  <span title="Use only when endpoint probes cannot establish reachability.">Display-only annotation; runtime and automatic OSPF stay machine-derived.</span>
                </div>
                <label>
                  <span>Assessment</span>
                  <select
                    aria-label={`Connectivity assessment for ${plan.name}`}
                    disabled={assessmentPending || !plan.enabled}
                    onChange={(event) => {
                      setAssessment(event.target.value as TunnelConnectionAssessment);
                      setAssessmentFeedback(null);
                    }}
                    value={assessment}
                  >
                    <option value="automatic">Automatic (measured)</option>
                    <option value="connected">Connected (operator)</option>
                    <option value="disconnected">Disconnected (operator)</option>
                  </select>
                </label>
                {assessment !== "automatic" && (
                  <label>
                    <span>Evidence note</span>
                    <input
                      aria-label={`Connectivity assessment note for ${plan.name}`}
                      disabled={assessmentPending || !plan.enabled}
                      maxLength={500}
                      onChange={(event) => {
                        setAssessmentNote(event.target.value);
                        setAssessmentFeedback(null);
                      }}
                      placeholder="e.g. application traffic verified; ICMP blocked"
                      title={assessmentNote || "Explain the operator evidence for this assessment"}
                      value={assessmentNote}
                    />
                  </label>
                )}
                <button
                  className="secondaryAction compactAction"
                  disabled={assessmentPending || !assessmentChanged || !assessmentValid}
                  title={!plan.enabled
                    ? "Enable the plan before recording a connectivity assessment"
                    : !assessmentChanged
                      ? "No assessment changes to save"
                      : !assessmentValid
                        ? "Add an evidence note of 500 characters or fewer"
                        : "Save this audited display assessment without changing runtime or OSPF automation"}
                  type="submit"
                >
                  {assessmentPending
                    ? "Saving"
                    : assessment === "automatic" && plan.connection_assessment !== "automatic"
                      ? "Clear assessment"
                      : "Save assessment"}
                </button>
              </form>
              <ActionFeedback className="localActionFeedback" message={assessmentFeedback?.message} tone={assessmentFeedback?.tone} />
              {(plan.plan.runtime_control?.manager ?? "agent_iproute2_managed") ===
                "agent_iproute2_managed" &&
                (plan.plan.runtime_topology?.desired_interfaces?.length ?? 0) > 0 && (
                <div className="tunnelIntentLine">
                  <strong>Declared interfaces</strong>
                  <code title={plan.plan.runtime_topology?.desired_interfaces?.join(", ")}>{plan.plan.runtime_topology?.desired_interfaces?.join(", ")}</code>
                </div>
              )}
              {plan.plan.conflicts.length > 0 && (
                <ActionFeedback message={plan.plan.conflicts.join("; ")} tone="warning" />
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

function TunnelPlanComposer({
  agents,
  configurationSources,
  configurationSourcesEvidenceState,
  existingPlans,
  initialPlan,
  onAllocateTunnelEndpoints,
  onClose,
  onSaveTunnelPlan,
  onOpenAdapterDefinitions,
  onOpenConfigurationSources,
  networkAdapterDefinitions,
}: {
  agents: AgentView[];
  configurationSources: ConfigurationSourceView[];
  configurationSourcesEvidenceState: "available" | "loading" | "unavailable";
  existingPlans: TunnelPlanRecord[];
  initialPlan: TunnelPlanRecord | null;
  onAllocateTunnelEndpoints: (request: AllocateTunnelEndpointsRequest) => Promise<AllocateTunnelEndpointsResponse>;
  onClose: () => void;
  onSaveTunnelPlan: (request: CreateTunnelPlanRequest) => Promise<void>;
  onOpenAdapterDefinitions: (domain: NetworkAdapterKind) => void;
  onOpenConfigurationSources: () => void;
  networkAdapterDefinitions: NetworkAdapterDefinitionRecord[];
}) {
  const [form, setForm] = useState<TunnelPlanForm>(() =>
    initialPlan ? tunnelPlanFormFromRecord(initialPlan) : initialTunnelPlanForm(),
  );
  const [feedback, setFeedback] = useState<
    (Feedback & { location: "allocation" | "form" | "manager" }) | null
  >(null);
  const [pending, setPending] = useState(false);
  const [allocationPending, setAllocationPending] = useState(false);
  const [snapshot, setSnapshot] = useState<CreateTunnelPlanRequest | null>(null);
  const runtimeDefinitions = networkAdapterDefinitions.filter(
    (definition) => definition.adapter_kind === "runtime_tunnel",
  );
  const routingDefinitions = networkAdapterDefinitions.filter(
    (definition) => definition.adapter_kind === "routing_cost",
  );
  const leftRuntimeDefinitions = definitionsForClient(runtimeDefinitions, form.leftClientId);
  const rightRuntimeDefinitions = definitionsForClient(runtimeDefinitions, form.rightClientId);
  const leftRoutingDefinitions = definitionsForClient(routingDefinitions, form.leftClientId);
  const rightRoutingDefinitions = definitionsForClient(routingDefinitions, form.rightClientId);
  const leftOspfSource = endpointOspfConfigurationSource(
    configurationSources,
    form.leftClientId,
  );
  const rightOspfSource = endpointOspfConfigurationSource(
    configurationSources,
    form.rightClientId,
  );
  const policy = ospfPolicyFromForm(form);
  const previewCost = calculateOspfCostPreview({
    bandwidthMbps: numberOr(form.bandwidthMbps, DEFAULT_TUNNEL_BANDWIDTH_MBPS),
    latencyMs: numberOr(form.plannedLatencyMs, 20),
    packetLossRatio: numberOr(form.packetLossPercent, 0) / 100,
    policy,
    preference: numberOr(form.preference, 1),
  });
  const duplicateName = !initialPlan
    && existingPlans.some((plan) => plan.name === form.name.trim());
  const formError = validateTunnelPlanForm(form);
  const resourceConflict = formError
    ? null
    : validateExistingTunnelPlanConflicts(form, existingPlans, initialPlan?.id);
  const bindingError = validateAdapterBindings(
    form,
    leftRuntimeDefinitions,
    rightRuntimeDefinitions,
    leftRoutingDefinitions,
    rightRoutingDefinitions,
  );
  const validationError = duplicateName
    ? "A tunnel plan with this name already exists; edit it from the table"
    : formError ?? resourceConflict ?? bindingError;
  const existing = initialPlan;
  const unchanged = Boolean(
    existing
      && !validationError
      && tunnelPlanFormMatchesInitial(form, existing),
  );

  function update<K extends keyof TunnelPlanForm>(key: K, value: TunnelPlanForm[K]) {
    setSnapshot(null);
    setFeedback(null);
    setForm((current) => ({ ...current, [key]: value }));
  }

  function changeManager(manager: RuntimeTunnelManager) {
    if (manager === "agent_iproute2_managed" && !AGENT_TUNNEL_KINDS.includes(form.kind)) {
      setFeedback({
        message: `${formatTunnelKind(form.kind)} cannot be agent-managed. Select GRE, IPIP, SIT, or FOU before choosing Agent iproute2.`,
        location: "manager",
        tone: "warning",
      });
      return;
    }
    setSnapshot(null);
    setFeedback(null);
    setForm((current) => ({
      ...current,
      runtimeManager: manager,
    }));
  }

  async function allocate() {
    setAllocationPending(true);
    setFeedback(null);
    try {
      const response = await onAllocateTunnelEndpoints({
        include_ipv4: form.includeIpv4,
        include_ipv6: form.includeIpv6,
        ipv4_pool_cidr: form.ipv4Pool.trim() || null,
        ipv6_pool_cidr: form.ipv6Pool.trim() || null,
        reserved_addresses: [],
      });
      setForm((current) => applyAllocation(current, response));
      setFeedback({
        message: response.ipv4_tunnel || response.ipv6_tunnel ? "Allocated unused endpoint addresses" : "No allocation pool is configured; enter endpoint addresses manually",
        location: "allocation",
        tone: response.ipv4_tunnel || response.ipv6_tunnel ? "success" : "warning",
      });
    } catch (error) {
      setFeedback({
        location: "allocation",
        message: error instanceof Error ? error.message : "Address allocation failed",
        tone: "danger",
      });
    } finally {
      setAllocationPending(false);
    }
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (validationError) {
      setFeedback({ location: "form", message: validationError, tone: "warning" });
      return;
    }
    if (unchanged) {
      setFeedback({
        location: "form",
        message: "No tunnel plan changes to review",
        tone: "warning",
      });
      return;
    }
    try {
      setSnapshot(buildTunnelPlanRequest(form));
      setFeedback(null);
    } catch (error) {
      setFeedback({
        location: "form",
        message: error instanceof Error ? error.message : "Tunnel plan is invalid",
        tone: "danger",
      });
    }
  }

  async function confirmCreate(request: CreateTunnelPlanRequest) {
    setPending(true);
    setFeedback(null);
    try {
      await onSaveTunnelPlan(request);
      setSnapshot(null);
    } catch (error) {
      setSnapshot(null);
      setFeedback({
        location: "form",
        message: tunnelPlanSaveError(error),
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  const kindOptions = form.runtimeManager === "agent_iproute2_managed" ? AGENT_TUNNEL_KINDS : ALL_TUNNEL_KINDS;
  return (
    <section className="fleetPanel tunnelPlanComposer">
      <div className="sectionHeader tunnelPlanComposerHeader">
        <div>
          <h2>{existing ? "Update tunnel plan" : "Create tunnel plan"}</h2>
          <span>Declare one exact point-to-point tunnel; OSPF remains optional</span>
        </div>
        <button aria-label="Close tunnel plan editor" className="iconAction" onClick={onClose} title="Close editor" type="button"><X size={17} /></button>
      </div>
      <form className="tunnelPlanForm" noValidate onSubmit={submit}>
        <fieldset className="topologyFormSection">
          <legend>Plan and endpoints</legend>
          <div className="topologyFormGrid threeColumn">
            <Field label="Plan name"><input aria-label="Tunnel plan name" maxLength={128} onChange={(event) => update("name", event.target.value)} placeholder="edge-a-edge-b" readOnly={Boolean(existing)} required title={existing ? "Plan identity is fixed; create a new declaration to use another name." : undefined} value={form.name} /></Field>
            <Field label="Interface" tooltip="Exact interface name to manage or observe on both endpoints."><input aria-label="Tunnel interface" maxLength={15} onChange={(event) => update("interfaceName", event.target.value)} placeholder="tun0" required value={form.interfaceName} /></Field>
            <Field label="Kind" tooltip="Agent iproute2 supports GRE, IPIP, SIT, and FOU. WireGuard, OpenVPN, TUN/TAP, and custom interfaces require external ownership."><select aria-label="Tunnel kind" onChange={(event) => update("kind", event.target.value as TunnelKind)} value={form.kind}>{kindOptions.map((kind) => <option key={kind} value={kind}>{formatTunnelKind(kind)}</option>)}</select></Field>
          </div>
          <div className="topologyFormGrid threeColumn tunnelEndpointUnderlayGrid">
            <Field label="Left VPS"><VpsCombobox agents={agents} ariaLabel="Left tunnel VPS" excludeIds={form.rightClientId ? [form.rightClientId] : []} onChange={(value) => update("leftClientId", value)} value={form.leftClientId} /></Field>
            <Field label="Left remote destination" tooltip="Outer destination reached from the left VPS. Enter the peer's public or NAT address; vpsman never derives it from the right-side fields."><input aria-label="Left remote underlay destination" onChange={(event) => update("leftRemoteUnderlay", event.target.value)} placeholder="203.0.113.20" required value={form.leftRemoteUnderlay} /></Field>
            <Field label="Left local source" tooltip="Optional outer-packet source bound on the left VPS. It may be a private interface address behind NAT. Leave empty to let the OS route choose."><input aria-label="Left local underlay source" onChange={(event) => update("leftLocalUnderlay", event.target.value)} placeholder="Automatic" value={form.leftLocalUnderlay} /></Field>
            <Field label="Right VPS"><VpsCombobox agents={agents} ariaLabel="Right tunnel VPS" excludeIds={form.leftClientId ? [form.leftClientId] : []} onChange={(value) => update("rightClientId", value)} value={form.rightClientId} /></Field>
            <Field label="Right remote destination" tooltip="Outer destination reached from the right VPS. Enter the peer's public or NAT address; vpsman never derives it from the left-side fields."><input aria-label="Right remote underlay destination" onChange={(event) => update("rightRemoteUnderlay", event.target.value)} placeholder="198.51.100.10" required value={form.rightRemoteUnderlay} /></Field>
            <Field label="Right local source" tooltip="Optional outer-packet source bound on the right VPS. It may be a private interface address behind NAT. Leave empty to let the OS route choose."><input aria-label="Right local underlay source" onChange={(event) => update("rightLocalUnderlay", event.target.value)} placeholder="Automatic" value={form.rightLocalUnderlay} /></Field>
          </div>
        </fieldset>

        <fieldset className="topologyFormSection">
          <legend>Runtime ownership</legend>
          <div className="runtimeManagerSegments" role="radiogroup" aria-label="Tunnel runtime ownership">
            <ManagerChoice active={form.runtimeManager === "agent_iproute2_managed"} detail="vpsman creates and removes GRE, IPIP, SIT, or FOU with iproute2." label="Agent iproute2" onClick={() => changeManager("agent_iproute2_managed")} />
            <ManagerChoice active={form.runtimeManager === "external_observed"} detail="vpsman inspects this exact declared interface and never mutates it." label="External observed" onClick={() => changeManager("external_observed")} />
            <ManagerChoice active={form.runtimeManager === "external_managed_adapter"} detail="vpsman executes the selected operator-owned adapter definition." label="External adapter" onClick={() => changeManager("external_managed_adapter")} />
          </div>
          {feedback?.location === "manager" ? (
            <ActionFeedback
              className="localActionFeedback"
              message={feedback.message}
              tone={feedback.tone}
            />
          ) : null}
          {form.runtimeManager === "external_managed_adapter" && (
            <div className="topologyFormGrid twoColumn adapterBindingGrid">
              <AdapterDefinitionField clientId={form.leftClientId} domain="runtime_tunnel" label="Left runtime adapter" onChange={(value) => update("leftRuntimeDefinitionId", value)} onOpenAdapterDefinitions={onOpenAdapterDefinitions} definitions={leftRuntimeDefinitions} value={form.leftRuntimeDefinitionId} />
              <AdapterDefinitionField clientId={form.rightClientId} domain="runtime_tunnel" label="Right runtime adapter" onChange={(value) => update("rightRuntimeDefinitionId", value)} onOpenAdapterDefinitions={onOpenAdapterDefinitions} definitions={rightRuntimeDefinitions} value={form.rightRuntimeDefinitionId} />
            </div>
          )}
          {form.runtimeManager !== "external_observed" && (
            <div className="topologyFormGrid fourColumn compactNumericGrid">
              <Field label="Ingress limit" tooltip="Optional ingress limit from 64 to 1000000 Kbps."><UnitInput ariaLabel="Ingress limit" max={1_000_000} min={64} onChange={(value) => update("ingressKbps", value)} unit="Kbps" value={form.ingressKbps} /></Field>
              <Field label="Egress limit" tooltip="Optional egress limit from 64 to 1000000 Kbps."><UnitInput ariaLabel="Egress limit" max={1_000_000} min={64} onChange={(value) => update("egressKbps", value)} unit="Kbps" value={form.egressKbps} /></Field>
              <Field label="Burst" tooltip="Optional traffic shaping burst from 1 to 1048576 KiB."><UnitInput ariaLabel="Traffic burst" max={1_048_576} min={1} onChange={(value) => update("burstKb", value)} unit="KiB" value={form.burstKb} /></Field>
              <Field label="Bandwidth" tooltip="Operator-declared planning bandwidth from 10 to 10000 Mbps."><UnitInput ariaLabel="Tunnel bandwidth" max={MAX_TUNNEL_BANDWIDTH_MBPS} min={MIN_TUNNEL_BANDWIDTH_MBPS} onChange={(value) => update("bandwidthMbps", value)} required unit="Mbps" value={form.bandwidthMbps} /></Field>
            </div>
          )}
          {form.runtimeManager === "external_observed" && (
            <div className="topologyFormGrid twoColumn compactNumericGrid">
              <Field label="Bandwidth" tooltip="Operator-declared planning bandwidth from 10 to 10000 Mbps."><UnitInput ariaLabel="Tunnel bandwidth" max={MAX_TUNNEL_BANDWIDTH_MBPS} min={MIN_TUNNEL_BANDWIDTH_MBPS} onChange={(value) => update("bandwidthMbps", value)} required unit="Mbps" value={form.bandwidthMbps} /></Field>
            </div>
          )}
          {form.kind === "fou" && form.runtimeManager !== "external_observed" && (
            <div className="topologyFormGrid threeColumn compactNumericGrid">
              <Field label="FOU port" tooltip="UDP port registered locally by ip fou on each endpoint."><UnitInput ariaLabel="FOU local port" max={65535} min={1} onChange={(value) => update("fouPort", value)} unit="port" value={form.fouPort} /></Field>
              <Field label="Peer port" tooltip="Remote UDP destination port used for FOU encapsulation."><UnitInput ariaLabel="FOU peer port" max={65535} min={1} onChange={(value) => update("fouPeerPort", value)} unit="port" value={form.fouPeerPort} /></Field>
              <Field label="IP protocol" tooltip="Inner IP protocol registered with FOU; 4 is IP-in-IP and 47 is GRE."><UnitInput ariaLabel="FOU IP protocol" max={255} min={1} onChange={(value) => update("fouIpProto", value)} unit="id" value={form.fouIpProto} /></Field>
            </div>
          )}
          {form.runtimeManager === "agent_iproute2_managed" && (
            <details className="topologyAdvancedFields">
              <summary>Agent-managed routes and cleanup</summary>
              <div className="topologyFormGrid twoColumn">
                <Field label="Desired interfaces" tooltip="One exact interface per line. The plan interface is always included by the server."><textarea aria-label="Desired tunnel interfaces" onChange={(event) => update("desiredInterfaces", event.target.value)} placeholder="Optional additional exact interfaces" rows={3} value={form.desiredInterfaces} /></Field>
                <Field label="Stale interfaces" tooltip="Only interfaces explicitly listed here are eligible for cleanup."><textarea aria-label="Stale tunnel interfaces" onChange={(event) => update("staleInterfaces", event.target.value)} placeholder="One exact interface per line" rows={3} value={form.staleInterfaces} /></Field>
                <Field label="Desired routes" tooltip="One route per line: CIDR, via=IP, dev=interface, metric=number."><textarea aria-label="Desired tunnel routes" onChange={(event) => update("routes", event.target.value)} placeholder="10.0.0.0/24, via=10.255.0.1" rows={3} value={form.routes} /></Field>
                <Field label="Stale routes" tooltip="Only exact routes listed here are eligible for cleanup."><textarea aria-label="Stale tunnel routes" onChange={(event) => update("staleRoutes", event.target.value)} placeholder="10.0.0.0/24, dev=tun0" rows={3} value={form.staleRoutes} /></Field>
              </div>
            </details>
          )}
        </fieldset>

        <fieldset className="topologyFormSection">
          <legend>Endpoint addresses</legend>
          <div className="addressAllocationToolbar">
            <label className="compactCheckbox"><input checked={form.includeIpv4} onChange={(event) => update("includeIpv4", event.target.checked)} type="checkbox" /> IPv4</label>
            <label className="compactCheckbox"><input checked={form.includeIpv6} onChange={(event) => update("includeIpv6", event.target.checked)} type="checkbox" /> IPv6</label>
            {form.includeIpv4 && form.includeIpv6 && (
              <label className="addressFamilySelect" title="Primary address family for continuous latency evidence. The other family remains configured.">
                <span>Probe family</span>
                <select aria-label="Latency probe address family" onChange={(event) => update("latencyPrimaryFamily", event.target.value as "ipv4" | "ipv6")} value={form.latencyPrimaryFamily}>
                  <option value="ipv4">IPv4</option>
                  <option value="ipv6">IPv6</option>
                </select>
              </label>
            )}
            <button className="secondaryAction compactAction" disabled={allocationPending || (!form.includeIpv4 && !form.includeIpv6)} onClick={() => void allocate()} title={!form.includeIpv4 && !form.includeIpv6 ? "Select IPv4 or IPv6 before allocating addresses" : "Allocate an unused endpoint pair from the configured pool"} type="button"><Settings2 size={14} />{allocationPending ? "Allocating" : "Allocate"}</button>
          </div>
          {feedback?.location === "allocation" ? (
            <ActionFeedback
              className="localActionFeedback"
              message={feedback.message}
              tone={feedback.tone}
            />
          ) : null}
          {form.includeIpv4 && (
            <div className="topologyFormGrid fourColumn compactNumericGrid">
              <Field label="IPv4 pool" tooltip="Optional. Leave empty to use the server-configured allocation pool."><input aria-label="IPv4 allocation pool" onChange={(event) => update("ipv4Pool", event.target.value)} placeholder="Configured pool" value={form.ipv4Pool} /></Field>
              <Field label="Left IPv4"><input aria-label="Left tunnel IPv4" onChange={(event) => update("leftIpv4", event.target.value)} placeholder="10.255.0.0" value={form.leftIpv4} /></Field>
              <Field label="Right IPv4"><input aria-label="Right tunnel IPv4" onChange={(event) => update("rightIpv4", event.target.value)} placeholder="10.255.0.1" value={form.rightIpv4} /></Field>
              <Field label="Prefix"><UnitInput ariaLabel="IPv4 tunnel prefix" max={32} min={0} onChange={(value) => update("ipv4Prefix", value)} unit="bits" value={form.ipv4Prefix} /></Field>
            </div>
          )}
          {form.includeIpv6 && (
            <div className="topologyFormGrid fourColumn compactNumericGrid">
              <Field label="IPv6 pool" tooltip="Optional. Leave empty to use the server-configured allocation pool."><input aria-label="IPv6 allocation pool" onChange={(event) => update("ipv6Pool", event.target.value)} placeholder="Configured pool" value={form.ipv6Pool} /></Field>
              <Field label="Left IPv6"><input aria-label="Left tunnel IPv6" onChange={(event) => update("leftIpv6", event.target.value)} placeholder="fd00::" value={form.leftIpv6} /></Field>
              <Field label="Right IPv6"><input aria-label="Right tunnel IPv6" onChange={(event) => update("rightIpv6", event.target.value)} placeholder="fd00::1" value={form.rightIpv6} /></Field>
              <Field label="Prefix"><UnitInput ariaLabel="IPv6 tunnel prefix" max={128} min={0} onChange={(value) => update("ipv6Prefix", value)} unit="bits" value={form.ipv6Prefix} /></Field>
            </div>
          )}
        </fieldset>

        <fieldset className="topologyFormSection ospfOptionalSection">
          <legend>OSPF cost control</legend>
          <label className="toggleLine" title="Optional external routing-cost control. Tunnel creation and observation do not require OSPF.">
            <input checked={form.ospfEnabled} onChange={(event) => update("ospfEnabled", event.target.checked)} type="checkbox" />
            <span><strong>Enable OSPF cost control</strong><small>Each endpoint uses its Configuration preset unless this plan selects an override.</small></span>
          </label>
          {form.ospfEnabled && (
            <>
              <div className="topologyFormGrid twoColumn adapterBindingGrid">
                <AdapterDefinitionField clientId={form.leftClientId} domain="routing_cost" emptyLabel={endpointOspfPresetOptionLabel(leftOspfSource, configurationSourcesEvidenceState)} label="Left OSPF command override (optional)" onChange={(value) => update("leftRoutingDefinitionId", value)} onOpenAdapterDefinitions={onOpenAdapterDefinitions} definitions={leftRoutingDefinitions} value={form.leftRoutingDefinitionId} />
                <AdapterDefinitionField clientId={form.rightClientId} domain="routing_cost" emptyLabel={endpointOspfPresetOptionLabel(rightOspfSource, configurationSourcesEvidenceState)} label="Right OSPF command override (optional)" onChange={(value) => update("rightRoutingDefinitionId", value)} onOpenAdapterDefinitions={onOpenAdapterDefinitions} definitions={rightRoutingDefinitions} value={form.rightRoutingDefinitionId} />
              </div>
              <div
                aria-label="Resolved endpoint OSPF commands"
                className="consoleInlineDetailGrid"
              >
                <span>
                  <strong>Left endpoint command</strong>
                  <span title={endpointOspfResolutionLabel(form.leftClientId, leftOspfSource, configurationSourcesEvidenceState, form.leftRoutingDefinitionId, networkAdapterDefinitions)}>
                    {endpointOspfResolutionLabel(form.leftClientId, leftOspfSource, configurationSourcesEvidenceState, form.leftRoutingDefinitionId, networkAdapterDefinitions)}
                  </span>
                </span>
                <span>
                  <strong>Right endpoint command</strong>
                  <span title={endpointOspfResolutionLabel(form.rightClientId, rightOspfSource, configurationSourcesEvidenceState, form.rightRoutingDefinitionId, networkAdapterDefinitions)}>
                    {endpointOspfResolutionLabel(form.rightClientId, rightOspfSource, configurationSourcesEvidenceState, form.rightRoutingDefinitionId, networkAdapterDefinitions)}
                  </span>
                </span>
              </div>
              <span className="formHint">
                A per-plan override takes precedence over that VPS&apos;s effective preset.{" "}
                <button className="linkButton" onClick={onOpenConfigurationSources} type="button">
                  Manage VPS presets
                </button>
                .
              </span>
              <div className="topologyFormGrid ospfControlGrid">
                <Field label="Control mode" tooltip="Reviewed waits for operator confirmation. Automatic is executed only by the server controller after configured health gates."><select aria-label="OSPF control mode" onChange={(event) => update("ospfMode", event.target.value as "reviewed" | "automatic")} value={form.ospfMode}><option value="reviewed">Reviewed</option><option value="automatic">Automatic</option></select></Field>
                  <Field label="Planned latency" tooltip="Baseline round-trip latency used only until recent probe evidence exists."><UnitInput ariaLabel="Planned tunnel latency" min={0} onChange={(value) => update("plannedLatencyMs", value)} required step="0.1" unit="ms" value={form.plannedLatencyMs} /></Field>
                  <Field label="Planned loss" tooltip="Baseline packet loss used for the planned cost preview; measured probes replace it when available."><UnitInput ariaLabel="Planned packet loss" max={100} min={0} onChange={(value) => update("packetLossPercent", value)} required step="0.01" unit="%" value={form.packetLossPercent} /></Field>
                <Field label="Preference" tooltip="Values above 1 make the tunnel more preferred by lowering cost; values below 1 raise cost."><input aria-label="Tunnel preference" max={100} min={0.1} onChange={(event) => update("preference", event.target.value)} step="0.1" type="number" value={form.preference} /></Field>
                <div className="topologyOspfPreviewInline" aria-label="Live OSPF cost preview" title={OSPF_COST_MODEL_DETAIL}><span>Cost</span><strong>{previewCost}</strong><small>live preview</small></div>
              </div>
              <details className="topologyAdvancedFields">
                <summary title="Tune the automatic probe gate and the explicit cost formula. Reviewed mode also uses this formula but waits for confirmation.">Automatic gates and cost policy</summary>
                <div className="topologyFormGrid fourColumn compactNumericGrid">
                  <Field label="Minimum delta" tooltip="Ignore smaller differences between the current endpoint cost and the recommendation."><UnitInput ariaLabel="Minimum OSPF cost delta" max={65535} min={1} onChange={(value) => update("minCostDelta", value)} unit="cost" value={form.minCostDelta} /></Field>
                  <Field label="Healthy probes" tooltip="Automatic mode requires this many consecutive healthy probes within the recent evidence window. Reviewed mode still shows the streak for judgment."><UnitInput ariaLabel="Required consecutive healthy OSPF probes" max={10} min={1} onChange={(value) => update("healthyWindows", value)} unit="probes" value={form.healthyWindows} /></Field>
                  <Field label="Latency weight" tooltip="Multiplier applied to measured or planned latency in the cost formula."><input aria-label="OSPF latency weight" min={0} onChange={(event) => update("latencyWeight", event.target.value)} step="0.1" type="number" value={form.latencyWeight} /></Field>
                  <Field label="Loss weight" tooltip="Multiplier applied to packet-loss ratio; higher values penalize lossy paths more strongly."><input aria-label="OSPF loss weight" min={0} onChange={(event) => update("lossWeight", event.target.value)} step="1" type="number" value={form.lossWeight} /></Field>
                  <Field label="Bandwidth weight" tooltip="Multiplier for the diminishing-return bandwidth penalty across 10-10000 Mbps."><input aria-label="OSPF bandwidth weight" min={0} onChange={(event) => update("bandwidthWeight", event.target.value)} step="0.1" type="number" value={form.bandwidthWeight} /></Field>
                  <Field label="Preference bias" tooltip="Global multiplier on computed cost before the per-plan preference divisor."><input aria-label="OSPF preference bias" min={0} onChange={(event) => update("preferenceBias", event.target.value)} step="0.1" type="number" value={form.preferenceBias} /></Field>
                  <Field label="Minimum cost" tooltip="Lower clamp for the final OSPF cost."><input aria-label="Minimum OSPF cost" max={65535} min={1} onChange={(event) => update("minCost", event.target.value)} type="number" value={form.minCost} /></Field>
                  <Field label="Maximum cost" tooltip="Upper clamp for the final OSPF cost."><input aria-label="Maximum OSPF cost" max={65535} min={1} onChange={(event) => update("maxCost", event.target.value)} type="number" value={form.maxCost} /></Field>
                </div>
              </details>
            </>
          )}
        </fieldset>

        {feedback?.location === "form" ? (
          <ActionFeedback
            className="localActionFeedback"
            message={feedback.message}
            tone={feedback.tone}
          />
        ) : null}
        <div className="tunnelComposerFooter">
          <label className="toggleLine compactToggle">
            <input checked={form.enabled} onChange={(event) => update("enabled", event.target.checked)} type="checkbox" />
            <span><strong>{existing ? "Enabled after update" : "Enable after save"}</strong><small>Push runtime state to both endpoints immediately.</small></span>
          </label>
          <div className="dispatchActions">
            <button className="secondaryAction" onClick={onClose} type="button">Cancel</button>
            <button className="primaryAction" disabled={pending || unchanged} title={pending ? "Saving tunnel plan" : unchanged ? "No tunnel plan changes to review" : validationError ?? "Review the exact declaration before saving"} type="submit"><CirclePlus size={16} />{existing ? "Review update" : "Review plan"}</button>
          </div>
        </div>
      </form>
      <ConfirmationPrompt
        confirmLabel={existing ? "Update plan" : "Save plan"}
        detail="Save this exact declaration. When enabled, vpsman pushes only the declared runtime plan and adapter definition snapshots to the two selected agents."
        items={snapshot ? createConfirmationItems(
          snapshot,
          existing ?? undefined,
          networkAdapterDefinitions,
          configurationSources,
          configurationSourcesEvidenceState,
        ) : []}
        onCancel={() => setSnapshot(null)}
        onConfirm={() => snapshot && void confirmCreate(snapshot)}
        open={snapshot !== null}
        pending={pending}
        title={existing ? "Confirm tunnel plan update" : "Confirm tunnel plan creation"}
        tone="normal"
      />
    </section>
  );
}

function ManagerChoice({ active, detail, label, onClick }: { active: boolean; detail: string; label: string; onClick: () => void }) {
  return (
    <button aria-pressed={active} className={active ? "isActive" : undefined} onClick={onClick} title={detail} type="button">
      <strong>{label}</strong><small>{detail}</small>
    </button>
  );
}

function AdapterDefinitionField({ clientId, domain, emptyLabel = "Select adapter definition", label, onChange, onOpenAdapterDefinitions, definitions, value }: { clientId: string; domain: NetworkAdapterKind; emptyLabel?: string; label: string; onChange: (value: string) => void; onOpenAdapterDefinitions: (domain: NetworkAdapterKind) => void; definitions: NetworkAdapterDefinitionRecord[]; value: string }) {
  return (
    <div className="topologyField" title="The adapter definition stores direct absolute argv. The agent never installs or edits the script.">
      <span>{label}</span>
      <div className="adapterDefinitionSelectRow">
        <select aria-label={label} disabled={!clientId || (domain !== "routing_cost" && definitions.length === 0)} onChange={(event) => onChange(event.target.value)} value={value}>
          <option value="">{!clientId ? "Select endpoint first" : domain !== "routing_cost" && definitions.length === 0 ? "No compatible adapter definitions" : emptyLabel}</option>
          {definitions.map((definition) => <option key={definition.id} value={definition.id}>{definition.name}</option>)}
        </select>
        <button aria-label={`Open adapter definitions for ${label}`} className="iconAction" onClick={() => onOpenAdapterDefinitions(domain)} title={`Create or manage ${domain === "routing_cost" ? "routing cost" : "tunnel runtime"} adapters`} type="button"><ExternalLink size={15} /></button>
      </div>
    </div>
  );
}

function Field({ children, label, tooltip }: { children: React.ReactNode; label: string; tooltip?: string }) {
  return <label title={tooltip}><span>{label}</span>{children}</label>;
}

function UnitInput({ ariaLabel, max, min, onChange, required, step, unit, value }: { ariaLabel: string; max?: number; min?: number; onChange: (value: string) => void; required?: boolean; step?: string; unit: string; value: string }) {
  return <div className="inlineUnitInput"><input aria-label={ariaLabel} max={max} min={min} onChange={(event) => onChange(event.target.value)} required={required} step={step} type="number" value={value} /><small>{unit}</small></div>;
}

function Metric({ label, title, tone = "normal", value }: { label: string; title?: string; tone?: "normal" | "warning"; value: number | string }) {
  return <span className={tone === "warning" ? "hasAttention" : undefined} title={title}><small>{label}</small><strong>{value}</strong></span>;
}

function WorkflowLink({ detail, icon, label, onClick }: { detail: string; icon: React.ReactNode; label: string; onClick: () => void }) {
  return <button onClick={onClick} type="button"><span className="workflowIcon">{icon}</span><span><strong>{label}</strong><small>{detail}</small></span><ExternalLink size={15} /></button>;
}

function PlanFact({ label, title, value }: { label: string; title?: string; value: string }) {
  return <span><small>{label}</small><strong title={title ?? value}>{value}</strong></span>;
}

function tunnelEndpointDisplayState(
  enabled: boolean,
  runtimeConfig: TunnelPlanEndpointRuntimeConfig,
  observedState?: string | null,
): string {
  switch (runtimeConfig.status) {
    case "queued":
    case "pending":
      return enabled ? "applying" : "removing";
    case "failed":
      return enabled ? "apply_failed" : "removal_failed";
    case "stale_pending":
      return "sync_required";
    case "applied":
      return observedState ?? "applied";
    case "removed":
      return "removed";
    case "removal_required":
      return "removal_required";
    case "not_applied":
    case "not_dispatched":
      return "not_applied";
    default:
      return runtimeConfig.status || "unknown";
  }
}

function tunnelRuntimeConfigLabel(
  runtimeConfig: TunnelPlanEndpointRuntimeConfig,
): string {
  return readableTelemetryToken(
    tunnelEndpointDisplayState(
      runtimeConfig.desired === "present",
      runtimeConfig,
    ),
  );
}

function tunnelEndpointStateTitle(
  label: string,
  runtimeConfig: TunnelPlanEndpointRuntimeConfig,
  reason?: string | null,
  observedAt?: string | null,
): string {
  return [
    `${label} desired: ${runtimeConfig.desired}`,
    `Runtime config: ${tunnelRuntimeConfigLabel(runtimeConfig)}`,
    runtimeConfig.error,
    runtimeConfig.job_id ? `Job ${runtimeConfig.job_id}` : null,
    runtimeConfig.updated_at ? `Apply state ${runtimeConfig.updated_at}` : null,
    reason ? readableTelemetryToken(reason) : null,
    observedAt ? `Observed ${observedAt}` : "No endpoint observation",
  ]
    .filter(Boolean)
    .join("; ");
}

function tunnelDispatchFeedback(
  sync: RuntimeConfigDispatchRecord[],
  savedMessage: string,
  runtimeAction = "Runtime apply",
): Feedback {
  const failures = sync.filter((outcome) => outcome.status !== "queued");
  if (failures.length > 0) {
    const details = failures
      .map(
        (outcome) =>
          `${outcome.client_id}: ${dispatchFailureReason(outcome.error, outcome.status, runtimeAction)}`,
      )
      .join("; ");
    return {
      message: `${savedMessage}. Desired state was saved, but ${runtimeAction.toLowerCase()} was not queued for ${failures.length} endpoint${failures.length === 1 ? "" : "s"}: ${details}`,
      tone: "warning",
    };
  }
  if (sync.length > 0) {
    return {
      message: `${savedMessage}. ${runtimeAction} queued for ${sync.length} endpoint${sync.length === 1 ? "" : "s"}.`,
      tone: "progress",
    };
  }
  return { message: savedMessage, tone: "success" };
}

function tunnelRetirementFeedback(
  sync: RuntimeConfigDispatchRecord[],
  savedMessage: string,
): Feedback {
  const feedback = tunnelDispatchFeedback(sync, savedMessage, "Runtime removal");
  if (feedback.tone !== "warning") return feedback;
  return {
    ...feedback,
    message: `${feedback.message} The plan remains deleted; affected agents reconcile current desired state on reconnect. Inspect Config > Overview if an online endpoint remains pending.`,
  };
}

function tunnelConnectivityPresentation(
  plan: TunnelPlanRecord,
  edge?: TopologyGraphEdge,
): { label: string; detail: string; statusClass: string; title: string } {
  if (!plan.enabled) {
    return {
      detail: "Plan disabled",
      label: "Disabled",
      statusClass: "neutral",
      title: "Connectivity is not evaluated while the tunnel plan is disabled.",
    };
  }
  if (plan.connection_assessment !== "automatic") {
    const label = plan.connection_assessment === "connected" ? "Connected" : "Disconnected";
    const assessedAt = plan.connection_assessed_at
      ? formatCompactTime(plan.connection_assessed_at)
      : "time unavailable";
    const actor = plan.connection_assessed_by
      ? `operator ${shortId(plan.connection_assessed_by)}`
      : "operator";
    return {
      detail: `Operator assessment · ${assessedAt}`,
      label,
      statusClass: plan.connection_assessment === "connected" ? "info" : "warning",
      title: `${label} assessed by ${actor}; ${plan.connection_assessment_note ?? "no evidence note"}. Runtime reconciliation and automatic OSPF still use machine evidence.`,
    };
  }
  const left = edge?.left_reachability_state ?? "unknown";
  const right = edge?.right_reachability_state ?? "unknown";
  const reasons = [edge?.left_reachability_reason, edge?.right_reachability_reason]
    .filter((value): value is string => Boolean(value))
    .map(readableTelemetryToken)
    .join("; ");
  if (left === "reachable" && right === "reachable") {
    return {
      detail: "Measured on both endpoints",
      label: "Reachable",
      statusClass: "ok",
      title: "Both declared endpoints reported successful reachability evidence.",
    };
  }
  if (left === "probe_failed" || right === "probe_failed") {
    if (left === "reachable" || right === "reachable") {
      return {
        detail: "Peer probe failed; not proof of disconnect",
        label: "Partially verified",
        statusClass: "warning",
        title: `One endpoint has positive reachability evidence while the peer probe failed${reasons ? `; ${reasons}` : ""}.`,
      };
    }
    return {
      detail: "Probe failed; not proof of disconnect",
      label: "Unverified",
      statusClass: "warning",
      title: `A configured reachability probe failed. ICMP or the selected probe may be blocked even when the tunnel carries traffic${reasons ? `; ${reasons}` : ""}.`,
    };
  }
  if (left === "reachable" || right === "reachable") {
    return {
      detail: "One endpoint measured",
      label: "Partially verified",
      statusClass: "info",
      title: `Only one endpoint has positive reachability evidence${reasons ? `; ${reasons}` : ""}.`,
    };
  }
  if (left === "not_configured" && right === "not_configured") {
    return {
      detail: "No reachability probe configured",
      label: "Unverified",
      statusClass: "neutral",
      title: "Runtime reconciliation remains visible, but neither endpoint has a configured reachability probe.",
    };
  }
  return {
    detail: "Awaiting reachability evidence",
    label: "Unverified",
    statusClass: "neutral",
    title: reasons || "No current endpoint reachability evidence is available.",
  };
}

function tunnelConnectionAssessmentError(error: unknown): string {
  if (error instanceof ApiResponseError) {
    if (error.code === "tunnel_plan_snapshot_stale") {
      return "This plan changed before the assessment was saved. Refresh its details and review the current evidence.";
    }
    if (error.code === "tunnel_connection_assessment_requires_enabled_plan") {
      return "Enable the tunnel plan before recording a connectivity assessment.";
    }
    if (error.code === "tunnel_connection_assessment_note_required") {
      return "Add a concise evidence note of 500 characters or fewer for a manual assessment.";
    }
  }
  return error instanceof Error ? error.message : "Connectivity assessment update failed";
}

function initialTunnelPlanForm(): TunnelPlanForm {
  return {
    bandwidthMbps: String(DEFAULT_TUNNEL_BANDWIDTH_MBPS),
    bandwidthWeight: String(DEFAULT_OSPF_POLICY.bandwidth_weight),
    burstKb: "",
    desiredInterfaces: "",
    egressKbps: "",
    enabled: false,
    fouIpProto: "4",
    fouPeerPort: "5555",
    fouPort: "5555",
    healthyWindows: "2",
    includeIpv4: true,
    includeIpv6: false,
    ingressKbps: "",
    interfaceName: "",
    ipv4Pool: "",
    ipv4Prefix: "31",
    ipv6Pool: "",
    ipv6Prefix: "127",
    kind: "gre",
    latencyWeight: String(DEFAULT_OSPF_POLICY.latency_weight),
    latencyPrimaryFamily: "ipv4",
    leftClientId: "",
    leftIpv4: "",
    leftIpv6: "",
    leftRoutingDefinitionId: "",
    leftRuntimeDefinitionId: "",
    leftLocalUnderlay: "",
    leftRemoteUnderlay: "",
    lossWeight: String(DEFAULT_OSPF_POLICY.loss_weight),
    maxCost: String(DEFAULT_OSPF_POLICY.max_cost),
    minCost: String(DEFAULT_OSPF_POLICY.min_cost),
    minCostDelta: "5",
    name: "",
    ospfEnabled: false,
    ospfMode: "reviewed",
    packetLossPercent: "0",
    plannedLatencyMs: "20",
    preference: "1",
    preferenceBias: String(DEFAULT_OSPF_POLICY.preference_bias),
    rightClientId: "",
    rightIpv4: "",
    rightIpv6: "",
    rightRoutingDefinitionId: "",
    rightRuntimeDefinitionId: "",
    rightLocalUnderlay: "",
    rightRemoteUnderlay: "",
    routes: "",
    runtimeManager: "agent_iproute2_managed",
    runtimeTopologyVersion: "",
    staleInterfaces: "",
    staleRoutes: "",
  };
}

function tunnelPlanFormFromRecord(record: TunnelPlanRecord): TunnelPlanForm {
  const input = record.input;
  const runtime = input.runtime_control ?? { manager: "agent_iproute2_managed" as const };
  const topology = input.runtime_topology ?? {};
  const traffic = runtime.traffic_limit ?? {};
  const fou = runtime.fou ?? { ipproto: 4, peer_port: 5555, port: 5555 };
  const ospf = input.ospf ?? null;
  const policy = ospf?.policy ?? DEFAULT_OSPF_POLICY;
  return {
    ...initialTunnelPlanForm(),
    bandwidthMbps: String(input.bandwidth_mbps),
    bandwidthWeight: String(policy.bandwidth_weight),
    burstKb: optionalNumberText(traffic.burst_kb),
    desiredInterfaces: (topology.desired_interfaces ?? []).join("\n"),
    egressKbps: optionalNumberText(traffic.egress_kbps),
    enabled: record.enabled,
    fouIpProto: String(fou.ipproto),
    fouPeerPort: String(fou.peer_port),
    fouPort: String(fou.port),
    healthyWindows: String(ospf?.healthy_windows ?? 2),
    includeIpv4: Boolean(input.ipv4_tunnel),
    includeIpv6: Boolean(input.ipv6_tunnel),
    ingressKbps: optionalNumberText(traffic.ingress_kbps),
    interfaceName: input.interface_name,
    ipv4Pool: input.address_pool_cidr,
    ipv4Prefix: String(input.ipv4_tunnel?.prefix_len ?? 31),
    ipv6Pool: input.ipv6_address_pool_cidr ?? "",
    ipv6Prefix: String(input.ipv6_tunnel?.prefix_len ?? 127),
    kind: input.kind,
    latencyPrimaryFamily: input.latency_primary_family ?? "ipv4",
    latencyWeight: String(policy.latency_weight),
    leftClientId: input.left_client_id,
    leftIpv4: input.ipv4_tunnel?.left ?? "",
    leftIpv6: input.ipv6_tunnel?.left ?? "",
    leftRoutingDefinitionId: ospf?.left_adapter_template_id ?? "",
    leftRuntimeDefinitionId: runtime.left_adapter_template_id ?? "",
    leftLocalUnderlay: input.left_local_underlay ?? "",
    leftRemoteUnderlay: input.left_remote_underlay,
    lossWeight: String(policy.loss_weight),
    maxCost: String(policy.max_cost),
    minCost: String(policy.min_cost),
    minCostDelta: String(ospf?.min_cost_delta ?? 5),
    name: record.name,
    ospfEnabled: Boolean(ospf),
    ospfMode: ospf?.mode ?? "reviewed",
    packetLossPercent: String((ospf?.planned_packet_loss_ratio ?? 0) * 100),
    plannedLatencyMs: String(ospf?.planned_latency_ms ?? 20),
    preference: String(ospf?.preference ?? 1),
    preferenceBias: String(policy.preference_bias),
    rightClientId: input.right_client_id,
    rightIpv4: input.ipv4_tunnel?.right ?? "",
    rightIpv6: input.ipv6_tunnel?.right ?? "",
    rightRoutingDefinitionId: ospf?.right_adapter_template_id ?? "",
    rightRuntimeDefinitionId: runtime.right_adapter_template_id ?? "",
    rightLocalUnderlay: input.right_local_underlay ?? "",
    rightRemoteUnderlay: input.right_remote_underlay,
    routes: (topology.routes ?? []).map(formatRuntimeRoute).join("\n"),
    runtimeManager: runtime.manager,
    runtimeTopologyVersion: topology.version ?? "",
    staleInterfaces: (topology.stale_interfaces ?? []).join("\n"),
    staleRoutes: (topology.stale_routes ?? []).map(formatRuntimeRoute).join("\n"),
  };
}

function formatRuntimeRoute(route: RuntimeTunnelRoute): string {
  return [
    route.destination_cidr,
    route.via ? `via=${route.via}` : null,
    route.interface_name ? `dev=${route.interface_name}` : null,
    route.metric !== null && route.metric !== undefined ? `metric=${route.metric}` : null,
  ]
    .filter(Boolean)
    .join(", ");
}

function optionalNumberText(value: number | null | undefined): string {
  return value === null || value === undefined ? "" : String(value);
}

function validateTunnelPlanForm(form: TunnelPlanForm): string | null {
  if (!form.name.trim()) return "Plan name is required";
  if (form.name.trim().length > 128) return "Plan name must be 128 characters or fewer";
  if (!form.interfaceName.trim()) return "Interface name is required";
  if (!/^[A-Za-z0-9_.-]{1,15}$/.test(form.interfaceName.trim())) return "Interface must be 1-15 letters, numbers, dots, underscores, or hyphens";
  if (!form.leftClientId || !form.rightClientId) return "Select both endpoint VPSs";
  if (form.leftClientId === form.rightClientId) return "Tunnel endpoints must be different VPSs";
  const leftUnderlayError = validateEndpointUnderlay(
    "Left",
    form.leftRemoteUnderlay,
    form.leftLocalUnderlay,
    form.runtimeManager,
  );
  if (leftUnderlayError) return leftUnderlayError;
  const rightUnderlayError = validateEndpointUnderlay(
    "Right",
    form.rightRemoteUnderlay,
    form.rightLocalUnderlay,
    form.runtimeManager,
  );
  if (rightUnderlayError) return rightUnderlayError;
  const bandwidth = Number(form.bandwidthMbps);
  if (!Number.isInteger(bandwidth) || bandwidth < MIN_TUNNEL_BANDWIDTH_MBPS || bandwidth > MAX_TUNNEL_BANDWIDTH_MBPS) return `Bandwidth must be a whole number from ${MIN_TUNNEL_BANDWIDTH_MBPS} to ${MAX_TUNNEL_BANDWIDTH_MBPS} Mbps`;
  if (form.runtimeManager !== "external_observed") {
    const trafficError = validateOptionalIntegerRange(form.ingressKbps, "Ingress limit", 64, 1_000_000)
      ?? validateOptionalIntegerRange(form.egressKbps, "Egress limit", 64, 1_000_000)
      ?? validateOptionalIntegerRange(form.burstKb, "Burst", 1, 1_048_576);
    if (trafficError) return trafficError;
  }
  if (form.kind === "fou" && form.runtimeManager !== "external_observed") {
    const fouError = validateIntegerRange(form.fouPort, "FOU port", 1, 65_535)
      ?? validateIntegerRange(form.fouPeerPort, "FOU peer port", 1, 65_535)
      ?? validateIntegerRange(form.fouIpProto, "FOU IP protocol", 1, 255);
    if (fouError) return fouError;
  }
  if (!form.includeIpv4 && !form.includeIpv6) return "Select IPv4 or IPv6 tunnel addresses";
  if (form.includeIpv4) {
    if (!form.leftIpv4.trim() || !form.rightIpv4.trim()) return "Enter or allocate both IPv4 endpoint addresses";
    if (!isIpv4Address(form.leftIpv4) || !isIpv4Address(form.rightIpv4) || form.leftIpv4.trim() === form.rightIpv4.trim()) return "IPv4 endpoint addresses must be valid and different";
    const prefixError = validateIntegerRange(form.ipv4Prefix, "IPv4 prefix", 0, 32);
    if (prefixError) return prefixError;
    if (!sameIpSubnet(form.leftIpv4, form.rightIpv4, Number(form.ipv4Prefix), 32)) return "IPv4 endpoint addresses must share the selected point-to-point prefix";
    if (form.ipv4Pool.trim() && !isIpCidr(form.ipv4Pool, 32)) return "IPv4 allocation pool must be a valid CIDR";
  }
  if (form.includeIpv6) {
    if (!form.leftIpv6.trim() || !form.rightIpv6.trim()) return "Enter or allocate both IPv6 endpoint addresses";
    if (!isIpv6Address(form.leftIpv6) || !isIpv6Address(form.rightIpv6) || form.leftIpv6.trim() === form.rightIpv6.trim()) return "IPv6 endpoint addresses must be valid and different";
    const prefixError = validateIntegerRange(form.ipv6Prefix, "IPv6 prefix", 0, 128);
    if (prefixError) return prefixError;
    if (!sameIpSubnet(form.leftIpv6, form.rightIpv6, Number(form.ipv6Prefix), 128)) return "IPv6 endpoint addresses must share the selected point-to-point prefix";
    if (form.ipv6Pool.trim() && !isIpCidr(form.ipv6Pool, 128)) return "IPv6 allocation pool must be a valid CIDR";
  }
  if (form.runtimeManager === "agent_iproute2_managed" && !AGENT_TUNNEL_KINDS.includes(form.kind)) return `${formatTunnelKind(form.kind)} requires external ownership`;
  if (form.runtimeManager === "external_managed_adapter" && (!form.leftRuntimeDefinitionId || !form.rightRuntimeDefinitionId)) return "Select a runtime adapter definition for both endpoints";
  if (form.ospfEnabled) {
    const ospfError = validateNumberRange(form.plannedLatencyMs, "Planned latency", 0, 60_000)
      ?? validateNumberRange(form.packetLossPercent, "Planned packet loss", 0, 100)
      ?? validateNumberRange(form.preference, "Preference", 0.1, 100)
      ?? validateIntegerRange(form.minCostDelta, "Minimum OSPF cost delta", 1, 65_535)
      ?? validateIntegerRange(form.healthyWindows, "Required healthy probes", 1, 10)
      ?? validateNonNegativeNumber(form.latencyWeight, "Latency weight")
      ?? validateNonNegativeNumber(form.lossWeight, "Loss weight")
      ?? validateNonNegativeNumber(form.bandwidthWeight, "Bandwidth weight")
      ?? validateNonNegativeNumber(form.preferenceBias, "Preference bias");
    if (ospfError) return ospfError;
  }
  const minCost = Number(form.minCost);
  const maxCost = Number(form.maxCost);
  if (form.ospfEnabled && (!Number.isInteger(minCost) || !Number.isInteger(maxCost) || minCost < 1 || minCost > maxCost || maxCost > 65535)) return "OSPF cost bounds must be whole numbers within 1-65535 and minimum cannot exceed maximum";
  return null;
}

function validateOptionalIntegerRange(value: string, label: string, min: number, max: number): string | null {
  return value.trim() ? validateIntegerRange(value, label, min, max) : null;
}

function validateIntegerRange(value: string, label: string, min: number, max: number): string | null {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= min && parsed <= max
    ? null
    : `${label} must be a whole number from ${min} to ${max}`;
}

function validateNumberRange(value: string, label: string, min: number, max: number): string | null {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= min && parsed <= max
    ? null
    : `${label} must be from ${min} to ${max}`;
}

function validateNonNegativeNumber(value: string, label: string): string | null {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= 0
    ? null
    : `${label} must be zero or greater`;
}

function validateEndpointUnderlay(
  side: "Left" | "Right",
  remoteValue: string,
  localValue: string,
  manager: RuntimeTunnelManager,
): string | null {
  const remote = remoteValue.trim();
  const local = localValue.trim();
  if (!remote) return `${side} remote underlay destination is required`;
  if (!isIpAddress(remote)) return `${side} remote underlay destination must be a valid IP address`;
  if (local && !isIpAddress(local)) return `${side} local underlay source must be a valid IP address`;
  if (local && isIpv4Address(local) !== isIpv4Address(remote)) {
    return `${side} local source and remote destination must use the same IP family`;
  }
  if (manager === "agent_iproute2_managed" && !isIpv4Address(remote)) {
    return `${side} agent iproute2 underlay requires an IPv4 remote destination`;
  }
  return null;
}

function isIpAddress(value: string): boolean {
  return isIpv4Address(value) || isIpv6Address(value);
}

function isIpv4Address(value: string): boolean {
  const parts = value.trim().split(".");
  return parts.length === 4 && parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255 && (part === "0" || !part.startsWith("0")));
}

function isIpv6Address(value: string): boolean {
  const address = value.trim();
  if (!address.includes(":") || !/^[0-9a-f:.]+$/i.test(address)) return false;
  try {
    new URL(`http://[${address}]/`);
    return true;
  } catch {
    return false;
  }
}

function isIpCidr(value: string, bits: 32 | 128): boolean {
  const [address, prefix, ...rest] = value.trim().split("/");
  const parsedPrefix = Number(prefix);
  return rest.length === 0
    && (bits === 32 ? isIpv4Address(address) : isIpv6Address(address))
    && Number.isInteger(parsedPrefix)
    && parsedPrefix >= 0
    && parsedPrefix <= bits;
}

function sameIpSubnet(left: string, right: string, prefix: number, bits: 32 | 128): boolean {
  const leftValue = ipAddressValue(left, bits);
  const rightValue = ipAddressValue(right, bits);
  if (leftValue === null || rightValue === null || prefix < 0 || prefix > bits) return false;
  const hostBits = BigInt(bits - prefix);
  return leftValue >> hostBits === rightValue >> hostBits;
}

function ipAddressValue(value: string, bits: 32 | 128): bigint | null {
  if (bits === 32) {
    if (!isIpv4Address(value)) return null;
    return value.trim().split(".").reduce((result, part) => (result << 8n) + BigInt(part), 0n);
  }
  return ipv6AddressValue(value);
}

function ipv6AddressValue(value: string): bigint | null {
  let address = value.trim().toLowerCase();
  const ipv4Tail = address.match(/(?:^|:)(\d+\.\d+\.\d+\.\d+)$/)?.[1];
  if (ipv4Tail) {
    const ipv4 = ipAddressValue(ipv4Tail, 32);
    if (ipv4 === null) return null;
    address = `${address.slice(0, -ipv4Tail.length)}${((ipv4 >> 16n) & 0xffffn).toString(16)}:${(ipv4 & 0xffffn).toString(16)}`;
  }
  const halves = address.split("::");
  if (halves.length > 2) return null;
  const left = halves[0] ? halves[0].split(":") : [];
  const right = halves.length === 2 && halves[1] ? halves[1].split(":") : [];
  const missing = 8 - left.length - right.length;
  if ((halves.length === 1 && missing !== 0) || (halves.length === 2 && missing < 1)) return null;
  const groups = [...left, ...Array.from({ length: missing }, () => "0"), ...right];
  if (groups.length !== 8 || groups.some((group) => !/^[0-9a-f]{1,4}$/.test(group))) return null;
  return groups.reduce((result, group) => (result << 16n) + BigInt(`0x${group}`), 0n);
}

function validateExistingTunnelPlanConflicts(
  form: TunnelPlanForm,
  plans: TunnelPlanRecord[],
  editingPlanId?: string,
): string | null {
  const clientIds = new Set([form.leftClientId, form.rightClientId]);
  const requestedAddresses = new Set(
    [
      form.includeIpv4 ? ipAddressKey(form.leftIpv4, 32) : null,
      form.includeIpv4 ? ipAddressKey(form.rightIpv4, 32) : null,
      form.includeIpv6 ? ipAddressKey(form.leftIpv6, 128) : null,
      form.includeIpv6 ? ipAddressKey(form.rightIpv6, 128) : null,
    ].filter((value): value is string => Boolean(value)),
  );
  for (const plan of plans) {
    if (plan.id === editingPlanId) continue;
    if (
      plan.plan.interface_name === form.interfaceName.trim()
      && (clientIds.has(plan.left_client_id) || clientIds.has(plan.right_client_id))
    ) {
      return "Another saved plan already uses this interface on one of the selected VPSs";
    }
    const savedAddresses = [
      plan.plan.ipv4_tunnel ? ipAddressKey(plan.plan.ipv4_tunnel.left, 32) : null,
      plan.plan.ipv4_tunnel ? ipAddressKey(plan.plan.ipv4_tunnel.right, 32) : null,
      plan.plan.ipv6_tunnel ? ipAddressKey(plan.plan.ipv6_tunnel.left, 128) : null,
      plan.plan.ipv6_tunnel ? ipAddressKey(plan.plan.ipv6_tunnel.right, 128) : null,
    ];
    if (savedAddresses.some((address) => address && requestedAddresses.has(address))) {
      return "Another saved plan already uses one of these tunnel endpoint addresses";
    }
  }
  return null;
}

function ipAddressKey(value: string, bits: 32 | 128): string | null {
  const parsed = ipAddressValue(value, bits);
  return parsed === null ? null : `${bits}:${parsed.toString(16)}`;
}

function validateAdapterBindings(
  form: TunnelPlanForm,
  leftRuntimeDefinitions: NetworkAdapterDefinitionRecord[],
  rightRuntimeDefinitions: NetworkAdapterDefinitionRecord[],
  leftRoutingDefinitions: NetworkAdapterDefinitionRecord[],
  rightRoutingDefinitions: NetworkAdapterDefinitionRecord[],
): string | null {
  if (form.runtimeManager === "external_managed_adapter") {
    const left = leftRuntimeDefinitions.find((definition) => definition.id === form.leftRuntimeDefinitionId);
    const right = rightRuntimeDefinitions.find((definition) => definition.id === form.rightRuntimeDefinitionId);
    if (form.leftRuntimeDefinitionId && !left) return "The left runtime adapter is no longer available for this VPS";
    if (form.rightRuntimeDefinitionId && !right) return "The right runtime adapter is no longer available for this VPS";
    const hasTrafficLimit = [form.ingressKbps, form.egressKbps, form.burstKb]
      .some((value) => value.trim() !== "");
    if (hasTrafficLimit && (!adapterHasDefinitionField(left, "traffic_limit_command") || !adapterHasDefinitionField(right, "traffic_limit_command"))) {
      return "Both selected runtime adapters must declare traffic_limit_command before this plan can apply traffic limits";
    }
  }
  if (form.ospfEnabled) {
    if (form.leftRoutingDefinitionId && !leftRoutingDefinitions.some((definition) => definition.id === form.leftRoutingDefinitionId)) {
      return "The left routing adapter is no longer available for this VPS";
    }
    if (form.rightRoutingDefinitionId && !rightRoutingDefinitions.some((definition) => definition.id === form.rightRoutingDefinitionId)) {
      return "The right routing adapter is no longer available for this VPS";
    }
  }
  return null;
}

function adapterHasDefinitionField(
  definition: NetworkAdapterDefinitionRecord | undefined,
  field: string,
): boolean {
  return Boolean(
    definition
      && typeof definition.definition === "object"
      && definition.definition !== null
      && !Array.isArray(definition.definition)
      && field in definition.definition,
  );
}

function buildTunnelPlanRequest(form: TunnelPlanForm): CreateTunnelPlanRequest {
  const runtimeControl = buildRuntimeControl(form.runtimeManager, {
    burstKb: form.burstKb,
    egressKbps: form.egressKbps,
    fouIpproto: form.kind === "fou" ? form.fouIpProto : undefined,
    fouPeerPort: form.kind === "fou" ? form.fouPeerPort : undefined,
    fouPort: form.kind === "fou" ? form.fouPort : undefined,
    ingressKbps: form.ingressKbps,
    leftAdapterDefinitionId: form.leftRuntimeDefinitionId,
    rightAdapterDefinitionId: form.rightRuntimeDefinitionId,
  });
  const runtimeTopology = form.runtimeManager === "agent_iproute2_managed"
    ? buildRuntimeTopology({
        desiredText: form.desiredInterfaces,
        routesText: form.routes,
        staleRoutesText: form.staleRoutes,
        staleText: form.staleInterfaces,
        version: form.runtimeTopologyVersion,
      })
    : {};
  return {
    address_pool_cidr: form.ipv4Pool.trim(),
    bandwidth_mbps: clampTunnelBandwidthMbps(form.bandwidthMbps),
    confirmed: true,
    enabled: form.enabled,
    interface_name: form.interfaceName.trim(),
    ipv4_tunnel: form.includeIpv4 ? addressPair(form.leftIpv4, form.rightIpv4, form.ipv4Prefix) : null,
    ipv6_address_pool_cidr: form.ipv6Pool.trim() || null,
    ipv6_tunnel: form.includeIpv6 ? addressPair(form.leftIpv6, form.rightIpv6, form.ipv6Prefix) : null,
    kind: form.kind,
    latency_primary_family: form.includeIpv4 && form.includeIpv6
      ? form.latencyPrimaryFamily
      : form.includeIpv4 ? "ipv4" : "ipv6",
    left_client_id: form.leftClientId,
    left_local_underlay: form.leftLocalUnderlay.trim() || null,
    left_remote_underlay: form.leftRemoteUnderlay.trim(),
    name: form.name.trim(),
    ospf: form.ospfEnabled ? {
      healthy_windows: integerOr(form.healthyWindows, 2),
      left_adapter_template_id: form.leftRoutingDefinitionId || null,
      min_cost_delta: integerOr(form.minCostDelta, 5),
      mode: form.ospfMode,
      planned_latency_ms: numberOr(form.plannedLatencyMs, 20),
      planned_packet_loss_ratio: numberOr(form.packetLossPercent, 0) / 100,
      policy: ospfPolicyFromForm(form),
      preference: numberOr(form.preference, 1),
      right_adapter_template_id: form.rightRoutingDefinitionId || null,
    } : null,
    reserved_addresses: [],
    right_client_id: form.rightClientId,
    right_local_underlay: form.rightLocalUnderlay.trim() || null,
    right_remote_underlay: form.rightRemoteUnderlay.trim(),
    runtime_control: runtimeControl,
    runtime_topology: runtimeTopology,
  };
}

function ospfPolicyFromForm(form: TunnelPlanForm): OspfCostPolicy {
  return {
    bandwidth_weight: numberOr(form.bandwidthWeight, 10),
    latency_weight: numberOr(form.latencyWeight, 1),
    loss_weight: numberOr(form.lossWeight, 400),
    max_cost: integerOr(form.maxCost, 65535),
    min_cost: integerOr(form.minCost, 5),
    preference_bias: numberOr(form.preferenceBias, 1),
  };
}

function applyAllocation(form: TunnelPlanForm, response: AllocateTunnelEndpointsResponse): TunnelPlanForm {
  return {
    ...form,
    ...(response.ipv4_tunnel ? {
      includeIpv4: true,
      ipv4Prefix: String(response.ipv4_tunnel.prefix_len),
      leftIpv4: response.ipv4_tunnel.left,
      rightIpv4: response.ipv4_tunnel.right,
    } : {}),
    ...(response.ipv6_tunnel ? {
      includeIpv6: true,
      ipv6Prefix: String(response.ipv6_tunnel.prefix_len),
      leftIpv6: response.ipv6_tunnel.left,
      rightIpv6: response.ipv6_tunnel.right,
    } : {}),
  };
}

function definitionsForClient(definitions: NetworkAdapterDefinitionRecord[], clientId: string): NetworkAdapterDefinitionRecord[] {
  return clientId ? definitions : [];
}

function endpointOspfConfigurationSource(
  configurationSources: ConfigurationSourceView[],
  clientId: string,
): ConfigurationSourceView | null {
  return configurationSources.find(
    (source) =>
      source.client_id === clientId &&
      source.behavior === "ospf_update_command",
  ) ?? null;
}

function endpointOspfPresetOptionLabel(
  source: ConfigurationSourceView | null,
  evidenceState: "available" | "loading" | "unavailable",
): string {
  if (evidenceState === "loading") {
    return "Loading endpoint Configuration preset";
  }
  if (evidenceState === "unavailable") {
    return "Endpoint Configuration preset unavailable";
  }
  return source
    ? `Use endpoint preset: ${source.effective_preset_name}`
    : "Endpoint Configuration preset not returned";
}

function endpointOspfResolutionLabel(
  clientId: string,
  source: ConfigurationSourceView | null,
  evidenceState: "available" | "loading" | "unavailable",
  overrideDefinitionId: string | null | undefined,
  networkAdapterDefinitions: NetworkAdapterDefinitionRecord[],
): string {
  if (!clientId) {
    return "Select endpoint to resolve its command";
  }
  if (overrideDefinitionId) {
    const definition = networkAdapterDefinitions.find(
      (candidate) => candidate.id === overrideDefinitionId,
    );
    return `Per-plan override · ${definition?.name ?? shortId(overrideDefinitionId)}`;
  }
  if (evidenceState === "loading") {
    return "Loading effective VPS preset";
  }
  if (evidenceState === "unavailable") {
    return "Effective VPS preset unavailable";
  }
  if (!source) {
    return "No effective VPS preset returned";
  }
  return [
    source.effective_preset_name,
    source.selection_origin === "explicit_override"
      ? "VPS override"
      : "Inherited system default",
    readableTelemetryToken(source.readiness.state),
  ].join(" · ");
}

function addressPair(left: string, right: string, prefix: string): TunnelAddressPair {
  return { left: left.trim(), prefix_len: integerOr(prefix, 0), right: right.trim() };
}

function createConfirmationItems(
  request: CreateTunnelPlanRequest,
  existing: TunnelPlanRecord | undefined,
  networkAdapterDefinitions: NetworkAdapterDefinitionRecord[],
  configurationSources: ConfigurationSourceView[],
  configurationSourcesEvidenceState: "available" | "loading" | "unavailable",
) {
  const runtime = request.runtime_control;
  const topology = request.runtime_topology;
  const traffic = runtime?.traffic_limit;
  const adapterDefinitionName = (definitionId: string | null | undefined) =>
    networkAdapterDefinitions.find((definition) => definition.id === definitionId)?.name
      ?? (definitionId ? shortId(definitionId) : "not bound");
  const trafficSummary = [
    traffic?.ingress_kbps ? `ingress ${traffic.ingress_kbps} Kbps` : null,
    traffic?.egress_kbps ? `egress ${traffic.egress_kbps} Kbps` : null,
    traffic?.burst_kb ? `burst ${traffic.burst_kb} KiB` : null,
  ].filter(Boolean).join(", ") || "No shaping";
  const cleanupSummary = [
    `${topology?.desired_interfaces?.length ?? 0} desired interfaces`,
    `${topology?.stale_interfaces?.length ?? 0} stale interfaces`,
    `${topology?.routes?.length ?? 0} desired routes`,
    `${topology?.stale_routes?.length ?? 0} stale routes`,
  ].join("; ");
  return [
    { label: "Operation", value: existing ? `Update declaration ${existing.id} at revision ${existing.revision}` : "Create declaration" },
    { label: "Plan", value: `${request.name} · ${formatTunnelKind(request.kind)} · ${request.interface_name}` },
    { label: "Endpoints", value: `${request.left_client_id} / ${request.right_client_id}` },
    { label: "Left outer path", value: formatEndpointUnderlay(request.left_local_underlay, request.left_remote_underlay) },
    { label: "Right outer path", value: formatEndpointUnderlay(request.right_local_underlay, request.right_remote_underlay) },
    { label: "Runtime owner", value: runtimeManagerLabel(request.runtime_control?.manager) },
    ...(runtime?.manager === "external_managed_adapter" ? [{
      label: "Runtime adapters",
      value: `${adapterDefinitionName(runtime.left_adapter_template_id)} / ${adapterDefinitionName(runtime.right_adapter_template_id)}`,
    }] : []),
    { label: "Traffic policy", value: trafficSummary },
    ...(runtime?.manager === "agent_iproute2_managed" ? [{
      label: "Routes and cleanup",
      value: cleanupSummary,
    }] : []),
    { label: "Addresses", value: [request.ipv4_tunnel ? `${request.ipv4_tunnel.left}/${request.ipv4_tunnel.prefix_len} / ${request.ipv4_tunnel.right}/${request.ipv4_tunnel.prefix_len}` : null, request.ipv6_tunnel ? `${request.ipv6_tunnel.left}/${request.ipv6_tunnel.prefix_len} / ${request.ipv6_tunnel.right}/${request.ipv6_tunnel.prefix_len}` : null].filter(Boolean).join("; ") },
    { label: "Planning bandwidth", value: `${request.bandwidth_mbps} Mbps` },
    { label: "OSPF", value: request.ospf ? `${formatOspfMode(request.ospf.mode)} · planned cost ${calculateOspfCostPreview({ bandwidthMbps: request.bandwidth_mbps, latencyMs: request.ospf.planned_latency_ms, packetLossRatio: request.ospf.planned_packet_loss_ratio, policy: request.ospf.policy, preference: request.ospf.preference })}` : "Off" },
    ...(request.ospf ? [
      {
        label: "OSPF command overrides",
        value: `${endpointOspfResolutionLabel(
          request.left_client_id,
          endpointOspfConfigurationSource(
            configurationSources,
            request.left_client_id,
          ),
          configurationSourcesEvidenceState,
          request.ospf.left_adapter_template_id,
          networkAdapterDefinitions,
        )} / ${endpointOspfResolutionLabel(
          request.right_client_id,
          endpointOspfConfigurationSource(
            configurationSources,
            request.right_client_id,
          ),
          configurationSourcesEvidenceState,
          request.ospf.right_adapter_template_id,
          networkAdapterDefinitions,
        )}`,
      },
      {
        label: "OSPF gates",
        value: `minimum delta ${request.ospf.min_cost_delta}; ${request.ospf.healthy_windows} consecutive healthy probes for automatic mode`,
      },
    ] : []),
    { label: "Activation", value: request.enabled ? "Enable and push to both endpoints" : "Save disabled" },
  ];
}

function tunnelPlanFormMatchesInitial(
  form: TunnelPlanForm,
  initialPlan: TunnelPlanRecord,
): boolean {
  try {
    return JSON.stringify(buildTunnelPlanRequest(form))
      === JSON.stringify(buildTunnelPlanRequest(tunnelPlanFormFromRecord(initialPlan)));
  } catch {
    return false;
  }
}

function tunnelPlanSaveError(error: unknown): string {
  if (error instanceof ApiResponseError) {
    if (error.code === "tunnel_plan_snapshot_stale") {
      return "This tunnel plan changed after the editor opened. Close and reopen the editor before reviewing the current declaration.";
    }
    if (error.code === "tunnel_plan_name_conflict") {
      return "A tunnel plan with this name already exists. Edit that declaration from the registry.";
    }
    if (error.code === "tunnel_plan_endpoint_agent_not_found") {
      return "One of these VPS endpoints was deleted after the editor opened. Close the editor and review the current inventory before creating or updating this declaration.";
    }
    if (error.code === "tunnel_plan_interface_conflict") {
      return "Another saved plan already uses this interface on one of the selected VPSs.";
    }
    if (error.code === "tunnel_plan_address_conflict") {
      return "Another saved plan already uses one of these tunnel endpoint addresses.";
    }
  }
  return error instanceof Error ? error.message : "Tunnel plan save failed";
}

function tunnelPlanDeleteError(error: unknown): string {
  if (error instanceof ApiResponseError) {
    if (error.code === "tunnel_plan_snapshot_stale") {
      return "This tunnel plan changed after the delete review opened. Refresh the registry and review it again.";
    }
  }
  return error instanceof Error ? error.message : "Tunnel plan deletion failed";
}

function formatTunnelAddresses(plan: TunnelPlanRecord): string {
  const values = [plan.plan.ipv4_tunnel, plan.plan.ipv6_tunnel]
    .filter((pair): pair is TunnelAddressPair => Boolean(pair))
    .map((pair) => `${pair.left}/${pair.prefix_len} / ${pair.right}/${pair.prefix_len}`);
  return values.join("; ") || "No addresses";
}

function formatEndpointUnderlay(
  local: string | null | undefined,
  remote: string,
): string {
  return `Source ${local?.trim() || "automatic"} -> destination ${remote}`;
}

function formatRuntimeBinding(plan: TunnelPlanRecord): string {
  const control = plan.plan.runtime_control;
  if (!control || control.manager === "agent_iproute2_managed") return "Agent iproute2";
  if (control.manager === "external_observed") return "External observed; no mutation";
  return `External adapters ${shortId(control.left_adapter_template_id ?? "missing")} / ${shortId(control.right_adapter_template_id ?? "missing")}`;
}

function formatPlanOspf(plan: TunnelPlanRecord): string {
  const ospf = plan.plan.ospf;
  if (!ospf) return "Off";
  return `${formatOspfMode(ospf.mode)} · cost ${plan.recommended_ospf_cost ?? "unknown"} · ${readableTelemetryToken(plan.ospf_status)}`;
}

function formatTunnelKind(kind: TunnelKind): string {
  const labels: Record<TunnelKind, string> = {
    custom: "Custom",
    fou: "FOU",
    gre: "GRE",
    ipip: "IPIP",
    openvpn: "OpenVPN",
    sit: "SIT",
    tun_tap: "TUN/TAP",
    wireguard: "WireGuard",
  };
  return labels[kind];
}

function formatOspfMode(mode: string): string {
  return mode === "automatic" ? "Automatic" : "Reviewed";
}

function matchesActionableOspfStatus(status: string): boolean {
  return [
    "adapter_unavailable",
    "needs_adapter_status",
    "review_degraded",
    "review_planned_baseline",
    "review_required",
    "automatic_waiting_evidence",
  ].includes(status);
}

function numberOr(value: string, fallback: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function integerOr(value: string, fallback: number): number {
  return Math.trunc(numberOr(value, fallback));
}

function safeFilename(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "tunnel-plan";
}

type TunnelPlanForm = {
  bandwidthMbps: string;
  bandwidthWeight: string;
  burstKb: string;
  desiredInterfaces: string;
  egressKbps: string;
  enabled: boolean;
  fouIpProto: string;
  fouPeerPort: string;
  fouPort: string;
  healthyWindows: string;
  includeIpv4: boolean;
  includeIpv6: boolean;
  ingressKbps: string;
  interfaceName: string;
  ipv4Pool: string;
  ipv4Prefix: string;
  ipv6Pool: string;
  ipv6Prefix: string;
  kind: TunnelKind;
  latencyWeight: string;
  latencyPrimaryFamily: "ipv4" | "ipv6";
  leftClientId: string;
  leftIpv4: string;
  leftIpv6: string;
  leftRoutingDefinitionId: string;
  leftRuntimeDefinitionId: string;
  leftLocalUnderlay: string;
  leftRemoteUnderlay: string;
  lossWeight: string;
  maxCost: string;
  minCost: string;
  minCostDelta: string;
  name: string;
  ospfEnabled: boolean;
  ospfMode: "reviewed" | "automatic";
  packetLossPercent: string;
  plannedLatencyMs: string;
  preference: string;
  preferenceBias: string;
  rightClientId: string;
  rightIpv4: string;
  rightIpv6: string;
  rightRoutingDefinitionId: string;
  rightRuntimeDefinitionId: string;
  rightLocalUnderlay: string;
  rightRemoteUnderlay: string;
  routes: string;
  runtimeManager: RuntimeTunnelManager;
  runtimeTopologyVersion: string;
  staleInterfaces: string;
  staleRoutes: string;
};

type Feedback = { message: string; tone: ActionFeedbackTone };
type LifecycleSnapshot = {
  enabled: boolean;
  retryApply: boolean;
  retryCleanup: boolean;
  targets: Array<TunnelPlanRevisionTarget & {
    declaration: string;
    name: string;
    ospfEnabled: boolean;
  }>;
};
type DeleteSnapshot = {
  plan: TunnelPlanRecord;
  target: TunnelPlanRevisionTarget;
};

type TopologyPanelProps = {
  activeSubpage: string;
  agents: AgentView[];
  configurationSources: ConfigurationSourceView[];
  configurationSourcesEvidenceState: "available" | "loading" | "unavailable";
  error: string | null;
  initialAdapterKind: NetworkAdapterKind | null;
  initialPlanWorkflow: "create" | null;
  initialTargetIntent?: {
    clientId: string;
    requestId: string;
  } | null;
  jobs: JobHistoryRecord[];
  loading: boolean;
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  onAllocateTunnelEndpoints: (request: AllocateTunnelEndpointsRequest) => Promise<AllocateTunnelEndpointsResponse>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateTunnelPlan: (request: CreateTunnelPlanRequest) => Promise<TunnelPlanMutationResponse>;
  onCreateNetworkAdapterDefinition: (
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  onDeleteNetworkAdapterDefinition: (definitionId: string) => Promise<void>;
  onDeleteTunnelPlan: (target: TunnelPlanRevisionTarget) => Promise<TunnelPlanMutationResponse>;
  onExportTunnelPlan: (planId: string) => Promise<TunnelPlan>;
  onInitialPlanWorkflowConsumed: () => void;
  onInitialAdapterKindConsumed: () => void;
  onInitialTargetIntentConsumed?: (requestId: string) => void;
  onLoadRuntimeConfigApplyStates: () => Promise<void>;
  onLoadConfigurationSources: () => Promise<void>;
  onLoadNetworkObservations: () => Promise<void>;
  onLoadNetworkTrends: () => Promise<void>;
  onLoadOspfRecommendations: () => Promise<void>;
  onLoadOspfUpdatePlans: () => Promise<void>;
  onLoadNetworkAdapterDefinitions: () => Promise<void>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onLoadTopologyGraph: () => Promise<void>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenCreateTunnelPlan: () => void;
  onOpenConfigurationSources: () => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenAdapterDefinitions: (domain: NetworkAdapterKind) => void;
  onOpenVpsDetail?: (clientId: string) => void;
  onBulkMutatePortForwardRules: (
    action: import("../types").PortForwardBulkAction,
    items: Array<{ id: string; expected_revision: number }>,
    reason?: string,
  ) => Promise<import("../types").PortForwardBulkResponse>;
  onCreatePortForwardRule: (
    request: import("../types").CreatePortForwardRuleRequest,
  ) => Promise<import("../types").PortForwardMutationResponse>;
  onLoadPortForwardRules: () => Promise<void>;
  onMutatePortForwardRule: (
    ruleId: string,
    operation: "enable" | "disable" | "delete" | "forget" | "reapply",
    request: import("../types").PortForwardMutationRequest,
  ) => Promise<import("../types").PortForwardMutationResponse>;
  onResolvePortForwardHostname: (
    hostname: string,
  ) => Promise<import("../types").ResolveHostnameResponse>;
  onUpdatePortForwardRule: (
    ruleId: string,
    request: import("../types").UpdatePortForwardRuleRequest,
  ) => Promise<import("../types").PortForwardMutationResponse>;
  onRefresh: () => Promise<void>;
  onRefreshTunnelPlanOspfStatus: (planId: string) => Promise<TunnelPlanOspfJobsResponse>;
  onSelectSubpage: (subpage: string) => void;
  onSetTunnelPlanEnabled: (targets: TunnelPlanRevisionTarget[], enabled: boolean) => Promise<TunnelPlanMutationResponse[]>;
  onUpdateTunnelConnectionAssessment: (planId: string, request: UpdateTunnelConnectionAssessmentRequest) => Promise<void>;
  onUpdateTunnelPlanOspfCost: (planId: string, request: UpdateTunnelPlanOspfCostRequest) => Promise<TunnelPlanOspfJobsResponse>;
  onUpdateTunnelPlan: (planId: string, request: UpdateTunnelPlanRequest) => Promise<TunnelPlanMutationResponse>;
  onUpdateNetworkAdapterDefinition: (
    definitionId: string,
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  operator: OperatorView | null;
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  portForwardError: string | null;
  portForwardLoading: boolean;
  portForwardRules: import("../types").PortForwardRuleListItem[];
  privilegeMaterial: PrivilegeMaterial | null;
  runtimeConfigEvidenceState: "available" | "loading" | "unavailable";
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  networkAdapterDefinitions: NetworkAdapterDefinitionRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  topologyGraph: TopologyGraph;
  tunnelPlanCorruptions: import("../types").TunnelPlanCorruptRecord[];
  tunnelPlans: TunnelPlanRecord[];
};
