import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { Activity, AlertTriangle, Bell, Boxes, Gauge, Network, Server } from "lucide-react";
import { Metric } from "../components/Metric";
import type {
  AgentView,
  FleetAlertPolicyRecord,
  FleetAlertPolicyRequest,
  FleetAlertRecord,
  FleetAlertNotificationChannelRecord,
  FleetAlertNotificationChannelRequest,
  FleetAlertNotificationDeliveryRecord,
  FleetAlertNotificationDispatchRequest,
  FleetAlertNotificationProcessRequest,
  FleetAlertStateRecord,
  FleetAlertStateRequest,
  FleetSummary,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";

type FleetDetailTab = "Overview" | "Telemetry" | "Jobs" | "Network";

const detailTabs: FleetDetailTab[] = ["Overview", "Telemetry", "Jobs", "Network"];

export function FleetWorkspace({
  agents,
  apiError,
  fleetAlerts,
  fleetAlertStates,
  fleetAlertPolicies,
  fleetAlertNotificationChannels,
  fleetAlertNotifications,
  lastLiveEvent,
  onDispatchFleetAlertNotifications,
  onProcessFleetAlertNotifications,
  onSelectAgent,
  onUpdateAgentAlias,
  onUpdateFleetAlertState,
  onUpsertFleetAlertNotificationChannel,
  onUpsertFleetAlertPolicy,
  scopeActive,
  selectedAgent,
  summary,
  telemetryNetworkRates,
  telemetryRollups,
  telemetryTunnels,
  wsState,
}: {
  agents: AgentView[];
  apiError: string | null;
  fleetAlerts: FleetAlertRecord[];
  fleetAlertStates: FleetAlertStateRecord[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  fleetAlertNotificationChannels: FleetAlertNotificationChannelRecord[];
  fleetAlertNotifications: FleetAlertNotificationDeliveryRecord[];
  lastLiveEvent: string;
  onDispatchFleetAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onProcessFleetAlertNotifications: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onSelectAgent: (agentId: string) => void;
  onUpdateAgentAlias: (clientId: string, displayName: string) => Promise<AgentView>;
  onUpdateFleetAlertState: (request: FleetAlertStateRequest) => Promise<FleetAlertStateRecord>;
  onUpsertFleetAlertNotificationChannel: (
    request: FleetAlertNotificationChannelRequest,
  ) => Promise<FleetAlertNotificationChannelRecord>;
  onUpsertFleetAlertPolicy: (request: FleetAlertPolicyRequest) => Promise<FleetAlertPolicyRecord>;
  scopeActive: boolean;
  selectedAgent: AgentView | null;
  summary: FleetSummary;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  wsState: string;
}) {
  const [activeDetailTab, setActiveDetailTab] = useState<FleetDetailTab>("Overview");
  const [aliasDraft, setAliasDraft] = useState("");
  const [aliasPending, setAliasPending] = useState(false);
  const [aliasError, setAliasError] = useState<string | null>(null);
  const selectedTags = selectedAgent?.tags ?? [];
  const isNetworkManaged = selectedTags.some((tag) => ["bgp", "bird2", "ospf", "tunnel"].includes(tag.toLowerCase()));
  const latestRollups = useMemo(() => latestTelemetryRollupsByClient(telemetryRollups), [telemetryRollups]);
  const latestNetworkRates = useMemo(
    () => latestTelemetryNetworkRatesByClient(telemetryNetworkRates),
    [telemetryNetworkRates],
  );
  const latestTunnels = useMemo(() => latestTelemetryTunnelsByClient(telemetryTunnels), [telemetryTunnels]);
  const selectedRollup = selectedAgent ? latestRollups.get(selectedAgent.id) ?? null : null;
  const selectedNetworkRates = selectedAgent ? latestNetworkRates.get(selectedAgent.id) ?? [] : [];
  const selectedTunnels = selectedAgent ? latestTunnels.get(selectedAgent.id) ?? [] : [];
  const selectedCapabilities = selectedAgent?.capabilities;

  useEffect(() => {
    setAliasDraft(selectedAgent?.display_name ?? "");
    setAliasError(null);
  }, [selectedAgent?.display_name, selectedAgent?.id]);

  async function submitAlias(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selectedAgent) {
      return;
    }
    const displayName = aliasDraft.trim();
    if (!displayName) {
      setAliasError("Alias is required");
      return;
    }
    setAliasPending(true);
    setAliasError(null);
    try {
      await onUpdateAgentAlias(selectedAgent.id, displayName);
    } catch (error) {
      setAliasError(error instanceof Error ? error.message : "Alias update failed");
    } finally {
      setAliasPending(false);
    }
  }

  return (
    <section className="workspace">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>VPS instances</h2>
            <span>{apiError ? "API unavailable" : "Live control-plane inventory"}</span>
          </div>
          <div className="segmented">
            <button className="selected" type="button">
              Pools
            </button>
            <button type="button">Tags</button>
          </div>
        </div>

        <FleetAlertList alerts={fleetAlerts} stateCount={fleetAlertStates.length} onUpdate={onUpdateFleetAlertState} />
        <FleetAlertPolicyManager policies={fleetAlertPolicies} onUpsert={onUpsertFleetAlertPolicy} />
        <FleetAlertNotificationManager
          channels={fleetAlertNotificationChannels}
          deliveries={fleetAlertNotifications}
          onDispatch={onDispatchFleetAlertNotifications}
          onProcess={onProcessFleetAlertNotifications}
          onUpsert={onUpsertFleetAlertNotificationChannel}
        />

        <div className="table">
          <div className="row heading">
            <span>Name</span>
            <span>Status</span>
            <span>CPU</span>
            <span>RAM</span>
            <span>Tags</span>
          </div>
          {agents.map((agent) => (
            <button
              className={selectedAgent?.id === agent.id ? "row agentRow selected" : "row agentRow"}
              key={agent.id}
              onClick={() => onSelectAgent(agent.id)}
              type="button"
            >
              <span className="instance">
                <Server size={17} />
                <span>
                  <strong>{agent.display_name || agent.id}</strong>
                  <small>{agent.id}</small>
                </span>
              </span>
              <span className={agent.status === "connected" ? "status ok" : "status warn"}>{agent.status}</span>
              <span className="mutedValue">{formatLoad(latestRollups.get(agent.id)?.cpu_load_1_avg)}</span>
              <span className="mutedValue">{formatMemoryUsed(latestRollups.get(agent.id))}</span>
              <span className="tags">
                {agent.tags.length === 0 ? <em>untagged</em> : agent.tags.map((tag) => <em key={tag}>{tag}</em>)}
              </span>
            </button>
          ))}
          {agents.length === 0 && (
            <div className="emptyState">
              <Server size={22} />
              <strong>{scopeActive ? "No VPS match this view" : "No agents connected"}</strong>
              <span>{apiError ?? (scopeActive ? "Adjust or clear the saved fleet view." : "Waiting for enrolled VPS agents to report in.")}</span>
            </div>
          )}
        </div>
      </div>

      <aside className="inspector">
        <div className="sectionHeader compact">
          <h2>{selectedAgent?.display_name ?? "No VPS selected"}</h2>
          <span>WebSocket {wsState}</span>
        </div>
        {selectedAgent && (
          <form className="aliasEditor" onSubmit={submitAlias}>
            <label>
              <span>Alias</span>
              <input
                aria-label="VPS alias"
                onChange={(event) => setAliasDraft(event.target.value)}
                value={aliasDraft}
              />
            </label>
            <button
              className="secondaryAction"
              disabled={aliasPending || aliasDraft.trim() === selectedAgent.display_name}
              type="submit"
            >
              Rename
            </button>
            {aliasError && <small className="errorText">{aliasError}</small>}
          </form>
        )}
        <div className="detailTabs" role="tablist" aria-label="VPS detail sections">
          {detailTabs.map((tab) => (
            <button
              aria-selected={activeDetailTab === tab}
              className={activeDetailTab === tab ? "selected" : ""}
              key={tab}
              onClick={() => setActiveDetailTab(tab)}
              role="tab"
              type="button"
            >
              {tab}
            </button>
          ))}
        </div>
        <div className="signalGrid">
          <Metric label="Latency" value="-" tone="blue" />
          <Metric label="Loss" value="-" tone="green" />
        </div>
        <div className="detailPane" role="tabpanel">
          {activeDetailTab === "Overview" && (
            <>
              <DetailLine icon={<Server size={18} />} label="Status" value={selectedAgent?.status ?? "No target"} />
              <DetailLine icon={<Boxes size={18} />} label="Client ID" value={selectedAgent?.id ?? "-"} mono />
              <DetailLine icon={<Gauge size={18} />} label="Privilege" value={formatPrivilege(selectedCapabilities)} />
              <DetailLine
                icon={<Gauge size={18} />}
                label="Fleet position"
                value={selectedAgent ? `${summary.connected} connected / ${summary.total} total` : "-"}
              />
            </>
          )}
          {activeDetailTab === "Telemetry" && (
            <>
              <DetailLine icon={<Activity size={18} />} label="Stream" value={wsState} />
              <DetailLine
                icon={<Gauge size={18} />}
                label="Last event"
                value={summary.total === 0 ? "No samples" : lastLiveEvent}
              />
              <DetailLine icon={<Gauge size={18} />} label="CPU load" value={formatLoad(selectedRollup?.cpu_load_1_avg)} />
              <DetailLine icon={<Server size={18} />} label="RAM used" value={formatMemoryUsed(selectedRollup)} />
              <DetailLine icon={<Boxes size={18} />} label="Disk free" value={formatDiskFree(selectedRollup)} />
              <DetailLine icon={<Network size={18} />} label="Network bytes" value={formatNetworkBytes(selectedRollup)} />
              <DetailLine icon={<Network size={18} />} label="Network rate" value={formatNetworkRateSummary(selectedNetworkRates)} />
              <DetailLine icon={<Activity size={18} />} label="Rollup samples" value={formatRollupSamples(selectedRollup)} />
              <DetailLine icon={<Server size={18} />} label="Agent status" value={selectedAgent?.status ?? "-"} />
            </>
          )}
          {activeDetailTab === "Jobs" && (
            <>
              <DetailLine icon={<Gauge size={18} />} label="Running jobs" value={String(summary.running_jobs)} />
              <DetailLine icon={<Server size={18} />} label="Target" value={selectedAgent?.id ?? "-"} mono />
              <DetailLine icon={<Activity size={18} />} label="Proof state" value="Local unlock required" />
            </>
          )}
          {activeDetailTab === "Network" && (
            <>
              <DetailLine icon={<Network size={18} />} label="Managed routing" value={isNetworkManaged ? "BGP/OSPF" : "Standard"} />
              <DetailLine
                icon={<Gauge size={18} />}
                label="Runtime control"
                value={formatTunnelCapability(selectedCapabilities)}
              />
              <DetailLine icon={<Boxes size={18} />} label="Tags" value={selectedTags.join(", ") || "untagged"} />
              <TunnelList tunnels={selectedTunnels} />
              <NetworkRateList rates={selectedNetworkRates} />
              <DetailLine icon={<Activity size={18} />} label="Tunnel apply" value="Observe and plan" />
              {selectedCapabilities?.unprivileged_hint && (
                <DetailLine icon={<Activity size={18} />} label="Privilege hint" value={selectedCapabilities.unprivileged_hint} />
              )}
            </>
          )}
        </div>
      </aside>
    </section>
  );
}

function FleetAlertPolicyManager({
  policies,
  onUpsert,
}: {
  policies: FleetAlertPolicyRecord[];
  onUpsert: (request: FleetAlertPolicyRequest) => Promise<FleetAlertPolicyRecord>;
}) {
  const [name, setName] = useState("edge-resource-policy");
  const [scopeKind, setScopeKind] = useState("tag");
  const [scopeValue, setScopeValue] = useState("edge");
  const [memoryWarning, setMemoryWarning] = useState("0.20");
  const [memoryCritical, setMemoryCritical] = useState("0.10");
  const [diskWarning, setDiskWarning] = useState("");
  const [diskCritical, setDiskCritical] = useState("");
  const [cpuWarning, setCpuWarning] = useState("");
  const [cpuCritical, setCpuCritical] = useState("");
  const [priority, setPriority] = useState("0");
  const [status, setStatus] = useState<string | null>(null);
  const topPolicies = policies.slice(0, 4);

  async function submit() {
    setStatus("saving");
    try {
      await onUpsert({
        name,
        scope_kind: scopeKind,
        scope_value: scopeKind === "global" ? null : scopeValue,
        memory_available_warning_ratio: optionalNumber(memoryWarning),
        memory_available_critical_ratio: optionalNumber(memoryCritical),
        disk_available_warning_ratio: optionalNumber(diskWarning),
        disk_available_critical_ratio: optionalNumber(diskCritical),
        cpu_load_warning: optionalNumber(cpuWarning),
        cpu_load_critical: optionalNumber(cpuCritical),
        priority: Number.parseInt(priority || "0", 10),
        enabled: true,
        confirmed: true,
      });
      setStatus("saved");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "save failed");
    }
  }

  return (
    <div className="fleetPolicyManager" aria-label="Fleet alert policy manager">
      <div className="fleetPolicyHeader">
        <strong>Alert policies</strong>
        <span>{policies.length} scoped</span>
      </div>
      <div className="fleetPolicyGrid">
        <input aria-label="Policy name" value={name} onChange={(event) => setName(event.target.value)} />
        <select aria-label="Policy scope kind" value={scopeKind} onChange={(event) => setScopeKind(event.target.value)}>
          <option value="global">global</option>
          <option value="provider">provider</option>
          <option value="pool">pool</option>
          <option value="tag">tag</option>
          <option value="client">client</option>
        </select>
        <input
          aria-label="Policy scope value"
          disabled={scopeKind === "global"}
          value={scopeValue}
          onChange={(event) => setScopeValue(event.target.value)}
        />
        <input aria-label="Memory warning ratio" value={memoryWarning} onChange={(event) => setMemoryWarning(event.target.value)} />
        <input aria-label="Memory critical ratio" value={memoryCritical} onChange={(event) => setMemoryCritical(event.target.value)} />
        <input aria-label="Disk warning ratio" value={diskWarning} onChange={(event) => setDiskWarning(event.target.value)} placeholder="disk warn" />
        <input aria-label="Disk critical ratio" value={diskCritical} onChange={(event) => setDiskCritical(event.target.value)} placeholder="disk crit" />
        <input aria-label="CPU warning load" value={cpuWarning} onChange={(event) => setCpuWarning(event.target.value)} placeholder="cpu warn" />
        <input aria-label="CPU critical load" value={cpuCritical} onChange={(event) => setCpuCritical(event.target.value)} placeholder="cpu crit" />
        <input aria-label="Policy priority" value={priority} onChange={(event) => setPriority(event.target.value)} />
        <button type="button" onClick={() => void submit()}>
          Save
        </button>
      </div>
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <div className="fleetPolicyRows">
        {topPolicies.map((policy) => (
          <span key={policy.id}>
            <strong>{policy.name}</strong>
            <small>
              {policy.scope_kind}
              {policy.scope_value ? `:${policy.scope_value}` : ""} priority {policy.priority}
            </small>
          </span>
        ))}
        {topPolicies.length === 0 && <small>No scoped policies saved</small>}
      </div>
    </div>
  );
}

function optionalNumber(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const parsed = Number.parseFloat(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

function optionalInteger(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const parsed = Number.parseInt(trimmed, 10);
  return Number.isFinite(parsed) ? parsed : null;
}

function csvValues(value: string): string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

function FleetAlertNotificationManager({
  channels,
  deliveries,
  onDispatch,
  onProcess,
  onUpsert,
}: {
  channels: FleetAlertNotificationChannelRecord[];
  deliveries: FleetAlertNotificationDeliveryRecord[];
  onDispatch: (request: FleetAlertNotificationDispatchRequest) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onProcess: (request: FleetAlertNotificationProcessRequest) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onUpsert: (request: FleetAlertNotificationChannelRequest) => Promise<FleetAlertNotificationChannelRecord>;
}) {
  const [name, setName] = useState("edge-audit-channel");
  const [scopeKind, setScopeKind] = useState("tag");
  const [scopeValue, setScopeValue] = useState("edge");
  const [minSeverity, setMinSeverity] = useState("warning");
  const [categories, setCategories] = useState("agent_status,network");
  const [operatorStates, setOperatorStates] = useState("open,escalated");
  const [deliveryKind, setDeliveryKind] = useState("audit_log");
  const [target, setTarget] = useState("audit:fleet");
  const [cooldownSecs, setCooldownSecs] = useState("3600");
  const [status, setStatus] = useState<string | null>(null);
  const topChannels = channels.slice(0, 4);
  const topDeliveries = deliveries.slice(0, 4);

  async function submit() {
    setStatus("saving channel");
    try {
      await onUpsert({
        name,
        scope_kind: scopeKind,
        scope_value: scopeKind === "global" ? null : scopeValue,
        min_severity: minSeverity,
        categories: csvValues(categories),
        operator_states: csvValues(operatorStates),
        delivery_kind: deliveryKind,
        target,
        cooldown_secs: optionalInteger(cooldownSecs),
        enabled: true,
        confirmed: true,
      });
      setStatus("channel saved");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "channel save failed");
    }
  }

  async function dispatch(dryRun: boolean) {
    setStatus(dryRun ? "matching" : "dispatching");
    try {
      const rows = await onDispatch({
        limit: 100,
        include_muted: true,
        dry_run: dryRun,
        confirmed: !dryRun,
      });
      setStatus(`${dryRun ? "matched" : "dispatched"} ${rows.length}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "dispatch failed");
    }
  }

  async function process(dryRun: boolean) {
    setStatus(dryRun ? "previewing delivery" : "delivering queued");
    try {
      const rows = await onProcess({
        limit: 50,
        status: "queued",
        delivery_kind: deliveryKind.trim() || null,
        dry_run: dryRun,
        confirmed: !dryRun,
      });
      setStatus(`${dryRun ? "previewed" : "processed"} ${rows.length}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "delivery processing failed");
    }
  }

  return (
    <div className="fleetPolicyManager fleetNotificationManager" aria-label="Fleet alert notification manager">
      <div className="fleetPolicyHeader">
        <span>
          <Bell size={16} />
          <strong>Notification channels</strong>
        </span>
        <span>{channels.length} channels</span>
      </div>
      <div className="fleetPolicyGrid notificationGrid">
        <input aria-label="Notification channel name" value={name} onChange={(event) => setName(event.target.value)} />
        <select aria-label="Notification scope kind" value={scopeKind} onChange={(event) => setScopeKind(event.target.value)}>
          <option value="global">global</option>
          <option value="provider">provider</option>
          <option value="pool">pool</option>
          <option value="tag">tag</option>
          <option value="client">client</option>
        </select>
        <input
          aria-label="Notification scope value"
          disabled={scopeKind === "global"}
          value={scopeValue}
          onChange={(event) => setScopeValue(event.target.value)}
        />
        <select aria-label="Minimum severity" value={minSeverity} onChange={(event) => setMinSeverity(event.target.value)}>
          <option value="critical">critical</option>
          <option value="warning">warning</option>
          <option value="info">info</option>
        </select>
        <input aria-label="Alert categories" value={categories} onChange={(event) => setCategories(event.target.value)} />
        <input aria-label="Operator states" value={operatorStates} onChange={(event) => setOperatorStates(event.target.value)} />
        <input aria-label="Delivery kind" value={deliveryKind} onChange={(event) => setDeliveryKind(event.target.value)} />
        <input aria-label="Delivery target" value={target} onChange={(event) => setTarget(event.target.value)} />
        <input aria-label="Cooldown seconds" value={cooldownSecs} onChange={(event) => setCooldownSecs(event.target.value)} />
        <button type="button" onClick={() => void submit()}>
          Save
        </button>
        <button type="button" onClick={() => void dispatch(true)}>
          Match
        </button>
        <button type="button" onClick={() => void dispatch(false)}>
          Dispatch
        </button>
        <button type="button" onClick={() => void process(true)}>
          Preview
        </button>
        <button type="button" onClick={() => void process(false)}>
          Deliver
        </button>
      </div>
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <div className="fleetPolicyRows notificationRows">
        {topChannels.map((channel) => (
          <span key={channel.id}>
            <strong>{channel.name}</strong>
            <small>
              {channel.scope_kind}
              {channel.scope_value ? `:${channel.scope_value}` : ""} {channel.delivery_kind} {channel.min_severity}
            </small>
          </span>
        ))}
        {topDeliveries.map((delivery) => (
          <span key={delivery.id}>
            <strong>{delivery.channel_name}</strong>
            <small>
              {delivery.status} {delivery.alert_category} {delivery.delivery_kind} attempts {delivery.attempt_count}
              {delivery.error ? ` error ${delivery.error}` : ""}
            </small>
          </span>
        ))}
        {topChannels.length === 0 && topDeliveries.length === 0 && <small>No notification channel saved</small>}
      </div>
    </div>
  );
}

function FleetAlertList({
  alerts,
  stateCount,
  onUpdate,
}: {
  alerts: FleetAlertRecord[];
  stateCount: number;
  onUpdate: (request: FleetAlertStateRequest) => Promise<FleetAlertStateRecord>;
}) {
  const [pending, setPending] = useState<string | null>(null);
  const topAlerts = alerts.slice(0, 6);
  const criticalCount = alerts.filter((alert) => alert.severity === "critical").length;
  const warningCount = alerts.filter((alert) => alert.severity === "warning").length;

  async function updateAlert(alert: FleetAlertRecord, action: FleetAlertStateRequest["action"]) {
    const pendingKey = `${alert.id}:${action}`;
    setPending(pendingKey);
    try {
      await onUpdate({
        alert_id: alert.id,
        action,
        muted_for_secs: action === "mute" ? 4 * 60 * 60 : null,
        reason: action === "mute" ? "panel mute" : action === "acknowledge" ? "panel acknowledgement" : "panel action",
        confirmed: true,
      });
    } finally {
      setPending(null);
    }
  }

  return (
    <div className="fleetAlertList" aria-label="Fleet alerts">
      <div className="fleetAlertHeader">
        <span>
          <AlertTriangle size={17} />
          <strong>Fleet alerts</strong>
        </span>
        <small>
          {alerts.length === 0
            ? "clear"
            : `${criticalCount} critical / ${warningCount} warning / ${stateCount} triaged`}
        </small>
      </div>
      {topAlerts.length === 0 ? (
        <span className="fleetAlertEmpty">No active alerts</span>
      ) : (
        topAlerts.map((alert) => (
          <div className={`fleetAlertRow ${alertTone(alert.severity)}`} key={alert.id}>
            <span className="status">{alert.severity}</span>
            <strong>{alert.title}</strong>
            <small>{alert.client_id ?? alert.target_id}</small>
            <span>{alert.detail}</span>
            <span className={`fleetAlertState ${alert.operator_state}`}>{alert.operator_state}</span>
            <div className="fleetAlertActions">
              {alert.operator_state === "open" && (
                <>
                  <button
                    type="button"
                    disabled={pending === `${alert.id}:acknowledge`}
                    onClick={() => void updateAlert(alert, "acknowledge")}
                  >
                    Ack
                  </button>
                  <button
                    type="button"
                    disabled={pending === `${alert.id}:mute`}
                    onClick={() => void updateAlert(alert, "mute")}
                  >
                    Mute
                  </button>
                  <button
                    type="button"
                    disabled={pending === `${alert.id}:escalate`}
                    onClick={() => void updateAlert(alert, "escalate")}
                  >
                    Escalate
                  </button>
                </>
              )}
              {alert.operator_state !== "open" && (
                <button
                  type="button"
                  disabled={pending === `${alert.id}:clear`}
                  onClick={() => void updateAlert(alert, "clear")}
                >
                  Clear
                </button>
              )}
            </div>
            {alert.state_reason && <small className="fleetAlertReason">{alert.state_reason}</small>}
          </div>
        ))
      )}
    </div>
  );
}

function alertTone(severity: string) {
  if (severity === "critical") {
    return "critical";
  }
  if (severity === "warning") {
    return "warning";
  }
  return "info";
}

function latestTelemetryRollupsByClient(rollups: TelemetryRollupRecord[]) {
  const latest = new Map<string, TelemetryRollupRecord>();
  for (const rollup of rollups) {
    const current = latest.get(rollup.client_id);
    if (!current || rollup.latest_observed_at > current.latest_observed_at) {
      latest.set(rollup.client_id, rollup);
    }
  }
  return latest;
}

function latestTelemetryNetworkRatesByClient(rates: TelemetryNetworkRateRecord[]) {
  const latest = new Map<string, Map<string, TelemetryNetworkRateRecord>>();
  for (const rate of rates) {
    const clientRates = latest.get(rate.client_id) ?? new Map<string, TelemetryNetworkRateRecord>();
    const current = clientRates.get(rate.interface);
    if (!current || rate.latest_observed_at > current.latest_observed_at) {
      clientRates.set(rate.interface, rate);
    }
    latest.set(rate.client_id, clientRates);
  }
  return new Map(
    Array.from(latest.entries(), ([clientId, byInterface]) => [clientId, Array.from(byInterface.values())]),
  );
}

function latestTelemetryTunnelsByClient(tunnels: TelemetryTunnelRecord[]) {
  const latest = new Map<string, Map<string, TelemetryTunnelRecord>>();
  for (const tunnel of tunnels) {
    const clientTunnels = latest.get(tunnel.client_id) ?? new Map<string, TelemetryTunnelRecord>();
    const current = clientTunnels.get(tunnel.interface);
    if (!current || tunnel.observed_at > current.observed_at) {
      clientTunnels.set(tunnel.interface, tunnel);
    }
    latest.set(tunnel.client_id, clientTunnels);
  }
  return new Map(
    Array.from(latest.entries(), ([clientId, byInterface]) => [clientId, Array.from(byInterface.values())]),
  );
}

function formatLoad(value: number | undefined) {
  return typeof value === "number" ? value.toFixed(2) : "-";
}

function formatMemoryUsed(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || rollup.memory_total_bytes_max <= 0) {
    return "-";
  }
  const used = rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg;
  return `${Math.round((used / rollup.memory_total_bytes_max) * 100)}%`;
}

function formatDiskFree(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || rollup.disk_total_bytes_max <= 0) {
    return "-";
  }
  const percent = Math.round((rollup.disk_available_bytes_avg / rollup.disk_total_bytes_max) * 100);
  return `${percent}% free`;
}

function formatNetworkBytes(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || (rollup.network_rx_bytes_max === 0 && rollup.network_tx_bytes_max === 0)) {
    return "-";
  }
  return `RX ${formatBytes(rollup.network_rx_bytes_max)} / TX ${formatBytes(rollup.network_tx_bytes_max)}`;
}

function formatNetworkRateSummary(rates: TelemetryNetworkRateRecord[]) {
  if (rates.length === 0) {
    return "-";
  }
  const rx = rates.reduce((total, rate) => total + rate.rx_bps_avg, 0);
  const tx = rates.reduce((total, rate) => total + rate.tx_bps_avg, 0);
  return `RX ${formatBitsPerSecond(rx)} / TX ${formatBitsPerSecond(tx)}`;
}

function formatPrivilege(capabilities: AgentView["capabilities"] | undefined) {
  if (!capabilities || capabilities.privilege_mode === "unknown") {
    return "Unknown";
  }
  const uid = typeof capabilities.effective_uid === "number" ? ` uid ${capabilities.effective_uid}` : "";
  return capabilities.privilege_mode === "root" ? `Root${uid}` : `Unprivileged${uid}`;
}

function formatTunnelCapability(capabilities: AgentView["capabilities"] | undefined) {
  if (!capabilities) {
    return "Unknown";
  }
  if (capabilities.can_manage_runtime_tunnels) {
    return "Client-managed runtime tunnels enabled";
  }
  return capabilities.can_attempt_privileged_ops
    ? "Unprivileged best-effort, root operations may be ineffective"
    : "Observation only";
}

function formatBitsPerSecond(value: number) {
  const units = ["bps", "Kbps", "Mbps", "Gbps", "Tbps"];
  let next = Math.max(0, value);
  let unit = 0;
  while (next >= 1000 && unit < units.length - 1) {
    next /= 1000;
    unit += 1;
  }
  return `${next >= 10 || unit === 0 ? Math.round(next) : next.toFixed(1)} ${units[unit]}`;
}

function formatBytes(value: number) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let next = Math.max(0, value);
  let unit = 0;
  while (next >= 1024 && unit < units.length - 1) {
    next /= 1024;
    unit += 1;
  }
  return `${next >= 10 || unit === 0 ? Math.round(next) : next.toFixed(1)} ${units[unit]}`;
}

function NetworkRateList({ rates }: { rates: TelemetryNetworkRateRecord[] }) {
  if (rates.length === 0) {
    return <DetailLine icon={<Network size={18} />} label="Interfaces" value="No rate samples" />;
  }
  return (
    <div className="timeline">
      <Network size={18} />
      <div>
        <strong>Interfaces</strong>
        <span>
          {rates
            .slice()
            .sort((left, right) => left.interface.localeCompare(right.interface))
            .map(
              (rate) =>
                `${rate.interface} RX ${formatBitsPerSecond(rate.rx_bps_avg)} / TX ${formatBitsPerSecond(rate.tx_bps_avg)}`,
            )
            .join("; ")}
        </span>
      </div>
    </div>
  );
}

function TunnelList({ tunnels }: { tunnels: TelemetryTunnelRecord[] }) {
  if (tunnels.length === 0) {
    return <DetailLine icon={<Network size={18} />} label="Runtime tunnels" value="No tunnel reports" />;
  }
  return (
    <div className="timeline">
      <Network size={18} />
      <div>
        <strong>Runtime tunnels</strong>
        <span>
          {tunnels
            .slice()
            .sort((left, right) => left.interface.localeCompare(right.interface))
            .map((tunnel) => `${tunnel.interface} ${tunnel.kind} ${tunnel.operstate ?? "unknown"} ${formatTunnelPolicy(tunnel)}`)
            .join("; ")}
        </span>
      </div>
    </div>
  );
}

function formatTunnelPolicy(tunnel: TelemetryTunnelRecord) {
  const adapterHealth = formatAdapterHealth(tunnel);
  const traffic = formatTunnelTraffic(tunnel);
  if (tunnel.plan_correlation === "matched_saved_plan") {
    const manager = tunnel.plan_runtime_manager ?? tunnel.ownership_mode;
    if (tunnel.mutation_policy === "observe_only_saved_plan") {
      return tunnel.plan_name
        ? `saved observed plan ${tunnel.plan_name} (${manager})${adapterHealth}${traffic}`
        : `saved observed plan (${manager})${adapterHealth}${traffic}`;
    }
    return tunnel.plan_name
      ? `managed by ${tunnel.plan_name} (${manager})${adapterHealth}${traffic}`
      : `managed (${manager})${adapterHealth}${traffic}`;
  }
  if (tunnel.promotion_required) {
    return `import candidate${adapterHealth}${traffic}`;
  }
  if (tunnel.mutation_policy === "managed_desired") {
    return `managed${adapterHealth}${traffic}`;
  }
  return `${tunnel.ownership_mode}${adapterHealth}${traffic}`;
}

function formatAdapterHealth(tunnel: TelemetryTunnelRecord) {
  const health = tunnel.adapter_health;
  if (!health) {
    return "";
  }
  if (health.success) {
    return " adapter healthy";
  }
  const reason = health.reason ?? health.status;
  return ` adapter ${reason}`;
}

function formatTunnelTraffic(tunnel: TelemetryTunnelRecord) {
  const source = tunnel.traffic_source;
  if (!source) {
    return "";
  }
  const status = tunnel.traffic_status && tunnel.traffic_status !== "ok" ? ` ${tunnel.traffic_status}` : "";
  return ` traffic ${source}${status}`;
}

function formatRollupSamples(rollup: TelemetryRollupRecord | null) {
  if (!rollup) {
    return "No rollup";
  }
  return `${rollup.sample_count} in ${Math.round(rollup.bucket_secs / 60)}m`;
}

function DetailLine({
  icon,
  label,
  mono = false,
  value,
}: {
  icon: ReactNode;
  label: string;
  mono?: boolean;
  value: string;
}) {
  return (
    <div className="timeline">
      {icon}
      <div>
        <strong>{label}</strong>
        <span className={mono ? "monoValue" : undefined}>{value}</span>
      </div>
    </div>
  );
}
