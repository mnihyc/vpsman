import { useMemo, useState, type FormEvent } from "react";
import { Network, Save, ShieldCheck } from "lucide-react";
import {
  buildRuntimeControl,
  buildRuntimeTopology,
  isDefaultRuntimeTopology,
  normalizeTelemetryPromotionRequest,
} from "../../topologyRuntime";
import type {
  AgentView,
  BandwidthTier,
  PromoteTelemetryTunnelRequest,
  TelemetryTunnelRecord,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import type { PromoteTunnelPlanToAdapterRequest } from "../../typesTopology";
import { runPanelAction, shortId } from "../../utils";

const bandwidthTiers: BandwidthTier[] = ["10m", "100m", "1000m"];

type AdapterPromotionForm = {
  planId: string;
  name: string;
  startupArgv: string;
  stopArgv: string;
  cleanupArgv: string;
  restartArgv: string;
  statusArgv: string;
  trafficArgv: string;
  trafficIngressKbps: string;
  trafficEgressKbps: string;
  trafficBurstKb: string;
  topologyVersion: string;
  topologyDesiredText: string;
  topologyStaleText: string;
  topologyRoutesText: string;
  topologyStaleRoutesText: string;
  confirmed: boolean;
};

export function TopologyPromotionPanel({
  agents,
  onPromoteTelemetryTunnel,
  onPromoteTunnelPlanToAdapter,
  telemetryTunnels,
  tunnelPlans,
}: {
  agents: AgentView[];
  onPromoteTelemetryTunnel: (request: PromoteTelemetryTunnelRequest) => Promise<void>;
  onPromoteTunnelPlanToAdapter: (request: PromoteTunnelPlanToAdapterRequest) => Promise<void>;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const [promoteForm, setPromoteForm] = useState<PromoteTelemetryTunnelRequest>({
    client_id: "",
    interface: "",
    peer_client_id: "",
    local_underlay: "",
    peer_underlay: "",
    address_pool_cidr: "10.255.0.0/30",
    side: "left",
    bandwidth: "100m",
    latency_ms: 20,
    packet_loss_ratio: 0,
    preference: 1,
  });
  const [adapterForm, setAdapterForm] = useState<AdapterPromotionForm>({
    planId: "",
    name: "",
    startupArgv: "",
    stopArgv: "",
    cleanupArgv: "",
    restartArgv: "",
    statusArgv: "",
    trafficArgv: "",
    trafficIngressKbps: "",
    trafficEgressKbps: "",
    trafficBurstKb: "",
    topologyVersion: "",
    topologyDesiredText: "",
    topologyStaleText: "",
    topologyRoutesText: "",
    topologyStaleRoutesText: "",
    confirmed: false,
  });
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const importCandidates = useMemo(
    () =>
      telemetryTunnels
        .filter((tunnel) => tunnel.promotion_required)
        .sort((left, right) => `${left.client_id}:${left.interface}`.localeCompare(`${right.client_id}:${right.interface}`)),
    [telemetryTunnels],
  );
  const observedPlans = useMemo(
    () =>
      tunnelPlans
        .filter((plan) => plan.plan.runtime_control?.manager === "external_observed")
        .sort((left, right) => left.name.localeCompare(right.name)),
    [tunnelPlans],
  );
  const selectedCandidateKey =
    promoteForm.client_id && promoteForm.interface ? `${promoteForm.client_id}:${promoteForm.interface}` : "";
  const selectedObservedPlan = observedPlans.find((plan) => plan.id === adapterForm.planId) ?? null;
  const promotionReady =
    promoteForm.client_id &&
    promoteForm.interface &&
    promoteForm.peer_client_id &&
    promoteForm.client_id !== promoteForm.peer_client_id &&
    promoteForm.local_underlay.trim() &&
    promoteForm.peer_underlay.trim() &&
    promoteForm.address_pool_cidr.trim();
  const adapterPromotionReady = selectedObservedPlan && adapterForm.statusArgv.trim() && adapterForm.confirmed;
  const status =
    actionError ??
    `${importCandidates.length} telemetry imports / ${observedPlans.length} observed plans`;

  async function submitTelemetryPromotion(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!promotionReady) {
        throw new Error("Promotion request is incomplete");
      }
      await onPromoteTelemetryTunnel(normalizeTelemetryPromotionRequest(promoteForm));
    });
  }

  async function submitAdapterPromotion(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedObservedPlan) {
        throw new Error("Select an observed tunnel plan");
      }
      if (!adapterForm.statusArgv.trim()) {
        throw new Error("Adapter status argv is required");
      }
      if (!adapterForm.confirmed) {
        throw new Error("Adapter promotion requires confirmation");
      }
      const runtimeTopology = buildRuntimeTopology({
        version: adapterForm.topologyVersion,
        desiredText: adapterForm.topologyDesiredText,
        staleText: adapterForm.topologyStaleText,
        routesText: adapterForm.topologyRoutesText,
        staleRoutesText: adapterForm.topologyStaleRoutesText,
      });
      await onPromoteTunnelPlanToAdapter({
        plan_id: selectedObservedPlan.id,
        runtime_control: buildRuntimeControl("external_managed_adapter", {
          startup: adapterForm.startupArgv,
          stop: adapterForm.stopArgv,
          cleanup: adapterForm.cleanupArgv,
          restart: adapterForm.restartArgv,
          status: adapterForm.statusArgv,
          traffic: adapterForm.trafficArgv,
          ingressKbps: adapterForm.trafficIngressKbps,
          egressKbps: adapterForm.trafficEgressKbps,
          burstKb: adapterForm.trafficBurstKb,
          fouPort: "",
          fouPeerPort: "",
          fouIpproto: "",
        }),
        runtime_topology: isDefaultRuntimeTopology(runtimeTopology) ? undefined : runtimeTopology,
        name: adapterForm.name.trim() || undefined,
        confirmed: true,
      });
    });
  }

  return (
    <section className="fleetPanel scheduleComposer">
      <div className="sectionHeader">
        <div>
          <h2>Tunnel promotion</h2>
          <span>{status}</span>
        </div>
        <Network size={20} />
      </div>
      <div className="dispatchControls">
        <form className="dispatchForm" onSubmit={submitTelemetryPromotion}>
          <div className="sectionHeader compactHeader">
            <div>
              <h3>Observed import</h3>
              <span>Save telemetry candidate</span>
            </div>
            <Save size={18} />
          </div>
          <div className="dispatchControls">
            <label>
              <span>Observed interface</span>
              <select value={selectedCandidateKey} onChange={(event) => selectPromotionCandidate(event.target.value)}>
                <option value="">Select</option>
                {importCandidates.map((candidate) => (
                  <option key={`${candidate.client_id}:${candidate.interface}`} value={`${candidate.client_id}:${candidate.interface}`}>
                    {candidate.client_id} / {candidate.interface} / {candidate.kind}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Peer VPS</span>
              <select
                value={promoteForm.peer_client_id}
                onChange={(event) => setPromotionField("peer_client_id", event.target.value)}
              >
                <option value="">Select</option>
                {agents
                  .filter((agent) => agent.id !== promoteForm.client_id)
                  .map((agent) => (
                    <option key={agent.id} value={agent.id}>
                      {agent.display_name || agent.id}
                    </option>
                  ))}
              </select>
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Name</span>
              <input value={promoteForm.name ?? ""} onChange={(event) => setPromotionField("name", event.target.value)} />
            </label>
            <label>
              <span>Side</span>
              <select
                value={promoteForm.side ?? "left"}
                onChange={(event) => setPromotionField("side", event.target.value as TunnelEndpointSide)}
              >
                <option value="left">Left</option>
                <option value="right">Right</option>
              </select>
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Local underlay</span>
              <input
                value={promoteForm.local_underlay}
                onChange={(event) => setPromotionField("local_underlay", event.target.value)}
              />
            </label>
            <label>
              <span>Peer underlay</span>
              <input
                value={promoteForm.peer_underlay}
                onChange={(event) => setPromotionField("peer_underlay", event.target.value)}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Address pool</span>
              <input
                value={promoteForm.address_pool_cidr}
                onChange={(event) => setPromotionField("address_pool_cidr", event.target.value)}
              />
            </label>
            <label>
              <span>Bandwidth</span>
              <select
                value={promoteForm.bandwidth ?? "100m"}
                onChange={(event) => setPromotionField("bandwidth", event.target.value as BandwidthTier)}
              >
                {bandwidthTiers.map((tier) => (
                  <option key={tier} value={tier}>
                    {tier}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Latency ms</span>
              <input
                min={0}
                onChange={(event) => setPromotionField("latency_ms", Number(event.target.value))}
                type="number"
                value={promoteForm.latency_ms ?? 20}
              />
            </label>
            <label>
              <span>Preference</span>
              <input
                min={0.1}
                onChange={(event) => setPromotionField("preference", Number(event.target.value))}
                step={0.1}
                type="number"
                value={promoteForm.preference ?? 1}
              />
            </label>
          </div>
          <button className="primaryAction" disabled={pending || !promotionReady} type="submit">
            <Save size={17} />
            Save observed plan
          </button>
        </form>

        <form className="dispatchForm" onSubmit={submitAdapterPromotion}>
          <div className="sectionHeader compactHeader">
            <div>
              <h3>Adapter contract</h3>
              <span>Promote saved observed plan</span>
            </div>
            <ShieldCheck size={18} />
          </div>
          <div className="dispatchControls">
            <label>
              <span>Observed plan</span>
              <select value={adapterForm.planId} onChange={(event) => selectAdapterPlan(event.target.value)}>
                <option value="">Select</option>
                {observedPlans.map((plan) => (
                  <option key={plan.id} value={plan.id}>
                    {plan.name} / {shortId(plan.id)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Name</span>
              <input value={adapterForm.name} onChange={(event) => setAdapterField("name", event.target.value)} />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Status argv</span>
              <textarea value={adapterForm.statusArgv} onChange={(event) => setAdapterField("statusArgv", event.target.value)} />
            </label>
            <label>
              <span>Start argv</span>
              <textarea value={adapterForm.startupArgv} onChange={(event) => setAdapterField("startupArgv", event.target.value)} />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Restart argv</span>
              <textarea value={adapterForm.restartArgv} onChange={(event) => setAdapterField("restartArgv", event.target.value)} />
            </label>
            <label>
              <span>Stop argv</span>
              <textarea value={adapterForm.stopArgv} onChange={(event) => setAdapterField("stopArgv", event.target.value)} />
            </label>
          </div>
          <label>
            <span>Cleanup argv</span>
            <textarea value={adapterForm.cleanupArgv} onChange={(event) => setAdapterField("cleanupArgv", event.target.value)} />
          </label>
          <label>
            <span>Traffic argv</span>
            <textarea value={adapterForm.trafficArgv} onChange={(event) => setAdapterField("trafficArgv", event.target.value)} />
          </label>
          <div className="dispatchControls">
            <label>
              <span>Egress Kbps</span>
              <input
                min={64}
                onChange={(event) => setAdapterField("trafficEgressKbps", event.target.value)}
                type="number"
                value={adapterForm.trafficEgressKbps}
              />
            </label>
            <label>
              <span>Ingress Kbps</span>
              <input
                min={64}
                onChange={(event) => setAdapterField("trafficIngressKbps", event.target.value)}
                type="number"
                value={adapterForm.trafficIngressKbps}
              />
            </label>
            <label>
              <span>Burst KB</span>
              <input
                min={1}
                onChange={(event) => setAdapterField("trafficBurstKb", event.target.value)}
                type="number"
                value={adapterForm.trafficBurstKb}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Topology version</span>
              <input value={adapterForm.topologyVersion} onChange={(event) => setAdapterField("topologyVersion", event.target.value)} />
            </label>
            <label>
              <span>Desired interfaces</span>
              <input
                value={adapterForm.topologyDesiredText}
                onChange={(event) => setAdapterField("topologyDesiredText", event.target.value)}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Stale interfaces</span>
              <input
                value={adapterForm.topologyStaleText}
                onChange={(event) => setAdapterField("topologyStaleText", event.target.value)}
              />
            </label>
            <label>
              <span>Routes</span>
              <textarea
                value={adapterForm.topologyRoutesText}
                onChange={(event) => setAdapterField("topologyRoutesText", event.target.value)}
              />
            </label>
          </div>
          <label>
            <span>Stale routes</span>
            <textarea
              value={adapterForm.topologyStaleRoutesText}
              onChange={(event) => setAdapterField("topologyStaleRoutesText", event.target.value)}
            />
          </label>
          <label className="inlineCheck">
            <input
              checked={adapterForm.confirmed}
              onChange={(event) => setAdapterField("confirmed", event.target.checked)}
              type="checkbox"
            />
            <span>Confirmed</span>
          </label>
          <button className="primaryAction" disabled={pending || !adapterPromotionReady} type="submit">
            <ShieldCheck size={17} />
            Promote adapter
          </button>
        </form>
      </div>
    </section>
  );

  function setPromotionField<K extends keyof PromoteTelemetryTunnelRequest>(
    key: K,
    value: PromoteTelemetryTunnelRequest[K],
  ) {
    setPromoteForm((current) => ({ ...current, [key]: value }));
  }

  function setAdapterField<K extends keyof AdapterPromotionForm>(key: K, value: AdapterPromotionForm[K]) {
    setAdapterForm((current) => ({ ...current, [key]: value }));
  }

  function selectPromotionCandidate(value: string) {
    const candidate = importCandidates.find((tunnel) => `${tunnel.client_id}:${tunnel.interface}` === value);
    if (!candidate) {
      setPromoteForm((current) => ({ ...current, client_id: "", interface: "" }));
      return;
    }
    setPromoteForm((current) => ({
      ...current,
      client_id: candidate.client_id,
      interface: candidate.interface,
      name: current.name || `${candidate.client_id}-${candidate.interface}-observed`,
      topology_version: current.topology_version || `telemetry-import:${candidate.interface}`,
    }));
  }

  function selectAdapterPlan(planId: string) {
    const plan = observedPlans.find((candidate) => candidate.id === planId);
    if (!plan) {
      setAdapterForm((current) => ({ ...current, planId: "" }));
      return;
    }
    setAdapterForm((current) => ({
      ...current,
      planId,
      name: current.name || `${plan.name}-adapter`,
      topologyVersion: current.topologyVersion || `adapter:${plan.plan.interface_name}`,
      topologyDesiredText: current.topologyDesiredText || plan.plan.interface_name,
    }));
  }
}
