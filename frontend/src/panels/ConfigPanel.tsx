import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  FileSliders,
  Play,
  RefreshCw,
  ServerCog,
  Trash2,
  X,
} from "lucide-react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { scrollIntoViewWithMotion } from "../motion";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  waitForBulkJobSet,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { sha256Hex } from "../fileTransfer";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  selectorExpressionForClientIds,
  VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE,
  vpsRuleSearchUnavailable,
} from "../searchExpression";
import { useVpsRuleSearchContext } from "../vpsRuleSearchContext";
import {
  clampJobMaxTimeoutSecs,
  clampInteger,
  DEFAULT_MAX_JOB_TIMEOUT_SECS,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "./jobDispatchModel";
import { LocalTargetPreview } from "./TargetImpactPreview";
import { SingleVpsConfigWorkspace } from "./config/SingleVpsConfigWorkspace";
import type {
  AgentView,
  ApplyRuntimeConfigBulkOverrideRequest,
  ApplyRuntimeConfigBulkOverrideResponse,
  ApplyRuntimeConfigOverrideRequest,
  ApplyRuntimeConfigOverrideResponse,
  BulkResolveResponse,
  CreateJobRequest,
  CreateJobResponse,
  FleetAlertPolicyRecord,
  ConfigurationPresetRecord,
  ConfigurationSourceView,
  DeleteRuntimeConfigPatchGeneratorRequest,
  RuntimeConfigApplyStateRecord,
  RuntimeConfigDispatchRecord,
  RuntimeConfigPatchGeneratorRecord,
  RuntimeConfigPatchGeneratorRenderResponse,
  TrafficAccountingRecord,
  VpsRuleChangePreview,
  VpsRuleValueRecord,
  VpsRulesBulkUnsetRequest,
  VpsRulesBulkUpsertRequest,
  VpsRulesDryRunRequest,
  VpsRulesDryRunResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetStatusRequestItem,
  JsonValue,
  PrivilegeAssertion,
  PreviewRuntimeConfigBulkOverrideRequest,
  PreviewRuntimeConfigOverrideRequest,
  RuntimeConfigBulkOverridePreview,
  RuntimeConfigClientWorkspace,
  RuntimeConfigOverridePreview,
  UpsertRuntimeConfigPatchGeneratorRequest,
} from "../types";
import {
  dispatchFailureReason,
  formatTime,
  formatVpsName,
  runPanelAction,
  shortId,
  timestampMillis,
} from "../utils";
import {
  VPS_RULE_FIELD_DEFINITIONS,
  VPS_RULE_KEYS,
  normalizeVpsRuleValue,
  tryNormalizeVpsRuleValue,
  type VpsRuleFieldDefinition,
} from "../vpsRules";

const CONFIG_BULK_SELECTOR_STORAGE_KEY =
  "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY = "vpsman.config.single.clientId";
const CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY =
  "vpsman.config.vpsRules.selectorExpression";
const CONFIG_HELP = {
  incrementalPatch:
    "Advanced VPS override patches modify reviewed runtime keys only. Use -field.path or -[section.path] to delete saved override values; inherited and locked values remain server-owned.",
  patchGenerator:
    "Saved generators render incremental TOML from reviewed JSON variables before any VPS target is touched.",
  targetSelector:
    "Selector expressions freeze the exact VPS set for preview and review so later fleet changes cannot silently expand scope.",
  maxTimeout:
    "Per-target command timeout enforced by the control plane so slow agents cannot hold config work indefinitely.",
  vpsRules:
    "Per-VPS traffic and optional billing values feed card display, accounting, and alert policies; dry-run previews changed rows before write.",
  ruleSelector:
    "Fleet selector used for the dry-run and final reviewed VPS rule mutation.",
  ruleSetValues:
    "Key=value lines become typed VPS rule values after control-plane validation and dry-run diffing.",
  ruleUnsetValues:
    "Explicit rule keys removed from every matched VPS after dry-run review.",
  previewHash:
    "Server-issued hash of the dry-run diff that the apply request must echo to prevent stale writes.",
} as const;
const RUNTIME_CONFIG_QUEUED_STALE_MS = 60 * 60 * 1000;

type BulkConfigApplySnapshot = {
  jobId: string;
  selectorExpression: string;
  clientIds: string[];
  targets: AgentView[];
  toml: string;
  patchName: string;
  patchSections: string[];
  patchSource: "generator" | "temporary";
  maxTimeoutSecs: number;
  privilegeAssertion: PrivilegeAssertion;
  payloadHashHex: string;
  previewHash: string;
};

type PatchGeneratorEditorState = {
  mode: "new" | "edit";
  id: string | null;
  name: string;
  category: string;
  domain: string;
  description: string;
  fieldSchemaText: string;
  rawGeneratorBody: string;
  docsMetadataText: string;
};

type EvidenceState = "available" | "loading" | "unavailable";

export function ConfigPanel({
  activeSubpage,
  agents,
  trafficAccounting,
  vpsRuleValues,
  configurationPresets,
  configurationPresetsEvidenceState,
  configurationSources,
  configurationSourcesEvidenceState,
  fleetConfigEvidenceAvailable,
  inventoryEvidenceState,
  error,
  runtimeConfigApplyStates,
  runtimeConfigEvidenceState,
  runtimeConfigPatchGenerators,
  fleetAlertPolicies,
  jobs,
  loading,
  onApplyRuntimeConfigBulkOverride,
  onApplyRuntimeConfigOverride,
  onCreateJob,
  onLoadExactJobTargetStatuses,
  onLoadJobOutputs,
  onLoadJobTargets,
  onLoadConfigurationInventory,
  onLoadRuntimeConfigClientWorkspace,
  onDeleteRuntimeConfigPatchGenerator,
  onOpenJobDetails,
  onOpenJobHistory,
  onOpenPrivilegeUnlock,
  onOpenAlerts,
  onRefresh,
  onBulkUnsetVpsRules,
  onBulkUpsertVpsRules,
  onDryRunVpsRules,
  onLoadEffectiveVpsRules,
  onRenderRuntimeConfigPatchGenerator,
  onPreviewRuntimeConfigBulkOverride,
  onPreviewRuntimeConfigOverride,
  onSelectSubpage,
  onUpsertRuntimeConfigPatchGenerator,
  privilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  trafficAccounting: TrafficAccountingRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
  configurationPresets: ConfigurationPresetRecord[];
  configurationPresetsEvidenceState: EvidenceState;
  configurationSources: ConfigurationSourceView[];
  configurationSourcesEvidenceState: EvidenceState;
  fleetConfigEvidenceAvailable: boolean;
  inventoryEvidenceState: EvidenceState;
  error: string | null;
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  runtimeConfigEvidenceState: EvidenceState;
  runtimeConfigPatchGenerators: RuntimeConfigPatchGeneratorRecord[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  jobs: Array<{
    id: string;
    command_type: string;
    status: string;
    created_at: string;
  }>;
  loading: boolean;
  onApplyRuntimeConfigBulkOverride: (
    request: ApplyRuntimeConfigBulkOverrideRequest,
  ) => Promise<ApplyRuntimeConfigBulkOverrideResponse>;
  onApplyRuntimeConfigOverride: (
    clientId: string,
    request: ApplyRuntimeConfigOverrideRequest,
  ) => Promise<ApplyRuntimeConfigOverrideResponse>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadExactJobTargetStatuses: (
    items: JobTargetStatusRequestItem[],
  ) => Promise<JobTargetRecord[]>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onLoadConfigurationInventory: () => Promise<void>;
  onLoadRuntimeConfigClientWorkspace: (
    clientId: string,
  ) => Promise<RuntimeConfigClientWorkspace>;
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobHistory: () => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenAlerts: () => void;
  onRefresh: (() => Promise<void>) | null;
  onBulkUnsetVpsRules: (
    request: VpsRulesBulkUnsetRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsertVpsRules: (
    request: VpsRulesBulkUpsertRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onDryRunVpsRules: (
    request: VpsRulesDryRunRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onLoadEffectiveVpsRules: (clientId: string) => Promise<VpsRuleValueRecord[]>;
  onRenderRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: { values: JsonValue },
  ) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onPreviewRuntimeConfigBulkOverride: (
    request: PreviewRuntimeConfigBulkOverrideRequest,
  ) => Promise<RuntimeConfigBulkOverridePreview>;
  onPreviewRuntimeConfigOverride: (
    clientId: string,
    request: PreviewRuntimeConfigOverrideRequest,
  ) => Promise<RuntimeConfigOverridePreview>;
  onSelectSubpage: (subpage: string) => void;
  onUpsertRuntimeConfigPatchGenerator: (
    request: UpsertRuntimeConfigPatchGeneratorRequest,
  ) => Promise<RuntimeConfigPatchGeneratorRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const subpage = normalizeConfigSubpage(activeSubpage);
  const rulesSelectorPrefill = activeSubpage.startsWith("rules:id:")
    ? `id:${activeSubpage.slice("rules:id:".length)}`
    : null;
  const configPageFeedbackMessage =
    error ?? (loading ? "Refreshing runtime config state" : null);
  const configPageFeedbackTone = error ? "danger" : "progress";
  const configActionFeedbackMessage =
    subpage === "bulk" || subpage === "single" ? actionError : null;
  const configActionFeedbackRef = useRef<HTMLDivElement | null>(null);
  const previousConfigActionFeedbackRef = useRef<string | null>(null);

  useEffect(() => {
    setActionError(null);
  }, [subpage]);

  useEffect(() => {
    if (!configActionFeedbackMessage) {
      previousConfigActionFeedbackRef.current = null;
      return;
    }
    if (
      previousConfigActionFeedbackRef.current === configActionFeedbackMessage
    ) {
      return;
    }
    previousConfigActionFeedbackRef.current = configActionFeedbackMessage;
    const frame = window.requestAnimationFrame(() => {
      if (configActionFeedbackRef.current) {
        scrollIntoViewWithMotion(configActionFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [configActionFeedbackMessage]);

  useEffect(() => {
    if (subpage === "overview") {
      void runPanelAction(
        setPending,
        setActionError,
        onLoadConfigurationInventory,
      );
    }
  }, [onLoadConfigurationInventory, subpage]);

  return (
    <section className="workspace singleColumn configWorkspace">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>{configTitle(subpage)}</h2>
            <span>{configSubtitle(subpage)}</span>
          </div>
          <div className="headerActionStack">
            {onRefresh ? (
              <button
                className="secondaryAction"
                disabled={loading || pending}
                onClick={() =>
                  void runPanelAction(setPending, setActionError, onRefresh)
                }
                title={
                  loading || pending
                    ? "Wait for the current configuration request to finish"
                    : "Refresh the data displayed by this configuration page"
                }
                type="button"
              >
                <RefreshCw size={15} />
                <span>Refresh</span>
              </button>
            ) : null}
            <ActionFeedback
              message={configPageFeedbackMessage}
              tone={configPageFeedbackTone}
            />
          </div>
        </div>
        <ActionFeedback
          className="localActionFeedback configActionFeedback"
          message={configActionFeedbackMessage}
          ref={configActionFeedbackRef}
          tone="danger"
        />
        {subpage === "overview" && (
          <ConfigOverview
            agents={agents}
            configurationPresets={configurationPresets}
            configurationPresetsEvidenceState={
              configurationPresetsEvidenceState
            }
            configurationSources={configurationSources}
            configurationSourcesEvidenceState={
              configurationSourcesEvidenceState
            }
            fleetConfigEvidenceAvailable={fleetConfigEvidenceAvailable}
            inventoryEvidenceState={inventoryEvidenceState}
            runtimeConfigApplyStates={runtimeConfigApplyStates}
            runtimeConfigEvidenceState={runtimeConfigEvidenceState}
            runtimeConfigPatchGenerators={runtimeConfigPatchGenerators}
            vpsRuleValues={vpsRuleValues}
            jobs={jobs}
            onSelectSubpage={onSelectSubpage}
          />
        )}
        {subpage === "bulk" && (
          <BulkConfigApply
            actionError={actionError}
            agents={agents}
            runtimeConfigPatchGenerators={runtimeConfigPatchGenerators}
            onApplyRuntimeConfigBulkOverride={onApplyRuntimeConfigBulkOverride}
            onDeleteRuntimeConfigPatchGenerator={
              onDeleteRuntimeConfigPatchGenerator
            }
            onCreateJob={onCreateJob}
            onLoadExactJobTargetStatuses={onLoadExactJobTargetStatuses}
            onLoadJobTargets={onLoadJobTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenJobHistory={onOpenJobHistory}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onRenderRuntimeConfigPatchGenerator={
              onRenderRuntimeConfigPatchGenerator
            }
            onPreviewRuntimeConfigBulkOverride={
              onPreviewRuntimeConfigBulkOverride
            }
            onUpsertRuntimeConfigPatchGenerator={
              onUpsertRuntimeConfigPatchGenerator
            }
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) =>
              runPanelAction(setPending, setActionError, action)
            }
          />
        )}
        {subpage === "single" && (
          <SingleVpsConfigWorkspace
            actionError={actionError}
            agents={agents}
            onApplyOverride={onApplyRuntimeConfigOverride}
            onCreateJob={onCreateJob}
            onLoadJobOutputs={onLoadJobOutputs}
            onLoadJobTargets={onLoadJobTargets}
            onLoadWorkspace={onLoadRuntimeConfigClientWorkspace}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onPreviewOverride={onPreviewRuntimeConfigOverride}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) =>
              runPanelAction(setPending, setActionError, action)
            }
          />
        )}
        {subpage === "rules" && (
          <VpsRulesPanel
            agents={agents}
            initialSelectorExpression={rulesSelectorPrefill}
            fleetAlertPolicies={fleetAlertPolicies}
            onOpenAlerts={onOpenAlerts}
            onBulkUnset={onBulkUnsetVpsRules}
            onBulkUpsert={onBulkUpsertVpsRules}
            onDryRun={onDryRunVpsRules}
            onLoadEffectiveVpsRules={onLoadEffectiveVpsRules}
            trafficAccounting={trafficAccounting}
            vpsRuleValues={vpsRuleValues}
          />
        )}
      </div>
    </section>
  );
}

function ConfigOverview({
  agents,
  configurationPresets,
  configurationPresetsEvidenceState,
  configurationSources,
  configurationSourcesEvidenceState,
  fleetConfigEvidenceAvailable,
  inventoryEvidenceState,
  runtimeConfigApplyStates,
  runtimeConfigEvidenceState,
  runtimeConfigPatchGenerators,
  vpsRuleValues,
  jobs,
  onSelectSubpage,
}: {
  agents: AgentView[];
  configurationPresets: ConfigurationPresetRecord[];
  configurationPresetsEvidenceState: EvidenceState;
  configurationSources: ConfigurationSourceView[];
  configurationSourcesEvidenceState: EvidenceState;
  fleetConfigEvidenceAvailable: boolean;
  inventoryEvidenceState: EvidenceState;
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  runtimeConfigEvidenceState: EvidenceState;
  runtimeConfigPatchGenerators: RuntimeConfigPatchGeneratorRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
  jobs: Array<{
    id: string;
    command_type: string;
    status: string;
    created_at: string;
  }>;
  onSelectSubpage: (subpage: string) => void;
}) {
  const agentNameById = new Map(
    agents.map((agent) => [agent.id, agent.display_name]),
  );
  const configJobs = jobs
    .filter((job) =>
      ["config_read", "runtime_config_sync"].includes(job.command_type),
    )
    .slice(0, 5);
  const sourceRiskRows = configurationSources.filter(
    configurationSourceNeedsAttention,
  );
  const sourceReadyRows = configurationSources.filter(
    configurationSourceIsReady,
  );
  const sourceNeutralRows = Math.max(
    configurationSources.length -
      sourceRiskRows.length -
      sourceReadyRows.length,
    0,
  );
  const runtimeEvidenceAvailable = runtimeConfigEvidenceState === "available";
  const inventoryEvidenceAvailable = inventoryEvidenceState === "available";
  const configurationPresetsEvidenceAvailable =
    configurationPresetsEvidenceState === "available";
  const configurationSourcesEvidenceAvailable =
    configurationSourcesEvidenceState === "available";
  const currentStateEvidenceAvailable =
    runtimeEvidenceAvailable && fleetConfigEvidenceAvailable;
  const completeSummaryEvidence =
    currentStateEvidenceAvailable &&
    inventoryEvidenceAvailable &&
    configurationPresetsEvidenceAvailable &&
    configurationSourcesEvidenceAvailable;
  const evidenceLoading =
    runtimeConfigEvidenceState === "loading" ||
    inventoryEvidenceState === "loading" ||
    configurationPresetsEvidenceState === "loading" ||
    configurationSourcesEvidenceState === "loading";
  const trustedRuntimeConfigApplyStates = runtimeEvidenceAvailable
    ? runtimeConfigApplyStates
    : [];
  const allConfigStateRows = currentStateEvidenceAvailable
    ? buildConfigCurrentStateRows(agents, trustedRuntimeConfigApplyStates)
    : [];
  const currentStateRows = allConfigStateRows.filter(
    (row) => row.resourceAvailable,
  );
  const historicalStateRows = allConfigStateRows.filter(
    (row) => !row.resourceAvailable,
  );
  const pendingSyncs = currentStateRows.filter(
    (row) => row.statusKind === "queued",
  ).length;
  const staleApplyRows = currentStateRows.filter(
    (row) => row.statusKind === "stale",
  ).length;
  const failedSyncs = currentStateRows.filter(
    (row) => row.statusKind === "failed",
  ).length;
  const appliedClientIds = new Set(
    currentStateRows
      .filter((row) => row.resourceAvailable && row.statusKind === "current")
      .map((row) => row.clientId),
  );
  const sourceClientIds = new Set(
    configurationSources.map((source) => source.client_id),
  );
  const missingApplyStates = currentStateRows.filter(
    (row) => row.resourceAvailable && row.statusKind === "unknown",
  ).length;
  const missingSourceEvidence = Math.max(
    agents.length - sourceClientIds.size,
    0,
  );
  const customPresetCount = configurationPresets.filter(
    (preset) => preset.kind === "custom",
  ).length;
  const effectivePresetCount = new Set(
    configurationSources.map((source) => source.effective_preset_id),
  ).size;
  const explicitOverrideCount = configurationSources.filter(
    (source) => source.selection_origin === "explicit_override",
  ).length;
  const invalidRuleRows = vpsRuleValues.filter(
    (row) => row.state !== "ok",
  ).length;
  const validRuleRows = vpsRuleValues.length - invalidRuleRows;
  const applyAttentionCount = currentStateRows.filter((row) =>
    ["failed", "stale", "queued", "unknown"].includes(row.statusKind),
  ).length;
  const attentionStateRows = currentStateRows.filter((row) =>
    ["failed", "stale", "queued", "unknown"].includes(row.statusKind),
  );
  const retryableApplyRows = currentStateRows.filter(
    (row) => row.actionKind === "retry",
  );
  const configHealth = completeSummaryEvidence
    ? configHealthStatus({
        failedSyncs,
        invalidRuleRows,
        missingApplyStates,
        missingSourceEvidence,
        pendingSyncs,
        staleApplyRows,
        sourceNeutralCount: sourceNeutralRows,
        sourceRiskCount: sourceRiskRows.length,
        totalRuleRows: vpsRuleValues.length,
        validRuleRows,
      })
    : {
        detail: evidenceLoading
          ? "Required config evidence is still loading. Health, drift, and zero-value claims remain unknown until the refresh finishes."
          : "Required config evidence is incomplete. Health, drift, and zero-value claims remain unknown; cached rows are retained only as historical context.",
        label: evidenceLoading ? "Checking evidence" : "Evidence incomplete",
        tone: "warning" as const,
      };
  const latestApplyStates = trustedRuntimeConfigApplyStates
    .slice()
    .sort(
      (left, right) =>
        configApplyStateSortValue(right) - configApplyStateSortValue(left),
    )
    .slice(0, 4);
  const recentChanges = [
    ...latestApplyStates.map((state) => ({
      detail: runtimeConfigApplyStateSummary(state),
      id: `apply:${state.client_id}`,
      operation: "Apply state",
      status: runtimeConfigApplyStatusLabel(state),
      target: agentNameById.get(state.client_id) ?? state.client_id,
      time: configApplyStateTime(state) ?? "",
      title: runtimeConfigApplyStateSummary(state, false),
      tone: runtimeConfigApplyTone(state),
    })),
    ...configJobs.map((job) => ({
      detail: `Job ${shortId(job.id)} created ${formatTime(job.created_at)}`,
      id: `job:${job.id}`,
      operation: job.command_type,
      status: job.status,
      target: "runtime config",
      time: job.created_at,
      title: `Job ${job.id} created ${formatTime(job.created_at)}`,
      tone: configJobStatusTone(job.status),
    })),
  ]
    .sort((left, right) => right.time.localeCompare(left.time))
    .slice(0, 6);
  const workflowLinks = [
    {
      action: "Open Per-VPS",
      detail:
        "Edit one server-desired runtime hierarchy and compare live evidence.",
      subpage: "per_vps",
      title: "Per-VPS",
    },
    {
      action: "Open VPS override patch",
      detail:
        "Use the Advanced-only patch flow with aggregate and per-VPS review.",
      subpage: "bulk_patch",
      title: "VPS override patch",
    },
    {
      action: "Open Sources",
      detail:
        "Review effective presets, inherited defaults, overrides, sync, and readiness.",
      subpage: "sources",
      title: "Sources",
    },
    {
      action: "Open Rules",
      detail:
        "Dry-run traffic and accounting rule values before they affect policy context.",
      subpage: "rules",
      title: "Rules",
    },
  ];
  function openCurrentStateAction(row: ConfigCurrentStateRow) {
    if (row.actionKind === "retry" && row.resourceAvailable) {
      writeLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY, `id:${row.clientId}`);
      onSelectSubpage("bulk_patch");
      return;
    }
    if (row.actionKind === "inspect" && row.resourceAvailable) {
      writeLocalString(CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY, row.clientId);
      onSelectSubpage("per_vps");
    }
  }
  return (
    <div className="configOverviewStack">
      <section className="configHealthPanel" aria-label="Config health posture">
        <div className="configHealthHeader">
          <div>
            <h3>Config health</h3>
            <span>
              Runtime apply state, effective sources, readiness, and
              traffic/accounting rule risk.
            </span>
          </div>
          <ConsoleStatusBadge tone={configHealth.tone}>
            {configHealth.label}
          </ConsoleStatusBadge>
        </div>
        <div className="configHealthSummary">
          <span>
            <strong>
              {currentStateEvidenceAvailable
                ? `${appliedClientIds.size}/${currentStateRows.length}`
                : "Unknown"}
            </strong>
            <small>current resources</small>
          </span>
          <span>
            <strong>
              {currentStateEvidenceAvailable ? applyAttentionCount : "Unknown"}
            </strong>
            <small>need attention</small>
          </span>
          <span>
            <strong>
              {currentStateEvidenceAvailable
                ? historicalStateRows.length
                : "Unknown"}
            </strong>
            <small>historical records</small>
          </span>
          <span>
            <strong>
              {configurationSourcesEvidenceAvailable
                ? `${sourceReadyRows.length}/${configurationSources.length}`
                : "Unknown"}
            </strong>
            <small>verified source checks</small>
          </span>
          <span>
            <strong>
              {fleetConfigEvidenceAvailable
                ? ruleValidityLabel(validRuleRows, vpsRuleValues.length)
                : "Unknown"}
            </strong>
            <small>traffic/accounting rows</small>
          </span>
        </div>
        <p>{configHealth.detail}</p>
      </section>

      <section
        className="configOverviewBlock"
        aria-label="Current config state by VPS"
      >
        <div className="configOverviewBlockHeader">
          <h3>Affected VPS current state</h3>
          <ConsoleStatusBadge
            tone={
              !currentStateEvidenceAvailable
                ? "warning"
                : retryableApplyRows.length
                  ? "warning"
                  : "ok"
            }
          >
            {runtimeConfigEvidenceState === "loading"
              ? "Checking evidence"
              : !currentStateEvidenceAvailable
                ? "Evidence unavailable"
                : `${applyAttentionCount} need attention`}
          </ConsoleStatusBadge>
        </div>
        <ConfigCurrentStateRowsList
          onOpenAction={openCurrentStateAction}
          rows={attentionStateRows}
        />
        {currentStateEvidenceAvailable && attentionStateRows.length === 0 ? (
          <div className="emptyState compactEmptyState">
            <strong>All current VPS config states are healthy</strong>
            <span>
              No queued, stale, failed, or unknown apply states need action.
            </span>
          </div>
        ) : null}
      </section>

      {historicalStateRows.length > 0 && (
        <details
          className="configOverviewBlock configHistoryDisclosure"
          aria-label="Historical config apply state"
          open
        >
          <summary className="configOverviewBlockHeader">
            <h3>Historical apply-state records</h3>
            <ConsoleStatusBadge tone="neutral">
              {historicalStateRows.length} unavailable
            </ConsoleStatusBadge>
          </summary>
          <p>
            These apply-state records belong to VPS IDs not present in the
            current fleet response. They stay visible for audit context but do
            not affect current retry or health counts.
          </p>
          <ConfigCurrentStateRowsList
            onOpenAction={openCurrentStateAction}
            rows={historicalStateRows}
          />
        </details>
      )}

      <div className="configOverviewColumns">
        <section
          className="configOverviewBlock"
          aria-label="Config drift summary"
        >
          <div className="configOverviewBlockHeader">
            <h3>Drift summary</h3>
            <ConsoleStatusBadge
              tone={
                !completeSummaryEvidence || sourceRiskRows.length || failedSyncs
                  ? "warning"
                  : "ok"
              }
            >
              {completeSummaryEvidence
                ? `${applyAttentionCount + sourceRiskRows.length + invalidRuleRows} action items`
                : evidenceLoading
                  ? "Checking evidence"
                  : "Evidence incomplete"}
            </ConsoleStatusBadge>
          </div>
          <div className="configRiskList">
            <ConfigOverviewRiskRow
              detail={
                currentStateEvidenceAvailable
                  ? `${failedSyncs} failed, ${staleApplyRows} stale, ${pendingSyncs} queued, ${missingApplyStates} unknown; current fleet only, historical records separated`
                  : runtimeConfigEvidenceState === "loading"
                    ? "Runtime apply evidence is still loading."
                    : "Current runtime apply evidence is unavailable."
              }
              label="Runtime apply state"
              tone={
                !currentStateEvidenceAvailable
                  ? "warning"
                  : failedSyncs
                    ? "critical"
                    : staleApplyRows || pendingSyncs || missingApplyStates
                      ? "warning"
                      : "ok"
              }
              value={
                currentStateEvidenceAvailable
                  ? failedSyncs +
                    staleApplyRows +
                    pendingSyncs +
                    missingApplyStates
                  : "Unknown"
              }
            />
            <ConfigOverviewRiskRow
              detail={
                configurationSourcesEvidenceAvailable
                  ? (configurationSourceAttentionReason(sourceRiskRows[0]) ??
                    `${sourceReadyRows.length} verified ready; ${sourceNeutralRows} offline or not yet verified`)
                  : configurationSourcesEvidenceState === "loading"
                    ? "Source readiness evidence is still loading."
                    : "Source readiness evidence is unavailable."
              }
              label="Source readiness drift"
              tone={
                !configurationSourcesEvidenceAvailable || sourceRiskRows.length
                  ? "warning"
                  : sourceNeutralRows
                    ? "neutral"
                    : "ok"
              }
              value={
                configurationSourcesEvidenceAvailable
                  ? sourceRiskRows.length
                  : "Unknown"
              }
            />
            <ConfigOverviewRiskRow
              detail={
                fleetConfigEvidenceAvailable
                  ? `${ruleValidityLabel(validRuleRows, vpsRuleValues.length)}; invalid values stay in Rules details`
                  : "Traffic and accounting rule evidence is unavailable."
              }
              label="Rule validation"
              tone={
                !fleetConfigEvidenceAvailable || invalidRuleRows
                  ? "warning"
                  : "ok"
              }
              value={fleetConfigEvidenceAvailable ? invalidRuleRows : "Unknown"}
            />
          </div>
        </section>

        <section
          className="configOverviewBlock"
          aria-label="Configuration source summary"
        >
          <div className="configOverviewBlockHeader">
            <h3>Configuration sources</h3>
            <ConsoleStatusBadge
              tone={
                !configurationPresetsEvidenceAvailable ||
                !configurationSourcesEvidenceAvailable ||
                !fleetConfigEvidenceAvailable ||
                missingSourceEvidence
                  ? "warning"
                  : "ok"
              }
            >
              {configurationPresetsEvidenceAvailable &&
              configurationSourcesEvidenceAvailable &&
              fleetConfigEvidenceAvailable
                ? `${sourceClientIds.size}/${agents.length} VPSs`
                : evidenceLoading
                  ? "Checking evidence"
                  : "Evidence incomplete"}
            </ConsoleStatusBadge>
          </div>
          <div className="configCoverageGrid">
            <span>
              <strong>
                {configurationSourcesEvidenceAvailable
                  ? effectivePresetCount
                  : "Unknown"}
              </strong>
              <small>effective presets</small>
            </span>
            <span>
              <strong>
                {configurationPresetsEvidenceAvailable
                  ? customPresetCount
                  : "Unknown"}
              </strong>
              <small>custom presets</small>
            </span>
            <span>
              <strong>
                {configurationSourcesEvidenceAvailable
                  ? explicitOverrideCount
                  : "Unknown"}
              </strong>
              <small>explicit overrides</small>
            </span>
            <span>
              <strong>
                {configurationSourcesEvidenceAvailable &&
                fleetConfigEvidenceAvailable
                  ? missingSourceEvidence
                  : "Unknown"}
              </strong>
              <small>VPS without source evidence</small>
            </span>
          </div>
          <p>
            Open Sources to see the exact preset distribution across the fleet.
            No single VPS is treated as the fleet-wide desired source.
          </p>
        </section>
      </div>

      <section
        className="configWorkflowLinks"
        aria-label="Config overview workflow links"
      >
        {workflowLinks.map((link) => (
          <button
            className="configWorkflowLink"
            key={link.subpage}
            onClick={() => onSelectSubpage(link.subpage)}
            type="button"
          >
            <strong>{link.title}</strong>
            <small>{link.detail}</small>
            <span>{link.action}</span>
          </button>
        ))}
      </section>

      <details
        className="configOverviewBlock configHistoryDisclosure"
        aria-label="Recent config changes"
      >
        <summary className="configOverviewBlockHeader">
          <h3>Recent changes</h3>
          <span>{recentChanges.length} historical runtime config records</span>
        </summary>
        <div
          aria-label="Recent config change records"
          className="table hierarchyTable"
          role="table"
        >
          <div className="historyRow heading configRecentGrid" role="row">
            <span role="columnheader">Target</span>
            <span role="columnheader">Operation</span>
            <span role="columnheader">Status</span>
            <span role="columnheader">Detail</span>
            <span role="columnheader">Updated</span>
          </div>
          {recentChanges.map((change) => {
            const updated = change.time
              ? formatTime(change.time)
              : "No timestamp";
            return (
              <div
                className="historyRow configRecentGrid"
                key={change.id}
                role="row"
              >
                <span className="truncateValue" role="cell">
                  {change.target}
                </span>
                <span className="truncateValue" role="cell">
                  {change.operation}
                </span>
                <span className="truncateValue" role="cell">
                  <ConsoleStatusBadge tone={change.tone}>
                    {change.status}
                  </ConsoleStatusBadge>
                </span>
                <span
                  className="truncateValue"
                  role="cell"
                  title={
                    change.title !== change.detail ? change.title : undefined
                  }
                >
                  {change.detail}
                </span>
                <span className="truncateValue" role="cell">
                  {updated}
                </span>
              </div>
            );
          })}
        </div>
        {recentChanges.length === 0 && (
          <div className="emptyState compactEmpty">
            No recent config changes.
          </div>
        )}
      </details>
    </div>
  );
}

function ConfigOverviewRiskRow({
  detail,
  label,
  tone,
  value,
}: {
  detail: string;
  label: string;
  tone: "critical" | "warning" | "ok" | "info" | "neutral";
  value: ReactNode;
}) {
  return (
    <div className="configRiskRow">
      <span>
        <strong>{label}</strong>
        <small>{detail}</small>
      </span>
      <ConsoleStatusBadge tone={tone}>{value}</ConsoleStatusBadge>
    </div>
  );
}

function ConfigCurrentStateRowsList({
  onOpenAction,
  rows,
}: {
  onOpenAction: (row: ConfigCurrentStateRow) => void;
  rows: ConfigCurrentStateRow[];
}) {
  return (
    <div className="configCurrentStateList">
      {rows.map((row) => (
        <div className={`configCurrentStateRow ${row.statusKind}`} key={row.id}>
          <span className="configCurrentTarget">
            <strong title={row.targetTitle}>{row.targetLabel}</strong>
            <small>{row.targetDetail}</small>
          </span>
          <span>
            <ConsoleStatusBadge tone={row.tone}>
              {row.statusLabel}
            </ConsoleStatusBadge>
            <small title={row.statusTitle}>{row.statusDetail}</small>
          </span>
          <span>
            <strong>{row.ruleLabel}</strong>
            <small>{row.ruleDetail}</small>
          </span>
          <span>
            <strong>
              {row.updatedAt ? formatTime(row.updatedAt) : "No apply evidence"}
            </strong>
            <small>{row.updatedDetail}</small>
          </span>
          {row.actionKind === "retry" || row.actionKind === "inspect" ? (
            <button
              className="secondaryAction compactAction"
              onClick={() => onOpenAction(row)}
              type="button"
            >
              {row.actionLabel}
            </button>
          ) : (
            <small className="configCurrentStateAction">
              {row.actionLabel}
            </small>
          )}
        </div>
      ))}
    </div>
  );
}

type ConfigApplyStatusKind =
  | "current"
  | "failed"
  | "queued"
  | "stale"
  | "unknown";

type ConfigCurrentStateRow = {
  actionKind: "inspect" | "none" | "retry" | "unavailable";
  actionLabel: string;
  clientId: string;
  id: string;
  resourceAvailable: boolean;
  ruleDetail: string;
  ruleLabel: string;
  statusDetail: string;
  statusKind: ConfigApplyStatusKind;
  statusLabel: string;
  statusTitle?: string;
  targetDetail: string;
  targetLabel: string;
  targetTitle: string;
  tone: "critical" | "warning" | "ok" | "info" | "neutral";
  updatedAt: string | null;
  updatedDetail: string;
};

function buildConfigCurrentStateRows(
  agents: AgentView[],
  states: RuntimeConfigApplyStateRecord[],
): ConfigCurrentStateRow[] {
  const agentById = new Map(agents.map((agent) => [agent.id, agent]));
  const latestStateByClient = latestRuntimeConfigApplyStateByClient(states);
  const visibleRows = agents.map((agent) =>
    buildConfigCurrentStateRow({
      agent,
      clientId: agent.id,
      resourceAvailable: true,
      state: latestStateByClient.get(agent.id) ?? null,
    }),
  );
  const unavailableRows = Array.from(latestStateByClient.entries())
    .filter(([clientId]) => !agentById.has(clientId))
    .map(([clientId, state]) =>
      buildConfigCurrentStateRow({
        agent: null,
        clientId,
        resourceAvailable: false,
        state,
      }),
    );
  return [...visibleRows, ...unavailableRows].sort(
    (left, right) =>
      configCurrentStatePriority(left) - configCurrentStatePriority(right) ||
      left.targetLabel.localeCompare(right.targetLabel),
  );
}

function latestRuntimeConfigApplyStateByClient(
  states: RuntimeConfigApplyStateRecord[],
): Map<string, RuntimeConfigApplyStateRecord> {
  const latest = new Map<string, RuntimeConfigApplyStateRecord>();
  for (const state of states) {
    const current = latest.get(state.client_id);
    if (
      !current ||
      configApplyStateSortValue(state) > configApplyStateSortValue(current)
    ) {
      latest.set(state.client_id, state);
    }
  }
  return latest;
}

function buildConfigCurrentStateRow({
  agent,
  clientId,
  resourceAvailable,
  state,
}: {
  agent: AgentView | null;
  clientId: string;
  resourceAvailable: boolean;
  state: RuntimeConfigApplyStateRecord | null;
}): ConfigCurrentStateRow {
  const status = runtimeConfigApplyCurrentStatus(state);
  const targetLabel = resourceAvailable
    ? agent?.display_name || clientId
    : "Deleted or unavailable VPS";
  const targetDetail = resourceAvailable
    ? `${clientId} · ${agent?.status ?? "unknown"}`
    : clientId;
  const actionKind = configCurrentStateActionKind(
    status.kind,
    resourceAvailable,
  );
  return {
    actionKind,
    actionLabel: configCurrentStateActionLabel(actionKind),
    clientId,
    id: `${resourceAvailable ? "visible" : "unavailable"}:${clientId}`,
    resourceAvailable,
    ruleDetail: resourceAvailable
      ? "Open Rules for per-key validation detail"
      : "Rules hidden because the VPS is not in the visible fleet",
    ruleLabel: resourceAvailable ? "Rules visible" : "Rules unavailable",
    statusDetail: status.detail,
    statusKind: status.kind,
    statusLabel: status.label,
    statusTitle:
      state?.applied_content_hash ??
      state?.pending_job_id ??
      state?.applied_job_id ??
      undefined,
    targetDetail,
    targetLabel,
    targetTitle: resourceAvailable ? clientId : `Missing resource ${clientId}`,
    tone: status.tone,
    updatedAt: state ? configApplyStateTime(state) : null,
    updatedDetail: status.updatedDetail,
  };
}

function runtimeConfigApplyCurrentStatus(
  state: RuntimeConfigApplyStateRecord | null,
): {
  detail: string;
  kind: ConfigApplyStatusKind;
  label: string;
  tone: "critical" | "warning" | "ok" | "info" | "neutral";
  updatedDetail: string;
} {
  if (!state) {
    return {
      detail: "No server-applied runtime sync recorded",
      kind: "unknown",
      label: "Unknown",
      tone: "neutral",
      updatedDetail: "no apply-state evidence",
    };
  }
  if (state.pending_status === "failed") {
    const error = state.pending_error ? `: ${state.pending_error}` : "";
    return {
      detail: `Apply failed${error}`,
      kind: "failed",
      label: "Failed apply",
      tone: "critical",
      updatedDetail: "failed apply evidence",
    };
  }
  if (state.pending_status === "queued") {
    if (runtimeConfigQueuedStateIsStale(state)) {
      const queuedAt = configApplyStateTime(state);
      return {
        detail: queuedAt
          ? `Queued since ${formatTime(queuedAt)}; treat as stale before retry`
          : "Queued timestamp is missing; treat as stale before retry",
        kind: "stale",
        label: "Stale apply",
        tone: "warning",
        updatedDetail: "stale queued apply",
      };
    }
    return {
      detail: state.pending_reason ?? "Runtime apply is queued",
      kind: "queued",
      label: "Queued apply",
      tone: "info",
      updatedDetail: "queued apply",
    };
  }
  if (state.applied_content_hash) {
    return {
      detail: `Hash ${shortId(state.applied_content_hash)}`,
      kind: "current",
      label: "Current",
      tone: "ok",
      updatedDetail: "latest applied state",
    };
  }
  return {
    detail: "No server-applied runtime sync recorded",
    kind: "unknown",
    label: "Unknown",
    tone: "neutral",
    updatedDetail: "no apply-state evidence",
  };
}

function runtimeConfigQueuedStateIsStale(
  state: RuntimeConfigApplyStateRecord,
): boolean {
  const stateTime = configApplyStateTime(state);
  const updatedAt = stateTime ? timestampMillis(stateTime) : NaN;
  if (!Number.isFinite(updatedAt)) {
    return true;
  }
  return Date.now() - updatedAt > RUNTIME_CONFIG_QUEUED_STALE_MS;
}

function configCurrentStateActionKind(
  status: ConfigApplyStatusKind,
  resourceAvailable: boolean,
): ConfigCurrentStateRow["actionKind"] {
  if (!resourceAvailable) {
    return "unavailable";
  }
  if (status === "failed" || status === "stale") {
    return "retry";
  }
  if (status === "unknown" || status === "queued") {
    return "inspect";
  }
  return "none";
}

function configCurrentStateActionLabel(
  action: ConfigCurrentStateRow["actionKind"],
): string {
  switch (action) {
    case "retry":
      return "Retry";
    case "inspect":
      return "Inspect";
    case "unavailable":
      return "Unavailable";
    default:
      return "Current";
  }
}

function configCurrentStatePriority(row: ConfigCurrentStateRow): number {
  if (row.statusKind === "failed") {
    return row.resourceAvailable ? 0 : 3;
  }
  if (row.statusKind === "stale") {
    return 1;
  }
  if (row.statusKind === "queued") {
    return 2;
  }
  if (row.statusKind === "unknown") {
    return 4;
  }
  return 5;
}

function ruleValidityLabel(validRows: number, totalRows: number): string {
  return totalRows > 0
    ? `${validRows}/${totalRows} rules valid`
    : "No rule rows";
}

function configurationSourceNeedsAttention(
  source: ConfigurationSourceView,
): boolean {
  return (
    ["failed", "stale"].includes(source.runtime_sync.state) ||
    ["degraded", "failed", "invalid"].includes(source.readiness.state)
  );
}

function configurationSourceIsReady(source: ConfigurationSourceView): boolean {
  return (
    source.runtime_sync.state === "applied" &&
    source.readiness.state === "ready"
  );
}

function configurationSourceAttentionReason(
  source: ConfigurationSourceView | undefined,
): string | null {
  if (!source) return null;
  if (["failed", "stale"].includes(source.runtime_sync.state)) {
    return source.runtime_sync.reason;
  }
  return source.readiness.reason;
}

function configHealthStatus({
  failedSyncs,
  invalidRuleRows,
  missingApplyStates,
  missingSourceEvidence,
  pendingSyncs,
  staleApplyRows,
  sourceNeutralCount,
  sourceRiskCount,
  totalRuleRows,
  validRuleRows,
}: {
  failedSyncs: number;
  invalidRuleRows: number;
  missingApplyStates: number;
  missingSourceEvidence: number;
  pendingSyncs: number;
  staleApplyRows: number;
  sourceNeutralCount: number;
  sourceRiskCount: number;
  totalRuleRows: number;
  validRuleRows: number;
}): { detail: string; label: string; tone: "critical" | "warning" | "ok" } {
  const failedOrStaleApplies = failedSyncs + staleApplyRows;
  if (failedOrStaleApplies > 0) {
    return {
      detail: `${failedOrStaleApplies} latest runtime applies failed or went stale. Retry affected VPSs before relying on generated config or traffic policy state.`,
      label: "Action required",
      tone: "critical",
    };
  }
  if (
    pendingSyncs > 0 ||
    sourceRiskCount > 0 ||
    invalidRuleRows > 0 ||
    missingApplyStates > 0 ||
    missingSourceEvidence > 0
  ) {
    return {
      detail: `${pendingSyncs} applies are queued, ${sourceRiskCount} source checks need review, ${missingApplyStates} VPSs lack apply-state evidence, and ${ruleValidityLabel(validRuleRows, totalRuleRows)}.`,
      label: "Needs review",
      tone: "warning",
    };
  }
  if (sourceNeutralCount > 0) {
    return {
      detail: `No actionable configuration blockers were found. ${sourceNeutralCount} source checks remain neutral because the VPS is offline or readiness has not yet been verified.`,
      label: "No blockers",
      tone: "ok",
    };
  }
  return {
    detail:
      "All loaded VPSs have applied runtime state, verified-ready configuration sources, and valid rule rows.",
    label: "Healthy",
    tone: "ok",
  };
}

function configApplyStateTime(
  state: RuntimeConfigApplyStateRecord,
): string | null {
  return (
    normalizeConfigApplyTimestamp(state.pending_updated_at) ??
    normalizeConfigApplyTimestamp(state.applied_at) ??
    normalizeConfigApplyTimestamp(state.updated_at)
  );
}

function configApplyStateSortValue(
  state: RuntimeConfigApplyStateRecord,
): number {
  const time = configApplyStateTime(state);
  if (!time) {
    return 0;
  }
  const value = Date.parse(time);
  return Number.isFinite(value) ? value : 0;
}

function normalizeConfigApplyTimestamp(
  value: string | null | undefined,
): string | null {
  const trimmed = value?.trim();
  if (!trimmed) {
    return null;
  }
  const parsed = Date.parse(trimmed);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return null;
  }
  return trimmed;
}

function runtimeConfigApplyStatusLabel(
  state: RuntimeConfigApplyStateRecord,
): string {
  return runtimeConfigApplyCurrentStatus(state).label;
}

function runtimeConfigApplyTone(
  state: RuntimeConfigApplyStateRecord,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  return runtimeConfigApplyCurrentStatus(state).tone;
}

function configJobStatusTone(
  status: string,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  if (status === "failed") {
    return "critical";
  }
  if (status === "queued") {
    return "warning";
  }
  if (status === "succeeded" || status === "completed") {
    return "ok";
  }
  return "info";
}

function BulkConfigApply({
  actionError,
  agents,
  runtimeConfigPatchGenerators,
  onApplyRuntimeConfigBulkOverride,
  onDeleteRuntimeConfigPatchGenerator,
  onCreateJob,
  onLoadExactJobTargetStatuses,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenJobHistory,
  onOpenPrivilegeUnlock,
  onRenderRuntimeConfigPatchGenerator,
  onPreviewRuntimeConfigBulkOverride,
  onUpsertRuntimeConfigPatchGenerator,
  pending,
  privilegeMaterial,
  runAction,
}: {
  actionError: string | null;
  agents: AgentView[];
  runtimeConfigPatchGenerators: RuntimeConfigPatchGeneratorRecord[];
  onApplyRuntimeConfigBulkOverride: (
    request: ApplyRuntimeConfigBulkOverrideRequest,
  ) => Promise<ApplyRuntimeConfigBulkOverrideResponse>;
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadExactJobTargetStatuses: (
    items: JobTargetStatusRequestItem[],
  ) => Promise<JobTargetRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobHistory: () => void;
  onOpenPrivilegeUnlock: () => void;
  onRenderRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: { values: JsonValue },
  ) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onPreviewRuntimeConfigBulkOverride: (
    request: PreviewRuntimeConfigBulkOverrideRequest,
  ) => Promise<RuntimeConfigBulkOverridePreview>;
  onUpsertRuntimeConfigPatchGenerator: (
    request: UpsertRuntimeConfigPatchGeneratorRequest,
  ) => Promise<RuntimeConfigPatchGeneratorRecord>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
}) {
  const vpsRuleSearch = useVpsRuleSearchContext();
  const [selectorExpression, setSelectorExpression] = useState(() =>
    readLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY),
  );
  const [patchMode, setPatchMode] = useState<"generator" | "temporary">(
    "generator",
  );
  const [generatorId, setGeneratorId] = useState("");
  const [valuesText, setValuesText] = useState("");
  const [temporaryToml, setTemporaryToml] = useState("");
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [changePreview, setChangePreview] =
    useState<RuntimeConfigBulkOverridePreview | null>(null);
  const [rendered, setRendered] =
    useState<RuntimeConfigPatchGeneratorRenderResponse | null>(null);
  const [applySnapshot, setApplySnapshot] =
    useState<BulkConfigApplySnapshot | null>(null);
  const [deleteGenerator, setDeleteGenerator] =
    useState<RuntimeConfigPatchGeneratorRecord | null>(null);
  const [manageGeneratorsOpen, setManageGeneratorsOpen] = useState(false);
  const [patchGeneratorEditor, setPatchGeneratorEditor] =
    useState<PatchGeneratorEditorState | null>(null);
  const [patchGeneratorStatus, setPatchGeneratorStatus] = useState<
    string | null
  >(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const generatorManagementRef = useRef<HTMLElement | null>(null);
  const patchGeneratorEditorRef = useRef<HTMLElement | null>(null);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(
    DEFAULT_MAX_JOB_TIMEOUT_SECS,
  );
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const reviewFeedbackTone: ActionFeedbackTone = reviewStatus?.includes(
    "Review runtime apply state",
  )
    ? "warning"
    : reviewStatus?.startsWith("VPS override patch saved")
      ? "success"
      : reviewStatus?.startsWith("Desired")
        ? "warning"
        : "progress";
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectedGenerator = runtimeConfigPatchGenerators.find(
    (generator) =>
      generator.id === (generatorId || runtimeConfigPatchGenerators[0]?.id),
  );
  const reviewedTargets = useCallback(
    (targetClientIds: string[]): BulkResolveResponse => {
      const agentsById = new Map(agents.map((agent) => [agent.id, agent]));
      const targets = targetClientIds
        .map((targetClientId) => agentsById.get(targetClientId))
        .filter((agent): agent is AgentView => Boolean(agent));
      if (targets.length !== targetClientIds.length) {
        throw new Error(
          "Reviewed VPS targets changed outside the loaded fleet inventory; refresh and preview again",
        );
      }
      return { target_count: targetClientIds.length, targets };
    },
    [agents],
  );
  const patchGeneratorDraftError = patchGeneratorEditor
    ? validatePatchGeneratorEditor(patchGeneratorEditor)
    : null;
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const selectorEvidenceUnavailable = vpsRuleSearchUnavailable(
    selectorExpression,
    vpsRuleSearch,
  );
  const localSelectorTargets = useMemo(
    () =>
      selectorExpression.trim() &&
      !selectorParse.error &&
      !selectorEvidenceUnavailable
        ? agentsMatchingExpression(agents, selectorExpression, vpsRuleSearch)
        : [],
    [
      agents,
      selectorEvidenceUnavailable,
      selectorExpression,
      selectorParse.error,
      vpsRuleSearch,
    ],
  );
  const previewToml =
    patchMode === "temporary" ? temporaryToml.trim() : rendered?.toml.trim();
  const previewPatchSections =
    patchMode === "temporary"
      ? inferTomlSections(temporaryToml)
      : (rendered?.affected_sections ?? []);
  const canPreviewChanges = Boolean(
    selectorExpression.trim() &&
    !selectorParse.error &&
    (patchMode === "temporary" ? temporaryToml.trim() : selectedGenerator),
  );
  const ready = Boolean(
    preview &&
    changePreview &&
    preview.target_count > 0 &&
    previewToml &&
    selectorExpression.trim() &&
    !selectorParse.error &&
    (patchMode === "temporary" || rendered),
  );
  const patchGeneratorColumns = useMemo<
    ConsoleDataGridColumn<RuntimeConfigPatchGeneratorRecord>[]
  >(
    () => [
      {
        cell: (generator) => (
          <span className="historyPrimary">
            <strong>{generator.name}</strong>
            <small>{generator.description}</small>
          </span>
        ),
        header: "Generator",
        id: "name",
        searchValue: (generator) =>
          `${generator.name} ${generator.description}`,
        sortValue: (generator) => generator.name,
      },
      {
        cell: (generator) => generator.category,
        header: "Category",
        id: "category",
        searchValue: (generator) => generator.category,
        sortValue: (generator) => generator.category,
      },
      {
        cell: (generator) => generator.domain,
        header: "Domain",
        id: "domain",
        searchValue: (generator) => generator.domain,
        sortValue: (generator) => generator.domain,
      },
      {
        cell: (generator) => (
          <span className={`status ${generator.built_in ? "neutral" : "ok"}`}>
            {generator.built_in ? "built-in" : "custom"}
          </span>
        ),
        header: "Scope",
        id: "scope",
        searchValue: (generator) =>
          generator.built_in ? "built-in" : "custom",
        sortValue: (generator) => (generator.built_in ? "0" : "1"),
      },
      {
        cell: (generator) => formatTime(generator.updated_at),
        header: "Updated",
        id: "updated",
        searchValue: (generator) => formatTime(generator.updated_at),
        sortValue: (generator) => generator.updated_at,
      },
    ],
    [],
  );
  const patchGeneratorActions = useMemo<
    ConsoleDataGridAction<RuntimeConfigPatchGeneratorRecord>[]
  >(
    () => [
      {
        icon: <Play size={14} />,
        label: "Load",
        onSelect: (rows) => loadPatchGeneratorForApply(rows[0]),
        disabled: (rows) => rows.length !== 1,
        description: (rows) =>
          `Load ${rows[0]?.name ?? "one patch generator"} into the apply form.`,
      },
      {
        icon: <FileSliders size={14} />,
        label: "Edit",
        onSelect: (rows) => openPatchGeneratorEditor(rows[0]),
        disabled: (rows) => rows.length !== 1 || rows[0].built_in,
        description: (rows) =>
          rows[0]?.built_in
            ? "Built-in patch generators are read-only; clone before editing."
            : `Edit ${rows[0]?.name ?? "one custom patch generator"}.`,
      },
      {
        label: "Clone",
        onSelect: (rows) => void clonePatchGenerator(rows[0]),
        disabled: (rows) => rows.length !== 1,
        description: (rows) =>
          `Clone ${rows[0]?.name ?? "one patch generator"} for editing outside built-ins.`,
      },
      {
        icon: <Trash2 size={14} />,
        label: "Delete",
        tone: "danger",
        separatorBefore: true,
        onSelect: (rows) => setDeleteGenerator(rows[0]),
        disabled: (rows) => rows.length !== 1 || rows[0].built_in,
        description: (rows) =>
          rows[0]?.built_in
            ? "Built-in patch generators cannot be deleted."
            : `Review deletion for ${rows[0]?.name ?? "one patch generator"}.`,
      },
    ],
    [generatorId],
  );

  useEffect(
    () =>
      writeLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY, selectorExpression),
    [selectorExpression],
  );

  useLayoutEffect(() => {
    if (selectedGenerator) {
      setValuesText(
        formatJsonObject(exampleValuesForGenerator(selectedGenerator)),
      );
      setRendered(null);
      clearBulkConfigReview();
    }
  }, [selectedGenerator?.id]);

  useLayoutEffect(() => {
    if (!patchGeneratorEditor) {
      return;
    }
    window.requestAnimationFrame(() => {
      const editor = patchGeneratorEditorRef.current;
      if (!editor) {
        return;
      }
      scrollIntoViewWithMotion(editor, { block: "start" });
      editor.querySelector<HTMLInputElement>("input")?.focus({
        preventScroll: true,
      });
    });
  }, [patchGeneratorEditor?.id, patchGeneratorEditor?.mode]);

  function clearBulkConfigReview() {
    invalidateReviewGeneration();
    setApplySnapshot(null);
    setChangePreview(null);
    setConfirmOpen(false);
    setReviewStatus(null);
  }

  function scrollGeneratorManagementIntoView() {
    window.requestAnimationFrame(() => {
      const element = generatorManagementRef.current;
      if (!element) {
        return;
      }
      scrollIntoViewWithMotion(element, { block: "start" });
      element.focus({ preventScroll: true });
    });
  }

  function openGeneratorManagement() {
    setManageGeneratorsOpen(true);
    scrollGeneratorManagementIntoView();
  }

  function loadPatchGeneratorForApply(
    generator: RuntimeConfigPatchGeneratorRecord,
  ) {
    setPatchMode("generator");
    setGeneratorId(generator.id);
    setValuesText(formatJsonObject(exampleValuesForGenerator(generator)));
    setRendered(null);
    clearBulkConfigReview();
  }

  async function clonePatchGenerator(
    generator: RuntimeConfigPatchGeneratorRecord,
  ) {
    await runAction(async () => {
      const cloned = await onUpsertRuntimeConfigPatchGenerator({
        category: generator.category,
        description: generator.description,
        docs_metadata: generator.docs_metadata,
        domain: generator.domain,
        field_schema: generator.field_schema,
        name: `${generator.name} (cloned)`,
        raw_generator_body: generator.raw_generator_body,
        confirmed: true,
      });
      setPatchGeneratorStatus(`cloned ${cloned.name}`);
      setGeneratorId(cloned.id);
      openGeneratorManagement();
    });
  }

  function openPatchGeneratorEditor(
    generator?: RuntimeConfigPatchGeneratorRecord,
  ) {
    setPatchGeneratorStatus(null);
    setPatchGeneratorEditor(
      generator
        ? {
            mode: "edit",
            id: generator.id,
            name: generator.name,
            category: generator.category,
            domain: generator.domain,
            description: generator.description,
            fieldSchemaText: formatJsonObject(generator.field_schema),
            rawGeneratorBody: generator.raw_generator_body,
            docsMetadataText: formatJsonObject(generator.docs_metadata),
          }
        : {
            mode: "new",
            id: null,
            name: "",
            category: "",
            domain: "",
            description: "",
            fieldSchemaText: '{\n  "fields": {}\n}',
            rawGeneratorBody: "",
            docsMetadataText:
              '{\n  "expandable": true,\n  "affected_sections": [],\n  "patch_only": true\n}',
          },
    );
    openGeneratorManagement();
  }

  function updatePatchGeneratorEditor(
    patch: Partial<PatchGeneratorEditorState>,
  ) {
    setPatchGeneratorEditor((current) =>
      current ? { ...current, ...patch } : current,
    );
  }

  async function savePatchGeneratorEditor() {
    const editor = patchGeneratorEditor;
    if (!editor || validatePatchGeneratorEditor(editor)) {
      return;
    }
    await runAction(async () => {
      const saved = await onUpsertRuntimeConfigPatchGenerator({
        id: editor.mode === "edit" ? editor.id : null,
        name: editor.name.trim(),
        category: editor.category.trim(),
        domain: editor.domain.trim(),
        description: editor.description.trim(),
        field_schema: parseJsonObject(editor.fieldSchemaText),
        raw_generator_body: editor.rawGeneratorBody,
        docs_metadata: parseJsonObject(editor.docsMetadataText),
        confirmed: true,
      });
      setGeneratorId(saved.id);
      setPatchGeneratorEditor(null);
      setPatchGeneratorStatus(`saved ${saved.name}`);
      clearBulkConfigReview();
      openGeneratorManagement();
    });
  }

  async function deleteSelectedPatchGenerator() {
    const generator = deleteGenerator;
    if (!generator) {
      return;
    }
    await runAction(async () => {
      await onDeleteRuntimeConfigPatchGenerator(generator.id, {
        confirmed: true,
        reviewed_name: generator.name,
      });
      if (generatorId === generator.id) {
        setGeneratorId("");
        setRendered(null);
      }
      setDeleteGenerator(null);
      clearBulkConfigReview();
    });
  }

  async function previewChanges() {
    clearBulkConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenSelector = selectorExpression.trim();
    const frozenPatchMode = patchMode;
    const frozenGenerator = selectedGenerator;
    const frozenValuesText = valuesText;
    const frozenTemporaryToml = temporaryToml;
    setReviewStatus("Previewing bulk patch changes");
    try {
      await runAction(async () => {
        await waitForReviewRender();
        if (selectorParse.error) {
          throw new Error(selectorParse.error);
        }
        if (!frozenSelector) {
          throw new Error("Add at least one target selector");
        }
        if (frozenPatchMode === "generator" && !frozenGenerator) {
          throw new Error("Select a patch generator");
        }
        if (frozenPatchMode === "temporary" && !frozenTemporaryToml.trim()) {
          throw new Error("Paste a temporary TOML patch");
        }
        let toml = frozenTemporaryToml.trim();
        let patchName = "Temporary patch";
        if (frozenPatchMode === "generator") {
          const frozenValues = parseJsonObject(frozenValuesText);
          const nextRendered = await onRenderRuntimeConfigPatchGenerator(
            frozenGenerator!.id,
            { values: frozenValues },
          );
          if (!isReviewGenerationCurrent(reviewGeneration)) {
            return;
          }
          setRendered(nextRendered);
          toml = nextRendered.toml;
          patchName = frozenGenerator!.name;
        }
        const nextChangePreview = await onPreviewRuntimeConfigBulkOverride({
          selector_expression: frozenSelector,
          target_client_ids: [],
          patch: toml,
          reason: patchName,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const nextPreview = reviewedTargets(
          nextChangePreview.target_client_ids,
        );
        setPreview(nextPreview);
        setChangePreview(nextChangePreview);
        setApplySnapshot(null);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
  }

  async function reviewApply() {
    const firstPreviewClientIds = [...(changePreview?.target_client_ids ?? [])];
    const firstPreviewHash = changePreview?.preview_hash ?? "";
    clearBulkConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenGenerator = selectedGenerator;
    const frozenPatchMode = patchMode;
    const frozenPrivilegeMaterial = privilegeMaterial;
    const frozenSelector = selectorExpression.trim();
    const frozenValuesText = valuesText;
    const frozenTemporaryToml = temporaryToml;
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
    setReviewStatus("Preparing bulk patch review");
    try {
      await runAction(async () => {
        await waitForReviewRender();
        if (!frozenPrivilegeMaterial) {
          throw new Error("Bulk patch apply is incomplete");
        }
        if (frozenPatchMode === "generator" && !frozenGenerator) {
          throw new Error("Select a patch generator");
        }
        if (selectorParse.error) {
          throw new Error(selectorParse.error);
        }
        if (!frozenSelector) {
          throw new Error("Add at least one target selector");
        }
        if (!firstPreviewClientIds.length || !firstPreviewHash) {
          throw new Error("Preview changes before applying this bulk patch");
        }
        if (frozenPatchMode === "temporary" && !frozenTemporaryToml.trim()) {
          throw new Error("Paste a temporary TOML patch");
        }
        let toml = frozenTemporaryToml.trim();
        let patchName = "Temporary patch";
        let patchSections = inferTomlSections(toml);
        if (frozenPatchMode === "generator") {
          const frozenValues = parseJsonObject(frozenValuesText);
          const nextRendered = await onRenderRuntimeConfigPatchGenerator(
            frozenGenerator!.id,
            { values: frozenValues },
          );
          if (!isReviewGenerationCurrent(reviewGeneration)) {
            return;
          }
          toml = nextRendered.toml;
          patchName = frozenGenerator!.name;
          patchSections = nextRendered.affected_sections;
          setRendered(nextRendered);
        }
        const patchPayloadHashHex = await sha256Hex(
          new TextEncoder().encode(toml),
        );
        const nextChangePreview = await onPreviewRuntimeConfigBulkOverride({
          selector_expression: frozenSelector,
          target_client_ids: firstPreviewClientIds,
          patch: toml,
          reason: patchName,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const clientIds = nextChangePreview.target_client_ids;
        if (!sameStringArray(clientIds, firstPreviewClientIds)) {
          throw new Error(
            "The server did not preserve the reviewed VPS target set; preview changes again",
          );
        }
        if (nextChangePreview.preview_hash !== firstPreviewHash) {
          throw new Error(
            "Desired or override state changed since the displayed preview; preview changes again",
          );
        }
        if (!clientIds.length) {
          throw new Error("Bulk patch confirmation resolved no VPSs");
        }
        const nextPreview = reviewedTargets(clientIds);
        const privilegeAssertion = await buildPrivilegeAssertion({
          intent: canonicalDbPrivilegeIntent({
            action: "runtime_config.override.bulk_apply",
            target: "runtime_config",
            selectorExpression: frozenSelector,
            resolvedTargets: clientIds,
            confirmed: true,
            payloadHash: nextChangePreview.preview_hash,
          }),
          privilegeMaterial: frozenPrivilegeMaterial,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(nextPreview);
        setChangePreview(nextChangePreview);
        setApplySnapshot({
          clientIds,
          jobId: crypto.randomUUID(),
          toml,
          patchName,
          patchSections,
          patchSource: frozenPatchMode,
          payloadHashHex: patchPayloadHashHex,
          previewHash: nextChangePreview.preview_hash,
          privilegeAssertion,
          selectorExpression: frozenSelector,
          targets: nextPreview.targets,
          maxTimeoutSecs: boundedMaxTimeoutSecs,
        });
        setConfirmOpen(true);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
  }

  async function applyPatch() {
    setConfirmOpen(false);
    await runAction(async () => {
      const snapshot = applySnapshot;
      if (!snapshot) {
        throw new Error(
          "Bulk patch confirmation snapshot is missing; review the apply again",
        );
      }
      const response = await onApplyRuntimeConfigBulkOverride({
        confirmed: true,
        reason: snapshot.patchName,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.clientIds,
        patch: snapshot.toml,
        preview_hash: snapshot.previewHash,
        privilege_assertion: snapshot.privilegeAssertion,
      });
      const dispatchWarning = runtimeConfigDispatchWarning(
        response.sync,
        "VPS override patch saved",
        response.preview.targets.filter(
          (target) => !target.no_op && !target.storage_only,
        ).length,
      );
      if (dispatchWarning) {
        setReviewStatus(dispatchWarning);
        setApplySnapshot(null);
        return;
      }
      const jobIds = response.sync_job_ids;
      if (jobIds.length === 0) {
        setReviewStatus(
          "VPS override patch saved; no runtime sync was required",
        );
        setApplySnapshot(null);
        return;
      }
      const queuedClientIds = new Set(
        response.sync.map((outcome) => outcome.client_id),
      );
      const targetStatusItems = response.sync.map((outcome) => {
        if (!outcome.job_id) {
          throw new Error(
            `Runtime apply job for ${outcome.client_id} omitted its job ID`,
          );
        }
        return { client_id: outcome.client_id, job_id: outcome.job_id };
      });
      const queuedTargets = snapshot.targets.filter((target) =>
        queuedClientIds.has(target.id),
      );
      const targetCount = queuedTargets.length;
      const initial = buildBulkJobProgress({
        targetCount,
        jobId: snapshot.jobId,
        jobIds,
        targetRecords: [],
        targets: queuedTargets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      setProgress(initial);
      const waited = await waitForBulkJobSet(jobIds, onLoadJobTargets, {
        operationId: snapshot.jobId,
        targetCount,
        onProgress: setProgress,
        targets: queuedTargets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
        exactTargetStatusItems: targetStatusItems,
        onLoadExactTargetStatuses: onLoadExactJobTargetStatuses,
      });
      setProgress(waited.progress);
      setApplySnapshot(null);
    });
  }

  return (
    <div className="configApplyGrid">
      <section
        className="compactForm bulkPatchPrimary"
        title={
          patchMode === "generator"
            ? "Supply values for the selected generator and review its server-rendered TOML patch before dispatch."
            : "Write an incremental TOML patch to review before dispatch."
        }
      >
        <div className="bulkPatchHeader">
          <ConfigHelpLabel
            help={CONFIG_HELP.incrementalPatch}
            label="Advanced · VPS override patch"
            strong
          />
          <button
            className="secondaryAction"
            onClick={() => {
              if (manageGeneratorsOpen) {
                setManageGeneratorsOpen(false);
                return;
              }
              openGeneratorManagement();
            }}
            type="button"
          >
            Manage generators
          </button>
        </div>
        <div className="segmentedControl" aria-label="Patch source">
          <button
            className={patchMode === "generator" ? "active" : ""}
            onClick={() => {
              setPatchMode("generator");
              clearBulkConfigReview();
            }}
            type="button"
          >
            Saved generator
          </button>
          <button
            className={patchMode === "temporary" ? "active" : ""}
            onClick={() => {
              setPatchMode("temporary");
              setRendered(null);
              clearBulkConfigReview();
            }}
            type="button"
          >
            Temporary patch
          </button>
        </div>
        <small className="formHint">
          Bulk editing intentionally stays text-based. Delete one saved value
          with <code>-field.path</code> or a saved section with{" "}
          <code>-[section.path]</code>; preview shows every resulting VPS
          override before apply.
        </small>
        {patchMode === "generator" ? (
          <>
            <small className="formHint" id="bulk-patch-generator-help">
              {CONFIG_HELP.patchGenerator}
            </small>
            <select
              aria-describedby="bulk-patch-generator-help"
              aria-label="Patch generator"
              onChange={(event) => {
                setGeneratorId(event.target.value);
                clearBulkConfigReview();
              }}
              value={selectedGenerator?.id ?? ""}
            >
              {runtimeConfigPatchGenerators.map((generator) => (
                <option key={generator.id} value={generator.id}>
                  {generator.name}
                </option>
              ))}
            </select>
            {selectedGenerator && (
              <div
                className="bulkPatchGeneratorSummary"
                title={selectedGenerator.description}
              >
                <strong>{selectedGenerator.name}</strong>
                <span>
                  {selectedGenerator.category} / {selectedGenerator.domain}
                </span>
              </div>
            )}
            <textarea
              aria-label="Patch generator values JSON"
              onChange={(event) => {
                setValuesText(event.target.value);
                setRendered(null);
                clearBulkConfigReview();
              }}
              rows={7}
              value={valuesText}
            />
            {rendered && (
              <textarea
                aria-label="Rendered bulk runtime config patch TOML"
                readOnly
                rows={8}
                value={rendered.toml}
              />
            )}
          </>
        ) : (
          <textarea
            aria-label="Temporary bulk runtime config patch TOML"
            onChange={(event) => {
              setTemporaryToml(event.target.value);
              clearBulkConfigReview();
            }}
            placeholder={
              "# set a value\ntelemetry_interval_secs = 30\n\n# delete a saved field\n-telemetry_interval_secs\n\n# delete a saved section\n-[update]"
            }
            rows={14}
            value={temporaryToml}
          />
        )}
      </section>
      <section className="compactForm bulkPatchTargetPanel">
        <ConfigHelpLabel
          help={CONFIG_HELP.targetSelector}
          label="Targets"
          strong
        />
        <SearchExpressionInput
          agents={agents}
          ariaLabel="Bulk patch target expression"
          className="targetExpressionBar"
          onChange={(value) => {
            setSelectorExpression(value);
            setPreview(null);
            clearBulkConfigReview();
          }}
          placeholder="provider:hetzner && country:US"
          showMatchCount
          value={selectorExpression}
          verification={
            selectorParse.error
              ? "invalid"
              : selectorExpression.trim()
                ? "valid"
                : "neutral"
          }
          verificationMessage={
            (selectorEvidenceUnavailable
              ? VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE
              : selectorParse.error) ??
            (preview
              ? `${preview.target_count}/${agents.length}`
              : selectorExpression.trim()
                ? `${localSelectorTargets.length}/${agents.length} local`
                : "no selector")
          }
        />
        <div className="bulkTargetState">
          <strong>
            {preview
              ? `${bulkVpsCountLabel(preview.target_count)} verified`
              : selectorEvidenceUnavailable
                ? VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE
                : selectorExpression.trim()
                  ? `${bulkVpsCountLabel(localSelectorTargets.length)} matched locally`
                  : "No target selector"}
          </strong>
          <span>
            {preview
              ? "These server-verified VPS IDs are frozen into this review. Apply submits exactly this set without re-resolving the selector."
              : selectorExpression.trim()
                ? "The matching VPSs update immediately below. Preview changes verifies them on the server and builds the per-VPS patch summary."
                : "Add a selector; an empty selector is never treated as all VPSs."}
          </span>
        </div>
        {!selectorEvidenceUnavailable ? (
          <LocalTargetPreview agents={localSelectorTargets} />
        ) : null}
        <button
          className="secondaryAction"
          disabled={pending || !canPreviewChanges}
          onClick={() => void previewChanges()}
          title={
            pending
              ? "Wait for the current config operation to finish before previewing changes."
              : !canPreviewChanges
                ? "Choose a patch source and target selector before previewing changes."
                : "Render the patch, resolve targets, and show per-VPS change summary."
          }
          type="button"
        >
          Preview changes
        </button>
        <ActionFeedback
          className="localActionFeedback configReviewFeedback"
          message={reviewStatus}
          tone={reviewFeedbackTone}
        />
        <BulkPatchChangeSummary
          changePreview={changePreview}
          patchMode={patchMode}
          patchName={
            patchMode === "temporary"
              ? "Temporary patch"
              : (rendered?.name ?? selectedGenerator?.name ?? "Saved generator")
          }
          preview={preview}
          sections={previewPatchSections}
          toml={previewToml ?? ""}
        />
        <details className="singleConfigAdvanced bulkPatchAdvanced">
          <summary>Advanced apply options</summary>
          <label>
            <ConfigHelpLabel
              help={CONFIG_HELP.maxTimeout}
              label="Max timeout seconds"
            />
            <input
              aria-label="Bulk patch max timeout seconds"
              max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
              min={1}
              onChange={(event) => {
                setMaxTimeoutSecs(Number(event.target.value));
                clearBulkConfigReview();
              }}
              type="number"
              value={maxTimeoutSecs}
            />
          </label>
        </details>
        <div className="singleConfigStickyActions bulkPatchApplyActions">
          <span>
            {ready
              ? privilegeMaterial
                ? `Ready to apply ${bulkVpsCountLabel(preview?.target_count ?? 0)}`
                : `Preview ready for ${bulkVpsCountLabel(preview?.target_count ?? 0)}; applying will open privilege unlock.`
              : "Preview changes before applying a bulk runtime config patch."}
          </span>
          <button
            className="primaryAction"
            disabled={pending || !ready}
            onClick={() => {
              if (!privilegeMaterial) {
                setReviewStatus(
                  "Unlock privilege to apply this reviewed patch",
                );
                onOpenPrivilegeUnlock();
                return;
              }
              void reviewApply();
            }}
            title={
              pending
                ? "Wait for the current config operation to finish before opening apply confirmation."
                : !ready
                  ? "Preview changes before applying."
                  : !privilegeMaterial
                    ? "Open the shared privilege unlock, then apply this unchanged preview."
                    : "Open the final runtime config apply confirmation."
            }
            type="button"
          >
            <FileSliders size={16} />
            Apply override patch
          </button>
        </div>
      </section>
      {manageGeneratorsOpen && (
        <section
          aria-label="Patch generator registry"
          className="bulkGeneratorManagement"
          ref={generatorManagementRef}
          tabIndex={-1}
        >
          <div className="bulkGeneratorManagementHeader">
            <strong>Patch generator registry</strong>
            <button
              aria-label="Close patch generator registry"
              className="iconButton"
              onClick={() => setManageGeneratorsOpen(false)}
              title="Close generator registry"
              type="button"
            >
              <X size={17} />
            </button>
          </div>
          <>
            <ActionFeedback
              className="localActionFeedback patchGeneratorActionFeedback"
              message={patchGeneratorStatus}
              tone="success"
            />
            <ConsoleDataGrid
              actions={patchGeneratorActions}
              columns={patchGeneratorColumns}
              defaultPageSize={10}
              expandOnRowClick
              getRowId={(generator) => generator.id}
              itemLabel="patch generators"
              empty="No patch generators match the current search."
              renderExpandedRow={(generator) => (
                <div className="consoleInlineDetailGrid">
                  {detailField("Generator ID", generator.id)}
                  {detailField("Name", generator.name)}
                  {detailField("Category", generator.category)}
                  {detailField("Domain", generator.domain)}
                  {detailField(
                    "Scope",
                    generator.built_in ? "built-in" : "custom",
                  )}
                  {detailField("Updated", formatTime(generator.updated_at))}
                  {detailField(
                    "Schema",
                    JSON.stringify(generator.field_schema, null, 2),
                    true,
                  )}
                  {detailField(
                    "Docs",
                    JSON.stringify(generator.docs_metadata, null, 2),
                    true,
                  )}
                </div>
              )}
              rowActions={patchGeneratorActions}
              rows={runtimeConfigPatchGenerators}
              searchPlaceholder="Search patch generators"
              storageKey="vpsman.config.patchGenerators"
              title="Patch generators"
              toolbarActions={
                <button
                  className="primaryAction compactAction"
                  onClick={() => openPatchGeneratorEditor()}
                  type="button"
                >
                  <FileSliders size={15} />
                  <span>New generator</span>
                </button>
              }
            />
            {patchGeneratorEditor ? (
              <section
                className="compactForm patchGeneratorEditor"
                ref={patchGeneratorEditorRef}
                aria-label={
                  patchGeneratorEditor.mode === "edit"
                    ? "Edit patch generator"
                    : "New patch generator"
                }
              >
                <div className="bulkPatchHeader">
                  <strong>
                    {patchGeneratorEditor.mode === "edit"
                      ? "Edit custom generator"
                      : "New custom generator"}
                  </strong>
                  <button
                    className="iconButton"
                    onClick={() => setPatchGeneratorEditor(null)}
                    title="Close generator editor"
                    type="button"
                  >
                    <X size={16} />
                  </button>
                </div>
                <div className="consoleFormGrid">
                  <label className="consoleField fieldWide">
                    <span>Name</span>
                    <input
                      required
                      value={patchGeneratorEditor.name}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({ name: event.target.value })
                      }
                      placeholder="Custom runtime toggle"
                    />
                  </label>
                  <label className="consoleField">
                    <span>Category</span>
                    <input
                      required
                      value={patchGeneratorEditor.category}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({
                          category: event.target.value,
                        })
                      }
                      placeholder="network"
                    />
                  </label>
                  <label className="consoleField">
                    <span>Domain</span>
                    <input
                      required
                      value={patchGeneratorEditor.domain}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({
                          domain: event.target.value,
                        })
                      }
                      placeholder="runtime"
                    />
                  </label>
                  <label className="consoleField fieldFull">
                    <span>Description</span>
                    <input
                      required
                      value={patchGeneratorEditor.description}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({
                          description: event.target.value,
                        })
                      }
                      placeholder="What this generator changes"
                    />
                  </label>
                  <label
                    className="consoleField fieldFull"
                    title="Define the patch generator fields as JSON."
                  >
                    <span>Field schema JSON</span>
                    <textarea
                      required
                      rows={7}
                      value={patchGeneratorEditor.fieldSchemaText}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({
                          fieldSchemaText: event.target.value,
                        })
                      }
                    />
                  </label>
                  <label
                    className="consoleField fieldFull"
                    title="Define the configuration template rendered by this patch generator."
                  >
                    <span>Generator body</span>
                    <textarea
                      required
                      rows={8}
                      value={patchGeneratorEditor.rawGeneratorBody}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({
                          rawGeneratorBody: event.target.value,
                        })
                      }
                      placeholder="[section]\nkey = {{value}}"
                    />
                  </label>
                  <label
                    className="consoleField fieldFull"
                    title="Define patch generator documentation metadata as JSON."
                  >
                    <span>Docs metadata JSON</span>
                    <textarea
                      required
                      rows={6}
                      value={patchGeneratorEditor.docsMetadataText}
                      onChange={(event) =>
                        updatePatchGeneratorEditor({
                          docsMetadataText: event.target.value,
                        })
                      }
                    />
                  </label>
                </div>
                <ActionFeedback
                  className="localActionFeedback patchGeneratorEditorFeedback"
                  message={patchGeneratorDraftError}
                  tone="warning"
                />
                <div className="consoleFormActions">
                  <button
                    className="secondaryAction"
                    onClick={() => setPatchGeneratorEditor(null)}
                    type="button"
                  >
                    Cancel
                  </button>
                  <button
                    className="primaryAction"
                    disabled={pending || Boolean(patchGeneratorDraftError)}
                    onClick={() => void savePatchGeneratorEditor()}
                    title={
                      pending
                        ? "Wait for the current configuration operation to finish"
                        : (patchGeneratorDraftError ??
                          "Save the custom patch generator")
                    }
                    type="button"
                  >
                    Save generator
                  </button>
                </div>
              </section>
            ) : null}
          </>
        </section>
      )}
      {progress && (
        <ExecutionResultPanel
          loading={pending}
          onClearResults={() => setProgress(null)}
          onOpenJobDetails={onOpenJobDetails}
          onOpenJobHistory={onOpenJobHistory}
          progress={progress}
        />
      )}
      <ConfirmationPrompt
        confirmLabel="Apply VPS override patch"
        detail={`Apply one reviewed override patch to ${bulkVpsCountLabel(applySnapshot?.clientIds.length ?? 0)}.`}
        error={actionError}
        expiresAtUnix={applySnapshot?.privilegeAssertion.expires_unix}
        items={[
          {
            label: "Selector",
            value: applySnapshot?.selectorExpression ?? "-",
            title:
              applySnapshot?.selectorExpression ??
              "No frozen selector is available because the bulk patch review is not open",
          },
          {
            label: "Targets",
            value: `${applySnapshot?.clientIds.length ?? 0}`,
          },
          {
            label: "Source",
            value: applySnapshot?.patchSource ?? "-",
            title:
              applySnapshot?.patchSource ??
              "No patch source is available because the bulk patch review is not open",
          },
          {
            label: "Patch",
            value: applySnapshot?.patchName ?? "-",
            title:
              applySnapshot?.patchName ??
              "No patch name is available because the bulk patch review is not open",
          },
          {
            label: "Sections",
            value: applySnapshot?.patchSections.join(", ") ?? "-",
            title:
              applySnapshot?.patchSections.join(", ") ||
              "No rendered configuration sections are available",
          },
          {
            label: "Payload",
            value: applySnapshot?.payloadHashHex
              ? shortId(applySnapshot.payloadHashHex)
              : "-",
            title:
              applySnapshot?.payloadHashHex ??
              "No frozen payload hash is available because the bulk patch review is not open",
          },
        ]}
        onCancel={() => {
          setConfirmOpen(false);
          setApplySnapshot(null);
        }}
        onConfirm={() => void applyPatch()}
        open={confirmOpen}
        pending={pending}
        title="Confirm VPS override patch"
      />
      <ConfirmationPrompt
        confirmLabel="Delete patch generator"
        detail="This removes the reviewed operator-managed patch generator. Built-in patch generators are read-only."
        error={actionError}
        items={[
          { label: "Generator", value: deleteGenerator?.name ?? "" },
          { label: "Domain", value: deleteGenerator?.domain ?? "" },
        ]}
        onCancel={() => setDeleteGenerator(null)}
        onConfirm={() => void deleteSelectedPatchGenerator()}
        open={deleteGenerator !== null}
        pending={pending}
        title="Delete patch generator"
        tone="danger"
      />
    </div>
  );
}

function detailField(label: string, value: string, pre = false) {
  return (
    <span key={label}>
      <strong>{label}</strong>
      {pre ? <pre>{value}</pre> : <span>{value}</span>}
    </span>
  );
}

function runtimeConfigDispatchWarning(
  sync: RuntimeConfigDispatchRecord[],
  savedMessage: string,
  requiredSyncTargetCount: number,
): string | null {
  const failures = sync.filter((outcome) => outcome.status !== "queued");
  if (requiredSyncTargetCount === 0 && sync.length === 0) return null;
  if (failures.length === 0 && sync.length > 0) return null;
  const queued = sync.length - failures.length;
  const failedTargets = failures
    .map(
      (outcome) =>
        `${outcome.client_id}: ${dispatchFailureReason(outcome.error, outcome.status, "Runtime apply job")}`,
    )
    .join("; ");
  return `${savedMessage}. ${queued > 0 ? `${queued} runtime apply job${queued === 1 ? " was" : "s were"} queued; ` : ""}${failures.length > 0 ? `apply was not queued for ${failedTargets}. ` : "No runtime apply job was queued. "}Review runtime apply state before treating the change as active.`;
}

function BulkPatchChangeSummary({
  changePreview,
  patchMode,
  patchName,
  preview,
  sections,
  toml,
}: {
  changePreview: RuntimeConfigBulkOverridePreview | null;
  patchMode: "generator" | "temporary";
  patchName: string;
  preview: BulkResolveResponse | null;
  sections: string[];
  toml: string;
}) {
  const visibleTargets = (preview?.targets ?? []).slice(0, 8);
  const sectionSummary =
    sections.length > 0
      ? sections.join(", ")
      : inferTomlSections(toml).join(", ");
  const sourceLabel =
    patchMode === "generator" ? "Saved generator" : "Temporary patch";
  const storageOnlyTargetCount =
    changePreview?.targets.filter((target) => target.storage_only).length ?? 0;
  const runtimeChangeTargetCount = Math.max(
    0,
    (changePreview?.changed_target_count ?? 0) - storageOnlyTargetCount,
  );
  const noOpTargetCount = Math.max(
    0,
    (preview?.target_count ?? 0) - (changePreview?.changed_target_count ?? 0),
  );

  return (
    <div
      className="bulkPatchPreviewSummary"
      aria-label="Bulk patch change summary"
    >
      <div>
        <strong>Preview changes</strong>
        <span>
          {preview && changePreview
            ? storageOnlyTargetCount > 0
              ? `${bulkVpsCountLabel(runtimeChangeTargetCount)} runtime change; ${bulkVpsCountLabel(storageOnlyTargetCount)} stored TOML only; ${bulkVpsCountLabel(noOpTargetCount)} no-op.`
              : `${bulkVpsCountLabel(changePreview.changed_target_count)} change; ${bulkVpsCountLabel(noOpTargetCount)} no-op.`
            : "Preview changes renders the patch and resolves exact VPS targets."}
        </span>
      </div>
      <div className="bulkPatchPreviewMeta">
        <span>{sourceLabel}</span>
        <span>{sectionSummary || "Sections pending"}</span>
        <span>{toml ? `${toml.length} TOML chars` : "Patch pending"}</span>
      </div>
      {visibleTargets.length > 0 ? (
        <div className="bulkPatchTargetRows">
          {visibleTargets.map((target) => {
            const targetPreview = changePreview?.targets.find(
              (candidate) => candidate.client_id === target.id,
            );
            return (
              <span key={target.id} title={target.id}>
                <strong>{target.display_name}</strong>
                <small>
                  {targetPreview
                    ? targetPreview.no_op
                      ? "No change"
                      : targetPreview.storage_only
                        ? "Stored TOML only"
                        : `${targetPreview.changes.length} ${targetPreview.changes.length === 1 ? "change" : "changes"}`
                    : sectionSummary || "VPS override patch"}
                </small>
              </span>
            );
          })}
          {preview && preview.target_count > visibleTargets.length && (
            <span className="mutedChip">
              +{preview.target_count - visibleTargets.length} more VPSs
            </span>
          )}
        </div>
      ) : null}
    </div>
  );
}

function bulkVpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function sameStringArray(left: string[], right: string[]): boolean {
  return (
    left.length === right.length &&
    left.every((value, index) => value === right[index])
  );
}

type VpsRulesReviewSnapshot = {
  operation: "upsert" | "unset";
  selectorExpression: string;
  values: Record<string, string>;
  keys: string[];
  preview: VpsRulesOperatorPreview;
};

type VpsRulesOperatorPreview = VpsRulesDryRunResponse & {
  no_op_row_count: number;
};

type VpsRulesEditMode = "upsert" | "unset";

type VpsRuleAlertPolicyImpact = {
  conditionExpression: string;
  enabled: boolean;
  phase: "Trigger" | "Resolve";
  policyId: string;
  policyName: string;
  ruleId: string;
  ruleName: string;
  severity: string;
};

function vpsRuleEditKeys(valuesText: string, unsetKeys: string[]): string[] {
  const keys = new Set<string>();
  for (const rawLine of valuesText.split(/\r?\n/)) {
    const key = rawLine.split("=")[0]?.trim();
    if (VPS_RULE_KEYS.includes(key as (typeof VPS_RULE_KEYS)[number])) {
      keys.add(key);
    }
  }
  for (const key of unsetKeys) {
    if (VPS_RULE_KEYS.includes(key as (typeof VPS_RULE_KEYS)[number])) {
      keys.add(key);
    }
  }
  return Array.from(keys).sort((left, right) => left.localeCompare(right));
}

type VpsRuleTextLine = {
  content: string;
  ending: string;
};

type ParsedVpsRuleTextLine = {
  equals: number;
  key: (typeof VPS_RULE_KEYS)[number];
};

function splitVpsRuleTextLines(text: string): VpsRuleTextLine[] {
  const lines: VpsRuleTextLine[] = [];
  const pattern = /([^\r\n]*)(\r\n|\r|\n|$)/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text)) !== null) {
    if (!match[0]) {
      break;
    }
    lines.push({ content: match[1], ending: match[2] });
    if (!match[2]) {
      break;
    }
  }
  return lines;
}

function joinVpsRuleTextLines(lines: VpsRuleTextLine[]): string {
  return lines.map((line) => `${line.content}${line.ending}`).join("");
}

function parseVpsRuleTextLine(content: string): ParsedVpsRuleTextLine | null {
  const equals = content.indexOf("=");
  if (equals <= 0) {
    return null;
  }
  const key = content.slice(0, equals).trim();
  if (!VPS_RULE_KEYS.includes(key as (typeof VPS_RULE_KEYS)[number])) {
    return null;
  }
  return { equals, key: key as (typeof VPS_RULE_KEYS)[number] };
}

function parseVpsRuleTextValues(text: string): Record<string, string> {
  const values: Record<string, string> = {};
  for (const line of splitVpsRuleTextLines(text)) {
    const parsed = parseVpsRuleTextLine(line.content);
    if (!parsed) {
      continue;
    }
    values[parsed.key] = line.content.slice(parsed.equals + 1);
  }
  return values;
}

function serializeVpsRuleTextValues(values: Record<string, string>): string {
  return VPS_RULE_KEYS.flatMap((key) => {
    const value = values[key]?.trim();
    return value ? [`${key}=${value}`] : [];
  }).join("\n");
}

function updateVpsRuleTextDraftValue(
  text: string,
  key: (typeof VPS_RULE_KEYS)[number],
  value: string,
): string {
  const lines = splitVpsRuleTextLines(text);
  let matchingLine = -1;
  let matchingEquals = -1;
  lines.forEach((line, index) => {
    const parsed = parseVpsRuleTextLine(line.content);
    if (parsed?.key === key) {
      matchingLine = index;
      matchingEquals = parsed.equals;
    }
  });
  if (matchingLine >= 0) {
    const line = lines[matchingLine];
    line.content = `${line.content.slice(0, matchingEquals + 1)}${value}`;
    return joinVpsRuleTextLines(lines);
  }
  if (lines.length > 0 && !lines[lines.length - 1].ending) {
    const separator = text.includes("\r\n")
      ? "\r\n"
      : text.includes("\r")
        ? "\r"
        : "\n";
    lines[lines.length - 1].ending = separator;
  }
  lines.push({ content: `${key}=${value}`, ending: "" });
  return joinVpsRuleTextLines(lines);
}

function normalizeVpsRuleTextValueOnBlur(
  text: string,
  key: (typeof VPS_RULE_KEYS)[number],
  value: string,
): string {
  const canonicalValue = tryNormalizeVpsRuleValue(key, value);
  const lines = splitVpsRuleTextLines(text);
  if (canonicalValue !== null) {
    for (let index = lines.length - 1; index >= 0; index -= 1) {
      if (parseVpsRuleTextLine(lines[index].content)?.key === key) {
        lines[index].content = `${key}=${canonicalValue}`;
        return joinVpsRuleTextLines(lines);
      }
    }
    return updateVpsRuleTextDraftValue(text, key, canonicalValue);
  }
  if (value.trim()) {
    return text;
  }
  return joinVpsRuleTextLines(
    lines.filter((line) => parseVpsRuleTextLine(line.content)?.key !== key),
  );
}

function normalizeVpsRuleTextOnBlur(text: string): string {
  const lines = splitVpsRuleTextLines(text);
  const parsedLines = lines.map((line) => parseVpsRuleTextLine(line.content));
  const keyCounts = new Map<(typeof VPS_RULE_KEYS)[number], number>();
  for (const parsed of parsedLines) {
    if (parsed) {
      keyCounts.set(parsed.key, (keyCounts.get(parsed.key) ?? 0) + 1);
    }
  }
  lines.forEach((line, index) => {
    const parsed = parsedLines[index];
    if (!parsed || keyCounts.get(parsed.key) !== 1) {
      return;
    }
    const canonicalValue = tryNormalizeVpsRuleValue(
      parsed.key,
      line.content.slice(parsed.equals + 1),
    );
    if (canonicalValue !== null) {
      line.content = `${parsed.key}=${canonicalValue}`;
    }
  });
  return joinVpsRuleTextLines(
    lines.filter((line, index) => {
      const parsed = parsedLines[index];
      return !(
        parsed &&
        keyCounts.get(parsed.key) === 1 &&
        !line.content.slice(parsed.equals + 1).trim()
      );
    }),
  );
}

function affectedAlertPolicyRules(
  policies: FleetAlertPolicyRecord[],
  keys: string[],
): VpsRuleAlertPolicyImpact[] {
  const matchKeys = keys.length > 0 ? keys : [...VPS_RULE_KEYS];
  return policies
    .flatMap((policy) =>
      policy.rules.flatMap((rule) =>
        [
          {
            conditionExpression: rule.trigger_condition_expression,
            phase: "Trigger" as const,
          },
          ...(rule.resolve_condition_expression
            ? [
                {
                  conditionExpression: rule.resolve_condition_expression,
                  phase: "Resolve" as const,
                },
              ]
            : []),
        ]
          .filter(({ conditionExpression }) =>
            matchKeys.some((key) => conditionExpression.includes(key)),
          )
          .map(({ conditionExpression, phase }) => ({
            conditionExpression,
            enabled: policy.enabled && rule.enabled,
            phase,
            policyId: policy.id,
            policyName: policy.name,
            ruleId: rule.id,
            ruleName: rule.name,
            severity: rule.severity,
          })),
      ),
    )
    .sort(
      (left, right) =>
        left.policyName.localeCompare(right.policyName) ||
        left.ruleName.localeCompare(right.ruleName),
    );
}

function buildOperatorVpsRulesPreview(
  preview: VpsRulesDryRunResponse,
): VpsRulesOperatorPreview {
  const changes = preview.changes.filter(
    (change) => !isValidVpsRuleChange(change) || !isNoOpVpsRuleChange(change),
  );
  const changedRowCount = changes.filter(isValidVpsRuleChange).length;
  const noOpRowCount = preview.changes.filter(
    (change) => isValidVpsRuleChange(change) && isNoOpVpsRuleChange(change),
  ).length;
  return {
    ...preview,
    changed_row_count: changedRowCount,
    changes,
    no_op_row_count: noOpRowCount,
  };
}

function isValidVpsRuleChange(change: VpsRuleChangePreview): boolean {
  return change.validation === "ok" && change.validation_errors.length === 0;
}

function isNoOpVpsRuleChange(change: VpsRuleChangePreview): boolean {
  if (!isValidVpsRuleChange(change)) {
    return false;
  }
  return (
    normalizeVpsRuleValue(change.key, change.before) ===
    normalizeVpsRuleValue(change.key, change.after)
  );
}

const VPS_RULE_VALIDATION_MESSAGES: Record<string, string> = {
  billing_plan_price_required:
    "Enter a price before the currency and /period, or use -1 to disable billing.",
  billing_plan_price_invalid:
    "Use a positive decimal price, or -1 to disable billing.",
  billing_plan_currency_required:
    "Add a currency symbol or three-letter currency code after the price.",
  billing_plan_currency_invalid:
    "Use $, ¥, €, £, or a three-letter currency code.",
  billing_plan_period_required:
    "Add /m, /q, /hy, or /y after the billing price.",
  billing_plan_period_invalid: "Billing period must be m, q, hy, h, or y.",
  billing_cycle_day_invalid: "Billing-cycle day must be between 1 and 31.",
  billing_cycle_month_invalid: "Billing-cycle month must be between 1 and 12.",
  billing_cycle_requires_price:
    "Set a billing price before setting its renewal cycle.",
  billing_cycle_disabled_price_invalid:
    "Remove the renewal cycle while billing is disabled with -1.",
  billing_month_cycle_requires_day:
    "Monthly billing uses a day only, such as 15.",
  billing_long_cycle_requires_month_day:
    "Quarterly, half-year, and yearly billing use MM-DD, such as 06-15. M-D shorthand is also accepted.",
  port_speed_unit_required: "Add bps, Kbps, Mbps, Gbps, or Tbps.",
  port_speed_unit_invalid:
    "Port-speed unit must be bps, Kbps, Mbps, Gbps, or Tbps.",
  port_speed_value_invalid:
    "Port speed must be a positive number with at most three decimal places.",
  port_speed_value_too_large: "Port speed is larger than the supported range.",
  network_rate_selector_source_invalid:
    "Live-rate selectors use host interfaces only; remove the tunnel: prefix.",
  traffic_selector_empty: "Enter at least one interface selector.",
  traffic_selector_empty_item:
    "Remove the empty selector entry between commas.",
  traffic_selector_source_invalid: "Selector source must be host or tunnel.",
  traffic_selector_interface_required: "Each selector needs an interface name.",
  traffic_selector_interface_invalid:
    "Use an exact interface name without spaces or wildcards.",
  traffic_selector_direction_invalid:
    "Selector direction must be rx, tx, total, tx/rx (or rx/tx), or rx+tx (or tx+rx).",
  traffic_selector_duplicate: "Remove the duplicate selector.",
  traffic_selector_direction_overlap:
    "Do not select the same interface direction more than once.",
  traffic_selector_too_many_items: "Use no more than 16 selectors.",
  network_interfaces_pattern_invalid:
    "Use exact interface names or one trailing * prefix wildcard.",
  network_interfaces_pattern_duplicate: "Remove the duplicate interface pattern.",
  network_interfaces_all_must_stand_alone:
    "Use * by itself to select every reported interface.",
  network_interfaces_too_many_patterns: "Use no more than 16 interface patterns.",
  traffic_reset_day_invalid:
    "Traffic reset time must be -1 for continuous accumulation, or a UTC day and hour such as 29 05:00.",
  byte_size_empty: "Enter a traffic quota or use -1 for unlimited.",
  byte_size_number_invalid: "Traffic quota must start with a valid number.",
  byte_size_unit_invalid: "Use bytes, KB, MB, GB, TB, KiB, MiB, GiB, or TiB.",
  byte_size_too_large: "Traffic quota is larger than the supported range.",
};

function vpsRuleValidationMessages(change: VpsRuleChangePreview): string[] {
  const codes =
    change.validation_errors.length > 0
      ? change.validation_errors
      : change.validation === "ok"
        ? []
        : [change.validation];
  return codes.map(
    (code) =>
      VPS_RULE_VALIDATION_MESSAGES[code] ??
      code
        .replace(/_/g, " ")
        .replace(/^./, (letter: string) => letter.toUpperCase()),
  );
}

function VpsRulesPanel({
  agents,
  fleetAlertPolicies,
  initialSelectorExpression,
  onBulkUnset,
  onBulkUpsert,
  onDryRun,
  onLoadEffectiveVpsRules,
  onOpenAlerts,
  trafficAccounting,
  vpsRuleValues,
}: {
  agents: AgentView[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  initialSelectorExpression: string | null;
  onBulkUnset: (
    request: VpsRulesBulkUnsetRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsert: (
    request: VpsRulesBulkUpsertRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onDryRun: (request: VpsRulesDryRunRequest) => Promise<VpsRulesDryRunResponse>;
  onLoadEffectiveVpsRules: (clientId: string) => Promise<VpsRuleValueRecord[]>;
  onOpenAlerts: () => void;
  trafficAccounting: TrafficAccountingRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
}) {
  const vpsRuleSearch = useVpsRuleSearchContext();
  const [selectorExpression, setSelectorExpression] = useState(
    () =>
      initialSelectorExpression ??
      readLocalString(CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY),
  );
  const selectorExpressionRef = useRef(selectorExpression);
  const [keyFilter, setKeyFilter] = useState("");
  const [stateFilter, setStateFilter] = useState("");
  const [showIncompleteOnly, setShowIncompleteOnly] = useState(false);
  const [valuesText, setValuesText] = useState("");
  const [unsetKeys, setUnsetKeys] = useState<string[]>([]);
  const [editMode, setEditMode] = useState<VpsRulesEditMode>("upsert");
  const [preview, setPreview] = useState<VpsRulesOperatorPreview | null>(null);
  const [reviewSnapshot, setReviewSnapshot] =
    useState<VpsRulesReviewSnapshot | null>(null);
  const [reviewPromptOpen, setReviewPromptOpen] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const [applyPending, setApplyPending] = useState(false);
  const [prefillPending, setPrefillPending] = useState(false);
  const [prefillFeedback, setPrefillFeedback] = useState<{
    message: string;
    tone: ActionFeedbackTone;
  } | null>(null);
  const pending = reviewPending || applyPending || prefillPending;
  const [status, setStatus] = useState<string | null>(null);
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");
  const statusFeedbackRef = useRef<HTMLDivElement | null>(null);
  const previousStatusFeedbackRef = useRef<string | null>(null);
  const selectorDraftGenerationRef = useRef(0);
  const ruleDraftTouchedRef = useRef(false);
  const preserveStatusOnNextDraftInvalidationRef = useRef(false);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();

  useEffect(() => {
    const terminalStatus =
      !preview && status && statusTone !== "info" && statusTone !== "progress"
        ? `${statusTone}:${status}`
        : null;
    if (!terminalStatus) {
      if (!status) {
        previousStatusFeedbackRef.current = null;
      }
      return;
    }
    if (previousStatusFeedbackRef.current === terminalStatus) {
      return;
    }
    previousStatusFeedbackRef.current = terminalStatus;
    const frame = window.requestAnimationFrame(() => {
      if (statusFeedbackRef.current) {
        scrollIntoViewWithMotion(statusFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [preview, status, statusTone]);
  const parsedSelector = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const selectorEvidenceUnavailable = vpsRuleSearchUnavailable(
    selectorExpression,
    vpsRuleSearch,
  );
  const localSelectorTargets = useMemo(
    () =>
      selectorExpression.trim() &&
      !parsedSelector.error &&
      !selectorEvidenceUnavailable
        ? agentsMatchingExpression(agents, selectorExpression, vpsRuleSearch)
        : [],
    [
      agents,
      parsedSelector.error,
      selectorEvidenceUnavailable,
      selectorExpression,
      vpsRuleSearch,
    ],
  );
  const localSelectorTargetIds = useMemo(
    () =>
      localSelectorTargets
        .map((agent) => agent.id)
        .sort((left, right) => left.localeCompare(right)),
    [localSelectorTargets],
  );
  const localSelectorResolutionKey = `${selectorExpression}\0${localSelectorTargetIds.join("\0")}`;
  const localSelectorResolutionKeyRef = useRef(localSelectorResolutionKey);
  localSelectorResolutionKeyRef.current = localSelectorResolutionKey;
  const singleResolvedClientId =
    localSelectorTargetIds.length === 1 ? localSelectorTargetIds[0] : null;
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
  const accountingByClient = useMemo(
    () => new Map(trafficAccounting.map((row) => [row.client_id, row])),
    [trafficAccounting],
  );
  const filteredRules = useMemo(
    () =>
      vpsRuleValues
        .filter((row) => {
          if (keyFilter && row.key !== keyFilter) {
            return false;
          }
          if (stateFilter && row.state !== stateFilter) {
            return false;
          }
          if (showIncompleteOnly && row.state === "ok") {
            return false;
          }
          return true;
        })
        .slice()
        .sort(
          (left, right) =>
            (agentNameById.get(left.client_id) ?? left.client_id).localeCompare(
              agentNameById.get(right.client_id) ?? right.client_id,
            ) || left.key.localeCompare(right.key),
        ),
    [agentNameById, keyFilter, showIncompleteOnly, stateFilter, vpsRuleValues],
  );
  const incompleteClients = new Set(
    trafficAccounting
      .filter(
        (row) =>
          row.state === "incomplete" || row.incomplete_reasons.length > 0,
      )
      .map((row) => row.client_id),
  );
  const editedRuleKeys = useMemo(
    () =>
      editMode === "upsert"
        ? vpsRuleEditKeys(valuesText, [])
        : vpsRuleEditKeys("", unsetKeys),
    [editMode, unsetKeys, valuesText],
  );
  const typedRuleValues = useMemo(
    () => parseVpsRuleTextValues(valuesText),
    [valuesText],
  );
  const affectedPolicyRules = useMemo(
    () => affectedAlertPolicyRules(fleetAlertPolicies, editedRuleKeys),
    [editedRuleKeys, fleetAlertPolicies],
  );
  const columns = useMemo<ConsoleDataGridColumn<VpsRuleValueRecord>[]>(
    () => [
      {
        id: "vps",
        header: "VPS",
        size: 220,
        minSize: 160,
        searchValue: (row) =>
          `${row.client_id} ${agentNameById.get(row.client_id) ?? ""}`,
        sortValue: (row) => agentNameById.get(row.client_id) ?? row.client_id,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{agentNameById.get(row.client_id) ?? row.client_id}</strong>
            <small className="monoValue">{row.client_id}</small>
          </span>
        ),
      },
      {
        id: "key",
        header: "Key",
        size: 190,
        minSize: 150,
        searchValue: (row) => row.key,
        sortValue: (row) => row.key,
        cell: (row) => <span className="monoValue">{row.key}</span>,
      },
      {
        id: "value",
        header: "Value",
        size: 190,
        minSize: 130,
        searchValue: (row) => row.value_raw,
        cell: (row) => row.value_raw,
      },
      {
        id: "parsed",
        header: "Parsed",
        size: 220,
        minSize: 150,
        searchValue: (row) => row.parsed_display,
        cell: (row) => row.parsed_display,
      },
      {
        id: "state",
        header: "State",
        size: 110,
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
        id: "source",
        header: "Source",
        size: 130,
        minSize: 100,
        searchValue: (row) => `${row.source_kind} ${row.source_id ?? ""}`,
        cell: (row) => row.source_kind,
      },
      {
        id: "updated_by",
        header: "Updated by",
        size: 145,
        minSize: 110,
        searchValue: (row) => row.updated_by ?? "",
        cell: (row) => row.updated_by ?? "unknown",
      },
      {
        id: "updated",
        header: "Updated",
        size: 155,
        minSize: 120,
        sortValue: (row) => row.updated_at,
        cell: (row) => formatTime(row.updated_at),
      },
    ],
    [agentNameById],
  );
  const previewColumns = useMemo<ConsoleDataGridColumn<VpsRuleChangePreview>[]>(
    () => [
      {
        id: "vps",
        header: "VPS",
        size: 220,
        minSize: 150,
        searchValue: (row) => `${row.client_id} ${row.display_name ?? ""}`,
        sortValue: (row) => row.display_name ?? row.client_id,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.display_name || row.client_id}</strong>
            <small className="monoValue">{row.client_id}</small>
          </span>
        ),
      },
      {
        id: "key",
        header: "Key",
        size: 210,
        minSize: 150,
        searchValue: (row) => row.key,
        sortValue: (row) => row.key,
        cell: (row) => <span className="monoValue">{row.key}</span>,
      },
      {
        id: "before",
        header: "Before",
        size: 170,
        minSize: 120,
        searchValue: (row) => row.before ?? "",
        cell: (row) => row.before ?? "unset",
      },
      {
        id: "after",
        header: "After",
        size: 170,
        minSize: 120,
        searchValue: (row) => row.after ?? "",
        cell: (row) => row.after ?? "unset",
      },
      {
        id: "action",
        header: "Action",
        size: 105,
        minSize: 90,
        searchValue: (row) => row.action,
        sortValue: (row) => row.action,
        cell: (row) => row.action,
      },
      {
        id: "validation",
        header: "Validation",
        size: 280,
        minSize: 180,
        searchValue: (row) =>
          `${row.validation} ${row.validation_errors.join(" ")}`,
        sortValue: (row) => row.validation,
        cell: (row) => {
          const messages = vpsRuleValidationMessages(row);
          return (
            <span className="vpsRuleValidation">
              <ConsoleStatusBadge
                tone={isValidVpsRuleChange(row) ? "ok" : "warning"}
              >
                {isValidVpsRuleChange(row) ? "valid" : "invalid"}
              </ConsoleStatusBadge>
              {messages.length > 0 ? (
                <small title={messages.join(" ")}>{messages.join(" ")}</small>
              ) : null}
            </span>
          );
        },
      },
    ],
    [],
  );
  const matchedPreviewClients = useMemo(
    () =>
      Array.from(
        new Map(
          (preview?.changes ?? []).map((change) => [
            change.client_id,
            change.display_name || change.client_id,
          ]),
        ).entries(),
      ),
    [preview],
  );

  const changeSelectorExpression = useCallback((nextExpression: string) => {
    if (nextExpression === selectorExpressionRef.current) {
      return;
    }
    selectorExpressionRef.current = nextExpression;
    selectorDraftGenerationRef.current += 1;
    ruleDraftTouchedRef.current = false;
    setValuesText("");
    setUnsetKeys([]);
    setPrefillPending(false);
    setPrefillFeedback(null);
    setSelectorExpression(nextExpression);
  }, []);

  useEffect(() => {
    if (initialSelectorExpression) {
      changeSelectorExpression(initialSelectorExpression);
    }
  }, [changeSelectorExpression, initialSelectorExpression]);

  useEffect(() => {
    selectorDraftGenerationRef.current += 1;
    const generation = selectorDraftGenerationRef.current;
    ruleDraftTouchedRef.current = false;
    setValuesText("");
    setUnsetKeys([]);
    setPrefillFeedback(null);
    if (!singleResolvedClientId) {
      setPrefillPending(false);
      return;
    }

    let active = true;
    setPrefillPending(true);
    setPrefillFeedback({
      message: `Loading existing VPS rules for ${singleResolvedClientId}`,
      tone: "progress",
    });
    void onLoadEffectiveVpsRules(singleResolvedClientId)
      .then((rows) => {
        if (
          !active ||
          generation !== selectorDraftGenerationRef.current ||
          ruleDraftTouchedRef.current
        ) {
          return;
        }
        const values = Object.fromEntries(
          rows.map((row) => [row.key, row.value_raw]),
        );
        setValuesText(serializeVpsRuleTextValues(values));
        setPrefillPending(false);
        setPrefillFeedback({
          message:
            rows.length > 0
              ? `Loaded ${rows.length} existing ${rows.length === 1 ? "rule" : "rules"} for ${singleResolvedClientId}`
              : `No existing VPS rules for ${singleResolvedClientId}; fields remain blank`,
          tone: "info",
        });
      })
      .catch((error) => {
        if (
          !active ||
          generation !== selectorDraftGenerationRef.current ||
          ruleDraftTouchedRef.current
        ) {
          return;
        }
        setPrefillPending(false);
        setPrefillFeedback({
          message:
            error instanceof Error
              ? `Could not load existing VPS rules: ${error.message}`
              : "Could not load existing VPS rules",
          tone: "danger",
        });
      });
    return () => {
      active = false;
    };
  }, [
    localSelectorResolutionKey,
    onLoadEffectiveVpsRules,
    singleResolvedClientId,
  ]);

  useEffect(() => {
    writeLocalString(CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  useEffect(() => {
    const preserveStatus = preserveStatusOnNextDraftInvalidationRef.current;
    preserveStatusOnNextDraftInvalidationRef.current = false;
    invalidateReviewGeneration();
    setPreview(null);
    setReviewSnapshot(null);
    setReviewPromptOpen(false);
    setReviewPending(false);
    if (!preserveStatus) {
      setStatus(null);
    }
  }, [
    editMode,
    invalidateReviewGeneration,
    selectorExpression,
    unsetKeys,
    valuesText,
  ]);

  function parseSetValues(): Record<string, string> {
    const values: Record<string, string> = {};
    for (const rawLine of valuesText.split(/\r?\n/)) {
      const line = rawLine.trim();
      if (!line) {
        continue;
      }
      const equals = line.indexOf("=");
      if (equals <= 0) {
        throw new Error("VPS rule set values must use key=value lines");
      }
      const key = line.slice(0, equals).trim();
      const value = line.slice(equals + 1).trim();
      if (!VPS_RULE_KEYS.includes(key as (typeof VPS_RULE_KEYS)[number])) {
        throw new Error(`Unsupported VPS rule key: ${key}`);
      }
      if (!value) {
        throw new Error(`VPS rule ${key} cannot be empty; use explicit unset`);
      }
      if (Object.prototype.hasOwnProperty.call(values, key)) {
        throw new Error(`Duplicate VPS rule key: ${key}`);
      }
      const canonicalValue = tryNormalizeVpsRuleValue(key, value);
      if (canonicalValue === null) {
        throw new Error(
          `VPS rule ${key} has an invalid value: ${value || "(empty)"}`,
        );
      }
      values[key] = canonicalValue;
    }
    if (Object.keys(values).length === 0) {
      throw new Error("Add at least one VPS rule value to set");
    }
    return values;
  }

  function setRuleStatus(message: string, tone: ActionFeedbackTone) {
    setStatus(message);
    setStatusTone(tone);
  }

  async function dryRun(operation: "upsert" | "unset") {
    const reviewGeneration = captureReviewGeneration();
    setReviewPending(true);
    setReviewPromptOpen(false);
    setRuleStatus(
      operation === "upsert"
        ? "dry-running set values"
        : "dry-running unset values",
      "progress",
    );
    try {
      const values = operation === "upsert" ? parseSetValues() : {};
      const keys = operation === "unset" ? unsetKeys : [];
      if (operation === "unset" && keys.length === 0) {
        throw new Error("Select at least one VPS rule key to unset");
      }
      const rawPreview = await onDryRun({
        operation,
        selector_expression: selectorExpression.trim(),
        values,
        keys,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      const nextPreview = buildOperatorVpsRulesPreview(rawPreview);
      setPreview(nextPreview);
      setReviewSnapshot(
        nextPreview.changed_row_count > 0 && nextPreview.invalid_row_count === 0
          ? {
              operation,
              selectorExpression: selectorExpression.trim(),
              values,
              keys,
              preview: nextPreview,
            }
          : null,
      );
      setRuleStatus(
        nextPreview.invalid_row_count > 0
          ? `${nextPreview.invalid_row_count} invalid ${nextPreview.invalid_row_count === 1 ? "row needs" : "rows need"} correction; review the listed reason${nextPreview.invalid_row_count === 1 ? "" : "s"} and preview again`
          : nextPreview.changed_row_count === 0
            ? `No changes detected across ${nextPreview.matched_vps_count} matched VPSs`
            : `${operation === "upsert" ? "set" : "unset"} preview found ${nextPreview.changed_row_count} changes across ${nextPreview.matched_vps_count} matched VPSs`,
        nextPreview.invalid_row_count > 0
          ? "danger"
          : nextPreview.changed_row_count === 0
            ? "warning"
            : "success",
      );
    } catch (error) {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setRuleStatus(
          error instanceof Error ? error.message : "VPS rules dry-run failed",
          "danger",
        );
        setReviewSnapshot(null);
      }
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewPending(false);
      }
    }
  }

  async function applyReview() {
    const snapshot = reviewSnapshot;
    if (!snapshot) {
      setRuleStatus("Run dry-run before applying VPS rules", "warning");
      return;
    }
    if (snapshot.preview.changed_row_count === 0) {
      setRuleStatus("No changes detected; Apply is disabled.", "warning");
      setReviewSnapshot(null);
      return;
    }
    if (snapshot.preview.invalid_row_count > 0) {
      setRuleStatus(
        "Correct every invalid VPS rule row and run Preview changes again before applying.",
        "danger",
      );
      setReviewSnapshot(null);
      return;
    }
    const reviewedLocalResolutionKey = localSelectorResolutionKey;
    const reviewedSelectorGeneration = selectorDraftGenerationRef.current;
    const reviewedSingleClientId = singleResolvedClientId;
    setApplyPending(true);
    setRuleStatus("applying VPS rule changes", "progress");
    try {
      const rawPreview =
        snapshot.operation === "upsert"
          ? await onBulkUpsert({
              selector_expression: snapshot.selectorExpression,
              values: snapshot.values,
              confirmed: true,
              preview_hash: snapshot.preview.preview_hash,
            })
          : await onBulkUnset({
              selector_expression: snapshot.selectorExpression,
              keys: snapshot.keys,
              confirmed: true,
              preview_hash: snapshot.preview.preview_hash,
            });
      const nextPreview = buildOperatorVpsRulesPreview(rawPreview);
      setPreview(null);
      setReviewSnapshot(null);
      setRuleStatus(
        `applied ${nextPreview.changed_row_count} VPS rule changes`,
        "success",
      );
      setReviewPromptOpen(false);
      if (
        reviewedSingleClientId &&
        selectorDraftGenerationRef.current === reviewedSelectorGeneration &&
        localSelectorResolutionKeyRef.current === reviewedLocalResolutionKey
      ) {
        const refreshedValues = parseVpsRuleTextValues(valuesText);
        if (snapshot.operation === "upsert") {
          Object.assign(refreshedValues, snapshot.values);
        } else {
          for (const key of snapshot.keys) {
            delete refreshedValues[key];
          }
        }
        const nextValuesText = serializeVpsRuleTextValues(refreshedValues);
        if (
          nextValuesText !== valuesText ||
          (snapshot.operation === "unset" && unsetKeys.length > 0)
        ) {
          preserveStatusOnNextDraftInvalidationRef.current = true;
        }
        if (snapshot.operation === "unset") {
          setUnsetKeys([]);
        }
        setValuesText(nextValuesText);
        const refreshedCount = Object.keys(refreshedValues).length;
        setPrefillFeedback({
          message:
            refreshedCount > 0
              ? `Loaded ${refreshedCount} existing ${refreshedCount === 1 ? "rule" : "rules"} for ${reviewedSingleClientId}`
              : `No existing VPS rules for ${reviewedSingleClientId}; fields remain blank`,
          tone: "info",
        });
      }
    } catch (error) {
      setRuleStatus(
        error instanceof Error ? error.message : "VPS rules apply failed",
        "danger",
      );
    } finally {
      setApplyPending(false);
    }
  }

  function toggleUnsetKey(key: string, checked: boolean) {
    setUnsetKeys((current) =>
      checked
        ? Array.from(new Set([...current, key]))
        : current.filter((stored) => stored !== key),
    );
  }
  const vpsRulesApplyReviewPrompt = (
    <ConfirmationPrompt
      confirmLabel={
        reviewSnapshot
          ? `Apply ${reviewSnapshot.preview.changed_row_count} ${reviewSnapshot.preview.changed_row_count === 1 ? "change" : "changes"}`
          : "Apply changes"
      }
      detail="Applies the reviewed preview bound to its selector and server-issued preview hash."
      error={statusTone === "danger" ? status : null}
      items={[
        {
          label: "Selector",
          value: reviewSnapshot?.selectorExpression ?? "-",
          title:
            reviewSnapshot?.selectorExpression ??
            "No frozen VPS selector is available because the rule review is not open",
        },
        {
          label: "Operation",
          value: reviewSnapshot?.operation ?? "-",
          title:
            reviewSnapshot?.operation ??
            "No rule operation is available because the review is not open",
        },
        {
          label: "Set keys",
          value: Object.keys(reviewSnapshot?.values ?? {}).join(", ") || "-",
          title:
            Object.keys(reviewSnapshot?.values ?? {}).join(", ") ||
            "This reviewed operation does not set any VPS rule keys",
        },
        {
          label: "Unset keys",
          value: reviewSnapshot?.keys.join(", ") || "-",
          title:
            reviewSnapshot?.keys.join(", ") ||
            "This reviewed operation does not unset any VPS rule keys",
        },
        {
          label: "Matched VPS",
          value: reviewSnapshot?.preview.matched_vps_count ?? 0,
        },
        {
          label: "Changed rows",
          value: reviewSnapshot?.preview.changed_row_count ?? 0,
        },
        {
          label: "No-op rows hidden",
          value: reviewSnapshot?.preview.no_op_row_count ?? 0,
        },
      ]}
      onCancel={() => setReviewPromptOpen(false)}
      onConfirm={() => void applyReview()}
      open={reviewPromptOpen && reviewSnapshot !== null}
      pending={pending}
      title="Confirm VPS rule write"
    />
  );

  return (
    <div className="consoleCrudPanel vpsRulesWorkspace">
      <div
        aria-label="VPS rule registry filters"
        className="consoleFilterBar vpsRulesRegistryFilters"
      >
        <label>
          <span>Key filter</span>
          <select
            value={keyFilter}
            onChange={(event) => setKeyFilter(event.target.value)}
          >
            <option value="">all keys</option>
            {VPS_RULE_KEYS.map((key) => (
              <option key={key} value={key}>
                {key}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>State filter</span>
          <select
            value={stateFilter}
            onChange={(event) => setStateFilter(event.target.value)}
          >
            <option value="">all states</option>
            <option value="ok">ok</option>
            <option value="invalid">invalid</option>
            <option value="incomplete">incomplete</option>
          </select>
        </label>
        <label className="checkLine inlineCheck">
          <input
            checked={showIncompleteOnly}
            onChange={(event) => setShowIncompleteOnly(event.target.checked)}
            type="checkbox"
          />
          <span>Show incomplete only</span>
        </label>
      </div>
      <div className="consoleInlineDetailGrid vpsRulesSummary">
        <span>
          <strong>Rule rows</strong>
          <span>{vpsRuleValues.length}</span>
        </span>
        <span>
          <strong>Accounting records</strong>
          <span>{trafficAccounting.length}</span>
        </span>
        <span>
          <strong>Incomplete VPS</strong>
          <span>{incompleteClients.size}</span>
        </span>
      </div>
      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={20}
        empty="No VPS rule rows loaded."
        getRowId={(row) => `${row.client_id}:${row.key}`}
        itemLabel="rules"
        pageResetKey={JSON.stringify([
          keyFilter,
          stateFilter,
          showIncompleteOnly,
        ])}
        renderExpandedRow={(row) => (
          <div className="consoleInlineDetailGrid">
            <span>
              <strong>Client ID</strong>
              <span className="monoValue">{row.client_id}</span>
            </span>
            <span>
              <strong>Raw value</strong>
              <span>{row.value_raw}</span>
            </span>
            <span>
              <strong>Parsed JSON</strong>
              <span className="monoValue">{jsonSummary(row.value_json)}</span>
            </span>
            <span>
              <strong>Validation</strong>
              <span>{row.validation_errors.join(", ") || "ok"}</span>
            </span>
            <span>
              <strong>Accounting state</strong>
              <span>
                {accountingByClient.get(row.client_id)?.state ?? "unknown"}
              </span>
            </span>
            <span>
              <strong>Last write request ID</strong>
              <span className="monoValue">{row.source_id ?? "unknown"}</span>
            </span>
          </div>
        )}
        rows={filteredRules}
        searchPlaceholder="Search VPS rules by VPS, key, value, or source"
        selectable={false}
        storageKey="vpsman.grid.config.vpsRules"
        title="VPS rule values"
      />
      <div className="vpsRulesMutationScope">
        <section className="consoleDetailPanel">
          <div className="consoleDetailPanelHeader">
            <span>
              <ConfigHelpLabel
                help={CONFIG_HELP.vpsRules}
                label="Bulk rule editor"
                strong
              />
              <small>
                Dry-run matched VPSs and changed keys before applying.
              </small>
            </span>
            <div className="consoleOperationsActions">
              <div
                aria-label="VPS rule edit mode"
                className="segmented vpsRulesModeSwitch"
                role="group"
              >
                <button
                  aria-pressed={editMode === "upsert"}
                  className={editMode === "upsert" ? "selected" : ""}
                  disabled={applyPending}
                  onClick={() => {
                    setEditMode("upsert");
                  }}
                  title={
                    applyPending
                      ? "Wait for the current VPS rule operation to finish"
                      : "Set typed rule values on the reviewed VPS scope"
                  }
                  type="button"
                >
                  Set values
                </button>
                <button
                  aria-pressed={editMode === "unset"}
                  className={editMode === "unset" ? "selected" : ""}
                  disabled={applyPending}
                  onClick={() => {
                    setEditMode("unset");
                  }}
                  title={
                    applyPending
                      ? "Wait for the current VPS rule operation to finish"
                      : "Remove selected rule keys from the reviewed VPS scope"
                  }
                  type="button"
                >
                  Unset values
                </button>
              </div>
            </div>
          </div>
          {!preview ? (
            <ActionFeedback
              className="localActionFeedback vpsRulesActionFeedback"
              message={status}
              ref={statusFeedbackRef}
              tone={statusTone}
            />
          ) : null}
          <div className="vpsRulesBulkEditor">
            <section
              className="vpsRulesEditorSection vpsRulesModeLegend"
              aria-label="VPS rule edit mode semantics"
            >
              <div>
                <strong>Set values</strong>
                <span title={CONFIG_HELP.ruleSetValues}>
                  Key=value lines become typed rule updates after dry-run.
                </span>
              </div>
              <div>
                <h4 title={CONFIG_HELP.ruleUnsetValues}>Unset values</h4>
                <span title={CONFIG_HELP.ruleUnsetValues}>
                  Explicit rule keys are removed only after preview review.
                </span>
              </div>
            </section>
            <section className="vpsRulesEditorSection vpsRulesTargetSection">
              <div className="sectionHeader compactHeader">
                <div>
                  <h4 title={CONFIG_HELP.ruleSelector}>Target VPS selector</h4>
                  <span>Choose the VPS scope for the reviewed mutation.</span>
                </div>
                <div className="consoleOperationsActions">
                  <button
                    className="secondaryAction compactAction"
                    disabled={applyPending}
                    onClick={() => changeSelectorExpression("")}
                    title={
                      applyPending
                        ? "Wait for the current VPS rule operation to finish"
                        : selectorExpression.trim()
                          ? "Clear the VPS rule selector and its prefilled field values"
                          : "The VPS rule selector is already empty"
                    }
                    type="button"
                  >
                    Clear
                  </button>
                </div>
              </div>
              <label
                className="consoleField"
                title={
                  applyPending
                    ? "VPS selector editing is disabled while a rule operation is pending"
                    : "Select the exact VPS scope for the reviewed rule mutation"
                }
              >
                <span>VPS selector expression</span>
                <SearchExpressionInput
                  agents={agents}
                  ariaLabel="VPS rules selector expression"
                  disabled={applyPending}
                  onChange={changeSelectorExpression}
                  placeholder="provider:hetzner && tag:edge"
                  showMatchCount
                  value={selectorExpression}
                  verification={
                    parsedSelector.error
                      ? "invalid"
                      : selectorExpression.trim()
                        ? "valid"
                        : "neutral"
                  }
                />
              </label>
              {!selectorEvidenceUnavailable ? (
                <LocalTargetPreview
                  agents={localSelectorTargets}
                  ariaLabel="Local VPS rule match preview"
                />
              ) : null}
              <small className="vpsRulesTargetHint">
                {selectorEvidenceUnavailable
                  ? `${VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE}. Preview changes can request an authoritative resolution.`
                  : "Local match only. Preview changes resolves and binds the authoritative VPS list."}
              </small>
              <ActionFeedback
                className="localActionFeedback vpsRulesActionFeedback"
                message={prefillFeedback?.message}
                tone={prefillFeedback?.tone}
              />
              {preview ? (
                <div
                  className="tokenPreview"
                  aria-label="Reviewed VPS rule targets"
                >
                  {matchedPreviewClients.length === 0 ? (
                    <span className="tokenChip">
                      {`${preview.matched_vps_count} matched · no effective changes`}
                    </span>
                  ) : (
                    matchedPreviewClients.map(([clientId, displayName]) => (
                      <span
                        className="tokenChip"
                        key={clientId}
                        title={clientId}
                      >
                        {displayName}
                      </span>
                    ))
                  )}
                </div>
              ) : null}
            </section>
            {editMode === "upsert" ? (
              <section className="vpsRulesEditorSection vpsRulesTypedEditor">
                <div className="sectionHeader compactHeader">
                  <div>
                    <h4 title={CONFIG_HELP.ruleSetValues}>Common rule cards</h4>
                    <span title={CONFIG_HELP.ruleSetValues}>
                      Typed fields for billing, live rate, quota, reset day, and
                      traffic interfaces
                    </span>
                  </div>
                </div>
                <div
                  className="vpsRuleTypedGrid"
                  aria-label="Common VPS rule fields"
                >
                  {VPS_RULE_FIELD_DEFINITIONS.map((field) => (
                    <label
                      className="vpsRuleTypedCard"
                      key={field.key}
                      title={
                        applyPending
                          ? `Editing ${field.label} is disabled while a VPS rule operation is pending`
                          : field.help
                      }
                    >
                      <span>
                        <ConfigHelpLabel
                          help={field.help}
                          label={field.label}
                          strong
                        />
                        <small className="monoValue">{field.key}</small>
                      </span>
                      <input
                        aria-label={field.label}
                        data-tooltip-disabled-reason={`Wait for the current VPS rule operation to finish before editing ${field.label}.`}
                        disabled={applyPending}
                        inputMode={field.inputMode ?? "text"}
                        onChange={(event) => {
                          const value = event.currentTarget.value;
                          ruleDraftTouchedRef.current = true;
                          setPrefillPending(false);
                          setPrefillFeedback(null);
                          setValuesText((current) =>
                            updateVpsRuleTextDraftValue(
                              current,
                              field.key,
                              value,
                            ),
                          );
                        }}
                        onBlur={(event) => {
                          const value = event.currentTarget.value;
                          setValuesText((current) =>
                            normalizeVpsRuleTextValueOnBlur(
                              current,
                              field.key,
                              value,
                            ),
                          );
                        }}
                        placeholder={field.placeholder}
                        value={typedRuleValues[field.key] ?? ""}
                      />
                    </label>
                  ))}
                </div>
                <details
                  className="vpsRulesAdvancedRaw"
                  title="Edit advanced VPS rule key/value lines not covered by the common typed cards"
                >
                  <summary>Advanced raw key/value</summary>
                  <textarea
                    aria-label="VPS rule set values"
                    data-tooltip-disabled-reason="Wait for the current VPS rule operation to finish before editing raw values."
                    disabled={applyPending}
                    value={valuesText}
                    onChange={(event) => {
                      ruleDraftTouchedRef.current = true;
                      setPrefillPending(false);
                      setPrefillFeedback(null);
                      setValuesText(event.target.value);
                    }}
                    onBlur={() => {
                      setValuesText((current) =>
                        normalizeVpsRuleTextOnBlur(current),
                      );
                    }}
                  />
                </details>
              </section>
            ) : (
              <section className="vpsRulesEditorSection">
                <div className="sectionHeader compactHeader">
                  <div>
                    <h4 title={CONFIG_HELP.ruleUnsetValues}>Unset values</h4>
                    <span title={CONFIG_HELP.ruleUnsetValues}>
                      Explicit key checklist
                    </span>
                  </div>
                </div>
                <div className="checkListPanel compactChecklist">
                  {VPS_RULE_KEYS.map((key) => (
                    <label
                      className="checkLine"
                      key={key}
                      title={
                        applyPending
                          ? `Unsetting ${key} is disabled while a VPS rule operation is pending`
                          : `Select ${key} for reviewed removal from matched VPSs`
                      }
                    >
                      <input
                        aria-label={`Unset ${key}`}
                        checked={unsetKeys.includes(key)}
                        data-tooltip-disabled-reason={`Wait for the current VPS rule operation to finish before selecting ${key}.`}
                        disabled={applyPending}
                        onChange={(event) =>
                          toggleUnsetKey(key, event.target.checked)
                        }
                        type="checkbox"
                      />
                      <span className="monoValue">{key}</span>
                    </label>
                  ))}
                </div>
              </section>
            )}
          </div>
          <div className="consoleFormActions vpsRulesPreviewActions">
            <button
              className="primaryAction compactAction"
              disabled={pending}
              onClick={() => void dryRun(editMode)}
              title={
                pending
                  ? "Wait for the current VPS rule operation to finish before preview."
                  : editMode === "upsert"
                    ? "Preview effective VPS rule value changes before applying them."
                    : "Preview effective VPS rule removals before applying them."
              }
              type="button"
            >
              Preview changes
            </button>
          </div>
          <section
            className="consoleDetailPanel vpsRulesAlertImpact"
            aria-label="Affected alert policy context"
          >
            <div className="consoleDetailPanelHeader">
              <span>
                <strong>Affected alert policies</strong>
                <small>
                  Policies whose rule conditions reference the current edit
                  keys:{" "}
                  <span className="monoValue">
                    {editedRuleKeys.join(", ") || "no edited keys"}
                  </span>
                </small>
              </span>
              <button
                className="secondaryAction compactAction"
                onClick={onOpenAlerts}
                type="button"
              >
                Open Observability alerts
              </button>
            </div>
            <div className="configRiskList">
              {affectedPolicyRules.slice(0, 6).map((impact) => (
                <div
                  className="configRiskRow"
                  key={`${impact.policyId}:${impact.ruleId}:${impact.phase}`}
                >
                  <span>
                    <strong>
                      {impact.policyName} / {impact.ruleName} / {impact.phase}
                    </strong>
                    <small className="monoValue">
                      {impact.conditionExpression}
                    </small>
                  </span>
                  <ConsoleStatusBadge
                    tone={impact.enabled ? "warning" : "neutral"}
                  >
                    {impact.severity}
                  </ConsoleStatusBadge>
                </div>
              ))}
              {affectedPolicyRules.length === 0 && (
                <div className="emptyState compactEmpty">
                  No loaded alert policy conditions reference the current rule
                  keys.
                </div>
              )}
            </div>
          </section>
          {preview ? (
            <VpsRulesPreviewTable
              columns={previewColumns}
              onRequestApply={() => setReviewPromptOpen(true)}
              pending={pending}
              preview={preview}
              reviewPrompt={vpsRulesApplyReviewPrompt}
              status={status}
              statusTone={statusTone}
            />
          ) : null}
        </section>
      </div>
    </div>
  );
}

function ConfigHelpLabel({
  help,
  label,
  strong = false,
}: {
  help: string;
  label: ReactNode;
  strong?: boolean;
}) {
  const accessibleLabel = typeof label === "string" ? label : "Field";
  const content = (
    <>
      <span>{label}</span>
      <span
        aria-label={`${accessibleLabel} help`}
        className="fieldHelpIcon"
        role="img"
        tabIndex={0}
        title={help}
      >
        ?
      </span>
    </>
  );

  return strong ? (
    <strong className="configHelpLabel" title={help}>
      {content}
    </strong>
  ) : (
    <span className="configHelpLabel" title={help}>
      {content}
    </span>
  );
}

function VpsRulesPreviewTable({
  columns,
  onRequestApply,
  pending,
  preview,
  reviewPrompt,
  status,
  statusTone,
}: {
  columns: ConsoleDataGridColumn<VpsRuleChangePreview>[];
  onRequestApply: () => void;
  pending: boolean;
  preview: VpsRulesOperatorPreview;
  reviewPrompt: ReactNode;
  status: string | null;
  statusTone: ActionFeedbackTone;
}) {
  const previewRef = useRef<HTMLDivElement | null>(null);
  const previousPreviewHashRef = useRef<string | null>(null);
  const previousPreviewOutcomeRef = useRef<string | null>(null);
  const finalActionLabel = `Apply ${preview.changed_row_count} ${preview.changed_row_count === 1 ? "change" : "changes"}`;
  const hasInvalidRows = preview.invalid_row_count > 0;
  const finalActionSummary = `${preview.changed_row_count} effective ${
    preview.changed_row_count === 1 ? "change" : "changes"
  } - ${preview.no_op_row_count} no-op${
    preview.no_op_row_count === 1 ? "" : "s"
  } hidden`;
  useEffect(() => {
    const isNewPreview =
      previousPreviewHashRef.current !== preview.preview_hash;
    previousPreviewHashRef.current = preview.preview_hash;
    const terminalOutcome =
      status && statusTone !== "info" && statusTone !== "progress"
        ? `${statusTone}:${status}`
        : null;
    if (!isNewPreview && !terminalOutcome) {
      return;
    }
    if (
      !isNewPreview &&
      previousPreviewOutcomeRef.current === terminalOutcome
    ) {
      return;
    }
    previousPreviewOutcomeRef.current = terminalOutcome;
    const frame = window.requestAnimationFrame(() => {
      const previewElement = previewRef.current;
      if (!previewElement) {
        return;
      }
      scrollIntoViewWithMotion(previewElement, {
        block: isNewPreview ? "start" : "nearest",
      });
      if (isNewPreview) {
        previewElement.focus({ preventScroll: true });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [preview.preview_hash, status, statusTone]);

  return (
    <div className="vpsRulesPreviewBlock" ref={previewRef} tabIndex={-1}>
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Matched VPS</strong>
          <span>{preview.matched_vps_count}</span>
        </span>
        <span>
          <strong>Effective changes</strong>
          <span>{preview.changed_row_count}</span>
        </span>
        <span>
          <strong>No-op rows hidden</strong>
          <span>{preview.no_op_row_count}</span>
        </span>
        <span>
          <strong>Invalid rows</strong>
          <span>{preview.invalid_row_count}</span>
        </span>
        <span>
          <strong>Preview details</strong>
          <details className="vpsRulesPreviewDetails">
            <summary>Preview binding</summary>
            <span className="monoValue" title={CONFIG_HELP.previewHash}>
              {preview.preview_hash}
            </span>
          </details>
        </span>
      </div>
      {preview.changed_row_count === 0 && !hasInvalidRows ? (
        <div className="emptyState compactEmpty">No changes detected</div>
      ) : null}
      <ActionFeedback
        className="localActionFeedback vpsRulesActionFeedback"
        message={status}
        tone={statusTone}
      />
      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={10}
        empty="No effective or invalid changes in preview."
        getRowId={(change) =>
          `${change.client_id}:${change.key}:${change.action}`
        }
        itemLabel="changes"
        pageResetKey={preview.preview_hash}
        rows={preview.changes}
        searchPlaceholder="Search dry-run changes"
        selectable={false}
        storageKey="vpsman.grid.config.vpsRules.preview"
        title="Preview changes"
      />
      <div
        className={
          preview.changed_row_count === 0 || hasInvalidRows
            ? "vpsRulesPreviewFinalAction neutral"
            : "vpsRulesPreviewFinalAction"
        }
        aria-label="VPS rules preview final action"
      >
        <span>
          <strong>
            {hasInvalidRows
              ? "Correct invalid values"
              : preview.changed_row_count === 0
                ? "No changes detected"
                : finalActionLabel}
          </strong>
          <small>
            {hasInvalidRows
              ? `${preview.invalid_row_count} invalid ${preview.invalid_row_count === 1 ? "row is" : "rows are"} listed above; no write is available.`
              : preview.changed_row_count === 0
                ? `${preview.matched_vps_count} matched VPSs; Apply is disabled.`
                : `${finalActionSummary}; final write uses this selector result for ${preview.matched_vps_count} matched VPSs.`}
          </small>
        </span>
        {preview.changed_row_count > 0 && !hasInvalidRows ? (
          <button
            className="primaryAction compactAction"
            disabled={pending}
            onClick={onRequestApply}
            title={
              pending
                ? "Wait for the current VPS rule operation to finish"
                : `${finalActionSummary}; open the final reviewed write confirmation`
            }
            type="button"
          >
            {finalActionLabel}
          </button>
        ) : null}
      </div>
      {reviewPrompt}
    </div>
  );
}

function jsonSummary(value: JsonValue): string {
  if (value === null) {
    return "null";
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function configTitle(subpage: string): string {
  switch (subpage) {
    case "bulk":
      return "VPS override patch";
    case "single":
      return "Per-VPS desired config";
    case "rules":
      return "VPS Rules";
    default:
      return "Runtime config overview";
  }
}

function configSubtitle(subpage: string): string {
  switch (subpage) {
    case "rules":
      return "Per-VPS traffic rule values used by traffic accounting and alert policies.";
    case "bulk":
      return "Advanced-only bulk editing with explicit deletion directives and per-VPS review";
    case "single":
      return "Edit inherited and overridden runtime values in one server-owned hierarchy";
    default:
      return "Runtime config workflows";
  }
}

function normalizeConfigSubpage(
  value: string,
): "overview" | "bulk" | "single" | "rules" {
  const base = value.split(":")[0];
  if (base === "per_vps") {
    return "single";
  }
  if (base === "bulk_patch") {
    return "bulk";
  }
  if (base === "bulk" || base === "single" || base === "rules") {
    return base;
  }
  return "overview";
}

function parseJsonObject(value: string): JsonValue {
  const parsed = JSON.parse(value) as JsonValue;
  if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("Values must be a JSON object");
  }
  return parsed;
}

function validatePatchGeneratorEditor(
  editor: PatchGeneratorEditorState,
): string | null {
  const requiredFields: Array<[string, string]> = [
    ["name", editor.name],
    ["category", editor.category],
    ["domain", editor.domain],
    ["description", editor.description],
    ["generator body", editor.rawGeneratorBody],
  ];
  const missing = requiredFields.find(([, value]) => !value.trim());
  if (missing) {
    return `Enter a ${missing[0]} before saving.`;
  }
  for (const [label, value] of [
    ["Field schema", editor.fieldSchemaText],
    ["Docs metadata", editor.docsMetadataText],
  ] as const) {
    try {
      parseJsonObject(value);
    } catch (error) {
      const reason = error instanceof Error ? error.message : "invalid JSON";
      return `${label} must be a JSON object: ${reason}.`;
    }
  }
  return null;
}

function inferTomlSections(toml: string): string[] {
  const sections = Array.from(
    new Set(
      toml
        .split(/\r?\n/)
        .map((line) => line.trim())
        .map(
          (line) =>
            /^-?\[([^[\]]+)\]$/.exec(line)?.[1]?.trim() ??
            /^-([A-Za-z0-9_-]+)(?:\.|$)/.exec(line)?.[1],
        )
        .filter((section): section is string => Boolean(section)),
    ),
  );
  return sections.length > 0 ? sections : ["root"];
}

function exampleValuesForGenerator(
  generator: RuntimeConfigPatchGeneratorRecord,
): Record<string, JsonValue> {
  const schema = asRecord(generator.field_schema) ?? {};
  const fields = asRecord(schema.fields) ?? asRecord(schema.properties) ?? {};
  const values: Record<string, JsonValue> = {};
  for (const [field, specValue] of Object.entries(fields)) {
    values[field] = exampleValueFromSchema(asRecord(specValue));
  }
  return values;
}

function exampleValueFromSchema(
  schema: Record<string, unknown> | null,
): JsonValue {
  if (!schema) {
    return "";
  }
  if (isJsonValue(schema.default)) {
    return schema.default;
  }
  if (
    Array.isArray(schema.enum) &&
    schema.enum.length > 0 &&
    isJsonValue(schema.enum[0])
  ) {
    return schema.enum[0];
  }
  const type = typeof schema.type === "string" ? schema.type : "string";
  if (type === "boolean") {
    return true;
  }
  if (type === "number" || type === "integer") {
    return typeof schema.minimum === "number" ? schema.minimum : 1;
  }
  if (type === "array") {
    return [];
  }
  return "";
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || Array.isArray(value) || typeof value !== "object") {
    return null;
  }
  return value as Record<string, unknown>;
}

function isJsonValue(value: unknown): value is JsonValue {
  if (
    value === null ||
    ["string", "number", "boolean"].includes(typeof value)
  ) {
    return true;
  }
  if (Array.isArray(value)) {
    return value.every(isJsonValue);
  }
  if (typeof value === "object") {
    return Object.values(value as Record<string, unknown>).every(isJsonValue);
  }
  return false;
}

function formatJsonObject(value: JsonValue): string {
  return JSON.stringify(asRecord(value) ?? {}, null, 2);
}

function runtimeConfigApplyStateSummary(
  state: RuntimeConfigApplyStateRecord | null,
  shortenIdentifiers = true,
): string {
  if (!state) {
    return "No server-applied runtime sync recorded";
  }
  if (state.pending_status === "failed") {
    const job = state.pending_job_id
      ? ` job ${shortenIdentifiers ? shortId(state.pending_job_id) : state.pending_job_id}`
      : "";
    const error = state.pending_error ? `: ${state.pending_error}` : "";
    return `Runtime sync failed${job}${error}`;
  }
  if (state.pending_status === "queued") {
    const job = state.pending_job_id
      ? ` job ${shortenIdentifiers ? shortId(state.pending_job_id) : state.pending_job_id}`
      : "";
    if (runtimeConfigQueuedStateIsStale(state)) {
      const queuedAt = configApplyStateTime(state);
      return queuedAt
        ? `Runtime sync stale${job}; queued since ${formatTime(queuedAt)}`
        : `Runtime sync stale${job}; queued timestamp missing`;
    }
    return `Runtime sync pending${job}`;
  }
  if (state.applied_content_hash) {
    const job = state.applied_job_id
      ? ` job ${shortenIdentifiers ? shortId(state.applied_job_id) : state.applied_job_id}`
      : "";
    const when = state.applied_at ? ` ${formatTime(state.applied_at)}` : "";
    const hash = shortenIdentifiers
      ? shortId(state.applied_content_hash)
      : state.applied_content_hash;
    return `Runtime config applied${job}${when}; hash ${hash}`;
  }
  return "No server-applied runtime sync recorded";
}

function readLocalString(key: string): string {
  try {
    return window.localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function writeLocalString(key: string, value: string) {
  try {
    if (value.trim()) {
      window.localStorage.setItem(key, value);
    } else {
      window.localStorage.removeItem(key);
    }
  } catch {
    // Browser-local selector persistence must not block config workflows.
  }
}
