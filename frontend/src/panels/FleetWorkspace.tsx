import {
  useCallback,
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
  DatabaseBackup,
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
import { agentDisplayState } from "../agentDisplayState";
import {
  buildBulkJobProgress,
  bulkOutcomeSummary,
  createJobTargetCount,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { DEFAULT_MAX_JOB_TIMEOUT_SECS } from "../jobMaxTimeout";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
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
import { fleetChartColors } from "../colorPalette";
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
  buildTagDisplayOrder,
  compareTagsByDisplayOrder,
  displayFleetTags as displayTags,
  isCountryTag,
  isProviderTag,
  sortTagsByDisplayOrder,
  type TagDisplayOrder,
} from "../tagDisplay";
import {
  buildPrivilegeAssertion,
  buildPrivilegeForJobOperation,
  canonicalDbPrivilegeIntent,
  type PrivilegeAssertion,
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
import {
  DEFAULT_UPDATE_VERSION_URL,
  type JobDispatchPresetInput,
} from "../jobDispatchPreset";
import type {
  ActiveView,
  AgentView,
  BulkTagMutationRequest,
  CreateJobRequest,
  CreateJobResponse,
  TemplateRuntimeConfigResponse,
  SourceTemplateAssignmentRecord,
  SourceStatusRecord,
  FleetAlertPolicyRecord,
  FleetAlertPolicyRequest,
  FleetAlertRecord,
  PolicyAlertRecord,
  PolicyDryRunRequest,
  PolicyDryRunResponse,
  PolicyRuleRecord,
  PolicyRuleRequest,
  FleetAlertNotificationChannelRecord,
  FleetAlertNotificationChannelRequest,
  FleetAlertNotificationDeliveryRecord,
  FleetAlertNotificationDispatchRequest,
  FleetAlertNotificationProcessRequest,
  FleetAlertStateRecord,
  FleetAlertStateRequest,
  FleetSummary,
  TrafficAccountingRecord,
  TrafficAccountingSelectorBreakdown,
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
  VpsRuleValueRecord,
} from "../types";

type FleetDetailTab =
  | "Overview"
  | "Telemetry"
  | "Traffic & Rules"
  | "Jobs"
  | "Network"
  | "Config";
type FleetSelectionStatsMode =
  | "telemetry"
  | "network"
  | "overview"
  | "capabilities";

type DeleteAgentConfirmationSnapshot = {
  clientId: string;
  displayName: string;
  status: string;
  privilegeAssertion: PrivilegeAssertion;
};

type AliasConfirmationSnapshot = {
  clientId: string;
  oldDisplayName: string;
  newDisplayName: string;
};

type AlertDeliveryQueueSnapshot =
  | {
      action: "dispatch";
      request: FleetAlertNotificationDispatchRequest;
      previewHash: string;
      reviewedRows: number;
    }
  | {
      action: "process";
      request: FleetAlertNotificationProcessRequest;
      previewHash: string;
      reviewedRows: number;
    };

type WebhookDeliveryQueueSnapshot =
  | {
      action: "dispatch";
      request: WebhookRuleDispatchRequest;
      previewHash: string;
      reviewedRows: number;
      eventLabel: string;
    }
  | {
      action: "process";
      request: WebhookRuleProcessRequest;
      previewHash: string;
      reviewedRows: number;
      eventLabel: string;
    };

const detailTabs: FleetDetailTab[] = [
  "Overview",
  "Telemetry",
  "Traffic & Rules",
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
const CONFIG_BULK_SELECTOR_STORAGE_KEY =
  "vpsman.config.bulk.selectorExpression";
const FILE_BROWSER_STATE_STORAGE_KEY = "vpsman.fileBrowser.state";
export function FleetWorkspace({
  activeSubpage,
  agents,
  apiError,
  fleetAlerts,
  fleetAlertStates,
  fleetAlertPolicies,
  policyAlerts,
  trafficAccounting,
  vpsRuleValues,
  fleetAlertNotificationChannels,
  fleetAlertNotifications,
  webhookRules,
  webhookRuleDeliveries,
  lastLiveEvent,
  sourceTemplateAssignments,
  sourceStatus,
  onCreateJob,
  onBulkMutateTags,
  onNavigatePanel,
  onOpenJobDispatchPreset,
  onRenderTemplateRuntimeConfig,
  onDeleteFleetAlertNotificationChannel,
  onDeleteFleetAlertPolicy,
  onDeleteWebhookRule,
  onDispatchFleetAlertNotifications,
  onDispatchWebhookRules,
  onDryRunFleetAlertPolicy,
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
  policyAlerts: PolicyAlertRecord[];
  trafficAccounting: TrafficAccountingRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
  fleetAlertNotificationChannels: FleetAlertNotificationChannelRecord[];
  fleetAlertNotifications: FleetAlertNotificationDeliveryRecord[];
  webhookRules: WebhookRuleRecord[];
  webhookRuleDeliveries: WebhookRuleDeliveryRecord[];
  lastLiveEvent: string;
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceStatus: SourceStatusRecord[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onBulkMutateTags: (
    request: BulkTagMutationRequest,
  ) => Promise<TagMutationResponse>;
  onNavigatePanel?: (view: ActiveView, subpage: string) => void;
  onOpenJobDispatchPreset: (preset: JobDispatchPresetInput) => void;
  onRenderTemplateRuntimeConfig: (
    clientId: string,
  ) => Promise<TemplateRuntimeConfigResponse>;
  onDeleteFleetAlertNotificationChannel: (
    channelId: string,
    reviewedName: string,
  ) => Promise<void>;
  onDeleteFleetAlertPolicy: (
    policyId: string,
    reviewedName: string,
  ) => Promise<void>;
  onDeleteWebhookRule: (ruleId: string, reviewedName: string) => Promise<void>;
  onDispatchFleetAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onDispatchWebhookRules: (
    request: WebhookRuleDispatchRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onDryRunFleetAlertPolicy: (
    request: PolicyDryRunRequest,
  ) => Promise<PolicyDryRunResponse>;
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
    confirmed: boolean,
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
  const [deleteSnapshot, setDeleteSnapshot] =
    useState<DeleteAgentConfirmationSnapshot | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteReviewPending, setDeleteReviewPending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const deleteReviewTargetRef = useRef<string | null>(null);
  const deleteSnapshotRef = useRef<DeleteAgentConfirmationSnapshot | null>(
    null,
  );
  const deleteReviewPendingRef = useRef(false);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
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
  const trafficByClient = useMemo(
    () =>
      new Map(trafficAccounting.map((record) => [record.client_id, record])),
    [trafficAccounting],
  );
  const vpsRulesByClient = useMemo(() => {
    const map = new Map<string, VpsRuleValueRecord[]>();
    for (const rule of vpsRuleValues) {
      const rows = map.get(rule.client_id) ?? [];
      rows.push(rule);
      map.set(rule.client_id, rows);
    }
    return map;
  }, [vpsRuleValues]);
  const policyAlertsByClient = useMemo(() => {
    const map = new Map<string, PolicyAlertRecord[]>();
    for (const alert of policyAlerts) {
      const rows = map.get(alert.client_id) ?? [];
      rows.push(alert);
      map.set(alert.client_id, rows);
    }
    return map;
  }, [policyAlerts]);
  const tagDisplayOrder = useMemo(() => buildTagDisplayOrder(tags), [tags]);
  const fleetSubpageBase = activeSubpage.split(":")[0];
  const policyFilterClientId = activeSubpage.startsWith("policies:id:")
    ? decodeURIComponent(activeSubpage.slice("policies:id:".length))
    : null;
  const policyFocusId = activeSubpage.startsWith("policies:policy:")
    ? decodeURIComponent(activeSubpage.slice("policies:policy:".length))
    : null;
  const fleetSubpage = ["instances", "policies", "notifications"].includes(
    fleetSubpageBase,
  )
    ? fleetSubpageBase
    : "instances";

  const clearDeleteReview = useCallback(() => {
    deleteReviewTargetRef.current = null;
    invalidateReviewGeneration();
    setDeleteSnapshot(null);
    setDeleteReviewPending(false);
  }, [invalidateReviewGeneration]);

  useEffect(() => {
    deleteSnapshotRef.current = deleteSnapshot;
  }, [deleteSnapshot]);

  useEffect(() => {
    deleteReviewPendingRef.current = deleteReviewPending;
  }, [deleteReviewPending]);

  useEffect(() => {
    clearDeleteReview();
  }, [activeSubpage, selectedAgent?.id, clearDeleteReview]);

  const handleFleetSelectionChange = useCallback(
    (rows: AgentView[]) => {
      const reviewedClientId =
        deleteSnapshotRef.current?.clientId ?? deleteReviewTargetRef.current;
      if (!reviewedClientId && !deleteReviewPendingRef.current) {
        return;
      }
      if (rows.length !== 1 || rows[0]?.id !== reviewedClientId) {
        clearDeleteReview();
      }
    },
    [clearDeleteReview],
  );
  const fleetColumns = useMemo<ConsoleDataGridColumn<AgentView>[]>(
    () => [
      {
        id: "name",
        header: "VPS",
        size: 175,
        minSize: 150,
        sortValue: (agent) => formatVpsName(agent, vpsNameDisplayMode),
        searchValue: (agent) =>
          `${formatVpsName(agent, vpsNameDisplayMode)} ${agent.id} ${agent.status} ${agentDisplayState(agent).label} ${agent.registration_ip ?? ""} ${agent.last_ip ?? ""}`,
        cell: (agent) => (
          <span className="instance">
            <Server size={17} />
            <span>
              <strong>{formatVpsName(agent, vpsNameDisplayMode)}</strong>
            </span>
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        size: 135,
        minSize: 115,
        sortValue: (agent) => agentDisplayState(agent).label,
        searchValue: (agent) => agentDisplayState(agent).detail,
        cell: (agent) => {
          const displayState = agentDisplayState(agent);
          return (
            <span className="historyPrimary">
              <ConsoleStatusBadge tone={displayState.tone}>
                {displayState.label}
              </ConsoleStatusBadge>
              <small>{displayState.detail}</small>
            </span>
          );
        },
      },
      {
        id: "last_ip",
        header: "IP",
        size: 105,
        minSize: 90,
        sortValue: (agent) => agent.last_ip ?? "",
        searchValue: (agent) => agent.last_ip ?? "",
        cell: (agent) => (
          <span className="monoValue">{agent.last_ip ?? "unknown"}</span>
        ),
      },
      {
        id: "last_seen",
        header: "Last contact",
        size: 120,
        minSize: 100,
        sortValue: (agent) => normalizedLastSeenSort(agent.last_seen_at),
        searchValue: (agent) => formatLastSeenDetail(agent.last_seen_at),
        cell: (agent) => (
          <span className="historyPrimary">
            <strong>{formatLastSeen(agent.last_seen_at)}</strong>
            <small>{formatLastSeenDetail(agent.last_seen_at)}</small>
          </span>
        ),
      },
      {
        id: "tags",
        header: "Tags",
        size: 260,
        minSize: 170,
        sortValue: (agent) =>
          displayTags(
            agent.tags,
            tagDisplayOrder,
            preferences.fleet_tag_visibility_overrides,
          ).join(" "),
        searchValue: (agent) => agent.tags.join(" "),
        cell: (agent) => {
          const agentTags = displayTags(
            agent.tags,
            tagDisplayOrder,
            preferences.fleet_tag_visibility_overrides,
          );
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
        id: "traffic_now",
        header: "Traffic Now",
        size: 210,
        minSize: 160,
        sortValue: (agent) =>
          trafficByClient.get(agent.id)?.latest_total_bytes ?? -1,
        searchValue: (agent) =>
          trafficNowSummary(trafficByClient.get(agent.id)),
        cell: (agent) => trafficNowSummary(trafficByClient.get(agent.id)),
      },
      {
        id: "cycle_usage",
        header: "Cycle Usage",
        size: 210,
        minSize: 160,
        sortValue: (agent) =>
          trafficByClient.get(agent.id)?.cycle_percent ?? -1,
        searchValue: (agent) =>
          cycleUsageSummary(trafficByClient.get(agent.id)),
        cell: (agent) => cycleUsageSummary(trafficByClient.get(agent.id)),
      },
      {
        id: "traffic_state",
        header: "Traffic State",
        size: 130,
        minSize: 110,
        sortValue: (agent) =>
          trafficStateForClient(
            trafficByClient.get(agent.id),
            policyAlertsByClient.get(agent.id),
          ),
        searchValue: (agent) =>
          trafficStateForClient(
            trafficByClient.get(agent.id),
            policyAlertsByClient.get(agent.id),
          ),
        cell: (agent) => {
          const state = trafficStateForClient(
            trafficByClient.get(agent.id),
            policyAlertsByClient.get(agent.id),
          );
          return (
            <ConsoleStatusBadge tone={trafficStateTone(state)}>
              {state}
            </ConsoleStatusBadge>
          );
        },
      },
      {
        id: "quota",
        header: "Quota",
        size: 170,
        minSize: 130,
        searchValue: (agent) => quotaSummary(trafficByClient.get(agent.id)),
        cell: (agent) => quotaSummary(trafficByClient.get(agent.id)),
      },
      {
        id: "reset_day",
        header: "Reset Day",
        size: 125,
        minSize: 105,
        sortValue: (agent) => trafficByClient.get(agent.id)?.reset_day ?? 0,
        searchValue: (agent) => resetDaySummary(trafficByClient.get(agent.id)),
        cell: (agent) => resetDaySummary(trafficByClient.get(agent.id)),
      },
      {
        id: "selectors",
        header: "Selectors",
        size: 230,
        minSize: 160,
        searchValue: (agent) => selectorSummary(trafficByClient.get(agent.id)),
        cell: (agent) => (
          <span className="monoValue">
            {selectorSummary(trafficByClient.get(agent.id))}
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
        cell: (agent) => (
          <span className="monoValue">
            {agent.registration_ip ?? "unknown"}
          </span>
        ),
      },
      {
        id: "agent_runtime",
        header: "Agent",
        size: 110,
        minSize: 95,
        sortValue: (agent) => agent.internal_build_number ?? -1,
        searchValue: (agent) =>
          `${agent.internal_build_number ?? ""} ${agent.arch ?? ""} ${agent.capabilities.privilege_mode}`,
        cell: (agent) => (
          <span className="historyPrimary">
            <strong>
              {typeof agent.internal_build_number === "number"
                ? `Build ${agent.internal_build_number}`
                : "Unknown"}
            </strong>
            <small>
              {[agent.arch ?? "arch unknown", privilegeModeLabel(agent)]
                .filter(Boolean)
                .join(" · ")}
            </small>
          </span>
        ),
      },
      {
        id: "cpu_load",
        header: "CPU",
        size: 78,
        minSize: 68,
        sortValue: (agent) =>
          latestRollups.get(agent.id)?.cpu_load_1_avg ?? -1,
        searchValue: (agent) =>
          formatLoad(latestRollups.get(agent.id)?.cpu_load_1_avg),
        cell: (agent) => {
          const rollup = latestRollups.get(agent.id);
          return (
            <span className="historyPrimary">
              <strong>{formatLoadCompact(rollup?.cpu_load_1_avg)}</strong>
              <small>{formatRollupFreshness(rollup)}</small>
            </span>
          );
        },
      },
      {
        id: "memory_used",
        header: "Memory",
        size: 88,
        minSize: 76,
        sortValue: (agent) => memoryUsedRatio(latestRollups.get(agent.id)) ?? -1,
        searchValue: (agent) => formatMemoryUsed(latestRollups.get(agent.id)),
        cell: (agent) => {
          const rollup = latestRollups.get(agent.id);
          return (
            <span className="historyPrimary">
              <strong>{formatMemoryUsedCompact(rollup)}</strong>
              <small>{formatRollupFreshness(rollup)}</small>
            </span>
          );
        },
      },
      {
        id: "disk_free",
        header: "Disk",
        size: 88,
        minSize: 76,
        sortValue: (agent) => diskFreeRatio(latestRollups.get(agent.id)) ?? -1,
        searchValue: (agent) => formatDiskFree(latestRollups.get(agent.id)),
        cell: (agent) => {
          const rollup = latestRollups.get(agent.id);
          return (
            <span className="historyPrimary">
              <strong>{formatDiskFreeCompact(rollup)}</strong>
              <small>{formatRollupFreshness(rollup)}</small>
            </span>
          );
        },
      },
      {
        id: "active_policy_alerts",
        header: "Alerts",
        size: 82,
        minSize: 72,
        sortValue: (agent) => policyAlertsByClient.get(agent.id)?.length ?? 0,
        searchValue: (agent) =>
          activePolicyAlertSummary(policyAlertsByClient.get(agent.id)),
        cell: (agent) =>
          activePolicyAlertSummary(policyAlertsByClient.get(agent.id)),
      },
      {
        id: "open_instance",
        header: "Action",
        size: 76,
        minSize: 68,
        enableHiding: false,
        cell: (agent) => (
          <button
            aria-label={`Open ${formatVpsName(agent, vpsNameDisplayMode)} detail`}
            className="secondaryAction compactAction"
            onClick={(event) => {
              event.stopPropagation();
              openSingleReleaseWorkflow([agent], "Fleet", "instance_detail");
            }}
            type="button"
          >
            Open
          </button>
        ),
      },
    ],
    [
      latestRollups,
      preferences.fleet_tag_visibility_overrides,
      preferences.show_country_flags,
      policyAlertsByClient,
      tagDisplayOrder,
      trafficByClient,
      vpsNameDisplayMode,
    ],
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
      target_client_ids: targetIds,
      tag: normalizedTag,
    });
  }

  function openSelectorWorkflow(
    rows: AgentView[],
    view: ActiveView,
    subpage: string,
    storageKey: string,
  ) {
    clearDeleteReview();
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
    clearDeleteReview();
    if (rows.length !== 1) {
      return;
    }
    seedSingleFileBrowser(rows[0]);
    onNavigatePanel?.("Remote Operations", "files");
  }

  function openSingleReleaseWorkflow(
    rows: AgentView[],
    view: ActiveView,
    subpage: string,
  ) {
    clearDeleteReview();
    if (rows.length !== 1) {
      return;
    }
    onSelectAgent(rows[0].id);
    onNavigatePanel?.(view, subpage);
  }

  function openUpdateCheckWorkflow(rows: AgentView[]) {
    clearDeleteReview();
    onOpenJobDispatchPreset({
      mode: "agent_update_check",
      selectorExpression: selectorExpressionForClientIds(
        rows.map((agent) => agent.id),
      ),
      maxTimeoutSecs: 300,
      updateCheckActivate: true,
      updateCheckRestartAgent: true,
      updateCheckVersionUrl: DEFAULT_UPDATE_VERSION_URL,
    });
  }

  async function requestDeleteAgent(rows: AgentView[]) {
    clearDeleteReview();
    if (rows.length !== 1) {
      return;
    }
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      setDeleteError("Privilege unlock is required");
      return;
    }
    const target = rows[0];
    deleteReviewTargetRef.current = target.id;
    const reviewGeneration = captureReviewGeneration();
    setDeleteReviewPending(true);
    try {
      await waitForReviewRender();
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "agent.delete",
          confirmed: true,
          resolvedTargets: [target.id],
          target: target.id,
        }),
        privilegeMaterial,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setDeleteError(null);
      setDeleteSnapshot({
        clientId: target.id,
        displayName: formatVpsName(target, vpsNameDisplayMode),
        status: target.status,
        privilegeAssertion,
      });
    } catch (error) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setDeleteError(error instanceof Error ? error.message : String(error));
    } finally {
      setDeleteReviewPending(false);
    }
  }

  async function confirmDeleteAgent() {
    if (!deleteSnapshot) {
      return;
    }
    await runPanelAction(setDeletePending, setDeleteError, async () => {
      await onDeleteAgent(deleteSnapshot.clientId, {
        confirmed: true,
        privilege_assertion: deleteSnapshot.privilegeAssertion,
        reason: "Deleted from fleet inventory selection action",
      });
      clearDeleteReview();
      onSelectAgent(null);
    });
  }

  const fleetInstanceActions: ConsoleDataGridAction<AgentView>[] = [
    {
      label: "Open detail",
      disabled: (rows) => rows.length !== 1,
      icon: <Eye size={15} />,
      onSelect: (rows) =>
        openSingleReleaseWorkflow(rows, "Fleet", "instance_detail"),
    },
    {
      label: "Open terminal",
      disabled: (rows) => rows.length !== 1,
      icon: <TerminalSquare size={15} />,
      separatorBefore: true,
      onSelect: (rows) =>
        openSingleReleaseWorkflow(rows, "Remote Operations", "terminal"),
    },
    {
      label: "Open files",
      disabled: (rows) => rows.length !== 1,
      icon: <FolderOpen size={15} />,
      onSelect: openFileBrowserWorkflow,
    },
    {
      label: "Open processes",
      disabled: (rows) => rows.length !== 1,
      icon: <Activity size={15} />,
      onSelect: (rows) =>
        openSingleReleaseWorkflow(rows, "Remote Operations", "processes"),
    },
    {
      label: "Open backups",
      disabled: (rows) => rows.length !== 1,
      icon: <DatabaseBackup size={15} />,
      onSelect: (rows) =>
        openSingleReleaseWorkflow(rows, "Backups", "requests"),
    },
    {
      label: "Open network",
      disabled: (rows) => rows.length !== 1,
      icon: <Network size={15} />,
      onSelect: (rows) => openSingleReleaseWorkflow(rows, "Network", "graph"),
    },
    {
      label: "Open bulk files",
      separatorBefore: true,
      onSelect: (rows) =>
        openSelectorWorkflow(
          rows,
          "Remote Operations",
          "bulk_files",
          MULTI_FILE_SELECTOR_STORAGE_KEY,
        ),
    },
    {
      label: "Open bulk groups",
      onSelect: (rows) =>
        openSelectorWorkflow(
          rows,
          "Fleet",
          "group_bulk",
          TAG_BULK_SELECTOR_STORAGE_KEY,
        ),
    },
    {
      label: "Copy client IDs",
      separatorBefore: true,
      onSelect: (rows) =>
        void copyText(rows.map((agent) => agent.id).join("\n")),
    },
    {
      label: "Copy selector",
      onSelect: (rows) =>
        void copyText(
          selectorExpressionForClientIds(rows.map((agent) => agent.id)),
        ),
    },
    {
      label: "Copy tag query",
      onSelect: (rows) =>
        void copyText(
          Array.from(new Set(rows.flatMap((agent) => agent.tags)))
            .sort((left, right) =>
              compareTagsByDisplayOrder(left, right, tagDisplayOrder),
            )
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
      separatorBefore: true,
      tone: "danger",
    },
  ];

  return (
    <section
      className={
        fleetSubpage === "instances"
          ? "workspace singleColumn fleetInstancesWorkspace"
          : "workspace singleColumn"
      }
    >
      {fleetSubpage === "instances" && (
        <FleetInstancesPanel
          actions={fleetInstanceActions}
          agents={agents}
          apiError={apiError}
          columns={fleetColumns}
          deleteError={deleteError}
          deletePending={deletePending}
          deleteSnapshot={deleteSnapshot}
          onCancelDelete={() => {
            setDeleteError(null);
            clearDeleteReview();
          }}
          onConfirmDelete={() => void confirmDeleteAgent()}
          onOpenMonitor={
            onNavigatePanel
              ? () => onNavigatePanel("Fleet", "monitor")
              : undefined
          }
          onOpenRow={(agent) =>
            openSingleReleaseWorkflow([agent], "Fleet", "instance_detail")
          }
          onSelectionChange={handleFleetSelectionChange}
          renderSelectionPanel={(rows) => (
            <FleetSelectionPanel
              agents={rows}
              allTags={tags}
              latestNetworkRates={latestNetworkRates}
              latestRollups={latestRollups}
              mutateTagsForAgents={mutateTagsForAgents}
              onOpenFileBrowser={openFileBrowserWorkflow}
              onOpenUpdateCheck={openUpdateCheckWorkflow}
              onOpenSelectorWorkflow={openSelectorWorkflow}
              selectionStatsMode={selectionStatsMode}
              setSelectionStatsMode={setSelectionStatsMode}
              tagDisplayOrder={tagDisplayOrder}
              tagVisibilityOverrides={
                preferences.fleet_tag_visibility_overrides
              }
              vpsNameDisplayMode={vpsNameDisplayMode}
            />
          )}
          scopeActive={scopeActive}
          summary={summary}
          wsState={wsState}
        />
      )}

      {fleetSubpage === "policies" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Alert policies</h2>
              <span>{`${fleetAlertPolicies.length} policy groups`}</span>
            </div>
            <span className="sectionContext">
              Selector expressions match VPSs; rule rows issue first-reach
              alerts
            </span>
          </div>
          <ConsoleFreshnessBanner error={apiError} />
          <FleetAlertPolicyManager
            agents={targetAgents}
            onDryRun={onDryRunFleetAlertPolicy}
            onDelete={onDeleteFleetAlertPolicy}
            onUpsert={onUpsertFleetAlertPolicy}
            policyAlerts={policyAlerts}
            policyFocusId={policyFocusId}
            policyFilterClientId={policyFilterClientId}
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

function FleetInstancesPanel({
  actions,
  agents,
  apiError,
  columns,
  deleteError,
  deletePending,
  deleteSnapshot,
  onCancelDelete,
  onConfirmDelete,
  onOpenMonitor,
  onOpenRow,
  onSelectionChange,
  renderSelectionPanel,
  scopeActive,
  summary,
  wsState,
}: {
  actions: ConsoleDataGridAction<AgentView>[];
  agents: AgentView[];
  apiError: string | null;
  columns: ConsoleDataGridColumn<AgentView>[];
  deleteError: string | null;
  deletePending: boolean;
  deleteSnapshot: DeleteAgentConfirmationSnapshot | null;
  onCancelDelete: () => void;
  onConfirmDelete: () => void;
  onOpenMonitor?: () => void;
  onOpenRow: (agent: AgentView) => void;
  onSelectionChange: (rows: AgentView[]) => void;
  renderSelectionPanel: (rows: AgentView[]) => ReactNode;
  scopeActive: boolean;
  summary: FleetSummary;
  wsState: string;
}) {
  return (
    <div className="fleetPanel fleetInstancesPanel">
      <div className="sectionHeader fleetInstancesHeader">
        <div>
          <h2>VPS instances</h2>
          <span>
            {apiError ? "API unavailable" : "Live control-plane inventory"}
          </span>
        </div>
        <span className="sectionContext">
          {summary.online} online / {summary.total} total ·{" "}
          {formatConsoleStreamState(wsState)}
        </span>
      </div>

      <ConsoleDataGrid
        actions={actions}
        columns={columns}
        defaultColumnVisibility={{
          country: false,
          cycle_usage: false,
          provider: false,
          quota: false,
          registration_ip: false,
          reset_day: false,
          selectors: false,
          tags: false,
          traffic_now: false,
          traffic_state: false,
        }}
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
        getRowId={(agent) => agent.id}
        itemLabel="instances"
        onOpenRow={onOpenRow}
        onSelectionChange={onSelectionChange}
        renderSelectionPanel={renderSelectionPanel}
        rows={agents}
        storageKey="vpsman.grid.fleet.instances.v2"
        title="VPS instance records"
        toolbarActions={
          <div
            aria-label="Fleet instance view mode"
            className="segmented fleetViewSwitch"
            role="group"
          >
            <button aria-pressed={true} className="selected" type="button">
              Table
            </button>
            <button
              disabled={!onOpenMonitor}
              onClick={() => onOpenMonitor?.()}
              type="button"
            >
              Cards
            </button>
          </div>
        }
      />
      <ConfirmationPrompt
        confirmLabel="Delete VPS"
        detail="This deactivates VPS access immediately and permanently removes it from inventory, selectors, dashboard, tags, topology, and future bulk targeting. Historical jobs and audit records remain."
        error={deleteError}
        items={
          deleteSnapshot
            ? [
                {
                  label: "VPS",
                  value: deleteSnapshot.displayName,
                },
                { label: "Client ID", value: deleteSnapshot.clientId },
                { label: "Status", value: deleteSnapshot.status },
              ]
            : []
        }
        onCancel={onCancelDelete}
        onConfirm={onConfirmDelete}
        open={Boolean(deleteSnapshot)}
        pending={deletePending}
        title="Delete VPS from panel"
        tone="danger"
      />
    </div>
  );
}

function FleetInstanceDetail({
  agent,
  sourceTemplateAssignments,
  sourceStatus,
  lastLiveEvent,
  policyAlerts,
  policies,
  requestedTab,
  onRequestedTabConsumed,
  latestNetworkRates,
  latestRollup,
  latestTunnels,
  mutateTagsForAgents,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onNavigatePanel,
  onRenderTemplateRuntimeConfig,
  onUpdateAgentAlias,
  privilegeMaterial,
  showCountryFlags,
  summary,
  tagDisplayOrder,
  tagVisibilityOverrides,
  telemetryNetworkRates,
  telemetryRollups,
  trafficAccounting,
  vpsRuleValues,
  vpsNameDisplayMode,
  wsState,
}: {
  agent: AgentView;
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceStatus: SourceStatusRecord[];
  lastLiveEvent: string;
  policyAlerts: PolicyAlertRecord[];
  policies: FleetAlertPolicyRecord[];
  requestedTab: FleetDetailTab | null;
  onRequestedTabConsumed: () => void;
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
  onNavigatePanel?: (view: ActiveView, subpage: string) => void;
  onRenderTemplateRuntimeConfig: (
    clientId: string,
  ) => Promise<TemplateRuntimeConfigResponse>;
  onUpdateAgentAlias: (
    clientId: string,
    displayName: string,
    confirmed: boolean,
  ) => Promise<AgentView>;
  privilegeMaterial: PrivilegeMaterial | null;
  showCountryFlags: boolean;
  summary: FleetSummary;
  tagDisplayOrder: TagDisplayOrder;
  tagVisibilityOverrides: Record<string, boolean>;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  trafficAccounting: TrafficAccountingRecord | null;
  vpsRuleValues: VpsRuleValueRecord[];
  vpsNameDisplayMode: VpsNameDisplayMode;
  wsState: string;
}) {
  const [activeDetailTab, setActiveDetailTab] =
    useState<FleetDetailTab>("Overview");
  const [aliasDraft, setAliasDraft] = useState(agent.display_name ?? "");
  const [aliasPending, setAliasPending] = useState(false);
  const [aliasError, setAliasError] = useState<string | null>(null);
  const [aliasSnapshot, setAliasSnapshot] =
    useState<AliasConfirmationSnapshot | null>(null);
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
    useState<TemplateRuntimeConfigResponse | null>(null);
  const country = countryFromTags(agent.tags);
  const provider = providerFromTags(agent.tags);
  const displayOnlyTags = displayTags(
    agent.tags,
    tagDisplayOrder,
    tagVisibilityOverrides,
  );
  const isNetworkManaged = agent.tags.some((tag) =>
    ["bgp", "bird2", "ospf", "tunnel"].includes(tag.toLowerCase()),
  );
  const agentLabel = formatVpsName(agent, vpsNameDisplayMode);
  const displayState = agentDisplayState(agent);
  const configPreviewSummary = configPreview
    ? `${configPreview.assignments.length} assignments · ${configPreview.unsupported_domains.length} unsupported domains`
    : "Load redacted runtime config view for this VPS.";

  useEffect(() => {
    if (requestedTab) {
      setActiveDetailTab(requestedTab);
      onRequestedTabConsumed();
    }
  }, [onRequestedTabConsumed, requestedTab]);

  useEffect(() => {
    setAliasDraft(agent.display_name ?? "");
    setAliasError(null);
    setAliasSnapshot(null);
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

  function submitAlias(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const displayName = aliasDraft.trim();
    if (!displayName) {
      setAliasError("Alias is required");
      return;
    }
    if (displayName === agent.display_name.trim()) {
      setAliasSnapshot(null);
      return;
    }
    setAliasError(null);
    setAliasSnapshot({
      clientId: agent.id,
      oldDisplayName: agent.display_name,
      newDisplayName: displayName,
    });
  }

  async function confirmAliasUpdate() {
    if (!aliasSnapshot) {
      return;
    }
    setAliasPending(true);
    setAliasError(null);
    try {
      await onUpdateAgentAlias(
        aliasSnapshot.clientId,
        aliasSnapshot.newDisplayName,
        true,
      );
      setAliasSnapshot(null);
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
        maxTimeoutSecs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
      });
      setInterfacePayloadHash(builtPrivilege.payloadHashHex);
      setInterfaceSnapshot(null);
      setInterfaceProgress(
        buildBulkJobProgress({
          jobId: "",
          targetCount: 1,
          targetRecords: [],
          targets: [agent],
          maxTimeoutSecs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        }),
      );
      const job = await onCreateJob({
        argv: [],
        selector_expression: selectorExpression,
        target_client_ids: [agent.id],
        command: "network_interfaces",
        confirmed: false,
        destructive: false,
        job_id: crypto.randomUUID(),
        operation,
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: builtPrivilege.privilegeAssertion,
        max_timeout_secs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
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
          maxTimeoutSecs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        },
      );
      setInterfaceProgress(progress.progress);
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
      setConfigPreview(await onRenderTemplateRuntimeConfig(agent.id));
    });
  }

  return (
    <div className="fleetNodeDetailCard">
      <div className="fleetNodeDetailHeader">
        <div>
          <h3>{agentLabel}</h3>
          <span className="monoValue">{agent.id}</span>
        </div>
        <span className="sectionContext">
          {formatConsoleStreamState(wsState)}
        </span>
      </div>
      <div className="fleetNodeDetailControls">
        <form className="aliasEditor" onSubmit={submitAlias}>
          <label>
            <span>Display name</span>
            <input
              aria-label="VPS display name"
              onChange={(event) => {
                setAliasDraft(event.target.value);
                setAliasSnapshot(null);
              }}
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
        <ConfirmationPrompt
          confirmLabel="Rename VPS"
          detail="Renames this visible VPS record exactly as reviewed. Display names must be unique across visible VPS records."
          error={aliasError}
          items={
            aliasSnapshot
              ? [
                  { label: "Client ID", value: aliasSnapshot.clientId },
                  {
                    label: "Current name",
                    value: aliasSnapshot.oldDisplayName,
                  },
                  { label: "New name", value: aliasSnapshot.newDisplayName },
                ]
              : []
          }
          onCancel={() => {
            setAliasError(null);
            setAliasSnapshot(null);
          }}
          onConfirm={() => void confirmAliasUpdate()}
          open={Boolean(aliasSnapshot)}
          pending={aliasPending}
          title="Confirm VPS rename"
        />
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
          sortTagsByDisplayOrder(agent.tags, tagDisplayOrder).map((tag) => (
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
              value={displayState.label}
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
              icon={<AlertTriangle size={18} />}
              label="Contact evidence"
              value={displayState.detail}
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
              label="Console stream"
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
        {activeDetailTab === "Traffic & Rules" && (
          <TrafficRulesDetail
            agent={agent}
            policyAlerts={policyAlerts}
            policies={policies}
            onNavigatePanel={onNavigatePanel}
            trafficAccounting={trafficAccounting}
            vpsRuleValues={vpsRuleValues}
          />
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
            <SourceTemplateConfigList
              assignments={sourceTemplateAssignments}
              statuses={sourceStatus}
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

function SourceTemplateConfigList({
  assignments,
  statuses,
}: {
  assignments: SourceTemplateAssignmentRecord[];
  statuses: SourceStatusRecord[];
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
          <span>Assigned template domains</span>
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
          key={`${assignment.client_id}-${assignment.domain}-${assignment.template_id}`}
        >
          <Boxes size={18} />
          <div>
            <span>{assignment.domain}</span>
            <strong>
              {assignment.template_name} · {assignment.template_scope}
            </strong>
          </div>
        </div>
      ))}
      {assignments.length > 8 && (
        <small className="mutedText">
          +{assignments.length - 8} more template assignment
          {assignments.length - 8 === 1 ? "" : "s"}
        </small>
      )}
      <div className="detailLine">
        <Activity size={18} />
        <div>
          <span>Runtime config sources</span>
          <strong>{formatSourceStatusSummary(statusRows)}</strong>
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
              {row.status} · {row.template_name} · {row.source_kind}
            </strong>
            <small>
              {row.status_reason || formatSourceTemplateEvidence(row)}
            </small>
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

function formatSourceStatusSummary(rows: SourceStatusRecord[]) {
  if (rows.length === 0) {
    return "No runtime config source status loaded";
  }
  const ok = rows.filter((row) => row.status.toLowerCase() === "ok").length;
  const degraded = rows.filter(
    (row) => row.status.toLowerCase() !== "ok",
  ).length;
  return `${rows.length} source${rows.length === 1 ? "" : "s"} · ${ok} ok${degraded > 0 ? ` · ${degraded} needs review` : ""}`;
}

function formatSourceTemplateEvidence(row: SourceStatusRecord) {
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
  preview: TemplateRuntimeConfigResponse | null;
  summary: string;
}) {
  return (
    <div className="timeline configPreviewBlock">
      <FileCog size={18} />
      <div>
        <strong>Rendered runtime config</strong>
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

function TrafficRulesDetail({
  agent,
  onNavigatePanel,
  policyAlerts,
  policies,
  trafficAccounting,
  vpsRuleValues,
}: {
  agent: AgentView;
  onNavigatePanel?: (view: ActiveView, subpage: string) => void;
  policyAlerts: PolicyAlertRecord[];
  policies: FleetAlertPolicyRecord[];
  trafficAccounting: TrafficAccountingRecord | null;
  vpsRuleValues: VpsRuleValueRecord[];
}) {
  const policyById = new Map(policies.map((policy) => [policy.id, policy]));
  const alertByRule = new Map(
    policyAlerts.map((alert) => [alert.policy_rule_id, alert]),
  );
  const matchedPolicyIds = new Set(
    policyAlerts.map((alert) => alert.policy_group_id),
  );
  const matchedPolicyRows = policies
    .filter((policy) => matchedPolicyIds.has(policy.id))
    .flatMap((policy) =>
      policy.rules.map((rule) => ({
        alert: alertByRule.get(rule.id) ?? null,
        policy,
        rule,
      })),
    );
  const trafficRows = trafficAccounting?.selector_breakdown ?? [];
  const selectedPolicyId = matchedPolicyRows[0]?.policy.id;
  const trafficColumns = useMemo<
    ConsoleDataGridColumn<TrafficAccountingSelectorBreakdown>[]
  >(
    () => [
      {
        id: "source",
        header: "Source",
        size: 90,
        minSize: 80,
        sortValue: (row) => row.source,
        searchValue: (row) => row.source,
        cell: (row) => row.source,
      },
      {
        id: "interface",
        header: "Interface",
        size: 130,
        minSize: 110,
        sortValue: (row) => row.interface,
        searchValue: (row) => row.interface,
        cell: (row) => <span className="monoValue">{row.interface}</span>,
      },
      {
        id: "direction",
        header: "Direction",
        size: 100,
        minSize: 90,
        sortValue: (row) => row.direction,
        searchValue: (row) => row.direction,
        cell: (row) => row.direction,
      },
      {
        id: "latest_rx",
        header: "Latest RX",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.latest_rx_bytes,
        cell: (row) => formatBytes(row.latest_rx_bytes),
      },
      {
        id: "latest_tx",
        header: "Latest TX",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.latest_tx_bytes,
        cell: (row) => formatBytes(row.latest_tx_bytes),
      },
      {
        id: "cycle_rx",
        header: "Cycle RX",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.cycle_rx_bytes,
        cell: (row) => formatBytes(row.cycle_rx_bytes),
      },
      {
        id: "cycle_tx",
        header: "Cycle TX",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.cycle_tx_bytes,
        cell: (row) => formatBytes(row.cycle_tx_bytes),
      },
      {
        id: "cycle_total",
        header: "Cycle Total",
        size: 130,
        minSize: 110,
        sortValue: (row) => row.cycle_total_bytes,
        cell: (row) => formatBytes(row.cycle_total_bytes),
      },
      {
        id: "sample_age",
        header: "Sample age",
        size: 110,
        minSize: 95,
        sortValue: (row) => row.sample_age_secs ?? Number.MAX_SAFE_INTEGER,
        cell: (row) => formatSampleAge(row.sample_age_secs),
      },
      {
        id: "state",
        header: "State",
        size: 105,
        minSize: 90,
        sortValue: (row) => row.state,
        searchValue: (row) =>
          `${row.state} ${row.incomplete_reasons.join(" ")}`,
        cell: (row) => (
          <ConsoleStatusBadge tone={row.state === "ok" ? "ok" : "warning"}>
            {row.state}
          </ConsoleStatusBadge>
        ),
      },
    ],
    [],
  );
  const vpsRuleColumns = useMemo<ConsoleDataGridColumn<VpsRuleValueRecord>[]>(
    () => [
      {
        id: "key",
        header: "Key",
        size: 210,
        minSize: 160,
        sortValue: (row) => row.key,
        searchValue: (row) => row.key,
        cell: (row) => <span className="monoValue">{row.key}</span>,
      },
      {
        id: "raw",
        header: "Raw value",
        size: 150,
        minSize: 110,
        searchValue: (row) => row.value_raw,
        cell: (row) => row.value_raw || "unset",
      },
      {
        id: "parsed",
        header: "Parsed value",
        size: 240,
        minSize: 160,
        searchValue: (row) => row.parsed_display,
        cell: (row) => row.parsed_display || "unset",
      },
      {
        id: "state",
        header: "State",
        size: 105,
        minSize: 90,
        searchValue: (row) => row.state,
        sortValue: (row) => row.state,
        cell: (row) => (
          <ConsoleStatusBadge tone={row.state === "ok" ? "ok" : "warning"}>
            {row.state}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "updated_by",
        header: "Updated by",
        size: 140,
        minSize: 110,
        searchValue: (row) => row.updated_by ?? "",
        cell: (row) => row.updated_by ?? "unknown",
      },
      {
        id: "updated",
        header: "Updated at",
        size: 155,
        minSize: 120,
        sortValue: (row) => row.updated_at,
        cell: (row) => formatCompactTime(row.updated_at),
      },
    ],
    [],
  );
  const policyColumns = useMemo<
    ConsoleDataGridColumn<(typeof matchedPolicyRows)[number]>[]
  >(
    () => [
      {
        id: "policy",
        header: "Policy",
        size: 170,
        minSize: 140,
        sortValue: (row) => row.policy.name,
        searchValue: (row) => row.policy.name,
        cell: (row) => row.policy.name,
      },
      {
        id: "rule",
        header: "Rule",
        size: 210,
        minSize: 160,
        searchValue: (row) =>
          `${row.rule.name} ${row.rule.condition_expression}`,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.rule.name}</strong>
            <small className="monoValue">{row.rule.condition_expression}</small>
          </span>
        ),
      },
      {
        id: "severity",
        header: "Severity",
        size: 100,
        minSize: 90,
        sortValue: (row) => row.rule.severity,
        cell: (row) => (
          <ConsoleStatusBadge
            tone={
              row.rule.severity === "critical"
                ? "critical"
                : row.rule.severity === "warning"
                  ? "warning"
                  : "info"
            }
          >
            {row.rule.severity}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "state",
        header: "Current state",
        size: 130,
        minSize: 110,
        sortValue: (row) => (row.alert ? 1 : 0),
        cell: (row) => (row.alert ? "true" : "false"),
      },
      {
        id: "window",
        header: "Window",
        size: 105,
        minSize: 90,
        sortValue: (row) => row.rule.window_secs,
        cell: (row) => formatPolicyWindow(row.rule.window_secs),
      },
      {
        id: "actual",
        header: "Actual",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.alert?.actual_value ?? -1,
        cell: (row) => formatMetricValue(row.alert?.actual_value),
      },
      {
        id: "threshold",
        header: "Threshold",
        size: 135,
        minSize: 110,
        sortValue: (row) => row.alert?.threshold_value ?? -1,
        cell: (row) =>
          row.alert?.threshold_value == null
            ? "condition value"
            : formatMetricValue(row.alert.threshold_value),
      },
      {
        id: "evaluated",
        header: "Last evaluated",
        size: 135,
        minSize: 110,
        sortValue: (row) => row.policy.last_evaluated_at ?? "",
        cell: (row) =>
          row.alert?.observed_at
            ? formatCompactTime(row.alert.observed_at)
            : row.policy.last_evaluated_at
              ? formatCompactTime(row.policy.last_evaluated_at)
              : "never",
      },
    ],
    [],
  );
  const alertColumns = useMemo<ConsoleDataGridColumn<PolicyAlertRecord>[]>(
    () => [
      {
        id: "time",
        header: "Time",
        size: 135,
        minSize: 110,
        sortValue: (row) => row.observed_at,
        cell: (row) => formatCompactTime(row.observed_at),
      },
      {
        id: "severity",
        header: "Severity",
        size: 105,
        minSize: 90,
        sortValue: (row) => row.severity,
        cell: (row) => (
          <ConsoleStatusBadge
            tone={
              row.severity === "critical"
                ? "critical"
                : row.severity === "warning"
                  ? "warning"
                  : "info"
            }
          >
            {row.severity}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "policy",
        header: "Policy",
        size: 165,
        minSize: 130,
        searchValue: (row) => policyById.get(row.policy_group_id)?.name ?? "",
        cell: (row) =>
          policyById.get(row.policy_group_id)?.name ??
          shortId(row.policy_group_id),
      },
      {
        id: "rule",
        header: "Rule",
        size: 185,
        minSize: 140,
        searchValue: (row) =>
          policyById
            .get(row.policy_group_id)
            ?.rules.find((rule) => rule.id === row.policy_rule_id)?.name ?? "",
        cell: (row) =>
          policyById
            .get(row.policy_group_id)
            ?.rules.find((rule) => rule.id === row.policy_rule_id)?.name ??
          shortId(row.policy_rule_id),
      },
      {
        id: "actual",
        header: "Actual",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.actual_value ?? -1,
        cell: (row) => formatMetricValue(row.actual_value),
      },
      {
        id: "threshold",
        header: "Threshold",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.threshold_value ?? -1,
        cell: (row) => formatMetricValue(row.threshold_value),
      },
      {
        id: "state",
        header: "State",
        size: 95,
        minSize: 80,
        cell: () => "open",
      },
    ],
    [policyById],
  );

  return (
    <div className="trafficRulesDetail detailStack">
      <div className="sectionHeader compactHeader">
        <div>
          <h4>Traffic & Rules</h4>
          <span>
            {formatVpsName(agent, "name_id_suffix")} ·{" "}
            <span className="monoValue">{agent.id}</span>
          </span>
        </div>
        <span className="sectionContext">
          Last accounting sample:{" "}
          {trafficAccounting?.last_sample_at
            ? formatTime(trafficAccounting.last_sample_at)
            : "none"}
        </span>
      </div>
      <div className="consoleOperationsBar">
        <span>
          <strong>{selectorSummary(trafficAccounting)}</strong>
          <small>
            {trafficAccounting?.selector_hash ?? "no selector hash"}
          </small>
        </span>
        <div className="consoleOperationsActions">
          <button
            className="secondaryAction compactAction"
            type="button"
            onClick={() =>
              onNavigatePanel?.(
                "Config",
                `rules:id:${encodeURIComponent(agent.id)}`,
              )
            }
          >
            Edit VPS Rules
          </button>
          <button
            className="secondaryAction compactAction"
            disabled={!selectedPolicyId}
            type="button"
            onClick={() =>
              selectedPolicyId && onNavigatePanel?.("Observability", "alerts")
            }
          >
            Open Alert Policy
          </button>
          <button
            className="secondaryAction compactAction"
            type="button"
            onClick={() => onNavigatePanel?.("Fleet", "alerts")}
          >
            Open Fleet Alerts
          </button>
        </div>
      </div>
      <div className="signalGrid fleetSignalGrid">
        <Metric
          label="Cycle used"
          value={
            trafficAccounting
              ? formatBytes(trafficAccounting.total_bytes)
              : "not configured"
          }
          tone="blue"
        />
        <Metric
          label="Quota"
          value={quotaSummary(trafficAccounting)}
          tone="green"
        />
        <Metric
          label="Cycle percent"
          value={
            trafficAccounting?.cycle_percent == null
              ? "incomplete"
              : `${trafficAccounting.cycle_percent.toFixed(1)}%`
          }
          tone="blue"
        />
        <Metric
          label="Traffic state"
          value={trafficStateForClient(trafficAccounting, policyAlerts)}
          tone="green"
        />
      </div>
      <div className="consoleInlineDetailGrid trafficAccountingSummary">
        <span>
          <strong>Cycle start</strong>
          <span>{trafficAccounting?.cycle_start ?? "not configured"}</span>
        </span>
        <span>
          <strong>Cycle end</strong>
          <span>{trafficAccounting?.cycle_end ?? "not configured"}</span>
        </span>
        <span>
          <strong>Reset day</strong>
          <span>{resetDaySummary(trafficAccounting)}</span>
        </span>
        <span>
          <strong>Cycle timezone</strong>
          <span>UTC</span>
        </span>
        <span>
          <strong>Last sample</strong>
          <span>
            {trafficAccounting?.last_sample_at
              ? formatCompactTime(trafficAccounting.last_sample_at)
              : "none"}
          </span>
        </span>
        <span>
          <strong>Counter epochs seen</strong>
          <span>{trafficAccounting?.counter_epochs_seen ?? 0}</span>
        </span>
        <span>
          <strong>Incomplete reasons</strong>
          <span>
            {trafficAccounting?.incomplete_reasons.join(", ") || "none"}
          </span>
        </span>
      </div>
      <div className="trafficRulesGridSection">
        <ConsoleDataGrid
          columns={trafficColumns}
          defaultPageSize={5}
          empty="No traffic selectors configured."
          getRowId={(row) => `${row.source}:${row.interface}:${row.direction}`}
          itemLabel="selectors"
          rows={trafficRows}
          searchPlaceholder="Search selected traffic"
          selectable={false}
          storageKey={`vpsman.grid.fleet.traffic.selected.${agent.id}`}
          title="Selected traffic"
        />
      </div>
      <div className="trafficRulesGridSection">
        <ConsoleDataGrid
          columns={vpsRuleColumns}
          defaultPageSize={6}
          empty="No VPS rule values set."
          getRowId={(row) => `${row.client_id}:${row.key}`}
          itemLabel="values"
          rows={vpsRuleValues}
          searchPlaceholder="Search VPS rule values"
          selectable={false}
          storageKey={`vpsman.grid.fleet.traffic.rules.${agent.id}`}
          title="VPS rule values"
        />
      </div>
      <div className="trafficRulesGridSection">
        <ConsoleDataGrid
          columns={policyColumns}
          defaultPageSize={6}
          empty="No matched policy rule state for this VPS."
          getRowId={(row) => `${row.policy.id}:${row.rule.id}`}
          itemLabel="policy rules"
          rows={matchedPolicyRows}
          searchPlaceholder="Search matched policy rules"
          selectable={false}
          storageKey={`vpsman.grid.fleet.traffic.policies.${agent.id}`}
          title="Matched policies"
        />
      </div>
      <div className="trafficRulesGridSection">
        <ConsoleDataGrid
          columns={alertColumns}
          defaultPageSize={6}
          empty="No issued policy alerts."
          getRowId={(row) => row.id}
          itemLabel="alerts"
          rows={policyAlerts}
          searchPlaceholder="Search recent policy alerts"
          selectable={false}
          storageKey={`vpsman.grid.fleet.traffic.alerts.${agent.id}`}
          title="Recent policy alerts"
        />
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
  onOpenUpdateCheck,
  onOpenSelectorWorkflow,
  selectionStatsMode,
  setSelectionStatsMode,
  tagDisplayOrder,
  tagVisibilityOverrides,
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
  onOpenUpdateCheck: (rows: AgentView[]) => void;
  onOpenSelectorWorkflow: (
    rows: AgentView[],
    view: ActiveView,
    subpage: string,
    storageKey: string,
  ) => void;
  selectionStatsMode: FleetSelectionStatsMode;
  setSelectionStatsMode: (mode: FleetSelectionStatsMode) => void;
  tagDisplayOrder: TagDisplayOrder;
  tagVisibilityOverrides: Record<string, boolean>;
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
  const tagNames = useMemo(() => allTags.map((tag) => tag.name), [allTags]);
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
            onClick={() => onOpenUpdateCheck(agents)}
            type="button"
          >
            <ArrowUpCircle size={14} /> Check update
          </button>
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
            <TerminalSquare size={14} /> Open dispatch
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={() =>
              onOpenSelectorWorkflow(
                agents,
                "Remote Operations",
                "bulk_files",
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
                "bulk_patch",
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
                "Fleet",
                "group_bulk",
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
        tagDisplayOrder={tagDisplayOrder}
        tagVisibilityOverrides={tagVisibilityOverrides}
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
  tagDisplayOrder,
  tagVisibilityOverrides,
  vpsNameDisplayMode,
}: {
  agents: AgentView[];
  latestNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestRollups: Map<string, TelemetryRollupRecord>;
  mode: FleetSelectionStatsMode;
  tagDisplayOrder: TagDisplayOrder;
  tagVisibilityOverrides: Record<string, boolean>;
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
        {rows.map((agent) => {
          const displayState = agentDisplayState(agent);
          return (
            <div className="fleetSelectionStatsRow" key={agent.id}>
              <span title={agent.id}>
                {formatVpsName(agent, vpsNameDisplayMode)}
              </span>
              <span title={displayState.detail}>{displayState.label}</span>
              <span>{countryFromTags(agent.tags) ?? "unset"}</span>
              <span>{providerFromTags(agent.tags) ?? "unset"}</span>
              <span>{formatLastSeen(agent.last_seen_at)}</span>
              <span>
                {displayTags(
                  agent.tags,
                  tagDisplayOrder,
                  tagVisibilityOverrides,
                ).join(", ") || "untagged"}
              </span>
            </div>
          );
        })}
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
        color: fleetChartColors[0],
        label: "RAM used",
        values: sorted.map((rollup) => memoryUsedRatio(rollup)),
      },
      {
        color: fleetChartColors[1],
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
        color: fleetChartColors[2],
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
        color: fleetChartColors[0],
        label: "RX",
        values: times.map((time) => rx.get(time) ?? null),
      },
      {
        color: fleetChartColors[3],
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
  const countryTag = tags.find(isCountryTag);
  if (!countryTag) {
    return null;
  }
  const [, code] = countryTag.split(/[:=_-]/, 2);
  return code ? code.toUpperCase() : null;
}

function providerFromTags(tags: string[]): string | null {
  const providerTag = tags.find(isProviderTag);
  if (!providerTag) {
    return null;
  }
  const [, provider] = providerTag.split(/[:=_-]/, 2);
  return provider || null;
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

function reviewedDeliveryHash(
  rows: Array<{ review_preview_hash?: string | null }>,
  operationLabel: string,
): string {
  const hashes = Array.from(
    new Set(
      rows
        .map((row) => row.review_preview_hash?.trim())
        .filter((hash): hash is string => Boolean(hash)),
    ),
  );
  if (rows.length === 0) {
    throw new Error(`${operationLabel} matched no delivery rows`);
  }
  if (hashes.length !== 1) {
    throw new Error(`${operationLabel} review hash is missing or inconsistent`);
  }
  return hashes[0];
}

function shortDeliveryError(error: string | null | undefined): string {
  const trimmed = error?.trim();
  if (!trimmed) {
    return "";
  }
  return trimmed.length > 96 ? `${trimmed.slice(0, 93)}...` : trimmed;
}

function scopeSummary(scopeKind: string, scopeValue?: string | null): string {
  return scopeValue ? `${scopeKind}:${scopeValue}` : scopeKind;
}

function tokenSummary(values: string[], empty: string): string {
  return values.length > 0 ? values.join(", ") : empty;
}

function policyRulesSummary(policy: FleetAlertPolicyRecord): string {
  if (policy.rule_count === 0) {
    return "no rules";
  }
  const trafficCount = policy.rules.filter((rule) =>
    rule.condition_expression.includes("traffic."),
  ).length;
  const resourceCount = policy.rule_count - trafficCount;
  const parts = [
    trafficCount > 0 ? `${trafficCount} traffic` : null,
    resourceCount > 0 ? `${resourceCount} resource` : null,
    `${policy.enabled_rule_count} enabled / ${policy.rule_count} total`,
  ].filter((part): part is string => Boolean(part));
  return parts.join(" · ");
}

function policyRequestRulesSummary(policy: FleetAlertPolicyRequest): string {
  const enabled = policy.rules.filter((rule) => rule.enabled !== false).length;
  return `${enabled} enabled / ${policy.rules.length} total`;
}

function policyActiveSummary(policy: FleetAlertPolicyRecord): string {
  if (policy.active_critical_count > 0) {
    return `${policy.active_critical_count} critical`;
  }
  if (policy.active_warning_count > 0) {
    return `${policy.active_warning_count} warning`;
  }
  return "0";
}

function policyRuleLabel(rule: {
  condition_expression: string;
  severity: string;
}): string {
  return `${rule.condition_expression} · ${rule.severity}`;
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
        <strong>Selector</strong>
        <span className="monoValue">{policy.selector_expression}</span>
      </span>
      <span>
        <strong>State</strong>
        <span>{policy.enabled ? "enabled" : "disabled"}</span>
      </span>
      <span>
        <strong>Rules</strong>
        <span>{policyRulesSummary(policy)}</span>
      </span>
      <span>
        <strong>Matched VPSs</strong>
        <span>{policy.matched_vps_count}</span>
      </span>
      <span>
        <strong>Active states</strong>
        <span>{policyActiveSummary(policy)}</span>
      </span>
      <span>
        <strong>Incomplete VPSs</strong>
        <span>{policy.incomplete_vps_count}</span>
      </span>
      <span>
        <strong>Last evaluated</strong>
        <span>
          {policy.last_evaluated_at
            ? formatCompactTime(policy.last_evaluated_at)
            : "never"}
        </span>
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
      <span>
        <strong>Rule rows</strong>
        <span>
          {policy.rules.length === 0
            ? "none"
            : policy.rules.map(policyRuleLabel).join(" · ")}
        </span>
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
        <strong>Signing</strong>
        <span>
          {rule.signing_secret_set
            ? "HMAC secret configured"
            : "no signing secret"}
        </span>
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

const POLICY_WINDOWS = [0, 60, 300, 900] as const;
const POLICY_SEVERITIES = ["info", "warning", "critical"] as const;

type PolicyRuleDraft = {
  localId: string;
  id?: string;
  name: string;
  enabled: boolean;
  condition_expression: string;
  traffic_selector: string;
  window_secs: string;
  severity: string;
};

type PolicySaveSnapshot = {
  request: FleetAlertPolicyRequest;
  preview: PolicyDryRunResponse;
  title: string;
};

function defaultPolicyRuleDraft(): PolicyRuleDraft {
  return {
    localId: crypto.randomUUID(),
    name: "80% total quota",
    enabled: true,
    condition_expression: "traffic.cycle.total >= traffic.quota.total * 0.8",
    traffic_selector: "",
    window_secs: "0",
    severity: "warning",
  };
}

function draftFromPolicyRule(rule: PolicyRuleRecord): PolicyRuleDraft {
  return {
    localId: rule.id,
    id: rule.id,
    name: rule.name,
    enabled: rule.enabled,
    condition_expression: rule.condition_expression,
    traffic_selector: rule.traffic_selector ?? "",
    window_secs: String(rule.window_secs),
    severity: rule.severity,
  };
}

function requestRuleFromDraft(draft: PolicyRuleDraft): PolicyRuleRequest {
  return {
    id: draft.id,
    name: draft.name.trim(),
    enabled: draft.enabled,
    condition_expression: draft.condition_expression.trim(),
    traffic_selector: draft.traffic_selector.trim() || null,
    window_secs: optionalInteger(draft.window_secs) ?? 0,
    severity: draft.severity,
  };
}

function policyRequestFromRecord(
  policy: FleetAlertPolicyRecord,
  overrides: Partial<FleetAlertPolicyRequest> = {},
): FleetAlertPolicyRequest {
  return {
    id: policy.id,
    name: policy.name,
    enabled: policy.enabled,
    selector_expression: policy.selector_expression,
    rules: policy.rules.map((rule) => ({
      id: rule.id,
      name: rule.name,
      enabled: rule.enabled,
      condition_expression: rule.condition_expression,
      traffic_selector: rule.traffic_selector,
      window_secs: rule.window_secs,
      severity: rule.severity,
    })),
    notes: policy.notes,
    confirmed: true,
    preview_hash: null,
    ...overrides,
  };
}

export function FleetAlertPolicyManager({
  agents,
  editorMode = "inline",
  onEditorOpenChange,
  policies,
  policyAlerts,
  policyFocusId,
  policyFilterClientId,
  onDelete,
  onDryRun,
  onUpsert,
}: {
  agents: AgentView[];
  editorMode?: "inline" | "focused";
  onEditorOpenChange?: (open: boolean) => void;
  policies: FleetAlertPolicyRecord[];
  policyAlerts: PolicyAlertRecord[];
  policyFocusId: string | null;
  policyFilterClientId: string | null;
  onDelete: (policyId: string, reviewedName: string) => Promise<void>;
  onDryRun: (request: PolicyDryRunRequest) => Promise<PolicyDryRunResponse>;
  onUpsert: (
    request: FleetAlertPolicyRequest,
  ) => Promise<FleetAlertPolicyRecord>;
}) {
  const focusedEditor = editorMode === "focused";
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [detailPolicyId, setDetailPolicyId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<FleetAlertPolicyRecord[] | null>(
    null,
  );
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [saveSnapshot, setSaveSnapshot] = useState<PolicySaveSnapshot | null>(
    null,
  );
  const [name, setName] = useState("edge-traffic");
  const [selectorExpression, setSelectorExpression] = useState("tag:edge");
  const [enabled, setEnabled] = useState(true);
  const [notes, setNotes] = useState("");
  const [ruleDrafts, setRuleDrafts] = useState<PolicyRuleDraft[]>([
    defaultPolicyRuleDraft(),
  ]);
  const [dryRunPreview, setDryRunPreview] =
    useState<PolicyDryRunResponse | null>(null);
  const [dryRunPending, setDryRunPending] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  const agentNameById = useMemo(
    () =>
      new Map(
        agents.map((agent) => [
          agent.id,
          formatVpsName(agent, "name_id_suffix"),
        ]),
      ),
    [agents],
  );
  const alertsByPolicy = useMemo(() => {
    const grouped = new Map<string, PolicyAlertRecord[]>();
    for (const alert of policyAlerts) {
      const existing = grouped.get(alert.policy_group_id) ?? [];
      existing.push(alert);
      grouped.set(alert.policy_group_id, existing);
    }
    return grouped;
  }, [policyAlerts]);

  const policyColumns = useMemo<
    ConsoleDataGridColumn<FleetAlertPolicyRecord>[]
  >(
    () => [
      {
        id: "name",
        header: "Policy",
        size: 250,
        minSize: 180,
        sortValue: (policy) => policy.name,
        searchValue: (policy) => policy.name + " " + (policy.notes ?? ""),
        cell: (policy) => (
          <span className="historyPrimary">
            <strong>{policy.name}</strong>
            <small className="monoValue">{shortId(policy.id)}</small>
          </span>
        ),
      },
      {
        id: "enabled",
        header: "Enabled",
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
        id: "selector",
        header: "Selector",
        size: 280,
        minSize: 180,
        searchValue: (policy) => policy.selector_expression,
        sortValue: (policy) => policy.selector_expression,
        cell: (policy) => (
          <span className="monoValue">{policy.selector_expression}</span>
        ),
      },
      {
        id: "matched",
        header: "Matched VPSs",
        size: 120,
        minSize: 95,
        sortValue: (policy) => policy.matched_vps_count,
        cell: (policy) => policy.matched_vps_count,
      },
      {
        id: "rules",
        header: "Rules",
        size: 210,
        minSize: 150,
        searchValue: policyRulesSummary,
        sortValue: (policy) => policy.enabled_rule_count,
        cell: (policy) => policyRulesSummary(policy),
      },
      {
        id: "active",
        header: "Active States",
        size: 130,
        minSize: 105,
        sortValue: (policy) =>
          policy.active_critical_count * 1000 + policy.active_warning_count,
        cell: (policy) => policyActiveSummary(policy),
      },
      {
        id: "incomplete",
        header: "Incomplete VPSs",
        size: 135,
        minSize: 110,
        sortValue: (policy) => policy.incomplete_vps_count,
        cell: (policy) => policy.incomplete_vps_count,
      },
      {
        id: "last_evaluated",
        header: "Last Evaluated",
        size: 145,
        minSize: 115,
        sortValue: (policy) => policy.last_evaluated_at ?? "",
        cell: (policy) =>
          policy.last_evaluated_at
            ? formatCompactTime(policy.last_evaluated_at)
            : "never",
      },
      {
        id: "updated",
        header: "Updated",
        size: 135,
        minSize: 110,
        sortValue: (policy) => policy.updated_at,
        cell: (policy) => formatCompactTime(policy.updated_at),
      },
    ],
    [],
  );

  useEffect(() => {
    setSaveSnapshot(null);
  }, [name, selectorExpression, enabled, notes, ruleDrafts]);

  useEffect(() => {
    onEditorOpenChange?.(editorOpen);
  }, [editorOpen, onEditorOpenChange]);

  useEffect(() => {
    if (!policyFocusId) {
      return;
    }
    const focused = policies.find((policy) => policy.id === policyFocusId);
    if (!focused) {
      setStatus("Policy not found: " + shortId(policyFocusId));
      return;
    }
    setEditorOpen(false);
    setDetailPolicyId(focused.id);
    setStatus("viewing " + focused.name);
  }, [policies, policyFocusId]);

  function currentDryRunRequest(): PolicyDryRunRequest {
    return {
      id: editingId ?? undefined,
      name: name.trim(),
      enabled,
      selector_expression: selectorExpression.trim(),
      rules: ruleDrafts.map(requestRuleFromDraft),
      notes: notes.trim() || null,
    };
  }

  function currentUpsertRequest(previewHash: string): FleetAlertPolicyRequest {
    return {
      ...currentDryRunRequest(),
      confirmed: true,
      preview_hash: previewHash,
    };
  }

  function resetForm() {
    setEditingId(null);
    setName("edge-traffic");
    setSelectorExpression("tag:edge");
    setEnabled(true);
    setNotes("");
    setRuleDrafts([defaultPolicyRuleDraft()]);
    setDryRunPreview(null);
    setSaveSnapshot(null);
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
    setSelectorExpression(policy.selector_expression);
    setEnabled(policy.enabled);
    setNotes(policy.notes ?? "");
    setRuleDrafts(
      policy.rules.length > 0
        ? policy.rules.map(draftFromPolicyRule)
        : [defaultPolicyRuleDraft()],
    );
    setDryRunPreview(null);
    setSaveSnapshot(null);
    setStatus("editing " + policy.name);
    setEditorOpen(true);
  }

  function openPolicyDetails(policy: FleetAlertPolicyRecord) {
    setEditorOpen(false);
    setDetailPolicyId(policy.id);
    setStatus("viewing " + policy.name);
  }

  function updateRuleDraft(localId: string, patch: Partial<PolicyRuleDraft>) {
    setRuleDrafts((current) =>
      current.map((draft) =>
        draft.localId === localId ? { ...draft, ...patch } : draft,
      ),
    );
  }

  function addRuleDraft() {
    setRuleDrafts((current) => [...current, defaultPolicyRuleDraft()]);
  }

  function removeRuleDraft(localId: string) {
    setRuleDrafts((current) =>
      current.length <= 1
        ? current
        : current.filter((draft) => draft.localId !== localId),
    );
  }

  async function dryRunCurrentPolicy(): Promise<PolicyDryRunResponse> {
    const request = currentDryRunRequest();
    setDryRunPending(true);
    setStatus("dry-running policy");
    try {
      const preview = await onDryRun(request);
      setDryRunPreview(preview);
      setStatus("dry-run matched " + preview.matched_vps_count + " VPSs");
      return preview;
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "policy dry-run failed";
      setStatus(message);
      throw error;
    } finally {
      setDryRunPending(false);
    }
  }

  async function reviewSubmit() {
    try {
      const preview = await dryRunCurrentPolicy();
      setSaveSnapshot({
        request: currentUpsertRequest(preview.preview_hash),
        preview,
        title: editingId ? "Update alert policy" : "Create alert policy",
      });
    } catch {
      // Status is set by dryRunCurrentPolicy.
    }
  }

  async function submit() {
    const snapshot = saveSnapshot;
    if (!snapshot) {
      setStatus("Run dry-run and review policy before saving");
      return;
    }
    setStatus(editingId ? "updating policy" : "creating policy");
    try {
      const policy = await onUpsert(snapshot.request);
      setEditingId(policy.id);
      setEditorOpen(true);
      setSaveSnapshot(null);
      setDryRunPreview(snapshot.preview);
      setStatus("saved " + policy.name);
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
        await onDelete(policy.id, policy.name);
      }
      if (rows.some((policy) => policy.id === editingId)) {
        resetForm();
        setEditorOpen(false);
      }
      if (rows.some((policy) => policy.id === detailPolicyId)) {
        setDetailPolicyId(null);
      }
      setDeleteRows(null);
      setStatus("deleted " + rows.length);
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
        const base = policyRequestFromRecord(policy, { enabled: nextEnabled });
        const preview = await onDryRun({
          id: base.id,
          name: base.name,
          enabled: base.enabled,
          selector_expression: base.selector_expression,
          rules: base.rules,
          notes: base.notes,
        });
        await onUpsert({ ...base, preview_hash: preview.preview_hash });
      }
      setStatus((nextEnabled ? "enabled " : "disabled ") + rows.length);
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
          "Opens policy group details below the table.",
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
          "Opens the policy group editor below the table.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Pencil size={14} />,
      onSelect: (rows) => rows[0] && editPolicy(rows[0]),
    },
    {
      label: "Enable",
      description: (rows) =>
        "Enable " +
        rows.filter((policy) => !policy.enabled).length +
        " disabled selected policy groups.",
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
        "Disable " +
        rows.filter((policy) => policy.enabled).length +
        " enabled selected policy groups.",
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
        "Delete " +
        rows.length +
        " selected policy groups. Issued alerts remain in alert history.",
      disabled: (rows) => rows.length === 0,
      icon: <Trash2 size={14} />,
      onSelect: requestDeletePolicies,
      tone: "danger",
    },
  ];

  const detailPolicy = detailPolicyId
    ? policies.find((candidate) => candidate.id === detailPolicyId)
    : null;
  const showPolicyList = !focusedEditor || !editorOpen;

  return (
    <div
      className={
        focusedEditor && editorOpen
          ? "consoleCrudPanel focusedPolicyEditor"
          : "consoleCrudPanel"
      }
    >
      <div className="consoleResourceLayout fullWidth">
        {showPolicyList && policyFilterClientId ? (
          <div className="notice infoNotice">
            Focused VPS:{" "}
            <span className="monoValue">{policyFilterClientId}</span>. Policy
            rows show server-evaluated match counts; open a policy dry-run to
            inspect exact matched VPSs.
          </div>
        ) : null}
        {showPolicyList ? (
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
            searchPlaceholder="Search policies by name, selector, rules, or notes"
            storageKey="vpsman.grid.fleet.alertPolicies.v3"
            title="Policy groups"
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
        ) : null}
        {showPolicyList && detailPolicy && !editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <button
                className="secondaryAction"
                type="button"
                onClick={() => editPolicy(detailPolicy)}
              >
                Edit policy
              </button>
            }
            description="Policy group metadata, rule rows, and recent issued alerts."
            onClose={() => setDetailPolicyId(null)}
            title="Alert policy details"
          >
            <PolicyDetailGrid policy={detailPolicy} />
            <IssuedPolicyAlertList
              alerts={alertsByPolicy.get(detailPolicy.id) ?? []}
              agentNameById={agentNameById}
            />
          </ConsoleDetailPanel>
        ) : null}
        {editorOpen ? (
          <ConsoleDetailPanel
            actions={
              focusedEditor ? (
                <>
                  <button
                    className="secondaryAction"
                    disabled={dryRunPending}
                    type="button"
                    onClick={() => void dryRunCurrentPolicy()}
                  >
                    Preview matches
                  </button>
                  <button
                    className="primaryAction"
                    disabled={dryRunPending}
                    type="button"
                    onClick={() => void reviewSubmit()}
                  >
                    {editingId ? "Update policy" : "Create policy"}
                  </button>
                </>
              ) : (
                <>
                  <button
                    className="secondaryAction"
                    disabled={dryRunPending}
                    type="button"
                    onClick={() => void dryRunCurrentPolicy()}
                  >
                    Dry-run
                  </button>
                  <button
                    className="primaryAction"
                    disabled={dryRunPending}
                    type="button"
                    onClick={() => void reviewSubmit()}
                  >
                    {editingId ? "Review update" : "Review create"}
                  </button>
                  <button
                    className="secondaryAction"
                    type="button"
                    onClick={createPolicy}
                  >
                    New policy
                  </button>
                </>
              )
            }
            description={
              focusedEditor
                ? "Preview exactly which VPSs match, then save this policy group with the reviewed activation state."
                : "Edit the selector expression, preview matched VPSs, then confirm the exact policy payload."
            }
            onClose={() => setEditorOpen(false)}
            title={editingId ? "Edit alert policy" : "Create alert policy"}
          >
            {focusedEditor ? (
              <PolicyMatchSummary
                enabled={enabled}
                editing={Boolean(editingId)}
                preview={dryRunPreview}
              />
            ) : null}
            <div className="consoleFormGrid">
              <ConsoleField label="Name" className="fieldWide">
                <input
                  aria-label="Policy name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label={focusedEditor ? "Activation" : "Enabled"}>
                <label className="checkLine inlineCheck">
                  <input
                    checked={enabled}
                    onChange={(event) => setEnabled(event.target.checked)}
                    type="checkbox"
                  />
                  <span>
                    {focusedEditor
                      ? editingId
                        ? "Policy enabled"
                        : "Enable after creation"
                      : "Evaluate policy"}
                  </span>
                </label>
              </ConsoleField>
              <ConsoleField
                className="fieldFull"
                hint="Use the same fleet selector expression design as saved fleet views and job targeting, including id:<client_id>, name:<name>, tag:<tag>, provider:<provider>, country:<code>, status:<state>, &&, ||, !, and parentheses."
                label="VPS selector expression"
              >
                <SearchExpressionInput
                  ariaLabel="Policy VPS selector expression"
                  onChange={setSelectorExpression}
                  placeholder="tag:edge && provider:hetzner"
                  value={selectorExpression}
                />
              </ConsoleField>
              <ConsoleField label="Notes" className="fieldFull">
                <textarea
                  aria-label="Policy notes"
                  value={notes}
                  onChange={(event) => setNotes(event.target.value)}
                />
              </ConsoleField>
            </div>
            <div className="gridBlock">
              <div className="sectionHeader compactHeader">
                <div>
                  <h4>Rule rows</h4>
                  <span>{ruleDrafts.length} rule rows</span>
                </div>
                <button
                  className="secondaryAction compactAction"
                  onClick={addRuleDraft}
                  type="button"
                >
                  <Plus size={14} />
                  <span>Add rule</span>
                </button>
              </div>
              <div className="policyRuleEditor">
                {ruleDrafts.map((draft) => (
                  <section className="policyRuleCard" key={draft.localId}>
                    <div className="policyRuleCardHeader">
                      <label className="checkLine inlineCheck">
                        <input
                          checked={draft.enabled}
                          onChange={(event) =>
                            updateRuleDraft(draft.localId, {
                              enabled: event.target.checked,
                            })
                          }
                          type="checkbox"
                        />
                        <span>Enabled</span>
                      </label>
                      <button
                        className="secondaryAction compactAction"
                        disabled={ruleDrafts.length <= 1}
                        onClick={() => removeRuleDraft(draft.localId)}
                        type="button"
                      >
                        Remove
                      </button>
                    </div>
                    <div className="consoleFormGrid policyRuleFormGrid">
                      <ConsoleField label="Rule">
                        <input
                          aria-label="Rule name"
                          value={draft.name}
                          onChange={(event) =>
                            updateRuleDraft(draft.localId, {
                              name: event.target.value,
                            })
                          }
                        />
                      </ConsoleField>
                      <ConsoleField
                        label="Condition expression"
                        className="fieldFull"
                      >
                        <textarea
                          aria-label="Rule condition expression"
                          value={draft.condition_expression}
                          onChange={(event) =>
                            updateRuleDraft(draft.localId, {
                              condition_expression: event.target.value,
                            })
                          }
                        />
                      </ConsoleField>
                      <ConsoleField label="Traffic selector override">
                        <input
                          aria-label="Traffic selector override"
                          placeholder="blank = VPS traffic.selectors"
                          value={draft.traffic_selector}
                          onChange={(event) =>
                            updateRuleDraft(draft.localId, {
                              traffic_selector: event.target.value,
                            })
                          }
                        />
                      </ConsoleField>
                      <ConsoleField label="Window">
                        <select
                          aria-label="Rule window"
                          value={draft.window_secs}
                          onChange={(event) =>
                            updateRuleDraft(draft.localId, {
                              window_secs: event.target.value,
                            })
                          }
                        >
                          {POLICY_WINDOWS.map((windowSecs) => (
                            <option key={windowSecs} value={String(windowSecs)}>
                              {windowSecs === 0
                                ? "immediate"
                                : windowSecs / 60 + "m"}
                            </option>
                          ))}
                        </select>
                      </ConsoleField>
                      <ConsoleField label="Severity">
                        <select
                          aria-label="Rule severity"
                          value={draft.severity}
                          onChange={(event) =>
                            updateRuleDraft(draft.localId, {
                              severity: event.target.value,
                            })
                          }
                        >
                          {POLICY_SEVERITIES.map((severity) => (
                            <option key={severity} value={severity}>
                              {severity}
                            </option>
                          ))}
                        </select>
                      </ConsoleField>
                    </div>
                  </section>
                ))}
              </div>
            </div>
            {dryRunPreview ? (
              <PolicyDryRunPreview
                agentNameById={agentNameById}
                preview={dryRunPreview}
                title={focusedEditor ? "Match preview" : undefined}
              />
            ) : null}
          </ConsoleDetailPanel>
        ) : null}
      </div>
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <ConfirmationPrompt
        confirmLabel={saveSnapshot?.title ?? "Save policy"}
        detail="Saves the reviewed policy group and all rule rows with the dry-run preview hash."
        items={[
          { label: "Policy", value: saveSnapshot?.request.name ?? "-" },
          {
            label: "Selector",
            value: saveSnapshot?.request.selector_expression ?? "-",
          },
          {
            label: "Matched VPSs",
            value: saveSnapshot
              ? String(saveSnapshot.preview.matched_vps_count)
              : "-",
          },
          {
            label: "Rules",
            value: saveSnapshot
              ? policyRequestRulesSummary(saveSnapshot.request)
              : "-",
          },
          {
            label: "Preview hash",
            value: saveSnapshot?.request.preview_hash ?? "-",
          },
        ]}
        onCancel={() => setSaveSnapshot(null)}
        onConfirm={() => void submit()}
        open={saveSnapshot !== null}
        pending={false}
        title="Confirm alert policy save"
      />
      <ConfirmationPrompt
        confirmLabel="Delete alert policies"
        detail="Deletes selected policy groups. Issued policy alerts remain available in Fleet alerts."
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

function PolicyDryRunPreview({
  agentNameById,
  preview,
  title = "Dry-run preview",
}: {
  agentNameById: Map<string, string>;
  preview: PolicyDryRunResponse;
  title?: string;
}) {
  return (
    <div className="gridBlock">
      <h4>{title}</h4>
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Matched VPSs</strong>
          <span>{preview.matched_vps_count}</span>
        </span>
        <span>
          <strong>Incomplete VPSs</strong>
          <span>{preview.incomplete_vps_count}</span>
        </span>
        <span>
          <strong>Invalid rules</strong>
          <span>{preview.invalid_rule_count}</span>
        </span>
        <span>
          <strong>Preview hash</strong>
          <span className="monoValue">{preview.preview_hash}</span>
        </span>
      </div>
      {preview.validation_errors.length > 0 ? (
        <div className="notice warningNotice">
          {preview.validation_errors.join(" · ")}
        </div>
      ) : null}
      <div className="miniTable">
        {preview.rule_previews.map((rule) => (
          <div
            className="miniTableRow"
            key={rule.rule_name + rule.condition_expression}
          >
            <strong>{rule.rule_name}</strong>
            <span className="monoValue">{rule.condition_expression}</span>
            <span>{rule.category}</span>
            <ConsoleStatusBadge
              tone={
                rule.severity === "critical"
                  ? "critical"
                  : rule.severity === "warning"
                    ? "warning"
                    : "info"
              }
            >
              {rule.severity}
            </ConsoleStatusBadge>
            <span>{rule.true_count} true</span>
            <span>{rule.false_count} false</span>
            <span>{rule.incomplete_count} incomplete</span>
          </div>
        ))}
      </div>
      <div className="tokenPreview">
        {preview.matched_vps.slice(0, 40).map((clientId) => (
          <span className="tokenChip" key={clientId} title={clientId}>
            {agentNameById.get(clientId) ?? clientId}
          </span>
        ))}
      </div>
    </div>
  );
}

function PolicyMatchSummary({
  enabled,
  editing,
  preview,
}: {
  enabled: boolean;
  editing: boolean;
  preview: PolicyDryRunResponse | null;
}) {
  const vpsLabel = preview?.matched_vps_count === 1 ? "VPS" : "VPSs";
  return (
    <div className="consoleInlineNotice policyMatchSummary">
      <strong>
        {preview
          ? `Matches ${preview.matched_vps_count} ${vpsLabel}`
          : "Preview matches before saving"}
      </strong>
      <small>
        {preview
          ? `${preview.incomplete_vps_count} incomplete VPSs; ${preview.invalid_rule_count} invalid rule rows.`
          : "Use Preview matches to verify the selector and rule conditions against current fleet data."}
      </small>
      <small>
        {enabled
          ? editing
            ? "Policy remains enabled after save."
            : "Enable after creation is on."
          : editing
            ? "Policy is saved disabled after update."
            : "Enable after creation is off; create saves a disabled policy."}
      </small>
    </div>
  );
}

function IssuedPolicyAlertList({
  alerts,
  agentNameById,
}: {
  alerts: PolicyAlertRecord[];
  agentNameById: Map<string, string>;
}) {
  return (
    <div className="gridBlock">
      <h4>Recent issued alerts</h4>
      {alerts.length === 0 ? (
        <p className="mutedText">No issued alerts for this policy.</p>
      ) : (
        <div className="miniTable">
          {alerts.slice(0, 8).map((alert) => (
            <div className="miniTableRow" key={alert.id}>
              <ConsoleStatusBadge
                tone={
                  alert.severity === "critical"
                    ? "critical"
                    : alert.severity === "warning"
                      ? "warning"
                      : "info"
                }
              >
                {alert.severity}
              </ConsoleStatusBadge>
              <strong>
                {agentNameById.get(alert.client_id) ?? alert.client_id}
              </strong>
              <span>{alert.title}</span>
              <span>{formatCompactTime(alert.observed_at)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

type NotificationRegistryTab =
  | "channels"
  | "webhooks"
  | "deliveries"
  | "maintenance";

export function FleetNotificationsHub({
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
  onDeleteAlertChannel: (
    channelId: string,
    reviewedName: string,
  ) => Promise<void>;
  onDeleteWebhookRule: (ruleId: string, reviewedName: string) => Promise<void>;
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

export function DeliveryPreviewSection({
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

export function FleetAlertNotificationManager({
  agents,
  channels,
  onDelete,
  onDispatch,
  onOpenDeliveries,
  onPreviewRows,
  onProcess,
  onUpsert,
  queueMode = "full",
}: {
  agents: AgentView[];
  channels: FleetAlertNotificationChannelRecord[];
  onDelete: (channelId: string, reviewedName: string) => Promise<void>;
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
  queueMode?: "full" | "configuration";
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [detailChannelId, setDetailChannelId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<
    FleetAlertNotificationChannelRecord[] | null
  >(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [saveSnapshot, setSaveSnapshot] = useState<{
    request: FleetAlertNotificationChannelRequest;
    title: string;
  } | null>(null);
  const [name, setName] = useState("critical-webhook-channel");
  const [scopeKind, setScopeKind] = useState("global");
  const [scopeValue, setScopeValue] = useState("");
  const [minSeverity, setMinSeverity] = useState("critical");
  const [categories, setCategories] = useState("");
  const [operatorStates, setOperatorStates] = useState("");
  const [deliveryKind, setDeliveryKind] = useState("webhook");
  const [target, setTarget] = useState("");
  const [cooldownSecs, setCooldownSecs] = useState("300");
  const [enabled, setEnabled] = useState(true);
  const [notes, setNotes] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [queueConfirmation, setQueueConfirmation] = useState<
    "dispatch" | "process" | null
  >(null);
  const [queueSnapshot, setQueueSnapshot] =
    useState<AlertDeliveryQueueSnapshot | null>(null);
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

  useEffect(() => {
    setSaveSnapshot(null);
  }, [
    name,
    scopeKind,
    scopeValue,
    minSeverity,
    categories,
    operatorStates,
    deliveryKind,
    target,
    cooldownSecs,
    enabled,
    notes,
  ]);

  function resetForm() {
    setEditingId(null);
    setName("critical-webhook-channel");
    setScopeKind("global");
    setScopeValue("");
    setMinSeverity("critical");
    setCategories("");
    setOperatorStates("");
    setDeliveryKind("webhook");
    setTarget("");
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
    setDeliveryKind(
      channel.delivery_kind === "webhook" ? channel.delivery_kind : "webhook",
    );
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

  function reviewSubmit() {
    setSaveSnapshot({
      request: {
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
      },
      title: editingId ? "Update channel" : "Create channel",
    });
  }

  async function submit() {
    const snapshot = saveSnapshot;
    if (!snapshot) {
      setStatus("Review channel before saving");
      return;
    }
    setStatus(editingId ? "updating channel" : "creating channel");
    try {
      const channel = await onUpsert(snapshot.request);
      setEditingId(channel.id);
      setEditorOpen(true);
      setSaveSnapshot(null);
      setStatus(`saved ${channel.name}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "channel save failed");
    }
  }

  function requestDeleteChannels(rows: FleetAlertNotificationChannelRecord[]) {
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
        await onDelete(channel.id, channel.name);
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

  async function dispatch(dryRun: boolean, openConfirmation = false) {
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
        if (!openConfirmation) {
          onOpenDeliveries();
        }
        if (openConfirmation) {
          const previewHash = reviewedDeliveryHash(
            rows,
            "Notification dispatch",
          );
          setQueueSnapshot({
            action: "dispatch",
            request: {
              limit: 50,
              dry_run: false,
              confirmed: true,
              preview_hash: previewHash,
            },
            previewHash,
            reviewedRows: rows.length,
          });
          setQueueConfirmation("dispatch");
        }
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

  async function process(dryRun: boolean, openConfirmation = false) {
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
        if (!openConfirmation) {
          onOpenDeliveries();
        }
        if (openConfirmation) {
          const previewHash = reviewedDeliveryHash(
            rows,
            "Notification delivery",
          );
          setQueueSnapshot({
            action: "process",
            request: {
              limit: 50,
              status: "queued",
              dry_run: false,
              confirmed: true,
              preview_hash: previewHash,
            },
            previewHash,
            reviewedRows: rows.length,
          });
          setQueueConfirmation("process");
        }
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
    const snapshot = queueSnapshot;
    if (!snapshot || queuePending) {
      return;
    }
    setQueuePending(true);
    setStatus(
      snapshot.action === "dispatch"
        ? "queueing reviewed alert notifications"
        : "delivering reviewed notifications",
    );
    try {
      const rows =
        snapshot.action === "dispatch"
          ? await onDispatch(snapshot.request)
          : await onProcess(snapshot.request);
      setStatus(
        `${snapshot.action === "dispatch" ? "queued" : "processed"} ${rows.length}`,
      );
      setQueueConfirmation(null);
      setQueueSnapshot(null);
    } catch (error) {
      setStatus(
        error instanceof Error
          ? error.message
          : "notification queue action failed",
      );
    } finally {
      setQueuePending(false);
    }
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
                  onClick={reviewSubmit}
                >
                  {editingId ? "Review update" : "Review create"}
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
              <ConsoleField
                label="Delivery kind"
                hint="Webhook is the supported delivery boundary."
              >
                <select
                  aria-label="Delivery kind"
                  value={deliveryKind}
                  onChange={(event) => setDeliveryKind(event.target.value)}
                >
                  <option value="webhook">webhook</option>
                </select>
              </ConsoleField>
              <ConsoleField
                label="Delivery target"
                className="fieldWide"
                hint="Use HTTPS, or local HTTP for agent-local receivers."
              >
                <input
                  aria-label="Delivery target"
                  placeholder="https://hooks.example/vpsman"
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
            {queueMode === "configuration"
              ? "Preview matching alerts, send or retry queued delivery records, or open delivery evidence."
              : "Review matching or process queued deliveries without leaving the registry."}
          </small>
        </span>
        <div className="consoleOperationsActions">
          <button
            className="secondaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => void dispatch(true)}
          >
            {queueMode === "configuration"
              ? "Preview matches"
              : "Review matches"}
          </button>
          {queueMode === "full" ? (
            <>
              <button
                className="secondaryAction"
                disabled={queuePending}
                type="button"
                onClick={() => void dispatch(true, true)}
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
            </>
          ) : (
            <button
              className="secondaryAction"
              disabled={queuePending}
              type="button"
              onClick={onOpenDeliveries}
            >
              Open deliveries
            </button>
          )}
          <button
            className="primaryAction"
            disabled={queuePending}
            type="button"
            onClick={() => void process(true, true)}
          >
            {queueMode === "configuration" ? "Send / retry" : "Review delivery"}
          </button>
        </div>
      </div>
      <ConfirmationPrompt
        confirmLabel={
          queueConfirmation === "dispatch" ? "Queue dispatch" : "Deliver queued"
        }
        detail={
          queueConfirmation === "dispatch"
            ? "Queues notification delivery records for matching active alerts."
            : "Processes queued notification delivery records and may contact configured delivery targets."
        }
        items={[
          {
            label: "Reviewed rows",
            value: queueSnapshot?.reviewedRows ?? 0,
          },
          {
            label: "Review hash",
            value: queueSnapshot
              ? `${queueSnapshot.previewHash.slice(0, 12)}...`
              : "review required",
          },
        ]}
        onCancel={() => {
          setQueueConfirmation(null);
          setQueueSnapshot(null);
        }}
        onConfirm={() => void confirmQueueAction()}
        open={queueConfirmation !== null && queueSnapshot !== null}
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
        confirmLabel={saveSnapshot?.title ?? "Save channel"}
        detail="Saves the reviewed notification channel request exactly as shown."
        items={[
          { label: "Channel", value: saveSnapshot?.request.name ?? "-" },
          {
            label: "Scope",
            value: saveSnapshot
              ? scopeSummary(
                  saveSnapshot.request.scope_kind,
                  saveSnapshot.request.scope_value,
                )
              : "-",
          },
          {
            label: "Severity",
            value: saveSnapshot?.request.min_severity ?? "-",
          },
          {
            label: "Delivery",
            value: saveSnapshot
              ? `${saveSnapshot.request.delivery_kind} -> ${saveSnapshot.request.target}`
              : "-",
          },
        ]}
        onCancel={() => setSaveSnapshot(null)}
        onConfirm={() => void submit()}
        open={saveSnapshot !== null}
        pending={false}
        title="Confirm notification channel save"
      />
      <ConfirmationPrompt
        confirmLabel="Delete channels"
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

export function NotificationDeliveryHistoryGrid({
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
          <span className="historyPrimary">
            <strong className="monoValue">{delivery.attempt_count}</strong>
            {delivery.next_attempt_at ? (
              <small title={delivery.next_attempt_at}>retry due</small>
            ) : null}
          </span>
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
          {delivery.next_attempt_at && (
            <span title={delivery.next_attempt_at}>
              retry at {formatCompactTime(delivery.next_attempt_at)}
            </span>
          )}
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

export function WebhookRuleManager({
  agents,
  editorMode = "inline",
  onDelete,
  onDispatch,
  onDryRun,
  onEditorOpenChange,
  onOpenDeliveries,
  onPreviewDryRun,
  onPreviewRows,
  onProcess,
  onUpsert,
  queueMode = "full",
  rules,
}: {
  agents: AgentView[];
  editorMode?: "inline" | "focused";
  onDelete: (ruleId: string, reviewedName: string) => Promise<void>;
  onDispatch: (
    request: WebhookRuleDispatchRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onDryRun: (
    request: WebhookRuleDryRunRequest,
  ) => Promise<WebhookRuleDryRunRecord>;
  onEditorOpenChange?: (open: boolean) => void;
  onOpenDeliveries: () => void;
  onPreviewDryRun: (preview: WebhookRuleDryRunRecord | null) => void;
  onPreviewRows: (rows: WebhookRuleDeliveryRecord[]) => void;
  onProcess: (
    request: WebhookRuleProcessRequest,
  ) => Promise<WebhookRuleDeliveryRecord[]>;
  onUpsert: (request: WebhookRuleRequest) => Promise<WebhookRuleRecord>;
  queueMode?: "full" | "configuration";
  rules: WebhookRuleRecord[];
}) {
  const configurationQueue = queueMode === "configuration";
  const focusedEditorMode = editorMode === "focused";
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editorTestPreview, setEditorTestPreview] =
    useState<WebhookRuleDryRunRecord | null>(null);
  const [detailRuleId, setDetailRuleId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<WebhookRuleRecord[] | null>(
    null,
  );
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [saveSnapshot, setSaveSnapshot] = useState<{
    request: WebhookRuleRequest;
    title: string;
  } | null>(null);
  const [name, setName] = useState("edge-interval-webhook");
  const [enabled, setEnabled] = useState(true);
  const [expression, setExpression] = useState("interval.30sec && tag:edge");
  const [target, setTarget] = useState("https://hooks.example/vpsman");
  const [bodyTemplate, setBodyTemplate] = useState(
    "{rule.name} {event.kind} {vps.id}",
  );
  const [signingSecret, setSigningSecret] = useState("");
  const [clearSigningSecret, setClearSigningSecret] = useState(false);
  const [cooldownSecs, setCooldownSecs] = useState("300");
  const [notes, setNotes] = useState("");
  const [eventKind, setEventKind] = useState("interval.30sec");
  const [eventId, setEventId] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [queueConfirmation, setQueueConfirmation] = useState<
    "dispatch" | "process" | null
  >(null);
  const [queueSnapshot, setQueueSnapshot] =
    useState<WebhookDeliveryQueueSnapshot | null>(null);
  const [queuePending, setQueuePending] = useState(false);

  const selectedPreviewNames = useMemo(() => {
    return agents
      .filter((agent) => agent.tags.some((tag) => expression.includes(tag)))
      .slice(0, 6)
      .map((agent) => formatVpsName(agent, "name"))
      .join(", ");
  }, [agents, expression]);
  const focusedEditorOpen = focusedEditorMode && editorOpen;
  const showRuleList = !focusedEditorOpen;
  const editingRule = editingId
    ? (rules.find((rule) => rule.id === editingId) ?? null)
    : null;
  const existingSecretConfigured = editingRule?.signing_secret_set ?? false;

  useEffect(() => {
    onEditorOpenChange?.(focusedEditorOpen);
  }, [focusedEditorOpen, onEditorOpenChange]);

  useEffect(() => {
    return () => onEditorOpenChange?.(false);
  }, [onEditorOpenChange]);

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

  useEffect(() => {
    setSaveSnapshot(null);
  }, [
    name,
    enabled,
    expression,
    target,
    bodyTemplate,
    signingSecret,
    clearSigningSecret,
    cooldownSecs,
    notes,
  ]);

  function resetForm() {
    setEditingId(null);
    setEditorTestPreview(null);
    setName("edge-interval-webhook");
    setEnabled(true);
    setExpression("interval.30sec && tag:edge");
    setTarget("https://hooks.example/vpsman");
    setBodyTemplate("{rule.name} {event.kind} {vps.id}");
    setSigningSecret("");
    setClearSigningSecret(false);
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
    setEditorTestPreview(null);
    setEditingId(rule.id);
    setName(rule.name);
    setEnabled(rule.enabled);
    setExpression(rule.expression);
    setTarget(rule.target);
    setBodyTemplate(rule.body_template);
    setSigningSecret("");
    setClearSigningSecret(false);
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
      signing_secret: null,
      clear_signing_secret: false,
      cooldown_secs: rule.cooldown_secs,
      notes: rule.notes,
      confirmed: true,
      ...overrides,
    };
  }

  function reviewSubmit() {
    const nextSigningSecret = signingSecret.trim();
    setSaveSnapshot({
      request: {
        id: editingId ?? undefined,
        name: name.trim(),
        enabled,
        expression: expression.trim(),
        target: target.trim(),
        body_template: bodyTemplate,
        signing_secret:
          clearSigningSecret || !nextSigningSecret ? null : nextSigningSecret,
        clear_signing_secret: clearSigningSecret,
        cooldown_secs: optionalInteger(cooldownSecs),
        notes: notes.trim() || null,
        confirmed: true,
      },
      title: editingId ? "Update rule" : "Create rule",
    });
  }

  async function submit() {
    const snapshot = saveSnapshot;
    if (!snapshot) {
      setStatus("Review webhook rule before saving");
      return;
    }
    setStatus(editingId ? "updating webhook rule" : "creating webhook rule");
    try {
      const rule = await onUpsert(snapshot.request);
      setEditingId(rule.id);
      setSigningSecret("");
      setClearSigningSecret(false);
      setEditorOpen(true);
      setSaveSnapshot(null);
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
        await onDelete(rule.id, rule.name);
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
      if (!rule) {
        setEditorTestPreview(preview);
      }
      onPreviewDryRun(preview);
      onPreviewRows(preview.delivery ? [preview.delivery] : []);
      if (!focusedEditorOpen) {
        onOpenDeliveries();
      }
      setStatus(`dry run matched ${preview.matched_vps.length}`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "dry run failed");
    }
  }

  function clearWebhookQueueReview() {
    setQueueConfirmation(null);
    setQueueSnapshot(null);
  }

  function setWebhookEventKind(value: string) {
    setEventKind(value);
    clearWebhookQueueReview();
  }

  function setWebhookEventId(value: string) {
    setEventId(value);
    clearWebhookQueueReview();
  }

  async function dispatch(
    dryRunMode: boolean,
    openConfirmation = false,
    rule?: WebhookRuleRecord,
  ) {
    setStatus(
      dryRunMode
        ? rule
          ? `matching webhook rule ${rule.name}`
          : "matching webhook rules"
        : rule
          ? `queueing webhook test for ${rule.name}`
          : "queueing webhooks",
    );
    if (!dryRunMode) {
      setQueuePending(true);
    }
    try {
      const rows = await onDispatch({
        rule_id: rule?.id ?? null,
        event_kind: eventKind.trim(),
        event_id: eventId.trim() || null,
        limit: 50,
        dry_run: dryRunMode,
        confirmed: !dryRunMode,
      });
      if (dryRunMode) {
        onPreviewRows(rows);
        if (!openConfirmation) {
          onOpenDeliveries();
        }
        if (openConfirmation) {
          const previewHash = reviewedDeliveryHash(rows, "Webhook dispatch");
          const frozenEventKind = eventKind.trim();
          const frozenEventId = eventId.trim();
          setQueueSnapshot({
            action: "dispatch",
            request: {
              rule_id: rule?.id ?? null,
              event_kind: frozenEventKind,
              event_id: frozenEventId || null,
              limit: 50,
              dry_run: false,
              confirmed: true,
              preview_hash: previewHash,
            },
            previewHash,
            reviewedRows: rows.length,
            eventLabel: `${rule ? `${rule.name} / ` : ""}${frozenEventKind || "event"}${frozenEventId ? ` / ${frozenEventId}` : ""}`,
          });
          setQueueConfirmation("dispatch");
        }
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

  async function process(
    dryRunMode: boolean,
    openConfirmation = false,
    deliveryStatus: NonNullable<WebhookRuleProcessRequest["status"]> = "queued",
  ) {
    const isRetry = deliveryStatus === "failed";
    setStatus(
      dryRunMode
        ? `previewing ${isRetry ? "failed" : "queued"} webhook deliveries`
        : isRetry
          ? "retrying failed webhooks"
          : "delivering webhooks",
    );
    if (!dryRunMode) {
      setQueuePending(true);
    }
    try {
      const rows = await onProcess({
        limit: 50,
        status: deliveryStatus,
        dry_run: dryRunMode,
        confirmed: !dryRunMode,
      });
      if (dryRunMode) {
        onPreviewRows(rows);
        if (!openConfirmation) {
          onOpenDeliveries();
        }
        if (openConfirmation) {
          const previewHash = reviewedDeliveryHash(rows, "Webhook delivery");
          setQueueSnapshot({
            action: "process",
            request: {
              limit: 50,
              status: deliveryStatus,
              dry_run: false,
              confirmed: true,
              preview_hash: previewHash,
            },
            previewHash,
            reviewedRows: rows.length,
            eventLabel: isRetry ? "failed deliveries" : "queued deliveries",
          });
          setQueueConfirmation("process");
        }
      }
      setStatus(
        `${dryRunMode ? "previewed" : isRetry ? "retried" : "processed"} ${rows.length}`,
      );
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
    const snapshot = queueSnapshot;
    if (!snapshot || queuePending) {
      return;
    }
    setQueuePending(true);
    setStatus(
      snapshot.action === "dispatch"
        ? "queueing reviewed webhooks"
        : "delivering reviewed webhooks",
    );
    try {
      const rows =
        snapshot.action === "dispatch"
          ? await onDispatch(snapshot.request)
          : await onProcess(snapshot.request);
      setStatus(
        `${snapshot.action === "dispatch" ? "queued" : "processed"} ${rows.length}`,
      );
      clearWebhookQueueReview();
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "webhook queue action failed",
      );
    } finally {
      setQueuePending(false);
    }
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
      label: configurationQueue ? "Send test" : "Review rule",
      description: (rows) =>
        actionTargetDescription(
          configurationQueue ? "Send test for" : "Review",
          "webhook rule",
          rows[0]?.name,
          configurationQueue
            ? "Reviews a rule-scoped test event before queueing event webhook deliveries."
            : "Runs a dry-run with the current preview event.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Eye size={14} />,
      onSelect: (rows) =>
        rows[0] &&
        (configurationQueue
          ? void dispatch(true, true, rows[0])
          : void dryRun(rows[0])),
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
        {showRuleList ? (
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
        ) : null}
        {showRuleList && detailRuleId && !editorOpen ? (
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
                      if (configurationQueue) {
                        void dispatch(true, true, rule);
                      } else {
                        void dryRun(rule);
                      }
                    }
                  }}
                >
                  {configurationQueue ? "Send test" : "Review rule"}
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
                  {focusedEditorMode ? "Test" : "Review rule"}
                </button>
                <button
                  className="primaryAction"
                  type="button"
                  onClick={reviewSubmit}
                >
                  {focusedEditorMode
                    ? editingId
                      ? "Update rule"
                      : "Create rule"
                    : editingId
                      ? "Review update"
                      : "Review create"}
                </button>
                {!focusedEditorMode ? (
                  <button
                    className="secondaryAction"
                    type="button"
                    onClick={createRule}
                  >
                    New rule
                  </button>
                ) : null}
              </>
            }
            description={
              focusedEditorMode
                ? "Test the event match and rendered payload before saving the event webhook rule."
                : "Webhook rules are saved expression records with explicit preview and delivery operations."
            }
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
                  <span>
                    {focusedEditorMode
                      ? editingId
                        ? "Rule enabled"
                        : "Enable after creation"
                      : "Evaluate rule"}
                  </span>
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
              <ConsoleField
                label="Signing secret"
                className="fieldFull"
                hint={
                  existingSecretConfigured
                    ? "Secret is configured. Leave blank to keep it, type a new value to rotate, or clear it explicitly."
                    : "Optional HMAC secret. Deliveries include X-Vpsman-Webhook-Signature."
                }
              >
                <input
                  aria-label="Webhook signing secret"
                  autoComplete="new-password"
                  disabled={clearSigningSecret}
                  placeholder={
                    existingSecretConfigured
                      ? "Configured; leave blank to keep"
                      : "Optional HMAC secret"
                  }
                  type="password"
                  value={signingSecret}
                  onChange={(event) => setSigningSecret(event.target.value)}
                />
                {existingSecretConfigured ? (
                  <label className="checkLine inlineCheck">
                    <input
                      checked={clearSigningSecret}
                      onChange={(event) => {
                        setClearSigningSecret(event.target.checked);
                        if (event.target.checked) {
                          setSigningSecret("");
                        }
                      }}
                      type="checkbox"
                    />
                    <span>Clear existing signing secret</span>
                  </label>
                ) : null}
              </ConsoleField>
              <ConsoleField label="Preview event kind">
                <input
                  aria-label="Webhook event kind"
                  value={eventKind}
                  onChange={(event) => setWebhookEventKind(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Preview event id">
                <input
                  aria-label="Webhook event id"
                  value={eventId}
                  onChange={(event) => setWebhookEventId(event.target.value)}
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
              {focusedEditorMode ? (
                <ConsoleField label="Sample payload" className="fieldFull">
                  <WebhookRuleSamplePreview preview={editorTestPreview} />
                </ConsoleField>
              ) : null}
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
      {!focusedEditorOpen ? (
        <div className="consoleOperationsBar">
          <span>
            <strong>
              {configurationQueue ? "Event webhook tests" : "Webhook queue"}
            </strong>
            <small>
              {configurationQueue
                ? "Send reviewed test events and retry failed event webhook deliveries."
                : "Review first; retained deliveries stay in the Deliveries tab."}
            </small>
          </span>
          <div className="consoleOperationsActions">
            <label className="consoleField">
              <span>Event kind</span>
              <input
                aria-label="Webhook dispatch event kind"
                value={eventKind}
                onChange={(event) => setWebhookEventKind(event.target.value)}
              />
            </label>
            <label className="consoleField">
              <span>Event id</span>
              <input
                aria-label="Webhook dispatch event id"
                value={eventId}
                onChange={(event) => setWebhookEventId(event.target.value)}
                placeholder="optional"
              />
            </label>
            {configurationQueue ? (
              <>
                <button
                  className="secondaryAction"
                  disabled={queuePending}
                  type="button"
                  onClick={() => void dispatch(true)}
                >
                  Preview event
                </button>
                <button
                  className="primaryAction"
                  disabled={queuePending}
                  type="button"
                  onClick={() => void dispatch(true, true)}
                >
                  Send test
                </button>
                <button
                  className="secondaryAction"
                  disabled={queuePending}
                  type="button"
                  onClick={() => void process(true, true, "failed")}
                >
                  Retry failed
                </button>
              </>
            ) : (
              <>
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
                  onClick={() => void dispatch(true, true)}
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
                  onClick={() => void process(true, true)}
                >
                  Review delivery
                </button>
              </>
            )}
          </div>
        </div>
      ) : null}
      <ConfirmationPrompt
        confirmLabel={
          queueConfirmation === "dispatch"
            ? configurationQueue
              ? "Send test"
              : "Queue dispatch"
            : queueSnapshot?.action === "process" &&
                queueSnapshot.request.status === "failed"
              ? "Retry failed"
              : configurationQueue
                ? "Send queued"
                : "Deliver queued"
        }
        detail={
          queueConfirmation === "dispatch"
            ? configurationQueue
              ? "Queues reviewed event webhook test deliveries for matching rules and the selected event."
              : "Queues webhook delivery records for matching rules and the selected event."
            : queueSnapshot?.action === "process" &&
                queueSnapshot.request.status === "failed"
              ? "Retries failed event webhook delivery records and may call external webhook endpoints."
              : "Processes queued webhook delivery records and may call external webhook endpoints."
        }
        items={[
          {
            label: "Event",
            value: queueSnapshot?.eventLabel ?? "review required",
          },
          {
            label: "Reviewed rows",
            value: queueSnapshot?.reviewedRows ?? 0,
          },
          {
            label: "Review hash",
            value: queueSnapshot
              ? `${queueSnapshot.previewHash.slice(0, 12)}...`
              : "review required",
          },
        ]}
        onCancel={clearWebhookQueueReview}
        onConfirm={() => void confirmQueueAction()}
        open={queueConfirmation !== null && queueSnapshot !== null}
        pending={queuePending}
        title={
          queueConfirmation === "dispatch"
            ? configurationQueue
              ? "Confirm event webhook test"
              : "Confirm webhook queue dispatch"
            : queueSnapshot?.action === "process" &&
                queueSnapshot.request.status === "failed"
              ? "Confirm failed webhook retry"
              : "Confirm webhook delivery"
        }
        tone={queueConfirmation === "process" ? "danger" : "normal"}
      />
      {status && <small className="fleetPolicyStatus">{status}</small>}
      <ConfirmationPrompt
        confirmLabel={saveSnapshot?.title ?? "Save rule"}
        detail="Saves the reviewed webhook rule request exactly as shown."
        items={[
          { label: "Rule", value: saveSnapshot?.request.name ?? "-" },
          {
            label: "Expression",
            value: saveSnapshot?.request.expression ?? "-",
          },
          {
            label: "Target",
            value: saveSnapshot?.request.target ?? "-",
          },
          {
            label: "State",
            value: saveSnapshot?.request.enabled ? "enabled" : "disabled",
          },
          {
            label: "Signing",
            value: saveSnapshot
              ? saveSnapshot.request.clear_signing_secret
                ? "clear existing secret"
                : saveSnapshot.request.signing_secret
                  ? existingSecretConfigured
                    ? "rotate secret"
                    : "set secret"
                  : existingSecretConfigured
                    ? "keep existing secret"
                    : "not configured"
              : "-",
          },
        ]}
        onCancel={() => setSaveSnapshot(null)}
        onConfirm={() => void submit()}
        open={saveSnapshot !== null}
        pending={false}
        title="Confirm webhook rule save"
      />
      <ConfirmationPrompt
        confirmLabel="Delete webhook rules"
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

export function WebhookDryRunNotice({
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

function WebhookRuleSamplePreview({
  preview,
}: {
  preview: WebhookRuleDryRunRecord | null;
}) {
  if (!preview) {
    return (
      <div className="consoleInlineNotice">
        <strong>Test before saving</strong>
        <small>
          Run Test to render the body template, matched VPSs, and sample payload
          for the selected event.
        </small>
      </div>
    );
  }
  const matchedNames = preview.matched_vps
    .slice(0, 6)
    .map((agent) => formatVpsName(agent, "name"))
    .join(", ");
  const samplePayload = JSON.stringify(preview.payload_context, null, 2);
  return (
    <div className="webhookRuleSamplePreview">
      <div className="consoleInlineNotice">
        <strong>{preview.matched_vps.length} VPSs matched</strong>
        <small>{matchedNames || "No VPSs matched this test event."}</small>
        {preview.validation_errors.length > 0 ? (
          <small>{preview.validation_errors.join(" · ")}</small>
        ) : null}
      </div>
      <div className="webhookRuleSampleGrid">
        <div>
          <span>Rendered message</span>
          <strong>{preview.rendered_message || "No message rendered"}</strong>
        </div>
        <div>
          <span>Delivery status</span>
          <strong>{preview.delivery?.status ?? "dry run only"}</strong>
        </div>
      </div>
      <pre>{samplePayload}</pre>
    </div>
  );
}

export function WebhookDeliveryHistoryGrid({
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

export function WebhookDeliveryMaintenancePanel({
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

  function clearRotationReview() {
    setRotationPreview(null);
    setConfirmDelete(false);
    setRotationError(null);
  }

  async function rotate(confirmed: boolean) {
    if (rotationPending) {
      return;
    }
    if (confirmed && !rotationPreview) {
      setRotationError("Review rotation before confirming cleanup");
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
        preview_hash: confirmed ? rotationPreview?.preview_hash : null,
      });
      setRotationPreview(response);
      setConfirmDelete(false);
      setStatus(
        `${response.matched_count} matched / ${response.deleted_count} deleted`,
      );
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "rotation failed";
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
              onChange={(event) => {
                setRotationDays(event.target.value);
                clearRotationReview();
              }}
            />
          </ConsoleField>
          <ConsoleField label="Status">
            <select
              aria-label="Webhook rotation status"
              value={rotationStatus}
              onChange={(event) => {
                setRotationStatus(
                  event.target.value as WebhookRuleDeliveryHistoryStatus,
                );
                clearRotationReview();
              }}
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
              onChange={(event) => {
                setRotationRuleId(event.target.value);
                clearRotationReview();
              }}
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
          {
            label: "Review hash",
            value: rotationPreview
              ? `${rotationPreview.preview_hash.slice(0, 12)}...`
              : "review required",
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
  const [reviewSnapshot, setReviewSnapshot] = useState<{
    action: FleetAlertStateRequest["action"];
    requests: FleetAlertStateRequest[];
    rows: FleetAlertRecord[];
  } | null>(null);
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

  useEffect(() => {
    setReviewSnapshot(null);
  }, [alerts]);

  function reviewAlertUpdate(
    rows: FleetAlertRecord[],
    action: FleetAlertStateRequest["action"],
  ) {
    if (rows.length === 0 || pending) {
      return;
    }
    setReviewSnapshot({
      action,
      rows,
      requests: rows.map((alert) => ({
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
      })),
    });
  }

  async function updateReviewedAlerts() {
    const snapshot = reviewSnapshot;
    if (!snapshot || pending) {
      return;
    }
    setPending(
      `${snapshot.action}:${snapshot.rows.map((alert) => alert.id).join(",")}`,
    );
    try {
      for (const request of snapshot.requests) {
        await onUpdate(request);
      }
      setReviewSnapshot(null);
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
              reviewAlertUpdate(openRows(rows), "acknowledge"),
          },
          {
            label: "Mute open 4h",
            description: (rows) =>
              `Mute ${openRows(rows).length} selected open fleet alerts for four hours.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <VolumeX size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "mute"),
          },
          {
            label: "Escalate open",
            description: (rows) =>
              `Escalate ${openRows(rows).length} selected open fleet alerts.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "escalate"),
          },
          {
            label: "Clear triaged",
            description: (rows) =>
              `Clear ${triagedRows(rows).length} selected triaged fleet alerts.`,
            disabled: (rows) =>
              pending != null || triagedRows(rows).length === 0,
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => reviewAlertUpdate(triagedRows(rows), "clear"),
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
            onSelect: (rows) => reviewAlertUpdate(rows, "acknowledge"),
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
            onSelect: (rows) => reviewAlertUpdate(rows, "mute"),
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
            onSelect: (rows) => reviewAlertUpdate(rows, "escalate"),
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
            onSelect: (rows) => reviewAlertUpdate(rows, "clear"),
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
      <ConfirmationPrompt
        confirmLabel={fleetAlertActionLabel(reviewSnapshot?.action)}
        detail="Applies the reviewed operator state update to the selected fleet alerts."
        items={[
          {
            label: "Action",
            value: fleetAlertActionLabel(reviewSnapshot?.action),
          },
          {
            label: "Alerts",
            value: selectedRecordSummary(
              reviewSnapshot?.rows ?? null,
              "alert",
              "alerts",
              (row) => row.title,
              (row) => row.id,
            ),
          },
        ]}
        onCancel={() => setReviewSnapshot(null)}
        onConfirm={() => void updateReviewedAlerts()}
        open={reviewSnapshot !== null}
        pending={pending !== null}
        title="Confirm fleet alert triage"
        tone={reviewSnapshot?.action === "clear" ? "normal" : "danger"}
      />
    </div>
  );
}

function formatUnixTime(value: number): string {
  return formatCompactTime(new Date(value * 1000).toISOString());
}

function fleetAlertActionLabel(
  action: FleetAlertStateRequest["action"] | undefined,
): string {
  switch (action) {
    case "acknowledge":
      return "Acknowledge";
    case "mute":
      return "Mute";
    case "escalate":
      return "Escalate";
    case "clear":
      return "Clear";
    default:
      return "Confirm";
  }
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

function formatLoadCompact(value: number | undefined) {
  return typeof value === "number" ? value.toFixed(2) : "None";
}

function formatMemoryUsedCompact(
  rollup: TelemetryRollupRecord | null | undefined,
) {
  const ratio = memoryUsedRatio(rollup);
  return ratio === null ? "None" : `${Math.round(ratio)}%`;
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

function formatDiskFreeCompact(
  rollup: TelemetryRollupRecord | null | undefined,
) {
  const ratio = diskFreeRatio(rollup);
  return ratio === null ? "None" : `${Math.round(ratio)}% free`;
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

function formatRollupFreshness(
  rollup: TelemetryRollupRecord | null | undefined,
) {
  return rollup ? `Telemetry ${formatCompactTime(rollup.latest_observed_at)}` : "No telemetry";
}

function formatConsoleStreamState(wsState: string) {
  return `Console stream ${wsState || "unknown"}`;
}

function privilegeModeLabel(agent: AgentView) {
  if (agent.capabilities.privilege_mode === "root") {
    return "root";
  }
  if (agent.capabilities.privilege_mode === "unprivileged") {
    return "unprivileged";
  }
  return "privilege unknown";
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
    return "Server-managed runtime tunnels enabled";
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

function formatMetricValue(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) {
    return "-";
  }
  if (Math.abs(value) >= 1024) {
    return formatBytes(value);
  }
  return value % 1 === 0 ? String(value) : value.toFixed(2);
}

function formatPolicyWindow(windowSecs: number): string {
  if (windowSecs <= 0) {
    return "immediate";
  }
  if (windowSecs % 60 === 0) {
    return `${windowSecs / 60}m`;
  }
  return `${windowSecs}s`;
}

function formatSampleAge(seconds: number | null | undefined): string {
  if (seconds == null) {
    return "unknown";
  }
  if (seconds < 60) {
    return `${seconds}s`;
  }
  if (seconds < 3600) {
    return `${Math.round(seconds / 60)}m`;
  }
  return `${Math.round(seconds / 3600)}h`;
}

function trafficNowSummary(
  traffic: TrafficAccountingRecord | null | undefined,
): string {
  if (!traffic) {
    return "not configured";
  }
  if (!traffic.last_sample_at) {
    return traffic.state === "incomplete" ? "incomplete" : "stale sample";
  }
  return `${formatBytes(traffic.latest_total_bytes)} total · RX ${formatBytes(
    traffic.latest_rx_bytes,
  )} · TX ${formatBytes(traffic.latest_tx_bytes)}`;
}

function cycleUsageSummary(
  traffic: TrafficAccountingRecord | null | undefined,
): string {
  if (!traffic) {
    return "not configured";
  }
  if (traffic.cycle_percent == null) {
    return traffic.state === "incomplete"
      ? "incomplete"
      : formatBytes(traffic.total_bytes);
  }
  const quotaLabel =
    traffic.quota_total_bytes != null
      ? `${formatBytes(traffic.total_bytes)} / ${formatBytes(
          traffic.quota_total_bytes,
        )}`
      : traffic.quota_tx_bytes != null && traffic.tx_bytes >= traffic.rx_bytes
        ? `${formatBytes(traffic.tx_bytes)} TX / ${formatBytes(
            traffic.quota_tx_bytes,
          )} TX`
        : traffic.quota_rx_bytes != null
          ? `${formatBytes(traffic.rx_bytes)} RX / ${formatBytes(
              traffic.quota_rx_bytes,
            )} RX`
          : formatBytes(traffic.total_bytes);
  return `${quotaLabel} · ${traffic.cycle_percent.toFixed(0)}%`;
}

function trafficStateForClient(
  traffic: TrafficAccountingRecord | null | undefined,
  alerts?: PolicyAlertRecord[] | null,
): string {
  const activeAlerts = alerts ?? [];
  if (activeAlerts.some((alert) => alert.severity === "critical")) {
    return "critical";
  }
  if (activeAlerts.some((alert) => alert.severity === "warning")) {
    return "warning";
  }
  if (!traffic) {
    return "incomplete";
  }
  if (!traffic.last_sample_at) {
    return "unknown";
  }
  if (traffic.state === "incomplete" || traffic.incomplete_reasons.length > 0) {
    return "incomplete";
  }
  return traffic.state || "ok";
}

function trafficStateTone(
  state: string,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  if (state === "critical") {
    return "critical";
  }
  if (state === "warning" || state === "incomplete") {
    return "warning";
  }
  if (state === "ok") {
    return "ok";
  }
  return "neutral";
}

function quotaSummary(
  traffic: TrafficAccountingRecord | null | undefined,
): string {
  if (!traffic) {
    return "not set";
  }
  const parts = [
    traffic.quota_total_bytes == null
      ? null
      : `total ${formatBytes(traffic.quota_total_bytes)}`,
    traffic.quota_rx_bytes == null
      ? null
      : `rx ${formatBytes(traffic.quota_rx_bytes)}`,
    traffic.quota_tx_bytes == null
      ? null
      : `tx ${formatBytes(traffic.quota_tx_bytes)}`,
  ].filter((part): part is string => Boolean(part));
  return parts.length > 0 ? parts.join(" · ") : "not set";
}

function resetDaySummary(
  traffic: TrafficAccountingRecord | null | undefined,
): string {
  if (!traffic?.reset_day) {
    return "not set";
  }
  return traffic.reset_day === 31
    ? "31 UTC, clamps short months"
    : `${traffic.reset_day} UTC`;
}

function selectorSummary(
  traffic: TrafficAccountingRecord | null | undefined,
): string {
  if (!traffic || traffic.selectors.length === 0) {
    return "not set";
  }
  return traffic.selectors.join(", ");
}

function activePolicyAlertSummary(
  alerts: PolicyAlertRecord[] | null | undefined,
): string {
  const rows = alerts ?? [];
  if (rows.length === 0) {
    return "0";
  }
  const critical = rows.filter((alert) => alert.severity === "critical").length;
  if (critical > 0) {
    return `${critical} critical`;
  }
  const warning = rows.filter((alert) => alert.severity === "warning").length;
  if (warning > 0) {
    return `${warning} warning`;
  }
  return `${rows.length} info`;
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
  metasource_templates?: string[];
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
              Open Privilege Vault
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
        <span>
          {rates.length} latest interface rate bucket
          {rates.length === 1 ? "" : "s"}
        </span>
        <div className="networkInterfaceList">
          {rates
            .slice()
            .sort((left, right) =>
              left.interface.localeCompare(right.interface),
            )
            .map((rate) => (
              <div
                className="networkInterfaceRow telemetryInterfaceRow"
                key={rate.interface}
              >
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
        <span>
          {tunnels.length} latest tunnel report{tunnels.length === 1 ? "" : "s"}
        </span>
        <div className="networkInterfaceList">
          {tunnels
            .slice()
            .sort((left, right) =>
              left.interface.localeCompare(right.interface),
            )
            .map((tunnel) => (
              <div
                className={`networkInterfaceRow telemetryTunnelRow ${tunnelRowClass(tunnel)}`}
                key={tunnel.interface}
              >
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
  if (
    tunnel.latency_status === "down" ||
    tunnel.auto_ospf_status === "failed"
  ) {
    return "telemetryRowCritical";
  }
  if (
    tunnel.latency_status === "missed" ||
    tunnel.auto_ospf_status === "report_only"
  ) {
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
  const metric =
    typeof tunnel.latency_avg_ms === "number"
      ? ` / ${tunnel.latency_avg_ms.toFixed(1)} ms`
      : "";
  const loss =
    typeof tunnel.packet_loss_ratio === "number"
      ? ` / ${(tunnel.packet_loss_ratio * 100).toFixed(1)}% loss`
      : "";
  return `Latency ${status}${metric}${loss}`;
}

function formatTunnelLatencyDetail(tunnel: TelemetryTunnelRecord): string {
  const checked =
    typeof tunnel.latency_checked_unix === "number"
      ? formatCompactTime(
          new Date(tunnel.latency_checked_unix * 1000).toISOString(),
        )
      : "not checked";
  const windows = [
    typeof tunnel.latency_healthy_windows === "number"
      ? `ok ${tunnel.latency_healthy_windows}`
      : "",
    typeof tunnel.latency_missed_windows === "number"
      ? `miss ${tunnel.latency_missed_windows}`
      : "",
  ]
    .filter(Boolean)
    .join(", ");
  return [
    addressFamilyLabel(tunnel.latency_primary_family),
    tunnel.latency_target ?? "target n/a",
    `checked ${checked}`,
    windows || "windows n/a",
    telemetryReasonLabel(tunnel.latency_reason),
  ]
    .filter(Boolean)
    .join("; ");
}

function formatTunnelOspfMain(tunnel: TelemetryTunnelRecord): string {
  const status = ospfStatusLabel(
    tunnel.auto_ospf_status,
    tunnel.auto_ospf_enabled,
  );
  const cost =
    tunnel.auto_ospf_current_cost || tunnel.auto_ospf_recommended_cost
      ? ` ${tunnel.auto_ospf_current_cost ?? "?"}->${tunnel.auto_ospf_recommended_cost ?? "?"}`
      : "";
  return `OSPF ${status}${cost}`;
}

function formatTunnelOspfDetail(tunnel: TelemetryTunnelRecord): string {
  const enabled = tunnel.auto_ospf_enabled ? "enabled" : "disabled";
  const updated =
    typeof tunnel.auto_ospf_updated_unix === "number"
      ? `updated ${formatCompactTime(new Date(tunnel.auto_ospf_updated_unix * 1000).toISOString())}`
      : "no update";
  return [enabled, updated, telemetryReasonLabel(tunnel.auto_ospf_reason)]
    .filter(Boolean)
    .join("; ");
}

function latencyTone(
  status: string | null | undefined,
): "critical" | "neutral" | "ok" | "warn" {
  if (status === "down") {
    return "critical";
  }
  if (
    status === "missed" ||
    status === "unconfigured" ||
    status === "disabled"
  ) {
    return "warn";
  }
  if (status === "healthy") {
    return "ok";
  }
  return "neutral";
}

function ospfTone(
  status: string | null | undefined,
): "critical" | "neutral" | "ok" | "warn" {
  if (status === "failed") {
    return "critical";
  }
  if (
    status === "report_only" ||
    status === "stabilizing" ||
    status === "monitoring_only"
  ) {
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
    const manager = runtimeManagerLabel(
      tunnel.plan_runtime_manager ?? tunnel.ownership_mode,
    );
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
  const reason =
    telemetryReasonLabel(health.reason) || readableAdapterStatus(health.status);
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
