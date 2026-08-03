import {
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
} from "../searchExpression";
import {
  clampJobMaxTimeoutSecs,
  clampInteger,
  DEFAULT_MAX_JOB_TIMEOUT_SECS,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "./jobDispatchModel";
import { LocalTargetPreview } from "./TargetImpactPreview";
import type {
  AgentView,
  BulkResolveResponse,
  RuntimeConfigPatchRequest,
  RuntimeConfigPatchResponse,
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
  JsonValue,
  PrivilegeAssertion,
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

const CONFIG_BULK_SELECTOR_STORAGE_KEY =
  "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY =
  "vpsman.config.single.selectorExpression";
const CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY = "vpsman.config.single.clientId";
const CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY =
  "vpsman.config.vpsRules.selectorExpression";
const CONFIG_HELP = {
  incrementalPatch:
    "Incremental TOML patches modify only reviewed runtime keys; bootstrap and server-managed keys stay immutable.",
  patchGenerator:
    "Saved generators render incremental TOML from reviewed JSON variables before any VPS target is touched.",
  targetSelector:
    "Selector expressions freeze the exact VPS set for preview and review so later fleet changes cannot silently expand scope.",
  maxTimeout:
    "Per-target command timeout enforced by the control plane so slow agents cannot hold config work indefinitely.",
  redactedRuntimeToml:
    "Runtime config returned by the agent with secret material removed; the base hash is used to detect stale overrides.",
  guardedOverride:
    "One-VPS override requires a current base hash, validated TOML sections, payload hash, and privilege assertion before apply.",
  currentBase:
    "Hash of the redacted config read used to prove the override was reviewed against the current runtime state.",
  sections:
    "Top-level TOML sections touched by the override; validate before review so the operator sees the blast radius.",
  payload:
    "Hash of the exact override payload that the confirmation prompt will bind to the privileged request.",
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
const VPS_RULE_KEYS = [
  "billing.price",
  "billing.cycle",
  "network.port_speed",
  "network.rate.interfaces",
  "traffic.reset_day",
  "traffic.quota.total",
  "traffic.quota.rx",
  "traffic.quota.tx",
  "traffic.selectors",
] as const;
const NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX = "[traffic.selectors]";
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

type SingleVpsConfigApplySnapshot = {
  clientId: string;
  selectorExpression: string;
  target: AgentView;
  toml: string;
  baseHash: string;
  patchSections: string[];
  maxTimeoutSecs: number;
  privilegeAssertion: PrivilegeAssertion;
  payloadHashHex: string;
};

type EvidenceState = "available" | "loading" | "unavailable";

export function ConfigPanel({
  activeSubpage,
  agents,
  trafficAccounting,
  vpsRuleValues,
  configurationPresets,
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
  onSubmitRuntimeConfigPatch,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onLoadConfigurationSources,
  onDeleteRuntimeConfigPatchGenerator,
  onOpenJobDetails,
  onOpenJobHistory,
  onOpenPrivilegeUnlock,
  onOpenAlerts,
  onRefresh,
  onBulkUnsetVpsRules,
  onBulkUpsertVpsRules,
  onDryRunVpsRules,
  onRenderRuntimeConfigPatchGenerator,
  onResolveBulk,
  onSelectSubpage,
  onUpsertRuntimeConfigPatchGenerator,
  privilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  trafficAccounting: TrafficAccountingRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
  configurationPresets: ConfigurationPresetRecord[];
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
  onSubmitRuntimeConfigPatch: (
    request: RuntimeConfigPatchRequest,
  ) => Promise<RuntimeConfigPatchResponse>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onLoadConfigurationSources: () => Promise<void>;
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobHistory: () => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenAlerts: () => void;
  onRefresh: () => void;
  onBulkUnsetVpsRules: (
    request: VpsRulesBulkUnsetRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsertVpsRules: (
    request: VpsRulesBulkUpsertRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onDryRunVpsRules: (
    request: VpsRulesDryRunRequest,
  ) => Promise<VpsRulesDryRunResponse>;
  onRenderRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: { values: JsonValue },
  ) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
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
        onLoadConfigurationSources,
      );
    }
  }, [onLoadConfigurationSources, subpage]);

  return (
    <section className="workspace singleColumn configWorkspace">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>{configTitle(subpage)}</h2>
            <span>{configSubtitle(subpage)}</span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction"
              disabled={loading || pending}
              onClick={onRefresh}
              type="button"
            >
              <RefreshCw size={15} />
              <span>Refresh</span>
            </button>
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
            onSubmitRuntimeConfigPatch={onSubmitRuntimeConfigPatch}
            onDeleteRuntimeConfigPatchGenerator={
              onDeleteRuntimeConfigPatchGenerator
            }
            onCreateJob={onCreateJob}
            onLoadJobOutputs={onLoadJobOutputs}
            onLoadJobTargets={onLoadJobTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenJobHistory={onOpenJobHistory}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onRenderRuntimeConfigPatchGenerator={
              onRenderRuntimeConfigPatchGenerator
            }
            onResolveBulk={onResolveBulk}
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
          <SingleVpsConfig
            actionError={actionError}
            agents={agents}
            runtimeConfigApplyStates={runtimeConfigApplyStates}
            runtimeConfigEvidenceState={runtimeConfigEvidenceState}
            onCreateJob={onCreateJob}
            onLoadJobOutputs={onLoadJobOutputs}
            onLoadJobTargets={onLoadJobTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onSubmitRuntimeConfigPatch={onSubmitRuntimeConfigPatch}
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
  const configurationSourcesEvidenceAvailable =
    configurationSourcesEvidenceState === "available";
  const currentStateEvidenceAvailable =
    runtimeEvidenceAvailable && fleetConfigEvidenceAvailable;
  const completeSummaryEvidence =
    currentStateEvidenceAvailable &&
    inventoryEvidenceAvailable &&
    configurationSourcesEvidenceAvailable;
  const evidenceLoading =
    runtimeConfigEvidenceState === "loading" ||
    inventoryEvidenceState === "loading" ||
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
      detail: "Read one VPS redacted config and inspect apply-state evidence.",
      subpage: "per_vps",
      title: "Per-VPS",
    },
    {
      action: "Open Bulk patch",
      detail:
        "Resolve target scope, render a patch, unlock privilege, and review apply.",
      subpage: "bulk_patch",
      title: "Bulk patch",
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
                !configurationSourcesEvidenceAvailable ||
                !fleetConfigEvidenceAvailable ||
                missingSourceEvidence
                  ? "warning"
                  : "ok"
              }
            >
              {configurationSourcesEvidenceAvailable &&
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
                {configurationSourcesEvidenceAvailable
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
                <span role="cell" title={change.target}>
                  {change.target}
                </span>
                <span role="cell" title={change.operation}>
                  {change.operation}
                </span>
                <span role="cell" title={change.status}>
                  <ConsoleStatusBadge tone={change.tone}>
                    {change.status}
                  </ConsoleStatusBadge>
                </span>
                <span role="cell" title={change.title}>
                  {change.detail}
                </span>
                <span role="cell" title={updated}>
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
  "current" | "failed" | "queued" | "stale" | "unknown";

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
  onSubmitRuntimeConfigPatch,
  onDeleteRuntimeConfigPatchGenerator,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenJobHistory,
  onOpenPrivilegeUnlock,
  onRenderRuntimeConfigPatchGenerator,
  onResolveBulk,
  onUpsertRuntimeConfigPatchGenerator,
  pending,
  privilegeMaterial,
  runAction,
}: {
  actionError: string | null;
  agents: AgentView[];
  runtimeConfigPatchGenerators: RuntimeConfigPatchGeneratorRecord[];
  onSubmitRuntimeConfigPatch: (
    request: RuntimeConfigPatchRequest,
  ) => Promise<RuntimeConfigPatchResponse>;
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobHistory: () => void;
  onOpenPrivilegeUnlock: () => void;
  onRenderRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: { values: JsonValue },
  ) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onUpsertRuntimeConfigPatchGenerator: (
    request: UpsertRuntimeConfigPatchGeneratorRequest,
  ) => Promise<RuntimeConfigPatchGeneratorRecord>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
}) {
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
  const reviewFeedbackTone: ActionFeedbackTone = reviewStatus?.startsWith(
    "Desired",
  )
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
  const patchGeneratorDraftError = patchGeneratorEditor
    ? validatePatchGeneratorEditor(patchGeneratorEditor)
    : null;
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const localSelectorTargets = useMemo(
    () =>
      selectorExpression.trim() && !selectorParse.error
        ? agentsMatchingExpression(agents, selectorExpression)
        : [],
    [agents, selectorExpression, selectorParse.error],
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
        }
        const nextPreview = await onResolveBulk(frozenSelector);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(nextPreview);
        setApplySnapshot(null);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
  }

  async function reviewApply() {
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
        if (frozenPatchMode === "temporary" && !frozenTemporaryToml.trim()) {
          throw new Error("Paste a temporary TOML patch");
        }
        const nextPreview = await onResolveBulk(frozenSelector);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const clientIds = nextPreview.targets.map((target) => target.id);
        if (!clientIds.length) {
          throw new Error("Bulk patch confirmation resolved no VPSs");
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
        const privilegeAssertion = await buildPrivilegeAssertion({
          intent: canonicalDbPrivilegeIntent({
            action: "runtime_config.patch",
            target: "runtime_config",
            selectorExpression: frozenSelector,
            resolvedTargets: clientIds,
            confirmed: true,
            payloadHash: patchPayloadHashHex,
          }),
          privilegeMaterial: frozenPrivilegeMaterial,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(nextPreview);
        setApplySnapshot({
          clientIds,
          jobId: crypto.randomUUID(),
          toml,
          patchName,
          patchSections,
          patchSource: frozenPatchMode,
          payloadHashHex: patchPayloadHashHex,
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
      const response = await onSubmitRuntimeConfigPatch({
        confirmed: true,
        reason: snapshot.patchName,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.clientIds,
        toml: snapshot.toml,
        privilege_assertion: snapshot.privilegeAssertion,
      });
      const dispatchWarning = runtimeConfigDispatchWarning(
        response.sync,
        "Desired patch saved",
      );
      if (dispatchWarning) {
        setReviewStatus(dispatchWarning);
        setApplySnapshot(null);
        return;
      }
      const jobIds = response.sync_job_ids;
      if (jobIds.length === 0) {
        throw new Error("Runtime config patch created no sync jobs");
      }
      const initial = buildBulkJobProgress({
        targetCount: response.target_count,
        jobId: snapshot.jobId,
        jobIds,
        targetRecords: [],
        targets: snapshot.targets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      setProgress(initial);
      const waited = await waitForBulkJobSet(jobIds, onLoadJobTargets, {
        operationId: snapshot.jobId,
        targetCount: response.target_count,
        onProgress: setProgress,
        targets: snapshot.targets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
        onLoadOutputs: onLoadJobOutputs,
      });
      setProgress(waited.progress);
      setApplySnapshot(null);
    });
  }

  return (
    <div className="configApplyGrid">
      <section className="compactForm bulkPatchPrimary">
        <div className="bulkPatchHeader">
          <ConfigHelpLabel
            help={CONFIG_HELP.incrementalPatch}
            label="Incremental patch"
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
            placeholder="[telemetry]\n# paste one incremental TOML patch"
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
            selectorParse.error ??
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
              : selectorExpression.trim()
                ? `${bulkVpsCountLabel(localSelectorTargets.length)} matched locally`
                : "No target selector"}
          </strong>
          <span>
            {preview
              ? "The final Apply confirmation will freeze this selector and re-resolve it before submission."
              : selectorExpression.trim()
                ? "The matching VPSs update immediately below. Preview changes verifies them on the server and builds the per-VPS patch summary."
                : "Add a selector; an empty selector is never treated as all VPSs."}
          </span>
        </div>
        <LocalTargetPreview agents={localSelectorTargets} />
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
            Apply patch
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
                  <label className="consoleField fieldFull">
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
                  <label className="consoleField fieldFull">
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
                  <label className="consoleField fieldFull">
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
                    title={patchGeneratorDraftError ?? "Save custom generator"}
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
        confirmLabel="Apply runtime config patch"
        detail={`Apply one generated incremental patch to ${bulkVpsCountLabel(applySnapshot?.clientIds.length ?? 0)}.`}
        error={actionError}
        expiresAtUnix={applySnapshot?.privilegeAssertion.expires_unix}
        items={[
          {
            label: "Selector",
            value: applySnapshot?.selectorExpression ?? "-",
          },
          {
            label: "Targets",
            value: `${applySnapshot?.clientIds.length ?? 0}`,
          },
          { label: "Source", value: applySnapshot?.patchSource ?? "-" },
          { label: "Patch", value: applySnapshot?.patchName ?? "-" },
          {
            label: "Sections",
            value: applySnapshot?.patchSections.join(", ") ?? "-",
          },
          {
            label: "Payload",
            value: applySnapshot?.payloadHashHex
              ? shortId(applySnapshot.payloadHashHex)
              : "-",
            title: applySnapshot?.payloadHashHex ?? "-",
          },
        ]}
        onCancel={() => {
          setConfirmOpen(false);
          setApplySnapshot(null);
        }}
        onConfirm={() => void applyPatch()}
        open={confirmOpen}
        pending={pending}
        title="Confirm bulk patch"
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
): string | null {
  const failures = sync.filter((outcome) => outcome.status !== "queued");
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
  patchMode,
  patchName,
  preview,
  sections,
  toml,
}: {
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

  return (
    <div
      className="bulkPatchPreviewSummary"
      aria-label="Bulk patch change summary"
    >
      <div>
        <strong>Preview changes</strong>
        <span>
          {preview
            ? `${bulkVpsCountLabel(preview.target_count)} will receive ${patchName}.`
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
          {visibleTargets.map((target) => (
            <span key={target.id} title={target.id}>
              <strong>{target.display_name}</strong>
              <small>{sectionSummary || "runtime config patch"}</small>
            </span>
          ))}
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

function SingleVpsConfig({
  actionError,
  agents,
  runtimeConfigApplyStates,
  runtimeConfigEvidenceState,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onSubmitRuntimeConfigPatch,
  pending,
  privilegeMaterial,
  runAction,
}: {
  actionError: string | null;
  agents: AgentView[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  runtimeConfigEvidenceState: EvidenceState;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onSubmitRuntimeConfigPatch: (
    request: RuntimeConfigPatchRequest,
  ) => Promise<RuntimeConfigPatchResponse>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [clientId, setClientId] = useState(() => readSingleConfigClientId());
  const clientIdRef = useRef(clientId);
  const [redactedToml, setRedactedToml] = useState("");
  const [baseHash, setBaseHash] = useState("");
  const [overrideToml, setOverrideToml] = useState("");
  const [overrideValidation, setOverrideValidation] = useState<{
    sections: string[];
    payloadHashHex: string;
  } | null>(null);
  const [overrideValidationGeneration, setOverrideValidationGeneration] =
    useState(0);
  const [applySnapshot, setApplySnapshot] =
    useState<SingleVpsConfigApplySnapshot | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [lastJobId, setLastJobId] = useState<string | null>(null);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(
    DEFAULT_MAX_JOB_TIMEOUT_SECS,
  );
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const [editorView, setEditorView] = useState<"current" | "patch">("patch");
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const singleTarget = useMemo(
    () => agents.find((agent) => agent.id === clientId) ?? null,
    [agents, clientId],
  );
  const runtimeApplyState = useMemo(
    () =>
      runtimeConfigEvidenceState === "available"
        ? (runtimeConfigApplyStates.find(
            (state) => state.client_id === clientId,
          ) ?? null)
        : null,
    [clientId, runtimeConfigApplyStates, runtimeConfigEvidenceState],
  );
  const overrideLineCount = useMemo(
    () => countConfigPatchLines(overrideToml),
    [overrideToml],
  );
  const overrideReady = Boolean(
    singleTarget && baseHash && overrideToml.trim(),
  );
  const reviewFeedbackTone = reviewStatus?.startsWith("Patch preview ready")
    ? "success"
    : reviewStatus?.startsWith("Desired")
      ? "warning"
      : "progress";

  useEffect(() => {
    clientIdRef.current = clientId;
    writeLocalString(CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY, clientId);
  }, [clientId]);

  useEffect(() => {
    let active = true;
    const frozenToml = overrideToml.trim();
    const frozenBaseHash = baseHash;
    const frozenTargetId = singleTarget?.id ?? "";
    if (!frozenTargetId || !frozenBaseHash || !frozenToml) {
      setOverrideValidation(null);
      return () => {
        active = false;
      };
    }
    const sections = inferTomlSections(frozenToml);
    void sha256Hex(new TextEncoder().encode(frozenToml)).then(
      (payloadHashHex) => {
        if (
          active &&
          clientIdRef.current === frozenTargetId &&
          baseHash === frozenBaseHash
        ) {
          setOverrideValidation({ sections, payloadHashHex });
          setReviewStatus(
            `Patch preview ready: ${sections.join(", ")} against base ${shortId(frozenBaseHash)}`,
          );
        }
      },
    );
    return () => {
      active = false;
    };
  }, [baseHash, overrideToml, overrideValidationGeneration, singleTarget?.id]);

  function clearSingleConfigReview() {
    invalidateReviewGeneration();
    setApplySnapshot(null);
    setConfirmOpen(false);
    setOverrideValidation(null);
    setReviewStatus(null);
  }

  function selectClientId(value: string) {
    if (value === clientIdRef.current) {
      return;
    }
    clientIdRef.current = value;
    clearSingleConfigReview();
    setClientId(value);
    setRedactedToml("");
    setBaseHash("");
    setProgress(null);
    setEditorView("patch");
  }

  async function reviewOverrideApply() {
    const reviewGeneration = captureReviewGeneration();
    const frozenTarget = singleTarget;
    const frozenPrivilegeMaterial = privilegeMaterial;
    const frozenToml = overrideToml.trim();
    const frozenBaseHash = baseHash;
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
    setApplySnapshot(null);
    setConfirmOpen(false);
    setReviewStatus("Preparing one-VPS override review");
    await runAction(async () => {
      await waitForReviewRender();
      if (!frozenTarget || !frozenPrivilegeMaterial) {
        throw new Error("Select one VPS and unlock privilege");
      }
      if (!frozenBaseHash) {
        throw new Error(
          "Read the current VPS config before applying an override",
        );
      }
      if (!frozenToml) {
        throw new Error("Paste a one-VPS runtime config override");
      }
      const selectorExpression = selectorExpressionForClientIds([
        frozenTarget.id,
      ]);
      const patchSections = inferTomlSections(frozenToml);
      const payloadHashHex = await sha256Hex(
        new TextEncoder().encode(frozenToml),
      );
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "runtime_config.patch",
          target: "runtime_config",
          selectorExpression,
          resolvedTargets: [frozenTarget.id],
          confirmed: true,
          payloadHash: payloadHashHex,
        }),
        privilegeMaterial: frozenPrivilegeMaterial,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setOverrideValidation({ sections: patchSections, payloadHashHex });
      setApplySnapshot({
        clientId: frozenTarget.id,
        selectorExpression,
        target: frozenTarget,
        toml: frozenToml,
        baseHash: frozenBaseHash,
        patchSections,
        maxTimeoutSecs: boundedMaxTimeoutSecs,
        privilegeAssertion,
        payloadHashHex,
      });
      setConfirmOpen(true);
      setReviewStatus(null);
    });
  }

  async function applyOverride() {
    setConfirmOpen(false);
    await runAction(async () => {
      const snapshot = applySnapshot;
      if (!snapshot) {
        throw new Error(
          "One-VPS override snapshot is missing; review the apply again",
        );
      }
      const response = await onSubmitRuntimeConfigPatch({
        confirmed: true,
        reason: `One-VPS override for ${snapshot.target.display_name}`,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: [snapshot.clientId],
        toml: snapshot.toml,
        privilege_assertion: snapshot.privilegeAssertion,
      });
      const dispatchWarning = runtimeConfigDispatchWarning(
        response.sync,
        "Desired one-VPS override saved",
      );
      if (dispatchWarning) {
        setReviewStatus(dispatchWarning);
        setApplySnapshot(null);
        return;
      }
      const firstJobId = response.sync_job_ids[0];
      if (!firstJobId) {
        throw new Error("One-VPS override created no sync job");
      }
      setLastJobId(firstJobId);
      const initial = buildBulkJobProgress({
        targetCount: response.target_count,
        jobId: firstJobId,
        targetRecords: [],
        targets: [snapshot.target],
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      setProgress(initial);
      const waited = await waitForBulkJobTargets(firstJobId, onLoadJobTargets, {
        targetCount: 1,
        onProgress: setProgress,
        targets: [snapshot.target],
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      let outputs: JobOutputRecord[] = [];
      let outputLoadWarning: string | null = null;
      try {
        outputs = await onLoadJobOutputs(firstJobId);
      } catch (error) {
        outputLoadWarning = `Final job output could not be loaded: ${
          error instanceof Error
            ? error.message
            : "the browser returned no failure detail."
        } Open job ${firstJobId} before retrying the override.`;
      }
      const finalProgress = buildBulkJobProgress({
        targetCount: response.target_count,
        jobId: firstJobId,
        outputs,
        targetRecords: waited.targets,
        targets: [snapshot.target],
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      setProgress(finalProgress);
      setApplySnapshot(null);
      if (
        finalProgress.total > 0 &&
        finalProgress.successful === finalProgress.total
      ) {
        setRedactedToml("");
        setBaseHash("");
        setOverrideToml("");
        setOverrideValidation(null);
        setReviewStatus(
          outputLoadWarning
            ? `Desired one-VPS override saved and the target completed, but ${outputLoadWarning}`
            : "Override applied. Read current config before drafting another change.",
        );
        setEditorView("current");
      } else if (outputLoadWarning) {
        setReviewStatus(
          `Desired one-VPS override saved, but ${outputLoadWarning}`,
        );
      }
    });
  }

  async function readConfig() {
    clearSingleConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenTarget = singleTarget;
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
    await runAction(async () => {
      if (!frozenTarget) {
        throw new Error("Select one VPS before reading runtime config");
      }
      const operation: JobOperation = { type: "config_read" };
      const selectorExpressionForTarget = selectorExpressionForClientIds([
        frozenTarget.id,
      ]);
      const response = await onCreateJob({
        argv: [],
        command: "config_read",
        confirmed: false,
        destructive: false,
        force_unprivileged: true,
        job_id: crypto.randomUUID(),
        operation,
        privileged: false,
        selector_expression: selectorExpressionForTarget,
        target_client_ids: [frozenTarget.id],
        max_timeout_secs: boundedMaxTimeoutSecs,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(
        response.job_id,
        onLoadJobTargets,
        {
          targetCount: createJobTargetCount(response),
          onProgress: setProgress,
          targets: [frozenTarget],
          maxTimeoutSecs: boundedMaxTimeoutSecs,
        },
      );
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      const outputs = await onLoadJobOutputs(response.job_id);
      setProgress(
        buildBulkJobProgress({
          targetCount: createJobTargetCount(response),
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: [frozenTarget],
          maxTimeoutSecs: boundedMaxTimeoutSecs,
        }),
      );
      const config = extractConfigRead(outputs);
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setRedactedToml(config.toml);
      setBaseHash(config.baseHash);
      setOverrideValidationGeneration((current) => current + 1);
      setEditorView("patch");
    });
  }

  return (
    <div className="configApplyGrid singleConfigFlow">
      <section
        className="compactForm singleConfigTargetPanel"
        aria-label="Per-VPS config target and load"
      >
        <ConfigHelpLabel
          help={CONFIG_HELP.targetSelector}
          label="VPS target"
          strong
        />
        <VpsCombobox
          agents={agents}
          ariaLabel="VPS config target"
          className="configTargetCombobox"
          onChange={selectClientId}
          placeholder="Search VPS config"
          value={clientId}
        />
        <div className="configTargetMeta">
          <span className="configTargetName">
            {singleTarget
              ? formatVpsName(singleTarget, vpsNameDisplayMode)
              : clientId
                ? "Select a listed VPS"
                : "no target selected"}
          </span>
          <span
            title={
              runtimeConfigEvidenceState === "available"
                ? runtimeConfigApplyStateSummary(runtimeApplyState, false)
                : undefined
            }
          >
            {runtimeConfigEvidenceState === "loading"
              ? "Checking apply-state evidence"
              : runtimeConfigEvidenceState === "unavailable"
                ? "Apply-state evidence unavailable"
                : runtimeConfigApplyStateSummary(runtimeApplyState)}
          </span>
        </div>
        <button
          className="secondaryAction"
          disabled={pending || !singleTarget}
          onClick={readConfig}
          title={
            pending
              ? "Wait for the current config operation to finish before reading runtime config."
              : !singleTarget
                ? "Select one VPS before reading runtime config."
                : "Read redacted runtime config from the selected VPS. No privilege unlock is required for this read-only inspection."
          }
          type="button"
        >
          <ServerCog size={16} />
          Read current config
        </button>
        {lastJobId && (
          <button
            className="secondaryAction"
            onClick={() => onOpenJobDetails(lastJobId)}
            title={lastJobId}
            type="button"
          >
            Open job {shortId(lastJobId)}
          </button>
        )}
        <details className="singleConfigAdvanced">
          <summary>Advanced read/apply options</summary>
          <label>
            <ConfigHelpLabel
              help={CONFIG_HELP.maxTimeout}
              label="Max timeout seconds"
            />
            <input
              aria-label="VPS config max timeout seconds"
              max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
              min={1}
              onChange={(event) => {
                clearSingleConfigReview();
                setMaxTimeoutSecs(Number(event.target.value));
              }}
              type="number"
              value={maxTimeoutSecs}
            />
          </label>
        </details>
      </section>

      {!singleTarget && (
        <section
          className="compactForm singleConfigEmpty"
          aria-label="Per-VPS config start"
        >
          <strong>Select one VPS</strong>
          <span>
            Choose a visible VPS to load its redacted current config, then draft
            one guarded TOML patch for that exact target.
          </span>
          <div
            className="singleConfigHelpGrid"
            aria-label="Per-VPS config safeguards"
          >
            <ConfigHelpLabel
              help={CONFIG_HELP.redactedRuntimeToml}
              label="Redacted runtime TOML"
            />
            <ConfigHelpLabel
              help={CONFIG_HELP.guardedOverride}
              label="Guarded one-VPS override"
            />
          </div>
          <SingleConfigGuardAnchors
            baseLabel="Read current config"
            payloadLabel="Patch hash before apply"
            sectionsLabel="Validated TOML sections"
          />
        </section>
      )}

      {singleTarget && !baseHash && (
        <section
          className="compactForm singleConfigLoadPanel"
          aria-label="Per-VPS config load current"
        >
          <strong>Load current config</strong>
          <span>
            Redacted config reads are inspection-only and do not require
            privilege unlock. The patch editor opens after the base hash is
            loaded.
          </span>
          <div
            className="singleConfigHelpGrid"
            aria-label="Per-VPS config safeguards"
          >
            <ConfigHelpLabel
              help={CONFIG_HELP.redactedRuntimeToml}
              label="Redacted runtime TOML"
            />
            <ConfigHelpLabel
              help={CONFIG_HELP.guardedOverride}
              label="Guarded one-VPS override"
            />
          </div>
          <SingleConfigGuardAnchors
            baseLabel="Read current config"
            payloadLabel="Patch hash before apply"
            sectionsLabel="Validated TOML sections"
          />
        </section>
      )}

      {singleTarget && baseHash && (
        <>
          <div
            className="singleConfigViewTabs"
            aria-label="Per-VPS config views"
          >
            <button
              className={editorView === "current" ? "active" : ""}
              onClick={() => setEditorView("current")}
              type="button"
            >
              Current base
            </button>
            <button
              className={editorView === "patch" ? "active" : ""}
              onClick={() => setEditorView("patch")}
              type="button"
            >
              Desired patch
            </button>
          </div>
          <section
            className={`compactForm configTomlEditor singleConfigPane singleConfigCurrentPane ${
              editorView === "current" ? "active" : ""
            }`}
            aria-label="Per-VPS current config"
          >
            <ConfigHelpLabel
              help={CONFIG_HELP.redactedRuntimeToml}
              label="Redacted runtime TOML"
              strong
            />
            <span title={baseHash}>
              base {shortId(baseHash)} / redacted runtime config for{" "}
              {formatVpsName(singleTarget, vpsNameDisplayMode)}
            </span>
            <textarea
              aria-label="VPS redacted runtime config TOML"
              readOnly
              rows={18}
              value={redactedToml}
            />
            <span className="formHint">
              This immutable redacted base is the guard for the one-VPS patch.
            </span>
          </section>
          <section
            className={`compactForm configTomlEditor configOverrideEditor singleConfigPane singleConfigPatchPane ${
              editorView === "patch" ? "active" : ""
            }`}
            aria-label="Per-VPS desired config patch"
          >
            <ConfigHelpLabel
              help={CONFIG_HELP.guardedOverride}
              label="Guarded one-VPS override"
              strong
            />
            <span>
              Draft one incremental TOML patch for this VPS. Sections and
              payload hash update while you type; Apply opens the final
              confirmation.
            </span>
            <SingleConfigGuardAnchors
              baseLabel={shortId(baseHash)}
              baseTitle={baseHash}
              exactTargetLabel={formatVpsName(singleTarget, vpsNameDisplayMode)}
              payloadLabel={
                overrideValidation?.payloadHashHex
                  ? shortId(overrideValidation.payloadHashHex)
                  : "Not ready"
              }
              payloadTitle={overrideValidation?.payloadHashHex}
              sectionsLabel={
                overrideValidation?.sections.join(", ") || "Type patch"
              }
            />
            <textarea
              aria-label="One-VPS runtime config override TOML"
              onChange={(event) => {
                clearSingleConfigReview();
                setOverrideToml(event.target.value);
              }}
              placeholder="[update]\n# one incremental override for this VPS"
              rows={14}
              value={overrideToml}
            />
            <ActionFeedback
              className="localActionFeedback configReviewFeedback"
              message={reviewStatus}
              tone={reviewFeedbackTone}
            />
            <div className="configOverrideActions singleConfigStickyActions">
              <span
                aria-label="One-VPS config change summary"
                className="singleConfigApplySummary"
              >
                <strong>
                  {overrideLineCount} changed{" "}
                  {overrideLineCount === 1 ? "line" : "lines"}
                </strong>
                <small>
                  {overrideValidation
                    ? `${overrideValidation.sections.length} ${overrideValidation.sections.length === 1 ? "section" : "sections"} - 0 errors`
                    : overrideToml.trim()
                      ? "Validating sections and payload hash"
                      : privilegeMaterial
                        ? "Type a patch before apply"
                        : "Unlock only when ready to apply"}
                </small>
              </span>
              <button
                className="primaryAction"
                disabled={pending || !overrideReady}
                onClick={() => {
                  if (!privilegeMaterial) {
                    setReviewStatus(
                      "Unlock privilege to apply this one-VPS patch",
                    );
                    onOpenPrivilegeUnlock();
                    return;
                  }
                  void reviewOverrideApply();
                }}
                title={
                  pending
                    ? "Wait for the current config operation to finish before applying."
                    : !baseHash
                      ? "Read the selected VPS runtime config before applying a patch."
                      : !overrideToml.trim()
                        ? "Enter one incremental TOML patch before applying."
                        : !privilegeMaterial
                          ? "Unlock privilege material before applying the patch."
                          : "Open the final one-VPS config apply confirmation."
                }
                type="button"
              >
                <FileSliders size={16} />
                Apply patch
              </button>
            </div>
          </section>
        </>
      )}
      {progress && (
        <ExecutionResultPanel
          loading={pending}
          onClearResults={() => setProgress(null)}
          onOpenJobDetails={onOpenJobDetails}
          progress={progress}
        />
      )}
      <ConfirmationPrompt
        confirmLabel="Apply one-VPS override"
        detail={`Apply one reviewed runtime config override to ${applySnapshot?.target.display_name ?? "one VPS"}.`}
        error={actionError}
        expiresAtUnix={applySnapshot?.privilegeAssertion.expires_unix}
        items={[
          { label: "VPS", value: applySnapshot?.target.display_name ?? "-" },
          {
            label: "Selector",
            value: applySnapshot?.selectorExpression ?? "-",
          },
          {
            label: "Base hash",
            value: applySnapshot?.baseHash
              ? shortId(applySnapshot.baseHash)
              : "-",
            title: applySnapshot?.baseHash ?? "-",
          },
          {
            label: "Sections",
            value: applySnapshot?.patchSections.join(", ") ?? "-",
          },
          {
            label: "Payload",
            value: applySnapshot?.payloadHashHex
              ? shortId(applySnapshot.payloadHashHex)
              : "-",
            title: applySnapshot?.payloadHashHex ?? "-",
          },
          {
            label: "Timeout",
            value: `${applySnapshot?.maxTimeoutSecs ?? maxTimeoutSecs}s`,
          },
        ]}
        onCancel={() => {
          setConfirmOpen(false);
          setApplySnapshot(null);
        }}
        onConfirm={() => void applyOverride()}
        open={confirmOpen}
        pending={pending}
        title="Confirm one-VPS runtime config override"
      />
    </div>
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

type VpsRuleFieldDefinition = {
  help: string;
  inputMode?: "decimal" | "numeric" | "text";
  key: (typeof VPS_RULE_KEYS)[number];
  label: string;
  placeholder: string;
};

const VPS_RULE_FIELD_DEFINITIONS: VpsRuleFieldDefinition[] = [
  {
    help: "Optional card price, for example 29.90 CNY/m, 48 USD/q, 60 €/hy, or 99 USD/y. Use -1 to explicitly disable billing display as n/a; blank leaves the rule unset.",
    inputMode: "text",
    key: "billing.price",
    label: "Billing price",
    placeholder: "29.90 CNY/m",
  },
  {
    help: "Optional renewal anchor, independent of traffic reset day. Use a day for /m (for example 15), or day-month for /q, /hy, and /y (for example 15-06).",
    inputMode: "text",
    key: "billing.cycle",
    label: "Billing cycle",
    placeholder: "15 or 15-06",
  },
  {
    help: "Optional display-only port speed, for example 400Mbps or 1.5 Gbps. It does not configure shaping, quotas, or the agent network.",
    inputMode: "text",
    key: "network.port_speed",
    label: "Port speed",
    placeholder: "1.5 Gbps",
  },
  {
    help: `Existing traffic-selector syntax for aggregate live rates and charts. An absent rule, clearing this typed field, or entering [] selects every reported interface and direction. Enter ${NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX} to store a live reference to traffic.selectors, or override with selectors such as eth0,eth1+tx. Unsetting the rule restores All interfaces.`,
    inputMode: "text",
    key: "network.rate.interfaces",
    label: "Live rate interfaces",
    placeholder: NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX,
  },
  {
    help: "Day of month in UTC when the traffic accounting cycle resets.",
    inputMode: "numeric",
    key: "traffic.reset_day",
    label: "Reset day",
    placeholder: "14",
  },
  {
    help: "Total monthly traffic quota. Type 4TB, 750GB, raw bytes, or -1 for explicitly unlimited. Blank leaves the rule unset.",
    inputMode: "text",
    key: "traffic.quota.total",
    label: "Total quota",
    placeholder: "4TB",
  },
  {
    help: "Optional receive-side traffic quota. Use -1 for explicitly unlimited; blank leaves the rule unset.",
    inputMode: "text",
    key: "traffic.quota.rx",
    label: "RX quota",
    placeholder: "Optional",
  },
  {
    help: "Optional transmit-side traffic quota. Use -1 for explicitly unlimited; blank leaves the rule unset.",
    inputMode: "text",
    key: "traffic.quota.tx",
    label: "TX quota",
    placeholder: "Optional",
  },
  {
    help: "Traffic selectors as comma-separated interface+direction tokens, for example ens3, eth0+tx, or tun0+rx.",
    inputMode: "text",
    key: "traffic.selectors",
    label: "Interfaces / selectors",
    placeholder: "ens3, eth0+tx",
  },
];

type VpsRuleAlertPolicyImpact = {
  conditionExpression: string;
  enabled: boolean;
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

function parseVpsRuleTextValues(text: string): Record<string, string> {
  const values: Record<string, string> = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) {
      continue;
    }
    const equals = line.indexOf("=");
    if (equals <= 0) {
      continue;
    }
    const key = line.slice(0, equals).trim();
    if (!VPS_RULE_KEYS.includes(key as (typeof VPS_RULE_KEYS)[number])) {
      continue;
    }
    const value = line.slice(equals + 1).trim();
    if (value || key === "network.rate.interfaces") {
      values[key] = value || "[]";
    }
  }
  return values;
}

function serializeVpsRuleTextValues(values: Record<string, string>): string {
  return VPS_RULE_KEYS.flatMap((key) => {
    const value = values[key]?.trim();
    return value ? [`${key}=${value}`] : [];
  }).join("\n");
}

function updateVpsRuleTextValue(
  text: string,
  key: (typeof VPS_RULE_KEYS)[number],
  value: string,
): string {
  const values = parseVpsRuleTextValues(text);
  const trimmed = value.trim();
  if (key === "network.rate.interfaces" && !trimmed) {
    values[key] = "[]";
    return serializeVpsRuleTextValues(values);
  }
  if (trimmed) {
    values[key] = trimmed;
  } else {
    delete values[key];
  }
  return serializeVpsRuleTextValues(values);
}

function affectedAlertPolicyRules(
  policies: FleetAlertPolicyRecord[],
  keys: string[],
): VpsRuleAlertPolicyImpact[] {
  const matchKeys = keys.length > 0 ? keys : [...VPS_RULE_KEYS];
  return policies
    .flatMap((policy) =>
      policy.rules
        .filter((rule) =>
          matchKeys.some((key) => rule.condition_expression.includes(key)),
        )
        .map((rule) => ({
          conditionExpression: rule.condition_expression,
          enabled: policy.enabled && rule.enabled,
          policyId: policy.id,
          policyName: policy.name,
          ruleId: rule.id,
          ruleName: rule.name,
          severity: rule.severity,
        })),
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
  billing_long_cycle_requires_day_month:
    "Quarterly, half-year, and yearly billing use day-month, such as 15-06.",
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
  traffic_selector_source_invalid:
    "Selector source must be host or tunnel.",
  traffic_selector_interface_required: "Each selector needs an interface name.",
  traffic_selector_interface_invalid:
    "Use an exact interface name without spaces or wildcards.",
  traffic_selector_direction_invalid:
    "Selector direction must be rx, tx, or total.",
  traffic_selector_duplicate: "Remove the duplicate selector.",
  traffic_selector_direction_overlap:
    "Do not select the same interface direction more than once.",
  traffic_selector_too_many_items: "Use no more than 16 selectors.",
  traffic_reset_day_invalid: "Traffic reset day must be between 1 and 31.",
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

function normalizeVpsRuleValue(key: string, value: string | null): string {
  if (value == null) {
    return "unset";
  }
  const text = value.trim();
  if (!text) {
    return "empty";
  }
  if (key.startsWith("traffic.quota.")) {
    const bytes = parseByteQuantity(text);
    if (bytes != null) {
      return `bytes:${bytes}`;
    }
  }
  if (key === "traffic.reset_day") {
    const numeric = parsePlainNumber(text);
    if (numeric != null) {
      return `number:${numeric}`;
    }
  }
  if (key === "traffic.selectors") {
    return normalizeSelectorRuleValue(text);
  }
  if (key === "network.rate.interfaces") {
    if (text === "[]") return "network-rate:all";
    if (text === NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX) {
      return "network-rate:reference:traffic.selectors";
    }
    return `network-rate:${normalizeSelectorRuleValue(text)}`;
  }
  return normalizeGenericRuleValue(text);
}

function normalizeSelectorRuleValue(text: string): string {
  const jsonValue = parseJsonValue(text);
  const rawItems = Array.isArray(jsonValue)
    ? jsonValue.map((item) => String(item))
    : text.split(",");
  const items = rawItems
    .map((item) => normalizeSelectorRuleToken(item))
    .filter(Boolean)
    .sort((left, right) => left.localeCompare(right));
  return `selectors:${items.join(",")}`;
}

function normalizeSelectorRuleToken(token: string): string {
  const normalized = token.trim().replace(/^host:/, "");
  if (!normalized) {
    return "";
  }
  return normalized.includes("+") ? normalized : `${normalized}+total`;
}

function normalizeGenericRuleValue(text: string): string {
  const jsonValue = parseJsonValue(text);
  if (jsonValue !== undefined) {
    return `json:${stableJsonStringify(normalizeJsonValue(jsonValue))}`;
  }
  const bytes = parseByteQuantity(text);
  if (bytes != null) {
    return `bytes:${bytes}`;
  }
  const numeric = parsePlainNumber(text);
  if (numeric != null) {
    return `number:${numeric}`;
  }
  const normalizedText = text.replace(/\s+/g, " ");
  if (normalizedText.includes(",")) {
    return `list:${normalizedText
      .split(",")
      .map((item) => item.trim())
      .filter(Boolean)
      .sort((left, right) => left.localeCompare(right))
      .join(",")}`;
  }
  const lower = normalizedText.toLowerCase();
  if (["false", "no", "off"].includes(lower)) {
    return "boolean:false";
  }
  if (["true", "yes", "on"].includes(lower)) {
    return "boolean:true";
  }
  return `text:${normalizedText}`;
}

function parseJsonValue(text: string): unknown | undefined {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function normalizeJsonValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    const values = value.map((item) => normalizeJsonValue(item));
    if (
      values.every(
        (item) =>
          item === null ||
          ["boolean", "number", "string"].includes(typeof item),
      )
    ) {
      return values.sort((left, right) =>
        stableJsonStringify(left).localeCompare(stableJsonStringify(right)),
      );
    }
    return values;
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([objectKey, objectValue]) => [
          objectKey,
          normalizeJsonValue(objectValue),
        ]),
    );
  }
  return typeof value === "string" ? value.trim().replace(/\s+/g, " ") : value;
}

function stableJsonStringify(value: unknown): string {
  return JSON.stringify(value);
}

function parseByteQuantity(text: string): number | null {
  const match = text
    .trim()
    .replace(/_/g, "")
    .match(
      /^([0-9]+(?:\.[0-9]+)?)\s*(bytes?|b|kb|mb|gb|tb|pb|kib|mib|gib|tib|pib)?$/i,
    );
  if (!match) {
    return null;
  }
  const amount = Number(match[1]);
  if (!Number.isFinite(amount)) {
    return null;
  }
  const unit = (match[2] ?? "b").toLowerCase();
  const multipliers: Record<string, number> = {
    b: 1,
    byte: 1,
    bytes: 1,
    gb: 1_000_000_000,
    gib: 1_073_741_824,
    kb: 1_000,
    kib: 1_024,
    mb: 1_000_000,
    mib: 1_048_576,
    pb: 1_000_000_000_000_000,
    pib: 1_125_899_906_842_624,
    tb: 1_000_000_000_000,
    tib: 1_099_511_627_776,
  };
  return Math.round(amount * (multipliers[unit] ?? 1));
}

function parsePlainNumber(text: string): number | null {
  const compact = text.trim().replace(/_/g, "");
  if (!/^[+-]?\d+(?:\.\d+)?$/.test(compact)) {
    return null;
  }
  const numeric = Number(compact);
  return Number.isFinite(numeric) ? numeric : null;
}

function VpsRulesPanel({
  agents,
  fleetAlertPolicies,
  initialSelectorExpression,
  onBulkUnset,
  onBulkUpsert,
  onDryRun,
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
  onOpenAlerts: () => void;
  trafficAccounting: TrafficAccountingRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
}) {
  const [selectorExpression, setSelectorExpression] = useState(
    () =>
      initialSelectorExpression ??
      readLocalString(CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY),
  );
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
  const pending = reviewPending || applyPending;
  const [status, setStatus] = useState<string | null>(null);
  const [statusTone, setStatusTone] = useState<ActionFeedbackTone>("info");
  const statusFeedbackRef = useRef<HTMLDivElement | null>(null);
  const previousStatusFeedbackRef = useRef<string | null>(null);
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
  const localSelectorTargets = useMemo(
    () =>
      selectorExpression.trim() && !parsedSelector.error
        ? agentsMatchingExpression(agents, selectorExpression)
        : [],
    [agents, parsedSelector.error, selectorExpression],
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

  useEffect(() => {
    if (initialSelectorExpression) {
      setSelectorExpression(initialSelectorExpression);
    }
  }, [initialSelectorExpression]);

  useEffect(() => {
    writeLocalString(CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  useEffect(() => {
    invalidateReviewGeneration();
    setPreview(null);
    setReviewSnapshot(null);
    setReviewPromptOpen(false);
    setReviewPending(false);
    setStatus(null);
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
      if (!value && key !== "network.rate.interfaces") {
        throw new Error(`VPS rule ${key} cannot be empty; use explicit unset`);
      }
      if (Object.prototype.hasOwnProperty.call(values, key)) {
        throw new Error(`Duplicate VPS rule key: ${key}`);
      }
      values[key] = value || "[]";
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
                  type="button"
                >
                  Unset values
                </button>
              </div>
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
                    onClick={() => setSelectorExpression("")}
                    type="button"
                  >
                    Clear
                  </button>
                </div>
              </div>
              <label className="consoleField">
                <span>VPS selector expression</span>
                <SearchExpressionInput
                  agents={agents}
                  ariaLabel="VPS rules selector expression"
                  disabled={applyPending}
                  onChange={setSelectorExpression}
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
              <LocalTargetPreview
                agents={localSelectorTargets}
                ariaLabel="Local VPS rule match preview"
              />
              <small className="vpsRulesTargetHint">
                Local match only. Preview changes resolves and binds the
                authoritative VPS list.
              </small>
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
                      Typed fields for billing, live rate, quota, reset day,
                      and traffic interfaces
                    </span>
                  </div>
                </div>
                <div
                  className="vpsRuleTypedGrid"
                  aria-label="Common VPS rule fields"
                >
                  {VPS_RULE_FIELD_DEFINITIONS.map((field) => (
                    <label className="vpsRuleTypedCard" key={field.key}>
                      <span>
                        <strong>{field.label}</strong>
                        <small className="monoValue">{field.key}</small>
                      </span>
                      <input
                        aria-label={field.label}
                        disabled={applyPending}
                        inputMode={field.inputMode ?? "text"}
                        onChange={(event) =>
                          setValuesText((current) =>
                            updateVpsRuleTextValue(
                              current,
                              field.key,
                              event.target.value,
                            ),
                          )
                        }
                        placeholder={field.placeholder}
                        title={field.help}
                        value={typedRuleValues[field.key] ?? ""}
                      />
                      <small>{field.help}</small>
                    </label>
                  ))}
                </div>
                <details className="vpsRulesAdvancedRaw">
                  <summary>Advanced raw key/value</summary>
                  <textarea
                    aria-label="VPS rule set values"
                    disabled={applyPending}
                    value={valuesText}
                    onChange={(event) => setValuesText(event.target.value)}
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
                    <label className="checkLine" key={key}>
                      <input
                        aria-label={`Unset ${key}`}
                        checked={unsetKeys.includes(key)}
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
                  key={`${impact.policyId}:${impact.ruleId}`}
                >
                  <span>
                    <strong>
                      {impact.policyName} / {impact.ruleName}
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
              status={status}
              statusTone={statusTone}
            />
          ) : null}
        </section>
      </div>
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
          },
          { label: "Operation", value: reviewSnapshot?.operation ?? "-" },
          {
            label: "Set keys",
            value: Object.keys(reviewSnapshot?.values ?? {}).join(", ") || "-",
          },
          {
            label: "Unset keys",
            value: reviewSnapshot?.keys.join(", ") || "-",
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

function SingleConfigGuardAnchors({
  baseLabel,
  baseTitle,
  exactTargetLabel,
  payloadLabel,
  payloadTitle,
  sectionsLabel,
}: {
  baseLabel: string;
  baseTitle?: string;
  exactTargetLabel?: string;
  payloadLabel: string;
  payloadTitle?: string;
  sectionsLabel: string;
}) {
  return (
    <div
      className="configOverrideSummary"
      aria-label="One-VPS config override guard"
    >
      {exactTargetLabel && (
        <span>
          <strong>Exact target</strong>
          <small>{exactTargetLabel}</small>
        </span>
      )}
      <span>
        <strong title={CONFIG_HELP.currentBase}>Current base</strong>
        <small title={baseTitle}>{baseLabel}</small>
      </span>
      <span>
        <strong title={CONFIG_HELP.sections}>Patch sections</strong>
        <small>{sectionsLabel}</small>
      </span>
      <span>
        <strong title={CONFIG_HELP.payload}>Payload</strong>
        <small title={payloadTitle}>{payloadLabel}</small>
      </span>
    </div>
  );
}

function countConfigPatchLines(toml: string): number {
  return toml
    .split(/\r?\n/)
    .filter((line) => line.trim() && !line.trim().startsWith("#")).length;
}

function VpsRulesPreviewTable({
  columns,
  onRequestApply,
  pending,
  preview,
  status,
  statusTone,
}: {
  columns: ConsoleDataGridColumn<VpsRuleChangePreview>[];
  onRequestApply: () => void;
  pending: boolean;
  preview: VpsRulesOperatorPreview;
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
            type="button"
          >
            {finalActionLabel}
          </button>
        ) : null}
      </div>
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
      return "Bulk patch";
    case "single":
      return "Per-VPS config";
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
      return "Reviewed runtime config patch workflow";
    case "single":
      return "Read and compare one VPS runtime config";
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
        .map((line) => /^\[([^[\]]+)\]$/.exec(line)?.[1]?.trim())
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

function extractConfigRead(outputs: JobOutputRecord[]): {
  toml: string;
  baseHash: string;
} {
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    const value = JSON.parse(base64ToText(output.data_base64)) as {
      type?: string;
      toml?: string;
      config_sha256_hex?: string;
    };
    if (value.type === "config_read" && value.toml && value.config_sha256_hex) {
      return { toml: value.toml, baseHash: value.config_sha256_hex };
    }
  }
  throw new Error("Config read output was not available yet");
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

function base64ToText(value: string): string {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return new TextDecoder().decode(bytes);
}

function readLocalString(key: string): string {
  try {
    return window.localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function readSingleConfigClientId(): string {
  const storedClientId = readLocalString(
    CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY,
  ).trim();
  if (storedClientId) {
    return storedClientId;
  }
  return clientIdFromLegacySelector(
    readLocalString(CONFIG_SINGLE_SELECTOR_STORAGE_KEY),
  );
}

function clientIdFromLegacySelector(value: string): string {
  const match = value
    .trim()
    .match(/^id:(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^\s()&|]+))$/i);
  if (!match) {
    return "";
  }
  return (match[1] ?? match[2] ?? match[3] ?? "").replace(/\\(["'\\])/g, "$1");
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
