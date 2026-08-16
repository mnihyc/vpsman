import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { basicSetup, EditorView } from "codemirror";
import {
  Activity,
  ArrowUpCircle,
  Bell,
  Boxes,
  Clock3,
  DatabaseBackup,
  Eye,
  FileCog,
  FolderOpen,
  Gauge,
  LockKeyhole,
  MapPin,
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
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { handleTabListKeyDown, tabId } from "../components/AccessibleTabs";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { CountryBadge } from "../components/CountryFlag";
import { ConsoleDetailPanel } from "../components/ConsoleDetailPanel";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import {
  FLEET_DETAIL_LIMIT,
  FLEET_TELEMETRY_SNAPSHOT_LIMIT,
  formatLowerBoundCount,
} from "../constants";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
import { useProjectedProductName } from "../hooks/useProjectedProductName";
import { WEBHOOK_RULE_DELIVERY_HISTORY_STATUSES } from "../generated/protocolContracts";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import {
  ExecutionResultPanel,
  FailureReasonGroups,
} from "../components/ExecutionResultPanel";
import { Metric } from "../components/Metric";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE,
  vpsRuleSearchUnavailable,
} from "../searchExpression";
import { useVpsRuleSearchContext } from "../vpsRuleSearchContext";
import {
  useByteCountFormatter,
  useByteRateFormatter,
  usePanelDisplaySettings,
  type ByteCountFormatter,
  type ByteRateFormatter,
} from "../panelDisplay";
import { scrollIntoViewWithMotion } from "../motion";
import { formatUptime } from "../telemetryMetrics";
import {
  resolveNetworkRateInterfaces,
  selectedNetworkRates,
  type NetworkRateInterfaceResolution,
} from "../networkRateSelection";
import {
  addressFamilyLabel,
  latencyStatusLabel,
  mutationPolicyLabel,
  runtimeManagerLabel,
  telemetryReasonLabel,
  telemetrySourceLabel,
  trafficStatusLabel,
} from "../topologyRuntime";
import {
  buildTagDisplayOrder,
  compareTagsByDisplayOrder,
  countryTagValue,
  displayFleetTags as displayTags,
  isProviderTag,
  regionTagValue,
  sortTagsByDisplayOrder,
  type TagDisplayOrder,
} from "../tagDisplay";
import { LocalTargetPreview } from "./TargetImpactPreview";
import {
  buildPrivilegeAssertion,
  buildPrivilegeForJobOperation,
  canonicalDbPrivilegeIntent,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../privilege";
import { selectorExpressionForClientIds } from "../searchExpression";
import { WEBHOOK_EXPRESSION_SUGGESTIONS } from "../webhookExpressionSuggestions";
import { productNameFromVpsRules, providerProductLabel } from "../vpsRules";
import {
  decodeOutputPreview,
  dispatchFailureReason,
  formatCompactTime,
  formatTime,
  formatVpsName,
  lifecycleOutcomeFailureReason,
  runPanelAction,
  shortId,
  trafficLimitingQuota,
  trafficQuotaState,
  trafficUnlimitedQuota,
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
  ConfigurationSourceView,
  EffectiveAgentConfigResponse,
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
  DeleteAgentBatchOutcome,
  DeleteAgentBatchTarget,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
  TelemetryUptimeRecord,
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

type FleetMutationTargetSnapshot = {
  agent: AgentView;
  clientId: string;
  displayName: string;
  status: string;
};

type DeleteAgentConfirmationSnapshot = {
  targets: Array<
    FleetMutationTargetSnapshot & {
      privilegeAssertion: PrivilegeAssertion;
    }
  >;
};

type AgentLifecycleAction = "stop" | "restart";

type AgentLifecycleConfirmationSnapshot = {
  action: AgentLifecycleAction;
  targets: FleetMutationTargetSnapshot[];
  selectorExpression: string;
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
      reviewedRules: string;
      reviewedTargets: string;
    }
  | {
      action: "process";
      request: WebhookRuleProcessRequest;
      previewHash: string;
      reviewedRows: number;
      eventLabel: string;
      reviewedRules: string;
      reviewedTargets: string;
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
  { id: "capabilities", label: "Capabilities" },
];

const JOB_SELECTOR_STORAGE_KEY = "vpsman.jobDispatch.selectorExpression";
const MULTI_FILE_SELECTOR_STORAGE_KEY = "vpsman.multiFile.selectorExpression";
const TAG_BULK_SELECTOR_STORAGE_KEY = "vpsman.tags.bulk.selectorExpression";
const CONFIG_BULK_SELECTOR_STORAGE_KEY =
  "vpsman.config.bulk.selectorExpression";
const FILE_BROWSER_STATE_STORAGE_KEY = "vpsman.fileBrowser.state";
const AGENT_LIFECYCLE_MAX_TIMEOUT_SECS = 120;
export function FleetWorkspace({
  activeSubpage,
  agents,
  apiToken,
  apiError,
  fleetCoreEvidenceAvailable,
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
  configurationSources,
  onCreateJob,
  onBulkMutateTags,
  onNavigatePanel,
  onRegisterVps,
  onOpenJobDispatchPreset,
  onLoadEffectiveAgentConfig,
  onLoadConfigurationSources,
  onDeleteFleetAlertNotificationChannel,
  onDeleteFleetAlertPolicy,
  onDeleteWebhookRule,
  onDispatchFleetAlertNotifications,
  onDispatchWebhookRules,
  onDryRunFleetAlertPolicy,
  onDryRunWebhookRule,
  onDeleteAgents,
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
  telemetryUptimes,
  privilegeMaterial,
  wsState,
}: {
  activeSubpage: string;
  agents: AgentView[];
  apiToken: string;
  apiError: string | null;
  fleetCoreEvidenceAvailable: boolean;
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
  configurationSources: ConfigurationSourceView[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onBulkMutateTags: (
    request: BulkTagMutationRequest,
  ) => Promise<TagMutationResponse>;
  onNavigatePanel?: (
    view: ActiveView,
    subpage: string,
    targetClientId?: string,
  ) => void;
  onRegisterVps?: () => void;
  onOpenJobDispatchPreset: (preset: JobDispatchPresetInput) => void;
  onLoadEffectiveAgentConfig: (
    clientId: string,
  ) => Promise<EffectiveAgentConfigResponse>;
  onLoadConfigurationSources: () => Promise<void>;
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
  onDeleteAgents: (
    targets: DeleteAgentBatchTarget[],
  ) => Promise<DeleteAgentBatchOutcome[]>;
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
  telemetryUptimes: TelemetryUptimeRecord[];
  privilegeMaterial: PrivilegeMaterial | null;
  wsState: string;
}) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const formatBytes = useByteCountFormatter();
  const fleetAlertPoliciesTruncated =
    fleetAlertPolicies.length >= FLEET_DETAIL_LIMIT;
  const policyAlertsTruncated = policyAlerts.length >= FLEET_DETAIL_LIMIT;
  const trafficAccountingTruncated =
    trafficAccounting.length >= FLEET_DETAIL_LIMIT;
  const notificationChannelsTruncated =
    fleetAlertNotificationChannels.length >= FLEET_DETAIL_LIMIT;
  const alertNotificationsTruncated =
    fleetAlertNotifications.length >= FLEET_DETAIL_LIMIT;
  const webhookRulesTruncated = webhookRules.length >= FLEET_DETAIL_LIMIT;
  const webhookDeliveriesTruncated =
    webhookRuleDeliveries.length >= FLEET_DETAIL_LIMIT;
  // Rule rows are a bounded configuration source (ten keys per VPS) and the
  // full fleet snapshot now returns them without the telemetry row cap.
  const vpsRuleValuesTruncated = false;
  const telemetryRollupsTruncated =
    telemetryRollups.length >= FLEET_TELEMETRY_SNAPSHOT_LIMIT;
  const telemetryNetworkRatesTruncated =
    telemetryNetworkRates.length >= FLEET_TELEMETRY_SNAPSHOT_LIMIT;
  const telemetryTunnelsTruncated =
    telemetryTunnels.length >= FLEET_TELEMETRY_SNAPSHOT_LIMIT;
  const telemetryTruncated =
    telemetryRollupsTruncated ||
    telemetryNetworkRatesTruncated ||
    telemetryTunnelsTruncated;
  const fleetLoadBoundaryLabels = [
    fleetAlertPoliciesTruncated ? "alert policies" : null,
    policyAlertsTruncated ? "policy alerts" : null,
    trafficAccountingTruncated ? "traffic accounting" : null,
    notificationChannelsTruncated ? "notification channels" : null,
    alertNotificationsTruncated ? "notification deliveries" : null,
    webhookRulesTruncated ? "webhook rules" : null,
    webhookDeliveriesTruncated ? "webhook deliveries" : null,
    vpsRuleValuesTruncated ? "VPS rules" : null,
    telemetryTruncated ? "telemetry" : null,
  ].filter((label): label is string => label !== null);
  const [selectionStatsMode, setSelectionStatsMode] =
    useState<FleetSelectionStatsMode>("telemetry");
  const [deleteSnapshot, setDeleteSnapshot] =
    useState<DeleteAgentConfirmationSnapshot | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteReviewPending, setDeleteReviewPending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [deleteFeedback, setDeleteFeedback] = useState<{
    message: string;
    tone: ActionFeedbackTone;
  } | null>(null);
  const deleteReviewTargetRef = useRef<string | null>(null);
  const deleteSnapshotRef = useRef<DeleteAgentConfirmationSnapshot | null>(
    null,
  );
  const deleteReviewPendingRef = useRef(false);
  const [lifecycleSnapshot, setLifecycleSnapshot] =
    useState<AgentLifecycleConfirmationSnapshot | null>(null);
  const [lifecyclePending, setLifecyclePending] = useState(false);
  const [lifecycleReviewPending, setLifecycleReviewPending] = useState(false);
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);
  const [lifecycleProgress, setLifecycleProgress] =
    useState<BulkJobProgress | null>(null);
  const [lifecycleResultAction, setLifecycleResultAction] =
    useState<AgentLifecycleAction | null>(null);
  const lifecycleReviewTargetRef = useRef<string | null>(null);
  const lifecycleSnapshotRef =
    useRef<AgentLifecycleConfirmationSnapshot | null>(null);
  const lifecycleReviewPendingRef = useRef(false);
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
  const uptimeByClient = useMemo(
    () => new Map(telemetryUptimes.map((record) => [record.client_id, record])),
    [telemetryUptimes],
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
  const selectedLatestNetworkRates = useMemo(() => {
    const selected = new Map<string, TelemetryNetworkRateRecord[]>();
    for (const [clientId, rates] of latestNetworkRates) {
      selected.set(
        clientId,
        selectedNetworkRates(rates, vpsRulesByClient.get(clientId) ?? []),
      );
    }
    return selected;
  }, [latestNetworkRates, vpsRulesByClient]);
  const networkRateSelectionsByClient = useMemo(() => {
    const selected = new Map<string, NetworkRateInterfaceResolution>();
    for (const agent of agents) {
      selected.set(
        agent.id,
        resolveNetworkRateInterfaces(vpsRulesByClient.get(agent.id) ?? []),
      );
    }
    return selected;
  }, [agents, vpsRulesByClient]);
  const policyAlertsByClient = useMemo(() => {
    const map = new Map<string, PolicyAlertRecord[]>();
    for (const alert of policyAlerts) {
      const rows = map.get(alert.client_id) ?? [];
      rows.push(alert);
      map.set(alert.client_id, rows);
    }
    return map;
  }, [policyAlerts]);
  const latestRollupsRef = useRef(latestRollups);
  const trafficByClientRef = useRef(trafficByClient);
  const policyAlertsByClientRef = useRef(policyAlertsByClient);
  const uptimeByClientRef = useRef(uptimeByClient);
  latestRollupsRef.current = latestRollups;
  trafficByClientRef.current = trafficByClient;
  policyAlertsByClientRef.current = policyAlertsByClient;
  uptimeByClientRef.current = uptimeByClient;
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
  const clearLifecycleReview = useCallback(() => {
    lifecycleReviewTargetRef.current = null;
    invalidateReviewGeneration();
    setLifecycleSnapshot(null);
    setLifecycleReviewPending(false);
  }, [invalidateReviewGeneration]);
  const clearFleetMutationReviews = useCallback(() => {
    deleteReviewTargetRef.current = null;
    lifecycleReviewTargetRef.current = null;
    invalidateReviewGeneration();
    setDeleteSnapshot(null);
    setDeleteReviewPending(false);
    setLifecycleSnapshot(null);
    setLifecycleReviewPending(false);
  }, [invalidateReviewGeneration]);
  const ignoreRequestedFleetDetailTab = useCallback(() => {}, []);

  useEffect(() => {
    deleteSnapshotRef.current = deleteSnapshot;
  }, [deleteSnapshot]);

  useEffect(() => {
    deleteReviewPendingRef.current = deleteReviewPending;
  }, [deleteReviewPending]);

  useEffect(() => {
    lifecycleSnapshotRef.current = lifecycleSnapshot;
  }, [lifecycleSnapshot]);

  useEffect(() => {
    lifecycleReviewPendingRef.current = lifecycleReviewPending;
  }, [lifecycleReviewPending]);

  useEffect(() => {
    clearFleetMutationReviews();
  }, [activeSubpage, selectedAgent?.id, clearFleetMutationReviews]);

  const handleFleetSelectionChange = useCallback(
    (rows: AgentView[]) => {
      const currentSignature = fleetSelectionSignature(rows);
      const reviewedDeleteSignature =
        deleteSnapshotRef.current !== null
          ? fleetTargetSignature(deleteSnapshotRef.current.targets)
          : deleteReviewTargetRef.current;
      if (
        (reviewedDeleteSignature || deleteReviewPendingRef.current) &&
        currentSignature !== reviewedDeleteSignature
      ) {
        clearDeleteReview();
      }
      const reviewedLifecycleSignature =
        lifecycleSnapshotRef.current !== null
          ? fleetTargetSignature(lifecycleSnapshotRef.current.targets)
          : lifecycleReviewTargetRef.current;
      if (
        (reviewedLifecycleSignature || lifecycleReviewPendingRef.current) &&
        currentSignature !== reviewedLifecycleSignature
      ) {
        clearLifecycleReview();
      }
    },
    [clearDeleteReview, clearLifecycleReview],
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
              <strong
                className="truncateValue"
                title={formatVpsName(agent, vpsNameDisplayMode)}
              >
                {formatVpsName(agent, vpsNameDisplayMode)}
              </strong>
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
        id: "uptime",
        header: "Uptime",
        size: 112,
        minSize: 96,
        sortValue: (agent) =>
          uptimeByClientRef.current.get(agent.id)?.uptime_secs ?? -1,
        searchValue: (agent) => {
          const uptime = uptimeByClientRef.current.get(agent.id);
          return uptime
            ? `${formatUptime(uptime.uptime_secs)} ${formatCompactTime(uptime.observed_at)}`
            : "";
        },
        cell: (agent) => {
          const uptime = uptimeByClientRef.current.get(agent.id);
          const uptimeTitle = uptime
            ? `Agent-reported uptime ${formatUptime(uptime.uptime_secs)}; sampled ${formatCompactTime(uptime.observed_at)}`
            : "Agent-reported uptime is unavailable";
          return (
            <span
              className="historyPrimary"
              data-tooltip-empty-reason={
                uptime ? undefined : "Agent-reported uptime is unavailable"
              }
              title={uptimeTitle}
            >
              <strong title={uptimeTitle}>
                {formatUptime(uptime?.uptime_secs)}
              </strong>
              {uptime ? (
                <small
                  title={`Uptime sample time ${formatCompactTime(uptime.observed_at)}`}
                >{`Observed ${formatCompactTime(uptime.observed_at)}`}</small>
              ) : null}
            </span>
          );
        },
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
        header: "Location",
        size: 110,
        minSize: 90,
        sortValue: (agent) =>
          fleetLocationSortValue(
            agent.tags,
            preferences.fleet_location_display_mode === "country_region",
          ),
        searchValue: (agent) => fleetLocationSearchValue(agent.tags),
        cell: (agent) => (
          <FleetLocationValue
            showRegion={
              preferences.fleet_location_display_mode === "country_region"
            }
            showFlag={preferences.show_country_flags}
            tags={agent.tags}
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
          trafficByClientRef.current.get(agent.id)?.latest_total_bytes ?? -1,
        searchValue: (agent) =>
          trafficNowSummary(
            trafficByClientRef.current.get(agent.id),
            formatBytes,
          ),
        cell: (agent) =>
          trafficNowSummary(
            trafficByClientRef.current.get(agent.id),
            formatBytes,
          ),
      },
      {
        id: "cycle_usage",
        header: "Cycle Usage",
        size: 210,
        minSize: 160,
        sortValue: (agent) =>
          trafficByClientRef.current.get(agent.id)?.cycle_percent ?? -1,
        searchValue: (agent) =>
          cycleUsageSummary(
            trafficByClientRef.current.get(agent.id),
            formatBytes,
          ),
        cell: (agent) =>
          cycleUsageSummary(
            trafficByClientRef.current.get(agent.id),
            formatBytes,
          ),
      },
      {
        id: "traffic_state",
        header: "Traffic State",
        size: 130,
        minSize: 110,
        sortValue: (agent) =>
          trafficStateForClient(
            trafficByClientRef.current.get(agent.id),
            policyAlertsByClientRef.current.get(agent.id),
          ),
        searchValue: (agent) =>
          trafficStateForClient(
            trafficByClientRef.current.get(agent.id),
            policyAlertsByClientRef.current.get(agent.id),
          ),
        cell: (agent) => {
          const state = trafficStateForClient(
            trafficByClientRef.current.get(agent.id),
            policyAlertsByClientRef.current.get(agent.id),
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
        searchValue: (agent) =>
          quotaSummary(trafficByClientRef.current.get(agent.id), formatBytes),
        cell: (agent) =>
          quotaSummary(trafficByClientRef.current.get(agent.id), formatBytes),
      },
      {
        id: "reset_day",
        header: "Reset Day",
        size: 125,
        minSize: 105,
        sortValue: (agent) =>
          trafficByClientRef.current.get(agent.id)?.reset_day ?? 0,
        searchValue: (agent) =>
          resetDaySummary(trafficByClientRef.current.get(agent.id)),
        cell: (agent) =>
          resetDaySummary(trafficByClientRef.current.get(agent.id)),
      },
      {
        id: "selectors",
        header: "Selectors",
        size: 230,
        minSize: 160,
        searchValue: (agent) =>
          selectorSummary(trafficByClientRef.current.get(agent.id)),
        cell: (agent) => (
          <span className="monoValue">
            {selectorSummary(trafficByClientRef.current.get(agent.id))}
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
            <strong>{agent.internal_build_number ?? "Unknown"}</strong>
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
          latestRollupsRef.current.get(agent.id)?.cpu_load_1_avg ?? -1,
        searchValue: (agent) =>
          formatLoad(latestRollupsRef.current.get(agent.id)?.cpu_load_1_avg),
        cell: (agent) => {
          const rollup = latestRollupsRef.current.get(agent.id);
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
        sortValue: (agent) =>
          memoryUsedRatio(latestRollupsRef.current.get(agent.id)) ?? -1,
        searchValue: (agent) =>
          formatMemoryUsed(latestRollupsRef.current.get(agent.id), formatBytes),
        cell: (agent) => {
          const rollup = latestRollupsRef.current.get(agent.id);
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
        sortValue: (agent) =>
          diskUsedRatio(latestRollupsRef.current.get(agent.id)) ?? -1,
        searchValue: (agent) =>
          formatDiskUsed(latestRollupsRef.current.get(agent.id), formatBytes),
        cell: (agent) => {
          const rollup = latestRollupsRef.current.get(agent.id);
          return (
            <span className="historyPrimary">
              <strong>{formatDiskUsedCompact(rollup)}</strong>
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
        sortValue: (agent) =>
          policyAlertsByClientRef.current.get(agent.id)?.length ?? 0,
        searchValue: (agent) =>
          activePolicyAlertSummary(
            policyAlertsByClientRef.current.get(agent.id),
          ),
        cell: (agent) =>
          activePolicyAlertSummary(
            policyAlertsByClientRef.current.get(agent.id),
          ),
      },
    ],
    [
      preferences.fleet_tag_visibility_overrides,
      preferences.show_country_flags,
      preferences.fleet_location_display_mode,
      formatBytes,
      tagDisplayOrder,
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
    const preview = await onBulkMutateTags({
      action,
      confirmed: false,
      privilege_assertion: null,
      selector_expression: selectorExpression,
      target_client_ids: targetIds,
      tag: normalizedTag,
    });
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
      preview_hash: preview.preview_hash,
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
    clearFleetMutationReviews();
    const selectorExpression = selectorExpressionForClientIds(
      rows.map((agent) => agent.id),
    );
    writeLocalString(storageKey, selectorExpression);
    if (rows.length === 1) {
      onSelectAgent(rows[0].id);
    }
    onNavigatePanel?.(
      view,
      subpage,
      rows.length === 1 ? rows[0].id : undefined,
    );
  }

  function openFileBrowserWorkflow(rows: AgentView[]) {
    clearFleetMutationReviews();
    if (rows.length !== 1) {
      return;
    }
    seedSingleFileBrowser(rows[0]);
    onNavigatePanel?.("Remote Operations", "files", rows[0].id);
  }

  function openSingleReleaseWorkflow(
    rows: AgentView[],
    view: ActiveView,
    subpage: string,
  ) {
    clearFleetMutationReviews();
    if (rows.length !== 1) {
      return;
    }
    onSelectAgent(rows[0].id);
    onNavigatePanel?.(view, subpage, rows[0].id);
  }

  function openUpdateCheckWorkflow(rows: AgentView[]) {
    clearFleetMutationReviews();
    onOpenJobDispatchPreset({
      mode: "agent_update_check",
      selectorExpression: selectorExpressionForClientIds(
        rows.map((agent) => agent.id),
      ),
      maxTimeoutSecs: 300,
      updateCheckVersionUrl: DEFAULT_UPDATE_VERSION_URL,
    });
  }

  async function requestDeleteAgent(rows: AgentView[]) {
    clearFleetMutationReviews();
    setLifecycleError(null);
    setDeleteError(null);
    setDeleteFeedback(null);
    if (rows.length === 0) {
      return;
    }
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      setDeleteError("Privilege unlock is required");
      return;
    }
    const targets = fleetMutationTargets(rows, vpsNameDisplayMode);
    deleteReviewTargetRef.current = fleetTargetSignature(targets);
    const reviewGeneration = captureReviewGeneration();
    setDeleteReviewPending(true);
    try {
      await waitForReviewRender();
      const reviewedTargets = await Promise.all(
        targets.map(async (target) => ({
          ...target,
          privilegeAssertion: await buildPrivilegeAssertion({
            intent: canonicalDbPrivilegeIntent({
              action: "agent.delete",
              confirmed: true,
              resolvedTargets: [target.clientId],
              target: target.clientId,
            }),
            privilegeMaterial,
          }),
        })),
      );
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setDeleteError(null);
      setDeleteSnapshot({ targets: reviewedTargets });
    } catch (error) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setDeleteError(error instanceof Error ? error.message : String(error));
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setDeleteReviewPending(false);
      }
    }
  }

  async function requestAgentLifecycle(
    rows: AgentView[],
    action: AgentLifecycleAction,
  ) {
    clearFleetMutationReviews();
    setDeleteError(null);
    setDeleteFeedback(null);
    setLifecycleError(null);
    setLifecycleProgress(null);
    setLifecycleResultAction(null);
    if (rows.length === 0) {
      return;
    }
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      setLifecycleError("Privilege unlock is required");
      return;
    }
    const targets = fleetMutationTargets(rows, vpsNameDisplayMode);
    const targetIds = targets.map((target) => target.clientId);
    const selectorExpression = selectorExpressionForClientIds(targetIds);
    const operation: JobOperation = {
      type: action === "stop" ? "agent_stop" : "agent_restart",
    };
    lifecycleReviewTargetRef.current = fleetTargetSignature(targets);
    const reviewGeneration = captureReviewGeneration();
    setLifecycleReviewPending(true);
    try {
      await waitForReviewRender();
      const builtPrivilege = await buildPrivilegeForJobOperation({
        clientIds: targetIds,
        commandType: operation.type,
        operation,
        privilegeMaterial,
        selectorExpression,
        maxTimeoutSecs: AGENT_LIFECYCLE_MAX_TIMEOUT_SECS,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setLifecycleSnapshot({
        action,
        targets,
        selectorExpression,
        privilegeAssertion: builtPrivilege.privilegeAssertion,
      });
    } catch (error) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setLifecycleError(error instanceof Error ? error.message : String(error));
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setLifecycleReviewPending(false);
      }
    }
  }

  async function confirmAgentLifecycle() {
    if (!lifecycleSnapshot) {
      return;
    }
    const snapshot = lifecycleSnapshot;
    await runPanelAction(setLifecyclePending, setLifecycleError, async () => {
      const targetIds = snapshot.targets.map((target) => target.clientId);
      const operation: JobOperation = {
        type: snapshot.action === "stop" ? "agent_stop" : "agent_restart",
      };
      setLifecycleResultAction(snapshot.action);
      const job = await onCreateJob({
        argv: [],
        selector_expression: snapshot.selectorExpression,
        target_client_ids: targetIds,
        command: operation.type,
        confirmed: true,
        destructive: true,
        job_id: crypto.randomUUID(),
        operation,
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: snapshot.privilegeAssertion,
        max_timeout_secs: AGENT_LIFECYCLE_MAX_TIMEOUT_SECS,
      });
      const targets = rowsForFleetMutationTargets(snapshot.targets);
      setLifecycleProgress(
        buildBulkJobProgress({
          jobId: job.job_id,
          targetCount: createJobTargetCount(job),
          targetRecords: [],
          targets,
          maxTimeoutSecs: AGENT_LIFECYCLE_MAX_TIMEOUT_SECS,
        }),
      );
      clearLifecycleReview();
      const result = await waitForBulkJobTargets(job.job_id, onLoadJobTargets, {
        onProgress: setLifecycleProgress,
        targetCount: createJobTargetCount(job),
        targets,
        maxTimeoutSecs: AGENT_LIFECYCLE_MAX_TIMEOUT_SECS,
        onLoadOutputs: onLoadJobOutputs,
      });
      setLifecycleProgress(result.progress);
    });
  }

  async function confirmDeleteAgent() {
    if (!deleteSnapshot) {
      return;
    }
    await runPanelAction(setDeletePending, setDeleteError, async () => {
      const outcomes = await onDeleteAgents(
        deleteSnapshot.targets.map((target) => ({
          client_id: target.clientId,
          request: {
            confirmed: true,
            privilege_assertion: target.privilegeAssertion,
            reason: "Deleted from fleet inventory selection action",
          },
        })),
      );
      const completed = outcomes.filter(
        (outcome) => outcome.response?.deleted === true,
      );
      const failedRequests = outcomes.filter(
        (outcome) => outcome.response?.deleted !== true,
      );
      const queuedSyncs = completed.flatMap((outcome) =>
        outcome.response!.runtime_sync.filter(
          (runtimeOutcome) => runtimeOutcome.status === "queued",
        ),
      );
      const failureReasons = [
        ...failedRequests.map(
          (outcome) =>
            `${fleetTargetLabel(deleteSnapshot.targets, outcome.client_id)}: ${outcome.error ?? "the API did not confirm deletion"}`,
        ),
        ...completed.flatMap((outcome) => [
          ...outcome
            .response!.runtime_sync.filter(
              (runtimeOutcome) => runtimeOutcome.status !== "queued",
            )
            .map(
              (runtimeOutcome) =>
                `Tunnel cleanup for ${runtimeOutcome.client_id}: ${dispatchFailureReason(
                  runtimeOutcome.error,
                  runtimeOutcome.status,
                  "Runtime apply job",
                )}`,
            ),
          ...outcome
            .response!.post_commit.filter(
              (postCommitOutcome) => postCommitOutcome.status !== "completed",
            )
            .map((postCommitOutcome) =>
              lifecycleOutcomeFailureReason(postCommitOutcome, "VPS deletion"),
            ),
        ]),
      ];
      if (completed.length === 0 && failedRequests.length > 0) {
        // No inventory mutation occurred, so keep the reviewed operation in
        // the confirmation error path instead of reporting a completed bulk
        // outcome. Inline prompts remain retryable and overlay prompts retain
        // their detached failure alert after submission.
        throw new Error(failureReasons.join(" "));
      }
      const total = deleteSnapshot.targets.length;
      const deletionMessage =
        total === 1 && completed.length === 1
          ? failureReasons.length > 0
            ? `VPS deleted. ${failureReasons.join(" ")}`
            : queuedSyncs.length > 0
              ? `VPS deleted; tunnel cleanup queued for ${queuedSyncs.length} surviving ${queuedSyncs.length === 1 ? "peer" : "peers"}.`
              : "VPS deleted; no surviving tunnel peer required cleanup."
          : [
              `Deleted ${completed.length} of ${total} selected VPS${total === 1 ? "" : "s"}.`,
              queuedSyncs.length > 0
                ? `Tunnel cleanup queued for ${queuedSyncs.length} surviving ${queuedSyncs.length === 1 ? "peer" : "peers"}.`
                : completed.length > 0
                  ? "No surviving tunnel peer required cleanup."
                  : "",
              failureReasons.join(" "),
            ]
              .filter(Boolean)
              .join(" ");
      setDeleteFeedback({
        message: deletionMessage,
        tone:
          completed.length === 0
            ? "danger"
            : failureReasons.length > 0
              ? "warning"
              : queuedSyncs.length > 0
                ? "progress"
                : "success",
      });
      clearDeleteReview();
      if (completed.length > 0) {
        onSelectAgent(null);
      }
    });
  }

  const fleetMutationPending =
    deletePending ||
    deleteReviewPending ||
    lifecyclePending ||
    lifecycleReviewPending;
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
      label: "Stop agent",
      description: (rows) =>
        `Stop ${rows.length} selected agent${rows.length === 1 ? "" : "s"}. External service start is required afterward.`,
      disabled: () => fleetMutationPending,
      icon: <PowerOff size={15} />,
      onSelect: (rows) => void requestAgentLifecycle(rows, "stop"),
      separatorBefore: true,
      tone: "danger",
    },
    {
      label: "Restart agent",
      description: (rows) =>
        `Restart ${rows.length} selected agent${rows.length === 1 ? "" : "s"} through each agent's configured lifecycle mode.`,
      disabled: () => fleetMutationPending,
      icon: <RefreshCw size={15} />,
      onSelect: (rows) => void requestAgentLifecycle(rows, "restart"),
    },
    {
      label: "Review VPS deletion",
      description: (rows) =>
        `Delete ${rows.length} selected VPS${rows.length === 1 ? "" : "s"} from panel inventory.`,
      disabled: () => fleetMutationPending,
      icon: <Trash2 size={15} />,
      onSelect: requestDeleteAgent,
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
      {fleetLoadBoundaryLabels.length > 0 ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`Loaded list limits reached for ${fleetLoadBoundaryLabels.join(
            ", ",
          )}; counts and filters below apply to loaded records.`}
          tone="info"
        />
      ) : null}
      {fleetSubpage === "instances" && (
        <FleetInstancesPanel
          actions={fleetInstanceActions}
          agents={agents}
          apiError={apiError}
          columns={fleetColumns}
          deleteError={deleteError}
          deleteFeedback={deleteFeedback}
          deletePending={deletePending}
          deleteSnapshot={deleteSnapshot}
          fleetCoreEvidenceAvailable={fleetCoreEvidenceAvailable}
          lifecycleError={lifecycleError}
          lifecyclePending={lifecyclePending}
          lifecycleProgress={lifecycleProgress}
          lifecycleResultAction={lifecycleResultAction}
          lifecycleSnapshot={lifecycleSnapshot}
          onCancelDelete={() => {
            setDeleteError(null);
            clearDeleteReview();
          }}
          onCancelLifecycle={() => {
            setLifecycleError(null);
            clearLifecycleReview();
          }}
          onClearLifecycleResult={() => {
            setLifecycleError(null);
            setLifecycleProgress(null);
            setLifecycleResultAction(null);
          }}
          onConfirmDelete={() => void confirmDeleteAgent()}
          onConfirmLifecycle={() => void confirmAgentLifecycle()}
          onOpenJobDetails={onOpenJobDetails}
          onOpenMonitor={
            onNavigatePanel
              ? () => onNavigatePanel("Fleet", "monitor")
              : undefined
          }
          onRegisterVps={onRegisterVps}
          onSelectionChange={handleFleetSelectionChange}
          renderSelectionPanel={(rows) => (
            <FleetSelectionPanel
              agents={rows}
              allTags={tags}
              allNetworkRates={latestNetworkRates}
              latestNetworkRates={selectedLatestNetworkRates}
              latestRollups={latestRollups}
              networkRateSelections={networkRateSelectionsByClient}
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
          renderExpandedRow={(agent) => (
            <FleetInstanceDetail
              agent={agent}
              apiToken={apiToken}
              lastLiveEvent={lastLiveEvent}
              latestNetworkRates={latestNetworkRates.get(agent.id) ?? []}
              latestRollup={latestRollups.get(agent.id) ?? null}
              latestTunnels={latestTunnels.get(agent.id) ?? []}
              uptime={uptimeByClient.get(agent.id) ?? null}
              mutateTagsForAgents={mutateTagsForAgents}
              onCreateJob={onCreateJob}
              onLoadJobOutputs={onLoadJobOutputs}
              onLoadJobTargets={onLoadJobTargets}
              onNavigatePanel={onNavigatePanel}
              onOpenJobDetails={onOpenJobDetails}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onLoadEffectiveAgentConfig={onLoadEffectiveAgentConfig}
              onLoadConfigurationSources={onLoadConfigurationSources}
              onRequestedTabConsumed={ignoreRequestedFleetDetailTab}
              onUpdateAgentAlias={onUpdateAgentAlias}
              policies={fleetAlertPolicies}
              policiesTruncated={fleetAlertPoliciesTruncated}
              policyAlerts={policyAlertsByClient.get(agent.id) ?? []}
              policyAlertsTruncated={policyAlertsTruncated}
              privilegeMaterial={privilegeMaterial}
              requestedTab={null}
              configurationSources={configurationSources.filter(
                (source) => source.client_id === agent.id,
              )}
              summary={summary}
              tagDisplayOrder={tagDisplayOrder}
              tagVisibilityOverrides={
                preferences.fleet_tag_visibility_overrides
              }
              telemetryNetworkRatesTruncated={telemetryNetworkRatesTruncated}
              telemetryRollupsTruncated={telemetryRollupsTruncated}
              telemetryTunnelsTruncated={telemetryTunnelsTruncated}
              trafficAccounting={trafficByClient.get(agent.id) ?? null}
              trafficAccountingTruncated={trafficAccountingTruncated}
              vpsNameDisplayMode={vpsNameDisplayMode}
              vpsRuleValues={vpsRulesByClient.get(agent.id) ?? []}
              vpsRuleValuesTruncated={vpsRuleValuesTruncated}
              wsState={wsState}
            />
          )}
          scopeActive={scopeActive}
          summary={summary}
          vpsNameDisplayMode={vpsNameDisplayMode}
          wsState={wsState}
        />
      )}

      {fleetSubpage === "policies" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Alert policies</h2>
              <span>{`${formatLowerBoundCount(
                fleetAlertPolicies.length,
                fleetAlertPoliciesTruncated,
              )}${fleetAlertPoliciesTruncated ? " loaded" : ""} policy groups`}</span>
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
            rowsTruncated={fleetAlertPoliciesTruncated}
          />
        </div>
      )}

      {fleetSubpage === "notifications" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Notification channels</h2>
              <span>{`${formatLowerBoundCount(
                fleetAlertNotificationChannels.length,
                notificationChannelsTruncated,
              )}${notificationChannelsTruncated ? " loaded" : ""} alert channels, ${formatLowerBoundCount(
                webhookRules.length,
                webhookRulesTruncated,
              )}${webhookRulesTruncated ? " loaded" : ""} expression webhooks`}</span>
            </div>
            <span className="sectionContext">
              {formatLowerBoundCount(
                fleetAlertNotifications.length + webhookRuleDeliveries.length,
                alertNotificationsTruncated || webhookDeliveriesTruncated,
              )}{" "}
              {alertNotificationsTruncated || webhookDeliveriesTruncated
                ? "loaded "
                : ""}
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

function fleetMutationTargets(
  rows: AgentView[],
  vpsNameDisplayMode: VpsNameDisplayMode,
): FleetMutationTargetSnapshot[] {
  return [...rows]
    .sort((left, right) => left.id.localeCompare(right.id))
    .map((agent) => ({
      agent: {
        ...agent,
        capabilities: { ...agent.capabilities },
        tags: [...agent.tags],
      },
      clientId: agent.id,
      displayName: formatVpsName(agent, vpsNameDisplayMode),
      status: agent.status,
    }));
}

function fleetSelectionSignature(rows: AgentView[]): string {
  return [...rows]
    .map((row) => row.id)
    .sort((left, right) => left.localeCompare(right))
    .join("\u001f");
}

function fleetTargetSignature(targets: FleetMutationTargetSnapshot[]): string {
  return targets.map((target) => target.clientId).join("\u001f");
}

function rowsForFleetMutationTargets(
  targets: FleetMutationTargetSnapshot[],
): AgentView[] {
  return targets.map((target) => target.agent);
}

function fleetTargetLabel(
  targets: FleetMutationTargetSnapshot[],
  clientId: string,
): string {
  const target = targets.find((candidate) => candidate.clientId === clientId);
  return target ? `${target.displayName} (${target.clientId})` : clientId;
}

function fleetMutationConfirmationItems(
  targets: FleetMutationTargetSnapshot[],
): Array<{ label: string; title?: string; value: ReactNode }> {
  if (targets.length === 1) {
    return [
      { label: "VPS", value: targets[0].displayName },
      { label: "Client ID", value: targets[0].clientId },
      { label: "Status", value: targets[0].status },
    ];
  }
  const fullTargetList = targets
    .map((target) => `${target.displayName} (${target.clientId})`)
    .join(", ");
  const visibleTargetList = targets
    .slice(0, 8)
    .map((target) => `${target.displayName} (${target.clientId})`)
    .join(", ");
  const statusCounts = new Map<string, number>();
  for (const target of targets) {
    statusCounts.set(target.status, (statusCounts.get(target.status) ?? 0) + 1);
  }
  return [
    { label: "VPS count", value: String(targets.length) },
    {
      label: "Selected VPSs",
      title: fullTargetList,
      value:
        targets.length > 8
          ? `${visibleTargetList} · +${targets.length - 8} more`
          : visibleTargetList,
    },
    {
      label: "Current states",
      value: [...statusCounts.entries()]
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([status, count]) => `${count} ${status}`)
        .join(" · "),
    },
  ];
}

function FleetInstancesPanel({
  actions,
  agents,
  apiError,
  columns,
  deleteError,
  deleteFeedback,
  deletePending,
  deleteSnapshot,
  fleetCoreEvidenceAvailable,
  lifecycleError,
  lifecyclePending,
  lifecycleProgress,
  lifecycleResultAction,
  lifecycleSnapshot,
  onCancelDelete,
  onCancelLifecycle,
  onClearLifecycleResult,
  onConfirmDelete,
  onConfirmLifecycle,
  onOpenJobDetails,
  onOpenMonitor,
  onRegisterVps,
  onSelectionChange,
  renderExpandedRow,
  renderSelectionPanel,
  scopeActive,
  summary,
  vpsNameDisplayMode,
  wsState,
}: {
  actions: ConsoleDataGridAction<AgentView>[];
  agents: AgentView[];
  apiError: string | null;
  columns: ConsoleDataGridColumn<AgentView>[];
  deleteError: string | null;
  deleteFeedback: { message: string; tone: ActionFeedbackTone } | null;
  deletePending: boolean;
  deleteSnapshot: DeleteAgentConfirmationSnapshot | null;
  fleetCoreEvidenceAvailable: boolean;
  lifecycleError: string | null;
  lifecyclePending: boolean;
  lifecycleProgress: BulkJobProgress | null;
  lifecycleResultAction: AgentLifecycleAction | null;
  lifecycleSnapshot: AgentLifecycleConfirmationSnapshot | null;
  onCancelDelete: () => void;
  onCancelLifecycle: () => void;
  onClearLifecycleResult: () => void;
  onConfirmDelete: () => void;
  onConfirmLifecycle: () => void;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenMonitor?: () => void;
  onRegisterVps?: () => void;
  onSelectionChange: (rows: AgentView[]) => void;
  renderExpandedRow: (row: AgentView) => ReactNode;
  renderSelectionPanel: (rows: AgentView[]) => ReactNode;
  scopeActive: boolean;
  summary: FleetSummary;
  vpsNameDisplayMode: VpsNameDisplayMode;
  wsState: string;
}) {
  const deleteOutcomeRef = useRef<HTMLDivElement | null>(null);
  const previousDeleteOutcomeRef = useRef<string | null>(null);
  const mutationOutcomeMessage =
    lifecycleError ?? deleteError ?? deleteFeedback?.message ?? null;
  const mutationOutcomeTone =
    lifecycleError || deleteError ? "danger" : (deleteFeedback?.tone ?? "info");
  const stableAgents = useMemo(
    () =>
      [...agents].sort(
        (left, right) =>
          formatVpsName(left, vpsNameDisplayMode).localeCompare(
            formatVpsName(right, vpsNameDisplayMode),
          ) || left.id.localeCompare(right.id),
      ),
    [agents, vpsNameDisplayMode],
  );

  useEffect(() => {
    if (!mutationOutcomeMessage) {
      previousDeleteOutcomeRef.current = null;
      return;
    }
    if (previousDeleteOutcomeRef.current === mutationOutcomeMessage) {
      return;
    }
    previousDeleteOutcomeRef.current = mutationOutcomeMessage;
    const frame = window.requestAnimationFrame(() => {
      if (deleteOutcomeRef.current) {
        scrollIntoViewWithMotion(deleteOutcomeRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [mutationOutcomeMessage]);

  return (
    <div className="fleetPanel fleetInstancesPanel">
      <div className="sectionHeader fleetInstancesHeader">
        <div>
          <h2>VPS instances</h2>
          <span>Live control-plane inventory</span>
        </div>
        <span className="sectionContext">
          {fleetCoreEvidenceAvailable
            ? `${summary.online} live / ${summary.revoked} access revoked / ${summary.never + summary.unknown} no contact / ${summary.total} total`
            : "Fleet inventory unavailable"}{" "}
          · {formatConsoleStreamState(wsState)}
        </span>
      </div>
      <ConsoleFreshnessBanner error={apiError} />
      <ActionFeedback
        className="localActionFeedback"
        message={mutationOutcomeMessage}
        ref={deleteOutcomeRef}
        tone={mutationOutcomeTone}
      />

      <ConsoleDataGrid
        actions={actions}
        columns={columns}
        defaultColumnVisibility={{
          cycle_usage: false,
          provider: false,
          quota: false,
          registration_ip: false,
          reset_day: false,
          selectors: false,
          tags: false,
          traffic_now: false,
          traffic_state: false,
          uptime: false,
        }}
        defaultPageSize={20}
        empty={
          <div className="emptyState">
            <Server size={22} />
            <strong>
              {!fleetCoreEvidenceAvailable
                ? "Fleet inventory unavailable"
                : scopeActive
                  ? "No VPS match this view"
                  : "No live agents"}
            </strong>
            <span>
              {!fleetCoreEvidenceAvailable
                ? "Retry the fleet refresh before assuming no VPS is registered."
                : scopeActive
                  ? "Adjust or clear the saved fleet view."
                  : "Waiting for VPS agents to connect through gateways and report in."}
            </span>
          </div>
        }
        getRowId={(agent) => agent.id}
        itemLabel="instances"
        onSelectionChange={onSelectionChange}
        renderExpandedRow={renderExpandedRow}
        renderSelectionPanel={renderSelectionPanel}
        rows={stableAgents}
        storageKey="vpsman.grid.fleet.instances.v2"
        title="VPS instance records"
        toolbarActions={
          <>
            <button
              className="primaryAction compactAction"
              data-tooltip-disabled-reason={
                onRegisterVps
                  ? undefined
                  : "VPS registration is unavailable in this context"
              }
              disabled={!onRegisterVps}
              onClick={() => onRegisterVps?.()}
              type="button"
            >
              <Plus size={15} />
              Register VPS
            </button>
            <div
              aria-label="Fleet instance view mode"
              className="segmented fleetViewSwitch"
              role="group"
            >
              <button aria-pressed={true} className="selected" type="button">
                Table
              </button>
              <button
                data-tooltip-disabled-reason={
                  onOpenMonitor
                    ? undefined
                    : "The VPS card monitor is unavailable in this context"
                }
                disabled={!onOpenMonitor}
                onClick={() => onOpenMonitor?.()}
                type="button"
              >
                Cards
              </button>
            </div>
          </>
        }
      />
      {lifecycleProgress ? (
        <ExecutionResultPanel
          context={
            lifecycleResultAction === "stop"
              ? "Agent stop request"
              : "Agent restart request"
          }
          label="Agent lifecycle result"
          loading={lifecyclePending}
          onClearResults={onClearLifecycleResult}
          onOpenJobDetails={onOpenJobDetails}
          progress={lifecycleProgress}
        >
          {lifecycleResultAction === "stop" ? (
            <p>
              Completed targets accepted the stop request. Start their agent
              service externally before they can receive another panel action.
            </p>
          ) : (
            <p>
              Completed targets accepted the restart request through their
              configured lifecycle mode; reconnect evidence updates
              independently.
            </p>
          )}
        </ExecutionResultPanel>
      ) : null}
      <ConfirmationPrompt
        confirmLabel={
          lifecycleSnapshot?.action === "stop"
            ? "Stop agents"
            : "Restart agents"
        }
        detail={
          lifecycleSnapshot?.action === "stop"
            ? "Each selected agent first retains a terminal job result, then exits through its configured lifecycle mode. A stopped agent cannot receive Restart from this panel; start the vpsman-agent service externally."
            : "Each selected agent first retains a terminal job result, then restarts through its configured lifecycle mode. Job completion confirms request acceptance; reconnect evidence is reported separately."
        }
        error={lifecycleError}
        items={
          lifecycleSnapshot
            ? fleetMutationConfirmationItems(lifecycleSnapshot.targets)
            : []
        }
        onCancel={onCancelLifecycle}
        onConfirm={onConfirmLifecycle}
        open={Boolean(lifecycleSnapshot)}
        pending={lifecyclePending}
        title={
          lifecycleSnapshot?.action === "stop"
            ? "Stop selected agents"
            : "Restart selected agents"
        }
        tone={lifecycleSnapshot?.action === "stop" ? "warning" : "normal"}
      />
      <ConfirmationPrompt
        confirmLabel="Delete VPSs"
        detail="This deactivates every selected VPS immediately and removes it from inventory, selectors, dashboard, tags, topology, and future bulk targeting. Tunnel declarations using these VPSs are retired and surviving peers receive cleanup sync jobs. Historical jobs and audit records remain. Each VPS is committed separately, so partial results identify exactly what must be retried."
        error={deleteError}
        items={
          deleteSnapshot
            ? fleetMutationConfirmationItems(deleteSnapshot.targets)
            : []
        }
        onCancel={onCancelDelete}
        onConfirm={onConfirmDelete}
        open={Boolean(deleteSnapshot)}
        pending={deletePending}
        title="Delete selected VPSs from panel"
        tone="danger"
      />
    </div>
  );
}

function FleetInstanceDetail({
  agent,
  apiToken,
  configurationSources,
  lastLiveEvent,
  policyAlerts,
  policyAlertsTruncated,
  policies,
  policiesTruncated,
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
  onLoadEffectiveAgentConfig,
  onLoadConfigurationSources,
  onUpdateAgentAlias,
  privilegeMaterial,
  summary,
  tagDisplayOrder,
  tagVisibilityOverrides,
  telemetryNetworkRatesTruncated,
  telemetryRollupsTruncated,
  telemetryTunnelsTruncated,
  trafficAccounting,
  trafficAccountingTruncated,
  uptime,
  vpsRuleValues,
  vpsRuleValuesTruncated,
  vpsNameDisplayMode,
  wsState,
}: {
  agent: AgentView;
  apiToken: string;
  configurationSources: ConfigurationSourceView[];
  lastLiveEvent: string;
  policyAlerts: PolicyAlertRecord[];
  policyAlertsTruncated: boolean;
  policies: FleetAlertPolicyRecord[];
  policiesTruncated: boolean;
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
  onNavigatePanel?: (
    view: ActiveView,
    subpage: string,
    targetClientId?: string,
  ) => void;
  onLoadEffectiveAgentConfig: (
    clientId: string,
  ) => Promise<EffectiveAgentConfigResponse>;
  onLoadConfigurationSources: () => Promise<void>;
  onUpdateAgentAlias: (
    clientId: string,
    displayName: string,
    confirmed: boolean,
  ) => Promise<AgentView>;
  privilegeMaterial: PrivilegeMaterial | null;
  summary: FleetSummary;
  tagDisplayOrder: TagDisplayOrder;
  tagVisibilityOverrides: Record<string, boolean>;
  telemetryNetworkRatesTruncated: boolean;
  telemetryRollupsTruncated: boolean;
  telemetryTunnelsTruncated: boolean;
  trafficAccounting: TrafficAccountingRecord | null;
  trafficAccountingTruncated: boolean;
  uptime: TelemetryUptimeRecord | null;
  vpsRuleValues: VpsRuleValueRecord[];
  vpsRuleValuesTruncated: boolean;
  vpsNameDisplayMode: VpsNameDisplayMode;
  wsState: string;
}) {
  const formatBytes = useByteCountFormatter();
  const formatByteRateFromBitsPerSecond = useByteRateFormatter();
  const detailTabNamespace = `fleet-detail-${useId().replace(/:/g, "")}`;
  const detailTabPanelId = `${detailTabNamespace}-tabpanel`;
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
  const [tagStatusTone, setTagStatusTone] =
    useState<ActionFeedbackTone>("info");
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
    useState<EffectiveAgentConfigResponse | null>(null);
  const provider = providerFromTags(agent.tags);
  const productName = useProjectedProductName(
    apiToken,
    agent.id,
    productNameFromVpsRules(vpsRuleValues, agent.id),
  );
  const country = countryFromTags(agent.tags);
  const region = regionTagValue(agent.tags);
  const providerProduct = providerProductLabel(provider, productName, "unset");
  const displayOnlyTags = displayTags(
    agent.tags,
    tagDisplayOrder,
    tagVisibilityOverrides,
  );
  const isNetworkManaged = agent.tags.some((tag) =>
    ["bgp", "ospf", "tunnel"].includes(tag.toLowerCase()),
  );
  const agentLabel = formatVpsName(agent, vpsNameDisplayMode);
  const displayState = agentDisplayState(agent);
  const statusDetail = displayState.detail.startsWith("Last contact ")
    ? null
    : displayState.detail;
  const configPreviewSummary = configPreview
    ? `${configPreview.sources.length} effective configuration sources`
    : "Load redacted runtime config view for this VPS.";
  const aggregateNetworkRates = selectedNetworkRates(
    latestNetworkRates,
    vpsRuleValues,
  );
  const networkRateSelection = resolveNetworkRateInterfaces(vpsRuleValues);

  useEffect(() => {
    void runPanelAction(
      setConfigPending,
      setConfigError,
      onLoadConfigurationSources,
    );
  }, [onLoadConfigurationSources]);

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
    setTagStatus(`${action === "add" ? "adding" : "removing"} ${tag}`);
    setTagStatusTone("progress");
    await runPanelAction(setTagPending, setTagError, async () => {
      const response = await mutateTagsForAgents([agent], action, tag);
      setTagStatus(
        `${response.action} ${response.tag}: ${response.changed_count} changed, ${response.skipped_count} skipped`,
      );
      setTagStatusTone("success");
      setTagDraft("");
    });
  }

  async function loadRenderedConfig() {
    await runPanelAction(setConfigPending, setConfigError, async () => {
      setConfigPreview(await onLoadEffectiveAgentConfig(agent.id));
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
            data-tooltip-disabled-reason={
              aliasPending
                ? "The VPS rename is already in progress"
                : aliasDraft.trim() === agent.display_name
                  ? "Enter a different display name before renaming"
                  : undefined
            }
            disabled={aliasPending || aliasDraft.trim() === agent.display_name}
            type="submit"
          >
            Rename
          </button>
          <ActionFeedback
            className="localActionFeedback fleetAliasFeedback"
            message={aliasError}
            tone="danger"
          />
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
            data-tooltip-disabled-reason={
              tagPending
                ? "A tag change is already in progress"
                : !tagDraft.trim()
                  ? "Enter a tag before adding it"
                  : undefined
            }
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
              data-tooltip-disabled-reason={
                tagPending ? "A tag change is already in progress" : undefined
              }
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
      <ActionFeedback
        className="localActionFeedback fleetDetailTagFeedback"
        message={tagError ?? tagStatus}
        tone={tagError ? "danger" : tagStatusTone}
      />
      <div
        className="detailTabs"
        role="tablist"
        aria-label="VPS detail sections"
        onKeyDown={handleTabListKeyDown}
      >
        {detailTabs.map((tab) => (
          <button
            aria-controls={detailTabPanelId}
            aria-selected={activeDetailTab === tab}
            className={activeDetailTab === tab ? "selected" : ""}
            id={tabId(detailTabNamespace, tab)}
            key={tab}
            onClick={() => setActiveDetailTab(tab)}
            role="tab"
            tabIndex={activeDetailTab === tab ? 0 : -1}
            type="button"
          >
            {tab}
          </button>
        ))}
      </div>
      <div className="signalGrid fleetSignalGrid">
        <Metric
          label="Traffic"
          value={
            !latestRollup &&
            networkRateSelection.valid &&
            aggregateNetworkRates.length === 0 &&
            (telemetryRollupsTruncated || telemetryNetworkRatesTruncated)
              ? "Unknown in loaded telemetry pages"
              : formatSignalTraffic(
                  aggregateNetworkRates,
                  networkRateSelection,
                  formatByteRateFromBitsPerSecond,
                )
          }
          tone="blue"
        />
        <Metric
          label="Samples"
          value={
            !latestRollup &&
            aggregateNetworkRates.length === 0 &&
            (telemetryRollupsTruncated || telemetryNetworkRatesTruncated)
              ? "Unknown in loaded telemetry pages"
              : formatSignalSamples(latestRollup, aggregateNetworkRates)
          }
          tone="green"
        />
        <Metric
          label="RAM used"
          value={
            !latestRollup && telemetryRollupsTruncated
              ? "Unknown in loaded rollup page"
              : formatMemoryUsed(latestRollup, formatBytes)
          }
          tone="blue"
        />
        <Metric
          label="Disk used"
          value={
            !latestRollup && telemetryRollupsTruncated
              ? "Unknown in loaded rollup page"
              : formatDiskUsed(latestRollup, formatBytes)
          }
          tone="green"
        />
      </div>
      <div
        aria-labelledby={tabId(detailTabNamespace, activeDetailTab)}
        className="detailPane fleetDetailPane"
        id={detailTabPanelId}
        role="tabpanel"
      >
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
              value={
                <span className="historyPrimary" title={displayState.detail}>
                  <span>{displayState.label}</span>
                  {statusDetail ? <small>{statusDetail}</small> : null}
                </span>
              }
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
              icon={<Clock3 size={18} />}
              label="Uptime"
              value={
                <span
                  data-tooltip-empty-reason={
                    uptime ? undefined : "Agent-reported uptime is unavailable"
                  }
                  title={
                    uptime
                      ? `Agent-reported uptime ${formatUptime(uptime.uptime_secs)}; sampled ${formatCompactTime(uptime.observed_at)}`
                      : "Agent-reported uptime is unavailable"
                  }
                >
                  {uptime
                    ? `${formatUptime(uptime.uptime_secs)} · observed ${formatCompactTime(uptime.observed_at)}`
                    : "-"}
                </span>
              }
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
              label="Provider"
              value={providerProduct}
            />
            <DetailLine
              icon={<MapPin size={18} />}
              label="Location"
              value={
                <span className="fleetDetailLocation">
                  <CountryBadge country={country} showFlag />
                  {region ? <span>· {region}</span> : null}
                </span>
              }
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="Privilege"
              value={formatPrivilege(agent.capabilities)}
            />
            <DetailLine
              icon={<Gauge size={18} />}
              label="Fleet position"
              value={`${summary.online} live / ${summary.revoked} access revoked / ${summary.never + summary.unknown} no contact / ${summary.total} total`}
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
              value={
                !latestRollup && telemetryRollupsTruncated
                  ? "Unknown in loaded rollup page; more may exist"
                  : formatLoad(latestRollup?.cpu_load_1_avg)
              }
            />
            <DetailLine
              icon={<Server size={18} />}
              label="RAM used"
              value={
                !latestRollup && telemetryRollupsTruncated
                  ? "Unknown in loaded rollup page; more may exist"
                  : formatMemoryUsed(latestRollup, formatBytes)
              }
            />
            <DetailLine
              icon={<Boxes size={18} />}
              label="Disk used"
              value={
                !latestRollup && telemetryRollupsTruncated
                  ? "Unknown in loaded rollup page; more may exist"
                  : formatDiskUsed(latestRollup, formatBytes)
              }
            />
            <DetailLine
              icon={<Network size={18} />}
              label="Network bytes"
              value={
                !latestRollup && telemetryRollupsTruncated
                  ? "Unknown in loaded rollup page; more may exist"
                  : formatNetworkBytes(latestRollup, formatBytes)
              }
            />
            <DetailLine
              icon={<Network size={18} />}
              label="Network rate"
              value={
                networkRateSelection.valid &&
                aggregateNetworkRates.length === 0 &&
                telemetryNetworkRatesTruncated
                  ? "Unknown in loaded network-rate page; more may exist"
                  : formatNetworkRateSummary(
                      aggregateNetworkRates,
                      networkRateSelection,
                      formatByteRateFromBitsPerSecond,
                    )
              }
            />
            <DetailLine
              icon={<Activity size={18} />}
              label="Rollup samples"
              value={
                !latestRollup && telemetryRollupsTruncated
                  ? "Unknown in loaded rollup page; more may exist"
                  : formatRollupSamples(latestRollup)
              }
            />
            <DetailLine
              icon={<Server size={18} />}
              label="Agent status"
              value={displayState.label}
            />
          </>
        )}
        {activeDetailTab === "Traffic & Rules" && (
          <TrafficRulesDetail
            agent={agent}
            policyAlerts={policyAlerts}
            policyAlertsTruncated={policyAlertsTruncated}
            policies={policies}
            policiesTruncated={policiesTruncated}
            onNavigatePanel={onNavigatePanel}
            trafficAccounting={trafficAccounting}
            trafficAccountingTruncated={trafficAccountingTruncated}
            vpsRuleValues={vpsRuleValues}
            vpsRuleValuesTruncated={vpsRuleValuesTruncated}
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
            <TunnelList
              tunnels={latestTunnels}
              tunnelsTruncated={telemetryTunnelsTruncated}
            />
            <NetworkRateList
              rates={latestNetworkRates}
              ratesTruncated={telemetryNetworkRatesTruncated}
              rollup={latestRollup}
            />
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
            <ConfigurationSourceList sources={configurationSources} />
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
    </div>
  );
}

function ConfigurationSourceList({
  sources,
}: {
  sources: ConfigurationSourceView[];
}) {
  const rows = sources
    .slice()
    .sort((left, right) => left.behavior.localeCompare(right.behavior));
  const explicitCount = rows.filter(
    (source) => source.selection_origin === "explicit_override",
  ).length;
  return (
    <div className="fleetConfigRows">
      <div className="detailLine">
        <FileCog size={18} />
        <div>
          <span>Effective configuration sources</span>
          <strong>
            {rows.length === 0
              ? "No source evidence loaded"
              : `${rows.length} behaviors · ${explicitCount} explicit overrides`}
          </strong>
        </div>
      </div>
      {rows.slice(0, 8).map((source) => (
        <div
          className="detailLine compactConfigLine"
          key={`${source.client_id}-${source.behavior}`}
        >
          <Boxes size={18} />
          <div>
            <span>{readableToken(source.behavior)}</span>
            <strong>
              {source.effective_preset_name} ·{" "}
              {source.selection_origin === "explicit_override"
                ? "explicit override"
                : "inherited system default"}
            </strong>
            <small>
              Sync {readableToken(source.runtime_sync.state)} · readiness{" "}
              {readableToken(source.readiness.state)}
            </small>
          </div>
        </div>
      ))}
      {rows.length > 8 && (
        <small className="mutedText">
          +{rows.length - 8} more configuration source
          {rows.length - 8 === 1 ? "" : "s"}
        </small>
      )}
    </div>
  );
}

function readableToken(value: string): string {
  return value
    .split("_")
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
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
  preview: EffectiveAgentConfigResponse | null;
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
          data-tooltip-disabled-reason={
            pending
              ? "The effective agent config is already loading"
              : undefined
          }
          disabled={pending}
          onClick={onLoad}
          type="button"
        >
          <RefreshCw size={14} />
          Load config
        </button>
        <ActionFeedback
          className="localActionFeedback configPreviewActionFeedback"
          message={error}
          tone="danger"
        />
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
  policyAlertsTruncated,
  policies,
  policiesTruncated,
  trafficAccounting,
  trafficAccountingTruncated,
  vpsRuleValues,
  vpsRuleValuesTruncated,
}: {
  agent: AgentView;
  onNavigatePanel?: (
    view: ActiveView,
    subpage: string,
    targetClientId?: string,
  ) => void;
  policyAlerts: PolicyAlertRecord[];
  policyAlertsTruncated: boolean;
  policies: FleetAlertPolicyRecord[];
  policiesTruncated: boolean;
  trafficAccounting: TrafficAccountingRecord | null;
  trafficAccountingTruncated: boolean;
  vpsRuleValues: VpsRuleValueRecord[];
  vpsRuleValuesTruncated: boolean;
}) {
  const formatBytes = useByteCountFormatter();
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
  const trafficMissingUnderCap =
    trafficAccounting === null && trafficAccountingTruncated;
  const unknownTrafficPage = "Unknown in loaded traffic page; more may exist";
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
    [formatBytes],
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
        cell: (row) => (
          <span
            data-tooltip-empty-reason={
              row.alert?.actual_value == null
                ? "No active alert has reported an actual metric value for this rule"
                : undefined
            }
          >
            {formatMetricValue(row.alert?.actual_value, formatBytes)}
          </span>
        ),
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
            : formatMetricValue(row.alert.threshold_value, formatBytes),
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
    [formatBytes],
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
        cell: (row) => (
          <span title={row.policy_group_id}>
            {policyById.get(row.policy_group_id)?.name ??
              shortId(row.policy_group_id)}
          </span>
        ),
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
        cell: (row) => (
          <span title={row.policy_rule_id}>
            {policyById
              .get(row.policy_group_id)
              ?.rules.find((rule) => rule.id === row.policy_rule_id)?.name ??
              shortId(row.policy_rule_id)}
          </span>
        ),
      },
      {
        id: "actual",
        header: "Actual",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.actual_value ?? -1,
        cell: (row) => (
          <span
            data-tooltip-empty-reason={
              row.actual_value == null
                ? "This alert did not record an actual metric value"
                : undefined
            }
          >
            {formatMetricValue(row.actual_value, formatBytes)}
          </span>
        ),
      },
      {
        id: "threshold",
        header: "Threshold",
        size: 120,
        minSize: 100,
        sortValue: (row) => row.threshold_value ?? -1,
        cell: (row) => (
          <span
            data-tooltip-empty-reason={
              row.threshold_value == null
                ? "This alert did not record a numeric threshold value"
                : undefined
            }
          >
            {formatMetricValue(row.threshold_value, formatBytes)}
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        size: 95,
        minSize: 80,
        cell: () => "open",
      },
    ],
    [formatBytes, policyById],
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
            : trafficMissingUnderCap
              ? unknownTrafficPage
              : "none"}
        </span>
      </div>
      <div className="consoleOperationsBar">
        <span>
          <strong>
            {trafficMissingUnderCap
              ? "Selector state unknown in loaded traffic page"
              : selectorSummary(trafficAccounting)}
          </strong>
          <small>
            {trafficAccounting?.selector_hash ??
              (trafficMissingUnderCap
                ? "More traffic records may exist"
                : "no selector hash")}
          </small>
        </span>
        <div className="consoleOperationsActions">
          <button
            className="secondaryAction compactAction"
            type="button"
            onClick={() => onNavigatePanel?.("Config", `rules:id:${agent.id}`)}
          >
            Edit VPS Rules
          </button>
          <button
            className="secondaryAction compactAction"
            data-tooltip-disabled-reason={
              selectedPolicyId
                ? undefined
                : "No alert policy is linked to this VPS"
            }
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
              : trafficMissingUnderCap
                ? unknownTrafficPage
                : "not configured"
          }
          tone="blue"
        />
        <Metric
          label="Quota"
          value={
            trafficMissingUnderCap
              ? unknownTrafficPage
              : quotaSummary(trafficAccounting, formatBytes)
          }
          tone="green"
        />
        <Metric
          label="Cycle percent"
          value={
            trafficAccounting?.cycle_percent == null
              ? trafficMissingUnderCap
                ? unknownTrafficPage
                : "incomplete"
              : `${trafficAccounting.cycle_percent.toFixed(1)}%`
          }
          tone="blue"
        />
        <Metric
          label="Traffic state"
          value={
            trafficMissingUnderCap && policyAlerts.length === 0
              ? unknownTrafficPage
              : trafficStateForClient(trafficAccounting, policyAlerts)
          }
          tone="green"
        />
      </div>
      <div className="consoleInlineDetailGrid trafficAccountingSummary">
        <span>
          <strong>Cycle start</strong>
          <span>
            {trafficAccounting?.reset_day === -1
              ? "-"
              : (trafficAccounting?.cycle_start ??
                (trafficMissingUnderCap
                  ? unknownTrafficPage
                  : "not configured"))}
          </span>
        </span>
        <span>
          <strong>Cycle end</strong>
          <span>
            {trafficAccounting?.reset_day === -1
              ? "-"
              : (trafficAccounting?.cycle_end ??
                (trafficMissingUnderCap
                  ? unknownTrafficPage
                  : "not configured"))}
          </span>
        </span>
        <span>
          <strong>Reset day</strong>
          <span>
            {trafficMissingUnderCap
              ? unknownTrafficPage
              : resetDaySummary(trafficAccounting)}
          </span>
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
              : trafficMissingUnderCap
                ? unknownTrafficPage
                : "none"}
          </span>
        </span>
        <span>
          <strong>Counter epochs seen</strong>
          <span>
            {trafficAccounting?.counter_epochs_seen ??
              (trafficMissingUnderCap ? unknownTrafficPage : 0)}
          </span>
        </span>
        <span>
          <strong>Incomplete reasons</strong>
          <span>
            {trafficAccounting?.incomplete_reasons.join(", ") ||
              (trafficMissingUnderCap ? unknownTrafficPage : "none")}
          </span>
        </span>
      </div>
      <div className="trafficRulesGridSection">
        <ConsoleDataGrid
          columns={trafficColumns}
          defaultPageSize={5}
          empty={
            trafficMissingUnderCap
              ? "No selector data for this VPS appears in the loaded traffic page; more records may exist."
              : "No traffic selectors configured."
          }
          getRowId={(row) => `${row.source}:${row.interface}:${row.direction}`}
          itemLabel="selectors"
          rows={trafficRows}
          rowsTruncated={trafficMissingUnderCap}
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
          empty={
            vpsRuleValuesTruncated
              ? "No rule values for this VPS appear in the loaded global page; more records may exist."
              : "No VPS rule values set."
          }
          getRowId={(row) => `${row.client_id}:${row.key}`}
          itemLabel="values"
          rows={vpsRuleValues}
          rowsTruncated={vpsRuleValuesTruncated}
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
          empty={
            policiesTruncated || policyAlertsTruncated
              ? "No matched policy state for this VPS appears in the loaded pages; more records may exist."
              : "No matched policy rule state for this VPS."
          }
          getRowId={(row) => `${row.policy.id}:${row.rule.id}`}
          itemLabel="policy rules"
          rows={matchedPolicyRows}
          rowsTruncated={policiesTruncated || policyAlertsTruncated}
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
          empty={
            policyAlertsTruncated
              ? "No policy alerts for this VPS appear in the loaded global page; more records may exist."
              : "No issued policy alerts."
          }
          getRowId={(row) => row.id}
          itemLabel="alerts"
          rows={policyAlerts}
          rowsTruncated={policyAlertsTruncated}
          searchPlaceholder="Search recent policy alerts"
          selectable={false}
          storageKey={`vpsman.grid.fleet.traffic.alerts.${agent.id}`}
          title="Recent policy alerts"
        />
      </div>
    </div>
  );
}

function FleetSelectionPanel({
  agents,
  allTags,
  allNetworkRates,
  latestNetworkRates,
  latestRollups,
  networkRateSelections,
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
  allNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestRollups: Map<string, TelemetryRollupRecord>;
  networkRateSelections: Map<string, NetworkRateInterfaceResolution>;
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
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");
  const [error, setError] = useState<string | null>(null);
  const selectorExpression = selectorExpressionForClientIds(
    agents.map((agent) => agent.id),
  );
  const tagNames = useMemo(() => allTags.map((tag) => tag.name), [allTags]);
  async function submitTag(action: "add" | "remove", tag: string) {
    setStatus(`${action === "add" ? "adding" : "removing"} ${tag}`);
    setStatusTone("progress");
    await runPanelAction(setPending, setError, async () => {
      const response = await mutateTagsForAgents(agents, action, tag);
      setStatus(
        `${response.action} ${response.tag}: ${response.changed_count} changed, ${response.skipped_count} skipped`,
      );
      setStatusTone("success");
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
            data-tooltip-disabled-reason={
              agents.length !== 1
                ? "Select exactly one VPS to open its file browser"
                : undefined
            }
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
            data-tooltip-disabled-reason={
              pending
                ? "A bulk tag change is already in progress"
                : !tagToAdd.trim()
                  ? "Enter a tag before adding it to the selected VPSs"
                  : undefined
            }
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
            data-tooltip-disabled-reason={
              pending
                ? "A bulk tag change is already in progress"
                : !tagToRemove.trim()
                  ? "Enter a tag before removing it from the selected VPSs"
                  : undefined
            }
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
        <ActionFeedback
          className="localActionFeedback fleetSelectionActionFeedback"
          message={error ?? status}
          tone={error ? "danger" : statusTone}
        />
      </div>
      <div
        className="selectionStatsTabs"
        role="tablist"
        aria-label="Selected VPS statistical tables"
        onKeyDown={handleTabListKeyDown}
      >
        {selectionStatsModes.map((mode) => (
          <button
            aria-controls="fleet-selection-stats-tabpanel"
            aria-selected={selectionStatsMode === mode.id}
            className={selectionStatsMode === mode.id ? "selected" : ""}
            id={tabId("fleet-selection-stats", mode.id)}
            key={mode.id}
            onClick={() => setSelectionStatsMode(mode.id)}
            role="tab"
            tabIndex={selectionStatsMode === mode.id ? 0 : -1}
            type="button"
          >
            {mode.label}
          </button>
        ))}
      </div>
      <FleetSelectionStatsTable
        agents={agents}
        allNetworkRates={allNetworkRates}
        latestNetworkRates={latestNetworkRates}
        latestRollups={latestRollups}
        mode={selectionStatsMode}
        networkRateSelections={networkRateSelections}
        tagDisplayOrder={tagDisplayOrder}
        tagVisibilityOverrides={tagVisibilityOverrides}
        vpsNameDisplayMode={vpsNameDisplayMode}
      />
    </div>
  );
}

function FleetSelectionStatsTable({
  agents,
  allNetworkRates,
  latestNetworkRates,
  latestRollups,
  mode,
  networkRateSelections,
  tagDisplayOrder,
  tagVisibilityOverrides,
  vpsNameDisplayMode,
}: {
  agents: AgentView[];
  allNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestNetworkRates: Map<string, TelemetryNetworkRateRecord[]>;
  latestRollups: Map<string, TelemetryRollupRecord>;
  mode: FleetSelectionStatsMode;
  networkRateSelections: Map<string, NetworkRateInterfaceResolution>;
  tagDisplayOrder: TagDisplayOrder;
  tagVisibilityOverrides: Record<string, boolean>;
  vpsNameDisplayMode: VpsNameDisplayMode;
}) {
  const formatBytes = useByteCountFormatter();
  const formatByteRateFromBitsPerSecond = useByteRateFormatter();
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
      <div
        aria-labelledby={tabId("fleet-selection-stats", mode)}
        id="fleet-selection-stats-tabpanel"
        role="tabpanel"
      >
        <div
          aria-label="Selected VPS network comparison"
          className="fleetSelectionStatsTable networkMode"
          role="table"
        >
          <div className="fleetSelectionStatsRow heading" role="row">
            <span role="columnheader">VPS</span>
            <span role="columnheader">Total rate</span>
            <span role="columnheader">Interface rates</span>
            <span role="columnheader">Counters</span>
          </div>
          {rows.map((agent) => {
            const rates = latestNetworkRates.get(agent.id) ?? [];
            const allRates = allNetworkRates.get(agent.id) ?? [];
            const rollup = latestRollups.get(agent.id) ?? null;
            return (
              <div className="fleetSelectionStatsRow" key={agent.id} role="row">
                <span role="cell" title={agent.id}>
                  {formatVpsName(agent, vpsNameDisplayMode)}
                </span>
                <span role="cell">
                  {formatNetworkRateSummary(
                    rates,
                    networkRateSelections.get(agent.id),
                    formatByteRateFromBitsPerSecond,
                  )}
                </span>
                <span role="cell">
                  {allRates
                    .map(
                      (rate) =>
                        `${rate.interface}: ${formatByteRateFromBitsPerSecond(rate.rx_bps_avg + rate.tx_bps_avg)}`,
                    )
                    .join("; ") || "no rate rollup"}
                </span>
                <span role="cell">
                  {formatNetworkBytes(rollup, formatBytes)}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    );
  if (mode === "overview")
    return (
      <div
        aria-labelledby={tabId("fleet-selection-stats", mode)}
        id="fleet-selection-stats-tabpanel"
        role="tabpanel"
      >
        <div
          aria-label="Selected VPS overview comparison"
          className="fleetSelectionStatsTable overviewMode"
          role="table"
        >
          <div className="fleetSelectionStatsRow heading" role="row">
            <span role="columnheader">VPS</span>
            <span role="columnheader">Status</span>
            <span role="columnheader">Location</span>
            <span role="columnheader">Provider</span>
            <span role="columnheader">Last seen</span>
            <span role="columnheader">Tags</span>
          </div>
          {rows.map((agent) => {
            const displayState = agentDisplayState(agent);
            return (
              <div className="fleetSelectionStatsRow" key={agent.id} role="row">
                <span role="cell" title={agent.id}>
                  {formatVpsName(agent, vpsNameDisplayMode)}
                </span>
                <span role="cell" title={displayState.detail}>
                  {displayState.label}
                </span>
                <span role="cell">
                  <FleetLocationValue
                    showFlag={false}
                    showRegion
                    tags={agent.tags}
                  />
                </span>
                <span role="cell">
                  {providerFromTags(agent.tags) ?? "unset"}
                </span>
                <span role="cell">{formatLastSeen(agent.last_seen_at)}</span>
                <span role="cell">
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
      </div>
    );
  if (mode === "capabilities")
    return (
      <div
        aria-labelledby={tabId("fleet-selection-stats", mode)}
        id="fleet-selection-stats-tabpanel"
        role="tabpanel"
      >
        <div
          aria-label="Selected VPS capability comparison"
          className="fleetSelectionStatsTable capabilitiesMode"
          role="table"
        >
          <div className="fleetSelectionStatsRow heading" role="row">
            <span role="columnheader">VPS</span>
            <span role="columnheader">Privilege</span>
            <span role="columnheader">UID</span>
            <span role="columnheader">Tunnels</span>
            <span role="columnheader">Process limits</span>
            <span role="columnheader">Build</span>
          </div>
          {rows.map((agent) => (
            <div className="fleetSelectionStatsRow" key={agent.id} role="row">
              <span role="cell" title={agent.id}>
                {formatVpsName(agent, vpsNameDisplayMode)}
              </span>
              <span role="cell">{formatPrivilege(agent.capabilities)}</span>
              <span role="cell">
                {agent.capabilities.effective_uid ?? "unknown"}
              </span>
              <span role="cell">
                {yesNo(agent.capabilities.can_manage_runtime_tunnels)}
              </span>
              <span role="cell">
                {yesNo(agent.capabilities.can_apply_process_limits)}
              </span>
              <span role="cell">
                {agent.internal_build_number
                  ? `#${agent.internal_build_number}`
                  : "unknown"}
              </span>
            </div>
          ))}
        </div>
      </div>
    );
  return (
    <div
      aria-labelledby={tabId("fleet-selection-stats", mode)}
      id="fleet-selection-stats-tabpanel"
      role="tabpanel"
    >
      <div
        aria-label="Selected VPS telemetry comparison"
        className="fleetSelectionStatsTable telemetryMode"
        role="table"
      >
        <div className="fleetSelectionStatsRow heading" role="row">
          <span role="columnheader">VPS</span>
          <span role="columnheader">CPU</span>
          <span role="columnheader">RAM used</span>
          <span role="columnheader">Disk used</span>
          <span role="columnheader">Network</span>
          <span role="columnheader">Samples</span>
        </div>
        {rows.map((agent) => {
          const rollup = latestRollups.get(agent.id) ?? null;
          const rates = latestNetworkRates.get(agent.id) ?? [];
          return (
            <div className="fleetSelectionStatsRow" key={agent.id} role="row">
              <span role="cell" title={agent.id}>
                {formatVpsName(agent, vpsNameDisplayMode)}
              </span>
              <span role="cell">{formatLoad(rollup?.cpu_load_1_avg)}</span>
              <span role="cell">{formatMemoryUsed(rollup, formatBytes)}</span>
              <span role="cell">{formatDiskUsed(rollup, formatBytes)}</span>
              <span role="cell">
                {formatNetworkRateSummary(
                  rates,
                  networkRateSelections.get(agent.id),
                  formatByteRateFromBitsPerSecond,
                )}
              </span>
              <span role="cell">{formatRollupSamples(rollup)}</span>
            </div>
          );
        })}
      </div>
    </div>
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

function memoryUsedRatio(
  rollup: TelemetryRollupRecord | null | undefined,
): number | null {
  if (!rollup || rollup.memory_total_bytes_max <= 0) return null;
  return rollup.memory_used_ratio_avg * 100;
}
function diskUsedRatio(
  rollup: TelemetryRollupRecord | null | undefined,
): number | null {
  if (!rollup || rollup.disk_total_bytes_max <= 0) return null;
  return rollup.disk_used_ratio_avg * 100;
}
function networkRateTotal(rates: TelemetryNetworkRateRecord[]) {
  return rates.reduce(
    (total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg,
    0,
  );
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
  labelTitle,
}: {
  children: ReactNode;
  className?: string;
  hint?: ReactNode;
  label: ReactNode;
  labelTitle?: string;
}) {
  return (
    <div className={className ? `consoleField ${className}` : "consoleField"}>
      <span title={labelTitle}>{label}</span>
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

function ConsoleFreshnessBanner({ error }: { error: string | null }) {
  if (!error) {
    return null;
  }
  return (
    <div className="consoleFreshnessBanner">
      <span>
        Last refresh failed; any previously loaded data remains visible: {error}
      </span>
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
        <span className="tokenChip" key={value}>
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
  return countryTagValue(tags);
}

function fleetLocationSortValue(
  tags: string[],
  includeRegion: boolean,
): string {
  return includeRegion
    ? [countryFromTags(tags) ?? "", regionTagValue(tags) ?? ""].join("\0")
    : (countryFromTags(tags) ?? "");
}

function fleetLocationSearchValue(tags: string[]): string {
  return [countryFromTags(tags), regionTagValue(tags)]
    .filter(Boolean)
    .join(" ");
}

function FleetLocationValue({
  showFlag,
  showRegion,
  tags,
}: {
  showFlag: boolean;
  showRegion: boolean;
  tags: string[];
}) {
  const country = countryFromTags(tags);
  const region = regionTagValue(tags);
  const location = [country ?? "unset", region].filter(Boolean).join(" · ");
  return (
    <span
      aria-label={location}
      className="historyPrimary fleetLocationValue"
      title={location}
    >
      <CountryBadge country={country} showFlag={showFlag} />
      {showRegion && region ? <small>{region}</small> : null}
    </span>
  );
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

function resourceCount(
  count: number,
  singular: string,
  plural = `${singular}s`,
): string {
  return `${count} ${count === 1 ? singular : plural}`;
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

function reviewedWebhookDispatchEventId(
  rows: Array<{ event_id?: string | null }>,
): string {
  const eventIds = Array.from(
    new Set(
      rows
        .map((row) => row.event_id?.trim())
        .filter((eventId): eventId is string => Boolean(eventId)),
    ),
  );
  if (rows.length === 0) {
    throw new Error("Webhook dispatch matched no delivery rows");
  }
  if (eventIds.length !== 1) {
    throw new Error("Webhook dispatch event ID is missing or inconsistent");
  }
  return eventIds[0];
}

function reviewedWebhookRuleSummary(rows: WebhookRuleDeliveryRecord[]): string {
  const rules = Array.from(
    new Map(
      rows.map((row) => [
        row.rule_id,
        `${row.rule_name} (${shortId(row.rule_id)})`,
      ]),
    ).values(),
  );
  return rules.join(", ") || "no rules";
}

function reviewedWebhookTargetSummary(
  rows: WebhookRuleDeliveryRecord[],
): string {
  const targets = Array.from(new Set(rows.map((row) => row.target)));
  return targets.join(", ") || "no targets";
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
        <strong>Matched VPS</strong>
        <span>{policy.matched_vps_count}</span>
      </span>
      <span>
        <strong>Active states</strong>
        <span>{policyActiveSummary(policy)}</span>
      </span>
      <span>
        <strong>Incomplete VPS</strong>
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
        <span>
          {channel.configuration_error
            ? "invalid — skipped"
            : channel.enabled
              ? "enabled"
              : "disabled"}
        </span>
      </span>
      <span>
        <strong>Categories</strong>
        <span>{tokenSummary(channel.categories, "all categories")}</span>
      </span>
      {channel.configuration_error ? (
        <span>
          <strong>Configuration</strong>
          <span>
            Stored filters are invalid; delete and replace this channel
          </span>
        </span>
      ) : null}
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
    name: "",
    enabled: true,
    condition_expression: "",
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

function policyDraftValidationMessage(
  request: PolicyDryRunRequest | FleetAlertPolicyRequest,
  requireLabels = true,
): string | null {
  if (requireLabels && !request.name.trim()) {
    return "Policy name is required";
  }
  if (!request.selector_expression.trim()) {
    return "Policy VPS selector expression is required";
  }
  const selectorParse = parseSearchExpression(request.selector_expression);
  if (selectorParse.error) {
    return `Invalid policy VPS selector: ${selectorParse.error}`;
  }
  if (request.rules.length === 0) {
    return "At least one rule row is required";
  }
  for (const [index, rule] of request.rules.entries()) {
    const row = index + 1;
    if (requireLabels && !rule.name.trim()) {
      return `Rule ${row} name is required`;
    }
    if (!rule.condition_expression.trim()) {
      return `Rule ${row} condition expression is required`;
    }
  }
  return null;
}

export function FleetAlertPolicyManager({
  agents,
  editorMode = "inline",
  onEditorOpenChange,
  onPolicyFocusChange,
  policies,
  rowsTruncated = policies.length >= FLEET_DETAIL_LIMIT,
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
  onPolicyFocusChange?: (policyId: string | null) => void;
  policies: FleetAlertPolicyRecord[];
  rowsTruncated?: boolean;
  policyAlerts: PolicyAlertRecord[];
  policyFocusId: string | null;
  policyFilterClientId: string | null;
  onDelete: (policyId: string, reviewedName: string) => Promise<void>;
  onDryRun: (request: PolicyDryRunRequest) => Promise<PolicyDryRunResponse>;
  onUpsert: (
    request: FleetAlertPolicyRequest,
  ) => Promise<FleetAlertPolicyRecord>;
}) {
  const vpsRuleSearch = useVpsRuleSearchContext();
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
  const [savePending, setSavePending] = useState(false);
  const savePendingRef = useRef(false);
  const [name, setName] = useState("");
  const [selectorExpression, setSelectorExpression] = useState("");
  const [enabled, setEnabled] = useState(false);
  const [notes, setNotes] = useState("");
  const [ruleDrafts, setRuleDrafts] = useState<PolicyRuleDraft[]>([
    defaultPolicyRuleDraft(),
  ]);
  const [dryRunPreview, setDryRunPreview] =
    useState<PolicyDryRunResponse | null>(null);
  const [dryRunPending, setDryRunPending] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");
  const policyWorkflowBusy = savePending || deletePending;
  const statusFeedbackRef = useRef<HTMLDivElement | null>(null);
  const {
    captureReviewGeneration: capturePolicyReviewGeneration,
    invalidateReviewGeneration: invalidatePolicyReviewGeneration,
    isReviewGenerationCurrent: isPolicyReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const policyDraftRequest = useMemo<PolicyDryRunRequest>(
    () => ({
      id: editingId ?? undefined,
      name: name.trim(),
      enabled,
      selector_expression: selectorExpression.trim(),
      rules: ruleDrafts.map(requestRuleFromDraft),
      notes: notes.trim() || null,
    }),
    [editingId, enabled, name, notes, ruleDrafts, selectorExpression],
  );
  const policyPreviewValidation = policyDraftValidationMessage(
    policyDraftRequest,
    false,
  );
  const policySaveValidation = policyDraftValidationMessage(
    policyDraftRequest,
    true,
  );
  const policySelectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const policySelectorEvidenceUnavailable = vpsRuleSearchUnavailable(
    selectorExpression,
    vpsRuleSearch,
  );
  const policyLocalTargets = useMemo(
    () =>
      selectorExpression.trim() &&
      !policySelectorParse.error &&
      !policySelectorEvidenceUnavailable
        ? agentsMatchingExpression(agents, selectorExpression, vpsRuleSearch)
        : [],
    [
      agents,
      policySelectorEvidenceUnavailable,
      policySelectorParse.error,
      selectorExpression,
      vpsRuleSearch,
    ],
  );

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
            <small className="monoValue" title={policy.id}>
              {shortId(policy.id)}
            </small>
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
        header: "Matched VPS",
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
        header: "Incomplete VPS",
        size: 135,
        minSize: 110,
        sortValue: (policy) => policy.incomplete_vps_count,
        cell: (policy) => policy.incomplete_vps_count,
      },
      {
        id: "last_evaluated",
        header: "Last run",
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
    invalidatePolicyReviewGeneration();
    setSaveSnapshot(null);
    setDryRunPreview(null);
    setDryRunPending(false);
    setStatus(null);
  }, [
    enabled,
    invalidatePolicyReviewGeneration,
    name,
    notes,
    ruleDrafts,
    selectorExpression,
  ]);

  useEffect(() => {
    if (status && statusTone !== "info" && statusFeedbackRef.current) {
      scrollIntoViewWithMotion(statusFeedbackRef.current, { block: "nearest" });
    }
  }, [status, statusTone]);

  useEffect(() => {
    return () => {
      invalidatePolicyReviewGeneration();
      onEditorOpenChange?.(false);
    };
  }, [invalidatePolicyReviewGeneration, onEditorOpenChange]);

  useEffect(() => {
    if (!policyFocusId) {
      if (onPolicyFocusChange) {
        setDetailPolicyId(null);
      }
      return;
    }
    invalidatePolicyReviewGeneration();
    setDryRunPending(false);
    setDryRunPreview(null);
    setSaveSnapshot(null);
    setDeleteRows(null);
    setDeleteError(null);
    const focused = policies.find((policy) => policy.id === policyFocusId);
    if (!focused) {
      updateEditorOpen(false);
      setDetailPolicyId(null);
      setPolicyStatus("Policy not found: " + shortId(policyFocusId), "danger");
      return;
    }
    updateEditorOpen(false);
    setDetailPolicyId(focused.id);
    setPolicyStatus("viewing " + focused.name, "info");
  }, [policies, policyFocusId]);

  function currentDryRunRequest(): PolicyDryRunRequest {
    return policyDraftRequest;
  }

  function reviewedUpsertRequest(
    request: PolicyDryRunRequest,
    previewHash: string,
  ): FleetAlertPolicyRequest {
    return {
      ...request,
      confirmed: true,
      preview_hash: previewHash,
    };
  }

  function beginSaveMutation() {
    if (savePendingRef.current) {
      return false;
    }
    savePendingRef.current = true;
    setSavePending(true);
    return true;
  }

  function finishSaveMutation() {
    savePendingRef.current = false;
    setSavePending(false);
  }

  function setPolicyStatus(message: string, tone: ActionFeedbackTone) {
    setStatus(message);
    setStatusTone(tone);
  }

  function updateEditorOpen(open: boolean) {
    setEditorOpen(open);
    onEditorOpenChange?.(focusedEditor && open);
  }

  function enterPolicyWorkflow(surface: "editor" | "details" | "table") {
    invalidatePolicyReviewGeneration();
    setDryRunPending(false);
    setDryRunPreview(null);
    setSaveSnapshot(null);
    setDeleteRows(null);
    setDeleteError(null);
    setStatus(null);
    if (surface !== "editor") {
      updateEditorOpen(false);
    }
    if (surface !== "details") {
      setDetailPolicyId(null);
      if (policyFocusId || detailPolicyId) {
        onPolicyFocusChange?.(null);
      }
    }
  }

  function resetForm() {
    setEditingId(null);
    setName("");
    setSelectorExpression("");
    setEnabled(false);
    setNotes("");
    setRuleDrafts([defaultPolicyRuleDraft()]);
    setDryRunPreview(null);
    setSaveSnapshot(null);
    setStatus(null);
  }

  function createPolicy() {
    if (policyWorkflowBusy) return;
    enterPolicyWorkflow("editor");
    resetForm();
    updateEditorOpen(true);
  }

  function editPolicy(policy: FleetAlertPolicyRecord) {
    if (policyWorkflowBusy) return;
    enterPolicyWorkflow("editor");
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
    setPolicyStatus("editing " + policy.name, "info");
    updateEditorOpen(true);
  }

  function openPolicyDetails(policy: FleetAlertPolicyRecord) {
    if (policyWorkflowBusy) return;
    enterPolicyWorkflow("details");
    setDetailPolicyId(policy.id);
    onPolicyFocusChange?.(policy.id);
    setPolicyStatus("viewing " + policy.name, "info");
  }

  function closePolicyDetails() {
    if (policyWorkflowBusy) return;
    setDetailPolicyId(null);
    onPolicyFocusChange?.(null);
  }

  function closePolicyEditor() {
    if (policyWorkflowBusy) return;
    enterPolicyWorkflow("table");
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

  async function dryRunCurrentPolicy(
    requireLabels = false,
    request = currentDryRunRequest(),
  ): Promise<PolicyDryRunResponse | null> {
    const reviewGeneration = capturePolicyReviewGeneration();
    const draftError = policyDraftValidationMessage(request, requireLabels);
    if (draftError) {
      setDryRunPreview(null);
      setPolicyStatus(draftError, "danger");
      throw new Error(draftError);
    }
    setDryRunPending(true);
    setPolicyStatus("dry-running policy", "progress");
    try {
      const preview = await onDryRun(request);
      if (!isPolicyReviewGenerationCurrent(reviewGeneration)) {
        return null;
      }
      setDryRunPreview(preview);
      setPolicyStatus(
        `dry-run matched ${resourceCount(
          preview.matched_vps_count,
          "VPS",
          "VPSs",
        )}`,
        "success",
      );
      return preview;
    } catch (error) {
      if (isPolicyReviewGenerationCurrent(reviewGeneration)) {
        const message =
          error instanceof Error ? error.message : "policy dry-run failed";
        setPolicyStatus(message, "danger");
      }
      throw error;
    } finally {
      if (isPolicyReviewGenerationCurrent(reviewGeneration)) {
        setDryRunPending(false);
      }
    }
  }

  async function reviewSubmit() {
    const request = currentDryRunRequest();
    try {
      const preview = await dryRunCurrentPolicy(true, request);
      if (!preview) {
        return;
      }
      setSaveSnapshot({
        request: reviewedUpsertRequest(request, preview.preview_hash),
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
      setPolicyStatus("Run dry-run and review policy before saving", "warning");
      return;
    }
    if (!beginSaveMutation()) {
      return;
    }
    setPolicyStatus(
      editingId ? "updating policy" : "creating policy",
      "progress",
    );
    try {
      const policy = await onUpsert(snapshot.request);
      setEditingId(policy.id);
      updateEditorOpen(true);
      setSaveSnapshot(null);
      setDryRunPreview(snapshot.preview);
      setPolicyStatus("saved " + policy.name, "success");
    } catch (error) {
      setPolicyStatus(
        error instanceof Error ? error.message : "policy save failed",
        "danger",
      );
    } finally {
      finishSaveMutation();
    }
  }

  function requestDeletePolicies(rows: FleetAlertPolicyRecord[]) {
    if (policyWorkflowBusy || rows.length === 0) return;
    enterPolicyWorkflow("table");
    setDeleteRows(rows);
  }

  async function confirmDeletePolicies() {
    const rows = deleteRows ?? [];
    if (rows.length === 0 || deletePending) return;
    setDeletePending(true);
    setDeleteError(null);
    setPolicyStatus("deleting policies", "progress");
    try {
      for (const policy of rows) {
        await onDelete(policy.id, policy.name);
      }
      if (rows.some((policy) => policy.id === editingId)) {
        resetForm();
        updateEditorOpen(false);
      }
      if (rows.some((policy) => policy.id === detailPolicyId)) {
        setDetailPolicyId(null);
        onPolicyFocusChange?.(null);
      }
      setDeleteRows(null);
      setPolicyStatus(
        `Deleted ${resourceCount(rows.length, "alert policy", "alert policies")}`,
        "success",
      );
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "policy delete failed";
      setDeleteError(message);
      setPolicyStatus(message, "danger");
    } finally {
      setDeletePending(false);
    }
  }

  async function setPoliciesEnabled(
    rows: FleetAlertPolicyRecord[],
    nextEnabled: boolean,
  ) {
    if (policyWorkflowBusy || rows.length === 0) return;
    enterPolicyWorkflow("table");
    if (!beginSaveMutation()) {
      return;
    }
    setPolicyStatus(
      nextEnabled ? "enabling policies" : "disabling policies",
      "progress",
    );
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
      setPolicyStatus(
        `${nextEnabled ? "Enabled" : "Disabled"} ${resourceCount(rows.length, "alert policy", "alert policies")}`,
        "success",
      );
    } catch (error) {
      setPolicyStatus(
        error instanceof Error ? error.message : "policy update failed",
        "danger",
      );
    } finally {
      finishSaveMutation();
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
      disabled: (rows) => policyWorkflowBusy || rows.length !== 1,
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
      disabled: (rows) => policyWorkflowBusy || rows.length !== 1,
      icon: <Pencil size={14} />,
      onSelect: (rows) => rows[0] && editPolicy(rows[0]),
    },
    {
      label: "Enable",
      description: (rows) =>
        "Enable " +
        rows.filter((policy) => !policy.enabled).length +
        " disabled selected policy groups.",
      disabled: (rows) =>
        policyWorkflowBusy ||
        rows.filter((policy) => !policy.enabled).length === 0,
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
      disabled: (rows) =>
        policyWorkflowBusy ||
        rows.filter((policy) => policy.enabled).length === 0,
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
      disabled: (rows) => policyWorkflowBusy || rows.length === 0,
      icon: <Trash2 size={14} />,
      onSelect: requestDeletePolicies,
      tone: "danger",
    },
  ];

  const detailPolicy = detailPolicyId
    ? policies.find((candidate) => candidate.id === detailPolicyId)
    : null;
  const showPolicyList = !focusedEditor || !editorOpen;
  const policySaveReviewPrompt = (
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
          label: "Matched VPS",
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
      pending={savePending}
      error={saveSnapshot && statusTone === "danger" ? status : null}
      title="Confirm alert policy save"
    />
  );

  return (
    <div
      className={
        focusedEditor && editorOpen
          ? "consoleCrudPanel focusedPolicyEditor"
          : "consoleCrudPanel"
      }
    >
      {status ? (
        <div ref={statusFeedbackRef}>
          <ActionFeedback
            className="localActionFeedback fleetPolicyActionFeedback"
            message={status}
            tone={statusTone}
          />
        </div>
      ) : null}
      <div className="consoleResourceLayout fullWidth">
        {showPolicyList && policyFilterClientId ? (
          <div className="consoleInlineNotice policyFocusNotice">
            <strong>
              Focused VPS:{" "}
              <span className="monoValue">{policyFilterClientId}</span>
            </strong>
            <small>
              Policy rows show server-evaluated match counts; open a policy
              dry-run to inspect exact matched VPSs.
            </small>
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
            rowsTruncated={rowsTruncated}
            searchPlaceholder="Search policies by name, selector, rules, or notes"
            storageKey="vpsman.grid.fleet.alertPolicies.v3"
            title="Policy groups"
            toolbarActions={
              <button
                className="primaryAction compactAction"
                data-tooltip-disabled-reason={
                  policyWorkflowBusy
                    ? "An alert policy save or delete is already in progress"
                    : undefined
                }
                disabled={policyWorkflowBusy}
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
            onClose={closePolicyDetails}
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
                    data-tooltip-disabled-reason={
                      dryRunPending || savePending
                        ? "An alert policy preview or save is already in progress"
                        : (policyPreviewValidation ?? undefined)
                    }
                    disabled={
                      dryRunPending ||
                      savePending ||
                      policyPreviewValidation !== null
                    }
                    title={
                      policyPreviewValidation ??
                      "Preview exact VPS matches with the current draft"
                    }
                    type="button"
                    onClick={() => void dryRunCurrentPolicy()}
                  >
                    Preview matches
                  </button>
                  <button
                    className="primaryAction"
                    data-tooltip-disabled-reason={
                      dryRunPending || savePending
                        ? "An alert policy preview or save is already in progress"
                        : (policySaveValidation ?? undefined)
                    }
                    disabled={
                      dryRunPending ||
                      savePending ||
                      policySaveValidation !== null
                    }
                    title={
                      policySaveValidation ??
                      "Dry-run and review the complete policy before saving"
                    }
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
                    data-tooltip-disabled-reason={
                      dryRunPending || savePending
                        ? "An alert policy preview or save is already in progress"
                        : (policyPreviewValidation ?? undefined)
                    }
                    disabled={
                      dryRunPending ||
                      savePending ||
                      policyPreviewValidation !== null
                    }
                    title={
                      policyPreviewValidation ??
                      "Preview exact VPS matches with the current draft"
                    }
                    type="button"
                    onClick={() => void dryRunCurrentPolicy()}
                  >
                    Dry-run
                  </button>
                  <button
                    className="primaryAction"
                    data-tooltip-disabled-reason={
                      dryRunPending || savePending
                        ? "An alert policy preview or save is already in progress"
                        : (policySaveValidation ?? undefined)
                    }
                    disabled={
                      dryRunPending ||
                      savePending ||
                      policySaveValidation !== null
                    }
                    title={
                      policySaveValidation ??
                      "Dry-run and review the complete policy before saving"
                    }
                    type="button"
                    onClick={() => void reviewSubmit()}
                  >
                    {editingId ? "Review update" : "Review create"}
                  </button>
                  <button
                    className="secondaryAction"
                    data-tooltip-disabled-reason={
                      policyWorkflowBusy
                        ? "An alert policy save or delete is already in progress"
                        : undefined
                    }
                    disabled={policyWorkflowBusy}
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
            onClose={closePolicyEditor}
            reviewPrompt={policySaveReviewPrompt}
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
                  placeholder="Edge traffic budget"
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
                hint="Use the same fleet selector expression design as saved fleet views and job targeting, including id:<client_id>, name:<name>, tag:<tag>, provider:<provider>, country:<code>, region:<region>, status:<state>, &&, ||, !, and parentheses."
                label="VPS selector expression"
              >
                <SearchExpressionInput
                  agents={agents}
                  ariaLabel="Policy VPS selector expression"
                  onChange={(value) => {
                    setSelectorExpression(value);
                    setDryRunPreview(null);
                    setSaveSnapshot(null);
                  }}
                  placeholder="tag:edge && provider:hetzner"
                  showMatchCount
                  value={selectorExpression}
                  verification={
                    policySelectorParse.error
                      ? "invalid"
                      : selectorExpression.trim()
                        ? "valid"
                        : "neutral"
                  }
                  verificationMessage={
                    (policySelectorEvidenceUnavailable
                      ? VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE
                      : policySelectorParse.error) ??
                    (selectorExpression.trim()
                      ? `${policyLocalTargets.length}/${agents.length}`
                      : "required")
                  }
                />
                {!policySelectorEvidenceUnavailable ? (
                  <LocalTargetPreview
                    agents={policyLocalTargets}
                    ariaLabel="Alert policy local VPS preview"
                  />
                ) : null}
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
                  <span>
                    {ruleDrafts.length}{" "}
                    {ruleDrafts.length === 1 ? "rule row" : "rule rows"}
                  </span>
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
                        data-tooltip-disabled-reason={
                          ruleDrafts.length <= 1
                            ? "An alert policy must keep at least one rule"
                            : undefined
                        }
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
                          placeholder="80% total quota"
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
                        labelTitle="Supported metrics: traffic quota/cycle values, cpu.load_1, cpu.load_saturation, memory.available_ratio, and disk.available_ratio. Operators: >, >=, <, <=, =, !=, arithmetic, &&, ||, and parentheses."
                      >
                        <textarea
                          aria-label="Rule condition expression"
                          placeholder="traffic.cycle.total >= traffic.quota.total * 0.8"
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
          <strong>Matched VPS</strong>
          <span>{preview.matched_vps_count}</span>
        </span>
        <span>
          <strong>Incomplete VPS</strong>
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
      <ActionFeedback
        className="policyDryRunValidationFeedback"
        message={
          preview.validation_errors.length > 0
            ? preview.validation_errors.join(" · ")
            : null
        }
        tone="warning"
      />
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
        {preview.matched_vps.map((clientId) => (
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
          ? `${resourceCount(
              preview.incomplete_vps_count,
              "incomplete VPS",
              "incomplete VPSs",
            )}; ${resourceCount(
              preview.invalid_rule_count,
              "invalid rule row",
              "invalid rule rows",
            )}.`
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
        onKeyDown={handleTabListKeyDown}
      >
        {[
          ["channels", "Channels"],
          ["webhooks", "Webhooks"],
          ["deliveries", "Deliveries"],
          ["maintenance", "Maintenance"],
        ].map(([id, label]) => (
          <button
            aria-controls="notification-registry-tabpanel"
            aria-selected={tab === id}
            className={tab === id ? "active" : undefined}
            id={tabId("notification-registry", id)}
            key={id}
            onClick={() => setTab(id as NotificationRegistryTab)}
            role="tab"
            tabIndex={tab === id ? 0 : -1}
            type="button"
          >
            {label}
          </button>
        ))}
      </div>
      <div
        aria-labelledby={tabId("notification-registry", tab)}
        id="notification-registry-tabpanel"
        role="tabpanel"
      >
        {tab === "channels" && (
          <FleetAlertNotificationManager
            agents={agents}
            channels={alertChannels}
            deliveries={alertDeliveries}
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
            deliveries={webhookDeliveries}
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
                  <WebhookDryRunNotice
                    agents={agents}
                    preview={dryRunPreview}
                  />
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
  deliveries,
  onDelete,
  onDispatch,
  onOpenDeliveries,
  onPreviewRows,
  onProcess,
  onUpsert,
  queueMode = "full",
  rowsTruncated = channels.length >= FLEET_DETAIL_LIMIT,
}: {
  agents: AgentView[];
  channels: FleetAlertNotificationChannelRecord[];
  deliveries: FleetAlertNotificationDeliveryRecord[];
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
  rowsTruncated?: boolean;
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [detailChannelId, setDetailChannelId] = useState<string | null>(null);
  const [deleteRows, setDeleteRows] = useState<
    FleetAlertNotificationChannelRecord[] | null
  >(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [enableRows, setEnableRows] = useState<
    FleetAlertNotificationChannelRecord[] | null
  >(null);
  const [saveSnapshot, setSaveSnapshot] = useState<{
    request: FleetAlertNotificationChannelRequest;
    title: string;
  } | null>(null);
  const [savePending, setSavePending] = useState(false);
  const savePendingRef = useRef(false);
  const [name, setName] = useState("");
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
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");
  const statusFeedbackRef = useRef<HTMLDivElement | null>(null);
  const [queueConfirmation, setQueueConfirmation] = useState<
    "dispatch" | "process" | null
  >(null);
  const [queueSnapshot, setQueueSnapshot] =
    useState<AlertDeliveryQueueSnapshot | null>(null);
  const [queuePending, setQueuePending] = useState(false);
  const channelWorkflowBusy = savePending || deletePending || queuePending;
  const hasEnabledChannels = channels.some(
    (channel) => channel.enabled && !channel.configuration_error,
  );
  const hasQueuedDeliveries = deliveries.some(
    (delivery) => delivery.status === "queued",
  );

  const categoryTokens = useMemo(() => csvValues(categories), [categories]);
  const operatorStateTokens = useMemo(
    () => csvValues(operatorStates),
    [operatorStates],
  );
  const channelDraftValidation = notificationChannelDraftValidationMessage({
    cooldownSecs,
    name,
    scopeKind,
    scopeValue,
    target,
  });

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
          `${channel.categories.join(" ")} ${channel.operator_states.join(" ")} ${channel.configuration_error ?? ""}`,
        cell: (channel) => (
          <span className="historyPrimary">
            <strong>
              {channel.configuration_error
                ? "Invalid stored filters"
                : channel.categories.length > 0
                  ? channel.categories.join(", ")
                  : "all categories"}
            </strong>
            <small>
              {channel.configuration_error
                ? "Channel is skipped until replaced"
                : channel.operator_states.length > 0
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
          <span
            className="historyPrimary"
            title={`${channel.delivery_kind}: ${channel.target}`}
          >
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
        searchValue: (channel) =>
          channel.configuration_error
            ? "invalid configuration"
            : channel.enabled
              ? "enabled"
              : "disabled",
        cell: (channel) => (
          <ConsoleStatusBadge
            tone={
              channel.configuration_error
                ? "critical"
                : channel.enabled
                  ? "ok"
                  : "warning"
            }
          >
            {channel.configuration_error
              ? "invalid"
              : channel.enabled
                ? "enabled"
                : "disabled"}
          </ConsoleStatusBadge>
        ),
      },
    ],
    [],
  );

  useEffect(() => {
    setSaveSnapshot(null);
    setStatus(null);
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

  useEffect(() => {
    if (status && statusTone !== "info" && statusFeedbackRef.current) {
      scrollIntoViewWithMotion(statusFeedbackRef.current, { block: "nearest" });
    }
  }, [status, statusTone]);

  function resetForm() {
    setEditingId(null);
    setName("");
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

  function setChannelStatus(message: string, tone: ActionFeedbackTone) {
    setStatus(message);
    setStatusTone(tone);
  }

  function enterChannelWorkflow(
    surface: "editor" | "details" | "table" | "queue",
  ) {
    setSaveSnapshot(null);
    setEnableRows(null);
    setDeleteRows(null);
    setDeleteError(null);
    setQueueConfirmation(null);
    setQueueSnapshot(null);
    setStatus(null);
    if (surface !== "editor") {
      setEditorOpen(false);
    }
    if (surface !== "details") {
      setDetailChannelId(null);
    }
  }

  function createChannel() {
    if (channelWorkflowBusy) return;
    enterChannelWorkflow("editor");
    resetForm();
    setEditorOpen(true);
  }

  function editChannel(channel: FleetAlertNotificationChannelRecord) {
    if (channelWorkflowBusy) return;
    if (channel.configuration_error) {
      setChannelStatus(
        "Stored channel filters are invalid. Delete this channel and create a reviewed replacement.",
        "danger",
      );
      return;
    }
    enterChannelWorkflow("editor");
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
    setSaveSnapshot(null);
    setChannelStatus(`editing ${channel.name}`, "info");
    setEditorOpen(true);
  }

  function closeChannelEditor() {
    if (savePending) return;
    setSaveSnapshot(null);
    setEditorOpen(false);
  }

  function openChannelDetails(channel: FleetAlertNotificationChannelRecord) {
    if (channelWorkflowBusy) return;
    enterChannelWorkflow("details");
    setDetailChannelId(channel.id);
    setChannelStatus(`viewing ${channel.name}`, "info");
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

  function beginSaveMutation() {
    if (savePendingRef.current) {
      return false;
    }
    savePendingRef.current = true;
    setSavePending(true);
    return true;
  }

  function finishSaveMutation() {
    savePendingRef.current = false;
    setSavePending(false);
  }

  function reviewSubmit() {
    if (channelDraftValidation) {
      setChannelStatus(channelDraftValidation, "danger");
      return;
    }
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
      setChannelStatus("Review channel before saving", "warning");
      return;
    }
    if (!beginSaveMutation()) {
      return;
    }
    setChannelStatus(
      editingId ? "updating channel" : "creating channel",
      "progress",
    );
    try {
      const channel = await onUpsert(snapshot.request);
      setEditingId(channel.id);
      setEditorOpen(true);
      setSaveSnapshot(null);
      setChannelStatus(`saved ${channel.name}`, "success");
    } catch (error) {
      setChannelStatus(
        error instanceof Error ? error.message : "channel save failed",
        "danger",
      );
    } finally {
      finishSaveMutation();
    }
  }

  function requestDeleteChannels(rows: FleetAlertNotificationChannelRecord[]) {
    if (channelWorkflowBusy) return;
    enterChannelWorkflow("table");
    setDeleteError(null);
    setDeleteRows(rows);
  }

  async function confirmDeleteChannels() {
    const rows = deleteRows ?? [];
    if (rows.length === 0 || deletePending) return;
    setDeletePending(true);
    setDeleteError(null);
    setChannelStatus("deleting channels", "progress");
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
      setChannelStatus(
        `Deleted ${resourceCount(rows.length, "notification channel")}`,
        "success",
      );
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "channel delete failed";
      setDeleteError(message);
      setChannelStatus(message, "danger");
    } finally {
      setDeletePending(false);
    }
  }

  async function setChannelsEnabled(
    rows: FleetAlertNotificationChannelRecord[],
    nextEnabled: boolean,
  ): Promise<boolean> {
    if (rows.length === 0) return false;
    if (!beginSaveMutation()) {
      return false;
    }
    setChannelStatus(
      nextEnabled ? "enabling channels" : "disabling channels",
      "progress",
    );
    try {
      for (const channel of rows) {
        await onUpsert(requestFromChannel(channel, { enabled: nextEnabled }));
      }
      setChannelStatus(
        `${nextEnabled ? "Enabled" : "Disabled"} ${resourceCount(rows.length, "notification channel")}`,
        "success",
      );
      return true;
    } catch (error) {
      setChannelStatus(
        error instanceof Error ? error.message : "channel update failed",
        "danger",
      );
      return false;
    } finally {
      finishSaveMutation();
    }
  }

  function requestEnableChannels(rows: FleetAlertNotificationChannelRecord[]) {
    if (channelWorkflowBusy) return;
    const disabledRows = rows.filter((channel) => !channel.enabled);
    if (disabledRows.length === 0) return;
    enterChannelWorkflow("table");
    setEnableRows(disabledRows);
  }

  async function confirmEnableChannels() {
    const rows = enableRows ?? [];
    if (await setChannelsEnabled(rows, true)) {
      setEnableRows(null);
    }
  }

  async function dispatch(dryRun: boolean, openConfirmation = false) {
    if (channelWorkflowBusy) {
      return;
    }
    if (!hasEnabledChannels) {
      setChannelStatus(
        "Create and enable a notification channel before matching alerts",
        "warning",
      );
      return;
    }
    enterChannelWorkflow("queue");
    setChannelStatus(
      dryRun ? "matching alerts" : "queueing alert notifications",
      "progress",
    );
    setQueuePending(true);
    try {
      const rows = await onDispatch({
        limit: 50,
        dry_run: dryRun,
        confirmed: !dryRun,
      });
      if (dryRun) {
        if (!openConfirmation) {
          onPreviewRows(rows);
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
      setChannelStatus(
        dryRun
          ? `Matched ${resourceCount(rows.length, "alert")}`
          : `Queued ${resourceCount(rows.length, "notification delivery", "notification deliveries")}`,
        "success",
      );
    } catch (error) {
      setChannelStatus(
        error instanceof Error ? error.message : "notification dispatch failed",
        "danger",
      );
    } finally {
      setQueuePending(false);
    }
  }

  async function process(dryRun: boolean, openConfirmation = false) {
    if (channelWorkflowBusy) {
      return;
    }
    if (!hasQueuedDeliveries) {
      setChannelStatus(
        "No queued notification deliveries are available",
        "info",
      );
      return;
    }
    enterChannelWorkflow("queue");
    setChannelStatus(
      dryRun ? "previewing notification queue" : "delivering notifications",
      "progress",
    );
    setQueuePending(true);
    try {
      const rows = await onProcess({
        limit: 50,
        status: "queued",
        dry_run: dryRun,
        confirmed: !dryRun,
      });
      if (dryRun) {
        if (!openConfirmation) {
          onPreviewRows(rows);
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
      setChannelStatus(
        `${dryRun ? "Previewed" : "Processed"} ${resourceCount(rows.length, "notification delivery", "notification deliveries")}`,
        "success",
      );
    } catch (error) {
      setChannelStatus(
        error instanceof Error
          ? error.message
          : "notification processing failed",
        "danger",
      );
    } finally {
      setQueuePending(false);
    }
  }

  async function confirmQueueAction() {
    const snapshot = queueSnapshot;
    if (!snapshot || queuePending) {
      return;
    }
    setQueuePending(true);
    setChannelStatus(
      snapshot.action === "dispatch"
        ? "queueing reviewed alert notifications"
        : "delivering reviewed notifications",
      "progress",
    );
    try {
      const rows =
        snapshot.action === "dispatch"
          ? await onDispatch(snapshot.request)
          : await onProcess(snapshot.request);
      setChannelStatus(
        snapshot.action === "dispatch"
          ? `Queued ${resourceCount(rows.length, "notification delivery", "notification deliveries")}`
          : `Processed ${resourceCount(rows.length, "notification delivery", "notification deliveries")}`,
        "success",
      );
      setQueueConfirmation(null);
      setQueueSnapshot(null);
    } catch (error) {
      setChannelStatus(
        error instanceof Error
          ? error.message
          : "notification queue action failed",
        "danger",
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
        disabled: (rows) => channelWorkflowBusy || rows.length !== 1,
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
        disabled: (rows) =>
          channelWorkflowBusy ||
          rows.length !== 1 ||
          Boolean(rows[0]?.configuration_error),
        icon: <Pencil size={14} />,
        onSelect: (rows) => rows[0] && editChannel(rows[0]),
      },
      {
        label: "Enable",
        description: (rows) =>
          `Enable ${rows.filter((channel) => !channel.enabled).length} disabled selected notification channel records.`,
        disabled: (rows) =>
          channelWorkflowBusy ||
          rows.some((channel) => Boolean(channel.configuration_error)) ||
          rows.filter((channel) => !channel.enabled).length === 0,
        icon: <Power size={14} />,
        onSelect: requestEnableChannels,
      },
      {
        label: "Disable",
        description: (rows) =>
          `Disable ${rows.filter((channel) => channel.enabled).length} enabled selected notification channel records.`,
        disabled: (rows) =>
          channelWorkflowBusy ||
          rows.some((channel) => Boolean(channel.configuration_error)) ||
          rows.filter((channel) => channel.enabled).length === 0,
        icon: <PowerOff size={14} />,
        onSelect: (rows) => {
          enterChannelWorkflow("table");
          void setChannelsEnabled(
            rows.filter((channel) => channel.enabled),
            false,
          );
        },
      },
      {
        label: "Review deletion",
        description: (rows) =>
          `Delete ${rows.length} selected notification channel records. Retained delivery history is not removed.`,
        disabled: (rows) => channelWorkflowBusy || rows.length === 0,
        icon: <Trash2 size={14} />,
        onSelect: requestDeleteChannels,
        tone: "danger",
      },
    ];
  const detailChannel = detailChannelId
    ? (channels.find((channel) => channel.id === detailChannelId) ?? null)
    : null;
  const notificationSaveReviewPrompt = (
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
          label: "Categories",
          value: saveSnapshot?.request.categories?.join(", ") || "all",
        },
        {
          label: "Operator states",
          value: saveSnapshot?.request.operator_states?.join(", ") || "all",
        },
        {
          label: "State",
          value: saveSnapshot?.request.enabled ? "enabled" : "disabled",
        },
        {
          label: "Delivery",
          value: saveSnapshot
            ? `${saveSnapshot.request.delivery_kind} -> ${saveSnapshot.request.target}`
            : "-",
        },
        {
          label: "Cooldown",
          value: saveSnapshot
            ? `${saveSnapshot.request.cooldown_secs ?? 300} seconds`
            : "-",
        },
        { label: "Notes", value: saveSnapshot?.request.notes || "None" },
      ]}
      onCancel={() => setSaveSnapshot(null)}
      onConfirm={() => void submit()}
      open={saveSnapshot !== null}
      pending={savePending}
      error={saveSnapshot && statusTone === "danger" ? status : null}
      title="Confirm notification channel save"
    />
  );

  return (
    <div className="consoleCrudPanel">
      {status ? (
        <div ref={statusFeedbackRef}>
          <ActionFeedback
            className="localActionFeedback fleetPolicyActionFeedback"
            message={status}
            tone={statusTone}
          />
        </div>
      ) : null}
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
          rowsTruncated={rowsTruncated}
          searchPlaceholder="Search channels by name, scope, delivery target, or filters"
          storageKey="vpsman.grid.fleet.notificationChannels.v2"
          title="Alert notification channels"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              data-tooltip-disabled-reason={
                channelWorkflowBusy
                  ? "A notification channel or delivery operation is already in progress"
                  : undefined
              }
              disabled={channelWorkflowBusy}
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
                data-tooltip-disabled-reason={
                  !detailChannel
                    ? "This notification channel no longer exists"
                    : detailChannel.configuration_error
                      ? "Stored filters are invalid; delete and replace this channel"
                      : undefined
                }
                disabled={
                  !detailChannel || Boolean(detailChannel.configuration_error)
                }
                title={
                  detailChannel?.configuration_error
                    ? "Stored filters are invalid; delete and replace this channel"
                    : "Edit notification channel"
                }
                type="button"
                onClick={() => {
                  if (detailChannel) {
                    editChannel(detailChannel);
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
            {detailChannel ? (
              <ChannelDetailGrid channel={detailChannel} />
            ) : (
              <span className="mutedText">Channel no longer exists.</span>
            )}
          </ConsoleDetailPanel>
        ) : null}
        {editorOpen ? (
          <ConsoleDetailPanel
            actions={
              <>
                <button
                  className="primaryAction"
                  data-tooltip-disabled-reason={
                    savePending
                      ? "A notification channel save is already in progress"
                      : (channelDraftValidation ?? undefined)
                  }
                  disabled={savePending || channelDraftValidation !== null}
                  title={
                    channelDraftValidation ??
                    (editingId
                      ? "Review the channel update"
                      : "Review the new channel")
                  }
                  type="button"
                  onClick={reviewSubmit}
                >
                  {editingId ? "Review update" : "Review create"}
                </button>
              </>
            }
            description="Routes alert deliveries through explicit saved records."
            onClose={closeChannelEditor}
            reviewPrompt={notificationSaveReviewPrompt}
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
                  placeholder="e.g. Production alerts"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField label="Scope kind">
                <select
                  aria-label="Notification scope kind"
                  value={scopeKind}
                  onChange={(event) => {
                    setScopeKind(event.target.value);
                    setScopeValue("");
                  }}
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
                    data-tooltip-disabled-reason={
                      scopeKind === "global"
                        ? "Global notification channels do not use a scope value"
                        : undefined
                    }
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
                  <span>
                    {editingId ? "Channel enabled" : "Enable after saving"}
                  </span>
                </label>
              </ConsoleField>
              <ConsoleField
                label="Categories"
                className="fieldWide"
                hint="Comma-separated alert categories. Empty matches all categories."
              >
                <input
                  aria-label="Alert categories"
                  placeholder="resource, network, backup"
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
                  placeholder="open, acknowledged, muted, escalated"
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
                labelTitle="HTTPS endpoint that receives alert notification deliveries."
                className="fieldWide"
                hint="Delivery is sent by the vpsman server. Production targets require a public HTTPS URL; local HTTP requires explicit server development opt-in."
              >
                <input
                  aria-label="Delivery target"
                  placeholder="https://hooks.example/vpsman"
                  value={target}
                  onChange={(event) => setTarget(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField
                label="Cooldown seconds"
                hint="Minimum time between automatic deliveries for this channel; enter a whole number from 0 to 2592000."
              >
                <input
                  aria-label="Notification cooldown seconds"
                  max={2592000}
                  min={0}
                  type="number"
                  value={cooldownSecs}
                  onChange={(event) => setCooldownSecs(event.target.value)}
                />
              </ConsoleField>
              <ConsoleField
                label="Outbound message format"
                className="fieldFull"
                hint="Notification channels send this fixed structured JSON envelope. Use Event webhooks when a custom rendered message is required."
              >
                <pre className="monoValue notificationPayloadExample">
                  {ALERT_NOTIFICATION_PAYLOAD_EXAMPLE}
                </pre>
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
            {!hasEnabledChannels
              ? "Create and enable a destination before previewing or queueing alert notifications."
              : queueMode === "configuration"
                ? "Preview matching alerts, queue or deliver reviewed records, or open delivery evidence."
                : "Review matching or process queued deliveries without leaving the registry."}
          </small>
        </span>
        <div className="consoleOperationsActions">
          <button
            className="secondaryAction"
            data-tooltip-disabled-reason={
              channelWorkflowBusy
                ? "A notification channel or delivery operation is already in progress"
                : !hasEnabledChannels
                  ? "Create and enable a notification channel first"
                  : undefined
            }
            disabled={channelWorkflowBusy || !hasEnabledChannels}
            title={
              hasEnabledChannels
                ? "Preview active alerts matched by enabled notification channels"
                : "Create and enable a notification channel first"
            }
            type="button"
            onClick={() => void dispatch(true)}
          >
            {queueMode === "configuration" ? "Preview match" : "Review matches"}
          </button>
          <button
            className="secondaryAction"
            data-tooltip-disabled-reason={
              channelWorkflowBusy
                ? "A notification channel or delivery operation is already in progress"
                : !hasEnabledChannels
                  ? "Create and enable a notification channel first"
                  : undefined
            }
            disabled={channelWorkflowBusy || !hasEnabledChannels}
            title={
              hasEnabledChannels
                ? "Review matching alerts before queueing notification deliveries"
                : "Create and enable a notification channel first"
            }
            type="button"
            onClick={() => void dispatch(true, true)}
          >
            Queue dispatch
          </button>
          {queueMode === "full" ? (
            <>
              <button
                className="secondaryAction"
                data-tooltip-disabled-reason={
                  channelWorkflowBusy
                    ? "A notification channel or delivery operation is already in progress"
                    : !hasQueuedDeliveries
                      ? "No queued notification deliveries are available"
                      : undefined
                }
                disabled={channelWorkflowBusy || !hasQueuedDeliveries}
                title={
                  hasQueuedDeliveries
                    ? "Preview queued notification deliveries"
                    : "No queued notification deliveries are available"
                }
                type="button"
                onClick={() => void process(true)}
              >
                Review queued deliveries
              </button>
            </>
          ) : (
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason={
                channelWorkflowBusy
                  ? "A notification channel or delivery operation is already in progress"
                  : undefined
              }
              disabled={channelWorkflowBusy}
              type="button"
              onClick={onOpenDeliveries}
            >
              Open delivery
            </button>
          )}
          <button
            className="primaryAction"
            data-tooltip-disabled-reason={
              channelWorkflowBusy
                ? "A notification channel or delivery operation is already in progress"
                : !hasQueuedDeliveries
                  ? "No queued notification deliveries are available"
                  : undefined
            }
            disabled={channelWorkflowBusy || !hasQueuedDeliveries}
            title={
              hasQueuedDeliveries
                ? "Review queued notification deliveries before sending"
                : "No queued notification deliveries are available"
            }
            type="button"
            onClick={() => void process(true, true)}
          >
            Deliver queued
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
            title: queueSnapshot?.previewHash ?? "review required",
          },
        ]}
        onCancel={() => {
          setQueueConfirmation(null);
          setQueueSnapshot(null);
        }}
        onConfirm={() => void confirmQueueAction()}
        open={queueConfirmation !== null && queueSnapshot !== null}
        pending={queuePending}
        error={queueConfirmation && statusTone === "danger" ? status : null}
        title={
          queueConfirmation === "dispatch"
            ? "Confirm notification queue dispatch"
            : "Confirm notification delivery"
        }
        tone={queueConfirmation === "process" ? "danger" : "normal"}
      />
      <ConfirmationPrompt
        confirmLabel="Enable notification channels"
        detail="Enabling starts matching future alerts and can send requests to the configured external targets. Disabling remains immediate."
        items={[
          {
            label: "Channels",
            value: selectedRecordSummary(
              enableRows,
              "channel",
              "channels",
              (row) => row.name,
              (row) => row.id,
            ),
          },
          {
            label: "Targets",
            value: enableRows?.map((row) => row.target).join(", ") ?? "-",
          },
        ]}
        onCancel={() => setEnableRows(null)}
        onConfirm={() => void confirmEnableChannels()}
        open={enableRows !== null}
        pending={savePending}
        error={enableRows && statusTone === "danger" ? status : null}
        title="Confirm notification enable"
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
  rowsTruncated = !preview && deliveries.length >= FLEET_DETAIL_LIMIT,
}: {
  deliveries: FleetAlertNotificationDeliveryRecord[];
  preview: boolean;
  rowsTruncated?: boolean;
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
            <small title={delivery.alert_id}>
              {shortId(delivery.alert_id)}
            </small>
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
          <span
            className="historyPrimary"
            title={`${delivery.delivery_kind}: ${delivery.target}`}
          >
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
            <span className="deliveryErrorText">error: {delivery.error}</span>
          )}
        </div>
      )}
      rows={deliveries}
      rowsTruncated={rowsTruncated}
      searchPlaceholder="Search notification deliveries"
      selectable={false}
      storageKey="vpsman.grid.fleet.notificationDeliveries.v2"
      title={
        preview
          ? "Notification delivery preview"
          : "Notification delivery history"
      }
    />
  );
}

const ALERT_NOTIFICATION_PAYLOAD_EXAMPLE = `{
  "schema": "vpsman.fleet_alert.webhook_delivery.v1",
  "delivery": {
    "id": "...",
    "channel_id": "...",
    "channel_name": "Production alerts",
    "alert_id": "...",
    "alert_severity": "critical",
    "alert_category": "resource",
    "dedupe_key": "fleet-alert-notification:...",
    "attempt": 1,
    "created_at": "2026-08-05T00:00:00Z"
  },
  "payload": {
    "schema": "vpsman.fleet_alert.notification.v1",
    "channel": {
      "id": "...",
      "name": "Production alerts",
      "scope_kind": "global",
      "scope_value": null,
      "delivery_kind": "webhook",
      "target": "https://alerts.example.net/vpsman"
    },
    "alert": {
      "id": "...",
      "severity": "critical",
      "category": "resource",
      "target_kind": "agent",
      "target_id": "v-1",
      "client_id": "v-1",
      "title": "CPU load critical",
      "detail": "...",
      "status": "firing",
      "evidence": {},
      "observed_at": "2026-08-05T00:00:00Z",
      "operator_state": "open",
      "muted_until_unix": null,
      "escalation_level": 0,
      "state_reason": null,
      "state_actor_id": null,
      "state_updated_at": null
    }
  }
}`;

const DEFAULT_WEBHOOK_BODY_TEMPLATE = `{#
Alert: [{alert.severity}] {alert.title} on {vps.display_name} ({event.id})
Traffic threshold: {vps.display_name} used {traffic.cycle_percent}% in {policy.name}; source rule {policy_rule.name}
Resource threshold: [{alert.severity}] {alert.title} on {vps.display_name}; condition {policy_rule.condition_expression}
VPS status event: [{event.kind}] {vps.display_name} is {vps.status}
Interval fleet summary: [{event.kind}] {matched_vps.length} VPSs: {matched_vps.map(vps.name).join(", ")}
#}
[{event.kind}] {rule.name}: {vps.display_name} ({vps.id}) is {vps.status}`;

function notificationChannelDraftValidationMessage({
  cooldownSecs,
  name,
  scopeKind,
  scopeValue,
  target,
}: {
  cooldownSecs: string;
  name: string;
  scopeKind: string;
  scopeValue: string;
  target: string;
}): string | null {
  if (!name.trim()) return "Channel name is required";
  if (name.trim().length > 128) return "Channel name is too long";
  if (scopeKind !== "global" && !scopeValue.trim()) {
    return "Scope value is required for this scope kind";
  }
  if (!target.trim()) return "Delivery target URL is required";
  try {
    const parsed = new URL(target.trim());
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return "Delivery target URL must use HTTP or HTTPS";
    }
  } catch {
    return "Delivery target URL is invalid";
  }
  const cooldown = Number(cooldownSecs);
  if (
    !cooldownSecs.trim() ||
    !Number.isInteger(cooldown) ||
    cooldown < 0 ||
    cooldown > 2_592_000
  ) {
    return "Cooldown must be a whole number from 0 to 2592000 seconds";
  }
  return null;
}

function webhookRuleDraftValidationMessage({
  bodyTemplate,
  cooldownSecs,
  eventKind,
  expression,
  name,
  target,
}: {
  bodyTemplate: string;
  cooldownSecs: string;
  eventKind: string;
  expression: string;
  name: string;
  target: string;
}): string | null {
  if (!name.trim()) return "Rule name is required";
  if (!expression.trim()) return "Expression is required";
  if (!target.trim()) return "Target URL is required";
  try {
    const parsed = new URL(target.trim());
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return "Target URL must use HTTP or HTTPS";
    }
  } catch {
    return "Target URL is invalid";
  }
  if (!eventKind.trim()) return "Preview event kind is required";
  if (bodyTemplate.length > 4096) return "Body template is too long";
  if (cooldownSecs.trim()) {
    const cooldown = Number(cooldownSecs);
    if (!Number.isInteger(cooldown) || cooldown < 0 || cooldown > 2_592_000) {
      return "Cooldown must be a whole number from 0 to 2592000 seconds";
    }
  }
  return null;
}

export function WebhookRuleManager({
  agents,
  deliveries,
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
  rowsTruncated = rules.length >= FLEET_DETAIL_LIMIT,
}: {
  agents: AgentView[];
  deliveries: WebhookRuleDeliveryRecord[];
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
  rowsTruncated?: boolean;
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
  const [enableRows, setEnableRows] = useState<WebhookRuleRecord[] | null>(
    null,
  );
  const [saveSnapshot, setSaveSnapshot] = useState<{
    request: WebhookRuleRequest;
    title: string;
  } | null>(null);
  const [savePending, setSavePending] = useState(false);
  const savePendingRef = useRef(false);
  const [name, setName] = useState("");
  const [enabled, setEnabled] = useState(false);
  const [expression, setExpression] = useState("");
  const [target, setTarget] = useState("");
  const [bodyTemplate, setBodyTemplate] = useState(
    DEFAULT_WEBHOOK_BODY_TEMPLATE,
  );
  const [signingSecret, setSigningSecret] = useState("");
  const [clearSigningSecret, setClearSigningSecret] = useState(false);
  const [cooldownSecs, setCooldownSecs] = useState("300");
  const [notes, setNotes] = useState("");
  const [eventKind, setEventKind] = useState("interval.30sec");
  const [eventId, setEventId] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");
  const [statusScope, setStatusScope] = useState<"queue" | "resource">(
    "resource",
  );
  const resourceFeedbackRef = useRef<HTMLDivElement | null>(null);
  const queueFeedbackRef = useRef<HTMLDivElement | null>(null);
  const [queueConfirmation, setQueueConfirmation] = useState<
    "dispatch" | "process" | null
  >(null);
  const [queueSnapshot, setQueueSnapshot] =
    useState<WebhookDeliveryQueueSnapshot | null>(null);
  const [queuePending, setQueuePending] = useState(false);
  const webhookWorkflowBusy = savePending || deletePending || queuePending;
  const hasEnabledRules = rules.some((rule) => rule.enabled);
  const hasFailedDeliveries = deliveries.some((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  );
  const hasQueuedDeliveries = deliveries.some(
    (delivery) => delivery.status === "queued",
  );

  const focusedEditorOpen = focusedEditorMode && editorOpen;
  const editingRule = editingId
    ? (rules.find((rule) => rule.id === editingId) ?? null)
    : null;
  const existingSecretConfigured = editingRule?.signing_secret_set ?? false;
  const webhookDraftValidation = webhookRuleDraftValidationMessage({
    bodyTemplate,
    cooldownSecs,
    eventKind,
    expression,
    name,
    target,
  });

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
        cell: (rule) => <small>{rule.target}</small>,
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
    setEditorTestPreview(null);
    setStatus(null);
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

  useEffect(() => {
    if (!status || statusTone === "info") return;
    const feedback =
      statusScope === "queue"
        ? queueFeedbackRef.current
        : resourceFeedbackRef.current;
    if (feedback) {
      scrollIntoViewWithMotion(feedback, { block: "nearest" });
    }
  }, [status, statusScope, statusTone]);

  function resetForm() {
    setEditingId(null);
    setEditorTestPreview(null);
    setName("");
    setEnabled(false);
    setExpression("");
    setTarget("");
    setBodyTemplate(DEFAULT_WEBHOOK_BODY_TEMPLATE);
    setSigningSecret("");
    setClearSigningSecret(false);
    setCooldownSecs("300");
    setNotes("");
    setStatus(null);
  }

  function setWebhookStatus(
    message: string,
    tone: ActionFeedbackTone,
    scope: "queue" | "resource" = "resource",
  ) {
    setStatus(message);
    setStatusTone(tone);
    setStatusScope(scope);
  }

  function enterWebhookWorkflow(
    surface: "editor" | "details" | "table" | "queue",
  ) {
    setSaveSnapshot(null);
    setEnableRows(null);
    setDeleteRows(null);
    setDeleteError(null);
    setQueueConfirmation(null);
    setQueueSnapshot(null);
    setStatus(null);
    if (surface !== "editor") {
      updateEditorOpen(false);
    }
    if (surface !== "details") {
      setDetailRuleId(null);
    }
  }

  function updateEditorOpen(open: boolean) {
    if (!open && savePending) return;
    if (!open) {
      setSaveSnapshot(null);
    }
    setEditorOpen(open);
    onEditorOpenChange?.(focusedEditorMode && open);
  }

  function createRule() {
    if (webhookWorkflowBusy) return;
    enterWebhookWorkflow("editor");
    resetForm();
    updateEditorOpen(true);
  }

  function editRule(rule: WebhookRuleRecord) {
    if (webhookWorkflowBusy) return;
    enterWebhookWorkflow("editor");
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
    setWebhookStatus(`editing ${rule.name}`, "info");
    updateEditorOpen(true);
  }

  function openRuleDetails(rule: WebhookRuleRecord) {
    if (webhookWorkflowBusy) return;
    enterWebhookWorkflow("details");
    setDetailRuleId(rule.id);
    setWebhookStatus(`viewing ${rule.name}`, "info");
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

  function beginSaveMutation() {
    if (savePendingRef.current) {
      return false;
    }
    savePendingRef.current = true;
    setSavePending(true);
    return true;
  }

  function finishSaveMutation() {
    savePendingRef.current = false;
    setSavePending(false);
  }

  function reviewSubmit() {
    if (webhookDraftValidation) {
      setWebhookStatus(webhookDraftValidation, "danger");
      return;
    }
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
      setWebhookStatus("Review webhook rule before saving", "warning");
      return;
    }
    if (!beginSaveMutation()) {
      return;
    }
    setWebhookStatus(
      editingId ? "updating webhook rule" : "creating webhook rule",
      "progress",
    );
    try {
      const rule = await onUpsert(snapshot.request);
      setEditingId(rule.id);
      setSigningSecret("");
      setClearSigningSecret(false);
      updateEditorOpen(true);
      setSaveSnapshot(null);
      setWebhookStatus(`saved ${rule.name}`, "success");
    } catch (error) {
      setWebhookStatus(
        error instanceof Error ? error.message : "webhook save failed",
        "danger",
      );
    } finally {
      finishSaveMutation();
    }
  }

  function requestDeleteRules(rows: WebhookRuleRecord[]) {
    if (webhookWorkflowBusy) return;
    enterWebhookWorkflow("table");
    setDeleteError(null);
    setDeleteRows(rows);
  }

  async function confirmDeleteRules() {
    const rows = deleteRows ?? [];
    if (rows.length === 0 || deletePending) return;
    setDeletePending(true);
    setDeleteError(null);
    setWebhookStatus("deleting webhook rules", "progress");
    try {
      for (const rule of rows) {
        await onDelete(rule.id, rule.name);
      }
      if (rows.some((rule) => rule.id === editingId)) {
        resetForm();
        updateEditorOpen(false);
      }
      if (rows.some((rule) => rule.id === detailRuleId)) {
        setDetailRuleId(null);
      }
      setDeleteRows(null);
      setWebhookStatus(
        `Deleted ${resourceCount(rows.length, "webhook rule")}`,
        "success",
      );
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "rule delete failed";
      setDeleteError(message);
      setWebhookStatus(message, "danger");
    } finally {
      setDeletePending(false);
    }
  }

  async function setRulesEnabled(
    rows: WebhookRuleRecord[],
    nextEnabled: boolean,
  ): Promise<boolean> {
    if (rows.length === 0) return false;
    if (!beginSaveMutation()) {
      return false;
    }
    setWebhookStatus(
      nextEnabled ? "enabling webhook rules" : "disabling webhook rules",
      "progress",
    );
    try {
      for (const rule of rows) {
        await onUpsert(requestFromRule(rule, { enabled: nextEnabled }));
      }
      setWebhookStatus(
        `${nextEnabled ? "Enabled" : "Disabled"} ${resourceCount(rows.length, "webhook rule")}`,
        "success",
      );
      return true;
    } catch (error) {
      setWebhookStatus(
        error instanceof Error ? error.message : "rule update failed",
        "danger",
      );
      return false;
    } finally {
      finishSaveMutation();
    }
  }

  function requestEnableRules(rows: WebhookRuleRecord[]) {
    if (webhookWorkflowBusy) return;
    const disabledRows = rows.filter((rule) => !rule.enabled);
    if (disabledRows.length === 0) return;
    enterWebhookWorkflow("table");
    setEnableRows(disabledRows);
  }

  async function confirmEnableRules() {
    const rows = enableRows ?? [];
    if (await setRulesEnabled(rows, true)) {
      setEnableRows(null);
    }
  }

  async function dryRun(rule?: WebhookRuleRecord) {
    if (webhookWorkflowBusy) {
      return;
    }
    if (!rule && webhookDraftValidation) {
      setWebhookStatus(webhookDraftValidation, "danger");
      return;
    }
    const shouldOpenDeliveries = Boolean(rule) || !focusedEditorOpen;
    enterWebhookWorkflow(rule ? "queue" : "editor");
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
    setWebhookStatus("rendering webhook dry run", "progress");
    setQueuePending(true);
    try {
      const preview = await onDryRun(request);
      if (!rule) {
        setEditorTestPreview(preview);
      }
      onPreviewDryRun(preview);
      onPreviewRows(preview.delivery ? [preview.delivery] : []);
      if (shouldOpenDeliveries) {
        onOpenDeliveries();
      }
      setWebhookStatus(
        `dry run matched ${preview.matched_vps.length}`,
        "success",
      );
    } catch (error) {
      setWebhookStatus(
        error instanceof Error ? error.message : "dry run failed",
        "danger",
      );
    } finally {
      setQueuePending(false);
    }
  }

  function clearWebhookQueueReview() {
    setQueueConfirmation(null);
    setQueueSnapshot(null);
  }

  function setWebhookEventKind(value: string) {
    setEventKind(value);
    setEditorTestPreview(null);
    clearWebhookQueueReview();
    setStatus(null);
  }

  function setWebhookEventId(value: string) {
    setEventId(value);
    setEditorTestPreview(null);
    clearWebhookQueueReview();
    setStatus(null);
  }

  async function dispatch(
    dryRunMode: boolean,
    openConfirmation = false,
    rule?: WebhookRuleRecord,
  ) {
    if (webhookWorkflowBusy) {
      return;
    }
    if (!rule && !hasEnabledRules) {
      setWebhookStatus(
        "Create and enable a webhook rule before matching test events",
        "warning",
        "queue",
      );
      return;
    }
    enterWebhookWorkflow("queue");
    setWebhookStatus(
      dryRunMode
        ? rule
          ? `matching webhook rule ${rule.name}`
          : "matching webhook rules"
        : rule
          ? `queueing webhook test for ${rule.name}`
          : "queueing webhooks",
      "progress",
      "queue",
    );
    setQueuePending(true);
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
        if (!openConfirmation) {
          onPreviewRows(rows);
          onOpenDeliveries();
        }
        if (openConfirmation) {
          const previewHash = reviewedDeliveryHash(rows, "Webhook dispatch");
          const frozenEventKind = eventKind.trim();
          const frozenEventId =
            eventId.trim() || reviewedWebhookDispatchEventId(rows);
          setQueueSnapshot({
            action: "dispatch",
            request: {
              rule_id: rule?.id ?? null,
              event_kind: frozenEventKind,
              event_id: frozenEventId,
              limit: 50,
              dry_run: false,
              confirmed: true,
              preview_hash: previewHash,
            },
            previewHash,
            reviewedRows: rows.length,
            eventLabel: `${rule ? `${rule.name} / ` : ""}${frozenEventKind || "event"}${frozenEventId ? ` / ${frozenEventId}` : ""}`,
            reviewedRules: reviewedWebhookRuleSummary(rows),
            reviewedTargets: reviewedWebhookTargetSummary(rows),
          });
          setQueueConfirmation("dispatch");
        }
      }
      setWebhookStatus(
        dryRunMode
          ? `Matched ${resourceCount(rows.length, "webhook delivery", "webhook deliveries")}`
          : `Queued ${resourceCount(rows.length, "webhook delivery", "webhook deliveries")}`,
        "success",
        "queue",
      );
    } catch (error) {
      setWebhookStatus(
        error instanceof Error ? error.message : "webhook dispatch failed",
        "danger",
        "queue",
      );
    } finally {
      setQueuePending(false);
    }
  }

  async function process(
    dryRunMode: boolean,
    openConfirmation = false,
    deliveryStatus: NonNullable<WebhookRuleProcessRequest["status"]> = "queued",
  ) {
    if (webhookWorkflowBusy) {
      return;
    }
    const isRetry = deliveryStatus === "failed";
    const hasProcessableDeliveries = isRetry
      ? hasFailedDeliveries
      : hasQueuedDeliveries;
    if (!hasProcessableDeliveries) {
      setWebhookStatus(
        isRetry
          ? "No failed event webhook deliveries are available"
          : "No queued event webhook deliveries are available",
        "info",
        "queue",
      );
      return;
    }
    enterWebhookWorkflow("queue");
    setWebhookStatus(
      dryRunMode
        ? `previewing ${isRetry ? "failed" : "queued"} webhook deliveries`
        : isRetry
          ? "retrying failed webhooks"
          : "delivering webhooks",
      "progress",
      "queue",
    );
    setQueuePending(true);
    try {
      const rows = await onProcess({
        limit: 50,
        status: deliveryStatus,
        dry_run: dryRunMode,
        confirmed: !dryRunMode,
      });
      if (dryRunMode) {
        if (!openConfirmation) {
          onPreviewRows(rows);
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
            reviewedRules: reviewedWebhookRuleSummary(rows),
            reviewedTargets: reviewedWebhookTargetSummary(rows),
          });
          setQueueConfirmation("process");
        }
      }
      setWebhookStatus(
        `${dryRunMode ? "Previewed" : isRetry ? "Retried" : "Processed"} ${resourceCount(rows.length, "webhook delivery", "webhook deliveries")}`,
        "success",
        "queue",
      );
    } catch (error) {
      setWebhookStatus(
        error instanceof Error ? error.message : "webhook processing failed",
        "danger",
        "queue",
      );
    } finally {
      setQueuePending(false);
    }
  }

  async function confirmQueueAction() {
    const snapshot = queueSnapshot;
    if (!snapshot || queuePending) {
      return;
    }
    setQueuePending(true);
    setWebhookStatus(
      snapshot.action === "dispatch"
        ? "queueing reviewed webhooks"
        : "delivering reviewed webhooks",
      "progress",
      "queue",
    );
    try {
      const rows =
        snapshot.action === "dispatch"
          ? await onDispatch(snapshot.request)
          : await onProcess(snapshot.request);
      setWebhookStatus(
        `${snapshot.action === "dispatch" ? "Queued" : "Processed"} ${resourceCount(rows.length, "webhook delivery", "webhook deliveries")}`,
        "success",
        "queue",
      );
      clearWebhookQueueReview();
    } catch (error) {
      setWebhookStatus(
        error instanceof Error ? error.message : "webhook queue action failed",
        "danger",
        "queue",
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
      disabled: (rows) => webhookWorkflowBusy || rows.length !== 1,
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
      disabled: (rows) => webhookWorkflowBusy || rows.length !== 1,
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
      disabled: (rows) => webhookWorkflowBusy || rows.length !== 1,
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
      disabled: (rows) =>
        webhookWorkflowBusy ||
        rows.filter((rule) => !rule.enabled).length === 0,
      icon: <Power size={14} />,
      onSelect: requestEnableRules,
    },
    {
      label: "Disable",
      description: (rows) =>
        `Disable ${rows.filter((rule) => rule.enabled).length} enabled selected webhook rule records.`,
      disabled: (rows) =>
        webhookWorkflowBusy || rows.filter((rule) => rule.enabled).length === 0,
      icon: <PowerOff size={14} />,
      onSelect: (rows) => {
        enterWebhookWorkflow("table");
        void setRulesEnabled(
          rows.filter((rule) => rule.enabled),
          false,
        );
      },
    },
    {
      label: "Review deletion",
      description: (rows) =>
        `Delete ${rows.length} selected webhook rule records. Retained delivery history is not removed.`,
      disabled: (rows) => webhookWorkflowBusy || rows.length === 0,
      icon: <Trash2 size={14} />,
      onSelect: requestDeleteRules,
      tone: "danger",
    },
  ];
  const webhookSaveReviewPrompt = (
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
          label: "Body template",
          value: saveSnapshot?.request.body_template || "Default message",
        },
        {
          label: "Cooldown",
          value: saveSnapshot
            ? `${saveSnapshot.request.cooldown_secs ?? 300} seconds`
            : "-",
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
        { label: "Notes", value: saveSnapshot?.request.notes || "None" },
      ]}
      onCancel={() => setSaveSnapshot(null)}
      onConfirm={() => void submit()}
      open={saveSnapshot !== null}
      pending={savePending}
      error={
        saveSnapshot && statusScope === "resource" && statusTone === "danger"
          ? status
          : null
      }
      title="Confirm webhook rule save"
    />
  );

  return (
    <div className="consoleCrudPanel">
      <div className="consoleResourceLayout fullWidth">
        {statusScope === "resource" && !editorOpen && !detailRuleId ? (
          <div ref={resourceFeedbackRef}>
            <ActionFeedback
              className="localActionFeedback fleetPolicyActionFeedback"
              message={status}
              tone={statusTone}
            />
          </div>
        ) : null}
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
          rowsTruncated={rowsTruncated}
          searchPlaceholder="Search webhook rules by name, expression, target, or notes"
          storageKey="vpsman.grid.fleet.webhookRules.v2"
          title="Webhook rules"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              data-tooltip-disabled-reason={
                webhookWorkflowBusy
                  ? "A webhook rule or delivery operation is already in progress"
                  : undefined
              }
              disabled={webhookWorkflowBusy}
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
                {statusScope === "resource" ? (
                  <div ref={resourceFeedbackRef}>
                    <ActionFeedback
                      className="localActionFeedback fleetPolicyActionFeedback webhookEditorActionFeedback"
                      message={status}
                      tone={statusTone}
                    />
                  </div>
                ) : null}
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
                {statusScope === "resource" ? (
                  <div ref={resourceFeedbackRef}>
                    <ActionFeedback
                      className="localActionFeedback fleetPolicyActionFeedback webhookEditorActionFeedback"
                      message={status}
                      tone={statusTone}
                    />
                  </div>
                ) : null}
                <button
                  className="secondaryAction"
                  data-tooltip-disabled-reason={
                    queuePending || savePending
                      ? "A webhook preview, delivery, or save is already in progress"
                      : (webhookDraftValidation ?? undefined)
                  }
                  disabled={
                    queuePending ||
                    savePending ||
                    webhookDraftValidation !== null
                  }
                  title={
                    webhookDraftValidation ??
                    "Render a dry run from the current rule draft"
                  }
                  type="button"
                  onClick={() => void dryRun()}
                >
                  {focusedEditorMode ? "Test" : "Review rule"}
                </button>
                <button
                  className="primaryAction"
                  data-tooltip-disabled-reason={
                    queuePending || savePending
                      ? "A webhook preview, delivery, or save is already in progress"
                      : (webhookDraftValidation ?? undefined)
                  }
                  disabled={
                    queuePending ||
                    savePending ||
                    webhookDraftValidation !== null
                  }
                  title={
                    webhookDraftValidation ??
                    "Review the complete webhook rule before saving"
                  }
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
                    data-tooltip-disabled-reason={
                      webhookWorkflowBusy
                        ? "A webhook rule or delivery operation is already in progress"
                        : undefined
                    }
                    disabled={webhookWorkflowBusy}
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
            onClose={() => updateEditorOpen(false)}
            reviewPrompt={webhookSaveReviewPrompt}
            title={editingId ? "Edit webhook rule" : "Create webhook rule"}
          >
            <form
              className="consoleFormGrid"
              onSubmit={(event) => event.preventDefault()}
            >
              <ConsoleField label="Rule name" className="fieldWide">
                <input
                  aria-label="Webhook rule name"
                  placeholder="Edge status webhook"
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
              <ConsoleField
                label="Cooldown seconds"
                hint="Minimum seconds between new automatic deliveries for this rule. Retries are controlled separately."
              >
                <input
                  aria-label="Webhook cooldown seconds"
                  max={2592000}
                  min={0}
                  type="number"
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
              <ConsoleField
                label="Target URL"
                labelTitle="HTTPS endpoint that receives webhook deliveries."
                className="fieldFull"
                hint="Delivery is sent by the vpsman server. Production targets require a public HTTPS URL; local HTTP requires explicit server development opt-in."
              >
                <input
                  aria-label="Webhook target"
                  placeholder="https://hooks.example.net/vpsman"
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
                  data-tooltip-disabled-reason={
                    clearSigningSecret
                      ? "Clear existing signing secret is selected"
                      : undefined
                  }
                  data-value-tooltip-skip="true"
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
              <ConsoleField
                label="Body template"
                labelTitle="Template used to render the webhook message field."
                className="fieldFull"
                hint="Renders the message field in the fixed webhook JSON envelope. The multiline block between standalone {# and #} markers contains non-rendering examples; copy one outside the block to use it. Available roots include vps, matched_vps, event, rule, alert, policy, policy_rule, and traffic."
              >
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
                <span>Use Test for the exact server-resolved VPS matches.</span>
              </ConsoleField>
            </form>
          </ConsoleDetailPanel>
        ) : null}
      </div>
      {!focusedEditorOpen ? (
        <div className="consoleOperationsBar webhookOperationsPanel">
          <span>
            <strong>
              {configurationQueue ? "Event webhook tests" : "Webhook queue"}
            </strong>
            <small>
              {!hasEnabledRules && !hasFailedDeliveries
                ? "Create and enable a rule before previewing or sending test events."
                : configurationQueue
                  ? "Preview and send reviewed test events, or retry failed event webhook deliveries."
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
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasEnabledRules
                        ? "Create and enable a webhook rule first"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasEnabledRules}
                  title={
                    hasEnabledRules
                      ? "Preview enabled rules matched by this test event"
                      : "Create and enable a webhook rule first"
                  }
                  type="button"
                  onClick={() => void dispatch(true)}
                >
                  Preview match
                </button>
                <button
                  className="primaryAction"
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasEnabledRules
                        ? "Create and enable a webhook rule first"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasEnabledRules}
                  title={
                    hasEnabledRules
                      ? "Review an event webhook test before queueing it"
                      : "Create and enable a webhook rule first"
                  }
                  type="button"
                  onClick={() => void dispatch(true, true)}
                >
                  Send test
                </button>
                <button
                  className="secondaryAction"
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasFailedDeliveries
                        ? "No failed event webhook deliveries are available"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasFailedDeliveries}
                  title={
                    hasFailedDeliveries
                      ? "Review failed event webhook deliveries before retrying"
                      : "No failed event webhook deliveries are available"
                  }
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
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasEnabledRules
                        ? "Create and enable a webhook rule first"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasEnabledRules}
                  title={
                    hasEnabledRules
                      ? "Preview enabled rules matched by this event"
                      : "Create and enable a webhook rule first"
                  }
                  type="button"
                  onClick={() => void dispatch(true)}
                >
                  Review matches
                </button>
                <button
                  className="secondaryAction"
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasEnabledRules
                        ? "Create and enable a webhook rule first"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasEnabledRules}
                  title={
                    hasEnabledRules
                      ? "Review matching rules before queueing deliveries"
                      : "Create and enable a webhook rule first"
                  }
                  type="button"
                  onClick={() => void dispatch(true, true)}
                >
                  Queue dispatch
                </button>
                <button
                  className="secondaryAction"
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasQueuedDeliveries
                        ? "No queued event webhook deliveries are available"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasQueuedDeliveries}
                  title={
                    hasQueuedDeliveries
                      ? "Preview queued event webhook deliveries"
                      : "No queued event webhook deliveries are available"
                  }
                  type="button"
                  onClick={() => void process(true)}
                >
                  Preview queued
                </button>
                <button
                  className="primaryAction"
                  data-tooltip-disabled-reason={
                    webhookWorkflowBusy
                      ? "A webhook rule or delivery operation is already in progress"
                      : !hasQueuedDeliveries
                        ? "No queued event webhook deliveries are available"
                        : undefined
                  }
                  disabled={webhookWorkflowBusy || !hasQueuedDeliveries}
                  title={
                    hasQueuedDeliveries
                      ? "Review queued event webhook deliveries before sending"
                      : "No queued event webhook deliveries are available"
                  }
                  type="button"
                  onClick={() => void process(true, true)}
                >
                  Deliver queued
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
            label: "Rules",
            value: queueSnapshot?.reviewedRules ?? "review required",
          },
          {
            label: "Targets",
            value: queueSnapshot?.reviewedTargets ?? "review required",
          },
          {
            label: "Review hash",
            value: queueSnapshot
              ? `${queueSnapshot.previewHash.slice(0, 12)}...`
              : "review required",
            title: queueSnapshot?.previewHash ?? "review required",
          },
        ]}
        onCancel={clearWebhookQueueReview}
        onConfirm={() => void confirmQueueAction()}
        open={queueConfirmation !== null && queueSnapshot !== null}
        pending={queuePending}
        error={
          queueConfirmation &&
          statusScope === "queue" &&
          statusTone === "danger"
            ? status
            : null
        }
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
      {statusScope === "queue" ? (
        <div ref={queueFeedbackRef}>
          <ActionFeedback
            className="localActionFeedback fleetPolicyActionFeedback"
            message={status}
            tone={statusTone}
          />
        </div>
      ) : null}
      <ConfirmationPrompt
        confirmLabel="Enable webhook rules"
        detail="Enabling starts matching future events and can send requests to the configured external targets. Disabling remains immediate."
        items={[
          {
            label: "Rules",
            value: selectedRecordSummary(
              enableRows,
              "rule",
              "rules",
              (row) => row.name,
              (row) => row.id,
            ),
          },
          {
            label: "Targets",
            value: enableRows?.map((row) => row.target).join(", ") ?? "-",
          },
          {
            label: "Cadence",
            value: enableRows?.map((row) => row.expression).join(", ") ?? "-",
          },
        ]}
        onCancel={() => setEnableRows(null)}
        onConfirm={() => void confirmEnableRules()}
        open={enableRows !== null}
        pending={savePending}
        error={
          enableRows && statusScope === "resource" && statusTone === "danger"
            ? status
            : null
        }
        title="Confirm webhook enable"
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
  const matchedNames = webhookMatchedVpsNames(preview.matched_vps);
  return (
    <div
      className="consoleInlineNotice"
      title="Server-resolved dry run; no webhook delivery is sent."
    >
      <strong>
        {preview.matched_vps.length}{" "}
        {preview.matched_vps.length === 1 ? "VPS" : "VPSs"} matched webhook dry
        run
      </strong>
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
  const matchedNames = webhookMatchedVpsNames(preview.matched_vps);
  const samplePayload = JSON.stringify(preview.payload_context, null, 2);
  return (
    <div
      className="webhookRuleSamplePreview"
      title="Server-rendered webhook preview; no delivery is sent."
    >
      <div className="consoleInlineNotice">
        <strong>
          {preview.matched_vps.length}{" "}
          {preview.matched_vps.length === 1 ? "VPS" : "VPSs"} matched
        </strong>
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
          <strong>
            {preview.delivery
              ? deliveryStatusLabel(preview.delivery.status)
              : "dry run only"}
          </strong>
        </div>
      </div>
      <pre>{samplePayload}</pre>
    </div>
  );
}

export function WebhookDeliveryHistoryGrid({
  deliveries,
  preview,
  rowsTruncated = !preview && deliveries.length >= FLEET_DETAIL_LIMIT,
}: {
  deliveries: WebhookRuleDeliveryRecord[];
  preview: boolean;
  rowsTruncated?: boolean;
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
        cell: (delivery) => <small>{delivery.target}</small>,
      },
      {
        id: "matched",
        header: "Matched VPS",
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
            <small title={webhookMatchedVpsNames(delivery.matched_vps)}>
              {webhookCompactMatchedVpsNames(delivery.matched_vps, 3)}
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
          <span className="monoValue">
            Matched VPS:{" "}
            {webhookMatchedVpsNames(delivery.matched_vps) || "none"}
          </span>
          {delivery.error && (
            <span className="deliveryErrorText">error: {delivery.error}</span>
          )}
        </div>
      )}
      rows={deliveries}
      rowsTruncated={rowsTruncated}
      searchPlaceholder="Search webhook deliveries"
      selectable={false}
      storageKey="vpsman.grid.fleet.webhookDeliveries.v2"
      title={preview ? "Webhook delivery preview" : "Webhook delivery history"}
    />
  );
}

function webhookMatchedVpsNames(agents: AgentView[]): string {
  return agents.map((agent) => formatVpsName(agent, "name")).join(", ");
}

function webhookCompactMatchedVpsNames(
  agents: AgentView[],
  limit: number,
): string {
  if (agents.length === 0) {
    return "none";
  }
  const visible = webhookMatchedVpsNames(agents.slice(0, limit));
  const remainder = Math.max(0, agents.length - limit);
  return remainder > 0 ? `${visible} · +${remainder} more` : visible;
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
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");

  function clearRotationReview() {
    setRotationPreview(null);
    setConfirmDelete(false);
    setRotationError(null);
  }

  function setRotationStatusMessage(message: string, tone: ActionFeedbackTone) {
    setStatus(message);
    setStatusTone(tone);
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
    setRotationStatusMessage(
      confirmed ? "deleting matched deliveries" : "previewing rotation",
      "progress",
    );
    try {
      const response = await onRotate({
        ...(confirmed
          ? { older_than: rotationPreview!.older_than }
          : { older_than_days: optionalInteger(rotationDays) }),
        status: rotationStatus,
        rule_id: rotationRuleId || null,
        confirmed,
        preview_hash: confirmed ? rotationPreview?.preview_hash : null,
      });
      setRotationPreview(confirmed ? null : response);
      setConfirmDelete(false);
      setRotationStatusMessage(
        `${response.matched_count} matched / ${response.deleted_count} deleted`,
        "success",
      );
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "rotation failed";
      setRotationError(message);
      setRotationStatusMessage(message, "danger");
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
              data-tooltip-disabled-reason={
                rotationPending
                  ? "A webhook delivery rotation review is already in progress"
                  : undefined
              }
              disabled={rotationPending}
              type="button"
              onClick={() => void rotate(false)}
            >
              Review rotation
            </button>
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason={
                rotationPending
                  ? "A webhook delivery rotation review is already in progress"
                  : !rotationPreview
                    ? "Review the rotation before reviewing cleanup"
                    : rotationPreview.matched_count === 0
                      ? "The reviewed rotation has no matching deliveries to clean up"
                      : undefined
              }
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
        <ActionFeedback
          className="localActionFeedback fleetPolicyActionFeedback"
          message={status}
          tone={statusTone}
        />
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
            title: rotationPreview?.preview_hash ?? "review required",
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
        EditorView.contentAttributes.of({
          "aria-label": "Webhook body template",
        }),
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

  return (
    <div
      className="webhookCodeMirror"
      ref={containerRef}
      title="Template used to render the webhook message field."
    />
  );
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
  return typeof value === "number" ? value.toFixed(2) : "Awaiting rollup";
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

function formatMemoryUsed(
  rollup: TelemetryRollupRecord | null | undefined,
  formatBytes: ByteCountFormatter,
) {
  if (!rollup || rollup.memory_total_bytes_max <= 0) {
    return "Awaiting rollup";
  }
  const percent = Math.round(rollup.memory_used_ratio_avg * 100);
  return `${percent}% (${formatBytes(rollup.memory_total_bytes_max)})`;
}

function formatDiskUsedCompact(
  rollup: TelemetryRollupRecord | null | undefined,
) {
  const ratio = diskUsedRatio(rollup);
  return ratio === null ? "None" : `${Math.round(ratio)}%`;
}

function formatDiskUsed(
  rollup: TelemetryRollupRecord | null | undefined,
  formatBytes: ByteCountFormatter,
) {
  if (!rollup || rollup.disk_total_bytes_max <= 0) {
    return "Awaiting rollup";
  }
  const percent = Math.round(rollup.disk_used_ratio_avg * 100);
  return `${percent}% (${formatBytes(rollup.disk_total_bytes_max)})`;
}

function formatRollupFreshness(
  rollup: TelemetryRollupRecord | null | undefined,
) {
  return rollup
    ? `Telemetry ${formatCompactTime(rollup.latest_observed_at)}`
    : "No telemetry";
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

function formatNetworkBytes(
  rollup: TelemetryRollupRecord | null | undefined,
  formatBytes: ByteCountFormatter,
) {
  if (
    !rollup ||
    (rollup.network_rx_bytes_max === 0 && rollup.network_tx_bytes_max === 0)
  ) {
    return "Awaiting counters";
  }
  return `RX ${formatBytes(rollup.network_rx_bytes_max)} / TX ${formatBytes(rollup.network_tx_bytes_max)}`;
}

function formatNetworkRateSummary(
  rates: TelemetryNetworkRateRecord[],
  selection: NetworkRateInterfaceResolution | undefined,
  formatByteRateFromBitsPerSecond: ByteRateFormatter,
) {
  if (selection && !selection.valid) {
    return "Live-rate interface rule unavailable";
  }
  if (rates.length === 0) {
    return "Awaiting selected rate";
  }
  const rx = rates.reduce((total, rate) => total + rate.rx_bps_avg, 0);
  const tx = rates.reduce((total, rate) => total + rate.tx_bps_avg, 0);
  return `RX ${formatByteRateFromBitsPerSecond(rx)} / TX ${formatByteRateFromBitsPerSecond(tx)}`;
}

function formatSignalTraffic(
  rates: TelemetryNetworkRateRecord[],
  selection: NetworkRateInterfaceResolution,
  formatByteRateFromBitsPerSecond: ByteRateFormatter,
) {
  if (!selection.valid) return "Live-rate rule unavailable";
  if (rates.length > 0) {
    const totalBps = rates.reduce(
      (total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg,
      0,
    );
    return formatByteRateFromBitsPerSecond(totalBps);
  }
  return "Awaiting selected rate";
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
  return rateSamples > 0 ? `${rateSamples} rate` : "Awaiting rollup";
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

function formatMetricValue(
  value: number | null | undefined,
  formatBytes: ByteCountFormatter,
): string {
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
  formatBytes: ByteCountFormatter,
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
  formatBytes: ByteCountFormatter,
): string {
  if (!traffic) {
    return "not configured";
  }
  const limitingQuota = trafficLimitingQuota(traffic);
  if (limitingQuota) {
    const percent =
      limitingQuota.percent >= 100
        ? limitingQuota.percent.toFixed(0)
        : limitingQuota.percent.toFixed(1);
    return `${formatBytes(limitingQuota.used)} / ${formatBytes(limitingQuota.quota)} · ${limitingQuota.direction} · ${percent}%`;
  }
  if (trafficQuotaState(traffic) === "unlimited") {
    const unlimited = trafficUnlimitedQuota(traffic);
    return unlimited
      ? `${formatBytes(unlimited.used)} / Unlimited · ${unlimited.direction}`
      : `${formatBytes(traffic.total_bytes)} / Unlimited`;
  }
  return traffic.state === "incomplete"
    ? "incomplete"
    : formatBytes(traffic.total_bytes);
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
  if (traffic.state === "stale") {
    return "stale";
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
  if (state === "warning" || state === "incomplete" || state === "stale") {
    return "warning";
  }
  if (state === "ok") {
    return "ok";
  }
  return "neutral";
}

function quotaSummary(
  traffic: TrafficAccountingRecord | null | undefined,
  formatBytes: ByteCountFormatter,
): string {
  if (!traffic) {
    return "not set";
  }
  const limitingQuota = trafficLimitingQuota(traffic);
  if (limitingQuota) {
    return `${limitingQuota.direction} ${formatBytes(limitingQuota.quota)}`;
  }
  if (trafficQuotaState(traffic) === "unlimited") {
    return "Unlimited";
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
  if (traffic?.reset_day === -1) {
    return "-";
  }
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
  counter_source?: { status?: string; error?: string | null };
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
  const live = selectedAgent
    ? agentDisplayState(selectedAgent).label === "Online"
    : false;
  const networkInterfaceSummary = snapshot
    ? `${snapshot.interfaces.length} interface${snapshot.interfaces.length === 1 ? "" : "s"}`
    : privilegeReady
      ? "No snapshot"
      : "Privilege locked";
  const networkInterfacesFeedbackMessage =
    error ??
    (pending
      ? "Refreshing host interfaces"
      : progress
        ? bulkOutcomeSummary(progress)
        : null);
  const observed =
    typeof snapshot?.observed_unix === "number"
      ? formatCompactTime(new Date(snapshot.observed_unix * 1000).toISOString())
      : null;
  return (
    <div className="timeline networkInterfacesPanel">
      <Network size={18} />
      <div>
        <strong>Host interfaces</strong>
        <span title={payloadHash ?? undefined}>
          {networkInterfaceSummary}
          {observed ? `; seen ${observed}` : ""}
          {payloadHash ? `; payload ${payloadHash.slice(0, 12)}` : ""}
        </span>
        <ActionFeedback
          className="localActionFeedback networkInterfacesActionFeedback"
          message={networkInterfacesFeedbackMessage}
          tone={error ? "danger" : "progress"}
        />
        <div className="interfaceActions">
          <button
            className="secondaryAction compactAction"
            data-tooltip-disabled-reason={
              pending
                ? "Host interfaces are already being refreshed"
                : !selectedAgent
                  ? "Select exactly one VPS before refreshing host interfaces"
                  : !privilegeReady
                    ? "Unlock privilege before refreshing host interfaces"
                    : !live
                      ? "The selected VPS must be live to refresh host interfaces"
                      : undefined
            }
            disabled={pending || !selectedAgent || !privilegeReady || !live}
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
              title={jobId}
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
  const formatBytes = useByteCountFormatter();
  const sources: Array<
    [string, { status?: string; error?: string | null } | null | undefined]
  > = [
    ["Interface metadata", snapshot.sysfs_source],
    ["Traffic counters", snapshot.counter_source],
    ["Interface addresses", snapshot.address_source],
  ];
  const sourceFailures = sources
    .filter(([, source]) => source?.status && source.status !== "ok")
    .map(
      ([label, source]) =>
        `${label}: ${source?.error?.trim() || source?.status || "unavailable"}`,
    );
  if (snapshot.interfaces.length === 0) {
    return (
      <>
        {sourceFailures.length > 0 && (
          <ActionFeedback
            className="localActionFeedback"
            message={sourceFailures.join("; ")}
            tone="danger"
          />
        )}
        <span>No interfaces returned</span>
      </>
    );
  }
  return (
    <>
      {sourceFailures.length > 0 && (
        <ActionFeedback
          className="localActionFeedback"
          message={sourceFailures.join("; ")}
          tone="danger"
        />
      )}
      <div className="networkInterfaceList">
        {snapshot.interfaces
          .slice()
          .sort((left, right) => left.name.localeCompare(right.name))
          .map((networkInterface) => (
            <div className="networkInterfaceRow" key={networkInterface.name}>
              <strong>{networkInterface.name}</strong>
              <span>{interfaceStateSummary(networkInterface)}</span>
              <span>{interfaceAddressSummary(networkInterface)}</span>
              <span>
                {interfaceTrafficSummary(networkInterface, formatBytes)}
              </span>
            </div>
          ))}
      </div>
    </>
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
  formatBytes: ByteCountFormatter,
) {
  const rx =
    typeof networkInterface.rx_bytes === "number"
      ? formatBytes(networkInterface.rx_bytes)
      : "unknown";
  const tx =
    typeof networkInterface.tx_bytes === "number"
      ? formatBytes(networkInterface.tx_bytes)
      : "unknown";
  return `RX ${rx} / TX ${tx}`;
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
  ratesTruncated,
  rollup,
}: {
  rates: TelemetryNetworkRateRecord[];
  ratesTruncated: boolean;
  rollup: TelemetryRollupRecord | null | undefined;
}) {
  const formatBytes = useByteCountFormatter();
  const formatByteRateFromBitsPerSecond = useByteRateFormatter();
  if (rates.length === 0) {
    return (
      <DetailLine
        icon={<Network size={18} />}
        label="Interfaces"
        value={
          ratesTruncated
            ? "Unknown in loaded network-rate page; more may exist"
            : rollup &&
                (rollup.network_rx_bytes_max > 0 ||
                  rollup.network_tx_bytes_max > 0)
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
                  detail={rateByteDetail(rate, formatBytes)}
                  main={`RX ${formatByteRateFromBitsPerSecond(rate.rx_bps_avg)} / TX ${formatByteRateFromBitsPerSecond(rate.tx_bps_avg)}`}
                />
                <TelemetryStack
                  detail={`updated ${formatCompactTime(rate.updated_at)}`}
                  main={`${rate.sample_count} sample${rate.sample_count === 1 ? "" : "s"} / ${formatDuration(rate.bucket_secs)}`}
                />
                <TelemetryStack
                  detail={`${formatDuration(rate.bucket_secs)} interval`}
                  main={rateDeltaDetail(rate, formatBytes)}
                />
              </div>
            ))}
        </div>
      </div>
    </div>
  );
}

function TunnelList({
  tunnels,
  tunnelsTruncated,
}: {
  tunnels: TelemetryTunnelRecord[];
  tunnelsTruncated: boolean;
}) {
  if (tunnels.length === 0) {
    return (
      <DetailLine
        icon={<Network size={18} />}
        label="Runtime tunnels"
        value={
          tunnelsTruncated
            ? "Unknown in loaded tunnel page; more may exist"
            : "No tunnel reports"
        }
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
                  detail={formatTunnelPlanDetail(tunnel)}
                  main={formatTunnelPlanMain(tunnel)}
                  tone={adapterTone(tunnel)}
                  title={[
                    tunnel.endpoint_side
                      ? `${tunnel.endpoint_side} endpoint`
                      : "endpoint",
                    tunnel.peer_client_id
                      ? `peer ${tunnel.peer_client_id}`
                      : "peer unavailable",
                    tunnel.plan_id
                      ? `plan ${tunnel.plan_id}`
                      : "plan unavailable",
                  ].join("; ")}
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
    tunnel.adapter_health?.configured === true &&
    tunnel.adapter_health.success === false
  ) {
    return "telemetryRowCritical";
  }
  if (tunnel.latency_status === "down" || tunnel.latency_status === "missed") {
    return "telemetryRowWarn";
  }
  return "";
}

function formatTunnelRuntime(tunnel: TelemetryTunnelRecord): string {
  const state = tunnel.operstate ?? "state unknown";
  const mtu = typeof tunnel.mtu === "number" ? `mtu ${tunnel.mtu}` : "mtu -";
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
    tunnel.latency_target ?? "target -",
    `checked ${checked}`,
    windows || "windows -",
    telemetryReasonLabel(tunnel.latency_reason),
  ]
    .filter(Boolean)
    .join("; ");
}

function formatTunnelPlanMain(tunnel: TelemetryTunnelRecord): string {
  return tunnel.plan_name ?? "Declared tunnel";
}

function formatTunnelPlanDetail(tunnel: TelemetryTunnelRecord): string {
  return [
    tunnel.endpoint_side ? `${tunnel.endpoint_side} endpoint` : "endpoint",
    tunnel.peer_client_id
      ? `peer ${tunnel.peer_client_id}`
      : "peer unavailable",
    tunnel.plan_id ? `plan ${shortId(tunnel.plan_id)}` : "plan unavailable",
  ].join("; ");
}

function latencyTone(
  status: string | null | undefined,
): "critical" | "neutral" | "ok" | "warn" {
  if (status === "down") {
    return "warn";
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

function adapterTone(
  tunnel: TelemetryTunnelRecord,
): "critical" | "neutral" | "ok" | "warn" {
  if (
    tunnel.adapter_health?.configured === true &&
    tunnel.adapter_health.success === false
  ) {
    return "critical";
  }
  if (
    tunnel.adapter_health?.success === true ||
    tunnel.ownership_mode === "external_observed"
  ) {
    return "ok";
  }
  return "neutral";
}

function formatTunnelPolicy(tunnel: TelemetryTunnelRecord) {
  const adapterHealth = formatAdapterHealth(tunnel);
  const traffic = formatTunnelTraffic(tunnel);
  const manager = runtimeManagerLabel(
    tunnel.plan_runtime_manager ?? tunnel.ownership_mode,
  );
  return `${manager}; ${mutationPolicyLabel(tunnel.mutation_policy)}${adapterHealth}${traffic}`;
}

function formatAdapterHealth(tunnel: TelemetryTunnelRecord) {
  const health = tunnel.adapter_health;
  if (!health?.configured) {
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

function rateByteDetail(
  rate: TelemetryNetworkRateRecord,
  formatBytes: ByteCountFormatter,
): string {
  return `avg bytes RX ${formatBytes(rate.rx_bytes_avg)} / TX ${formatBytes(rate.tx_bytes_avg)}`;
}

function rateDeltaDetail(
  rate: TelemetryNetworkRateRecord,
  formatBytes: ByteCountFormatter,
): string {
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
    return "Awaiting rollup";
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
