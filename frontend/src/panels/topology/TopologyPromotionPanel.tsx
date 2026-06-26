import { useMemo, useState, type FormEvent } from "react";
import { Network, Save, ShieldCheck, Wand2, X } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { VpsCombobox } from "../../components/VpsCombobox";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  buildRuntimeControl,
  buildRuntimeTopology,
  isDefaultRuntimeTopology,
  latencyStatusLabel,
  mutationPolicyLabel,
  normalizeTelemetryPromotionRequest,
  ospfStatusLabel,
  planCorrelationLabel,
  readableTelemetryToken,
  runtimeManagerLabel,
  telemetrySourceLabel,
} from "../../topologyRuntime";
import type {
  AgentView,
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
  BandwidthTier,
  OperatorPreferences,
  PromoteTelemetryTunnelRequest,
  TelemetryTunnelRecord,
  TunnelAddressFamily,
  TunnelAddressPair,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import type { PromoteTunnelPlanToCustomAdapterRequest } from "../../typesTopology";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  runPanelAction,
  shortId,
} from "../../utils";

const bandwidthTiers: BandwidthTier[] = ["10m", "100m", "1000m"];
const adapterArgvTooltip =
  "Command and arguments executed by the adapter. Separate arguments with spaces; pasted comma or newline lists are also accepted.";

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
  topologyDesiredText: string;
  topologyStaleText: string;
  topologyRoutesText: string;
  topologyStaleRoutesText: string;
};

type AdapterPromotionSnapshot = {
  detail: string;
  items: Array<{ label: string; value: string }>;
  request: PromoteTunnelPlanToCustomAdapterRequest;
};
type TelemetryPromotionSnapshot = {
  detail: string;
  items: Array<{ label: string; value: string }>;
  request: PromoteTelemetryTunnelRequest;
};
type PromotionReviewItem = {
  detail: string;
  label: string;
  tone?: "attention" | "ready";
  value: string;
};

export function TopologyPromotionPanel({
  agents,
  onAllocateTunnelEndpoints,
  onClose,
  onPromoteTelemetryTunnel,
  onPromoteTunnelPlanToCustomAdapter,
  telemetryTunnels,
  tunnelPlans,
}: {
  agents: AgentView[];
  onAllocateTunnelEndpoints: (request: AllocateTunnelEndpointsRequest) => Promise<AllocateTunnelEndpointsResponse>;
  onClose?: () => void;
  onPromoteTelemetryTunnel: (request: PromoteTelemetryTunnelRequest) => Promise<void>;
  onPromoteTunnelPlanToCustomAdapter: (request: PromoteTunnelPlanToCustomAdapterRequest) => Promise<void>;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const [promoteForm, setPromoteForm] = useState<PromoteTelemetryTunnelRequest>(() =>
    initialTelemetryPromotionForm(preferences),
  );
  const [reservedText, setReservedText] = useState("");
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
    topologyDesiredText: "",
    topologyStaleText: "",
    topologyRoutesText: "",
    topologyStaleRoutesText: "",
  });
  const [adapterTrafficLimitEnabled, setAdapterTrafficLimitEnabled] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [telemetryPromotionSnapshot, setTelemetryPromotionSnapshot] = useState<TelemetryPromotionSnapshot | null>(null);
  const [adapterPromotionSnapshot, setAdapterPromotionSnapshot] = useState<AdapterPromotionSnapshot | null>(null);
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
  const selectedCandidate = importCandidates.find((tunnel) => `${tunnel.client_id}:${tunnel.interface}` === selectedCandidateKey) ?? null;
  const selectedObservedPlan = observedPlans.find((plan) => plan.id === adapterForm.planId) ?? null;
  const currentPromotionPlan = selectedObservedPlan ?? findCurrentPromotionPlan(selectedCandidate, promoteForm, tunnelPlans);
  const promotionReady =
    promoteForm.client_id &&
    promoteForm.interface &&
    promoteForm.peer_client_id &&
    promoteForm.client_id !== promoteForm.peer_client_id &&
    promoteForm.local_underlay.trim() &&
    promoteForm.peer_underlay.trim() &&
    hasPromotionAddressSource(promoteForm);
  const adapterPromotionReady = Boolean(selectedObservedPlan && adapterForm.statusArgv.trim());
  const status =
    actionError ??
    `${importCandidates.length} telemetry imports / ${observedPlans.length} observed plans`;
  const promotionReviewItems = buildPromotionReviewItems({
    adapterForm,
    adapterPromotionReady,
    adapterTrafficLimitEnabled,
    clientLabel,
    currentPlan: currentPromotionPlan,
    promotionReady: Boolean(promotionReady),
    promoteForm,
    selectedCandidate,
    selectedObservedPlan,
  });

  function submitTelemetryPromotion(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setActionError(null);
    if (!promotionReady) {
      setActionError("Promotion request is incomplete");
      return;
    }
    const request = normalizeTelemetryPromotionRequest({
      ...promoteForm,
      confirmed: true,
    });
    setTelemetryPromotionSnapshot({
      detail: "Confirm saving the observed telemetry candidate as a reviewed observed topology plan.",
      items: [
        ...promotionReviewItems.map(({ label, value }) => ({ label, value })),
        { label: "Candidate", value: `${clientLabel(request.client_id)} / ${request.interface}` },
        { label: "Peer", value: clientLabel(request.peer_client_id) },
        { label: "Name", value: request.name ?? "generated" },
        { label: "Side", value: request.side ?? "left" },
        { label: "Activation", value: request.enabled ? "Enabled now" : "Deferred" },
      ],
      request,
    });
  }

  async function executeTelemetryPromotion(snapshot: TelemetryPromotionSnapshot) {
    await runPanelAction(setPending, setActionError, async () => {
      await onPromoteTelemetryTunnel(snapshot.request);
      setTelemetryPromotionSnapshot(null);
    });
  }

  function submitAdapterPromotion(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setActionError(null);
    if (!selectedObservedPlan) {
      setActionError("Select an observed tunnel plan");
      return;
    }
    if (!adapterForm.statusArgv.trim()) {
      setActionError("Adapter status argv is required");
      return;
    }
    const runtimeTopology = buildRuntimeTopology({
      desiredText: adapterForm.topologyDesiredText,
      staleText: adapterForm.topologyStaleText,
      routesText: adapterForm.topologyRoutesText,
      staleRoutesText: adapterForm.topologyStaleRoutesText,
    });
    const request: PromoteTunnelPlanToCustomAdapterRequest = {
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
    };
    setAdapterPromotionSnapshot({
      detail: "Confirm promoting the observed tunnel plan into a custom adapter workflow with reviewed runtime commands.",
      items: [
        ...promotionReviewItems.map(({ label, value }) => ({ label, value })),
        { label: "Plan", value: selectedObservedPlan.name },
        { label: "Runtime", value: "Custom adapter" },
        { label: "Status argv", value: adapterForm.statusArgv.trim() },
        { label: "Traffic", value: adapterTrafficLimitEnabled ? "enabled" : "disabled" },
      ],
      request,
    });
  }

  async function executeAdapterPromotion(snapshot: AdapterPromotionSnapshot) {
    await runPanelAction(setPending, setActionError, async () => {
      await onPromoteTunnelPlanToCustomAdapter(snapshot.request);
    });
  }

  async function allocatePromotionEndpoints() {
    await runPanelAction(setPending, setActionError, async () => {
      const ipv4Pool = promoteForm.address_pool_cidr.trim();
      const ipv6Pool = (promoteForm.ipv6_address_pool_cidr ?? "").trim();
      const hasLocalPool = Boolean(ipv4Pool || ipv6Pool);
      const reservedAddresses = mergeReservedAddresses(
        splitReserved(reservedText),
        currentPromotionAddresses(promoteForm),
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
      setTelemetryPromotionSnapshot(null);
      setPromoteForm((current) => ({
        ...current,
        ipv4_tunnel: allocation.ipv4_tunnel,
        ipv6_tunnel: allocation.ipv6_tunnel,
        latency_primary_family: allocation.latency_primary_family,
      }));
    });
  }

  return (
    <section aria-label="Tunnel plan promotion workflow" className="fleetPanel scheduleComposer" id="network-tunnel-promotion">
      <div className="sectionHeader">
        <div>
          <h2>Tunnel promotion</h2>
          <span>{status}</span>
        </div>
        <div className="sectionActions">
          <Network size={20} />
          {onClose ? (
            <button
              aria-label="Close tunnel promotion workflow"
              className="iconButton"
              onClick={onClose}
              title="Close tunnel promotion workflow"
              type="button"
            >
              <X size={18} />
            </button>
          ) : null}
        </div>
      </div>
      <div className="promotionWorkflow">
        <ConfirmationPrompt
          confirmLabel="Promote observed plan"
          detail={telemetryPromotionSnapshot?.detail ?? ""}
          items={telemetryPromotionSnapshot?.items ?? []}
          onCancel={() => setTelemetryPromotionSnapshot(null)}
          onConfirm={() => {
            const snapshot = telemetryPromotionSnapshot;
            if (!snapshot) {
              return;
            }
            void executeTelemetryPromotion(snapshot);
          }}
          open={telemetryPromotionSnapshot !== null}
          pending={pending}
          title="Confirm external observe"
        />
        <ConfirmationPrompt
          confirmLabel="Save custom adapter"
          detail={adapterPromotionSnapshot?.detail ?? ""}
          items={adapterPromotionSnapshot?.items ?? []}
          onCancel={() => setAdapterPromotionSnapshot(null)}
          onConfirm={() => {
            const snapshot = adapterPromotionSnapshot;
            if (!snapshot) {
              return;
            }
            setAdapterPromotionSnapshot(null);
            void executeAdapterPromotion(snapshot);
          }}
          open={adapterPromotionSnapshot !== null}
          pending={pending}
          title="Confirm custom adapter"
        />
        <div className="promotionDiffHeader">
          <strong>Promotion diff workflow</strong>
          <span>Review observed topology against saved state before converting it into an observed plan or a managed adapter.</span>
        </div>
        <div className="promotionDiffStrip" aria-label="Topology promotion diff workflow">
          {promotionReviewItems.map((item) => (
            <div className={item.tone ? item.tone : undefined} key={item.label}>
              <span>{item.label}</span>
              <strong>{item.value}</strong>
              <p>{item.detail}</p>
            </div>
          ))}
        </div>
        <form className="dispatchForm promotionStageCard promotionImportCard" onSubmit={submitTelemetryPromotion}>
          <div
            className="sectionHeader compactHeader promotionStageHeader"
            title="Convert one telemetry candidate into a saved observed plan."
          >
            <div>
              <small>Observed topology</small>
              <h3>External observe</h3>
              <span>Convert a telemetry-only tunnel into a saved observed plan.</span>
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
                ariaLabel="External observe peer VPS"
                excludeIds={promoteForm.client_id ? [promoteForm.client_id] : []}
                onChange={(value) => setPromotionPeerClient(value)}
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
            <label className="checkLine">
              <input
                checked={promoteForm.enabled}
                onChange={(event) => setPromotionField("enabled", event.target.checked)}
                type="checkbox"
              />
              <span>Plan enabled</span>
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
              <span>Self IPv4 CIDR</span>
              <input
                value={promotionEndpointCidr(promoteForm.ipv4_tunnel ?? null, promoteForm.side ?? "left", "local")}
                onChange={(event) => setPromotionAddressCidr("ipv4_tunnel", "local", event.target.value, 31)}
                placeholder="IPv4 CIDR"
              />
            </label>
            <label>
              <span>Peer IPv4 CIDR</span>
              <input
                value={promotionEndpointCidr(promoteForm.ipv4_tunnel ?? null, promoteForm.side ?? "left", "peer")}
                onChange={(event) => setPromotionAddressCidr("ipv4_tunnel", "peer", event.target.value, 31)}
                placeholder="IPv4 CIDR"
              />
            </label>
          </div>
          <div className="dispatchControls">
            <label>
              <span>Self IPv6 CIDR</span>
              <input
                value={promotionEndpointCidr(promoteForm.ipv6_tunnel ?? null, promoteForm.side ?? "left", "local")}
                onChange={(event) => setPromotionAddressCidr("ipv6_tunnel", "local", event.target.value, 127)}
                placeholder="IPv6 CIDR"
              />
            </label>
            <label>
              <span>Peer IPv6 CIDR</span>
              <input
                value={promotionEndpointCidr(promoteForm.ipv6_tunnel ?? null, promoteForm.side ?? "left", "peer")}
                onChange={(event) => setPromotionAddressCidr("ipv6_tunnel", "peer", event.target.value, 127)}
                placeholder="IPv6 CIDR"
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
                  value={promoteForm.address_pool_cidr}
                  onChange={(event) => setPromotionField("address_pool_cidr", event.target.value)}
                  placeholder="No default"
                />
              </label>
              <label>
                <span>IPv6 pool override</span>
                <input
                  value={promoteForm.ipv6_address_pool_cidr ?? ""}
                  onChange={(event) => setPromotionField("ipv6_address_pool_cidr", event.target.value)}
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

        <details className="promotionAdvancedDrawer">
          <summary>
            <span>
              <strong>Advanced: custom adapter promotion</strong>
              <small>Attach lifecycle, status, traffic, and drift evidence commands to an observed plan.</small>
            </span>
            <ShieldCheck size={18} />
          </summary>
          <form className="dispatchForm promotionStageCard promotionAdapterCard" onSubmit={submitAdapterPromotion}>
            <div
              className="sectionHeader compactHeader promotionStageHeader"
              title="Attach status, startup, shutdown, restart, cleanup, and optional traffic commands to a saved observed plan."
            >
              <div>
                <small>Proposed adapter</small>
                <h3>Custom adapter</h3>
                <span>Promote a saved observed plan into an externally managed adapter contract.</span>
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
            <label className="adapterArgvField">
              <span>Status argv</span>
              <input
                title={adapterArgvTooltip}
                value={adapterForm.statusArgv}
                onChange={(event) => setAdapterField("statusArgv", event.target.value)}
              />
            </label>
            <details
              className="promotionDisclosure"
              title="Optional commands used when the agent starts, restarts, stops, or cleans up the custom adapter."
            >
              <summary>Lifecycle hooks</summary>
              <div className="dispatchControls">
                <label className="adapterArgvField">
                  <span>Start argv</span>
                  <input
                    title={adapterArgvTooltip}
                    value={adapterForm.startupArgv}
                    onChange={(event) => setAdapterField("startupArgv", event.target.value)}
                  />
                </label>
                <label className="adapterArgvField">
                  <span>Restart argv</span>
                  <input
                    title={adapterArgvTooltip}
                    value={adapterForm.restartArgv}
                    onChange={(event) => setAdapterField("restartArgv", event.target.value)}
                  />
                </label>
              </div>
              <div className="dispatchControls">
                <label className="adapterArgvField">
                  <span>Stop argv</span>
                  <input
                    title={adapterArgvTooltip}
                    value={adapterForm.stopArgv}
                    onChange={(event) => setAdapterField("stopArgv", event.target.value)}
                  />
                </label>
                <label className="adapterArgvField">
                  <span>Cleanup argv</span>
                  <input
                    title={adapterArgvTooltip}
                    value={adapterForm.cleanupArgv}
                    onChange={(event) => setAdapterField("cleanupArgv", event.target.value)}
                  />
                </label>
              </div>
            </details>
            <details
              className="promotionDisclosure"
              title="Optional adapter traffic command and speed limits."
            >
              <summary>Traffic shaping</summary>
              <label className="adapterArgvField">
                <span>Traffic argv</span>
                <input
                  title={adapterArgvTooltip}
                  value={adapterForm.trafficArgv}
                  onChange={(event) => setAdapterField("trafficArgv", event.target.value)}
                />
              </label>
              <label className="checkLine">
                <input
                  checked={adapterTrafficLimitEnabled}
                  onChange={(event) => {
                    setAdapterPromotionSnapshot(null);
                    setAdapterTrafficLimitEnabled(event.target.checked);
                  }}
                  type="checkbox"
                />
                <span>Enable shaping</span>
              </label>
              <div className="dispatchControls">
                <label>
                  <span>Egress Kbps</span>
                  <input
                    disabled={!adapterTrafficLimitEnabled}
                    min={64}
                    onChange={(event) => setAdapterField("trafficEgressKbps", event.target.value)}
                    placeholder="disabled"
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
                    placeholder="disabled"
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
                    placeholder="disabled"
                    type="number"
                    value={adapterForm.trafficBurstKb}
                  />
                </label>
              </div>
            </details>
            <details
              className="promotionDisclosure"
              title="Optional desired/stale interface and route evidence used for drift checks."
            >
              <summary>Network evidence</summary>
              <div className="dispatchControls">
                <label>
                  <span>Desired interfaces</span>
                  <input
                    value={adapterForm.topologyDesiredText}
                    onChange={(event) => setAdapterField("topologyDesiredText", event.target.value)}
                  />
                </label>
                <label>
                  <span>Stale interfaces</span>
                  <input
                    value={adapterForm.topologyStaleText}
                    onChange={(event) => setAdapterField("topologyStaleText", event.target.value)}
                  />
                </label>
              </div>
              <div className="dispatchControls">
                <label>
                  <span>Routes</span>
                  <textarea
                    value={adapterForm.topologyRoutesText}
                    onChange={(event) => setAdapterField("topologyRoutesText", event.target.value)}
                  />
                </label>
                <label>
                  <span>Stale routes</span>
                  <textarea
                    value={adapterForm.topologyStaleRoutesText}
                    onChange={(event) => setAdapterField("topologyStaleRoutesText", event.target.value)}
                  />
                </label>
              </div>
            </details>
            {!adapterPromotionSnapshot && (
              <button className="primaryAction" disabled={pending || !adapterPromotionReady} type="submit">
                <ShieldCheck size={17} />
                Review custom adapter
              </button>
            )}
          </form>
        </details>
      </div>
    </section>
  );

  function setPromotionField<K extends keyof PromoteTelemetryTunnelRequest>(
    key: K,
    value: PromoteTelemetryTunnelRequest[K],
  ) {
    setTelemetryPromotionSnapshot(null);
    setPromoteForm((current) => ({ ...current, [key]: value }));
  }

  function setPromotionSide(side: TunnelEndpointSide) {
    setTelemetryPromotionSnapshot(null);
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

  function setPromotionPeerClient(clientId: string) {
    setTelemetryPromotionSnapshot(null);
    setPromoteForm((current) => ({
      ...current,
      peer_client_id: clientId,
      peer_underlay: autoUnderlayValue(
        current.peer_underlay,
        current.peer_client_id,
        clientId,
        agents,
      ),
    }));
  }

  function setPromotionAddressCidr(
    key: "ipv4_tunnel" | "ipv6_tunnel",
    role: "local" | "peer",
    value: string,
    fallbackPrefix: number,
  ) {
    setTelemetryPromotionSnapshot(null);
    setPromoteForm((current) => ({
      ...current,
      [key]: updatePairCidrForSide(current[key] ?? null, current.side ?? "left", role, value, fallbackPrefix),
    }));
  }

  function setAdapterField<K extends keyof AdapterPromotionForm>(key: K, value: AdapterPromotionForm[K]) {
    setAdapterPromotionSnapshot(null);
    setAdapterForm((current) => ({ ...current, [key]: value }));
  }

  function selectPromotionCandidate(value: string) {
    setTelemetryPromotionSnapshot(null);
    const candidate = importCandidates.find((tunnel) => `${tunnel.client_id}:${tunnel.interface}` === value);
    if (!candidate) {
      setPromoteForm((current) => ({ ...current, client_id: "", interface: "" }));
      return;
    }
    setPromoteForm((current) => ({
      ...current,
      client_id: candidate.client_id,
      interface: candidate.interface,
      local_underlay: autoUnderlayValue(
        current.local_underlay,
        current.client_id,
        candidate.client_id,
        agents,
      ),
      name: current.name || `${clientLabel(candidate.client_id)}-${candidate.interface}-observed`,
    }));
  }

  function selectAdapterPlan(planId: string) {
    setAdapterPromotionSnapshot(null);
    const plan = observedPlans.find((candidate) => candidate.id === planId);
    if (!plan) {
      setAdapterForm((current) => ({ ...current, planId: "" }));
      return;
    }
    setAdapterForm((current) => ({
      ...current,
      planId,
      name: current.name || `${plan.name}-adapter`,
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

function findCurrentPromotionPlan(
  candidate: TelemetryTunnelRecord | null,
  form: PromoteTelemetryTunnelRequest,
  tunnelPlans: TunnelPlanRecord[],
): TunnelPlanRecord | null {
  if (candidate?.plan_id) {
    return tunnelPlans.find((plan) => plan.id === candidate.plan_id) ?? null;
  }
  const clientId = candidate?.client_id || form.client_id;
  const interfaceName = candidate?.interface || form.interface;
  if (!clientId || !interfaceName) {
    return null;
  }
  return tunnelPlans.find((plan) => {
    const planClientIds = [plan.left_client_id, plan.right_client_id];
    const peerMatches = !form.peer_client_id || planClientIds.includes(form.peer_client_id);
    return plan.plan.interface_name === interfaceName && planClientIds.includes(clientId) && peerMatches;
  }) ?? null;
}

function buildPromotionReviewItems({
  adapterForm,
  adapterPromotionReady,
  adapterTrafficLimitEnabled,
  clientLabel,
  currentPlan,
  promotionReady,
  promoteForm,
  selectedCandidate,
  selectedObservedPlan,
}: {
  adapterForm: AdapterPromotionForm;
  adapterPromotionReady: boolean;
  adapterTrafficLimitEnabled: boolean;
  clientLabel: (clientId: string) => string;
  currentPlan: TunnelPlanRecord | null;
  promotionReady: boolean;
  promoteForm: PromoteTelemetryTunnelRequest;
  selectedCandidate: TelemetryTunnelRecord | null;
  selectedObservedPlan: TunnelPlanRecord | null;
}): PromotionReviewItem[] {
  const adapterMode = selectedObservedPlan !== null;
  return [
    buildObservedReviewItem(adapterMode, selectedCandidate, selectedObservedPlan, clientLabel),
    buildCurrentReviewItem(selectedCandidate, currentPlan, clientLabel),
    buildProposedReviewItem(adapterMode, adapterForm, adapterPromotionReady, promoteForm, selectedCandidate, selectedObservedPlan),
    buildConflictReviewItem(adapterMode, adapterPromotionReady, currentPlan, promotionReady, promoteForm, selectedCandidate),
    buildRiskReviewItem(adapterMode, adapterTrafficLimitEnabled, promoteForm, selectedCandidate, selectedObservedPlan),
    buildReviewActionItem(adapterMode, adapterPromotionReady, promotionReady),
  ];
}

function buildObservedReviewItem(
  adapterMode: boolean,
  selectedCandidate: TelemetryTunnelRecord | null,
  selectedObservedPlan: TunnelPlanRecord | null,
  clientLabel: (clientId: string) => string,
): PromotionReviewItem {
  if (adapterMode && selectedObservedPlan) {
    return {
      detail: `${formatPlanEndpoints(selectedObservedPlan, clientLabel)}; ${runtimeManagerLabel(selectedObservedPlan.plan.runtime_control?.manager)}`,
      label: "Observed topology",
      tone: "ready",
      value: `${selectedObservedPlan.name} / ${selectedObservedPlan.plan.interface_name}`,
    };
  }
  if (selectedCandidate) {
    return {
      detail: `${readableTelemetryToken(selectedCandidate.kind)}; ${mutationPolicyLabel(selectedCandidate.mutation_policy)}; ${telemetrySourceLabel(selectedCandidate.source)}`,
      label: "Observed topology",
      tone: "ready",
      value: `${clientLabel(selectedCandidate.client_id)} / ${selectedCandidate.interface}`,
    };
  }
  return {
    detail: "Select an unmatched telemetry interface or an observed plan to start promotion review.",
    label: "Observed topology",
    value: "Waiting for selection",
  };
}

function buildCurrentReviewItem(
  selectedCandidate: TelemetryTunnelRecord | null,
  currentPlan: TunnelPlanRecord | null,
  clientLabel: (clientId: string) => string,
): PromotionReviewItem {
  if (currentPlan) {
    return {
      detail: `${readableTelemetryToken(currentPlan.status)}; ${formatPlanEndpoints(currentPlan, clientLabel)}; OSPF cost ${currentPlan.recommended_ospf_cost}`,
      label: "Current applied topology",
      tone: currentPlan.enabled ? "ready" : undefined,
      value: `${currentPlan.name} / ${runtimeManagerLabel(currentPlan.plan.runtime_control?.manager)}`,
    };
  }
  if (selectedCandidate) {
    return {
      detail: `${planCorrelationLabel(selectedCandidate.plan_correlation)}; ${selectedCandidate.plan_name ?? "no saved plan match"}`,
      label: "Current applied topology",
      tone: selectedCandidate.plan_id ? "ready" : undefined,
      value: selectedCandidate.plan_id ? "Saved plan found" : "No saved plan match",
    };
  }
  return {
    detail: "Saved-plan comparison appears after a candidate or observed plan is selected.",
    label: "Current applied topology",
    value: "Not selected",
  };
}

function buildProposedReviewItem(
  adapterMode: boolean,
  adapterForm: AdapterPromotionForm,
  adapterPromotionReady: boolean,
  promoteForm: PromoteTelemetryTunnelRequest,
  selectedCandidate: TelemetryTunnelRecord | null,
  selectedObservedPlan: TunnelPlanRecord | null,
): PromotionReviewItem {
  if (adapterMode && selectedObservedPlan) {
    return {
      detail: adapterPromotionReady
        ? "Status command will be saved as the required adapter health contract."
        : "Status argv is required before custom adapter review.",
      label: "Proposed plan",
      tone: adapterPromotionReady ? "ready" : "attention",
      value: `${adapterForm.name.trim() || `${selectedObservedPlan.name}-adapter`} / Custom adapter`,
    };
  }
  if (selectedCandidate) {
    return {
      detail: `${bandwidthTierLabel(promoteForm.bandwidth ?? "100m")}; ${promoteForm.latency_ms ?? 20} ms baseline; ${formatPromotionAddresses(promoteForm)}`,
      label: "Proposed plan",
      tone: hasPromotionAddressSource(promoteForm) ? "ready" : "attention",
      value: `${promoteForm.name || `${selectedCandidate.interface}-observed`} / ${promoteForm.enabled ? "enabled" : "deferred"}`,
    };
  }
  return {
    detail: "The proposed observed plan appears after selecting telemetry and completing peer/endpoints.",
    label: "Proposed plan",
    value: "Not drafted",
  };
}

function buildConflictReviewItem(
  adapterMode: boolean,
  adapterPromotionReady: boolean,
  currentPlan: TunnelPlanRecord | null,
  promotionReady: boolean,
  promoteForm: PromoteTelemetryTunnelRequest,
  selectedCandidate: TelemetryTunnelRecord | null,
): PromotionReviewItem {
  const conflicts = currentPlan?.plan.conflicts ?? [];
  if (conflicts.length > 0) {
    return {
      detail: conflicts.slice(0, 2).join("; "),
      label: "Conflicts",
      tone: "attention",
      value: `${conflicts.length} generated conflict${conflicts.length === 1 ? "" : "s"}`,
    };
  }
  if (adapterMode) {
    return {
      detail: adapterPromotionReady
        ? "Selected observed plan and status command are ready for adapter review."
        : "Select an observed plan and provide a status argv before review.",
      label: "Conflicts",
      tone: adapterPromotionReady ? "ready" : "attention",
      value: adapterPromotionReady ? "No visible blockers" : "Adapter review blocked",
    };
  }
  const missing = promotionMissingParts(promoteForm, selectedCandidate);
  return {
    detail: missing.length > 0 ? `Complete ${missing.join(", ")} before review.` : "Peer, underlay, and endpoint pair are complete.",
    label: "Conflicts",
    tone: missing.length > 0 ? "attention" : "ready",
    value: missing.length > 0 ? "Promotion blocked" : "No visible blockers",
  };
}

function buildRiskReviewItem(
  adapterMode: boolean,
  adapterTrafficLimitEnabled: boolean,
  promoteForm: PromoteTelemetryTunnelRequest,
  selectedCandidate: TelemetryTunnelRecord | null,
  selectedObservedPlan: TunnelPlanRecord | null,
): PromotionReviewItem {
  if (adapterMode && selectedObservedPlan) {
    return {
      detail: `${runtimeManagerLabel(selectedObservedPlan.plan.runtime_control?.manager)} -> Custom adapter; ${selectedObservedPlan.enabled ? "plan currently enabled" : "plan disabled"}.`,
      label: "Risk",
      tone: adapterTrafficLimitEnabled ? "attention" : undefined,
      value: adapterTrafficLimitEnabled ? "Traffic mutation enabled" : "Adapter health contract",
    };
  }
  if (selectedCandidate) {
    const latency = latencyStatusLabel(selectedCandidate.latency_status);
    const ospf = ospfStatusLabel(selectedCandidate.auto_ospf_status, selectedCandidate.auto_ospf_enabled);
    return {
      detail: `${latency}; OSPF ${ospf}; ${mutationPolicyLabel(selectedCandidate.mutation_policy)}.`,
      label: "Risk",
      tone: promoteForm.enabled ? "attention" : undefined,
      value: promoteForm.enabled ? "Enabled on save" : "Deferred save",
    };
  }
  return {
    detail: "Risk summary appears after selecting a promotion source.",
    label: "Risk",
    value: "Not assessed",
  };
}

function buildReviewActionItem(
  adapterMode: boolean,
  adapterPromotionReady: boolean,
  promotionReady: boolean,
): PromotionReviewItem {
  if (adapterMode) {
    return {
      detail: adapterPromotionReady ? "Use Review custom adapter for the final confirmation." : "Complete the adapter contract before confirmation.",
      label: "Review / approve",
      tone: adapterPromotionReady ? "ready" : "attention",
      value: adapterPromotionReady ? "Ready to review" : "Blocked",
    };
  }
  return {
    detail: promotionReady ? "Use Save observed plan for the final confirmation." : "Complete peer, underlay, and endpoint CIDR before saving.",
    label: "Review / approve",
    tone: promotionReady ? "ready" : "attention",
    value: promotionReady ? "Ready to review" : "Blocked",
  };
}

function promotionMissingParts(
  form: PromoteTelemetryTunnelRequest,
  selectedCandidate: TelemetryTunnelRecord | null,
): string[] {
  const missing: string[] = [];
  if (!selectedCandidate) {
    missing.push("observed interface");
  }
  if (!form.peer_client_id) {
    missing.push("peer VPS");
  } else if (form.client_id === form.peer_client_id) {
    missing.push("distinct peer VPS");
  }
  if (!form.local_underlay.trim()) {
    missing.push("local underlay");
  }
  if (!form.peer_underlay.trim()) {
    missing.push("peer underlay");
  }
  if (!hasPromotionAddressSource(form)) {
    missing.push("endpoint CIDRs");
  }
  return missing;
}

function formatPlanEndpoints(plan: TunnelPlanRecord, clientLabel: (clientId: string) => string): string {
  return `${clientLabel(plan.left_client_id)} -> ${clientLabel(plan.right_client_id)}`;
}

function formatPromotionAddresses(form: PromoteTelemetryTunnelRequest): string {
  const ipv4 = completePairOrNull(form.ipv4_tunnel ?? null);
  if (ipv4) {
    return `IPv4 ${ipv4.left}/${ipv4.prefix_len} -> ${ipv4.right}/${ipv4.prefix_len}`;
  }
  const ipv6 = completePairOrNull(form.ipv6_tunnel ?? null);
  if (ipv6) {
    return `IPv6 ${ipv6.left}/${ipv6.prefix_len} -> ${ipv6.right}/${ipv6.prefix_len}`;
  }
  return "endpoint CIDRs required";
}

function initialTelemetryPromotionForm(preferences: OperatorPreferences): PromoteTelemetryTunnelRequest {
  return {
    client_id: "",
    interface: "",
    peer_client_id: "",
    local_underlay: "",
    peer_underlay: "",
    address_pool_cidr: preferences.tunnel_ipv4_allocation_pool_cidr,
    ipv4_tunnel: null,
    ipv6_address_pool_cidr: preferences.tunnel_ipv6_allocation_pool_cidr,
    ipv6_tunnel: null,
    latency_primary_family: "ipv4",
    side: "left",
    bandwidth: "100m",
    latency_ms: 20,
    packet_loss_ratio: 0,
    preference: 1,
    enabled: false,
    confirmed: false,
  };
}

function hasPromotionAddressSource(form: PromoteTelemetryTunnelRequest): boolean {
  return Boolean(completePairOrNull(form.ipv4_tunnel ?? null) || completePairOrNull(form.ipv6_tunnel ?? null));
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

function currentPromotionAddresses(form: PromoteTelemetryTunnelRequest): string[] {
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

function promotionEndpointCidr(
  pair: TunnelAddressPair | null,
  side: TunnelEndpointSide,
  role: "local" | "peer",
): string {
  const address = promotionAddressValue(pair, side, role);
  if (!address || !pair) {
    return "";
  }
  return `${address}/${pair.prefix_len}`;
}

function updatePairCidrForSide(
  pair: TunnelAddressPair | null,
  side: TunnelEndpointSide,
  role: "local" | "peer",
  value: string,
  fallbackPrefix: number,
): TunnelAddressPair | null {
  const field = fieldForSide(side, role);
  const parsed = parseEndpointCidr(value, pair?.prefix_len ?? fallbackPrefix);
  const nextPair = {
    left: pair?.left ?? "",
    right: pair?.right ?? "",
    prefix_len: parsed.prefix_len,
    [field]: parsed.address,
  };
  return normalizePair(nextPair);
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
