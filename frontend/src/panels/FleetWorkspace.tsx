import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { basicSetup, EditorView } from "codemirror";
import {
  Activity,
  AlertTriangle,
  ArrowUpCircle,
  BarChart3,
  Bell,
  Boxes,
  Check,
  CircleCheck,
  Clock3,
  Eye,
  FileCog,
  FolderOpen,
  Gauge,
  LockKeyhole,
  Network,
  Pencil,
  Plus,
  Power,
  PowerOff,
  RefreshCw,
  Server,
  Tags,
  TerminalSquare,
  Trash2,
  VolumeX,
  X,
} from "lucide-react";
import {
  buildBulkJobProgress,
  bulkProgressTimeoutMs,
  bulkOutcomeSummary,
  createJobTargetCount,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { WEBHOOK_RULE_DELIVERY_HISTORY_STATUSES } from "../generated/protocolContracts";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { FailureReasonGroups } from "../components/ExecutionResultPanel";
import { Metric } from "../components/Metric";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../components/TimeSeriesChart";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  addressFamilyLabel,
  latencyStatusLabel,
  mutationPolicyLabel,
  ospfStatusLabel,
  planCorrelationLabel,
  runtimeManagerLabel,
  telemetryReasonLabel,
  telemetrySourceLabel,
  trafficStatusLabel,
} from "../topologyRuntime";
import {
  buildPrivilegeAssertion,
  buildPrivilegeForJobOperation,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../privilege";
import { selectorExpressionForClientIds } from "../searchExpression";
import { WEBHOOK_EXPRESSION_SUGGESTIONS } from "../webhookExpressionSuggestions";
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
  ActiveView,
  AgentView,
  BulkTagMutationRequest,
  CreateJobRequest,
  CreateJobResponse,
  DataSourceHotConfigResponse,
  DataSourcePresetAssignmentRecord,
  DataSourceStatusRecord,
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
  WebhookRuleDeliveryRecord,
  WebhookRuleDeliveryHistoryStatus,
  WebhookDeliveryRotationRequest,
  WebhookDeliveryRotationResponse,
  WebhookRuleDispatchRequest,
  WebhookRuleDryRunRecord,
  WebhookRuleDryRunRequest,
  WebhookRuleProcessRequest,
  WebhookRuleRecord,
  WebhookRuleRequest,
  DeleteAgentRequest,
  DeleteAgentResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
  TagMutationResponse,
  TagView,
} from "../types";

type FleetDetailTab = "Overview" | "Telemetry" | "Jobs" | "Network" | "Config";
type FleetSelectionStatsMode =
  | "telemetry"
  | "network"
  | "overview"
  | "capabilities";

const detailTabs: FleetDetailTab[] = [
  "Overview",
  "Telemetry",
  "Jobs",
  "Network",
  "Config",
];
const selectionStatsModes: Array<{
  id: FleetSelectionStatsMode;
  label: string;
}> = [
  { id: "telemetry", label: "Telemetry" },
  { id: "network", label: "Network" },
  { id: "overview", label: "Overview" },
  { id: "capabilities", label: "Config" },
];

const JOB_SELECTOR_STORAGE_KEY = "vpsman.jobDispatch.selectorExpression";
const MULTI_FILE_SELECTOR_STORAGE_KEY = "vpsman.multiFile.selectorExpression";
const TAG_BULK_SELECTOR_STORAGE_KEY = "vpsman.tags.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY =
  "vpsman.config.single.selectorExpression";
const CONFIG_BULK_SELECTOR_STORAGE_KEY =
  "vpsman.config.bulk.selectorExpression";
const FILE_BROWSER_STATE_STORAGE_KEY = "vpsman.fileBrowser.state";
const CHART_COLORS = ["#1a73e8", "#188038", "#f29900", "#a142f4", "#d93025"];

export function FleetWorkspace({
  activeSubpage,
  agents,
  apiError,
  fleetAlerts,
  fleetAlertStates,
  fleetAlertPolicies,
  fleetAlertNotificationChannels,
  fleetAlertNotifications,
  webhookRules,
  webhookRuleDeliveries,
  lastLiveEvent,
  dataSourceAssignments,
  dataSourceStatus,
  onCreateJob,
  onBulkMutateTags,
  onNavigatePanel,
  onRenderDataSourceHotConfig,
  onDeleteFleetAlertNotificationChannel,
  onDeleteFleetAlertPolicy,
  onDeleteWebhookRule,
  onDispatchFleetAlertNotifications,
  onDispatchWebhookRules,
  onDryRunWebhookRule,
  onDeleteAgent,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onProcessFleetAlertNotifications,
  onProcessWebhookRuleDeliveries,
  onRotateWebhookDeliveryHistory,
  onSelectAgent,
  onUpdateAgentAlias,
  onUpdateFleetAlertState,
  onUpsertFleetAlertNotificationChannel,
  onUpsertFleetAlertPolicy,
  onUpsertWebhookRule,
  scopeActive,
  selectedAgent,
  summary,
  tags,
  targetAgents,
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
  webhookRules: WebhookRuleRecord[];
  webhookRuleDeliveries: WebhookRuleDeliveryRecord[];
  lastLiveEvent: string;
  dataSourceAssignments: DataSourcePresetAssignmentRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onBulkMutateTags: (
    request: BulkTagMutationRequest,
  ) => Promise<TagMutationResponse>;
  onNavigatePanel?: (view: ActiveView, subpage: string) => void;
  onRenderDataSourceHotConfig: (
    clientId: string,
  ) => Promise<DataSourceHotConfigResponse>;
  onDeleteFleetAlertNotificationChannel: (channelId: string) => Promise<void>;
  onDeleteFleetAlertPolicy: (policyId: string) => Promise<void>;
  onDeleteWebhookRule: (ruleId: string) => Promise<void>;
  onDispatchFleetAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onDispatchWebhookRules: (
    request: WebhookRuleDispatchRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onDryRunWebhookRule: (
    request: WebhookRuleDryRunRequest,
  ) => Promise<WebhookRuleDryRunRecord>;
  onDeleteAgent: (
    clientId: string,
    request: DeleteAgentRequest,
  ) => Promise<DeleteAgentResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onProcessFleetAlertNotifications: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onProcessWebhookRuleDeliveries: (
    request: WebhookRuleProcessRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onRotateWebhookDeliveryHistory: (
    request: WebhookDeliveryRotationRequest,
  ) => Promise<WebhookDeliveryRotationResponse>;
  onSelectAgent: (agentId: string | null) => void;
  onUpdateAgentAlias: (
    clientId: string,
    displayName: string,
  ) => Promise<AgentView>;
  onUpdateFleetAlertState: (
    request: FleetAlertStateRequest,
  ) => Promise<FleetAlertStateRecord>;
  onUpsertFleetAlertNotificationChannel: (
    request: FleetAlertNotificationChannelRequest,
  ) => Promise<FleetAlertNotificationChannelRecord>;
  onUpsertFleetAlertPolicy: (
    request: FleetAlertPolicyRequest,
  ) => Promise<FleetAlertPolicyRecord>;
  onUpsertWebhookRule: (
    request: WebhookRuleRequest,
  ) => Promise<WebhookRuleRecord>;
  scopeActive: boolean;
  selectedAgent: AgentView | null;
  summary: FleetSummary;
  tags: TagView[];
  targetAgents: AgentView[];
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  privilegeMaterial: PrivilegeMaterial | null;
  wsState: string;
}) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectionStatsMode, setSelectionStatsMode] =
    useState<FleetSelectionStatsMode>("telemetry");
  const [deleteTarget, setDeleteTarget] = useState<AgentView | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const latestRollups = useMemo(
    () => latestTelemetryRollupsByClient(telemetryRollups),
    [telemetryRollups],
  );
  const latestNetworkRates = useMemo(
    () => latestTelemetryNetworkRatesByClient(telemetryNetworkRates),
    [telemetryNetworkRates],
  );
  const latestTunnels = useMemo(
    () => latestTelemetryTunnelsByClient(telemetryTunnels),
    [telemetryTunnels],
  );
  const fleetSubpage = [
    "instances",
    "alerts",
    "policies",
    "notifications",
  ].includes(activeSubpage)
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
              <ConsoleStatusBadge
                tone={agent.status === "online" ? "ok" : "warning"}
              >
                {agent.status}
              </ConsoleStatusBadge>
            </span>
          </span>
        ),
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
            {!agent.last_seen_at && <small>until first gateway report</small>}
          </span>
        ),
      },
      {
        id: "tags",
        header: "Tags",
        size: 260,
        minSize: 170,
        sortValue: (agent) => displayTags(agent.tags).join(" "),
        searchValue: (agent) => agent.tags.join(" "),
        cell: (agent) => {
          const agentTags = displayTags(agent.tags);
          return (
            <span className="tags">
              {agentTags.length === 0 ? (
                <em>untagged</em>
              ) : (
                agentTags.map((tag) => <em key={tag}>{tag}</em>)
              )}
            </span>
          );
        },
      },
      {
        id: "country",
        header: "Country",
        size: 110,
        minSize: 90,
        sortValue: (agent) => countryFromTags(agent.tags) ?? "",
        searchValue: (agent) => countryFromTags(agent.tags) ?? "",
        cell: (agent) => (
          <CountryBadge
            country={countryFromTags(agent.tags)}
            showFlag={preferences.show_country_flags}
          />
        ),
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
        id: "last_ip",
        header: "Last IP",
        size: 135,
        minSize: 110,
        sortValue: (agent) => agent.last_ip ?? "",
        searchValue: (agent) => agent.last_ip ?? "",
        cell: (agent) => (
          <span className="monoValue">{agent.last_ip ?? "unknown"}</span>
        ),
      },
      {
        id: "registration_ip",
        header: "Reg IP",
        size: 135,
        minSize: 110,
        sortValue: (agent) => agent.registration_ip ?? "",
        searchValue: (agent) => agent.registration_ip ?? "",
        cell: (agent) => (
          <span className="monoValue">
            {agent.registration_ip ?? "unknown"}
          </span>
        ),
      },
    ],
    [preferences.show_country_flags, vpsNameDisplayMode],
  );

  async function mutateTagsForAgents(
    rows: AgentView[],
    action: "add" | "remove",
    tag: string,
  ) {
    const normalizedTag = tag.trim();
    if (!normalizedTag) {
      throw new Error("Tag is required");
    }
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Privilege unlock is required");
    }
    const targetIds = rows.map((agent) => agent.id);
    const selectorExpression = selectorExpressionForClientIds(targetIds);
    const privilegeAssertion = await buildPrivilegeAssertion({
      intent: canonicalDbPrivilegeIntent({
        action: action === "add" ? "tag.bulk_add" : "tag.bulk_remove",
        confirmed: true,
        resolvedTargets: targetIds,
        selectorExpression,
        target: normalizedTag,
      }),
      privilegeMaterial,
    });
    return onBulkMutateTags({
      action,
      confirmed: true,
      privilege_assertion: privilegeAssertion,
      selector_expression: selectorExpression,
      tag: normalizedTag,
    });
  }

  function openSelectorWorkflow(
    rows: AgentView[],
    view: ActiveView,
    subpage: string,
    storageKey: string,
  ) {
    const selectorExpression = selectorExpressionForClientIds(
      rows.map((agent) => agent.id),
    );
    writeLocalString(storageKey, selectorExpression);
    if (rows.length === 1) {
      onSelectAgent(rows[0].id);
    }
    onNavigatePanel?.(view, subpage);
  }

  function openFileBrowserWorkflow(rows: AgentView[]) {
    if (rows.length !== 1) {
      return;
    }
    seedSingleFileBrowser(rows[0]);
    onNavigatePanel?.("Jobs", "files");
  }

  function requestDeleteAgent(rows: AgentView[]) {
    if (rows.length !== 1) {
      return;
    }
    setDeleteError(null);
    setDeleteTarget(rows[0]);
  }

  async function confirmDeleteAgent() {
    if (!deleteTarget) {
      return;
    }
    await runPanelAction(setDeletePending, setDeleteError, async () => {
      await onDeleteAgent(deleteTarget.id, {
        confirmed: true,
        reason: "Deleted from fleet inventory selection action",
      });
      setDeleteTarget(null);
      onSelectAgent(null);
    });
  }

  return (
    <section
      className={
        fleetSubpage === "instances"
          ? "workspace singleColumn fleetInstancesWorkspace"
          : "workspace singleColumn"
      }
    >
      {fleetSubpage === "instances" && (
        <div className="fleetPanel fleetInstancesPanel">
          <div className="sectionHeader fleetInstancesHeader">
            <div>
              <h2>VPS instances</h2>
              <span>
                {apiError ? "API unavailable" : "Live control-plane inventory"}
              </span>
            </div>
            <span className="sectionContext">
              {summary.online} online / {summary.total} total · WebSocket{" "}
              {wsState}
            </span>
          </div>

          <ConsoleDataGrid
            actions={[
              {
                label: "Inspect selected",
                disabled: (rows) => rows.length !== 1,
                expandRow: true,
                onSelect: (rows) => onSelectAgent(rows[0].id),
              },
              {
                label: "Open bulk execution",
                onSelect: (rows) =>
                  openSelectorWorkflow(
                    rows,
                    "Jobs",
                    "dispatch",
                    JOB_SELECTOR_STORAGE_KEY,
                  ),
              },
              {
                label: "Open multi-file actions",
                onSelect: (rows) =>
                  openSelectorWorkflow(
                    rows,
                    "Jobs",
                    "multi_files",
                    MULTI_FILE_SELECTOR_STORAGE_KEY,
                  ),
              },
              {
                label: "Open file browser",
                disabled: (rows) => rows.length !== 1,
                onSelect: openFileBrowserWorkflow,
              },
              {
                label: "Open bulk tags",
                onSelect: (rows) =>
                  openSelectorWorkflow(
                    rows,
                    "Tags",
                    "bulk",
                    TAG_BULK_SELECTOR_STORAGE_KEY,
                  ),
              },
              {
                label: "Open config bulk apply",
                onSelect: (rows) =>
                  openSelectorWorkflow(
                    rows,
                    "Config",
                    "bulk",
                    CONFIG_BULK_SELECTOR_STORAGE_KEY,
                  ),
              },
              {
                label: "Open single config",
                disabled: (rows) => rows.length !== 1,
                onSelect: (rows) =>
                  openSelectorWorkflow(
                    rows,
                    "Config",
                    "single",
                    CONFIG_SINGLE_SELECTOR_STORAGE_KEY,
                  ),
              },
              {
                label: "Copy client IDs",
                onSelect: (rows) =>
                  void copyText(rows.map((agent) => agent.id).join("\n")),
              },
              {
                label: "Copy selector",
                onSelect: (rows) =>
                  void copyText(
                    selectorExpressionForClientIds(
                      rows.map((agent) => agent.id),
                    ),
                  ),
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
              {
                label: "Review VPS deletion",
                description: (rows) =>
                  rows.length === 1
                    ? `Delete ${formatVpsName(rows[0], vpsNameDisplayMode)}`
                    : "Select exactly one VPS to delete.",
                disabled: (rows) => rows.length !== 1,
                icon: <Trash2 size={15} />,
                onSelect: requestDeleteAgent,
                tone: "danger",
              },
            ]}
            columns={fleetColumns}
            defaultColumnVisibility={{ last_ip: false, registration_ip: false }}
            defaultPageSize={20}
            empty={
              <div className="emptyState">
                <Server size={22} />
                <strong>
                  {scopeActive ? "No VPS match this view" : "No agents online"}
                </strong>
                <span>
                  {apiError ??
                    (scopeActive
                      ? "Adjust or clear the saved fleet view."
                      : "Waiting for VPS agents to connect through gateways and report in.")}
                </span>
              </div>
            }
            expandOnRowClick
            getRowId={(agent) => agent.id}
            itemLabel="instances"
            onOpenRow={(agent) => onSelectAgent(agent.id)}
            renderExpandedRow={(agent) => (
              <FleetInstanceDetail
                agent={agent}
                dataSourceAssignments={dataSourceAssignments.filter(
                  (assignment) => assignment.client_id === agent.id,
                )}
                dataSourceStatus={dataSourceStatus.filter(
                  (status) => status.client_id === agent.id,
                )}
                lastLiveEvent={lastLiveEvent}
                latestNetworkRates={latestNetworkRates.get(agent.id) ?? []}
                latestRollup={latestRollups.get(agent.id) ?? null}
                latestTunnels={latestTunnels.get(agent.id) ?? []}
                mutateTagsForAgents={mutateTagsForAgents}
                onCreateJob={onCreateJob}
                onLoadJobOutputs={onLoadJobOutputs}
                onLoadJobTargets={onLoadJobTargets}
                onOpenJobDetails={onOpenJobDetails}
                onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
                onRenderDataSourceHotConfig={onRenderDataSourceHotConfig}
                onUpdateAgentAlias={onUpdateAgentAlias}
                privilegeMaterial={privilegeMaterial}
                showCountryFlags={preferences.show_country_flags}
                summary={summary}
                telemetryNetworkRates={telemetryNetworkRates.filter(
                  (rate) => rate.client_id === agent.id,
                )}
                telemetryRollups={telemetryRollups.filter(
                  (rollup) => rollup.client_id === agent.id,
                )}
                vpsNameDisplayMode={vpsNameDisplayMode}
                wsState={wsState}
              />
            )}
            renderSelectionPanel={(rows) => (
              <FleetSelectionPanel
                agents={rows}
                allTags={tags}
                latestNetworkRates={latestNetworkRates}
                latestRollups={latestRollups}
                mutateTagsForAgents={mutateTagsForAgents}
                onOpenFileBrowser={openFileBrowserWorkflow}
                onOpenSelectorWorkflow={openSelectorWorkflow}
                selectionStatsMode={selectionStatsMode}
                setSelectionStatsMode={setSelectionStatsMode}
                vpsNameDisplayMode={vpsNameDisplayMode}
              />
            )}
            rows={agents}
            singleExpandedRow
            storageKey="vpsman.grid.fleet.instances.v2"
            title="VPS instance records"
          />
          <ConfirmationPrompt
            confirmLabel="Delete VPS"
            detail="This deactivates VPS access immediately and permanently removes it from inventory, selectors, dashboard, tags, topology, and future bulk targeting. Historical jobs and audit records remain."
            error={deleteError}
            items={
              deleteTarget
                ? [
                    {
                      label: "VPS",
                      value: formatVpsName(deleteTarget, vpsNameDisplayMode),
                    },
                    { label: "Client ID", value: deleteTarget.id },
                    { label: "Status", value: deleteTarget.status },
                  ]
                : []
            }
            onCancel={() => {
              setDeleteError(null);
              setDeleteTarget(null);
            }}
            onConfirm={() => void confirmDeleteAgent()}
            open={Boolean(deleteTarget)}
            pending={deletePending}
            title="Delete VPS from panel"
            tone="danger"
          />
        </div>
      )}

      {fleetSubpage === "alerts" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Fleet alerts</h2>
              <span>{`${fleetAlerts.length} active fleet alerts`}</span>
            </div>
            <span className="sectionContext">
              {fleetAlertStates.length} triaged states
            </span>
          </div>
          <ConsoleFreshnessBanner error={apiError} />
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
              <span>{`${fleetAlertPolicies.length} scoped thresholds`}</span>
            </div>
            <span className="sectionContext">
              Thresholds resolve by tag, provider, client, or global scope
            </span>
          </div>
          <ConsoleFreshnessBanner error={apiError} />
          <FleetAlertPolicyManager
            agents={targetAgents}
            onDelete={onDeleteFleetAlertPolicy}
            onUpsert={onUpsertFleetAlertPolicy}
            policies={fleetAlertPolicies}
          />
        </div>
      )}

      {fleetSubpage === "notifications" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Notification channels</h2>
              <span>{`${fleetAlertNotificationChannels.length} alert channels, ${webhookRules.length} expression webhooks`}</span>
            </div>
            <span className="sectionContext">
              {fleetAlertNotifications.length + webhookRuleDeliveries.length}{" "}
              retained deliveries
            </span>
          </div>
          <ConsoleFreshnessBanner error={apiError} />
          <FleetNotificationsHub
            agents={targetAgents}
            alertChannels={fleetAlertNotificationChannels}
            alertDeliveries={fleetAlertNotifications}
            webhookDeliveries={webhookRuleDeliveries}
            webhookRules={webhookRules}
            onDeleteAlertChannel={onDeleteFleetAlertNotificationChannel}
            onDeleteWebhookRule={onDeleteWebhookRule}
            onDispatchAlertNotifications={onDispatchFleetAlertNotifications}
            onDispatchWebhookRules={onDispatchWebhookRules}
            onDryRunWebhookRule={onDryRunWebhookRule}
            onProcessAlertNotifications={onProcessFleetAlertNotifications}
            onProcessWebhookRuleDeliveries={onProcessWebhookRuleDeliveries}
            onRotateWebhookDeliveryHistory={onRotateWebhookDeliveryHistory}
            onUpsertAlertChannel={onUpsertFleetAlertNotificationChannel}
            onUpsertWebhookRule={onUpsertWebhookRule}
          />
        </div>
      )}
    </section>
  );
}

function FleetInstanceDetail({
  agent,
  dataSourceAssignments,
  dataSourceStatus,
  lastLiveEvent,
  latestNetworkRates,
  latestRollup,
  latestTunnels,
  mutateTagsForAgents,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRenderDataSourceHotConfig,
  onUpdateAgentAlias,
  privilegeMaterial,
  showCountryFlags,
  summary,
  telemetryNetworkRates,
  telemetryRollups,
  vpsNameDisplayMode,
  wsState,
}: {
  agent: AgentView;
  dataSourceAssignments: DataSourcePresetAssignmentRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  lastLiveEvent: string;
  latestNetworkRates: TelemetryNetworkRateRecord[];
  latestRollup: TelemetryRollupRecord | null;
  latestTunnels: TelemetryTunnelRecord[];
  mutateTagsForAgents: (
    rows: AgentView[],
    action: "add" | "remove",
    tag: string,
  ) => Promise<TagMutationResponse>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRenderDataSourceHotConfig: (
    clientId: string,
  ) => Promise<DataSourceHotConfigResponse>;
  onUpdateAgentAlias: (
    clientId: string,
    displayName: string,
  ) => Promise<AgentView>;
  privilegeMaterial: PrivilegeMaterial | null;
  showCountryFlags: boolean;
  summary: FleetSummary;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  vpsNameDisplayMode: VpsNameDisplayMode;
  wsState: string;
}) {
  const [activeDetailTab, setActiveDetailTab] =
    useState<FleetDetailTab>("Overview");
  const [aliasDraft, setAliasDraft] = useState(agent.display_name ?? "");
  const [aliasPending, setAliasPending] = useState(false);
  const [aliasError, setAliasError] = useState<string | null>(null);
  const [tagDraft, setTagDraft] = useState("");
  const [tagPending, setTagPending] = useState(false);
  const [tagStatus, setTagStatus] = useState<string | null>(null);
  const [tagError, setTagError] = useState<string | null>(null);
  const [interfacePending, setInterfacePending] = useState(false);
  const [interfaceError, setInterfaceError] = useState<string | null>(null);
  const [interfaceProgress, setInterfaceProgress] =
    useState<BulkJobProgress | null>(null);
  const [interfaceSnapshot, setInterfaceSnapshot] =
    useState<NetworkInterfacesSnapshot | null>(null);
  const [interfaceJobId, setInterfaceJobId] = useState<string | null>(null);
  const [interfacePayloadHash, setInterfacePayloadHash] = useState<
    string | null
  >(null);
  const [configPending, setConfigPending] = useState(false);
  const [configError, setConfigError] = useState<string | null>(null);
  const [configPreview, setConfigPreview] =
    useState<DataSourceHotConfigResponse | null>(null);
  const country = countryFromTags(agent.tags);
  const provider = providerFromTags(agent.tags);
  const displayOnlyTags = displayTags(agent.tags);
  const isNetworkManaged = agent.tags.some((tag) =>
    ["bgp", "bird2", "ospf", "tunnel"].includes(tag.toLowerCase()),
  );
  const agentLabel = formatVpsName(agent, vpsNameDisplayMode);
  const configPreviewSummary = configPreview
    ? `${configPreview.assignments.length} assignments · ${configPreview.unsupported_domains.length} unsupported domains`
    : "Load redacted hot-config view for this VPS.";

  useEffect(() => {
    setAliasDraft(agent.display_name ?? "");
    setAliasError(null);
    setTagDraft("");
    setTagError(null);
    setTagStatus(null);
    setInterfaceError(null);
    setInterfaceProgress(null);
    setInterfaceSnapshot(null);
    setInterfaceJobId(null);
    setInterfacePayloadHash(null);
    setConfigError(null);
    setConfigPreview(null);
  }, [agent.display_name, agent.id]);

  async function submitAlias(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const displayName = aliasDraft.trim();
    if (!displayName) {
      setAliasError("Alias is required");
      return;
    }
    setAliasPending(true);
    setAliasError(null);
    try {
      await onUpdateAgentAlias(agent.id, displayName);
    } catch (error) {
      setAliasError(
        error instanceof Error ? error.message : "Alias update failed",
      );
    } finally {
      setAliasPending(false);
    }
  }

  async function refreshInterfaces() {
    await runPanelAction(setInterfacePending, setInterfaceError, async () => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is locked");
      }
      const operation: JobOperation = { type: "network_interfaces" };
      const selectorExpression = selectorExpressionForClientIds([agent.id]);
      const builtPrivilege = await buildPrivilegeForJobOperation({
        clientIds: [agent.id],
        commandType: "network_interfaces",
        operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: 30,
      });
      setInterfacePayloadHash(builtPrivilege.payloadHashHex);
      setInterfaceSnapshot(null);
      setInterfaceProgress(buildBulkJobProgress({
        jobId: "",
        targetCount: 1,
        targetRecords: [],
        targets: [agent],
      }));
      const job = await onCreateJob({
        argv: [],
        selector_expression: selectorExpression,
        target_client_ids: [agent.id],
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
      const targetCount = createJobTargetCount(job);
      const progress = await waitForBulkJobTargets(
        job.job_id,
        onLoadJobTargets,
        {
          onProgress: setInterfaceProgress,
          targetCount,
          targets: [agent],
          timeoutMs: bulkProgressTimeoutMs(30),
        },
      );
      setInterfaceProgress(progress.progress);
      if (progress.timedOut) {
        throw new Error("Timed out waiting for network interface snapshot target");
      }
      const outputs = await onLoadJobOutputs(job.job_id);
      const snapshot = parseNetworkInterfacesSnapshot(outputs);
      if (!snapshot) {
        throw new Error("No network interface snapshot returned");
      }
      setInterfaceSnapshot(snapshot);
    });
  }

  async function mutateTag(action: "add" | "remove", tag: string) {
    await runPanelAction(setTagPending, setTagError, async () => {
      const response = await mutateTagsForAgents([agent], action, tag);
      setTagStatus(
        `${response.action} ${response.tag}: ${response.changed_count} changed, ${response.skipped_count} skipped`,
      );
      setTagDraft("");
    });
  }

  async function loadRenderedConfig() {
    await runPanelAction(setConfigPending, setConfigError, async () => {
      setConfigPreview(await onRenderDataSourceHotConfig(agent.id));
    });
  }

  return (
    <div className="fleetNodeDetailCard">
      <div className="fleetNodeDetailHeader">
        <div>
          <h3>{agentLabel}</h3>
          <span className="monoValue">{agent.id}</span>
        </div>
        <span className="sectionContext">WebSocket {wsState}</span>
      </div>
      <div className="fleetNodeDetailControls">
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
            disabled={aliasPending || aliasDraft.trim() === agent.display_name}
            type="submit"
          >
            Rename
          </button>
          {aliasError && <small className="errorText">{aliasError}</small>}
        </form>
        <form
          className="fleetInlineTagForm"
          onSubmit={(event) => {
            event.preventDefault();
            void mutateTag("add", tagDraft);
          }}
        >
          <label>
            <span>Add tag</span>
            <input
              aria-label="Fleet inline tag"
              onChange={(event) => setTagDraft(event.target.value)}
              placeholder="app:edge"
              value={tagDraft}
            />
          </label>
          <button
            className="secondaryAction"
            disabled={tagPending || !tagDraft.trim()}
            type="submit"
          >
            <Plus size={15} />
            Add
          </button>
        </form>
      </div>
      <div className="fleetInlineTagChips">
        {agent.tags.length === 0 ? (
          <span className="mutedText">No tags assigned</span>
        ) : (
          agent.tags
            .slice()
            .sort()
            .map((tag) => (
              <button
                className="tagEditChip"
                disabled={tagPending}
                key={tag}
                onClick={() => void mutateTag("remove", tag)}
                title={`Remove ${tag}`}
                type="button"
              >
                <span>{tag}</span>
                <X size={13} />
              </button>
            ))
        )}
      </div>
      {(tagError || tagStatus) && (
        <small
          className={tagError ? "errorText panelErrorText" : "panelStatusText"}
        >
          {tagError ?? tagStatus}
        </small>
      )}
      <div
        className="detailTabs"
        role="tablist"
        aria-label="VPS detail sections"
      >
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
      <div className="signalGrid fleetSignalGrid">
        <Metric
          label="Traffic"
          value={formatSignalTraffic(latestRollup, latestNetworkRates)}
          tone="blue"
        />
        <Metric
          label="Samples"
          value={formatSignalSamples(latestRollup, latestNetworkRates)}
          tone="green"
        />
        <Metric
          label="RAM used"
          value={formatMemoryUsed(latestRollup)}
          tone="blue"
        />
        <Metric
          label="Disk free"
          value={formatDiskFree(latestRollup)}
          tone="green"
        />
      </div>
      <div className="detailPane fleetDetailPane" role="tabpanel">
        {activeDetailTab === "Overview" && (
          <>
            <DetailLine
              icon={<Server size={18} />}
              label="Name"
              value={agentLabel}
            />
            <DetailLine
              icon={<Server size={18} />}
              label="Status"
              value={agent.status}
            />
            <DetailLine
              icon={<Boxes size={18} />}
              label="Client ID"
              value={agent.id}
              mono
            />
            <DetailLine
              icon={<Clock3 size={18} />}
              label="Last seen"
              value={formatLastSeenDetail(agent.last_seen_at)}
            />
            <DetailLine
              icon={<Network size={18} />}
              label="Registration IP"
              value={agent.registration_ip ?? "unknown"}
              mono
            />
            <DetailLine
              icon={<Network size={18} />}
              label="Last IP"
              value={agent.last_ip ?? "unknown"}
              mono
            />
            <DetailLine
              icon={<Boxes size={18} />}
              label="Country"
              value={
                <CountryBadge country={country} showFlag={showCountryFlags} />
              }
            />
            <DetailLine
              icon={<Boxes size={18} />}
              label="Provider"
              value={provider || "unset"}
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="Privilege"
              value={formatPrivilege(agent.capabilities)}
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="Fleet position"
              value={`${summary.online} online / ${summary.total} total`}
            />
          </>
        )}
        {activeDetailTab === "Telemetry" && (
          <>
            <DetailLine
              icon={<Activity size={18} />}
              label="Stream"
              value={wsState}
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="Last event"
              value={summary.total === 0 ? "No samples" : lastLiveEvent}
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="CPU load"
              value={formatLoad(latestRollup?.cpu_load_1_avg)}
            />
            <DetailLine
              icon={<Server size={18} />}
              label="RAM used"
              value={formatMemoryUsed(latestRollup)}
            />
            <DetailLine
              icon={<Boxes size={18} />}
              label="Disk free"
              value={formatDiskFree(latestRollup)}
            />
            <DetailLine
              icon={<Network size={18} />}
              label="Network bytes"
              value={formatNetworkBytes(latestRollup)}
            />
            <DetailLine
              icon={<Network size={18} />}
              label="Network rate"
              value={formatNetworkRateSummary(latestNetworkRates, latestRollup)}
            />
            <DetailLine
              icon={<Activity size={18} />}
              label="Rollup samples"
              value={formatRollupSamples(latestRollup)}
            />
            <DetailLine
              icon={<Server size={18} />}
              label="Agent status"
              value={agent.status}
            />
          </>
        )}
        {activeDetailTab === "Jobs" && (
          <>
            <DetailLine
              icon={<Gauge size={18} />}
              label="Running jobs"
              value={String(summary.running_jobs)}
            />
            <DetailLine
              icon={<Server size={18} />}
              label="Target"
              value={agent.id}
              mono
            />
            <DetailLine
              icon={<Activity size={18} />}
              label="Privilege state"
              value={
                privilegeMaterial ? "Unlocked locally" : "Local unlock required"
              }
            />
          </>
        )}
        {activeDetailTab === "Network" && (
          <>
            <DetailLine
              icon={<Network size={18} />}
              label="Managed routing"
              value={isNetworkManaged ? "BGP/OSPF/tunnel tagged" : "Standard"}
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="Runtime control"
              value={formatTunnelCapability(agent.capabilities)}
            />
            <DetailLine
              icon={<Boxes size={18} />}
              label="Tags"
              value={displayOnlyTags.join(", ") || "untagged"}
            />
            <NetworkInterfacesPanel
              error={interfaceError}
              jobId={interfaceJobId}
              onOpenJobDetails={onOpenJobDetails}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onRefresh={() => void refreshInterfaces()}
              payloadHash={interfacePayloadHash}
              pending={interfacePending}
              progress={interfaceProgress}
              privilegeReady={Boolean(privilegeMaterial)}
              selectedAgent={agent}
              snapshot={interfaceSnapshot}
            />
            <TunnelList tunnels={latestTunnels} />
            <NetworkRateList rates={latestNetworkRates} rollup={latestRollup} />
            {agent.capabilities.unprivileged_hint && (
              <DetailLine
                icon={<Activity size={18} />}
                label="Privilege hint"
                value={agent.capabilities.unprivileged_hint}
              />
            )}
          </>
        )}
        {activeDetailTab === "Config" && (
          <>
            <DetailLine
              icon={<Gauge size={18} />}
              label="Privilege"
              value={formatPrivilege(agent.capabilities)}
            />
            <DetailLine
              icon={<FileCog size={18} />}
              label="Runtime tunnels"
              value={formatTunnelCapability(agent.capabilities)}
            />
            <DetailLine
              icon={<FileCog size={18} />}
              label="Process limits"
              value={yesNo(agent.capabilities.can_apply_process_limits)}
            />
            <DataSourceConfigList
              assignments={dataSourceAssignments}
              statuses={dataSourceStatus}
            />
            <ConfigPreviewBlock
              error={configError}
              onLoad={() => void loadRenderedConfig()}
              pending={configPending}
              preview={configPreview}
              summary={configPreviewSummary}
            />
          </>
        )}
      </div>
      <FleetNodeCharts
        networkRates={telemetryNetworkRates}
        rollups={telemetryRollups}
        title={agentLabel}
      />
    </div>
  );
}

function DataSourceConfigList({
  assignments,
  statuses,
}: {
  assignments: DataSourcePresetAssignmentRecord[];
  statuses: DataSourceStatusRecord[];
}) {
  const domains = Array.from(
    new Set(assignments.map((assignment) => assignment.domain)),
  ).sort();
  const statusRows = statuses
    .slice()
    .sort((left, right) =>
      `${left.domain}:${left.module}`.localeCompare(
        `${right.domain}:${right.module}`,
      ),
    );
  return (
    <div className="fleetConfigRows">
      <div className="detailLine">
        <FileCog size={18} />
        <div>
          <span>Assigned preset domains</span>
          <strong>
            {domains.length === 0
              ? "No explicit assignments"
              : domains.join(", ")}
          </strong>
        </div>
      </div>
      {assignments.slice(0, 8).map((assignment) => (
        <div
          className="detailLine compactConfigLine"
          key={`${assignment.client_id}-${assignment.domain}-${assignment.preset_id}`}
        >
          <Boxes size={18} />
          <div>
            <span>{assignment.domain}</span>
            <strong>
              {assignment.preset_name} · {assignment.preset_scope}
            </strong>
          </div>
        </div>
      ))}
      {assignments.length > 8 && (
        <small className="mutedText">
          +{assignments.length - 8} more preset assignment
          {assignments.length - 8 === 1 ? "" : "s"}
        </small>
      )}
      <div className="detailLine">
        <Activity size={18} />
        <div>
          <span>Runtime config sources</span>
          <strong>{formatDataSourceStatusSummary(statusRows)}</strong>
        </div>
      </div>
      {statusRows.slice(0, 8).map((row) => (
        <div
          className="detailLine compactConfigLine"
          key={`${row.client_id}-${row.domain}-${row.module}`}
        >
          <FileCog size={18} />
          <div>
            <span>
              {row.domain} / {row.module}
            </span>
            <strong>
              {row.status} · {row.preset_name} · {row.source_kind}
            </strong>
            <small>{row.status_reason || formatDataSourceEvidence(row)}</small>
          </div>
        </div>
      ))}
      {statusRows.length > 8 && (
        <small className="mutedText">
          +{statusRows.length - 8} more runtime config source
          {statusRows.length - 8 === 1 ? "" : "s"}
        </small>
      )}
    </div>
  );
}

function formatDataSourceStatusSummary(rows: DataSourceStatusRecord[]) {
  if (rows.length === 0) {
    return "No runtime config source status loaded";
  }
  const ok = rows.filter((row) => row.status.toLowerCase() === "ok").length;
  const degraded = rows.filter(
    (row) => row.status.toLowerCase() !== "ok",
  ).length;
  return `${rows.length} source${rows.length === 1 ? "" : "s"} · ${ok} ok${degraded > 0 ? ` · ${degraded} needs review` : ""}`;
}

function formatDataSourceEvidence(row: DataSourceStatusRecord) {
  const evidence = row.evidence;
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    return row.status_reason || "No evidence reported";
  }
  const parts: string[] = [];
  for (const key of [
    "sample_count",
    "artifact_count",
    "release_count",
    "backup_request_count",
    "restore_source_count",
  ]) {
    const value = evidence[key];
    if (typeof value === "number") {
      parts.push(`${key.replace(/_/g, " ")}: ${value}`);
    }
  }
  const objectStoreKind = evidence.server_object_store_kind;
  if (typeof objectStoreKind === "string" && objectStoreKind) {
    parts.push(`object store: ${objectStoreKind}`);
  }
  return parts.join(" · ") || row.status_reason || "Evidence available";
}

function ConfigPreviewBlock({
  error,
  onLoad,
  pending,
  preview,
  summary,
}: {
  error: string | null;
  onLoad: () => void;
  pending: boolean;
  preview: DataSourceHotConfigResponse | null;
  summary: string;
}) {
  return (
    <div className="timeline configPreviewBlock">
      <FileCog size={18} />
      <div>
        <strong>Rendered custom agent config</strong>
        <span>{summary}</span>
        <button
          className="secondaryAction compactAction"
          disabled={pending}
          onClick={onLoad}
          type="button"
        >
          <RefreshCw size={14} />
          Load config
        </button>
        {error ? <small className="errorText">{error}</small> : null}
        {preview ? (
          <pre className="configPreviewToml">{preview.toml}</pre>
        ) : null}
      </div>
    </div>
  );
}

function FleetNodeCharts({
  networkRates,
  rollups,
  title,
}: {
  networkRates: TelemetryNetworkRateRecord[];
  rollups: TelemetryRollupRecord[];
  title: string;
}) {
  const resourceChart = resourcePercentChartData(rollups);
  const cpuChart = cpuLoadChartData(rollups);
  const networkChart = networkRateChartData(networkRates);
  return (
    <div className="fleetNodeCharts">
      <div className="fleetNodeChartsHeader">
        <span>
          <BarChart3 size={16} /> Overview curves
        </span>
        <small>{title}</small>
      </div>
      <div className="fleetNodeChartGrid">
        <TimeSeriesChart
          ariaLabel={`${title} resource percentage curves`}
          emptyLabel="No resource rollup history"
          height={210}
          lines={resourceChart.lines}
          times={resourceChart.times}
          valueFormatter={formatChartPercent}
        />
        <TimeSeriesChart
          ariaLabel={`${title} CPU load curve`}
          emptyLabel="No CPU rollup history"
          height={210}
          lines={cpuChart.lines}
          times={cpuChart.times}
          valueFormatter={formatChartLoad}
        />
        <TimeSeriesChart
          ariaLabel={`${title} network rate curves`}
          emptyLabel="No network rate history"
          height={210}
          lines={networkChart.lines}
          times={networkChart.times}
          valueFormatter={formatChartBitsPerSecond}
        />
      </div>
    </div>
  );
}

function FleetSelectionPanel({
  agents,
  allTags,
  latestNetworkRates,
  latestRollups,
  mutateTagsForAgents,
  onOpenFileBrowser,
  onOpenSelectorWorkflow,
  selectionStatsMode,
  setSelectionStatsMode,
  vpsNameDisplayMode,
}: {
  agents: AgentView[];
  allTags: TagView[];
  latestNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestRollups: Map<string, TelemetryRollupRecord>;
  mutateTagsForAgents: (
    rows: AgentView[],
    action: "add" | "remove",
    tag: string,
  ) => Promise<TagMutationResponse>;
  onOpenFileBrowser: (rows: AgentView[]) => void;
  onOpenSelectorWorkflow: (
    rows: AgentView[],
    view: ActiveView,
    subpage: string,
    storageKey: string,
  ) => void;
  selectionStatsMode: FleetSelectionStatsMode;
  setSelectionStatsMode: (mode: FleetSelectionStatsMode) => void;
  vpsNameDisplayMode: VpsNameDisplayMode;
}) {
  const [tagToAdd, setTagToAdd] = useState("");
  const [tagToRemove, setTagToRemove] = useState("");
  const [pending, setPending] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const selectorExpression = selectorExpressionForClientIds(
    agents.map((agent) => agent.id),
  );
  const tagNames = useMemo(
    () => allTags.map((tag) => tag.name).sort(),
    [allTags],
  );
  async function submitTag(action: "add" | "remove", tag: string) {
    await runPanelAction(setPending, setError, async () => {
      const response = await mutateTagsForAgents(agents, action, tag);
      setStatus(
        `${response.action} ${response.tag}: ${response.changed_count} changed, ${response.skipped_count} skipped`,
      );
      if (action === "add") setTagToAdd("");
      else setTagToRemove("");
    });
  }
  return (
    <div className="fleetSelectionPanel">
      <div className="fleetSelectionHeader">
        <div>
          <strong>
            {agents.length} selected VPS{agents.length === 1 ? "" : "s"}
          </strong>
          <span className="monoValue">{selectorExpression}</span>
        </div>
        <div className="fleetSelectionActions">
          <button
            className="secondaryAction compactAction"
            onClick={() =>
              onOpenSelectorWorkflow(
                agents,
                "Jobs",
                "dispatch",
                JOB_SELECTOR_STORAGE_KEY,
              )
            }
            type="button"
          >
            <TerminalSquare size={14} /> Bulk execution
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={() =>
              onOpenSelectorWorkflow(
                agents,
                "Jobs",
                "multi_files",
                MULTI_FILE_SELECTOR_STORAGE_KEY,
              )
            }
            type="button"
          >
            <FolderOpen size={14} /> Multi-file
          </button>
          <button
            className="secondaryAction compactAction"
            disabled={agents.length !== 1}
            onClick={() => onOpenFileBrowser(agents)}
            type="button"
          >
            <FolderOpen size={14} /> File browser
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={() =>
              onOpenSelectorWorkflow(
                agents,
                "Config",
                "bulk",
                CONFIG_BULK_SELECTOR_STORAGE_KEY,
              )
            }
            type="button"
          >
            <FileCog size={14} /> Config bulk
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={() =>
              onOpenSelectorWorkflow(
                agents,
                "Tags",
                "bulk",
                TAG_BULK_SELECTOR_STORAGE_KEY,
              )
            }
            type="button"
          >
            <Tags size={14} /> Tags bulk
          </button>
        </div>
      </div>
      <div className="fleetInlineMutationPanel">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submitTag("add", tagToAdd);
          }}
        >
          <input
            aria-label="Tag to add to selected VPSs"
            list="fleet-selected-tag-options"
            onChange={(event) => setTagToAdd(event.target.value)}
            placeholder="provider:alpha or app:edge"
            value={tagToAdd}
          />
          <button
            className="secondaryAction compactAction"
            disabled={pending || !tagToAdd.trim()}
            type="submit"
          >
            Add tag
          </button>
        </form>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submitTag("remove", tagToRemove);
          }}
        >
          <input
            aria-label="Tag to remove from selected VPSs"
            list="fleet-selected-tag-options"
            onChange={(event) => setTagToRemove(event.target.value)}
            placeholder="tag to remove"
            value={tagToRemove}
          />
          <button
            className="secondaryAction compactAction dangerAction"
            disabled={pending || !tagToRemove.trim()}
            type="submit"
          >
            Remove tag
          </button>
        </form>
        <datalist id="fleet-selected-tag-options">
          {tagNames.map((tag) => (
            <option key={tag} value={tag} />
          ))}
        </datalist>
        {(error || status) && (
          <small className={error ? "errorText" : undefined}>
            {error ?? status}
          </small>
        )}
      </div>
      <div
        className="selectionStatsTabs"
        role="tablist"
        aria-label="Selected VPS statistical tables"
      >
        {selectionStatsModes.map((mode) => (
          <button
            aria-selected={selectionStatsMode === mode.id}
            className={selectionStatsMode === mode.id ? "selected" : ""}
            key={mode.id}
            onClick={() => setSelectionStatsMode(mode.id)}
            type="button"
          >
            {mode.label}
          </button>
        ))}
      </div>
      <FleetSelectionStatsTable
        agents={agents}
        latestNetworkRates={latestNetworkRates}
        latestRollups={latestRollups}
        mode={selectionStatsMode}
        vpsNameDisplayMode={vpsNameDisplayMode}
      />
    </div>
  );
}

function FleetSelectionStatsTable({
  agents,
  latestNetworkRates,
  latestRollups,
  mode,
  vpsNameDisplayMode,
}: {
  agents: AgentView[];
  latestNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestRollups: Map<string, TelemetryRollupRecord>;
  mode: FleetSelectionStatsMode;
  vpsNameDisplayMode: VpsNameDisplayMode;
}) {
  const rows = agents
    .slice()
    .sort((left, right) =>
      mode === "telemetry"
        ? (memoryUsedRatio(latestRollups.get(right.id)) ?? -1) -
          (memoryUsedRatio(latestRollups.get(left.id)) ?? -1)
        : mode === "network"
          ? networkRateTotal(latestNetworkRates.get(right.id) ?? []) -
            networkRateTotal(latestNetworkRates.get(left.id) ?? [])
          : formatVpsName(left, vpsNameDisplayMode).localeCompare(
              formatVpsName(right, vpsNameDisplayMode),
            ),
    );
  if (mode === "network")
    return (
      <div className="fleetSelectionStatsTable networkMode">
        <div className="fleetSelectionStatsRow heading">
          <span>VPS</span>
          <span>Total rate</span>
          <span>Interface rates</span>
          <span>Counters</span>
        </div>
        {rows.map((agent) => {
          const rates = latestNetworkRates.get(agent.id) ?? [];
          const rollup = latestRollups.get(agent.id) ?? null;
          return (
            <div className="fleetSelectionStatsRow" key={agent.id}>
              <span title={agent.id}>
                {formatVpsName(agent, vpsNameDisplayMode)}
              </span>
              <span>{formatNetworkRateSummary(rates, rollup)}</span>
              <span>
                {rates
                  .map(
                    (rate) =>
                      `${rate.interface}: ${formatBitsPerSecond(rate.rx_bps_avg + rate.tx_bps_avg)}`,
                  )
                  .join("; ") || "no rate rollup"}
              </span>
              <span>{formatNetworkBytes(rollup)}</span>
            </div>
          );
        })}
      </div>
    );
  if (mode === "overview")
    return (
      <div className="fleetSelectionStatsTable overviewMode">
        <div className="fleetSelectionStatsRow heading">
          <span>VPS</span>
          <span>Status</span>
          <span>Country</span>
          <span>Provider</span>
          <span>Last seen</span>
          <span>Tags</span>
        </div>
        {rows.map((agent) => (
          <div className="fleetSelectionStatsRow" key={agent.id}>
            <span title={agent.id}>
              {formatVpsName(agent, vpsNameDisplayMode)}
            </span>
            <span>{agent.status}</span>
            <span>{countryFromTags(agent.tags) ?? "unset"}</span>
            <span>{providerFromTags(agent.tags) ?? "unset"}</span>
            <span>{formatLastSeen(agent.last_seen_at)}</span>
            <span>{displayTags(agent.tags).join(", ") || "untagged"}</span>
          </div>
        ))}
      </div>
    );
  if (mode === "capabilities")
    return (
      <div className="fleetSelectionStatsTable capabilitiesMode">
        <div className="fleetSelectionStatsRow heading">
          <span>VPS</span>
          <span>Privilege</span>
          <span>UID</span>
          <span>Tunnels</span>
          <span>Process limits</span>
          <span>Build</span>
        </div>
        {rows.map((agent) => (
          <div className="fleetSelectionStatsRow" key={agent.id}>
            <span title={agent.id}>
              {formatVpsName(agent, vpsNameDisplayMode)}
            </span>
            <span>{formatPrivilege(agent.capabilities)}</span>
            <span>{agent.capabilities.effective_uid ?? "unknown"}</span>
            <span>{yesNo(agent.capabilities.can_manage_runtime_tunnels)}</span>
            <span>{yesNo(agent.capabilities.can_apply_process_limits)}</span>
            <span>
              {agent.internal_build_number
                ? `#${agent.internal_build_number}`
                : "unknown"}
            </span>
          </div>
        ))}
      </div>
    );
  return (
    <div className="fleetSelectionStatsTable telemetryMode">
      <div className="fleetSelectionStatsRow heading">
        <span>VPS</span>
        <span>CPU</span>
        <span>RAM used</span>
        <span>Disk free</span>
        <span>Network</span>
        <span>Samples</span>
      </div>
      {rows.map((agent) => {
        const rollup = latestRollups.get(agent.id) ?? null;
        const rates = latestNetworkRates.get(agent.id) ?? [];
        return (
          <div className="fleetSelectionStatsRow" key={agent.id}>
            <span title={agent.id}>
              {formatVpsName(agent, vpsNameDisplayMode)}
            </span>
            <span>{formatLoad(rollup?.cpu_load_1_avg)}</span>
            <span>{formatMemoryUsed(rollup)}</span>
            <span>{formatDiskFree(rollup)}</span>
            <span>{formatNetworkRateSummary(rates, rollup)}</span>
            <span>{formatRollupSamples(rollup)}</span>
          </div>
        );
      })}
    </div>
  );
}

function CountryBadge({
  country,
  showFlag,
}: {
  country: string | null;
  showFlag: boolean;
}) {
  if (!country) return <span className="countryBadge">unset</span>;
  const normalized = country.toUpperCase();
  return (
    <span className="countryBadge" title={normalized}>
      {showFlag && /^[A-Z]{2}$/.test(normalized) && (
        <img
          alt=""
          className="countryFlag"
          loading="lazy"
          referrerPolicy="no-referrer"
          src={`https://flagcdn.com/16x12/${normalized.toLowerCase()}.png`}
        />
      )}
      <span>{normalized}</span>
    </span>
  );
}

function writeLocalString(key: string, value: string) {
  if (typeof window === "undefined") return;
  try {
    if (value.trim()) window.localStorage.setItem(key, value);
    else window.localStorage.removeItem(key);
  } catch {
    /* local handoff only */
  }
}

function seedSingleFileBrowser(agent: AgentView) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      FILE_BROWSER_STATE_STORAGE_KEY,
      JSON.stringify({
        path: "/",
        showHidden: false,
        targetExpression: selectorExpressionForClientIds([agent.id]),
      }),
    );
  } catch {
    /* local handoff only */
  }
}

function resourcePercentChartData(rollups: TelemetryRollupRecord[]) {
  const sorted = sortRollups(rollups);
  return {
    times: sorted.map((rollup) => rollup.bucket_start),
    lines: [
      {
        color: CHART_COLORS[0],
        label: "RAM used",
        values: sorted.map((rollup) => memoryUsedRatio(rollup)),
      },
      {
        color: CHART_COLORS[1],
        label: "Disk free",
        values: sorted.map((rollup) => diskFreeRatio(rollup)),
      },
    ] satisfies TimeSeriesChartLine[],
  };
}
function cpuLoadChartData(rollups: TelemetryRollupRecord[]) {
  const sorted = sortRollups(rollups);
  return {
    times: sorted.map((rollup) => rollup.bucket_start),
    lines: [
      {
        color: CHART_COLORS[2],
        label: "CPU load",
        values: sorted.map((rollup) => rollup.cpu_load_1_avg),
      },
    ] satisfies TimeSeriesChartLine[],
  };
}
function networkRateChartData(rates: TelemetryNetworkRateRecord[]) {
  const times = sortedUniqueTimes(rates.map((rate) => rate.bucket_start));
  const rx = new Map<string, number>();
  const tx = new Map<string, number>();
  for (const rate of rates) {
    rx.set(
      rate.bucket_start,
      (rx.get(rate.bucket_start) ?? 0) + rate.rx_bps_avg,
    );
    tx.set(
      rate.bucket_start,
      (tx.get(rate.bucket_start) ?? 0) + rate.tx_bps_avg,
    );
  }
  return {
    times,
    lines: [
      {
        color: CHART_COLORS[0],
        label: "RX",
        values: times.map((time) => rx.get(time) ?? null),
      },
      {
        color: CHART_COLORS[3],
        label: "TX",
        values: times.map((time) => tx.get(time) ?? null),
      },
    ] satisfies TimeSeriesChartLine[],
  };
}
function sortRollups(rollups: TelemetryRollupRecord[]) {
  return rollups
    .slice()
    .sort((left, right) => left.bucket_start.localeCompare(right.bucket_start));
}
function sortedUniqueTimes(times: string[]) {
  return Array.from(new Set(times)).sort((left, right) =>
    left.localeCompare(right),
  );
}
function memoryUsedRatio(
  rollup: TelemetryRollupRecord | null | undefined,
): number | null {
  if (!rollup || rollup.memory_total_bytes_max <= 0) return null;
  return (
    ((rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg) /
      rollup.memory_total_bytes_max) *
    100
  );
}
function diskFreeRatio(
  rollup: TelemetryRollupRecord | null | undefined,
): number | null {
  if (!rollup || rollup.disk_total_bytes_max <= 0) return null;
  return (rollup.disk_available_bytes_avg / rollup.disk_total_bytes_max) * 100;
}
function networkRateTotal(rates: TelemetryNetworkRateRecord[]) {
  return rates.reduce(
    (total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg,
    0,
  );
}
function formatChartPercent(value: number | null) {
  return typeof value === "number" && Number.isFinite(value)
    ? `${Math.round(value)}%`
    : "-";
}
function formatChartLoad(value: number | null) {
  return typeof value === "number" && Number.isFinite(value)
    ? value.toFixed(2)
    : "-";
}
function formatChartBitsPerSecond(value: number | null) {
  return typeof value === "number" && Number.isFinite(value)
    ? formatBitsPerSecond(value)
    : "-";
}
function yesNo(value: boolean | null | undefined) {
  return value ? "yes" : "no";
}

function formatJsonInline(value: unknown): string {
  if (value === null || value === undefined) {
    return "-";
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    const rendered = JSON.stringify(value);
    if (!rendered) {
      return "-";
    }
    return rendered.length > 120 ? `${rendered.slice(0, 117)}...` : rendered;
  } catch {
    return String(value);
  }
}

function ConsoleField({
  children,
  className,
  hint,
  label,
}: {
  children: ReactNode;
  className?: string;
  hint?: ReactNode;
  label: ReactNode;
}) {
  return (
    <div className={className ? `consoleField ${className}` : "consoleField"}>
      <span>{label}</span>
      {children}
      {hint && <small>{hint}</small>}
    </div>
  );
}

function ConsoleFormGroup({
  actions,
  children,
  className,
  description,
  title,
}: {
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  description?: ReactNode;
  title: ReactNode;
}) {
  return (
    <section
      className={
        className ? `consoleFormGroup ${className}` : "consoleFormGroup"
      }
    >
      <div className="consoleFormGroupHeader">
        <strong>{title}</strong>
        {description && <span>{description}</span>}
      </div>
      <div className="consoleFormGrid">{children}</div>
      {actions && <div className="consoleFormActions">{actions}</div>}
    </section>
  );
}

function ConsoleDetailPanel({
  actions,
  children,
  description,
  onClose,
  title,
}: {
  actions?: ReactNode;
  children: ReactNode;
  description?: ReactNode;
  onClose?: () => void;
  title: ReactNode;
}) {
  return (
    <section className="consoleDetailPanel">
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>{title}</strong>
          {description && <small>{description}</small>}
        </span>
        {onClose && (
          <button
            aria-label="Close detail panel"
            className="iconButton"
            onClick={onClose}
            type="button"
          >
            <X size={16} />
          </button>
        )}
      </div>
      {children}
      {actions && <div className="consoleFormActions">{actions}</div>}
    </section>
  );
}

function ConsoleFreshnessBanner({ error }: { error: string | null }) {
  if (!error) {
    return null;
  }
  return (
    <div className="consoleFreshnessBanner">
      <span>Using cached data. Last refresh failed: {error}</span>
    </div>
  );
}

function TokenPreview({
  empty = "all",
  values,
}: {
  empty?: string;
  values: string[];
}) {
  if (values.length === 0) {
    return <small>{empty}</small>;
  }
  return (
    <span className="tokenPreview">
      {values.map((value) => (
        <span className="tokenChip" key={value} title={value}>
          {value}
        </span>
      ))}
    </span>
  );
}

function formatEditableNumber(value: number | null | undefined): string {
  return value == null ? "" : String(value);
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
    .map((part) => part.trim())
    .filter(Boolean);
}

function compactArray(values: string[]): string {
  return values.length > 0 ? values.join(", ") : "all";
}

function agentNamesById(
  agents: AgentView[],
  mode: VpsNameDisplayMode,
): Map<string, string> {
  return new Map(agents.map((agent) => [agent.id, formatVpsName(agent, mode)]));
}

function countryFromTags(tags: string[]): string | null {
  const countryTag = tags.find((tag) =>
    /^country[:=_-][a-z0-9_-]{2,32}$/i.test(tag),
  );
  if (!countryTag) {
    return null;
  }
  const [, code] = countryTag.split(/[:=_-]/, 2);
  return code ? code.toUpperCase() : null;
}

function providerFromTags(tags: string[]): string | null {
  const providerTag = tags.find((tag) =>
    /^provider[:=_-][a-z0-9_.-]{1,64}$/i.test(tag),
  );
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
  return normalized
    ? formatTime(normalized)
    : "never seen until first gateway report";
}

function normalizedLastSeenSort(value: string | null | undefined): string {
  return normalizeAgentTimestamp(value) ?? "";
}

function normalizeAgentTimestamp(
  value: string | null | undefined,
): string | null {
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

function actionTargetDescription(
  action: string,
  kind: string,
  name: string | undefined,
  detail?: string,
): string {
  const target = name ? `${kind} ${name}` : kind;
  return detail ? `${action} ${target}. ${detail}` : `${action} ${target}.`;
}

function selectedRecordSummary<T>(
  rows: T[] | null,
  singularLabel: string,
  pluralLabel: string,
  getName: (row: T) => string,
  getId: (row: T) => string,
) {
  const selectedRows = rows ?? [];
  if (selectedRows.length === 0) {
    return `0 ${pluralLabel}`;
  }
  const names = selectedRows.map(getName).join(", ");
  const ids = selectedRows.map(getId).join(", ");
  return (
    <span title={ids}>
      {selectedRows.length}{" "}
      {selectedRows.length === 1 ? singularLabel : pluralLabel}: {names}
    </span>
  );
}

function shortDeliveryError(error: string | null | undefined): string {
  const trimmed = error?.trim();
  if (!trimmed) {
    return "";
  }
  return trimmed.length > 96 ? `${trimmed.slice(0, 93)}...` : trimmed;
}

function thresholdSummary(policy: FleetAlertPolicyRecord): string {
  const parts = [
    policy.memory_available_warning_ratio == null
      ? null
      : `mem warn ${policy.memory_available_warning_ratio}`,
    policy.memory_available_critical_ratio == null
      ? null
      : `mem crit ${policy.memory_available_critical_ratio}`,
    policy.disk_available_warning_ratio == null
      ? null
      : `disk warn ${policy.disk_available_warning_ratio}`,
    policy.disk_available_critical_ratio == null
      ? null
      : `disk crit ${policy.disk_available_critical_ratio}`,
    policy.cpu_load_warning == null
      ? null
      : `cpu warn ${policy.cpu_load_warning}`,
    policy.cpu_load_critical == null
      ? null
      : `cpu crit ${policy.cpu_load_critical}`,
  ].filter((part): part is string => Boolean(part));
  return parts.length > 0 ? parts.join(" · ") : "no thresholds";
}

function scopeSummary(scopeKind: string, scopeValue?: string | null): string {
  return scopeValue ? `${scopeKind}:${scopeValue}` : scopeKind;
}

function tokenSummary(values: string[], empty: string): string {
  return values.length > 0 ? values.join(", ") : empty;
}

function PolicyDetailGrid({ policy }: { policy: FleetAlertPolicyRecord }) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>Policy</strong>
        <span>{policy.name}</span>
      </span>
      <span>
        <strong>ID</strong>
        <span className="monoValue">{policy.id}</span>
      </span>
      <span>
        <strong>Scope</strong>
        <span className="monoValue">
          {scopeSummary(policy.scope_kind, policy.scope_value)}
        </span>
      </span>
      <span>
        <strong>Priority</strong>
        <span>{policy.priority}</span>
      </span>
      <span>
        <strong>State</strong>
        <span>{policy.enabled ? "enabled" : "disabled"}</span>
      </span>
      <span>
        <strong>Thresholds</strong>
        <span>{thresholdSummary(policy)}</span>
      </span>
      <span>
        <strong>Created</strong>
        <span>{formatCompactTime(policy.created_at)}</span>
      </span>
      <span>
        <strong>Updated</strong>
        <span>{formatCompactTime(policy.updated_at)}</span>
      </span>
      <span>
        <strong>Notes</strong>
        <span>{policy.notes || "none"}</span>
      </span>
    </div>
  );
}

function ChannelDetailGrid({
  channel,
}: {
  channel: FleetAlertNotificationChannelRecord;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>Channel</strong>
        <span>{channel.name}</span>
      </span>
      <span>
        <strong>ID</strong>
        <span className="monoValue">{channel.id}</span>
      </span>
      <span>
        <strong>Scope</strong>
        <span className="monoValue">
          {scopeSummary(channel.scope_kind, channel.scope_value)}
        </span>
      </span>
      <span>
        <strong>Severity</strong>
        <span>{channel.min_severity}</span>
      </span>
      <span>
        <strong>State</strong>
        <span>{channel.enabled ? "enabled" : "disabled"}</span>
      </span>
      <span>
        <strong>Categories</strong>
        <span>{tokenSummary(channel.categories, "all categories")}</span>
      </span>
      <span>
        <strong>Operator states</strong>
        <span>{tokenSummary(channel.operator_states, "all states")}</span>
      </span>
      <span>
        <strong>Delivery</strong>
        <span>
          {channel.delivery_kind}: {channel.target}
        </span>
      </span>
      <span>
        <strong>Cooldown</strong>
        <span>{channel.cooldown_secs}s</span>
      </span>
      <span>
        <strong>Updated</strong>
        <span>{formatCompactTime(channel.updated_at)}</span>
      </span>
      <span>
        <strong>Notes</strong>
        <span>{channel.notes || "none"}</span>
      </span>
    </div>
  );
}

function WebhookRuleDetailGrid({ rule }: { rule: WebhookRuleRecord }) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>Rule</strong>
        <span>{rule.name}</span>
      </span>
      <span>
        <strong>ID</strong>
        <span className="monoValue">{rule.id}</span>
      </span>
      <span>
        <strong>State</strong>
        <span>{rule.enabled ? "enabled" : "disabled"}</span>
      </span>
      <span>
        <strong>Expression</strong>
        <span className="monoValue">{rule.expression}</span>
      </span>
      <span>
        <strong>Target</strong>
        <span>{rule.target}</span>
      </span>
      <span>
        <strong>Cooldown</strong>
        <span>{rule.cooldown_secs}s</span>
      </span>
      <span>
        <strong>Body template</strong>
        <span className="monoValue">{rule.body_template}</span>
      </span>
      <span>
        <strong>Updated</strong>
        <span>{formatCompactTime(rule.updated_at)}</span>
      </span>
      <span>
        <strong>Notes</strong>
        <span>{rule.notes || "none"}</span>
      </span>
    </div>
  );
}

function FleetAlertPolicyManager({
  agents,
  policies,
  onDelete,
  onUpsert,
}: {
  agents: AgentView[];
  policies: FleetAlertPolicyRecord[];
  onDelete: (policyId: string) => Promise<void>;
  onUpsert: (
    request: FleetAlertPolicyRequest,
  ) => Promise<FleetAlertPolicyRecord>;
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [detailPolicyId, setDetailPolicyId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<FleetAlertPolicyRecord[] | null>(
    null,
  );
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
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
  const [enabled, setEnabled] = useState(true);
  const [notes, setNotes] = useState("");
  const [status, setStatus] = useState<string | null>(null);

  const policyColumns = useMemo<
    ConsoleDataGridColumn<FleetAlertPolicyRecord>[]
  >(
    () => [
      {
        id: "name",
        header: "Policy",
        size: 260,
        minSize: 190,
        sortValue: (policy) => policy.name,
        searchValue: (policy) => `${policy.name} ${policy.notes ?? ""}`,
        cell: (policy) => (
          <span className="historyPrimary">
            <strong>{policy.name}</strong>
            <small>{policy.notes || "no notes"}</small>
          </span>
        ),
      },
      {
        id: "scope",
        header: "Scope",
        size: 170,
        minSize: 130,
        sortValue: (policy) =>
          `${policy.scope_kind}:${policy.scope_value ?? ""}`,
        searchValue: (policy) =>
          `${policy.scope_kind} ${policy.scope_value ?? ""}`,
        cell: (policy) => (
          <span className="monoValue">
            {policy.scope_kind}
            {policy.scope_value ? `:${policy.scope_value}` : ""}
          </span>
        ),
      },
      {
        id: "enabled",
        header: "State",
        size: 105,
        minSize: 90,
        sortValue: (policy) => policy.enabled,
        searchValue: (policy) => (policy.enabled ? "enabled" : "disabled"),
        cell: (policy) => (
          <ConsoleStatusBadge tone={policy.enabled ? "ok" : "warning"}>
            {policy.enabled ? "enabled" : "disabled"}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "priority",
        header: "Priority",
        size: 95,
        minSize: 80,
        sortValue: (policy) => policy.priority,
        cell: (policy) => <span className="monoValue">{policy.priority}</span>,
      },
      {
        id: "thresholds",
        header: "Thresholds",
        size: 360,
        minSize: 240,
        searchValue: thresholdSummary,
        cell: (policy) => (
          <span className="historyPrimary">
            <strong>{thresholdSummary(policy)}</strong>
            <small>Memory, disk, and CPU matrix.</small>
          </span>
        ),
      },
      {
        id: "updated",
        header: "Updated",
        size: 140,
        minSize: 110,
        sortValue: (policy) => policy.updated_at,
        cell: (policy) => formatCompactTime(policy.updated_at),
      },
    ],
    [],
  );

  function resetForm() {
    setEditingId(null);
    setName("edge-resource-policy");
    setScopeKind("tag");
    setScopeValue("edge");
    setMemoryWarning("0.20");
    setMemoryCritical("0.10");
    setDiskWarning("");
    setDiskCritical("");
    setCpuWarning("");
    setCpuCritical("");
    setPriority("0");
    setEnabled(true);
    setNotes("");
    setStatus(null);
  }

  function createPolicy() {
    resetForm();
    setDetailPolicyId(null);
    setEditorOpen(true);
  }

  function editPolicy(policy: FleetAlertPolicyRecord) {
    setDetailPolicyId(null);
    setEditingId(policy.id);
    setName(policy.name);
    setScopeKind(policy.scope_kind);
    setScopeValue(policy.scope_value ?? "");
    setMemoryWarning(
      formatEditableNumber(policy.memory_available_warning_ratio),
    );
    setMemoryCritical(
      formatEditableNumber(policy.memory_available_critical_ratio),
    );
    setDiskWarning(formatEditableNumber(policy.disk_available_warning_ratio));
    setDiskCritical(formatEditableNumber(policy.disk_available_critical_ratio));
    setCpuWarning(formatEditableNumber(policy.cpu_load_warning));
    setCpuCritical(formatEditableNumber(policy.cpu_load_critical));
    setPriority(String(policy.priority));
    setEnabled(policy.enabled);
    setNotes(policy.notes ?? "");
    setStatus(`editing ${policy.name}`);
    setEditorOpen(true);
  }

  function openPolicyDetails(policy: FleetAlertPolicyRecord) {
    setEditorOpen(false);
    setDetailPolicyId(policy.id);
    setStatus(`viewing ${policy.name}`);
  }

  function requestFromPolicy(
    policy: FleetAlertPolicyRecord,
    overrides: Partial<FleetAlertPolicyRequest> = {},
  ): FleetAlertPolicyRequest {
    return {
      id: policy.id,
      name: policy.name,
      scope_kind: policy.scope_kind,
      scope_value: policy.scope_value,
      memory_available_warning_ratio: policy.memory_available_warning_ratio,
      memory_available_critical_ratio: policy.memory_available_critical_ratio,
      disk_available_warning_ratio: policy.disk_available_warning_ratio,
      disk_available_critical_ratio: policy.disk_available_critical_ratio,
      cpu_load_warning: policy.cpu_load_warning,
      cpu_load_critical: policy.cpu_load_critical,
      priority: policy.priority,
      enabled: policy.enabled,
      notes: policy.notes,
      confirmed: true,
      ...overrides,
    };
  }

  async function submit() {
    setStatus(editingId ? "updating policy" : "creating policy");
    try {
      const policy = await onUpsert({
        id: editingId ?? undefined,
        name: name.trim(),
        scope_kind: scopeKind,
        scope_value: scopeKind === "global" ? null : scopeValue.trim(),
        memory_available_warning_ratio: optionalNumber(memoryWarning),
        memory_available_critical_ratio: optionalNumber(memoryCritical),
        disk_available_warning_ratio: optionalNumber(diskWarning),
        disk_available_critical_ratio: optionalNumber(diskCritical),
        cpu_load_warning: optionalNumber(cpuWarning),
        cpu_load_critical: optionalNumber(cpuCritical),
        priority: optionalInteger(priority) ?? 0,
        enabled,
        notes: notes.trim() || null,
        confirmed: true,
      });
      setEditingId(policy.id);
      setEditorOpen(true);
      setStatus(`saved ${policy.name}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "policy save failed");
    }
  }

  function requestDeletePolicies(rows: FleetAlertPolicyRecord[]) {
    setDeleteError(null);
    setDeleteRows(rows);
  }

  async function confirmDeletePolicies() {
    const rows = deleteRows ?? [];
    if (rows.length === 0 || deletePending) return;
    setDeletePending(true);
    setDeleteError(null);
    setStatus("deleting policies");
    try {
      for (const policy of rows) {
        await onDelete(policy.id);
      }
      if (rows.some((policy) => policy.id === editingId)) {
        resetForm();
        setEditorOpen(false);
      }
      if (rows.some((policy) => policy.id === detailPolicyId)) {
        setDetailPolicyId(null);
      }
      setDeleteRows(null);
      setStatus(`deleted ${rows.length}`);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "policy delete failed";
      setDeleteError(message);
      setStatus(message);
    } finally {
      setDeletePending(false);
    }
  }

  async function setPoliciesEnabled(
    rows: FleetAlertPolicyRecord[],
    nextEnabled: boolean,
  ) {
    if (rows.length === 0) return;
    setStatus(nextEnabled ? "enabling policies" : "disabling policies");
    try {
      for (const policy of rows) {
        await onUpsert(requestFromPolicy(policy, { enabled: nextEnabled }));
      }
      setStatus(`${nextEnabled ? "enabled" : "disabled"} ${rows.length}`);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "policy update failed",
      );
    }
  }

  const policyActions: ConsoleDataGridAction<FleetAlertPolicyRecord>[] = [
    {
      label: "Details",
      description: (rows) =>
        actionTargetDescription(
          "Open details for",
          "alert policy",
          rows[0]?.name,
          "Opens read-only policy details below the table.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Eye size={14} />,
      onSelect: (rows) => rows[0] && openPolicyDetails(rows[0]),
    },
    {
      label: "Edit",
      description: (rows) =>
        actionTargetDescription(
          "Edit",
          "alert policy",
          rows[0]?.name,
          "Opens the policy editor below the table.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Pencil size={14} />,
      onSelect: (rows) => rows[0] && editPolicy(rows[0]),
    },
    {
      label: "Enable",
      description: (rows) =>
        `Enable ${rows.filter((policy) => !policy.enabled).length} disabled selected alert policy records.`,
      disabled: (rows) => rows.filter((policy) => !policy.enabled).length === 0,
      icon: <Power size={14} />,
      onSelect: (rows) =>
        void setPoliciesEnabled(
          rows.filter((policy) => !policy.enabled),
          true,
        ),
    },
    {
      label: "Disable",
      description: (rows) =>
        `Disable ${rows.filter((policy) => policy.enabled).length} enabled selected alert policy records.`,
      disabled: (rows) => rows.filter((policy) => policy.enabled).length === 0,
      icon: <PowerOff size={14} />,
      onSelect: (rows) =>
        void setPoliciesEnabled(
          rows.filter((policy) => policy.enabled),
          false,
        ),
    },
    {
      label: "Review deletion",
      description: (rows) =>
        `Delete ${rows.length} selected alert policy records. Existing alert states are not changed.`,
      disabled: (rows) => rows.length === 0,
      icon: <Trash2 size={14} />,
      onSelect: requestDeletePolicies,
      tone: "danger",
    },
  ];

  return (
    <div className="consoleCrudPanel">
      <div className="consoleResourceLayout fullWidth">
        <ConsoleDataGrid
          actions={policyActions}
          columns={policyColumns}
          defaultPageSize={10}
          empty="No alert policies saved."
          getRowId={(policy) => policy.id}
          itemLabel="policies"
          renderExpandedRow={(policy) => <PolicyDetailGrid policy={policy} />}
          rowActions={policyActions}
          rows={policies}
          searchPlaceholder="Search policies by name, scope, thresholds, or notes"
          storageKey="vpsman.grid.fleet.alertPolicies.v2"
          title="Alert policy rules"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              onClick={createPolicy}
              type="button"
            >
              <Plus size={16} />
              <span>Create policy</span>
            </button>
          }
        />
        {detailPolicyId && !editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <button
                className="secondaryAction"
                type="button"
                onClick={() => {
                  const policy = policies.find(
                    (candidate) => candidate.id === detailPolicyId,
                  );
                  if (policy) {
                    editPolicy(policy);
                  }
                }}
              >
                Edit policy
              </button>
            }
            description="Policy metadata and thresholds."
            onClose={() => setDetailPolicyId(null)}
            title="Alert policy details"
          >
            {(() => {
              const policy = policies.find(
                (candidate) => candidate.id === detailPolicyId,
              );
              return policy ? (
                <PolicyDetailGrid policy={policy} />
              ) : (
                <span className="mutedText">Policy no longer exists.</span>
              );
            })()}
          </ConsoleDetailPanel>
        ) : null}
        {editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <>
                <button
                  className="primaryAction"
                  type="button"
                  onClick={() => void submit()}
                >
                  {editingId ? "Update policy" : "Create policy"}
                </button>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={createPolicy}
                >
                  New policy
                </button>
              </>
            }
            description="Scoped thresholds are saved records. Table context stays visible while editing."
            onClose={() => setEditorOpen(false)}
            title={editingId ? "Edit alert policy" : "Create alert policy"}
          >
            <div className="consoleFormGrid">
              <ConsoleField label="Policy name" className="fieldWide">
                <input
                  aria-label="Policy name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Scope kind">
                <select
                  aria-label="Policy scope kind"
                  value={scopeKind}
                  onChange={(event) => setScopeKind(event.target.value)}
                >
                  <option value="global">global</option>
                  <option value="provider">provider</option>
                  <option value="tag">tag</option>
                  <option value="client">client</option>
                </select>
              </ConsoleField>
              <ConsoleField
                label="Scope value"
                hint={
                  scopeKind === "global"
                    ? "Global policies do not need a value."
                    : scopeKind === "client"
                      ? "Exact VPS ID is saved; type to search names or IDs."
                      : "Tag or provider value."
                }
              >
                {scopeKind === "client" ? (
                  <VpsCombobox
                    agents={agents}
                    ariaLabel="Policy scope value"
                    onChange={setScopeValue}
                    placeholder="Search policy VPS"
                    value={scopeValue}
                  />
                ) : (
                  <input
                    aria-label="Policy scope value"
                    disabled={scopeKind === "global"}
                    value={scopeValue}
                    onChange={(event) => setScopeValue(event.target.value)}
                  />
                )}
              </ConsoleField>
              <ConsoleField label="Priority">
                <input
                  aria-label="Policy priority"
                  value={priority}
                  onChange={(event) => setPriority(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="State">
                <label className="checkLine inlineCheck">
                  <input
                    checked={enabled}
                    onChange={(event) => setEnabled(event.target.checked)}
                    type="checkbox"
                  />
                  <span>Evaluate policy</span>
                </label>
              </ConsoleField>
              <div className="thresholdMatrix">
                <div className="thresholdMatrixRow header">
                  <span>Resource</span>
                  <span>Warning</span>
                  <span>Critical</span>
                </div>
                <div className="thresholdMatrixRow">
                  <strong>Memory available ratio</strong>
                  <input
                    aria-label="Memory warning ratio"
                    value={memoryWarning}
                    onChange={(event) => setMemoryWarning(event.target.value)}
                    placeholder="0.20"
                  />
                  <input
                    aria-label="Memory critical ratio"
                    value={memoryCritical}
                    onChange={(event) => setMemoryCritical(event.target.value)}
                    placeholder="0.10"
                  />
                </div>
                <div className="thresholdMatrixRow">
                  <strong>Disk available ratio</strong>
                  <input
                    aria-label="Disk warning ratio"
                    value={diskWarning}
                    onChange={(event) => setDiskWarning(event.target.value)}
                    placeholder="0.15"
                  />
                  <input
                    aria-label="Disk critical ratio"
                    value={diskCritical}
                    onChange={(event) => setDiskCritical(event.target.value)}
                    placeholder="0.08"
                  />
                </div>
                <div className="thresholdMatrixRow">
                  <strong>CPU load</strong>
                  <input
                    aria-label="CPU warning load"
                    value={cpuWarning}
                    onChange={(event) => setCpuWarning(event.target.value)}
                    placeholder="4.0"
                  />
                  <input
                    aria-label="CPU critical load"
                    value={cpuCritical}
                    onChange={(event) => setCpuCritical(event.target.value)}
                    placeholder="8.0"
                  />
                </div>
              </div>
              <ConsoleField label="Notes" className="fieldFull">
                <textarea
                  aria-label="Policy notes"
                  value={notes}
                  onChange={(event) => setNotes(event.target.value)}
                />
              </ConsoleField>
            </div>
          </ConsoleDetailPanel>
        ) : null}
      </div>
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <ConfirmationPrompt
        confirmLabel="Delete"
        detail="Deletes selected alert policy records. Existing alert states are not changed."
        items={[
          {
            label: "Policies",
            value: selectedRecordSummary(
              deleteRows,
              "policy",
              "policies",
              (row) => row.name,
              (row) => row.id,
            ),
          },
        ]}
        error={deleteError}
        onCancel={() => {
          setDeleteError(null);
          setDeleteRows(null);
        }}
        onConfirm={() => void confirmDeletePolicies()}
        open={deleteRows !== null}
        pending={deletePending}
        title="Delete alert policies"
        tone="danger"
      />
    </div>
  );
}

type NotificationRegistryTab =
  | "channels"
  | "webhooks"
  | "deliveries"
  | "maintenance";

function FleetNotificationsHub({
  agents,
  alertChannels,
  alertDeliveries,
  webhookDeliveries,
  webhookRules,
  onDeleteAlertChannel,
  onDeleteWebhookRule,
  onDispatchAlertNotifications,
  onDispatchWebhookRules,
  onDryRunWebhookRule,
  onProcessAlertNotifications,
  onProcessWebhookRuleDeliveries,
  onRotateWebhookDeliveryHistory,
  onUpsertAlertChannel,
  onUpsertWebhookRule,
}: {
  agents: AgentView[];
  alertChannels: FleetAlertNotificationChannelRecord[];
  alertDeliveries: FleetAlertNotificationDeliveryRecord[];
  webhookDeliveries: WebhookRuleDeliveryRecord[];
  webhookRules: WebhookRuleRecord[];
  onDeleteAlertChannel: (channelId: string) => Promise<void>;
  onDeleteWebhookRule: (ruleId: string) => Promise<void>;
  onDispatchAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onDispatchWebhookRules: (
    request: WebhookRuleDispatchRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onDryRunWebhookRule: (
    request: WebhookRuleDryRunRequest,
  ) => Promise<WebhookRuleDryRunRecord>;
  onProcessAlertNotifications: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onProcessWebhookRuleDeliveries: (
    request: WebhookRuleProcessRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onRotateWebhookDeliveryHistory: (
    request: WebhookDeliveryRotationRequest,
  ) => Promise<WebhookDeliveryRotationResponse>;
  onUpsertAlertChannel: (
    request: FleetAlertNotificationChannelRequest,
  ) => Promise<FleetAlertNotificationChannelRecord>;
  onUpsertWebhookRule: (
    request: WebhookRuleRequest,
  ) => Promise<WebhookRuleRecord>;
}) {
  const [tab, setTab] = useState<NotificationRegistryTab>("channels");
  const [alertPreviewRows, setAlertPreviewRows] = useState<
    FleetAlertNotificationDeliveryRecord[]
  >([]);
  const [webhookPreviewRows, setWebhookPreviewRows] = useState<
    WebhookRuleDeliveryRecord[]
  >([]);
  const [dryRunPreview, setDryRunPreview] =
    useState<WebhookRuleDryRunRecord | null>(null);

  function openDeliveries() {
    setTab("deliveries");
  }

  function clearAlertPreview() {
    setAlertPreviewRows([]);
  }

  function clearWebhookPreview() {
    setDryRunPreview(null);
    setWebhookPreviewRows([]);
  }

  return (
    <div className="consoleCrudPanel">
      <div
        className="consoleRegistryTabs"
        role="tablist"
        aria-label="Notification registries"
      >
        {[
          ["channels", "Channels"],
          ["webhooks", "Webhooks"],
          ["deliveries", "Deliveries"],
          ["maintenance", "Maintenance"],
        ].map(([id, label]) => (
          <button
            aria-selected={tab === id}
            className={tab === id ? "active" : undefined}
            key={id}
            onClick={() => setTab(id as NotificationRegistryTab)}
            role="tab"
            type="button"
          >
            {label}
          </button>
        ))}
      </div>
      {tab === "channels" && (
        <FleetAlertNotificationManager
          agents={agents}
          channels={alertChannels}
          onDelete={onDeleteAlertChannel}
          onDispatch={onDispatchAlertNotifications}
          onOpenDeliveries={openDeliveries}
          onPreviewRows={setAlertPreviewRows}
          onProcess={onProcessAlertNotifications}
          onUpsert={onUpsertAlertChannel}
        />
      )}
      {tab === "webhooks" && (
        <WebhookRuleManager
          agents={agents}
          onDelete={onDeleteWebhookRule}
          onDispatch={onDispatchWebhookRules}
          onDryRun={onDryRunWebhookRule}
          onOpenDeliveries={openDeliveries}
          onPreviewDryRun={setDryRunPreview}
          onPreviewRows={setWebhookPreviewRows}
          onProcess={onProcessWebhookRuleDeliveries}
          onUpsert={onUpsertWebhookRule}
          rules={webhookRules}
        />
      )}
      {tab === "deliveries" && (
        <div className="consoleCrudPanel">
          {alertPreviewRows.length > 0 && (
            <DeliveryPreviewSection
              count={alertPreviewRows.length}
              onClear={clearAlertPreview}
              title="Notification delivery preview"
            >
              <NotificationDeliveryHistoryGrid
                deliveries={alertPreviewRows}
                preview
              />
            </DeliveryPreviewSection>
          )}
          {(dryRunPreview || webhookPreviewRows.length > 0) && (
            <DeliveryPreviewSection
              count={webhookPreviewRows.length}
              onClear={clearWebhookPreview}
              title="Webhook delivery preview"
            >
              {dryRunPreview && (
                <WebhookDryRunNotice agents={agents} preview={dryRunPreview} />
              )}
              <WebhookDeliveryHistoryGrid
                deliveries={webhookPreviewRows}
                preview
              />
            </DeliveryPreviewSection>
          )}
          <NotificationDeliveryHistoryGrid
            deliveries={alertDeliveries}
            preview={false}
          />
          <WebhookDeliveryHistoryGrid
            deliveries={webhookDeliveries}
            preview={false}
          />
        </div>
      )}
      {tab === "maintenance" && (
        <WebhookDeliveryMaintenancePanel
          onRotate={onRotateWebhookDeliveryHistory}
          rules={webhookRules}
        />
      )}
    </div>
  );
}

function DeliveryPreviewSection({
  children,
  count,
  onClear,
  title,
}: {
  children: ReactNode;
  count: number;
  onClear: () => void;
  title: string;
}) {
  return (
    <section className="deliveryPreviewSection">
      <div className="deliveryPreviewHeader">
        <span>
          <strong>{title}</strong>
          <small>
            {count} preview {count === 1 ? "row" : "rows"}. Retained history is
            still shown below.
          </small>
        </span>
        <button
          className="secondaryAction compactAction"
          onClick={onClear}
          type="button"
        >
          Clear preview
        </button>
      </div>
      {children}
    </section>
  );
}

function FleetAlertNotificationManager({
  agents,
  channels,
  onDelete,
  onDispatch,
  onOpenDeliveries,
  onPreviewRows,
  onProcess,
  onUpsert,
}: {
  agents: AgentView[];
  channels: FleetAlertNotificationChannelRecord[];
  onDelete: (channelId: string) => Promise<void>;
  onDispatch: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onOpenDeliveries: () => void;
  onPreviewRows: (rows: FleetAlertNotificationDeliveryRecord[]) => void;
  onProcess: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onUpsert: (
    request: FleetAlertNotificationChannelRequest,
  ) => Promise<FleetAlertNotificationChannelRecord>;
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [detailChannelId, setDetailChannelId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<
    FleetAlertNotificationChannelRecord[] | null
  >(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [name, setName] = useState("critical-audit-channel");
  const [scopeKind, setScopeKind] = useState("global");
  const [scopeValue, setScopeValue] = useState("");
  const [minSeverity, setMinSeverity] = useState("critical");
  const [categories, setCategories] = useState("");
  const [operatorStates, setOperatorStates] = useState("");
  const [deliveryKind, setDeliveryKind] = useState("audit_log");
  const [target, setTarget] = useState("fleet-alerts");
  const [cooldownSecs, setCooldownSecs] = useState("300");
  const [enabled, setEnabled] = useState(true);
  const [notes, setNotes] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [queueConfirmation, setQueueConfirmation] = useState<
    "dispatch" | "process" | null
  >(null);
  const [queuePending, setQueuePending] = useState(false);

  const categoryTokens = useMemo(() => csvValues(categories), [categories]);
  const operatorStateTokens = useMemo(
    () => csvValues(operatorStates),
    [operatorStates],
  );

  const channelColumns = useMemo<
    ConsoleDataGridColumn<FleetAlertNotificationChannelRecord>[]
  >(
    () => [
      {
        id: "name",
        header: "Channel",
        size: 250,
        minSize: 180,
        sortValue: (channel) => channel.name,
        searchValue: (channel) => `${channel.name} ${channel.notes ?? ""}`,
        cell: (channel) => (
          <span className="historyPrimary">
            <strong>{channel.name}</strong>
            <small>{channel.notes || "no notes"}</small>
          </span>
        ),
      },
      {
        id: "scope",
        header: "Scope",
        size: 170,
        minSize: 130,
        sortValue: (channel) =>
          `${channel.scope_kind}:${channel.scope_value ?? ""}`,
        searchValue: (channel) =>
          `${channel.scope_kind} ${channel.scope_value ?? ""}`,
        cell: (channel) => (
          <span className="monoValue">
            {channel.scope_kind}
            {channel.scope_value ? `:${channel.scope_value}` : ""}
          </span>
        ),
      },
      {
        id: "severity",
        header: "Severity",
        size: 110,
        minSize: 90,
        sortValue: (channel) => channel.min_severity,
        searchValue: (channel) => channel.min_severity,
        cell: (channel) => (
          <ConsoleStatusBadge tone={alertTone(channel.min_severity)}>
            {channel.min_severity}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "filters",
        header: "Filters",
        size: 260,
        minSize: 190,
        searchValue: (channel) =>
          `${channel.categories.join(" ")} ${channel.operator_states.join(" ")}`,
        cell: (channel) => (
          <span className="historyPrimary">
            <strong>
              {channel.categories.length > 0
                ? channel.categories.join(", ")
                : "all categories"}
            </strong>
            <small>
              {channel.operator_states.length > 0
                ? channel.operator_states.join(", ")
                : "all states"}
            </small>
          </span>
        ),
      },
      {
        id: "delivery",
        header: "Delivery",
        size: 240,
        minSize: 170,
        sortValue: (channel) => `${channel.delivery_kind}:${channel.target}`,
        searchValue: (channel) => `${channel.delivery_kind} ${channel.target}`,
        cell: (channel) => (
          <span className="historyPrimary">
            <strong>{channel.delivery_kind}</strong>
            <small>{channel.target}</small>
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        size: 100,
        minSize: 90,
        sortValue: (channel) => channel.enabled,
        searchValue: (channel) => (channel.enabled ? "enabled" : "disabled"),
        cell: (channel) => (
          <ConsoleStatusBadge tone={channel.enabled ? "ok" : "warning"}>
            {channel.enabled ? "enabled" : "disabled"}
          </ConsoleStatusBadge>
        ),
      },
    ],
    [],
  );

  function resetForm() {
    setEditingId(null);
    setName("critical-audit-channel");
    setScopeKind("global");
    setScopeValue("");
    setMinSeverity("critical");
    setCategories("");
    setOperatorStates("");
    setDeliveryKind("audit_log");
    setTarget("fleet-alerts");
    setCooldownSecs("300");
    setEnabled(true);
    setNotes("");
    setStatus(null);
  }

  function createChannel() {
    resetForm();
    setDetailChannelId(null);
    setEditorOpen(true);
  }

  function editChannel(channel: FleetAlertNotificationChannelRecord) {
    setDetailChannelId(null);
    setEditingId(channel.id);
    setName(channel.name);
    setScopeKind(channel.scope_kind);
    setScopeValue(channel.scope_value ?? "");
    setMinSeverity(channel.min_severity);
    setCategories(channel.categories.join(", "));
    setOperatorStates(channel.operator_states.join(", "));
    setDeliveryKind(channel.delivery_kind);
    setTarget(channel.target);
    setCooldownSecs(String(channel.cooldown_secs));
    setEnabled(channel.enabled);
    setNotes(channel.notes ?? "");
    setStatus(`editing ${channel.name}`);
    setEditorOpen(true);
  }

  function openChannelDetails(channel: FleetAlertNotificationChannelRecord) {
    setEditorOpen(false);
    setDetailChannelId(channel.id);
    setStatus(`viewing ${channel.name}`);
  }

  function requestFromChannel(
    channel: FleetAlertNotificationChannelRecord,
    overrides: Partial<FleetAlertNotificationChannelRequest> = {},
  ): FleetAlertNotificationChannelRequest {
    return {
      id: channel.id,
      name: channel.name,
      scope_kind: channel.scope_kind,
      scope_value: channel.scope_value,
      min_severity: channel.min_severity,
      categories: channel.categories,
      operator_states: channel.operator_states,
      delivery_kind: channel.delivery_kind,
      target: channel.target,
      cooldown_secs: channel.cooldown_secs,
      enabled: channel.enabled,
      notes: channel.notes,
      confirmed: true,
      ...overrides,
    };
  }

  async function submit() {
    setStatus(editingId ? "updating channel" : "creating channel");
    try {
      const channel = await onUpsert({
        id: editingId ?? undefined,
        name: name.trim(),
        scope_kind: scopeKind,
        scope_value: scopeKind === "global" ? null : scopeValue.trim(),
        min_severity: minSeverity,
        categories: categoryTokens,
        operator_states: operatorStateTokens,
        delivery_kind: deliveryKind.trim(),
        target: target.trim(),
        cooldown_secs: optionalInteger(cooldownSecs),
        enabled,
        notes: notes.trim() || null,
        confirmed: true,
      });
      setEditingId(channel.id);
      setEditorOpen(true);
      setStatus(`saved ${channel.name}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "channel save failed");
    }
  }

  function requestDeleteChannels(
    rows: FleetAlertNotificationChannelRecord[],
  ) {
    setDeleteError(null);
    setDeleteRows(rows);
  }

  async function confirmDeleteChannels() {
    const rows = deleteRows ?? [];
    if (rows.length === 0 || deletePending) return;
    setDeletePending(true);
    setDeleteError(null);
    setStatus("deleting channels");
    try {
      for (const channel of rows) {
        await onDelete(channel.id);
      }
      if (rows.some((channel) => channel.id === editingId)) {
        resetForm();
        setEditorOpen(false);
      }
      if (rows.some((channel) => channel.id === detailChannelId)) {
        setDetailChannelId(null);
      }
      setDeleteRows(null);
      setStatus(`deleted ${rows.length}`);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "channel delete failed";
      setDeleteError(message);
      setStatus(message);
    } finally {
      setDeletePending(false);
    }
  }

  async function setChannelsEnabled(
    rows: FleetAlertNotificationChannelRecord[],
    nextEnabled: boolean,
  ) {
    if (rows.length === 0) return;
    setStatus(nextEnabled ? "enabling channels" : "disabling channels");
    try {
      for (const channel of rows) {
        await onUpsert(requestFromChannel(channel, { enabled: nextEnabled }));
      }
      setStatus(`${nextEnabled ? "enabled" : "disabled"} ${rows.length}`);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "channel update failed",
      );
    }
  }

  async function dispatch(dryRun: boolean) {
    setStatus(dryRun ? "matching alerts" : "queueing alert notifications");
    if (!dryRun) {
      setQueuePending(true);
    }
    try {
      const rows = await onDispatch({
        limit: 50,
        dry_run: dryRun,
        confirmed: !dryRun,
      });
      if (dryRun) {
        onPreviewRows(rows);
        onOpenDeliveries();
      }
      setStatus(`${dryRun ? "matched" : "queued"} ${rows.length}`);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "notification dispatch failed",
      );
    } finally {
      if (!dryRun) {
        setQueuePending(false);
      }
    }
  }

  async function process(dryRun: boolean) {
    setStatus(
      dryRun ? "previewing notification queue" : "delivering notifications",
    );
    if (!dryRun) {
      setQueuePending(true);
    }
    try {
      const rows = await onProcess({
        limit: 50,
        status: "queued",
        dry_run: dryRun,
        confirmed: !dryRun,
      });
      if (dryRun) {
        onPreviewRows(rows);
        onOpenDeliveries();
      }
      setStatus(`${dryRun ? "previewed" : "processed"} ${rows.length}`);
    } catch (error) {
      setStatus(
        error instanceof Error
          ? error.message
          : "notification processing failed",
      );
    } finally {
      if (!dryRun) {
        setQueuePending(false);
      }
    }
  }

  async function confirmQueueAction() {
    const action = queueConfirmation;
    if (!action || queuePending) {
      return;
    }
    if (action === "dispatch") {
      await dispatch(false);
    } else {
      await process(false);
    }
    setQueueConfirmation(null);
  }

  const channelActions: ConsoleDataGridAction<FleetAlertNotificationChannelRecord>[] =
    [
      {
        label: "Details",
        description: (rows) =>
          actionTargetDescription(
            "Open details for",
            "notification channel",
            rows[0]?.name,
            "Opens read-only channel details below the table.",
          ),
        disabled: (rows) => rows.length !== 1,
        icon: <Eye size={14} />,
        onSelect: (rows) => rows[0] && openChannelDetails(rows[0]),
      },
      {
        label: "Edit",
        description: (rows) =>
          actionTargetDescription(
            "Edit",
            "notification channel",
            rows[0]?.name,
            "Opens the channel editor below the table.",
          ),
        disabled: (rows) => rows.length !== 1,
        icon: <Pencil size={14} />,
        onSelect: (rows) => rows[0] && editChannel(rows[0]),
      },
      {
        label: "Enable",
        description: (rows) =>
          `Enable ${rows.filter((channel) => !channel.enabled).length} disabled selected notification channel records.`,
        disabled: (rows) =>
          rows.filter((channel) => !channel.enabled).length === 0,
        icon: <Power size={14} />,
        onSelect: (rows) =>
          void setChannelsEnabled(
            rows.filter((channel) => !channel.enabled),
            true,
          ),
      },
      {
        label: "Disable",
        description: (rows) =>
          `Disable ${rows.filter((channel) => channel.enabled).length} enabled selected notification channel records.`,
        disabled: (rows) =>
          rows.filter((channel) => channel.enabled).length === 0,
        icon: <PowerOff size={14} />,
        onSelect: (rows) =>
          void setChannelsEnabled(
            rows.filter((channel) => channel.enabled),
            false,
          ),
      },
      {
        label: "Review deletion",
        description: (rows) =>
          `Delete ${rows.length} selected notification channel records. Retained delivery history is not removed.`,
        disabled: (rows) => rows.length === 0,
        icon: <Trash2 size={14} />,
        onSelect: requestDeleteChannels,
        tone: "danger",
      },
    ];

  return (
    <div className="consoleCrudPanel">
      <div className="consoleResourceLayout fullWidth">
        <ConsoleDataGrid
          actions={channelActions}
          columns={channelColumns}
          defaultPageSize={10}
          empty="No notification channels saved."
          getRowId={(channel) => channel.id}
          itemLabel="channels"
          renderExpandedRow={(channel) => (
            <ChannelDetailGrid channel={channel} />
          )}
          rowActions={channelActions}
          rows={channels}
          searchPlaceholder="Search channels by name, scope, delivery target, or filters"
          storageKey="vpsman.grid.fleet.notificationChannels.v2"
          title="Alert notification channels"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              onClick={createChannel}
              type="button"
            >
              <Plus size={16} />
              <span>Create channel</span>
            </button>
          }
        />
        {detailChannelId && !editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <button
                className="secondaryAction"
                type="button"
                onClick={() => {
                  const channel = channels.find(
                    (candidate) => candidate.id === detailChannelId,
                  );
                  if (channel) {
                    editChannel(channel);
                  }
                }}
              >
                Edit channel
              </button>
            }
            description="Routing filters and delivery target."
            onClose={() => setDetailChannelId(null)}
            title="Notification channel details"
          >
            {(() => {
              const channel = channels.find(
                (candidate) => candidate.id === detailChannelId,
              );
              return channel ? (
                <ChannelDetailGrid channel={channel} />
              ) : (
                <span className="mutedText">Channel no longer exists.</span>
              );
            })()}
          </ConsoleDetailPanel>
        ) : null}
        {editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <>
                <button
                  className="primaryAction"
                  type="button"
                  onClick={() => void submit()}
                >
                  {editingId ? "Update channel" : "Create channel"}
                </button>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={createChannel}
                >
                  New channel
                </button>
              </>
            }
            description="Routes alert deliveries through explicit saved records."
            onClose={() => setEditorOpen(false)}
            title={
              editingId
                ? "Edit notification channel"
                : "Create notification channel"
            }
          >
            <div className="consoleFormGrid">
              <ConsoleField label="Channel name" className="fieldWide">
                <input
                  aria-label="Notification channel name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Scope kind">
                <select
                  aria-label="Notification scope kind"
                  value={scopeKind}
                  onChange={(event) => setScopeKind(event.target.value)}
                >
                  <option value="global">global</option>
                  <option value="provider">provider</option>
                  <option value="tag">tag</option>
                  <option value="client">client</option>
                </select>
              </ConsoleField>
              <ConsoleField
                label="Scope value"
                hint={
                  scopeKind === "global"
                    ? "Global channels do not need a value."
                    : scopeKind === "client"
                      ? "Exact VPS ID is saved; type to search names or IDs."
                      : "Tag or provider value."
                }
              >
                {scopeKind === "client" ? (
                  <VpsCombobox
                    agents={agents}
                    ariaLabel="Notification scope value"
                    onChange={setScopeValue}
                    placeholder="Search notification VPS"
                    value={scopeValue}
                  />
                ) : (
                  <input
                    aria-label="Notification scope value"
                    disabled={scopeKind === "global"}
                    value={scopeValue}
                    onChange={(event) => setScopeValue(event.target.value)}
                  />
                )}
              </ConsoleField>
              <ConsoleField label="Minimum severity">
                <select
                  aria-label="Minimum severity"
                  value={minSeverity}
                  onChange={(event) => setMinSeverity(event.target.value)}
                >
                  <option value="critical">critical</option>
                  <option value="warning">warning</option>
                  <option value="info">info</option>
                </select>
              </ConsoleField>
              <ConsoleField label="State">
                <label className="checkLine inlineCheck">
                  <input
                    checked={enabled}
                    onChange={(event) => setEnabled(event.target.checked)}
                    type="checkbox"
                  />
                  <span>Route matching alerts</span>
                </label>
              </ConsoleField>
              <ConsoleField
                label="Categories"
                className="fieldWide"
                hint="Comma-separated alert categories. Empty matches all categories."
              >
                <input
                  aria-label="Alert categories"
                  value={categories}
                  onChange={(event) => setCategories(event.target.value)}
                />
                <TokenPreview empty="all categories" values={categoryTokens} />
              </ConsoleField>
              <ConsoleField
                label="Operator states"
                className="fieldWide"
                hint="Comma-separated operator states. Empty matches all states."
              >
                <input
                  aria-label="Operator states"
                  value={operatorStates}
                  onChange={(event) => setOperatorStates(event.target.value)}
                />
                <TokenPreview empty="all states" values={operatorStateTokens} />
              </ConsoleField>
              <ConsoleField label="Delivery kind">
                <input
                  aria-label="Delivery kind"
                  list="alert-delivery-kinds"
                  value={deliveryKind}
                  onChange={(event) => setDeliveryKind(event.target.value)}
                />
                <datalist id="alert-delivery-kinds">
                  <option value="audit_log" />
                  <option value="webhook" />
                  <option value="email" />
                  <option value="slack" />
                </datalist>
              </ConsoleField>
              <ConsoleField label="Delivery target" className="fieldWide">
                <input
                  aria-label="Delivery target"
                  value={target}
                  onChange={(event) => setTarget(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Cooldown seconds">
                <input
                  aria-label="Notification cooldown seconds"
                  value={cooldownSecs}
                  onChange={(event) => setCooldownSecs(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Notes" className="fieldFull">
                <textarea
                  aria-label="Notification channel notes"
                  value={notes}
                  onChange={(event) => setNotes(event.target.value)}
                />
              </ConsoleField>
            </div>
          </ConsoleDetailPanel>
        ) : null}
      </div>
      <div className="consoleOperationsBar">
        <span>
          <strong>Alert delivery queue</strong>
          <small>
            Review matching or process queued deliveries without leaving the
            registry.
          </small>
        </span>
        <div className="consoleOperationsActions">
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => void dispatch(true)}
          >
            Review matches
          </button>
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => setQueueConfirmation("dispatch")}
          >
            Review queue dispatch
          </button>
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => void process(true)}
          >
            Review queued deliveries
          </button>
          <button
            className="primaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => setQueueConfirmation("process")}
          >
            Review delivery
          </button>
        </div>
      </div>
      <ConfirmationPrompt
        confirmLabel={
          queueConfirmation === "dispatch"
            ? "Queue dispatch"
            : "Deliver queued"
        }
        detail={
          queueConfirmation === "dispatch"
            ? "Queues notification delivery records for matching active alerts."
            : "Processes queued notification delivery records and may contact configured delivery targets."
        }
        items={[
          {
            label: "Limit",
            value: "50 queued records",
          },
        ]}
        onCancel={() => setQueueConfirmation(null)}
        onConfirm={() => void confirmQueueAction()}
        open={queueConfirmation !== null}
        pending={queuePending}
        title={
          queueConfirmation === "dispatch"
            ? "Confirm notification queue dispatch"
            : "Confirm notification delivery"
        }
        tone={queueConfirmation === "process" ? "danger" : "normal"}
      />
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <ConfirmationPrompt
        confirmLabel="Delete"
        detail="Deletes selected alert notification channel records. Retained delivery history is not removed."
        items={[
          {
            label: "Channels",
            value: selectedRecordSummary(
              deleteRows,
              "channel",
              "channels",
              (row) => row.name,
              (row) => row.id,
            ),
          },
        ]}
        error={deleteError}
        onCancel={() => {
          setDeleteError(null);
          setDeleteRows(null);
        }}
        onConfirm={() => void confirmDeleteChannels()}
        open={deleteRows !== null}
        pending={deletePending}
        title="Delete notification channels"
        tone="danger"
      />
    </div>
  );
}

function NotificationDeliveryHistoryGrid({
  deliveries,
  preview,
}: {
  deliveries: FleetAlertNotificationDeliveryRecord[];
  preview: boolean;
}) {
  const columns = useMemo<
    ConsoleDataGridColumn<FleetAlertNotificationDeliveryRecord>[]
  >(
    () => [
      {
        id: "channel",
        header: "Channel",
        size: 230,
        minSize: 160,
        sortValue: (delivery) => delivery.channel_name,
        searchValue: (delivery) =>
          `${delivery.channel_name} ${delivery.alert_category}`,
        cell: (delivery) => (
          <span className="historyPrimary">
            <strong>{delivery.channel_name}</strong>
            <small>{delivery.alert_category}</small>
          </span>
        ),
      },
      {
        id: "status",
        header: "Status",
        size: 110,
        minSize: 90,
        sortValue: (delivery) => delivery.status,
        searchValue: (delivery) => `${delivery.status} ${delivery.error ?? ""}`,
        cell: (delivery) => (
          <span className="historyPrimary">
            <ConsoleStatusBadge tone={deliveryStatusTone(delivery.status)}>
              {deliveryStatusLabel(delivery.status)}
            </ConsoleStatusBadge>
            {delivery.error && (
              <small className="deliveryErrorText" title={delivery.error}>
                {shortDeliveryError(delivery.error)}
              </small>
            )}
          </span>
        ),
      },
      {
        id: "alert",
        header: "Alert",
        size: 190,
        minSize: 140,
        sortValue: (delivery) => delivery.alert_severity,
        searchValue: (delivery) =>
          `${delivery.alert_id} ${delivery.alert_severity}`,
        cell: (delivery) => (
          <span className="historyPrimary">
            <strong>{delivery.alert_severity}</strong>
            <small>{shortId(delivery.alert_id)}</small>
          </span>
        ),
      },
      {
        id: "target",
        header: "Target",
        size: 260,
        minSize: 180,
        sortValue: (delivery) => `${delivery.delivery_kind}:${delivery.target}`,
        searchValue: (delivery) =>
          `${delivery.delivery_kind} ${delivery.target}`,
        cell: (delivery) => (
          <span className="historyPrimary">
            <strong>{delivery.delivery_kind}</strong>
            <small>{delivery.target}</small>
          </span>
        ),
      },
      {
        id: "attempts",
        header: "Attempts",
        size: 105,
        minSize: 90,
        align: "end",
        sortValue: (delivery) => delivery.attempt_count,
        cell: (delivery) => (
          <span className="monoValue">{delivery.attempt_count}</span>
        ),
      },
      {
        id: "created",
        header: "Created",
        size: 140,
        minSize: 110,
        sortValue: (delivery) => delivery.created_at,
        cell: (delivery) => formatCompactTime(delivery.created_at),
      },
    ],
    [],
  );

  return (
    <ConsoleDataGrid
      columns={columns}
      defaultPageSize={8}
      empty={
        preview
          ? "No notification preview delivery rows."
          : "No notification deliveries retained."
      }
      getRowId={(delivery) => delivery.id}
      itemLabel="deliveries"
      renderExpandedRow={(delivery) => (
        <div className="gridDetailLine">
          <strong>{delivery.channel_name}</strong>
          <span>{deliveryStatusLabel(delivery.status)}</span>
          <span>{delivery.delivery_kind}</span>
          <span>{delivery.target}</span>
          <span>{delivery.attempt_count} attempts</span>
          {delivery.error && (
            <span className="deliveryErrorText" title={delivery.error}>
              error: {delivery.error}
            </span>
          )}
        </div>
      )}
      rows={deliveries}
      searchPlaceholder="Search notification deliveries"
      storageKey="vpsman.grid.fleet.notificationDeliveries.v2"
      title={
        preview
          ? "Notification delivery preview"
          : "Notification delivery history"
      }
    />
  );
}

function WebhookRuleManager({
  agents,
  onDelete,
  onDispatch,
  onDryRun,
  onOpenDeliveries,
  onPreviewDryRun,
  onPreviewRows,
  onProcess,
  onUpsert,
  rules,
}: {
  agents: AgentView[];
  onDelete: (ruleId: string) => Promise<void>;
  onDispatch: (
    request: WebhookRuleDispatchRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onDryRun: (
    request: WebhookRuleDryRunRequest,
  ) => Promise<WebhookRuleDryRunRecord>;
  onOpenDeliveries: () => void;
  onPreviewDryRun: (preview: WebhookRuleDryRunRecord | null) => void;
  onPreviewRows: (rows: WebhookRuleDeliveryRecord[]) => void;
  onProcess: (
    request: WebhookRuleProcessRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onUpsert: (request: WebhookRuleRequest) => Promise<WebhookRuleRecord>;
  rules: WebhookRuleRecord[];
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [detailRuleId, setDetailRuleId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<WebhookRuleRecord[] | null>(
    null,
  );
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [name, setName] = useState("edge-interval-webhook");
  const [enabled, setEnabled] = useState(true);
  const [expression, setExpression] = useState("interval.30sec && tag:edge");
  const [target, setTarget] = useState("https://hooks.example/vpsman");
  const [bodyTemplate, setBodyTemplate] = useState(
    "{rule.name} {event.kind} {vps.id}",
  );
  const [cooldownSecs, setCooldownSecs] = useState("300");
  const [notes, setNotes] = useState("");
  const [eventKind, setEventKind] = useState("interval.30sec");
  const [eventId, setEventId] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [queueConfirmation, setQueueConfirmation] = useState<
    "dispatch" | "process" | null
  >(null);
  const [queuePending, setQueuePending] = useState(false);

  const selectedPreviewNames = useMemo(() => {
    return agents
      .filter((agent) => agent.tags.some((tag) => expression.includes(tag)))
      .slice(0, 6)
      .map((agent) => formatVpsName(agent, "name"))
      .join(", ");
  }, [agents, expression]);

  const ruleColumns = useMemo<ConsoleDataGridColumn<WebhookRuleRecord>[]>(
    () => [
      {
        id: "name",
        header: "Rule",
        size: 260,
        minSize: 190,
        sortValue: (rule) => rule.name,
        searchValue: (rule) => `${rule.name} ${rule.notes ?? ""}`,
        cell: (rule) => (
          <span className="historyPrimary">
            <strong>{rule.name}</strong>
            <small>{rule.notes || "no notes"}</small>
          </span>
        ),
      },
      {
        id: "expression",
        header: "Expression",
        size: 320,
        minSize: 220,
        sortValue: (rule) => rule.expression,
        searchValue: (rule) => rule.expression,
        cell: (rule) => <span className="monoValue">{rule.expression}</span>,
      },
      {
        id: "target",
        header: "Target",
        size: 260,
        minSize: 180,
        sortValue: (rule) => rule.target,
        searchValue: (rule) => rule.target,
        cell: (rule) => <small title={rule.target}>{rule.target}</small>,
      },
      {
        id: "state",
        header: "State",
        size: 100,
        minSize: 90,
        sortValue: (rule) => rule.enabled,
        searchValue: (rule) => (rule.enabled ? "enabled" : "disabled"),
        cell: (rule) => (
          <ConsoleStatusBadge tone={rule.enabled ? "ok" : "warning"}>
            {rule.enabled ? "enabled" : "disabled"}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "cooldown",
        header: "Cooldown",
        size: 110,
        minSize: 90,
        sortValue: (rule) => rule.cooldown_secs,
        cell: (rule) => (
          <span className="monoValue">{rule.cooldown_secs}s</span>
        ),
      },
      {
        id: "updated",
        header: "Updated",
        size: 140,
        minSize: 110,
        sortValue: (rule) => rule.updated_at,
        cell: (rule) => formatCompactTime(rule.updated_at),
      },
    ],
    [],
  );

  function resetForm() {
    setEditingId(null);
    setName("edge-interval-webhook");
    setEnabled(true);
    setExpression("interval.30sec && tag:edge");
    setTarget("https://hooks.example/vpsman");
    setBodyTemplate("{rule.name} {event.kind} {vps.id}");
    setCooldownSecs("300");
    setNotes("");
    setStatus(null);
  }

  function createRule() {
    resetForm();
    setDetailRuleId(null);
    setEditorOpen(true);
  }

  function editRule(rule: WebhookRuleRecord) {
    setDetailRuleId(null);
    setEditingId(rule.id);
    setName(rule.name);
    setEnabled(rule.enabled);
    setExpression(rule.expression);
    setTarget(rule.target);
    setBodyTemplate(rule.body_template);
    setCooldownSecs(String(rule.cooldown_secs));
    setNotes(rule.notes ?? "");
    setStatus(`editing ${rule.name}`);
    setEditorOpen(true);
  }

  function openRuleDetails(rule: WebhookRuleRecord) {
    setEditorOpen(false);
    setDetailRuleId(rule.id);
    setStatus(`viewing ${rule.name}`);
  }

  function requestFromRule(
    rule: WebhookRuleRecord,
    overrides: Partial<WebhookRuleRequest> = {},
  ): WebhookRuleRequest {
    return {
      id: rule.id,
      name: rule.name,
      enabled: rule.enabled,
      expression: rule.expression,
      target: rule.target,
      body_template: rule.body_template,
      cooldown_secs: rule.cooldown_secs,
      notes: rule.notes,
      confirmed: true,
      ...overrides,
    };
  }

  async function submit() {
    setStatus(editingId ? "updating webhook rule" : "creating webhook rule");
    try {
      const rule = await onUpsert({
        id: editingId ?? undefined,
        name: name.trim(),
        enabled,
        expression: expression.trim(),
        target: target.trim(),
        body_template: bodyTemplate,
        cooldown_secs: optionalInteger(cooldownSecs),
        notes: notes.trim() || null,
        confirmed: true,
      });
      setEditingId(rule.id);
      setEditorOpen(true);
      setStatus(`saved ${rule.name}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "webhook save failed");
    }
  }

  function requestDeleteRules(rows: WebhookRuleRecord[]) {
    setDeleteError(null);
    setDeleteRows(rows);
  }

  async function confirmDeleteRules() {
    const rows = deleteRows ?? [];
    if (rows.length === 0 || deletePending) return;
    setDeletePending(true);
    setDeleteError(null);
    setStatus("deleting webhook rules");
    try {
      for (const rule of rows) {
        await onDelete(rule.id);
      }
      if (rows.some((rule) => rule.id === editingId)) {
        resetForm();
        setEditorOpen(false);
      }
      if (rows.some((rule) => rule.id === detailRuleId)) {
        setDetailRuleId(null);
      }
      setDeleteRows(null);
      setStatus(`deleted ${rows.length}`);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "rule delete failed";
      setDeleteError(message);
      setStatus(message);
    } finally {
      setDeletePending(false);
    }
  }

  async function setRulesEnabled(
    rows: WebhookRuleRecord[],
    nextEnabled: boolean,
  ) {
    if (rows.length === 0) return;
    setStatus(
      nextEnabled ? "enabling webhook rules" : "disabling webhook rules",
    );
    try {
      for (const rule of rows) {
        await onUpsert(requestFromRule(rule, { enabled: nextEnabled }));
      }
      setStatus(`${nextEnabled ? "enabled" : "disabled"} ${rows.length}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "rule update failed");
    }
  }

  async function dryRun(rule?: WebhookRuleRecord) {
    const request = rule
      ? {
          name: rule.name,
          enabled: rule.enabled,
          expression: rule.expression,
          target: rule.target,
          body_template: rule.body_template,
          cooldown_secs: rule.cooldown_secs,
          notes: rule.notes,
          event_kind: eventKind.trim(),
          event_id: eventId.trim() || null,
        }
      : {
          name: name.trim(),
          enabled,
          expression: expression.trim(),
          target: target.trim(),
          body_template: bodyTemplate,
          cooldown_secs: optionalInteger(cooldownSecs),
          notes: notes.trim() || null,
          event_kind: eventKind.trim(),
          event_id: eventId.trim() || null,
        };
    setStatus("rendering webhook dry run");
    try {
      const preview = await onDryRun(request);
      onPreviewDryRun(preview);
      onPreviewRows(preview.delivery ? [preview.delivery] : []);
      onOpenDeliveries();
      setStatus(`dry run matched ${preview.matched_vps.length}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "dry run failed");
    }
  }

  async function dispatch(dryRunMode: boolean) {
    setStatus(dryRunMode ? "matching webhook rules" : "queueing webhooks");
    if (!dryRunMode) {
      setQueuePending(true);
    }
    try {
      const rows = await onDispatch({
        event_kind: eventKind.trim(),
        event_id: eventId.trim() || null,
        limit: 50,
        dry_run: dryRunMode,
        confirmed: !dryRunMode,
      });
      if (dryRunMode) {
        onPreviewRows(rows);
        onOpenDeliveries();
      }
      setStatus(`${dryRunMode ? "matched" : "queued"} ${rows.length}`);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "webhook dispatch failed",
      );
    } finally {
      if (!dryRunMode) {
        setQueuePending(false);
      }
    }
  }

  async function process(dryRunMode: boolean) {
    setStatus(dryRunMode ? "previewing webhook queue" : "delivering webhooks");
    if (!dryRunMode) {
      setQueuePending(true);
    }
    try {
      const rows = await onProcess({
        limit: 50,
        status: "queued",
        dry_run: dryRunMode,
        confirmed: !dryRunMode,
      });
      if (dryRunMode) {
        onPreviewRows(rows);
        onOpenDeliveries();
      }
      setStatus(`${dryRunMode ? "previewed" : "processed"} ${rows.length}`);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "webhook processing failed",
      );
    } finally {
      if (!dryRunMode) {
        setQueuePending(false);
      }
    }
  }

  async function confirmQueueAction() {
    const action = queueConfirmation;
    if (!action || queuePending) {
      return;
    }
    if (action === "dispatch") {
      await dispatch(false);
    } else {
      await process(false);
    }
    setQueueConfirmation(null);
  }

  const ruleActions: ConsoleDataGridAction<WebhookRuleRecord>[] = [
    {
      label: "Details",
      description: (rows) =>
        actionTargetDescription(
          "Open details for",
          "webhook rule",
          rows[0]?.name,
          "Opens read-only rule details below the table.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Eye size={14} />,
      onSelect: (rows) => rows[0] && openRuleDetails(rows[0]),
    },
    {
      label: "Edit",
      description: (rows) =>
        actionTargetDescription(
          "Edit",
          "webhook rule",
          rows[0]?.name,
          "Opens the rule editor below the table.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Pencil size={14} />,
      onSelect: (rows) => rows[0] && editRule(rows[0]),
    },
    {
      label: "Review rule",
      description: (rows) =>
        actionTargetDescription(
          "Review",
          "webhook rule",
          rows[0]?.name,
          "Runs a dry-run with the current preview event.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Eye size={14} />,
      onSelect: (rows) => rows[0] && void dryRun(rows[0]),
    },
    {
      label: "Enable",
      description: (rows) =>
        `Enable ${rows.filter((rule) => !rule.enabled).length} disabled selected webhook rule records.`,
      disabled: (rows) => rows.filter((rule) => !rule.enabled).length === 0,
      icon: <Power size={14} />,
      onSelect: (rows) =>
        void setRulesEnabled(
          rows.filter((rule) => !rule.enabled),
          true,
        ),
    },
    {
      label: "Disable",
      description: (rows) =>
        `Disable ${rows.filter((rule) => rule.enabled).length} enabled selected webhook rule records.`,
      disabled: (rows) => rows.filter((rule) => rule.enabled).length === 0,
      icon: <PowerOff size={14} />,
      onSelect: (rows) =>
        void setRulesEnabled(
          rows.filter((rule) => rule.enabled),
          false,
        ),
    },
    {
      label: "Review deletion",
      description: (rows) =>
        `Delete ${rows.length} selected webhook rule records. Retained delivery history is not removed.`,
      disabled: (rows) => rows.length === 0,
      icon: <Trash2 size={14} />,
      onSelect: requestDeleteRules,
      tone: "danger",
    },
  ];

  return (
    <div className="consoleCrudPanel">
      <div className="consoleResourceLayout fullWidth">
        <ConsoleDataGrid
          actions={ruleActions}
          columns={ruleColumns}
          defaultPageSize={10}
          empty="No webhook rules saved."
          getRowId={(rule) => rule.id}
          itemLabel="rules"
          renderExpandedRow={(rule) => <WebhookRuleDetailGrid rule={rule} />}
          rowActions={ruleActions}
          rows={rules}
          searchPlaceholder="Search webhook rules by name, expression, target, or notes"
          storageKey="vpsman.grid.fleet.webhookRules.v2"
          title="Webhook rules"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              onClick={createRule}
              type="button"
            >
              <Plus size={16} />
              <span>Create rule</span>
            </button>
          }
        />
        {detailRuleId && !editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={() => {
                    const rule = rules.find(
                      (candidate) => candidate.id === detailRuleId,
                    );
                    if (rule) {
                      void dryRun(rule);
                    }
                  }}
                >
                  Review rule
                </button>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={() => {
                    const rule = rules.find(
                      (candidate) => candidate.id === detailRuleId,
                    );
                    if (rule) {
                      editRule(rule);
                    }
                  }}
                >
                  Edit rule
                </button>
              </>
            }
            description="Expression, target, and delivery template."
            onClose={() => setDetailRuleId(null)}
            title="Webhook rule details"
          >
            {(() => {
              const rule = rules.find(
                (candidate) => candidate.id === detailRuleId,
              );
              return rule ? (
                <WebhookRuleDetailGrid rule={rule} />
              ) : (
                <span className="mutedText">Rule no longer exists.</span>
              );
            })()}
          </ConsoleDetailPanel>
        ) : null}
        {editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={() => void dryRun()}
                >
                  Review rule
                </button>
                <button
                  className="primaryAction"
                  type="button"
                  onClick={() => void submit()}
                >
                  {editingId ? "Update rule" : "Create rule"}
                </button>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={createRule}
                >
                  New rule
                </button>
              </>
            }
            description="Webhook rules are saved expression records with explicit preview and delivery operations."
            onClose={() => setEditorOpen(false)}
            title={editingId ? "Edit webhook rule" : "Create webhook rule"}
          >
            <div className="consoleFormGrid">
              <ConsoleField label="Rule name" className="fieldWide">
                <input
                  aria-label="Webhook rule name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="State">
                <label className="checkLine inlineCheck">
                  <input
                    checked={enabled}
                    onChange={(event) => setEnabled(event.target.checked)}
                    type="checkbox"
                  />
                  <span>Evaluate rule</span>
                </label>
              </ConsoleField>
              <ConsoleField label="Cooldown seconds">
                <input
                  aria-label="Webhook cooldown seconds"
                  value={cooldownSecs}
                  onChange={(event) => setCooldownSecs(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField
                label="Expression"
                className="fieldFull"
                hint="Example: interval.30sec && tag:edge"
              >
                <SearchExpressionInput
                  agents={agents}
                  ariaLabel="Webhook expression"
                  className="targetExpressionBar"
                  onChange={setExpression}
                  placeholder="interval.30sec && tag:edge"
                  suggestions={WEBHOOK_EXPRESSION_SUGGESTIONS}
                  value={expression}
                />
              </ConsoleField>
              <ConsoleField label="Target URL" className="fieldFull">
                <input
                  aria-label="Webhook target"
                  value={target}
                  onChange={(event) => setTarget(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Preview event kind">
                <input
                  aria-label="Webhook event kind"
                  value={eventKind}
                  onChange={(event) => setEventKind(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Preview event id">
                <input
                  aria-label="Webhook event id"
                  value={eventId}
                  onChange={(event) => setEventId(event.target.value)}
                  placeholder="optional"
                />
              </ConsoleField>
              <ConsoleField label="Notes" className="fieldFull">
                <textarea
                  aria-label="Webhook rule notes"
                  value={notes}
                  onChange={(event) => setNotes(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Body template" className="fieldFull">
                <WebhookTemplateEditor
                  value={bodyTemplate}
                  onChange={setBodyTemplate}
                />
              </ConsoleField>
              <ConsoleField label="Local hint" className="fieldFull">
                <span className="monoValue">
                  {selectedPreviewNames ||
                    "Use server dry-run for exact matches."}
                </span>
              </ConsoleField>
            </div>
          </ConsoleDetailPanel>
        ) : null}
      </div>
      <div className="consoleOperationsBar">
        <span>
          <strong>Webhook queue</strong>
          <small>
            Review first; retained deliveries stay in the Deliveries tab.
          </small>
        </span>
        <div className="consoleOperationsActions">
          <label className="consoleField">
            <span>Event kind</span>
            <input
              aria-label="Webhook dispatch event kind"
              value={eventKind}
              onChange={(event) => setEventKind(event.target.value)}
            />
          </label>
          <label className="consoleField">
            <span>Event id</span>
            <input
              aria-label="Webhook dispatch event id"
              value={eventId}
              onChange={(event) => setEventId(event.target.value)}
              placeholder="optional"
            />
          </label>
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => void dispatch(true)}
          >
            Review matches
          </button>
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => setQueueConfirmation("dispatch")}
          >
            Review queue dispatch
          </button>
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => void process(true)}
          >
            Review queued deliveries
          </button>
          <button
            className="primaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => setQueueConfirmation("process")}
          >
            Review delivery
          </button>
        </div>
      </div>
      <ConfirmationPrompt
        confirmLabel={
          queueConfirmation === "dispatch"
            ? "Queue dispatch"
            : "Deliver queued"
        }
        detail={
          queueConfirmation === "dispatch"
            ? "Queues webhook delivery records for matching rules and the selected event."
            : "Processes queued webhook delivery records and may call external webhook endpoints."
        }
        items={[
          {
            label: "Event",
            value: `${eventKind.trim() || "event"}${eventId.trim() ? ` / ${eventId.trim()}` : ""}`,
          },
          {
            label: "Limit",
            value: "50 queued records",
          },
        ]}
        onCancel={() => setQueueConfirmation(null)}
        onConfirm={() => void confirmQueueAction()}
        open={queueConfirmation !== null}
        pending={queuePending}
        title={
          queueConfirmation === "dispatch"
            ? "Confirm webhook queue dispatch"
            : "Confirm webhook delivery"
        }
        tone={queueConfirmation === "process" ? "danger" : "normal"}
      />
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <ConfirmationPrompt
        confirmLabel="Delete"
        detail="Deletes selected webhook rule records. Retained delivery history is not removed."
        items={[
          {
            label: "Rules",
            value: selectedRecordSummary(
              deleteRows,
              "rule",
              "rules",
              (row) => row.name,
              (row) => row.id,
            ),
          },
        ]}
        error={deleteError}
        onCancel={() => {
          setDeleteError(null);
          setDeleteRows(null);
        }}
        onConfirm={() => void confirmDeleteRules()}
        open={deleteRows !== null}
        pending={deletePending}
        title="Delete webhook rules"
        tone="danger"
      />
    </div>
  );
}

function WebhookDryRunNotice({
  agents: _agents,
  preview,
}: {
  agents: AgentView[];
  preview: WebhookRuleDryRunRecord;
}) {
  const matchedNames = preview.matched_vps
    .slice(0, 8)
    .map((agent) => formatVpsName(agent, "name"))
    .join(", ");
  return (
    <div className="consoleInlineNotice">
      <strong>{preview.matched_vps.length} VPSs matched webhook dry run</strong>
      <small>{matchedNames || "No VPSs matched this rule."}</small>
      {preview.validation_errors.length > 0 && (
        <small>{preview.validation_errors.join(" · ")}</small>
      )}
      <small>{preview.rendered_message}</small>
    </div>
  );
}

function WebhookDeliveryHistoryGrid({
  deliveries,
  preview,
}: {
  deliveries: WebhookRuleDeliveryRecord[];
  preview: boolean;
}) {
  const columns = useMemo<ConsoleDataGridColumn<WebhookRuleDeliveryRecord>[]>(
    () => [
      {
        id: "rule",
        header: "Rule",
        size: 230,
        minSize: 160,
        sortValue: (delivery) => delivery.rule_name,
        searchValue: (delivery) =>
          `${delivery.rule_name} ${delivery.event_kind}`,
        cell: (delivery) => (
          <span className="historyPrimary">
            <strong>{delivery.rule_name}</strong>
            <small>
              {delivery.event_kind}
              {delivery.event_id ? ` · ${delivery.event_id}` : ""}
            </small>
          </span>
        ),
      },
      {
        id: "status",
        header: "Status",
        size: 110,
        minSize: 90,
        sortValue: (delivery) => delivery.status,
        searchValue: (delivery) => `${delivery.status} ${delivery.error ?? ""}`,
        cell: (delivery) => (
          <span className="historyPrimary">
            <ConsoleStatusBadge tone={deliveryStatusTone(delivery.status)}>
              {deliveryStatusLabel(delivery.status)}
            </ConsoleStatusBadge>
            {delivery.error && (
              <small className="deliveryErrorText" title={delivery.error}>
                {shortDeliveryError(delivery.error)}
              </small>
            )}
          </span>
        ),
      },
      {
        id: "target",
        header: "Target",
        size: 260,
        minSize: 180,
        sortValue: (delivery) => delivery.target,
        searchValue: (delivery) => delivery.target,
        cell: (delivery) => (
          <small title={delivery.target}>{delivery.target}</small>
        ),
      },
      {
        id: "matched",
        header: "Matched VPSs",
        size: 160,
        minSize: 130,
        sortValue: (delivery) => delivery.matched_vps.length,
        searchValue: (delivery) =>
          delivery.matched_vps
            .map((agent) => agent.display_name || agent.id)
            .join(" "),
        cell: (delivery) => (
          <span className="historyPrimary">
            <strong>{delivery.matched_vps.length}</strong>
            <small>
              {delivery.matched_vps
                .slice(0, 3)
                .map((agent) => formatVpsName(agent, "name"))
                .join(", ") || "none"}
            </small>
          </span>
        ),
      },
      {
        id: "attempts",
        header: "Attempts",
        size: 105,
        minSize: 90,
        align: "end",
        sortValue: (delivery) => delivery.attempt_count,
        cell: (delivery) => (
          <span className="monoValue">{delivery.attempt_count}</span>
        ),
      },
      {
        id: "created",
        header: "Created",
        size: 140,
        minSize: 110,
        sortValue: (delivery) => delivery.created_at,
        cell: (delivery) => formatCompactTime(delivery.created_at),
      },
    ],
    [],
  );

  return (
    <ConsoleDataGrid
      columns={columns}
      defaultPageSize={8}
      empty={
        preview
          ? "No webhook preview delivery rows."
          : "No webhook deliveries retained."
      }
      getRowId={(delivery) => delivery.id}
      itemLabel="deliveries"
      renderExpandedRow={(delivery) => (
        <div className="gridDetailLine">
          <strong>{delivery.rule_name}</strong>
          <span>{deliveryStatusLabel(delivery.status)}</span>
          <span>{delivery.event_kind}</span>
          <span>{delivery.target}</span>
          <span>{delivery.attempt_count} attempts</span>
          {delivery.error && (
            <span className="deliveryErrorText" title={delivery.error}>
              error: {delivery.error}
            </span>
          )}
        </div>
      )}
      rows={deliveries}
      searchPlaceholder="Search webhook deliveries"
      storageKey="vpsman.grid.fleet.webhookDeliveries.v2"
      title={preview ? "Webhook delivery preview" : "Webhook delivery history"}
    />
  );
}

function WebhookDeliveryMaintenancePanel({
  onRotate,
  rules,
}: {
  onRotate: (
    request: WebhookDeliveryRotationRequest,
  ) => Promise<WebhookDeliveryRotationResponse>;
  rules: WebhookRuleRecord[];
}) {
  const [rotationDays, setRotationDays] = useState("90");
  const [rotationStatus, setRotationStatus] =
    useState<WebhookRuleDeliveryHistoryStatus>("delivered");
  const [rotationRuleId, setRotationRuleId] = useState("");
  const [rotationPreview, setRotationPreview] =
    useState<WebhookDeliveryRotationResponse | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [rotationPending, setRotationPending] = useState(false);
  const [rotationError, setRotationError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  async function rotate(confirmed: boolean) {
    if (rotationPending) {
      return;
    }
    setRotationPending(true);
    setRotationError(null);
    setStatus(
      confirmed ? "deleting matched deliveries" : "previewing rotation",
    );
    try {
      const response = await onRotate({
        older_than_days: optionalInteger(rotationDays),
        status: rotationStatus,
        rule_id: rotationRuleId || null,
        confirmed,
      });
      setRotationPreview(response);
      setConfirmDelete(false);
      setStatus(
        `${response.matched_count} matched / ${response.deleted_count} deleted`,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : "rotation failed";
      setRotationError(message);
      setStatus(message);
    } finally {
      setRotationPending(false);
    }
  }

  return (
    <div className="consoleResourceLayout fullWidth">
      <ConsoleDetailPanel
        actions={
          <>
            <button
              className="secondaryAction"
              disabled={rotationPending}
              type="button"
              onClick={() => void rotate(false)}
            >
              Review rotation
            </button>
            <button
              className="secondaryAction"
              disabled={
                rotationPending ||
                !rotationPreview ||
                rotationPreview.matched_count === 0
              }
              type="button"
              onClick={() => {
                setRotationError(null);
                setConfirmDelete(true);
              }}
            >
              Review cleanup
            </button>
          </>
        }
        description="Rotation is a deliberate maintenance operation: review first, then confirm deletion."
        title="Webhook delivery maintenance"
      >
        <div className="consoleFormGrid">
          <ConsoleField label="Older than days">
            <input
              aria-label="Webhook rotation days"
              value={rotationDays}
              onChange={(event) => setRotationDays(event.target.value)}
            />
          </ConsoleField>
          <ConsoleField label="Status">
            <select
              aria-label="Webhook rotation status"
              value={rotationStatus}
              onChange={(event) =>
                setRotationStatus(event.target.value as WebhookRuleDeliveryHistoryStatus)
              }
            >
              {WEBHOOK_RULE_DELIVERY_HISTORY_STATUSES.map((status) => (
                <option key={status} value={status}>
                  {status.replace(/_/g, " ")}
                </option>
              ))}
            </select>
          </ConsoleField>
          <ConsoleField label="Rule">
            <select
              aria-label="Webhook rotation rule"
              value={rotationRuleId}
              onChange={(event) => setRotationRuleId(event.target.value)}
            >
              <option value="">all rules</option>
              {rules.map((rule) => (
                <option key={rule.id} value={rule.id}>
                  {rule.name}
                </option>
              ))}
            </select>
          </ConsoleField>
          <ConsoleField label="Rotation result" className="fieldWide">
            <span className="monoValue">
              {rotationPreview
                ? `${rotationPreview.matched_count} matched / ${rotationPreview.deleted_count} deleted`
                : "not reviewed"}
            </span>
          </ConsoleField>
        </div>
        {status && <small className="fleetPolicyStatus">{status}</small>}
      </ConsoleDetailPanel>
      <ConfirmationPrompt
        confirmLabel="Delete retained history"
        detail="Deletes only the webhook delivery history rows matched by the last preview. Rules are not deleted."
        items={[
          {
            label: "Matched rows",
            value: rotationPreview?.matched_count ?? 0,
          },
          {
            label: "Status",
            value: rotationPreview?.status ?? "any",
          },
        ]}
        error={rotationError}
        onCancel={() => {
          setRotationError(null);
          setConfirmDelete(false);
        }}
        onConfirm={() => void rotate(true)}
        open={confirmDelete}
        pending={rotationPending}
        title="Delete webhook delivery history"
        tone="danger"
      />
    </div>
  );
}

function deliveryStatusTone(
  status: string,
): "critical" | "warning" | "ok" | "info" {
  if (status === "delivered" || status === "sent") {
    return "ok";
  }
  if (status === "failed") {
    return "critical";
  }
  if (status === "queued" || status === "retrying") {
    return "warning";
  }
  return "info";
}

function deliveryStatusLabel(status: string): string {
  if (status === "delivery_dry_run" || status === "matched_dry_run") {
    return "dry run";
  }
  return status.replace(/_/g, " ");
}

function WebhookTemplateEditor({
  onChange,
  value,
}: {
  onChange: (value: string) => void;
  value: string;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => {
    if (!containerRef.current) {
      return;
    }
    const view = new EditorView({
      doc: value,
      extensions: [
        basicSetup,
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current(update.state.doc.toString());
          }
        }),
      ],
      parent: containerRef.current,
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) {
      return;
    }
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
    });
  }, [value]);

  return <div className="webhookCodeMirror" ref={containerRef} />;
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
  const criticalCount = alerts.filter(
    (alert) => alert.severity === "critical",
  ).length;
  const warningCount = alerts.filter(
    (alert) => alert.severity === "warning",
  ).length;
  const nameById = useMemo(
    () => agentNamesById(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );

  const alertColumns = useMemo<ConsoleDataGridColumn<FleetAlertRecord>[]>(
    () => [
      {
        id: "severity",
        header: "Severity",
        size: 115,
        minSize: 95,
        sortValue: (alert) => alert.severity,
        searchValue: (alert) => alert.severity,
        cell: (alert) => (
          <ConsoleStatusBadge tone={alertTone(alert.severity)}>
            {alert.severity}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "alert",
        header: "Alert",
        size: 360,
        minSize: 240,
        sortValue: (alert) => alert.title,
        searchValue: (alert) => `${alert.title} ${alert.detail}`,
        cell: (alert) => (
          <span className="historyPrimary">
            <strong>{alert.title}</strong>
            <small>{alert.detail}</small>
          </span>
        ),
      },
      {
        id: "target",
        header: "Target",
        size: 210,
        minSize: 150,
        sortValue: (alert) =>
          alert.client_id
            ? (nameById.get(alert.client_id) ?? alert.client_id)
            : alertTargetLabel(alert),
        searchValue: (alert) =>
          `${alert.target_kind} ${alert.target_id} ${alert.client_id ?? ""} ${
            alert.client_id ? (nameById.get(alert.client_id) ?? "") : ""
          }`,
        cell: (alert) => {
          const label = alert.client_id
            ? (nameById.get(alert.client_id) ?? "Unnamed VPS")
            : alertTargetLabel(alert);
          return (
            <span
              className="historyPrimary"
              title={`${alert.target_kind}:${alert.target_id}`}
            >
              <strong>{label}</strong>
              <small>{alert.target_kind}</small>
            </span>
          );
        },
      },
      {
        id: "category",
        header: "Category",
        size: 140,
        minSize: 110,
        sortValue: (alert) => alert.category,
        searchValue: (alert) => alert.category,
        cell: (alert) => <span className="monoValue">{alert.category}</span>,
      },
      {
        id: "state",
        header: "Operator state",
        size: 170,
        minSize: 150,
        sortValue: alertOperatorState,
        searchValue: (alert) =>
          `${alertOperatorState(alert)} ${alert.state_reason ?? ""}`,
        cell: (alert) => {
          const operatorState = alertOperatorState(alert);
          return (
            <span className="historyPrimary">
              <ConsoleStatusBadge
                tone={operatorState === "open" ? "warning" : "info"}
              >
                {operatorState}
              </ConsoleStatusBadge>
              {alert.state_reason && <small>{alert.state_reason}</small>}
            </span>
          );
        },
      },
      {
        id: "observed",
        header: "Observed",
        size: 140,
        minSize: 110,
        sortValue: (alert) => alert.observed_at,
        cell: (alert) => formatCompactTime(alert.observed_at),
      },
    ],
    [nameById],
  );

  async function updateAlerts(
    rows: FleetAlertRecord[],
    action: FleetAlertStateRequest["action"],
  ) {
    if (rows.length === 0 || pending) {
      return;
    }
    setPending(`${action}:${rows.map((alert) => alert.id).join(",")}`);
    try {
      for (const alert of rows) {
        await onUpdate({
          alert_id: alert.id,
          action,
          muted_for_secs: action === "mute" ? 4 * 60 * 60 : null,
          reason:
            action === "mute"
              ? "panel mute"
              : action === "acknowledge"
                ? "panel acknowledgement"
                : action === "escalate"
                  ? "panel escalation"
                  : "panel clear",
          confirmed: true,
        });
      }
    } finally {
      setPending(null);
    }
  }

  const openRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => alertOperatorState(alert) === "open");
  const triagedRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => alertOperatorState(alert) !== "open");

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
      <ConsoleDataGrid
        actions={[
          {
            label: "Acknowledge open",
            description: (rows) =>
              `Acknowledge ${openRows(rows).length} selected open fleet alerts.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <Check size={14} />,
            onSelect: (rows) =>
              void updateAlerts(openRows(rows), "acknowledge"),
          },
          {
            label: "Mute open 4h",
            description: (rows) =>
              `Mute ${openRows(rows).length} selected open fleet alerts for four hours.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <VolumeX size={14} />,
            onSelect: (rows) => void updateAlerts(openRows(rows), "mute"),
          },
          {
            label: "Escalate open",
            description: (rows) =>
              `Escalate ${openRows(rows).length} selected open fleet alerts.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => void updateAlerts(openRows(rows), "escalate"),
          },
          {
            label: "Clear triaged",
            description: (rows) =>
              `Clear ${triagedRows(rows).length} selected triaged fleet alerts.`,
            disabled: (rows) =>
              pending != null || triagedRows(rows).length === 0,
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => void updateAlerts(triagedRows(rows), "clear"),
          },
        ]}
        columns={alertColumns}
        defaultPageSize={10}
        empty="No active fleet alerts."
        getRowId={(alert) => alert.id}
        itemLabel="alerts"
        renderExpandedRow={(alert) => (
          <div className="consoleGridDetails">
            <span>
              <strong>Status:</strong> {alert.status}
            </span>
            <span>
              <strong>Target:</strong> {alert.target_kind}:{alert.target_id}
            </span>
            {alert.muted_until_unix && (
              <span>
                <strong>Muted until:</strong>{" "}
                {formatUnixTime(alert.muted_until_unix)}
              </span>
            )}
            <span>
              <strong>Escalation:</strong> {alert.escalation_level ?? 0}
            </span>
            <pre>{JSON.stringify(alert.evidence, null, 2)}</pre>
          </div>
        )}
        rowActions={[
          {
            label: "Ack",
            description: (rows) =>
              actionTargetDescription(
                "Acknowledge",
                "fleet alert",
                rows[0]?.title,
                "Marks the open alert as acknowledged.",
              ),
            disabled: (rows) =>
              pending != null ||
              !rows[0] ||
              alertOperatorState(rows[0]) !== "open",
            icon: <Check size={14} />,
            onSelect: (rows) => void updateAlerts(rows, "acknowledge"),
          },
          {
            label: "Mute",
            description: (rows) =>
              actionTargetDescription(
                "Mute",
                "fleet alert",
                rows[0]?.title,
                "Suppresses the open alert for four hours.",
              ),
            disabled: (rows) =>
              pending != null ||
              !rows[0] ||
              alertOperatorState(rows[0]) !== "open",
            icon: <VolumeX size={14} />,
            onSelect: (rows) => void updateAlerts(rows, "mute"),
          },
          {
            label: "Escalate",
            description: (rows) =>
              actionTargetDescription(
                "Escalate",
                "fleet alert",
                rows[0]?.title,
                "Raises the open alert escalation level.",
              ),
            disabled: (rows) =>
              pending != null ||
              !rows[0] ||
              alertOperatorState(rows[0]) !== "open",
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => void updateAlerts(rows, "escalate"),
          },
          {
            label: "Clear",
            description: (rows) =>
              actionTargetDescription(
                "Clear",
                "fleet alert",
                rows[0]?.title,
                "Clears a triaged alert.",
              ),
            disabled: (rows) =>
              pending != null ||
              !rows[0] ||
              alertOperatorState(rows[0]) === "open",
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => void updateAlerts(rows, "clear"),
          },
        ]}
        renderSelectionPanel={(rows) => {
          const selectedOpen = openRows(rows).length;
          const selectedTriaged = triagedRows(rows).length;
          return (
            <span>
              {rows.length} selected · {selectedOpen} open · {selectedTriaged}{" "}
              triaged
            </span>
          );
        }}
        rows={alerts}
        searchPlaceholder="Search alerts by VPS, category, state, or detail"
        storageKey="vpsman.grid.fleet.alerts.v1"
        title="Fleet alerts"
      />
    </div>
  );
}

function formatUnixTime(value: number): string {
  return formatCompactTime(new Date(value * 1000).toISOString());
}

function alertTone(severity: string): "critical" | "warning" | "info" {
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

function alertOperatorState(alert: FleetAlertRecord): string {
  return alert.operator_state?.trim() || "open";
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

function latestTelemetryNetworkRatesByClient(
  rates: TelemetryNetworkRateRecord[],
) {
  const latest = new Map<string, Map<string, TelemetryNetworkRateRecord>>();
  for (const rate of rates) {
    const clientRates =
      latest.get(rate.client_id) ??
      new Map<string, TelemetryNetworkRateRecord>();
    const current = clientRates.get(rate.interface);
    if (!current || rate.bucket_start > current.bucket_start) {
      clientRates.set(rate.interface, rate);
    }
    latest.set(rate.client_id, clientRates);
  }
  return new Map(
    Array.from(latest.entries(), ([clientId, byInterface]) => [
      clientId,
      Array.from(byInterface.values()),
    ]),
  );
}

function latestTelemetryTunnelsByClient(tunnels: TelemetryTunnelRecord[]) {
  const latest = new Map<string, Map<string, TelemetryTunnelRecord>>();
  for (const tunnel of tunnels) {
    const clientTunnels =
      latest.get(tunnel.client_id) ?? new Map<string, TelemetryTunnelRecord>();
    const current = clientTunnels.get(tunnel.interface);
    if (!current || tunnel.observed_at > current.observed_at) {
      clientTunnels.set(tunnel.interface, tunnel);
    }
    latest.set(tunnel.client_id, clientTunnels);
  }
  return new Map(
    Array.from(latest.entries(), ([clientId, byInterface]) => [
      clientId,
      Array.from(byInterface.values()),
    ]),
  );
}

function formatLoad(value: number | undefined) {
  return typeof value === "number" ? value.toFixed(2) : "No rollup";
}

function formatMemoryUsed(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || rollup.memory_total_bytes_max <= 0) {
    return "No rollup";
  }
  const used =
    rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg;
  const percent = Math.round((used / rollup.memory_total_bytes_max) * 100);
  return `${percent}% (${formatBytes(used)} / ${formatBytes(rollup.memory_total_bytes_max)})`;
}

function formatDiskFree(rollup: TelemetryRollupRecord | null | undefined) {
  if (!rollup || rollup.disk_total_bytes_max <= 0) {
    return "No rollup";
  }
  const percent = Math.round(
    (rollup.disk_available_bytes_avg / rollup.disk_total_bytes_max) * 100,
  );
  return `${percent}% free (${formatBytes(rollup.disk_available_bytes_avg)} / ${formatBytes(rollup.disk_total_bytes_max)})`;
}

function formatNetworkBytes(rollup: TelemetryRollupRecord | null | undefined) {
  if (
    !rollup ||
    (rollup.network_rx_bytes_max === 0 && rollup.network_tx_bytes_max === 0)
  ) {
    return "No counters";
  }
  return `RX ${formatBytes(rollup.network_rx_bytes_max)} / TX ${formatBytes(rollup.network_tx_bytes_max)}`;
}

function formatNetworkRateSummary(
  rates: TelemetryNetworkRateRecord[],
  rollup: TelemetryRollupRecord | null | undefined,
) {
  if (rates.length === 0) {
    return rollup &&
      (rollup.network_rx_bytes_max > 0 || rollup.network_tx_bytes_max > 0)
      ? "Rate rollup pending; counters active"
      : "Awaiting rate rollup";
  }
  const rx = rates.reduce((total, rate) => total + rate.rx_bps_avg, 0);
  const tx = rates.reduce((total, rate) => total + rate.tx_bps_avg, 0);
  return `RX ${formatBitsPerSecond(rx)} / TX ${formatBitsPerSecond(tx)}`;
}

function formatSignalTraffic(
  rollup: TelemetryRollupRecord | null | undefined,
  rates: TelemetryNetworkRateRecord[],
) {
  if (rates.length > 0) {
    const totalBps = rates.reduce(
      (total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg,
      0,
    );
    return formatBitsPerSecond(totalBps);
  }
  if (
    rollup &&
    (rollup.network_rx_bytes_max > 0 || rollup.network_tx_bytes_max > 0)
  ) {
    return formatBytes(
      rollup.network_rx_bytes_max + rollup.network_tx_bytes_max,
    );
  }
  return "Awaiting rate rollup";
}

function formatSignalSamples(
  rollup: TelemetryRollupRecord | null | undefined,
  rates: TelemetryNetworkRateRecord[],
) {
  if (rollup && rollup.sample_count > 0) {
    return `${rollup.sample_count} rollup`;
  }
  const rateSamples = rates.reduce(
    (total, rate) => total + rate.sample_count,
    0,
  );
  return rateSamples > 0 ? `${rateSamples} rate` : "No rollup";
}

function formatPrivilege(capabilities: AgentView["capabilities"] | undefined) {
  if (!capabilities || capabilities.privilege_mode === "unknown") {
    return "Unknown";
  }
  const uid =
    typeof capabilities.effective_uid === "number"
      ? ` uid ${capabilities.effective_uid}`
      : "";
  return capabilities.privilege_mode === "root"
    ? `Root${uid}`
    : `Unprivileged${uid}`;
}

function formatTunnelCapability(
  capabilities: AgentView["capabilities"] | undefined,
) {
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
            <button
              className="secondaryAction compactAction"
              onClick={onOpenPrivilegeUnlock}
              type="button"
            >
              <LockKeyhole size={15} />
              Unlock privilege
            </button>
          )}
          {jobId && onOpenJobDetails && (
            <button
              className="secondaryAction compactAction"
              onClick={() => onOpenJobDetails(jobId)}
              type="button"
            >
              Job {shortId(jobId)}
            </button>
          )}
        </div>
        {progress && (
          <FailureReasonGroups reasons={progress.failureReasons ?? []} />
        )}
        {snapshot && <NetworkInterfaceList snapshot={snapshot} />}
      </div>
    </div>
  );
}

function NetworkInterfaceList({
  snapshot,
}: {
  snapshot: NetworkInterfacesSnapshot;
}) {
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

function interfaceStateSummary(
  networkInterface: NetworkInterfaceSnapshotRecord,
) {
  const state =
    networkInterface.operstate ??
    (networkInterface.flags?.includes("up") ? "up" : "unknown");
  const mtu =
    typeof networkInterface.mtu === "number"
      ? `mtu ${networkInterface.mtu}`
      : "mtu unknown";
  const mac = networkInterface.mac
    ? `mac ${networkInterface.mac}`
    : "mac unknown";
  return `${state}; ${mtu}; ${mac}`;
}

function interfaceAddressSummary(
  networkInterface: NetworkInterfaceSnapshotRecord,
) {
  const addresses = networkInterface.addresses ?? [];
  if (addresses.length === 0) {
    return "no IPs reported";
  }
  return addresses
    .map(
      (address) =>
        `${address.family} ${address.address}${typeof address.prefix_len === "number" ? `/${address.prefix_len}` : ""}`,
    )
    .join(", ");
}

function interfaceTrafficSummary(
  networkInterface: NetworkInterfaceSnapshotRecord,
) {
  const rxBytes =
    typeof networkInterface.rx_bytes === "number"
      ? networkInterface.rx_bytes
      : 0;
  const txBytes =
    typeof networkInterface.tx_bytes === "number"
      ? networkInterface.tx_bytes
      : 0;
  return `RX ${formatBytes(rxBytes)} / TX ${formatBytes(txBytes)}`;
}

function parseNetworkInterfacesSnapshot(
  outputs: JobOutputRecord[],
): NetworkInterfacesSnapshot | null {
  const snapshots = outputs
    .filter((output) => output.stream === "status" && output.data_base64)
    .map((output) => {
      try {
        const value = JSON.parse(
          decodeOutputPreview(output.data_base64),
        ) as unknown;
        return isNetworkInterfacesSnapshot(value) ? value : null;
      } catch {
        return null;
      }
    })
    .filter((value): value is NetworkInterfacesSnapshot => value !== null);
  return snapshots.length > 0 ? snapshots[snapshots.length - 1] : null;
}

function isNetworkInterfacesSnapshot(
  value: unknown,
): value is NetworkInterfacesSnapshot {
  if (!value || typeof value !== "object") {
    return false;
  }
  const record = value as Partial<NetworkInterfacesSnapshot>;
  return (
    record.type === "network_interfaces" && Array.isArray(record.interfaces)
  );
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
          rollup &&
          (rollup.network_rx_bytes_max > 0 || rollup.network_tx_bytes_max > 0)
            ? "Counter-only telemetry; rate rollup pending"
            : "Awaiting rate rollup"
        }
      />
    );
  }
  return (
    <div className="timeline telemetryTimeline">
      <Network size={18} />
      <div>
        <strong>Interfaces</strong>
        <span>{rates.length} latest interface rate bucket{rates.length === 1 ? "" : "s"}</span>
        <div className="networkInterfaceList">
          {rates
            .slice()
            .sort((left, right) =>
              left.interface.localeCompare(right.interface),
            )
            .map((rate) => (
              <div className="networkInterfaceRow telemetryInterfaceRow" key={rate.interface}>
                <TelemetryStack
                  detail={rateBucketDetail(rate)}
                  main={rate.interface}
                />
                <TelemetryStack
                  detail={rateByteDetail(rate)}
                  main={`RX ${formatBitsPerSecond(rate.rx_bps_avg)} / TX ${formatBitsPerSecond(rate.tx_bps_avg)}`}
                />
                <TelemetryStack
                  detail={`updated ${formatCompactTime(rate.updated_at)}`}
                  main={`${rate.sample_count} sample${rate.sample_count === 1 ? "" : "s"} / ${formatDuration(rate.bucket_secs)}`}
                />
                <TelemetryStack
                  detail={`${formatDuration(rate.bucket_secs)} interval`}
                  main={rateDeltaDetail(rate)}
                />
              </div>
            ))}
        </div>
      </div>
    </div>
  );
}

function TunnelList({ tunnels }: { tunnels: TelemetryTunnelRecord[] }) {
  if (tunnels.length === 0) {
    return (
      <DetailLine
        icon={<Network size={18} />}
        label="Runtime tunnels"
        value="No tunnel reports"
      />
    );
  }
  return (
    <div className="timeline telemetryTimeline">
      <Network size={18} />
      <div>
        <strong>Runtime tunnels</strong>
        <span>{tunnels.length} latest tunnel report{tunnels.length === 1 ? "" : "s"}</span>
        <div className="networkInterfaceList">
          {tunnels
            .slice()
            .sort((left, right) =>
              left.interface.localeCompare(right.interface),
            )
            .map((tunnel) => (
              <div className={`networkInterfaceRow telemetryTunnelRow ${tunnelRowClass(tunnel)}`} key={tunnel.interface}>
                <TelemetryStack
                  detail={`${tunnel.kind}; observed ${formatCompactTime(tunnel.observed_at)}`}
                  main={tunnel.interface}
                />
                <TelemetryStack
                  detail={formatTunnelPolicy(tunnel)}
                  main={formatTunnelRuntime(tunnel)}
                />
                <TelemetryStack
                  detail={formatTunnelLatencyDetail(tunnel)}
                  main={formatTunnelLatencyMain(tunnel)}
                  tone={latencyTone(tunnel.latency_status)}
                />
                <TelemetryStack
                  detail={formatTunnelOspfDetail(tunnel)}
                  main={formatTunnelOspfMain(tunnel)}
                  tone={ospfTone(tunnel.auto_ospf_status)}
                />
              </div>
            ))}
        </div>
      </div>
    </div>
  );
}

function TelemetryStack({
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

function tunnelRowClass(tunnel: TelemetryTunnelRecord): string {
  if (tunnel.latency_status === "down" || tunnel.auto_ospf_status === "failed") {
    return "telemetryRowCritical";
  }
  if (tunnel.latency_status === "missed" || tunnel.auto_ospf_status === "report_only") {
    return "telemetryRowWarn";
  }
  return "";
}

function formatTunnelRuntime(tunnel: TelemetryTunnelRecord): string {
  const state = tunnel.operstate ?? "state unknown";
  const mtu = typeof tunnel.mtu === "number" ? `mtu ${tunnel.mtu}` : "mtu n/a";
  return `${state}; ${mtu}`;
}

function formatTunnelLatencyMain(tunnel: TelemetryTunnelRecord): string {
  const status = latencyStatusLabel(tunnel.latency_status);
  const metric = typeof tunnel.latency_avg_ms === "number" ? ` / ${tunnel.latency_avg_ms.toFixed(1)} ms` : "";
  const loss = typeof tunnel.packet_loss_ratio === "number" ? ` / ${(tunnel.packet_loss_ratio * 100).toFixed(1)}% loss` : "";
  return `Latency ${status}${metric}${loss}`;
}

function formatTunnelLatencyDetail(tunnel: TelemetryTunnelRecord): string {
  const checked = typeof tunnel.latency_checked_unix === "number"
    ? formatCompactTime(new Date(tunnel.latency_checked_unix * 1000).toISOString())
    : "not checked";
  const windows = [
    typeof tunnel.latency_healthy_windows === "number" ? `ok ${tunnel.latency_healthy_windows}` : "",
    typeof tunnel.latency_missed_windows === "number" ? `miss ${tunnel.latency_missed_windows}` : "",
  ].filter(Boolean).join(", ");
  return [
    addressFamilyLabel(tunnel.latency_primary_family),
    tunnel.latency_target ?? "target n/a",
    `checked ${checked}`,
    windows || "windows n/a",
    telemetryReasonLabel(tunnel.latency_reason),
  ].filter(Boolean).join("; ");
}

function formatTunnelOspfMain(tunnel: TelemetryTunnelRecord): string {
  const status = ospfStatusLabel(tunnel.auto_ospf_status, tunnel.auto_ospf_enabled);
  const cost = tunnel.auto_ospf_current_cost || tunnel.auto_ospf_recommended_cost
    ? ` ${tunnel.auto_ospf_current_cost ?? "?"}->${tunnel.auto_ospf_recommended_cost ?? "?"}`
    : "";
  return `OSPF ${status}${cost}`;
}

function formatTunnelOspfDetail(tunnel: TelemetryTunnelRecord): string {
  const enabled = tunnel.auto_ospf_enabled ? "enabled" : "disabled";
  const updated = typeof tunnel.auto_ospf_updated_unix === "number"
    ? `updated ${formatCompactTime(new Date(tunnel.auto_ospf_updated_unix * 1000).toISOString())}`
    : "no update";
  return [enabled, updated, telemetryReasonLabel(tunnel.auto_ospf_reason)].filter(Boolean).join("; ");
}

function latencyTone(status: string | null | undefined): "critical" | "neutral" | "ok" | "warn" {
  if (status === "down") {
    return "critical";
  }
  if (status === "missed" || status === "unconfigured" || status === "disabled") {
    return "warn";
  }
  if (status === "healthy") {
    return "ok";
  }
  return "neutral";
}

function ospfTone(status: string | null | undefined): "critical" | "neutral" | "ok" | "warn" {
  if (status === "failed") {
    return "critical";
  }
  if (status === "report_only" || status === "stabilizing" || status === "monitoring_only") {
    return "warn";
  }
  if (status === "stable" || status === "updated" || status === "disabled") {
    return "ok";
  }
  return "neutral";
}

function formatTunnelPolicy(tunnel: TelemetryTunnelRecord) {
  const adapterHealth = formatAdapterHealth(tunnel);
  const traffic = formatTunnelTraffic(tunnel);
  if (tunnel.plan_correlation === "matched_saved_plan") {
    const manager = runtimeManagerLabel(tunnel.plan_runtime_manager ?? tunnel.ownership_mode);
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
  return `${planCorrelationLabel(tunnel.plan_correlation)} ${mutationPolicyLabel(tunnel.mutation_policy)}${adapterHealth}${traffic}`;
}

function formatAdapterHealth(tunnel: TelemetryTunnelRecord) {
  const health = tunnel.adapter_health;
  if (!health) {
    return "";
  }
  if (health.success) {
    return " adapter healthy";
  }
  const reason = telemetryReasonLabel(health.reason) || readableAdapterStatus(health.status);
  return ` adapter ${reason}`;
}

function formatTunnelTraffic(tunnel: TelemetryTunnelRecord) {
  const source = tunnel.traffic_source;
  if (!source) {
    return "";
  }
  const status =
    tunnel.traffic_status && tunnel.traffic_status !== "ok"
      ? ` ${trafficStatusLabel(tunnel.traffic_status)}`
      : "";
  return ` traffic ${telemetrySourceLabel(source)}${status}`;
}

function readableAdapterStatus(status: string | null | undefined): string {
  return status ? status.replace(/[_-]+/g, " ") : "unknown";
}

function rateBucketDetail(rate: TelemetryNetworkRateRecord): string {
  return `bucket ${formatCompactTime(rate.bucket_start)}`;
}

function rateByteDetail(rate: TelemetryNetworkRateRecord): string {
  return `avg bytes RX ${formatBytes(rate.rx_bytes_avg)} / TX ${formatBytes(rate.tx_bytes_avg)}`;
}

function rateDeltaDetail(rate: TelemetryNetworkRateRecord): string {
  if (rate.rx_bytes_delta === 0 && rate.tx_bytes_delta === 0) {
    return "delta pending";
  }
  return `delta RX ${formatBytes(rate.rx_bytes_delta)} / TX ${formatBytes(rate.tx_bytes_delta)}`;
}

function formatDuration(seconds: number): string {
  if (seconds < 60) {
    return `${seconds}s`;
  }
  if (seconds < 3600) {
    return `${Math.round(seconds / 60)}m`;
  }
  return `${Math.round(seconds / 3600)}h`;
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
  value: ReactNode;
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
