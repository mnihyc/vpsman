import { useMemo, useState, type FormEvent } from "react";
import { Network, Save, ShieldCheck, Wand2 } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { VpsCombobox } from "../../components/VpsCombobox";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  buildRuntimeControl,
  buildRuntimeTopology,
  isDefaultRuntimeTopology,
  normalizeTelemetryPromotionRequest,
} from "../../topologyRuntime";
import type {
  AgentView,
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
  BandwidthTier,
  PromoteTelemetryTunnelRequest,
  TelemetryTunnelRecord,
  TunnelAddressFamily,
  TunnelAddressPair,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import type { PromoteTunnelPlanToAdapterRequest } from "../../typesTopology";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  runPanelAction,
  shortId,
} from "../../utils";

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
};

export function TopologyPromotionPanel({
  agents,
  onAllocateTunnelEndpoints,
  onPromoteTelemetryTunnel,
  onPromoteTunnelPlanToAdapter,
  telemetryTunnels,
  tunnelPlans,
}: {
  agents: AgentView[];
  onAllocateTunnelEndpoints: (request: AllocateTunnelEndpointsRequest) => Promise<AllocateTunnelEndpointsResponse>;
  onPromoteTelemetryTunnel: (request: PromoteTelemetryTunnelRequest) => Promise<void>;
  onPromoteTunnelPlanToAdapter: (request: PromoteTunnelPlanToAdapterRequest) => Promise<void>;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [promoteForm, setPromoteForm] = useState<PromoteTelemetryTunnelRequest>({
    client_id: "",
    interface: "",
    peer_client_id: "",
    local_underlay: "",
    peer_underlay: "",
    address_pool_cidr: "10.255.0.0/30",
    ipv4_tunnel: null,
    ipv6_address_pool_cidr: "",
    ipv6_tunnel: null,
    latency_primary_family: "ipv4",
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
  });
  const [adapterTrafficLimitEnabled, setAdapterTrafficLimitEnabled] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [adapterConfirmationOpen, setAdapterConfirmationOpen] = useState(false);
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
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
    hasPromotionAddressSource(promoteForm);
  const adapterPromotionReady = selectedObservedPlan && adapterForm.statusArgv.trim();
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

  function submitAdapterPromotion(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setAdapterConfirmationOpen(true);
  }

  async function executeAdapterPromotion() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedObservedPlan) {
        throw new Error("Select an observed tunnel plan");
      }
      if (!adapterForm.statusArgv.trim()) {
        throw new Error("Adapter status argv is required");
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
          traffic: adapterTrafficLimitEnabled ? adapterForm.trafficArgv : "",
          ingressKbps: adapterTrafficLimitEnabled ? adapterForm.trafficIngressKbps : "",
          egressKbps: adapterTrafficLimitEnabled ? adapterForm.trafficEgressKbps : "",
          burstKb: adapterTrafficLimitEnabled ? adapterForm.trafficBurstKb : "",
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

  async function allocatePromotionEndpoints() {
    await runPanelAction(setPending, setActionError, async () => {
      const includeIpv4 = Boolean(promoteForm.address_pool_cidr.trim());
      const includeIpv6 = Boolean((promoteForm.ipv6_address_pool_cidr ?? "").trim());
      if (!includeIpv4 && !includeIpv6) {
        throw new Error("Enter an IPv4 or IPv6 pool before generating endpoints");
      }
      const allocation = await onAllocateTunnelEndpoints({
        ipv4_pool_cidr: includeIpv4 ? promoteForm.address_pool_cidr.trim() : null,
        ipv6_pool_cidr: includeIpv6 ? (promoteForm.ipv6_address_pool_cidr ?? "").trim() : null,
        include_ipv4: includeIpv4,
        include_ipv6: includeIpv6,
      });
      setPromoteForm((current) => ({
        ...current,
        ipv4_tunnel: allocation.ipv4_tunnel,
        ipv6_tunnel: allocation.ipv6_tunnel,
        latency_primary_family: allocation.latency_primary_family,
      }));
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
      <div className="promotionWorkflow">
        <ConfirmationPrompt
          confirmLabel="Promote adapter"
          detail="Confirm promoting the observed tunnel plan into an externally managed runtime adapter."
          items={[
            { label: "Plan", value: selectedObservedPlan?.name ?? "none" },
            { label: "Runtime", value: "external_managed_adapter" },
            { label: "Status argv", value: adapterForm.statusArgv.trim() || "missing" },
          ]}
          onCancel={() => setAdapterConfirmationOpen(false)}
          onConfirm={() => {
            setAdapterConfirmationOpen(false);
            void executeAdapterPromotion();
          }}
          open={adapterConfirmationOpen}
          pending={pending}
          title="Promote tunnel adapter"
        />
        <form className="dispatchForm promotionStageCard promotionImportCard" onSubmit={submitTelemetryPromotion}>
          <div className="sectionHeader compactHeader promotionStageHeader">
            <div>
              <small>Step 1</small>
              <h3>Observed import</h3>
              <span>Convert one telemetry candidate into a saved observed plan.</span>
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
                    {clientLabel(candidate.client_id)} / {candidate.interface} / {candidate.kind}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Peer VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Observed import peer VPS"
                excludeIds={promoteForm.client_id ? [promoteForm.client_id] : []}
                onChange={(value) => setPromotionField("peer_client_id", value)}
                placeholder="Search peer VPS"
                value={promoteForm.peer_client_id}
              />
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
                onChange={(event) => setPromotionSide(event.target.value as TunnelEndpointSide)}
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
              <span>IPv4 allocation pool</span>
              <input
                value={promoteForm.address_pool_cidr}
                onChange={(event) => setPromotionField("address_pool_cidr", event.target.value)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>IPv6 allocation pool</span>
              <input
                value={promoteForm.ipv6_address_pool_cidr ?? ""}
                onChange={(event) => setPromotionField("ipv6_address_pool_cidr", event.target.value)}
                placeholder="Empty means disabled"
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Self IPv4</span>
              <input
                value={promotionAddressValue(promoteForm.ipv4_tunnel ?? null, promoteForm.side ?? "left", "local")}
                onChange={(event) => setPromotionAddressPair("ipv4_tunnel", "local", event.target.value, 31)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>Peer IPv4</span>
              <input
                value={promotionAddressValue(promoteForm.ipv4_tunnel ?? null, promoteForm.side ?? "left", "peer")}
                onChange={(event) => setPromotionAddressPair("ipv4_tunnel", "peer", event.target.value, 31)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>IPv4 prefix</span>
              <input
                max={32}
                min={0}
                onChange={(event) => setPromotionAddressPrefix("ipv4_tunnel", Number(event.target.value), 31)}
                type="number"
                value={promoteForm.ipv4_tunnel?.prefix_len ?? 31}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Self IPv6</span>
              <input
                value={promotionAddressValue(promoteForm.ipv6_tunnel ?? null, promoteForm.side ?? "left", "local")}
                onChange={(event) => setPromotionAddressPair("ipv6_tunnel", "local", event.target.value, 127)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>Peer IPv6</span>
              <input
                value={promotionAddressValue(promoteForm.ipv6_tunnel ?? null, promoteForm.side ?? "left", "peer")}
                onChange={(event) => setPromotionAddressPair("ipv6_tunnel", "peer", event.target.value, 127)}
                placeholder="Empty means disabled"
              />
            </label>
            <label>
              <span>IPv6 prefix</span>
              <input
                max={128}
                min={0}
                onChange={(event) => setPromotionAddressPrefix("ipv6_tunnel", Number(event.target.value), 127)}
                type="number"
                value={promoteForm.ipv6_tunnel?.prefix_len ?? 127}
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Latency primary</span>
              <select
                value={promoteForm.latency_primary_family ?? "ipv4"}
                onChange={(event) => setPromotionField("latency_primary_family", event.target.value as TunnelAddressFamily)}
              >
                <option value="ipv4">IPv4</option>
                <option value="ipv6">IPv6</option>
              </select>
            </label>
            <button className="secondaryAction" disabled={pending} onClick={allocatePromotionEndpoints} type="button">
              <Wand2 size={17} />
              Generate endpoints
            </button>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Bandwidth</span>
              <select
                value={promoteForm.bandwidth ?? "100m"}
                onChange={(event) => setPromotionField("bandwidth", event.target.value as BandwidthTier)}
              >
                {bandwidthTiers.map((tier) => (
                  <option key={tier} value={tier}>
                    {bandwidthTierLabel(tier)}
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

        <form className="dispatchForm promotionStageCard promotionAdapterCard" onSubmit={submitAdapterPromotion}>
          <div className="sectionHeader compactHeader promotionStageHeader">
            <div>
              <small>Step 2</small>
              <h3>Adapter contract</h3>
              <span>Attach status/start/traffic commands to a saved observed plan.</span>
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
          <div className="promotionGroupLabel">
            <strong>Runtime commands</strong>
            <span>Status is required; other commands are optional lifecycle hooks.</span>
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
          <label className="checkLine">
            <input
              checked={adapterTrafficLimitEnabled}
              onChange={(event) => setAdapterTrafficLimitEnabled(event.target.checked)}
              type="checkbox"
            />
            <span>Enable traffic shaping</span>
          </label>
          <div className="dispatchControls">
            <label>
              <span>Egress Kbps</span>
              <input
                disabled={!adapterTrafficLimitEnabled}
                min={64}
                onChange={(event) => setAdapterField("trafficEgressKbps", event.target.value)}
                placeholder="Empty means disabled"
                type="number"
                value={adapterForm.trafficEgressKbps}
              />
            </label>
            <label>
              <span>Ingress Kbps</span>
              <input
                disabled={!adapterTrafficLimitEnabled}
                min={64}
                onChange={(event) => setAdapterField("trafficIngressKbps", event.target.value)}
                placeholder="Empty means disabled"
                type="number"
                value={adapterForm.trafficIngressKbps}
              />
            </label>
            <label>
              <span>Burst KB</span>
              <input
                disabled={!adapterTrafficLimitEnabled}
                min={1}
                onChange={(event) => setAdapterField("trafficBurstKb", event.target.value)}
                placeholder="Empty means disabled"
                type="number"
                value={adapterForm.trafficBurstKb}
              />
            </label>
          </div>
          <div className="promotionGroupLabel">
            <strong>Topology evidence</strong>
            <span>Optional desired/stale interface and route evidence for drift checks.</span>
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
          {!adapterConfirmationOpen && (
            <button className="primaryAction" disabled={pending || !adapterPromotionReady} type="submit">
              <ShieldCheck size={17} />
              Review promotion
            </button>
          )}
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

  function setPromotionSide(side: TunnelEndpointSide) {
    setPromoteForm((current) => {
      const previousSide = current.side ?? "left";
      return {
        ...current,
        side,
        ipv4_tunnel: remapPairForSide(current.ipv4_tunnel ?? null, previousSide, side),
        ipv6_tunnel: remapPairForSide(current.ipv6_tunnel ?? null, previousSide, side),
      };
    });
  }

  function setPromotionAddressPair(
    key: "ipv4_tunnel" | "ipv6_tunnel",
    role: "local" | "peer",
    value: string,
    fallbackPrefix: number,
  ) {
    setPromoteForm((current) => ({
      ...current,
      [key]: updatePairForSide(current[key] ?? null, current.side ?? "left", role, value, fallbackPrefix),
    }));
  }

  function setPromotionAddressPrefix(key: "ipv4_tunnel" | "ipv6_tunnel", value: number, fallbackPrefix: number) {
    setPromoteForm((current) => ({
      ...current,
      [key]: updatePairPrefix(current[key] ?? null, value, fallbackPrefix),
    }));
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
      name: current.name || `${clientLabel(candidate.client_id)}-${candidate.interface}-observed`,
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

function bandwidthTierLabel(tier: BandwidthTier): string {
  if (tier === "1000m") {
    return "1000m (1000 Mbps)";
  }
  if (tier === "100m") {
    return "100m (100 Mbps)";
  }
  return "10m (10 Mbps)";
}

function hasPromotionAddressSource(form: PromoteTelemetryTunnelRequest): boolean {
  return Boolean(completePairOrNull(form.ipv4_tunnel ?? null) || completePairOrNull(form.ipv6_tunnel ?? null));
}

function normalizePair(pair: TunnelAddressPair): TunnelAddressPair | null {
  if (!pair.left && !pair.right) {
    return null;
  }
  return pair;
}

function completePairOrNull(pair: TunnelAddressPair | null): TunnelAddressPair | null {
  if (!pair?.left || !pair.right) {
    return null;
  }
  return pair;
}

function promotionAddressValue(
  pair: TunnelAddressPair | null,
  side: TunnelEndpointSide,
  role: "local" | "peer",
): string {
  if (!pair) {
    return "";
  }
  return fieldForSide(side, role) === "left" ? pair.left : pair.right;
}

function updatePairForSide(
  pair: TunnelAddressPair | null,
  side: TunnelEndpointSide,
  role: "local" | "peer",
  value: string,
  fallbackPrefix: number,
): TunnelAddressPair | null {
  const field = fieldForSide(side, role);
  const nextPair = {
    left: pair?.left ?? "",
    right: pair?.right ?? "",
    prefix_len: pair?.prefix_len ?? fallbackPrefix,
    [field]: value.trim(),
  };
  return normalizePair(nextPair);
}

function updatePairPrefix(pair: TunnelAddressPair | null, value: number, fallbackPrefix: number): TunnelAddressPair | null {
  const nextPair = {
    left: pair?.left ?? "",
    right: pair?.right ?? "",
    prefix_len: Number.isFinite(value) ? Math.trunc(value) : fallbackPrefix,
  };
  return normalizePair(nextPair);
}

function remapPairForSide(
  pair: TunnelAddressPair | null,
  fromSide: TunnelEndpointSide,
  toSide: TunnelEndpointSide,
): TunnelAddressPair | null {
  if (!pair || fromSide === toSide) {
    return pair;
  }
  const local = promotionAddressValue(pair, fromSide, "local");
  const peer = promotionAddressValue(pair, fromSide, "peer");
  return normalizePair({
    left: fieldForSide(toSide, "local") === "left" ? local : peer,
    right: fieldForSide(toSide, "local") === "right" ? local : peer,
    prefix_len: pair.prefix_len,
  });
}

function fieldForSide(side: TunnelEndpointSide, role: "local" | "peer"): "left" | "right" {
  if (side === "left") {
    return role === "local" ? "left" : "right";
  }
  return role === "local" ? "right" : "left";
}
