import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { Activity, AlertTriangle, Bell, Boxes, Clock3, Gauge, LockKeyhole, Network, RefreshCw, Server, Trash2 } from "lucide-react";
import { bulkOutcomeSummary, targetPreflightUnavailable, waitForBulkJobTargets, type BulkJobProgress } from "../bulkJobProgress";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleDataGrid, type ConsoleDataGridColumn } from "../components/ConsoleDataGrid";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { FailureReasonGroups } from "../components/ExecutionResultPanel";
import { Metric } from "../components/Metric";
import { usePanelDisplaySettings } from "../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../privilege";
import { selectorExpressionForClientIds } from "../searchExpression";
import {
  decodeOutputPreview,
  formatCompactTime,
  formatTime,
  formatVpsName,
  runPanelAction,
  shortId,
  type VpsNameDisplayMode,
} from "../utils";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
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
  DeleteAgentRequest,
  DeleteAgentResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";

type FleetDetailTab = "Overview" | "Telemetry" | "Jobs" | "Network";

const detailTabs: FleetDetailTab[] = ["Overview", "Telemetry", "Jobs", "Network"];

export function FleetWorkspace({
  activeSubpage,
  agents,
  apiError,
  fleetAlerts,
  fleetAlertStates,
  fleetAlertPolicies,
  fleetAlertNotificationChannels,
  fleetAlertNotifications,
  lastLiveEvent,
  onCreateJob,
  onDispatchFleetAlertNotifications,
  onDeleteAgent,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
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
  privilegeMaterial,
  wsState,
}: {
  activeSubpage: string;
  agents: AgentView[];
  apiError: string | null;
  fleetAlerts: FleetAlertRecord[];
  fleetAlertStates: FleetAlertStateRecord[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  fleetAlertNotificationChannels: FleetAlertNotificationChannelRecord[];
  fleetAlertNotifications: FleetAlertNotificationDeliveryRecord[];
  lastLiveEvent: string;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDispatchFleetAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onDeleteAgent: (clientId: string, request: DeleteAgentRequest) => Promise<DeleteAgentResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onProcessFleetAlertNotifications: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onSelectAgent: (agentId: string | null) => void;
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
  privilegeMaterial: PrivilegeMaterial | null;
  wsState: string;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [activeDetailTab, setActiveDetailTab] = useState<FleetDetailTab>("Overview");
  const [aliasDraft, setAliasDraft] = useState("");
  const [aliasPending, setAliasPending] = useState(false);
  const [aliasError, setAliasError] = useState<string | null>(null);
  const [deletePromptOpen, setDeletePromptOpen] = useState(false);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [interfacePending, setInterfacePending] = useState(false);
  const [interfaceError, setInterfaceError] = useState<string | null>(null);
  const [interfaceProgress, setInterfaceProgress] = useState<BulkJobProgress | null>(null);
  const [interfaceSnapshot, setInterfaceSnapshot] = useState<NetworkInterfacesSnapshot | null>(null);
  const [interfaceJobId, setInterfaceJobId] = useState<string | null>(null);
  const [interfacePayloadHash, setInterfacePayloadHash] = useState<string | null>(null);
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
  const selectedTrafficSummary = formatSignalTraffic(selectedRollup, selectedNetworkRates);
  const selectedSampleSummary = formatSignalSamples(selectedRollup, selectedNetworkRates);
  const selectedCapabilities = selectedAgent?.capabilities;
  const selectedCountry = selectedAgent ? countryFromTags(selectedAgent.tags) : null;
  const selectedProvider = selectedAgent ? providerFromTags(selectedAgent.tags) : null;
  const selectedDisplayTags = selectedAgent ? displayTags(selectedAgent.tags) : [];
  const fleetSubpage = ["instances", "alerts", "policies", "notifications"].includes(activeSubpage)
    ? activeSubpage
    : "instances";
  const fleetColumns = useMemo<ConsoleDataGridColumn<AgentView>[]>(
    () => [
      {
        id: "name",
        header: "Name",
        size: 300,
        minSize: 220,
        sortValue: (agent) => formatVpsName(agent, vpsNameDisplayMode),
        searchValue: (agent) =>
          `${formatVpsName(agent, vpsNameDisplayMode)} ${agent.id} ${agent.status} ${agent.registration_ip ?? ""} ${agent.last_ip ?? ""}`,
        cell: (agent) => (
          <span className="instance">
            <Server size={17} />
            <span>
              <strong>{formatVpsName(agent, vpsNameDisplayMode)}</strong>
              <ConsoleStatusBadge tone={agent.status === "online" ? "ok" : "warning"}>
                {agent.status}
              </ConsoleStatusBadge>
            </span>
          </span>
        ),
      },
      {
        id: "registration_ip",
        header: "Reg IP",
        size: 135,
        minSize: 110,
        sortValue: (agent) => agent.registration_ip ?? "",
        searchValue: (agent) => agent.registration_ip ?? "",
        cell: (agent) => <span className="monoValue">{agent.registration_ip ?? "unknown"}</span>,
      },
      {
        id: "last_ip",
        header: "Last IP",
        size: 135,
        minSize: 110,
        sortValue: (agent) => agent.last_ip ?? "",
        searchValue: (agent) => agent.last_ip ?? "",
        cell: (agent) => <span className="monoValue">{agent.last_ip ?? "unknown"}</span>,
      },
      {
        id: "last_seen",
        header: "Last seen",
        size: 150,
        minSize: 125,
        sortValue: (agent) => normalizedLastSeenSort(agent.last_seen_at),
        searchValue: (agent) => formatLastSeen(agent.last_seen_at),
        cell: (agent) => (
          <span className="historyPrimary">
            <strong>{formatLastSeen(agent.last_seen_at)}</strong>
            {!agent.last_seen_at && <small>after enrollment</small>}
          </span>
        ),
      },
      {
        id: "country",
        header: "Country",
        size: 110,
        minSize: 90,
        sortValue: (agent) => countryFromTags(agent.tags) ?? "",
        searchValue: (agent) => countryFromTags(agent.tags) ?? "",
        cell: (agent) => <span className="countryBadge">{countryLabel(countryFromTags(agent.tags))}</span>,
      },
      {
        id: "provider",
        header: "Provider",
        size: 130,
        minSize: 100,
        sortValue: (agent) => providerFromTags(agent.tags) ?? "",
        searchValue: (agent) => providerFromTags(agent.tags) ?? "",
        cell: (agent) => (
          <span className="tags providerTags">
            <em>{providerFromTags(agent.tags) || "unset"}</em>
          </span>
        ),
      },
      {
        id: "tags",
        header: "Tags",
        size: 240,
        minSize: 150,
        sortValue: (agent) => displayTags(agent.tags).join(" "),
        searchValue: (agent) => agent.tags.join(" "),
        cell: (agent) => {
          const agentTags = displayTags(agent.tags);
          return (
            <span className="tags">
              {agentTags.length === 0 ? <em>untagged</em> : agentTags.map((tag) => <em key={tag}>{tag}</em>)}
            </span>
          );
        },
      },
    ],
    [vpsNameDisplayMode],
  );

  useEffect(() => {
    setAliasDraft(selectedAgent?.display_name ?? "");
    setAliasError(null);
    setDeletePromptOpen(false);
    setDeleteError(null);
    setInterfaceError(null);
    setInterfaceProgress(null);
    setInterfaceSnapshot(null);
    setInterfaceJobId(null);
    setInterfacePayloadHash(null);
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

  async function deleteSelectedAgent() {
    if (!selectedAgent) {
      return;
    }
    const clientId = selectedAgent.id;
    await runPanelAction(setDeletePending, setDeleteError, async () => {
      await onDeleteAgent(clientId, {
        confirmed: true,
        reason: "Deleted from fleet inventory panel",
      });
      setDeletePromptOpen(false);
      onSelectAgent(null);
    });
  }

  async function refreshSelectedInterfaces() {
    await runPanelAction(setInterfacePending, setInterfaceError, async () => {
      if (!selectedAgent) {
        throw new Error("Select a VPS");
      }
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      const operation: JobOperation = { type: "network_interfaces" };
      const selectorExpression = selectorExpressionForClientIds([selectedAgent.id]);
      const builtPrivilege = await buildPrivilegeForJobOperation({
        clientIds: [selectedAgent.id],
        commandType: "network_interfaces",
        operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: 30,
      });
      setInterfacePayloadHash(builtPrivilege.payloadHashHex);
      setInterfaceSnapshot(null);
      setInterfaceProgress({
        accepted: 0,
        completed: 0,
        doing: 0,
        expected: 1,
        failed: 0,
        jobId: "",
        retrieved: 0,
        unavailable: targetPreflightUnavailable(selectedAgent) ? 1 : 0,
      });
      const job = await onCreateJob({
        argv: [],
        selector_expression: selectorExpression,
        command: "network_interfaces",
        confirmed: false,
        destructive: false,
        operation,
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: builtPrivilege.privilegeAssertion,
        timeout_secs: 30,
      });
      setInterfaceJobId(job.job_id);
      const progress = await waitForBulkJobTargets(job.job_id, onLoadJobTargets, {
        acceptedTargets: job.accepted_targets,
        onProgress: setInterfaceProgress,
        targets: [selectedAgent],
      });
      setInterfaceProgress(progress.progress);
      const outputs = await onLoadJobOutputs(job.job_id);
      const snapshot = parseNetworkInterfacesSnapshot(outputs);
      if (!snapshot) {
        throw new Error("No network interface snapshot returned");
      }
      setInterfaceSnapshot(snapshot);
    });
  }

  return (
    <section className={fleetSubpage === "instances" ? "workspace" : "workspace singleColumn"}>
      {fleetSubpage === "instances" && (
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>VPS instances</h2>
            <span>{apiError ? "API unavailable" : "Live control-plane inventory"}</span>
          </div>
          <span className="sectionContext">Tags scoped from inventory</span>
        </div>

        <ConsoleDataGrid
          actions={[
            {
              label: "Inspect selected",
              disabled: (rows) => rows.length !== 1,
              onSelect: (rows) => onSelectAgent(rows[0].id),
            },
            {
              label: "Copy client IDs",
              onSelect: (rows) => void copyText(rows.map((agent) => agent.id).join("\n")),
            },
            {
              label: "Copy tag query",
              onSelect: (rows) =>
                void copyText(
                  Array.from(new Set(rows.flatMap((agent) => agent.tags)))
                    .sort()
                    .map((tag) => `tag:${tag}`)
                    .join(" "),
                ),
            },
          ]}
          columns={fleetColumns}
          defaultPageSize={10}
          empty={
            <div className="emptyState">
              <Server size={22} />
              <strong>{scopeActive ? "No VPS match this view" : "No agents online"}</strong>
              <span>{apiError ?? (scopeActive ? "Adjust or clear the saved fleet view." : "Waiting for enrolled VPS agents to report in.")}</span>
            </div>
          }
          getRowId={(agent) => agent.id}
          itemLabel="instances"
          onOpenRow={(agent) => onSelectAgent(agent.id)}
          renderExpandedRow={(agent) => (
            <div className="gridDetailLine">
              <strong>{formatVpsName(agent, vpsNameDisplayMode)}</strong>
              <span>{agent.id}</span>
              <span>{agent.capabilities.privilege_mode}; uid {agent.capabilities.effective_uid ?? "unknown"}</span>
            </div>
          )}
          rows={agents}
          storageKey="vpsman.grid.fleet.instances"
          title="VPS instance records"
        />
      </div>
      )}

      {fleetSubpage === "alerts" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Fleet alerts</h2>
              <span>{apiError ?? `${fleetAlerts.length} active fleet alerts`}</span>
            </div>
            <span className="sectionContext">{fleetAlertStates.length} triaged states</span>
          </div>
          <FleetAlertList
            agents={agents}
            alerts={fleetAlerts}
            stateCount={fleetAlertStates.length}
            onUpdate={onUpdateFleetAlertState}
          />
        </div>
      )}

      {fleetSubpage === "policies" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Alert policies</h2>
              <span>{apiError ?? `${fleetAlertPolicies.length} scoped thresholds`}</span>
            </div>
            <span className="sectionContext">Thresholds resolve by tag, provider, client, or global scope</span>
          </div>
          <FleetAlertPolicyManager policies={fleetAlertPolicies} onUpsert={onUpsertFleetAlertPolicy} />
        </div>
      )}

      {fleetSubpage === "notifications" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Notification channels</h2>
              <span>{apiError ?? `${fleetAlertNotificationChannels.length} delivery channels`}</span>
            </div>
            <span className="sectionContext">{fleetAlertNotifications.length} retained deliveries</span>
          </div>
          <FleetAlertNotificationManager
            channels={fleetAlertNotificationChannels}
            deliveries={fleetAlertNotifications}
            onDispatch={onDispatchFleetAlertNotifications}
            onProcess={onProcessFleetAlertNotifications}
            onUpsert={onUpsertFleetAlertNotificationChannel}
          />
        </div>
      )}

      {fleetSubpage === "instances" && (
      <aside className="inspector">
        <div className="sectionHeader compact">
          <h2>{selectedAgent ? formatVpsName(selectedAgent, vpsNameDisplayMode) : "No VPS selected"}</h2>
          <span>WebSocket {wsState}</span>
        </div>
        {selectedAgent && (
          <form className="aliasEditor" onSubmit={submitAlias}>
            <label>
              <span>Display name</span>
              <input
                aria-label="VPS display name"
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
        {selectedAgent && (
          <div className="deleteVpsControls">
            <div>
              <strong>Delete VPS</strong>
              <span>Deactivate access immediately and remove this VPS from normal workflows.</span>
            </div>
            <button
              className="secondaryAction dangerAction"
              disabled={deletePending}
              onClick={() => setDeletePromptOpen(true)}
              type="button"
            >
              <Trash2 size={16} />
              Delete VPS
            </button>
          </div>
        )}
        {selectedAgent && (
          <ConfirmationPrompt
            confirmLabel="Delete VPS"
            detail="This deactivates VPS access immediately and permanently removes it from inventory, selectors, dashboard, tags, topology, and future bulk targeting. Historical jobs and audit records remain."
            items={[
              { label: "VPS", value: formatVpsName(selectedAgent, vpsNameDisplayMode) },
              { label: "Client ID", value: selectedAgent.id },
              { label: "Status", value: selectedAgent.status },
            ]}
            onCancel={() => setDeletePromptOpen(false)}
            onConfirm={() => void deleteSelectedAgent()}
            open={deletePromptOpen}
            pending={deletePending}
            title="Delete VPS from panel"
            tone="danger"
          />
        )}
        {deleteError && <small className="errorText">{deleteError}</small>}
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
          <Metric label="Traffic" value={selectedTrafficSummary} tone="blue" />
          <Metric label="Samples" value={selectedSampleSummary} tone="green" />
        </div>
        <div className="detailPane" role="tabpanel">
          {activeDetailTab === "Overview" && (
            <>
              <DetailLine
                icon={<Server size={18} />}
                label="Name"
                value={selectedAgent ? formatVpsName(selectedAgent, vpsNameDisplayMode) : "No target"}
              />
              <DetailLine icon={<Server size={18} />} label="Status" value={selectedAgent?.status ?? "No target"} />
              <DetailLine icon={<Boxes size={18} />} label="Client ID" value={selectedAgent?.id ?? "No VPS selected"} mono />
              <DetailLine
                icon={<Clock3 size={18} />}
                label="Last seen"
                value={formatLastSeenDetail(selectedAgent?.last_seen_at)}
              />
              <DetailLine
                icon={<Network size={18} />}
                label="Registration IP"
                value={selectedAgent?.registration_ip ?? "unknown"}
                mono
              />
              <DetailLine icon={<Network size={18} />} label="Last IP" value={selectedAgent?.last_ip ?? "unknown"} mono />
              <DetailLine icon={<Boxes size={18} />} label="Country" value={countryLabel(selectedCountry)} />
              <DetailLine icon={<Boxes size={18} />} label="Provider" value={selectedProvider || "unset"} />
              <DetailLine icon={<Gauge size={18} />} label="Privilege" value={formatPrivilege(selectedCapabilities)} />
              <DetailLine
                icon={<Gauge size={18} />}
                label="Fleet position"
                value={selectedAgent ? `${summary.online} online / ${summary.total} total` : "No VPS selected"}
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
              <DetailLine icon={<Network size={18} />} label="Network rate" value={formatNetworkRateSummary(selectedNetworkRates, selectedRollup)} />
              <DetailLine icon={<Activity size={18} />} label="Rollup samples" value={formatRollupSamples(selectedRollup)} />
              <DetailLine icon={<Server size={18} />} label="Agent status" value={selectedAgent?.status ?? "No VPS selected"} />
            </>
          )}
          {activeDetailTab === "Jobs" && (
            <>
              <DetailLine icon={<Gauge size={18} />} label="Running jobs" value={String(summary.running_jobs)} />
              <DetailLine icon={<Server size={18} />} label="Target" value={selectedAgent?.id ?? "No VPS selected"} mono />
              <DetailLine icon={<Activity size={18} />} label="Privilege state" value="Local unlock required" />
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
              <DetailLine icon={<Boxes size={18} />} label="Tags" value={selectedDisplayTags.join(", ") || "untagged"} />
              <NetworkInterfacesPanel
                error={interfaceError}
                jobId={interfaceJobId}
                onOpenJobDetails={onOpenJobDetails}
                onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
                onRefresh={() => void refreshSelectedInterfaces()}
                payloadHash={interfacePayloadHash}
                pending={interfacePending}
                progress={interfaceProgress}
                privilegeReady={Boolean(privilegeMaterial)}
                selectedAgent={selectedAgent}
                snapshot={interfaceSnapshot}
              />
              <TunnelList tunnels={selectedTunnels} />
              <NetworkRateList rates={selectedNetworkRates} rollup={selectedRollup} />
              <DetailLine icon={<Activity size={18} />} label="Tunnel apply" value="Observe and plan" />
              {selectedCapabilities?.unprivileged_hint && (
                <DetailLine icon={<Activity size={18} />} label="Privilege hint" value={selectedCapabilities.unprivileged_hint} />
              )}
            </>
          )}
        </div>
      </aside>
      )}
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

function agentNamesById(agents: AgentView[], mode: VpsNameDisplayMode): Map<string, string> {
  return new Map(agents.map((agent) => [agent.id, formatVpsName(agent, mode)]));
}

function countryFromTags(tags: string[]): string | null {
  const countryTag = tags.find((tag) => /^country[:=_-][a-z0-9_-]{2,32}$/i.test(tag));
  if (!countryTag) {
    return null;
  }
  const [, code] = countryTag.split(/[:=_-]/, 2);
  return code ? code.toUpperCase() : null;
}

function providerFromTags(tags: string[]): string | null {
  const providerTag = tags.find((tag) => /^provider[:=_-][a-z0-9_.-]{1,64}$/i.test(tag));
  if (!providerTag) {
    return null;
  }
  const [, provider] = providerTag.split(/[:=_-]/, 2);
  return provider || null;
}

function displayTags(tags: string[]): string[] {
  return tags
    .filter((tag) => !/^country[:=_-][a-z0-9_-]{2,32}$/i.test(tag))
    .filter((tag) => !/^provider[:=_-][a-z0-9_.-]{1,64}$/i.test(tag))
    .sort((left, right) => left.localeCompare(right));
}

function formatLastSeen(value: string | null | undefined): string {
  const normalized = normalizeAgentTimestamp(value);
  return normalized ? formatCompactTime(normalized) : "never seen";
}

function formatLastSeenDetail(value: string | null | undefined): string {
  const normalized = normalizeAgentTimestamp(value);
  return normalized ? formatTime(normalized) : "never seen after enrollment";
}

function normalizedLastSeenSort(value: string | null | undefined): string {
  return normalizeAgentTimestamp(value) ?? "";
}

function normalizeAgentTimestamp(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  if (!trimmed) {
    return null;
  }
  if (/^\d{10}$/.test(trimmed)) {
    return new Date(Number(trimmed) * 1000).toISOString();
  }
  if (/^\d{13}$/.test(trimmed)) {
    return new Date(Number(trimmed)).toISOString();
  }
  return trimmed;
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function countryLabel(country: string | null): string {
  if (!country) {
    return "unset";
  }
  if (/^[A-Z]{2}$/.test(country)) {
    return `${countryFlag(country)} ${country}`;
  }
  return country;
}

function countryFlag(country: string): string {
  const base = 0x1f1e6;
  return Array.from(country)
    .map((letter) => String.fromCodePoint(base + letter.charCodeAt(0) - 65))
    .join("");
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
  agents,
  alerts,
  stateCount,
  onUpdate,
}: {
  agents: AgentView[];
  alerts: FleetAlertRecord[];
  stateCount: number;
  onUpdate: (request: FleetAlertStateRequest) => Promise<FleetAlertStateRecord>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [pending, setPending] = useState<string | null>(null);
  const topAlerts = alerts.slice(0, 6);
  const criticalCount = alerts.filter((alert) => alert.severity === "critical").length;
  const warningCount = alerts.filter((alert) => alert.severity === "warning").length;
  const nameById = useMemo(() => agentNamesById(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);

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
            <small>{alert.client_id ? nameById.get(alert.client_id) ?? "Unnamed VPS" : alertTargetLabel(alert)}</small>
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

function alertTargetLabel(alert: FleetAlertRecord) {
  return alert.target_kind === "client" ? "Unknown VPS" : alert.target_id;
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
    if (!current || rate.bucket_start > current.bucket_start) {
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
  return typeof value === "number" ? value.toFixed(2) : "No rollup";
}

function formatMemoryUsed(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || rollup.memory_total_bytes_max <= 0) {
    return "No rollup";
  }
  const used = rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg;
  return `${Math.round((used / rollup.memory_total_bytes_max) * 100)}%`;
}

function formatDiskFree(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || rollup.disk_total_bytes_max <= 0) {
    return "No rollup";
  }
  const percent = Math.round((rollup.disk_available_bytes_avg / rollup.disk_total_bytes_max) * 100);
  return `${percent}% free`;
}

function formatNetworkBytes(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || (rollup.network_rx_bytes_max === 0 && rollup.network_tx_bytes_max === 0)) {
    return "No counters";
  }
  return `RX ${formatBytes(rollup.network_rx_bytes_max)} / TX ${formatBytes(rollup.network_tx_bytes_max)}`;
}

function formatNetworkRateSummary(rates: TelemetryNetworkRateRecord[], rollup: TelemetryRollupRecord | null | undefined) {
  if (rates.length === 0) {
    return rollup && (rollup.network_rx_bytes_max > 0 || rollup.network_tx_bytes_max > 0)
      ? "Rate rollup pending; counters active"
      : "Awaiting rate rollup";
  }
  const rx = rates.reduce((total, rate) => total + rate.rx_bps_avg, 0);
  const tx = rates.reduce((total, rate) => total + rate.tx_bps_avg, 0);
  return `RX ${formatBitsPerSecond(rx)} / TX ${formatBitsPerSecond(tx)}`;
}

function formatSignalTraffic(rollup: TelemetryRollupRecord | null | undefined, rates: TelemetryNetworkRateRecord[]) {
  if (rates.length > 0) {
    const totalBps = rates.reduce((total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg, 0);
    return formatBitsPerSecond(totalBps);
  }
  if (rollup && (rollup.network_rx_bytes_max > 0 || rollup.network_tx_bytes_max > 0)) {
    return formatBytes(rollup.network_rx_bytes_max + rollup.network_tx_bytes_max);
  }
  return "Awaiting rate rollup";
}

function formatSignalSamples(rollup: TelemetryRollupRecord | null | undefined, rates: TelemetryNetworkRateRecord[]) {
  if (rollup && rollup.sample_count > 0) {
    return `${rollup.sample_count} rollup`;
  }
  const rateSamples = rates.reduce((total, rate) => total + rate.sample_count, 0);
  return rateSamples > 0 ? `${rateSamples} rate` : "No rollup";
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

type NetworkInterfacesSnapshot = {
  type: "network_interfaces";
  client_id?: string;
  observed_unix?: number;
  interface_count?: number;
  address_source?: { status?: string; error?: string | null };
  sysfs_source?: { status?: string; error?: string | null };
  counter_source?: { status?: string };
  interfaces: NetworkInterfaceSnapshotRecord[];
};

type NetworkInterfaceSnapshotRecord = {
  name: string;
  ifindex?: number;
  operstate?: string;
  mtu?: number;
  mac?: string;
  link_type?: number;
  flags?: string[];
  addresses?: NetworkInterfaceAddressRecord[];
  rx_bytes?: number;
  tx_bytes?: number;
  metadata_sources?: string[];
};

type NetworkInterfaceAddressRecord = {
  family: string;
  address: string;
  prefix_len?: number;
  scope?: string;
};

function NetworkInterfacesPanel({
  error,
  jobId,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRefresh,
  payloadHash,
  pending,
  progress,
  privilegeReady,
  selectedAgent,
  snapshot,
}: {
  error: string | null;
  jobId: string | null;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => void;
  payloadHash: string | null;
  pending: boolean;
  progress: BulkJobProgress | null;
  privilegeReady: boolean;
  selectedAgent: AgentView | null;
  snapshot: NetworkInterfacesSnapshot | null;
}) {
  const online = selectedAgent?.status === "online";
  const status =
    error ??
    (pending
      ? "Refreshing"
      : progress
        ? bulkOutcomeSummary(progress)
        : snapshot
          ? `${snapshot.interfaces.length} interface${snapshot.interfaces.length === 1 ? "" : "s"}`
          : privilegeReady
            ? "No snapshot"
            : "Privilege locked");
  const observed =
    typeof snapshot?.observed_unix === "number"
      ? formatCompactTime(new Date(snapshot.observed_unix * 1000).toISOString())
      : null;
  return (
    <div className="timeline networkInterfacesPanel">
      <Network size={18} />
      <div>
        <strong>Host interfaces</strong>
        <span>
          {status}
          {observed ? `; seen ${observed}` : ""}
          {payloadHash ? `; payload ${payloadHash.slice(0, 12)}` : ""}
        </span>
        <div className="interfaceActions">
          <button
            className="secondaryAction compactAction"
            disabled={pending || !selectedAgent || !privilegeReady || !online}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCw size={15} />
            Refresh interfaces
          </button>
          {!privilegeReady && (
            <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
              <LockKeyhole size={15} />
              Unlock privilege
            </button>
          )}
          {jobId && onOpenJobDetails && (
            <button className="secondaryAction compactAction" onClick={() => onOpenJobDetails(jobId)} type="button">
              Job {shortId(jobId)}
            </button>
          )}
        </div>
        {progress && <FailureReasonGroups reasons={progress.failureReasons ?? []} />}
        {snapshot && <NetworkInterfaceList snapshot={snapshot} />}
      </div>
    </div>
  );
}

function NetworkInterfaceList({ snapshot }: { snapshot: NetworkInterfacesSnapshot }) {
  if (snapshot.interfaces.length === 0) {
    return <span>No interfaces returned</span>;
  }
  return (
    <div className="networkInterfaceList">
      {snapshot.interfaces
        .slice()
        .sort((left, right) => left.name.localeCompare(right.name))
        .map((networkInterface) => (
          <div className="networkInterfaceRow" key={networkInterface.name}>
            <strong>{networkInterface.name}</strong>
            <span>{interfaceStateSummary(networkInterface)}</span>
            <span>{interfaceAddressSummary(networkInterface)}</span>
            <span>{interfaceTrafficSummary(networkInterface)}</span>
          </div>
        ))}
    </div>
  );
}

function interfaceStateSummary(networkInterface: NetworkInterfaceSnapshotRecord) {
  const state = networkInterface.operstate ?? (networkInterface.flags?.includes("up") ? "up" : "unknown");
  const mtu = typeof networkInterface.mtu === "number" ? `mtu ${networkInterface.mtu}` : "mtu unknown";
  const mac = networkInterface.mac ? `mac ${networkInterface.mac}` : "mac unknown";
  return `${state}; ${mtu}; ${mac}`;
}

function interfaceAddressSummary(networkInterface: NetworkInterfaceSnapshotRecord) {
  const addresses = networkInterface.addresses ?? [];
  if (addresses.length === 0) {
    return "no IPs reported";
  }
  return addresses
    .map((address) => `${address.family} ${address.address}${typeof address.prefix_len === "number" ? `/${address.prefix_len}` : ""}`)
    .join(", ");
}

function interfaceTrafficSummary(networkInterface: NetworkInterfaceSnapshotRecord) {
  const rxBytes = typeof networkInterface.rx_bytes === "number" ? networkInterface.rx_bytes : 0;
  const txBytes = typeof networkInterface.tx_bytes === "number" ? networkInterface.tx_bytes : 0;
  return `RX ${formatBytes(rxBytes)} / TX ${formatBytes(txBytes)}`;
}

function parseNetworkInterfacesSnapshot(outputs: JobOutputRecord[]): NetworkInterfacesSnapshot | null {
  const snapshots = outputs
    .filter((output) => output.stream === "status" && output.data_base64)
    .map((output) => {
      try {
        const value = JSON.parse(decodeOutputPreview(output.data_base64)) as unknown;
        return isNetworkInterfacesSnapshot(value) ? value : null;
      } catch {
        return null;
      }
    })
    .filter((value): value is NetworkInterfacesSnapshot => value !== null);
  return snapshots.length > 0 ? snapshots[snapshots.length - 1] : null;
}

function isNetworkInterfacesSnapshot(value: unknown): value is NetworkInterfacesSnapshot {
  if (!value || typeof value !== "object") {
    return false;
  }
  const record = value as Partial<NetworkInterfacesSnapshot>;
  return record.type === "network_interfaces" && Array.isArray(record.interfaces);
}

function NetworkRateList({
  rates,
  rollup,
}: {
  rates: TelemetryNetworkRateRecord[];
  rollup: TelemetryRollupRecord | null | undefined;
}) {
  if (rates.length === 0) {
    return (
      <DetailLine
        icon={<Network size={18} />}
        label="Interfaces"
        value={
          rollup && (rollup.network_rx_bytes_max > 0 || rollup.network_tx_bytes_max > 0)
            ? "Counter-only telemetry; rate rollup pending"
            : "Awaiting rate rollup"
        }
      />
    );
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
