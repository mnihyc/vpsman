import { useMemo, useState, type FormEvent } from "react";
import { GitBranch, RefreshCcw, Route, Save } from "lucide-react";
import { usePanelDisplaySettings } from "../panelDisplay";
import { buildRuntimeControl, buildRuntimeTopology, runtimeManagerLabel } from "../topologyRuntime";
import type {
  AgentView,
  BandwidthTier,
  CreateJobRequest,
  CreateJobResponse,
  CreateTunnelPlanRequest,
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
  TopologyGraph,
  TunnelKind,
  TunnelPlanRecord,
} from "../types";
import type { PromoteTunnelPlanToAdapterRequest } from "../typesTopology";
import type { PrivilegeMaterial } from "../privilege";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatTime,
  formatVpsName,
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
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
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
    form.address_pool_cidr.trim();
  const status = actionError ?? error ?? (loading ? "Loading" : `${tunnelPlans.length} plans`);

  async function submitPlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!ready) {
        throw new Error("Tunnel plan is incomplete");
      }
      await onCreateTunnelPlan({
        ...form,
        reserved_addresses: reservedText
          .split(",")
          .map((value) => value.trim())
          .filter(Boolean),
        runtime_control: buildRuntimeControl(runtimeManager, {
          startup: runtimeStartupArgv,
          stop: runtimeStopArgv,
          cleanup: runtimeCleanupArgv,
          restart: runtimeRestartArgv,
          status: runtimeStatusArgv,
          traffic: runtimeTrafficArgv,
          ingressKbps: trafficIngressKbps,
          egressKbps: trafficEgressKbps,
          burstKb: trafficBurstKb,
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

  return (
    <div className="workspaceGrid">
      {(topologySubpage === "graph" || topologySubpage === "plans") && (
      <section className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Tunnel plans</h2>
            <span>{status}</span>
          </div>
          <button className="secondaryAction" disabled={loading || pending} onClick={onRefresh} type="button">
            <RefreshCcw size={17} />
            Refresh
          </button>
        </div>
        <div className="tableWrap">
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Kind</th>
                <th>Runtime</th>
                <th>Endpoints</th>
                <th>Endpoint state</th>
                <th>OSPF</th>
                <th>State</th>
                <th>Updated</th>
              </tr>
            </thead>
            <tbody>
              {tunnelPlans.map((plan) => (
                <tr key={plan.id}>
                  <td>
                    <strong>{plan.name}</strong>
                    <span className="mutedCell">{shortId(plan.id)}</span>
                  </td>
                  <td>{plan.kind.toUpperCase()}</td>
                  <td>{runtimeManagerLabel(plan.plan.runtime_control?.manager)}</td>
                  <td>
                    {clientLabel(plan.left_client_id)} / {clientLabel(plan.right_client_id)}
                  </td>
                  <td>
                    <span className="mutedCell">
                      L {plan.left_status} / R {plan.right_status}
                    </span>
                  </td>
                  <td>{plan.recommended_ospf_cost}</td>
                  <td>
                    <span className={`status ${plan.status.includes("applied") ? "ok" : plan.status.includes("rolled_back") ? "warn" : "neutral"}`}>
                      {plan.status}
                    </span>
                  </td>
                  <td>{formatTime(plan.updated_at ?? plan.created_at)}</td>
                </tr>
              ))}
              {tunnelPlans.length === 0 && (
                <tr>
                  <td colSpan={8}>No tunnel plans</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
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
      <section className="fleetPanel scheduleComposer">
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
                    {tier}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="operationNote formSectionNote">
            <strong>Endpoints and address plan</strong>
            <span>Pair exactly two VPSs, provide their underlay addresses, then reserve the overlay address pool.</span>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Left VPS</span>
              <select value={form.left_client_id} onChange={(event) => setField("left_client_id", event.target.value)}>
                <option value="">Select</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {formatVpsName(agent, vpsNameDisplayMode)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Right VPS</span>
              <select value={form.right_client_id} onChange={(event) => setField("right_client_id", event.target.value)}>
                <option value="">Select</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {formatVpsName(agent, vpsNameDisplayMode)}
                  </option>
                ))}
              </select>
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
              <span>Address pool</span>
              <input
                value={form.address_pool_cidr}
                onChange={(event) => setField("address_pool_cidr", event.target.value)}
              />
            </label>
            <label>
              <span>Reserved</span>
              <input value={reservedText} onChange={(event) => setReservedText(event.target.value)} />
            </label>
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
          <div className="dispatchControls">
            <label>
              <span>Egress Kbps</span>
              <input
                min={64}
                onChange={(event) => setTrafficEgressKbps(event.target.value)}
                type="number"
                value={trafficEgressKbps}
              />
            </label>
            <label>
              <span>Ingress Kbps</span>
              <input
                min={64}
                onChange={(event) => setTrafficIngressKbps(event.target.value)}
                type="number"
                value={trafficIngressKbps}
              />
            </label>
            <label>
              <span>Burst KB</span>
              <input min={1} onChange={(event) => setTrafficBurstKb(event.target.value)} type="number" value={trafficBurstKb} />
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
}
