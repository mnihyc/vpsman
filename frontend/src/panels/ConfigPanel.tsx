import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { FileSliders, Play, RefreshCw, ServerCog, Trash2 } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  buildBulkJobProgress,
  createJobTargetCount,
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
  parseSearchExpression,
  selectorExpressionForClientIds,
} from "../searchExpression";
import {
  clampJobMaxTimeoutSecs,
  clampInteger,
  DEFAULT_MAX_JOB_TIMEOUT_SECS,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "./jobDispatchModel";
import type {
  AgentView,
  BulkResolveResponse,
  RuntimeConfigPatchRequest,
  RuntimeConfigPatchResponse,
  CreateJobRequest,
  CreateJobResponse,
  FleetAlertPolicyRecord,
  SourceTemplateAssignmentRecord,
  SourceTemplateRecord,
  SourceStatusRecord,
  DeleteRuntimeConfigPatchGeneratorRequest,
  RuntimeConfigApplyStateRecord,
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
import { formatTime, formatVpsName, runPanelAction, shortId } from "../utils";

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
    "Per-target command timeout bounded by the backend so slow agents cannot hold config work indefinitely.",
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
    "Per-VPS traffic rule values feed accounting and alert policies; dry-run previews changed rows before write.",
  ruleSelector:
    "Fleet selector used for the dry-run and final reviewed VPS rule mutation.",
  ruleSetValues:
    "Key=value lines become typed VPS rule values after backend validation and dry-run diffing.",
  ruleUnsetValues:
    "Explicit rule keys removed from every matched VPS after dry-run review.",
  previewHash:
    "Backend hash of the dry-run diff that the apply request must echo to prevent stale writes.",
} as const;
const VPS_RULE_KEYS = [
  "traffic.reset_day",
  "traffic.quota.total",
  "traffic.quota.rx",
  "traffic.quota.tx",
  "traffic.selectors",
] as const;
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

export function ConfigPanel({
  activeSubpage,
  agents,
  trafficAccounting,
  vpsRuleValues,
  sourceTemplateAssignments,
  sourceTemplates,
  sourceStatus,
  error,
  runtimeConfigApplyStates,
  runtimeConfigPatchGenerators,
  fleetAlertPolicies,
  jobs,
  loading,
  onSubmitRuntimeConfigPatch,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onDeleteRuntimeConfigPatchGenerator,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onOpenSourceTemplates,
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
  setPrivilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  trafficAccounting: TrafficAccountingRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceTemplates: SourceTemplateRecord[];
  sourceStatus: SourceStatusRecord[];
  error: string | null;
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
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
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenSourceTemplates: () => void;
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
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const subpage = normalizeConfigSubpage(activeSubpage);
  const rulesSelectorPrefill = activeSubpage.startsWith("rules:id:")
    ? `id:${decodeURIComponent(activeSubpage.slice("rules:id:".length))}`
    : null;

  return (
    <section className="workspace singleColumn configWorkspace">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>{configTitle(subpage)}</h2>
            <span>
              {actionError ??
                error ??
                (loading
                  ? "Refreshing runtime config state"
                  : configSubtitle(subpage))}
            </span>
          </div>
          <button
            className="secondaryAction"
            disabled={loading || pending}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
        {subpage === "overview" && (
          <ConfigOverview
            agents={agents}
            sourceTemplateAssignments={sourceTemplateAssignments}
            sourceTemplates={sourceTemplates}
            sourceStatus={sourceStatus}
            runtimeConfigApplyStates={runtimeConfigApplyStates}
            runtimeConfigPatchGenerators={runtimeConfigPatchGenerators}
            vpsRuleValues={vpsRuleValues}
            jobs={jobs}
            onSelectSubpage={onSelectSubpage}
          />
        )}
        {subpage === "bulk" && (
          <BulkConfigApply
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
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        )}
        {subpage === "single" && (
          <SingleVpsConfig
            agents={agents}
            runtimeConfigApplyStates={runtimeConfigApplyStates}
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
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        )}
        {subpage === "templates" && (
          <ConfigTemplateSummary
            agents={agents}
            assignments={sourceTemplateAssignments}
            onOpenSourceTemplates={onOpenSourceTemplates}
            sourceStatus={sourceStatus}
            templates={sourceTemplates}
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
  sourceTemplateAssignments,
  sourceTemplates,
  sourceStatus,
  runtimeConfigApplyStates,
  runtimeConfigPatchGenerators,
  vpsRuleValues,
  jobs,
  onSelectSubpage,
}: {
  agents: AgentView[];
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceTemplates: SourceTemplateRecord[];
  sourceStatus: SourceStatusRecord[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
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
  const sourceRiskRows = sourceStatus.filter(
    (row) => !isReadySourceStatus(row.status),
  );
  const sourceReadyRows = sourceStatus.length - sourceRiskRows.length;
  const currentStateRows = buildConfigCurrentStateRows(
    agents,
    runtimeConfigApplyStates,
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
  const assignedClientIds = new Set(
    sourceTemplateAssignments.map((assignment) => assignment.client_id),
  );
  const missingApplyStates = currentStateRows.filter(
    (row) => row.resourceAvailable && row.statusKind === "unknown",
  ).length;
  const missingTemplateCoverage = Math.max(
    agents.length - assignedClientIds.size,
    0,
  );
  const customTemplateCount = sourceTemplates.filter(
    (template) => !template.built_in,
  ).length;
  const invalidRuleRows = vpsRuleValues.filter(
    (row) => row.state !== "ok",
  ).length;
  const validRuleRows = vpsRuleValues.length - invalidRuleRows;
  const applyAttentionCount = currentStateRows.filter((row) =>
    ["failed", "stale", "queued", "unknown"].includes(row.statusKind),
  ).length;
  const retryableApplyRows = currentStateRows.filter(
    (row) => row.actionKind === "retry",
  );
  const configHealth = configHealthStatus({
    failedSyncs,
    invalidRuleRows,
    missingApplyStates,
    missingTemplateCoverage,
    pendingSyncs,
    staleApplyRows,
    sourceRiskCount: sourceRiskRows.length,
    totalRuleRows: vpsRuleValues.length,
    validRuleRows,
  });
  const latestApplyStates = runtimeConfigApplyStates
    .slice()
    .sort((left, right) =>
      configApplyStateTime(right).localeCompare(configApplyStateTime(left)),
    )
    .slice(0, 4);
  const recentChanges = [
    ...latestApplyStates.map((state) => ({
      detail: runtimeConfigApplyStateSummary(state),
      id: `apply:${state.client_id}`,
      operation: "Apply state",
      status: runtimeConfigApplyStatusLabel(state),
      target: agentNameById.get(state.client_id) ?? state.client_id,
      time: configApplyStateTime(state),
      tone: runtimeConfigApplyTone(state),
    })),
    ...configJobs.map((job) => ({
      detail: `Job ${shortId(job.id)} created ${formatTime(job.created_at)}`,
      id: `job:${job.id}`,
      operation: job.command_type,
      status: job.status,
      target: "runtime config",
      time: job.created_at,
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
      action: "Open Template coverage",
      detail:
        "Review coverage and assignments; persistent authoring belongs to Source templates.",
      subpage: "templates",
      title: "Template coverage",
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
              Runtime apply state, template coverage, source readiness, and
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
              {appliedClientIds.size}/{agents.length || 0}
            </strong>
            <small>VPSs current</small>
          </span>
          <span>
            <strong>{failedSyncs + staleApplyRows}</strong>
            <small>failed or stale applies</small>
          </span>
          <span>
            <strong>
              {sourceReadyRows}/{sourceStatus.length || 0}
            </strong>
            <small>source checks ready</small>
          </span>
          <span>
            <strong>
              {ruleValidityLabel(validRuleRows, vpsRuleValues.length)}
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
            tone={retryableApplyRows.length ? "warning" : "ok"}
          >
            {retryableApplyRows.length} retryable
          </ConsoleStatusBadge>
        </div>
        <div className="configCurrentStateList">
          {currentStateRows.map((row) => (
            <div
              className={`configCurrentStateRow ${row.statusKind}`}
              key={row.id}
            >
              <span className="configCurrentTarget">
                <strong title={row.targetTitle}>{row.targetLabel}</strong>
                <small>{row.targetDetail}</small>
              </span>
              <span>
                <ConsoleStatusBadge tone={row.tone}>
                  {row.statusLabel}
                </ConsoleStatusBadge>
                <small>{row.statusDetail}</small>
              </span>
              <span>
                <strong>{row.ruleLabel}</strong>
                <small>{row.ruleDetail}</small>
              </span>
              <span>
                <strong>{formatTime(row.updatedAt)}</strong>
                <small>{row.updatedDetail}</small>
              </span>
              {row.actionKind === "retry" || row.actionKind === "inspect" ? (
                <button
                  className="secondaryAction compactAction"
                  onClick={() => openCurrentStateAction(row)}
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
      </section>

      <div className="configOverviewColumns">
        <section
          className="configOverviewBlock"
          aria-label="Config drift summary"
        >
          <div className="configOverviewBlockHeader">
            <h3>Drift summary</h3>
            <ConsoleStatusBadge
              tone={sourceRiskRows.length || failedSyncs ? "warning" : "ok"}
            >
              {applyAttentionCount + sourceRiskRows.length + invalidRuleRows}{" "}
              action items
            </ConsoleStatusBadge>
          </div>
          <div className="configRiskList">
            <ConfigOverviewRiskRow
              detail={`${failedSyncs} failed, ${staleApplyRows} stale, ${pendingSyncs} queued, ${missingApplyStates} unknown; latest state per VPS only`}
              label="Runtime apply state"
              tone={
                failedSyncs
                  ? "critical"
                  : staleApplyRows || pendingSyncs || missingApplyStates
                    ? "warning"
                    : "ok"
              }
              value={
                failedSyncs + staleApplyRows + pendingSyncs + missingApplyStates
              }
            />
            <ConfigOverviewRiskRow
              detail={
                sourceRiskRows[0]?.status_reason ??
                `${sourceReadyRows} source checks are ready`
              }
              label="Source readiness drift"
              tone={sourceRiskRows.length ? "warning" : "ok"}
              value={sourceRiskRows.length}
            />
            <ConfigOverviewRiskRow
              detail={`${ruleValidityLabel(validRuleRows, vpsRuleValues.length)}; invalid values stay in Rules details`}
              label="Rule validation"
              tone={invalidRuleRows ? "warning" : "ok"}
              value={invalidRuleRows}
            />
          </div>
        </section>

        <section
          className="configOverviewBlock"
          aria-label="Config template coverage"
        >
          <div className="configOverviewBlockHeader">
            <h3>Template coverage</h3>
            <ConsoleStatusBadge
              tone={missingTemplateCoverage ? "warning" : "ok"}
            >
              {assignedClientIds.size}/{agents.length || 0} VPSs
            </ConsoleStatusBadge>
          </div>
          <div className="configCoverageGrid">
            <span>
              <strong>{sourceTemplates.length}</strong>
              <small>templates</small>
            </span>
            <span>
              <strong>{customTemplateCount}</strong>
              <small>custom templates</small>
            </span>
            <span>
              <strong>{sourceTemplateAssignments.length}</strong>
              <small>assignments</small>
            </span>
            <span>
              <strong>{missingTemplateCoverage}</strong>
              <small>VPSs without assignment evidence</small>
            </span>
          </div>
          <p>
            Persistent source-template authoring stays in Automation / Source
            Templates; Config uses assignments and rendered runtime state for
            operator review.
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
        <div className="table hierarchyTable">
          <div className="historyRow heading configRecentGrid">
            <span>Target</span>
            <span>Operation</span>
            <span>Status</span>
            <span>Detail</span>
            <span>Updated</span>
          </div>
          {recentChanges.map((change) => (
            <div className="historyRow configRecentGrid" key={change.id}>
              <span>{change.target}</span>
              <span>{change.operation}</span>
              <span>
                <ConsoleStatusBadge tone={change.tone}>
                  {change.status}
                </ConsoleStatusBadge>
              </span>
              <span>{change.detail}</span>
              <span>{formatTime(change.time)}</span>
            </div>
          ))}
          {recentChanges.length === 0 && (
            <div className="emptyState compactEmpty">
              No recent config changes.
            </div>
          )}
        </div>
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
  value: number;
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
  targetDetail: string;
  targetLabel: string;
  targetTitle: string;
  tone: "critical" | "warning" | "ok" | "info" | "neutral";
  updatedAt: string;
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
      configApplyStateTime(state) > configApplyStateTime(current)
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
    targetDetail,
    targetLabel,
    targetTitle: resourceAvailable ? clientId : `Missing resource ${clientId}`,
    tone: status.tone,
    updatedAt: state ? configApplyStateTime(state) : new Date(0).toISOString(),
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
      return {
        detail: `Queued since ${formatTime(configApplyStateTime(state))}; treat as stale before retry`,
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
    const version = state.applied_version
      ? `v${state.applied_version}`
      : "applied";
    return {
      detail: `${version}; hash ${shortId(state.applied_content_hash)}`,
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
  const updatedAt = Date.parse(configApplyStateTime(state));
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

type ConfigTemplateCoverageRow = {
  assignedClients: number;
  assignments: number;
  attentionLabel: string;
  attentionChecks: number;
  attentionRows: SourceStatusRecord[];
  domain: string;
  domainLabel: string;
  desiredDetail: string;
  desiredSource: string;
  fixDetail: string;
  fixFilter: string;
  fixLabel: string;
  readyChecks: number;
  readinessTotal: number;
  storedDetail: string;
  storedLabel: string;
  templates: number;
  updatedAt: string;
};

function ConfigTemplateSummary({
  agents,
  assignments,
  onOpenSourceTemplates,
  sourceStatus,
  templates,
}: {
  agents: AgentView[];
  assignments: SourceTemplateAssignmentRecord[];
  onOpenSourceTemplates: () => void;
  sourceStatus: SourceStatusRecord[];
  templates: SourceTemplateRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const assignedClientIds = useMemo(
    () => new Set(assignments.map((assignment) => assignment.client_id)),
    [assignments],
  );
  const readyStatusCount = sourceStatus.filter((row) =>
    isReadySourceStatus(row.status),
  ).length;
  const attentionStatusRows = sourceStatus.filter(
    (row) => !isReadySourceStatus(row.status),
  );
  const customTemplateCount = templates.filter(
    (template) => !template.built_in,
  ).length;
  const coverageRows = useMemo<ConfigTemplateCoverageRow[]>(() => {
    const domains = Array.from(
      new Set([
        ...templates.map((template) => template.domain),
        ...assignments.map((assignment) => assignment.domain),
        ...sourceStatus.map((row) => row.domain),
      ]),
    ).sort((left, right) => left.localeCompare(right));
    return domains.map((domain) => {
      const domainTemplates = templates.filter(
        (template) => template.domain === domain,
      );
      const domainAssignments = assignments.filter(
        (assignment) => assignment.domain === domain,
      );
      const domainStatus = sourceStatus.filter((row) => row.domain === domain);
      const defaultTemplate =
        domainTemplates.find((template) => template.is_default)?.name ??
        domainTemplates[0]?.name ??
        "";
      const attentionRows = domainStatus.filter(
        (row) => !isReadySourceStatus(row.status),
      );
      const readyChecks = domainStatus.filter((row) =>
        isReadySourceStatus(row.status),
      ).length;
      const primaryStatus = attentionRows[0] ?? domainStatus[0] ?? null;
      const desiredSource =
        primaryStatus?.template_name ??
        defaultTemplate ??
        "No selected source";
      const desiredDetail = primaryStatus
        ? `${sourceDomainLabel(domain)} uses ${sourceTokenLabel(
            primaryStatus.source_kind,
          )}`
        : defaultTemplate
          ? "Domain default from template registry"
          : "No source status or template record loaded";
      const storedLabel =
        domainTemplates.length > 0
          ? `${domainTemplates.length} stored`
          : primaryStatus?.template_name
            ? "Runtime selected only"
            : "No stored template";
      const storedDetail =
        domainTemplates.length > 0
          ? [
              `${domainTemplates.filter((template) => template.built_in).length} built-in`,
              `${domainTemplates.filter((template) => !template.built_in).length} custom`,
              defaultTemplate ? `default ${defaultTemplate}` : null,
            ]
              .filter(Boolean)
              .join(" · ")
          : primaryStatus?.template_name
            ? `${primaryStatus.template_name} is visible from source status, not the registry`
            : "Create or import a source template before assignment";
      const attentionLabel =
        attentionRows.length > 0
          ? sourceStatusLabel(attentionRows[0].status)
          : domainStatus.length > 0
            ? "No attention"
            : "No checks";
      const fixLabel =
        attentionRows.length > 0
          ? "Fix source"
          : domainTemplates.length === 0
            ? "Add"
            : "Review";
      const fixFilter = [
        sourceDomainLabel(domain),
        domain,
        primaryStatus?.template_name,
        primaryStatus?.source_kind,
      ]
        .filter(Boolean)
        .join(" ");
      const fixDetail =
        attentionRows.length > 0
          ? `${formatVpsName(
              attentionRows[0],
              vpsNameDisplayMode,
            )}: ${sourceStatusLabel(attentionRows[0].status)}`
          : domainTemplates.length === 0
            ? "Open authoring filtered to this source domain"
            : "Open the source template registry for this domain";
      const updatedAt =
        domainTemplates
          .map((template) => template.updated_at)
          .filter(Boolean)
          .sort((left, right) => right.localeCompare(left))[0] ??
        domainAssignments
          .map((assignment) => assignment.assigned_at)
          .filter(Boolean)
          .sort((left, right) => right.localeCompare(left))[0] ??
        "";
      return {
        assignedClients: new Set(
          domainAssignments.map((assignment) => assignment.client_id),
        ).size,
        assignments: domainAssignments.length,
        attentionChecks: attentionRows.length,
        attentionLabel,
        attentionRows,
        domain,
        domainLabel: sourceDomainLabel(domain),
        desiredDetail,
        desiredSource,
        fixDetail,
        fixFilter,
        fixLabel,
        readyChecks,
        readinessTotal: domainStatus.length,
        storedDetail,
        storedLabel,
        templates: domainTemplates.length,
        updatedAt,
      };
    });
  }, [assignments, sourceStatus, templates, vpsNameDisplayMode]);
  const openCoverageFix = (row: ConfigTemplateCoverageRow) => {
    seedSourceTemplateSearch(row.fixFilter);
    onOpenSourceTemplates();
  };
  const coverageColumns = useMemo<
    ConsoleDataGridColumn<ConfigTemplateCoverageRow>[]
  >(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary sourceCoverageCellText">
            <strong>{row.domainLabel}</strong>
            <small>Desired source: {row.desiredSource}</small>
          </span>
        ),
        header: "Desired source",
        id: "desired",
        minSize: 220,
        searchValue: (row) =>
          `${row.domain} ${row.domainLabel} ${row.desiredSource} ${row.desiredDetail}`,
        sortValue: (row) => row.domainLabel,
      },
      {
        cell: (row) => (
          <span className="historyPrimary sourceCoverageCellText">
            <strong>{row.storedLabel}</strong>
            <small>{row.storedDetail}</small>
          </span>
        ),
        header: "Stored/available",
        id: "stored",
        minSize: 220,
        searchValue: (row) => `${row.storedLabel} ${row.storedDetail}`,
        sortValue: (row) => row.templates,
      },
      {
        cell: (row) => (
          <span className="historyPrimary sourceCoverageCellText">
            <strong>
              {row.assignedClients}/{agents.length} VPSs
            </strong>
            <small>{row.assignments} assignment rows</small>
          </span>
        ),
        header: "Assigned VPSs",
        id: "assignments",
        minSize: 150,
        searchValue: (row) =>
          `${row.assignedClients} VPSs ${row.assignments} assignment rows`,
        sortValue: (row) => row.assignedClients,
      },
      {
        cell: (row) => (
          <ConsoleStatusBadge tone={row.readyChecks > 0 ? "ok" : "neutral"}>
            {row.readinessTotal > 0
              ? `${row.readyChecks}/${row.readinessTotal} ready`
              : "No checks"}
          </ConsoleStatusBadge>
        ),
        header: "Ready",
        id: "ready",
        searchValue: (row) =>
          `${row.readyChecks} ready ${row.readinessTotal} checks`,
        sortValue: (row) => row.readyChecks,
      },
      {
        cell: (row) => (
          <span className="historyPrimary sourceCoverageCellText">
            <ConsoleStatusBadge
              tone={row.attentionChecks > 0 ? "warning" : "ok"}
            >
              {row.attentionChecks > 0
                ? `${row.attentionChecks} attention`
                : "None"}
            </ConsoleStatusBadge>
            <small>{row.attentionLabel}</small>
          </span>
        ),
        header: "Attention",
        id: "attention",
        minSize: 170,
        searchValue: (row) =>
          `${row.attentionChecks} attention ${row.attentionLabel}`,
        sortValue: (row) => row.attentionChecks,
      },
      {
        cell: (row) => (
          <button
            className="secondaryAction compactAction sourceCoverageFixAction"
            onClick={(event) => {
              event.stopPropagation();
              openCoverageFix(row);
            }}
            title={row.fixDetail}
            type="button"
          >
            <FileSliders size={15} />
            <span>{row.fixLabel}</span>
          </button>
        ),
        header: "Fix",
        id: "fix",
        minSize: 150,
        searchValue: (row) => `${row.fixLabel} ${row.fixDetail}`,
        sortValue: (row) => row.attentionChecks,
      },
    ],
    [agents.length, onOpenSourceTemplates],
  );
  return (
    <div
      className="configOverviewStack configTemplateSummary"
      aria-label="Config template summary"
    >
      <section
        className="configHealthPanel"
        aria-label="Config template coverage summary"
      >
        <div className="configHealthHeader">
          <div>
            <h3>Template coverage</h3>
            <span>
              Read-only source coverage for Config: selected source, stored
              template, assigned VPSs, readiness, and fix target.
            </span>
          </div>
          <ConsoleStatusBadge
            tone={attentionStatusRows.length > 0 ? "warning" : "ok"}
          >
            {attentionStatusRows.length > 0 ? "Needs review" : "Ready"}
          </ConsoleStatusBadge>
        </div>
        <div className="configHealthSummary">
          <span>
            <strong>{templates.length}</strong>
            <small>templates</small>
          </span>
          <span>
            <strong>{customTemplateCount}</strong>
            <small>custom</small>
          </span>
          <span>
            <strong>{assignments.length}</strong>
            <small>assignments</small>
          </span>
          <span>
            <strong>
              {assignedClientIds.size}/{agents.length}
            </strong>
            <small>VPSs covered</small>
          </span>
          <span>
            <strong>{readyStatusCount}</strong>
            <small>ready checks</small>
          </span>
          <span>
            <strong>{attentionStatusRows.length}</strong>
            <small>needs review</small>
          </span>
        </div>
      </section>

      <section
        className="configOverviewBlock configTemplateCanonical"
        aria-label="Source template canonical home"
      >
        <div className="configOverviewBlockHeader">
          <h3>Source template authoring</h3>
          <span>Automation / Source Templates</span>
        </div>
        <p>
          Config shows runtime source coverage only. Create, clone, diff, test,
          update, assign, and render persistent templates in Automation so
          emergency config patches and source authoring stay separate.
        </p>
        <button
          className="primaryAction"
          onClick={onOpenSourceTemplates}
          type="button"
        >
          <FileSliders size={16} />
          Open Source Templates
        </button>
      </section>

      <ConsoleDataGrid
        columns={coverageColumns}
        defaultPageSize={8}
        empty={
          <div className="emptyState compactEmpty">
            No template domains are loaded.
          </div>
        }
        expandOnRowClick
        getRowId={(row) => row.domain}
        itemLabel="template domains"
        renderExpandedRow={(row) => (
          <div className="consoleInlineDetailGrid">
            <span>
              <strong>Desired source</strong>
              <span>{row.desiredSource}</span>
            </span>
            <span>
              <strong>Stored/available</strong>
              <span>{row.storedDetail}</span>
            </span>
            <span>
              <strong>Assigned VPSs</strong>
              <span>
                {row.assignedClients}/{agents.length} VPSs from{" "}
                {row.assignments} assignments
              </span>
            </span>
            <span>
              <strong>Ready</strong>
              <span>
                {row.readinessTotal > 0
                  ? `${row.readyChecks}/${row.readinessTotal} source checks`
                  : "No source checks loaded"}
              </span>
            </span>
            <span>
              <strong>Attention</strong>
              <span>{row.attentionLabel}</span>
            </span>
            <span>
              <strong>Latest evidence</strong>
              <span>
                {row.updatedAt ? formatTime(row.updatedAt) : "No update evidence"}
              </span>
            </span>
            <span>
              <strong>Fix target</strong>
              <span>{row.fixDetail}</span>
            </span>
          </div>
        )}
        rows={coverageRows}
        searchPlaceholder="Search source coverage"
        selectable={false}
        storageKey="vpsman.config.templateSummary.domains"
        title="Source coverage by domain"
      />

      <section
        className="configOverviewBlock"
        aria-label="Config source readiness exceptions"
      >
        <div className="configOverviewBlockHeader">
          <h3>Source readiness exceptions</h3>
          <span>{attentionStatusRows.length} records need review</span>
        </div>
        <div className="configRiskList">
          {attentionStatusRows.slice(0, 6).map((row) => (
            <div
              className="configRiskRow"
              key={`${row.client_id}:${row.domain}`}
            >
              <span>
                <strong>
                  {formatVpsName(row, vpsNameDisplayMode)} /{" "}
                  {sourceDomainLabel(row.domain)}
                </strong>
                <small>
                  {row.template_name} / {row.status_reason}
                </small>
              </span>
              <div className="sourceCoverageExceptionActions">
                <ConsoleStatusBadge tone="warning">
                  {sourceStatusLabel(row.status)}
                </ConsoleStatusBadge>
                <button
                  className="secondaryAction compactAction sourceCoverageFixAction"
                  onClick={() => {
                    seedSourceTemplateSearch(
                      [
                        sourceDomainLabel(row.domain),
                        row.domain,
                        row.template_name,
                        row.source_kind,
                      ]
                        .filter(Boolean)
                        .join(" "),
                    );
                    onOpenSourceTemplates();
                  }}
                  type="button"
                >
                  <FileSliders size={15} />
                  <span>Fix source</span>
                </button>
              </div>
            </div>
          ))}
          {attentionStatusRows.length === 0 && (
            <div className="emptyState compactEmpty">
              All loaded source checks are ready.
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

function seedSourceTemplateSearch(filter: string) {
  const trimmed = filter.trim();
  if (!trimmed || typeof window === "undefined") {
    return;
  }
  for (const storageKey of [
    "vpsman.sourceTemplates.registry",
    "vpsman.sourceTemplates.activeSources",
  ]) {
    try {
      const current = window.localStorage.getItem(storageKey);
      const parsed = current ? JSON.parse(current) : {};
      const preferences =
        parsed && typeof parsed === "object" && !Array.isArray(parsed)
          ? parsed
          : {};
      window.localStorage.setItem(
        storageKey,
        JSON.stringify({ ...preferences, globalFilter: trimmed }),
      );
    } catch {
      window.localStorage.setItem(
        storageKey,
        JSON.stringify({ globalFilter: trimmed }),
      );
    }
  }
}

function sourceStatusLabel(status: SourceStatusRecord["status"]): string {
  switch (status) {
    case "ok":
    case "ready":
      return "Ready";
    case "ready_on_demand":
      return "Ready on demand";
    case "selected":
      return "Selected";
    case "selected_workflow":
      return "Workflow selected";
    case "metadata_only":
      return "Metadata only";
    case "agent_offline":
      return "VPS offline";
    case "unknown_domain":
      return "Unknown source domain";
    case "selected_no_store":
      return "Server storage missing";
    case "selected_no_artifacts":
      return "Artifacts missing";
    case "selected_no_limits":
      return "Limits unavailable";
    case "selected_no_samples":
      return "Samples missing";
    case "needs_promotion":
      return "Promotion needed";
    case "degraded":
      return "Degraded";
    default:
      return sourceTokenLabel(status);
  }
}

function sourceDomainLabel(value: string): string {
  return sourceTokenLabel(value)
    .replace(/\bospf\b/gi, "OSPF")
    .replace(/\bvps\b/gi, "VPS");
}

function sourceTokenLabel(value: string | null | undefined): string {
  const trimmed = value?.trim() ?? "";
  if (!trimmed) {
    return "Not configured";
  }
  return trimmed
    .replace(/[_:-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function isReadySourceStatus(status: SourceStatusRecord["status"]): boolean {
  return ["ok", "ready", "ready_on_demand", "selected"].includes(status);
}

function configHealthStatus({
  failedSyncs,
  invalidRuleRows,
  missingApplyStates,
  missingTemplateCoverage,
  pendingSyncs,
  staleApplyRows,
  sourceRiskCount,
  totalRuleRows,
  validRuleRows,
}: {
  failedSyncs: number;
  invalidRuleRows: number;
  missingApplyStates: number;
  missingTemplateCoverage: number;
  pendingSyncs: number;
  staleApplyRows: number;
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
    missingTemplateCoverage > 0
  ) {
    return {
      detail: `${pendingSyncs} applies are queued, ${sourceRiskCount} source checks need review, ${missingApplyStates} VPSs lack apply-state evidence, and ${ruleValidityLabel(validRuleRows, totalRuleRows)}.`,
      label: "Needs review",
      tone: "warning",
    };
  }
  return {
    detail:
      "All loaded VPSs have applied runtime state, source readiness, template assignment evidence, and valid rule rows.",
    label: "Healthy",
    tone: "ok",
  };
}

function configApplyStateTime(state: RuntimeConfigApplyStateRecord): string {
  return state.pending_updated_at ?? state.applied_at ?? state.updated_at;
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
  agents,
  runtimeConfigPatchGenerators,
  onSubmitRuntimeConfigPatch,
  onDeleteRuntimeConfigPatchGenerator,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRenderRuntimeConfigPatchGenerator,
  onResolveBulk,
  onUpsertRuntimeConfigPatchGenerator,
  pending,
  privilegeMaterial,
  runAction,
  setPrivilegeMaterial,
}: {
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
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
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
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(
    DEFAULT_MAX_JOB_TIMEOUT_SECS,
  );
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectedGenerator = runtimeConfigPatchGenerators.find(
    (generator) =>
      generator.id === (generatorId || runtimeConfigPatchGenerators[0]?.id),
  );
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
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
    privilegeMaterial &&
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

  function clearBulkConfigReview() {
    invalidateReviewGeneration();
    setApplySnapshot(null);
    setConfirmOpen(false);
    setReviewStatus(null);
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
      await onUpsertRuntimeConfigPatchGenerator({
        category: generator.category,
        description: generator.description,
        docs_metadata: generator.docs_metadata,
        domain: generator.domain,
        field_schema: generator.field_schema,
        name: `${generator.name}.copy`,
        raw_generator_body: generator.raw_generator_body,
        confirmed: true,
      });
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
      const firstJobId = response.sync_job_ids[0] ?? snapshot.jobId;
      const initial = buildBulkJobProgress({
        targetCount: response.target_count,
        jobId: firstJobId,
        targetRecords: [],
        targets: snapshot.targets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      setProgress(initial);
      const waited = await waitForBulkJobTargets(firstJobId, onLoadJobTargets, {
        targetCount: 1,
        onProgress: setProgress,
        targets: snapshot.targets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      const outputs = await onLoadJobOutputs(firstJobId).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          targetCount: response.target_count,
          jobId: firstJobId,
          outputs,
          targetRecords: waited.targets,
          targets: snapshot.targets,
          maxTimeoutSecs: snapshot.maxTimeoutSecs,
        }),
      );
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
            onClick={() => setManageGeneratorsOpen((open) => !open)}
            type="button"
          >
            Manage generators
          </button>
        </div>
        <div className="segmentedControl" aria-label="Patch source">
          <button
            className={patchMode === "generator" ? "activeAction" : ""}
            onClick={() => {
              setPatchMode("generator");
              clearBulkConfigReview();
            }}
            type="button"
          >
            Saved generator
          </button>
          <button
            className={patchMode === "temporary" ? "activeAction" : ""}
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
                ? "not previewed"
                : "no selector")
          }
        />
        <div className="bulkTargetState">
          <strong>
            {preview
              ? `${bulkVpsCountLabel(preview.target_count)} resolved`
              : selectorExpression.trim()
                ? "Selector not previewed"
                : "No target selector"}
          </strong>
          <span>
            {preview
              ? "The final Apply confirmation will freeze this selector and re-resolve it before submission."
              : selectorExpression.trim()
                ? "Preview changes to show the resolved VPS count and per-VPS patch summary."
                : "Add a selector; an empty selector is never treated as all VPSs."}
          </span>
        </div>
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
        {reviewStatus && <span className="formHint">{reviewStatus}</span>}
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
        <PrivilegeVaultBox
          labelPrefix="Runtime config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={(material) => {
            setPrivilegeMaterial(material);
            clearBulkConfigReview();
          }}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Open Privilege Vault for runtime config"
        />
        <div className="singleConfigStickyActions bulkPatchApplyActions">
          <span>
            {ready
              ? `Ready to apply ${bulkVpsCountLabel(preview?.target_count ?? 0)}`
              : "Preview changes and unlock before applying a bulk runtime config patch."}
          </span>
          <button
            className="primaryAction"
            disabled={pending || !ready}
            onClick={() => void reviewApply()}
            title={
              pending
                ? "Wait for the current config operation to finish before opening apply confirmation."
                : !ready
                  ? "Preview changes and unlock privilege material before applying."
                  : "Open the final runtime config apply confirmation."
            }
            type="button"
          >
            <FileSliders size={16} />
            Apply patch
          </button>
        </div>
      </section>
      <details
        className="bulkGeneratorManagement"
        open={manageGeneratorsOpen}
        onToggle={(event) => setManageGeneratorsOpen(event.currentTarget.open)}
      >
        <summary>Patch generator registry</summary>
        {manageGeneratorsOpen && (
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
                <span>Generator ID</span>
                <strong>{generator.id}</strong>
                <span>Name</span>
                <strong>{generator.name}</strong>
                <span>Category</span>
                <strong>{generator.category}</strong>
                <span>Domain</span>
                <strong>{generator.domain}</strong>
                <span>Scope</span>
                <strong>{generator.built_in ? "built-in" : "custom"}</strong>
                <span>Updated</span>
                <strong>{formatTime(generator.updated_at)}</strong>
                <span>Schema</span>
                <pre>{JSON.stringify(generator.field_schema, null, 2)}</pre>
                <span>Docs</span>
                <pre>{JSON.stringify(generator.docs_metadata, null, 2)}</pre>
              </div>
            )}
            rowActions={patchGeneratorActions}
            rows={runtimeConfigPatchGenerators}
            searchPlaceholder="Search patch generators"
            storageKey="vpsman.config.patchGenerators"
            title="Patch generators"
          />
        )}
      </details>
      {progress && (
        <ExecutionResultPanel
          loading={pending}
          onClearResults={() => setProgress(null)}
          onOpenJobDetails={onOpenJobDetails}
          progress={progress}
        />
      )}
      <ConfirmationPrompt
        confirmLabel="Apply runtime config patch"
        detail={`Apply one generated incremental patch to ${applySnapshot?.clientIds.length ?? 0} frozen VPS targets.`}
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
  agents,
  runtimeConfigApplyStates,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onSubmitRuntimeConfigPatch,
  pending,
  privilegeMaterial,
  runAction,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
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
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
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
      runtimeConfigApplyStates.find((state) => state.client_id === clientId) ??
      null,
    [clientId, runtimeConfigApplyStates],
  );
  const overrideReady = Boolean(
    singleTarget && privilegeMaterial && baseHash && overrideToml.trim(),
  );

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
  }, [baseHash, overrideToml, singleTarget?.id]);

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
      const firstJobId = response.sync_job_ids[0] ?? crypto.randomUUID();
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
      const outputs = await onLoadJobOutputs(firstJobId).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          targetCount: response.target_count,
          jobId: firstJobId,
          outputs,
          targetRecords: waited.targets,
          targets: [snapshot.target],
          maxTimeoutSecs: snapshot.maxTimeoutSecs,
        }),
      );
      setApplySnapshot(null);
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
          <span>{runtimeConfigApplyStateSummary(runtimeApplyState)}</span>
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
          <PrivilegeVaultBox
            labelPrefix="Runtime config apply"
            lastPayloadHash={null}
            onOpenUnlock={onOpenPrivilegeUnlock}
            onPrivilegeMaterialChange={setPrivilegeMaterial}
            privilegeMaterial={privilegeMaterial}
            unlockRedirectLabel="Open Privilege Vault for runtime config apply"
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
          <PrivilegeVaultBox
            labelPrefix="Runtime config apply"
            lastPayloadHash={null}
            onOpenUnlock={onOpenPrivilegeUnlock}
            onPrivilegeMaterialChange={setPrivilegeMaterial}
            privilegeMaterial={privilegeMaterial}
            unlockRedirectLabel="Open Privilege Vault for runtime config apply"
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
            <span>
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
              exactTargetLabel={formatVpsName(singleTarget, vpsNameDisplayMode)}
              payloadLabel={
                overrideValidation?.payloadHashHex
                  ? shortId(overrideValidation.payloadHashHex)
                  : "Not ready"
              }
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
            {reviewStatus && <span className="formHint">{reviewStatus}</span>}
            <PrivilegeVaultBox
              labelPrefix="Runtime config apply"
              lastPayloadHash={overrideValidation?.payloadHashHex ?? null}
              onOpenUnlock={onOpenPrivilegeUnlock}
              onPrivilegeMaterialChange={(material) => {
                clearSingleConfigReview();
                setPrivilegeMaterial(material);
              }}
              privilegeMaterial={privilegeMaterial}
              unlockRedirectLabel="Open Privilege Vault for runtime config apply"
            />
            <div className="configOverrideActions singleConfigStickyActions">
              <span>
                {privilegeMaterial
                  ? "Privilege unlocked for final apply"
                  : "Unlock only when ready to apply"}
              </span>
              <button
                className="primaryAction"
                disabled={pending || !overrideReady}
                onClick={() => void reviewOverrideApply()}
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
    help: "Day of month in UTC when the traffic accounting cycle resets.",
    inputMode: "numeric",
    key: "traffic.reset_day",
    label: "Reset day",
    placeholder: "14",
  },
  {
    help: "Total monthly traffic quota. Operators may type units such as 4TB, 750GB, or raw bytes.",
    inputMode: "text",
    key: "traffic.quota.total",
    label: "Total quota",
    placeholder: "4TB",
  },
  {
    help: "Optional receive-side traffic quota. Leave blank to keep this key out of the set request.",
    inputMode: "text",
    key: "traffic.quota.rx",
    label: "RX quota",
    placeholder: "Optional",
  },
  {
    help: "Optional transmit-side traffic quota. Leave blank to keep this key out of the set request.",
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
    if (value) {
      values[key] = value;
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
    (change) => !isNoOpVpsRuleChange(change),
  );
  return {
    ...preview,
    changed_row_count: changes.length,
    changes,
    no_op_row_count: preview.changes.length - changes.length,
  };
}

function isNoOpVpsRuleChange(change: VpsRuleChangePreview): boolean {
  if (change.validation !== "ok" || change.validation_errors.length > 0) {
    return false;
  }
  return (
    normalizeVpsRuleValue(change.key, change.before) ===
    normalizeVpsRuleValue(change.key, change.after)
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
      (readLocalString(CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY) || "tag:edge"),
  );
  const [keyFilter, setKeyFilter] = useState("");
  const [stateFilter, setStateFilter] = useState("");
  const [showIncompleteOnly, setShowIncompleteOnly] = useState(false);
  const [valuesText, setValuesText] = useState(
    "traffic.reset_day=14\ntraffic.quota.total=3TB\ntraffic.selectors=eth0+tx,ens3",
  );
  const [unsetKeys, setUnsetKeys] = useState<string[]>([]);
  const [editMode, setEditMode] = useState<VpsRulesEditMode>("upsert");
  const [preview, setPreview] = useState<VpsRulesOperatorPreview | null>(null);
  const [reviewSnapshot, setReviewSnapshot] =
    useState<VpsRulesReviewSnapshot | null>(null);
  const [pending, setPending] = useState(false);
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
        size: 130,
        minSize: 100,
        searchValue: (row) =>
          `${row.validation} ${row.validation_errors.join(" ")}`,
        sortValue: (row) => row.validation,
        cell: (row) => (
          <ConsoleStatusBadge tone={row.validation === "ok" ? "ok" : "warning"}>
            {row.validation}
          </ConsoleStatusBadge>
        ),
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
    setReviewSnapshot(null);
  }, [selectorExpression]);

  useEffect(() => {
    setReviewSnapshot(null);
  }, [valuesText, unsetKeys]);

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
      values[key] = value;
    }
    if (Object.keys(values).length === 0) {
      throw new Error("Add at least one VPS rule value to set");
    }
    return values;
  }

  async function dryRun(operation: "upsert" | "unset") {
    setPending(true);
    setStatus(
      operation === "upsert"
        ? "dry-running set values"
        : "dry-running unset values",
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
      const nextPreview = buildOperatorVpsRulesPreview(rawPreview);
      setPreview(nextPreview);
      setReviewSnapshot(
        nextPreview.changed_row_count > 0
          ? {
              operation,
              selectorExpression: selectorExpression.trim(),
              values,
              keys,
              preview: nextPreview,
            }
          : null,
      );
      setStatus(
        nextPreview.changed_row_count === 0
          ? `No changes detected across ${nextPreview.matched_vps_count} matched VPSs`
          : `${operation === "upsert" ? "set" : "unset"} preview found ${nextPreview.changed_row_count} changes across ${nextPreview.matched_vps_count} matched VPSs`,
      );
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "VPS rules dry-run failed",
      );
      setReviewSnapshot(null);
    } finally {
      setPending(false);
    }
  }

  async function applyReview() {
    const snapshot = reviewSnapshot;
    if (!snapshot) {
      setStatus("Run dry-run before applying VPS rules");
      return;
    }
    if (snapshot.preview.changed_row_count === 0) {
      setStatus("No changes detected; Apply is disabled.");
      setReviewSnapshot(null);
      return;
    }
    setPending(true);
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
      setPreview(nextPreview);
      setReviewSnapshot(null);
      setStatus(`applied ${nextPreview.changed_row_count} VPS rule changes`);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "VPS rules apply failed",
      );
    } finally {
      setPending(false);
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
      <div className="consoleFilterBar">
        <label>
          <span>VPS selector expression</span>
          <SearchExpressionInput
            ariaLabel="VPS rules selector expression"
            onChange={setSelectorExpression}
            placeholder="provider:hetzner && tag:edge"
            value={selectorExpression}
          />
        </label>
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
          <strong>Incomplete VPSs</strong>
          <span>{incompleteClients.size}</span>
        </span>
        <span>
          <strong>Current selector</strong>
          <span className="monoValue">{selectorExpression || "unset"}</span>
        </span>
        <span>
          <strong>Affected policies</strong>
          <span>{affectedPolicyRules.length}</span>
        </span>
      </div>
      <section
        className="consoleDetailPanel vpsRulesAlertImpact"
        aria-label="Affected alert policy context"
      >
        <div className="consoleDetailPanelHeader">
          <span>
            <strong>Affected alert policies</strong>
            <small>
              Policies whose rule conditions reference the current edit keys:{" "}
              <span className="monoValue">
                {editedRuleKeys.join(", ") || "traffic.*"}
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
              <ConsoleStatusBadge tone={impact.enabled ? "warning" : "neutral"}>
                {impact.severity}
              </ConsoleStatusBadge>
            </div>
          ))}
          {affectedPolicyRules.length === 0 && (
            <div className="emptyState compactEmpty">
              No loaded alert policy conditions reference the current rule keys.
            </div>
          )}
        </div>
      </section>
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
        storageKey="vpsman.grid.config.vpsRules"
        title="VPS rule values"
      />
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
                onClick={() => {
                  setEditMode("upsert");
                  setReviewSnapshot(null);
                }}
                type="button"
              >
                Set values
              </button>
              <button
                aria-pressed={editMode === "unset"}
                className={editMode === "unset" ? "selected" : ""}
                onClick={() => {
                  setEditMode("unset");
                  setReviewSnapshot(null);
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
          <section className="vpsRulesEditorSection">
            <div className="sectionHeader compactHeader">
              <div>
                <h4 title={CONFIG_HELP.ruleSelector}>Target VPS selector</h4>
                <span className="monoValue">
                  {selectorExpression || "unset"}
                </span>
              </div>
              <div className="consoleOperationsActions">
                <button
                  className="secondaryAction compactAction"
                  onClick={() => setSelectorExpression("")}
                  type="button"
                >
                  Clear
                </button>
              </div>
            </div>
            <div className="tokenPreview">
              {matchedPreviewClients.length === 0 ? (
                <span className="tokenChip">
                  {preview
                    ? `${preview.matched_vps_count} matched · no effective changes`
                    : "No preview yet"}
                </span>
              ) : (
                matchedPreviewClients.map(([clientId, displayName]) => (
                  <span className="tokenChip" key={clientId} title={clientId}>
                    {displayName}
                  </span>
                ))
              )}
            </div>
          </section>
          {editMode === "upsert" ? (
            <section className="vpsRulesEditorSection vpsRulesTypedEditor">
              <div className="sectionHeader compactHeader">
                <div>
                  <h4 title={CONFIG_HELP.ruleSetValues}>Common rule cards</h4>
                  <span title={CONFIG_HELP.ruleSetValues}>
                    Typed fields for quota, reset day, and traffic interfaces
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
        {preview ? (
          <VpsRulesPreviewTable columns={previewColumns} preview={preview} />
        ) : null}
        {status && <small className="fleetPolicyStatus">{status}</small>}
      </section>
      <ConfirmationPrompt
        confirmLabel={
          reviewSnapshot
            ? `Apply ${reviewSnapshot.preview.changed_row_count} ${reviewSnapshot.preview.changed_row_count === 1 ? "change" : "changes"}`
            : "Apply changes"
        }
        detail="Applies the reviewed preview by selector and backend preview hash."
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
            label: "Matched VPSs",
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
        onCancel={() => setReviewSnapshot(null)}
        onConfirm={() => void applyReview()}
        open={reviewSnapshot !== null}
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
  exactTargetLabel,
  payloadLabel,
  sectionsLabel,
}: {
  baseLabel: string;
  exactTargetLabel?: string;
  payloadLabel: string;
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
        <small>{baseLabel}</small>
      </span>
      <span>
        <strong title={CONFIG_HELP.sections}>Patch sections</strong>
        <small>{sectionsLabel}</small>
      </span>
      <span>
        <strong title={CONFIG_HELP.payload}>Payload</strong>
        <small>{payloadLabel}</small>
      </span>
    </div>
  );
}

function VpsRulesPreviewTable({
  columns,
  preview,
}: {
  columns: ConsoleDataGridColumn<VpsRuleChangePreview>[];
  preview: VpsRulesOperatorPreview;
}) {
  return (
    <div className="vpsRulesPreviewBlock">
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Matched VPSs</strong>
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
            <summary>Backend binding</summary>
            <span className="monoValue" title={CONFIG_HELP.previewHash}>
              {preview.preview_hash}
            </span>
          </details>
        </span>
      </div>
      {preview.changed_row_count === 0 ? (
        <div className="emptyState compactEmpty">No changes detected</div>
      ) : null}
      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={10}
        empty="No effective changes in preview."
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
    case "templates":
      return "Template coverage";
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
    case "templates":
      return "Read-only runtime template coverage and source readiness";
    default:
      return "Runtime config workflows";
  }
}

function normalizeConfigSubpage(
  value: string,
): "overview" | "bulk" | "single" | "rules" | "templates" {
  const base = value.split(":")[0];
  if (base === "per_vps") {
    return "single";
  }
  if (base === "bulk_patch") {
    return "bulk";
  }
  if (
    base === "bulk" ||
    base === "single" ||
    base === "rules" ||
    base === "templates"
  ) {
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

function formatJsonObject(value: Record<string, JsonValue>): string {
  return JSON.stringify(value, null, 2);
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
      base_config_sha256_hex?: string;
    };
    if (
      value.type === "config_read" &&
      value.toml &&
      value.base_config_sha256_hex
    ) {
      return { toml: value.toml, baseHash: value.base_config_sha256_hex };
    }
  }
  throw new Error("Config read output was not available yet");
}

function runtimeConfigApplyStateSummary(
  state: RuntimeConfigApplyStateRecord | null,
): string {
  if (!state) {
    return "No server-applied runtime sync recorded";
  }
  if (state.pending_status === "failed") {
    const job = state.pending_job_id
      ? ` job ${shortId(state.pending_job_id)}`
      : "";
    const error = state.pending_error ? `: ${state.pending_error}` : "";
    return `Runtime sync failed${job}${error}`;
  }
  if (state.pending_status === "queued") {
    const job = state.pending_job_id
      ? ` job ${shortId(state.pending_job_id)}`
      : "";
    const version = state.pending_version ? ` v${state.pending_version}` : "";
    if (runtimeConfigQueuedStateIsStale(state)) {
      return `Runtime sync stale${version}${job}; queued since ${formatTime(configApplyStateTime(state))}`;
    }
    return `Runtime sync pending${version}${job}`;
  }
  if (state.applied_content_hash) {
    const version = state.applied_version ? ` v${state.applied_version}` : "";
    const job = state.applied_job_id
      ? ` job ${shortId(state.applied_job_id)}`
      : "";
    const when = state.applied_at ? ` ${formatTime(state.applied_at)}` : "";
    return `Runtime config applied${version}${job}${when}; hash ${shortId(state.applied_content_hash)}`;
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
