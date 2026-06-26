import { useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
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
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
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
  buildPrivilegeForJobOperation,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../privilege";
import { parseSearchExpression, selectorExpressionForClientIds } from "../searchExpression";
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

const CONFIG_BULK_SELECTOR_STORAGE_KEY = "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY = "vpsman.config.single.selectorExpression";
const CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY = "vpsman.config.single.clientId";
const CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY = "vpsman.config.vpsRules.selectorExpression";
const CONFIG_HELP = {
  incrementalPatch: "Incremental TOML patches modify only reviewed runtime keys; bootstrap and server-managed keys stay immutable.",
  patchGenerator: "Saved generators render incremental TOML from reviewed JSON variables before any VPS target is touched.",
  targetSelector: "Selector expressions freeze the exact VPS set for preview and review so later fleet changes cannot silently expand scope.",
  maxTimeout: "Per-target command timeout bounded by the backend so slow agents cannot hold config work indefinitely.",
  redactedRuntimeToml: "Runtime config returned by the agent with secret material removed; the base hash is used to detect stale overrides.",
  guardedOverride: "One-VPS override requires a current base hash, validated TOML sections, payload hash, and privilege assertion before apply.",
  currentBase: "Hash of the redacted config read used to prove the override was reviewed against the current runtime state.",
  sections: "Top-level TOML sections touched by the override; validate before review so the operator sees the blast radius.",
  payload: "Hash of the exact override payload that the confirmation prompt will bind to the privileged request.",
  vpsRules: "Per-VPS traffic rule values feed accounting and alert policies; dry-run previews changed rows before write.",
  ruleSelector: "Fleet selector used for the dry-run and final reviewed VPS rule mutation.",
  ruleSetValues: "Key=value lines become typed VPS rule values after backend validation and dry-run diffing.",
  ruleUnsetValues: "Explicit rule keys removed from every matched VPS after dry-run review.",
  previewHash: "Backend hash of the dry-run diff that the apply request must echo to prevent stale writes.",
} as const;
const VPS_RULE_KEYS = [
  "traffic.reset_day",
  "traffic.quota.total",
  "traffic.quota.rx",
  "traffic.quota.tx",
  "traffic.selectors",
] as const;

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
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  loading: boolean;
  onSubmitRuntimeConfigPatch: (request: RuntimeConfigPatchRequest) => Promise<RuntimeConfigPatchResponse>;
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
  onBulkUnsetVpsRules: (request: VpsRulesBulkUnsetRequest) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsertVpsRules: (request: VpsRulesBulkUpsertRequest) => Promise<VpsRulesDryRunResponse>;
  onDryRunVpsRules: (request: VpsRulesDryRunRequest) => Promise<VpsRulesDryRunResponse>;
  onRenderRuntimeConfigPatchGenerator: (generatorId: string, request: { values: JsonValue }) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onSelectSubpage: (subpage: string) => void;
  onUpsertRuntimeConfigPatchGenerator: (request: UpsertRuntimeConfigPatchGeneratorRequest) => Promise<RuntimeConfigPatchGeneratorRecord>;
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
          <button className="secondaryAction" disabled={loading || pending} onClick={onRefresh} type="button">
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
            onDeleteRuntimeConfigPatchGenerator={onDeleteRuntimeConfigPatchGenerator}
            onCreateJob={onCreateJob}
            onLoadJobOutputs={onLoadJobOutputs}
            onLoadJobTargets={onLoadJobTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onRenderRuntimeConfigPatchGenerator={onRenderRuntimeConfigPatchGenerator}
            onResolveBulk={onResolveBulk}
            onUpsertRuntimeConfigPatchGenerator={onUpsertRuntimeConfigPatchGenerator}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
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
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
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
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  onSelectSubpage: (subpage: string) => void;
}) {
  const agentNameById = new Map(agents.map((agent) => [agent.id, agent.display_name]));
  const configJobs = jobs
    .filter((job) => ["config_read", "runtime_config_sync"].includes(job.command_type))
    .slice(0, 5);
  const sourceRiskRows = sourceStatus.filter((row) => !isReadySourceStatus(row.status));
  const sourceReadyRows = sourceStatus.length - sourceRiskRows.length;
  const pendingSyncs = runtimeConfigApplyStates.filter((state) => state.pending_status === "queued").length;
  const failedSyncs = runtimeConfigApplyStates.filter((state) => state.pending_status === "failed").length;
  const appliedClientIds = new Set(
    runtimeConfigApplyStates
      .filter((state) => Boolean(state.applied_content_hash))
      .map((state) => state.client_id),
  );
  const assignedClientIds = new Set(sourceTemplateAssignments.map((assignment) => assignment.client_id));
  const missingApplyStates = Math.max(agents.length - appliedClientIds.size, 0);
  const missingTemplateCoverage = Math.max(agents.length - assignedClientIds.size, 0);
  const customTemplateCount = sourceTemplates.filter((template) => !template.built_in).length;
  const invalidRuleRows = vpsRuleValues.filter((row) => row.state !== "ok").length;
  const configHealth = configHealthStatus({
    failedSyncs,
    invalidRuleRows,
    missingApplyStates,
    missingTemplateCoverage,
    pendingSyncs,
    sourceRiskCount: sourceRiskRows.length,
  });
  const latestApplyStates = runtimeConfigApplyStates
    .slice()
    .sort((left, right) => configApplyStateTime(right).localeCompare(configApplyStateTime(left)))
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
      detail: "Resolve target scope, render a patch, unlock privilege, and review apply.",
      subpage: "bulk_patch",
      title: "Bulk patch",
    },
    {
      action: "Open Templates",
      detail: "Review coverage and assignments; persistent authoring belongs to Source templates.",
      subpage: "templates",
      title: "Templates",
    },
    {
      action: "Open Rules",
      detail: "Dry-run traffic and accounting rule values before they affect policy context.",
      subpage: "rules",
      title: "Rules",
    },
  ];
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
          <ConsoleStatusBadge tone={configHealth.tone}>{configHealth.label}</ConsoleStatusBadge>
        </div>
        <div className="configHealthSummary">
          <span>
            <strong>{failedSyncs}</strong>
            <small>failed runtime syncs</small>
          </span>
          <span>
            <strong>{pendingSyncs}</strong>
            <small>queued runtime syncs</small>
          </span>
          <span>
            <strong>{sourceRiskRows.length}</strong>
            <small>source checks needing review</small>
          </span>
          <span>
            <strong>{invalidRuleRows}</strong>
            <small>rule rows needing review</small>
          </span>
        </div>
        <p>{configHealth.detail}</p>
      </section>

      <div className="configOverviewColumns">
        <section className="configOverviewBlock" aria-label="Config drift summary">
          <div className="configOverviewBlockHeader">
            <h3>Drift summary</h3>
            <ConsoleStatusBadge tone={sourceRiskRows.length || failedSyncs ? "warning" : "ok"}>
              {sourceRiskRows.length + failedSyncs + pendingSyncs} open signals
            </ConsoleStatusBadge>
          </div>
          <div className="configRiskList">
            <ConfigOverviewRiskRow
              detail={`${failedSyncs} failed, ${pendingSyncs} queued, ${missingApplyStates} without applied-state evidence`}
              label="Runtime apply drift"
              tone={failedSyncs ? "critical" : pendingSyncs || missingApplyStates ? "warning" : "ok"}
              value={failedSyncs + pendingSyncs + missingApplyStates}
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
              detail={`${invalidRuleRows} of ${vpsRuleValues.length} traffic/accounting rule rows are not ok`}
              label="Rule validation drift"
              tone={invalidRuleRows ? "warning" : "ok"}
              value={invalidRuleRows}
            />
          </div>
        </section>

        <section className="configOverviewBlock" aria-label="Config template coverage">
          <div className="configOverviewBlockHeader">
            <h3>Template coverage</h3>
            <ConsoleStatusBadge tone={missingTemplateCoverage ? "warning" : "ok"}>
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

        <section className="configOverviewBlock" aria-label="Config apply-state summary">
          <div className="configOverviewBlockHeader">
            <h3>Apply-state summary</h3>
            <ConsoleStatusBadge tone={failedSyncs ? "critical" : pendingSyncs ? "warning" : "ok"}>
              {appliedClientIds.size}/{agents.length || 0} applied
            </ConsoleStatusBadge>
          </div>
          <div className="configCoverageGrid">
            <span>
              <strong>{appliedClientIds.size}</strong>
              <small>Applied runtime state</small>
            </span>
            <span>
              <strong>{pendingSyncs}</strong>
              <small>Queued apply</small>
            </span>
            <span>
              <strong>{failedSyncs}</strong>
              <small>Failed apply</small>
            </span>
            <span>
              <strong>{runtimeConfigPatchGenerators.length}</strong>
              <small>Patch generators available</small>
            </span>
          </div>
          <p>
            Apply evidence is per VPS. Operators should inspect one target or
            run a reviewed bulk patch instead of editing from the overview.
          </p>
        </section>
      </div>

      <section className="configWorkflowLinks" aria-label="Config overview workflow links">
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

      <section className="configOverviewBlock" aria-label="Recent config changes">
        <div className="configOverviewBlockHeader">
          <h3>Recent changes</h3>
          <span>{recentChanges.length} runtime config records</span>
        </div>
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
              <span><ConsoleStatusBadge tone={change.tone}>{change.status}</ConsoleStatusBadge></span>
              <span>{change.detail}</span>
              <span>{formatTime(change.time)}</span>
            </div>
          ))}
          {recentChanges.length === 0 && <div className="emptyState compactEmpty">No recent config changes.</div>}
        </div>
      </section>
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

type ConfigTemplateCoverageRow = {
  assignedClients: number;
  assignments: number;
  attentionChecks: number;
  defaultTemplate: string;
  domain: string;
  readyChecks: number;
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
  const readyStatusCount = sourceStatus.filter((row) => isReadySourceStatus(row.status)).length;
  const attentionStatusRows = sourceStatus.filter((row) => !isReadySourceStatus(row.status));
  const customTemplateCount = templates.filter((template) => !template.built_in).length;
  const coverageRows = useMemo<ConfigTemplateCoverageRow[]>(() => {
    const domains = Array.from(
      new Set([
        ...templates.map((template) => template.domain),
        ...assignments.map((assignment) => assignment.domain),
        ...sourceStatus.map((row) => row.domain),
      ]),
    ).sort((left, right) => left.localeCompare(right));
    return domains.map((domain) => {
      const domainTemplates = templates.filter((template) => template.domain === domain);
      const domainAssignments = assignments.filter((assignment) => assignment.domain === domain);
      const domainStatus = sourceStatus.filter((row) => row.domain === domain);
      const defaultTemplate =
        domainTemplates.find((template) => template.is_default)?.name ??
        domainTemplates[0]?.name ??
        "No template";
      const updatedAt = domainTemplates
        .map((template) => template.updated_at)
        .filter(Boolean)
        .sort((left, right) => right.localeCompare(left))[0] ??
        domainAssignments
          .map((assignment) => assignment.assigned_at)
          .filter(Boolean)
          .sort((left, right) => right.localeCompare(left))[0] ??
        "";
      return {
        assignedClients: new Set(domainAssignments.map((assignment) => assignment.client_id)).size,
        assignments: domainAssignments.length,
        attentionChecks: domainStatus.filter((row) => !isReadySourceStatus(row.status)).length,
        defaultTemplate,
        domain,
        readyChecks: domainStatus.filter((row) => isReadySourceStatus(row.status)).length,
        templates: domainTemplates.length,
        updatedAt,
      };
    });
  }, [assignments, sourceStatus, templates]);
  const coverageColumns = useMemo<ConsoleDataGridColumn<ConfigTemplateCoverageRow>[]>(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.domain}</strong>
            <small>{row.defaultTemplate}</small>
          </span>
        ),
        header: "Domain",
        id: "domain",
        minSize: 220,
        searchValue: (row) => `${row.domain} ${row.defaultTemplate}`,
        sortValue: (row) => row.domain,
      },
      {
        cell: (row) => row.templates,
        header: "Templates",
        id: "templates",
        sortValue: (row) => row.templates,
      },
      {
        cell: (row) => `${row.assignedClients} VPSs / ${row.assignments} rows`,
        header: "Assignments",
        id: "assignments",
        searchValue: (row) => `${row.assignedClients} ${row.assignments}`,
        sortValue: (row) => row.assignedClients,
      },
      {
        cell: (row) => (
          <ConsoleStatusBadge tone={row.attentionChecks > 0 ? "warning" : "ok"}>
            {row.readyChecks} ready / {row.attentionChecks} review
          </ConsoleStatusBadge>
        ),
        header: "Readiness",
        id: "readiness",
        searchValue: (row) => `${row.readyChecks} ready ${row.attentionChecks} review`,
        sortValue: (row) => row.attentionChecks,
      },
      {
        cell: (row) => (row.updatedAt ? formatTime(row.updatedAt) : "No update evidence"),
        header: "Latest update",
        id: "updated",
        searchValue: (row) => row.updatedAt,
        sortValue: (row) => row.updatedAt,
      },
    ],
    [],
  );
  return (
    <div className="configOverviewStack configTemplateSummary" aria-label="Config template summary">
      <section className="configHealthPanel" aria-label="Config template coverage summary">
        <div className="configHealthHeader">
          <div>
            <h3>Template coverage</h3>
            <span>
              Read-only runtime template posture for Config. Persistent template
              authoring lives in Automation / Source Templates.
            </span>
          </div>
          <ConsoleStatusBadge tone={attentionStatusRows.length > 0 ? "warning" : "ok"}>
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
            <strong>{assignedClientIds.size}/{agents.length}</strong>
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

      <section className="configOverviewBlock configTemplateCanonical" aria-label="Source template canonical home">
        <div className="configOverviewBlockHeader">
          <h3>Canonical authoring</h3>
          <span>Automation / Source Templates</span>
        </div>
        <p>
          Config shows coverage and source readiness only. Create, clone, diff,
          test, update, assign, and render persistent source templates in the
          Automation workflow so emergency patches and persistent authoring stay
          separate.
        </p>
        <button className="primaryAction" onClick={onOpenSourceTemplates} type="button">
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
            <span>Domain</span>
            <strong>{row.domain}</strong>
            <span>Default template</span>
            <strong>{row.defaultTemplate}</strong>
            <span>Template records</span>
            <strong>{row.templates}</strong>
            <span>Assignment records</span>
            <strong>{row.assignments}</strong>
            <span>Readiness checks</span>
            <strong>{row.readyChecks + row.attentionChecks}</strong>
          </div>
        )}
        rows={coverageRows}
        searchPlaceholder="Search template domains"
        selectable={false}
        storageKey="vpsman.config.templateSummary.domains"
        title="Template domain coverage"
      />

      <section className="configOverviewBlock" aria-label="Config source readiness exceptions">
        <div className="configOverviewBlockHeader">
          <h3>Source readiness exceptions</h3>
          <span>{attentionStatusRows.length} records need review</span>
        </div>
        <div className="configRiskList">
          {attentionStatusRows.slice(0, 6).map((row) => (
            <div className="configRiskRow" key={`${row.client_id}:${row.domain}`}>
              <span>
                <strong>{formatVpsName(row, vpsNameDisplayMode)} / {row.domain}</strong>
                <small>{row.template_name} / {row.status_reason}</small>
              </span>
              <ConsoleStatusBadge tone="warning">{row.status}</ConsoleStatusBadge>
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

function isReadySourceStatus(status: SourceStatusRecord["status"]): boolean {
  return ["ok", "ready", "ready_on_demand", "selected"].includes(status);
}

function configHealthStatus({
  failedSyncs,
  invalidRuleRows,
  missingApplyStates,
  missingTemplateCoverage,
  pendingSyncs,
  sourceRiskCount,
}: {
  failedSyncs: number;
  invalidRuleRows: number;
  missingApplyStates: number;
  missingTemplateCoverage: number;
  pendingSyncs: number;
  sourceRiskCount: number;
}): { detail: string; label: string; tone: "critical" | "warning" | "ok" } {
  if (failedSyncs > 0) {
    return {
      detail: `${failedSyncs} runtime syncs failed. Review the affected VPS before relying on generated config or traffic policy state.`,
      label: "Action required",
      tone: "critical",
    };
  }
  if (pendingSyncs > 0 || sourceRiskCount > 0 || invalidRuleRows > 0 || missingApplyStates > 0 || missingTemplateCoverage > 0) {
    return {
      detail: `${pendingSyncs} syncs queued, ${sourceRiskCount} source checks need review, and ${missingTemplateCoverage} VPSs lack template assignment evidence.`,
      label: "Needs review",
      tone: "warning",
    };
  }
  return {
    detail: "All loaded VPSs have applied runtime state, source readiness, template assignment evidence, and valid rule rows.",
    label: "Healthy",
    tone: "ok",
  };
}

function configApplyStateTime(state: RuntimeConfigApplyStateRecord): string {
  return state.pending_updated_at ?? state.applied_at ?? state.updated_at;
}

function runtimeConfigApplyStatusLabel(state: RuntimeConfigApplyStateRecord): string {
  if (state.pending_status === "failed") {
    return "failed";
  }
  if (state.pending_status === "queued") {
    return "queued";
  }
  if (state.applied_content_hash) {
    return "applied";
  }
  return "missing";
}

function runtimeConfigApplyTone(
  state: RuntimeConfigApplyStateRecord,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  if (state.pending_status === "failed") {
    return "critical";
  }
  if (state.pending_status === "queued") {
    return "warning";
  }
  if (state.applied_content_hash) {
    return "ok";
  }
  return "neutral";
}

function configJobStatusTone(status: string): "critical" | "warning" | "ok" | "info" | "neutral" {
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
  onSubmitRuntimeConfigPatch: (request: RuntimeConfigPatchRequest) => Promise<RuntimeConfigPatchResponse>;
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRenderRuntimeConfigPatchGenerator: (generatorId: string, request: { values: JsonValue }) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onUpsertRuntimeConfigPatchGenerator: (request: UpsertRuntimeConfigPatchGeneratorRequest) => Promise<RuntimeConfigPatchGeneratorRecord>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY));
  const [patchMode, setPatchMode] = useState<"generator" | "temporary">("generator");
  const [generatorId, setGeneratorId] = useState("");
  const [valuesText, setValuesText] = useState("");
  const [temporaryToml, setTemporaryToml] = useState("");
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [rendered, setRendered] = useState<RuntimeConfigPatchGeneratorRenderResponse | null>(null);
  const [applySnapshot, setApplySnapshot] = useState<BulkConfigApplySnapshot | null>(null);
  const [deleteGenerator, setDeleteGenerator] = useState<RuntimeConfigPatchGeneratorRecord | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(DEFAULT_MAX_JOB_TIMEOUT_SECS);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectedGenerator = runtimeConfigPatchGenerators.find((generator) => generator.id === (generatorId || runtimeConfigPatchGenerators[0]?.id));
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);
  const ready = Boolean(
    selectorExpression.trim() &&
      privilegeMaterial &&
      !selectorParse.error &&
      (patchMode === "temporary" ? temporaryToml.trim() : selectedGenerator),
  );
  const patchGeneratorColumns = useMemo<ConsoleDataGridColumn<RuntimeConfigPatchGeneratorRecord>[]>(
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
        searchValue: (generator) => `${generator.name} ${generator.description}`,
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
        searchValue: (generator) => (generator.built_in ? "built-in" : "custom"),
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
  const patchGeneratorActions = useMemo<ConsoleDataGridAction<RuntimeConfigPatchGeneratorRecord>[]>(
    () => [
      {
        icon: <Play size={14} />,
        label: "Load",
        onSelect: (rows) => loadPatchGeneratorForApply(rows[0]),
        disabled: (rows) => rows.length !== 1,
        description: (rows) => `Load ${rows[0]?.name ?? "one patch generator"} into the apply form.`,
      },
      {
        label: "Clone",
        onSelect: (rows) => void clonePatchGenerator(rows[0]),
        disabled: (rows) => rows.length !== 1,
        description: (rows) => `Clone ${rows[0]?.name ?? "one patch generator"} for editing outside built-ins.`,
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

  useEffect(() => writeLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  useLayoutEffect(() => {
    if (selectedGenerator) {
      setValuesText(formatJsonObject(exampleValuesForGenerator(selectedGenerator)));
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

  function loadPatchGeneratorForApply(generator: RuntimeConfigPatchGeneratorRecord) {
    setPatchMode("generator");
    setGeneratorId(generator.id);
    setValuesText(formatJsonObject(exampleValuesForGenerator(generator)));
    setRendered(null);
    clearBulkConfigReview();
  }

  async function clonePatchGenerator(generator: RuntimeConfigPatchGeneratorRecord) {
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

  async function previewTargets() {
    clearBulkConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenSelector = selectorExpression.trim();
    setReviewStatus("Resolving patch targets");
    try {
      await runAction(async () => {
        await waitForReviewRender();
        if (selectorParse.error) {
          throw new Error(selectorParse.error);
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

  async function renderPatch() {
    if (patchMode !== "generator" || !selectedGenerator) {
      return;
    }
    clearBulkConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenGeneratorId = selectedGenerator.id;
    const frozenValuesText = valuesText;
    setReviewStatus("Rendering runtime config patch");
    try {
      await runAction(async () => {
        const frozenValues = parseJsonObject(frozenValuesText);
        await waitForReviewRender();
        const nextRendered = await onRenderRuntimeConfigPatchGenerator(frozenGeneratorId, { values: frozenValues });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setRendered(nextRendered);
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
          const nextRendered = await onRenderRuntimeConfigPatchGenerator(frozenGenerator!.id, { values: frozenValues });
          if (!isReviewGenerationCurrent(reviewGeneration)) {
            return;
          }
          toml = nextRendered.toml;
          patchName = frozenGenerator!.name;
          patchSections = nextRendered.affected_sections;
          setRendered(nextRendered);
        }
        const patchPayloadHashHex = await sha256Hex(new TextEncoder().encode(toml));
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
        throw new Error("Bulk patch confirmation snapshot is missing; review the apply again");
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
      <div className="compactForm">
        <ConfigHelpLabel help={CONFIG_HELP.incrementalPatch} label="Incremental patch" strong />
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
            Temporary
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
            <button
              className="secondaryAction"
              disabled={pending || !selectedGenerator}
              onClick={renderPatch}
              title={
                pending
                  ? "Wait for the current config operation to finish before rendering a patch."
                  : !selectedGenerator
                    ? "Select a saved generator before rendering a patch."
                    : "Render the saved generator into incremental TOML."
              }
              type="button"
            >
              Render patch
            </button>
            {rendered && <textarea aria-label="Rendered bulk runtime config patch TOML" readOnly rows={8} value={rendered.toml} />}
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
      </div>
      <div className="compactForm">
        <ConfigHelpLabel help={CONFIG_HELP.targetSelector} label="Targets" strong />
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
          verification={selectorParse.error ? "invalid" : selectorExpression.trim() ? "valid" : "neutral"}
          verificationMessage={selectorParse.error ?? (preview ? `${preview.target_count}/${agents.length}` : selectorExpression.trim() ? undefined : "no selector")}
        />
        <button
          className="secondaryAction"
          disabled={pending || !selectorExpression.trim()}
          onClick={previewTargets}
          title={
            pending
              ? "Wait for the current config operation to finish before reviewing targets."
              : !selectorExpression.trim()
                ? "Enter a target selector expression before previewing matched VPSs."
                : "Preview and freeze matched VPS targets for review."
          }
          type="button"
        >
          Review targets
        </button>
        <div className="targetChipList">
          {(preview?.targets ?? []).slice(0, 24).map((agent) => (
            <span className="targetChip" key={agent.id} title={agent.id}>
              {agent.display_name}
            </span>
          ))}
          {preview && preview.target_count > 24 && <span className="targetChip mutedChip">+{preview.target_count - 24} more</span>}
        </div>
        <div className="inlinePrivilege">
          <label>
            <ConfigHelpLabel help={CONFIG_HELP.maxTimeout} label="Max timeout seconds" />
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
        </div>
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
        {reviewStatus && <span className="formHint">{reviewStatus}</span>}
        <button
          className="primaryAction"
          disabled={pending || !ready}
          onClick={() => void reviewApply()}
          title={
            pending
              ? "Wait for the current config operation to finish before opening review."
              : !ready
                ? "Render or paste a patch, preview targets, and unlock privilege material before review."
                : "Open the reviewed runtime config apply prompt."
          }
          type="button"
        >
          <FileSliders size={16} />
          Review apply
        </button>
      </div>
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
          { label: "Selector", value: applySnapshot?.selectorExpression ?? "-" },
          { label: "Targets", value: `${applySnapshot?.clientIds.length ?? 0}` },
          { label: "Source", value: applySnapshot?.patchSource ?? "-" },
          { label: "Patch", value: applySnapshot?.patchName ?? "-" },
          { label: "Sections", value: applySnapshot?.patchSections.join(", ") ?? "-" },
          { label: "Payload", value: applySnapshot?.payloadHashHex ? shortId(applySnapshot.payloadHashHex) : "-" },
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
  onSubmitRuntimeConfigPatch: (request: RuntimeConfigPatchRequest) => Promise<RuntimeConfigPatchResponse>;
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
  const [overrideValidation, setOverrideValidation] =
    useState<{ sections: string[]; payloadHashHex: string } | null>(null);
  const [applySnapshot, setApplySnapshot] =
    useState<SingleVpsConfigApplySnapshot | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [lastJobId, setLastJobId] = useState<string | null>(null);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(DEFAULT_MAX_JOB_TIMEOUT_SECS);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const singleTarget = useMemo(() => agents.find((agent) => agent.id === clientId) ?? null, [agents, clientId]);
  const runtimeApplyState = useMemo(
    () => runtimeConfigApplyStates.find((state) => state.client_id === clientId) ?? null,
    [clientId, runtimeConfigApplyStates],
  );
  const overrideReady = Boolean(singleTarget && privilegeMaterial && baseHash && overrideToml.trim());

  useEffect(() => {
    clientIdRef.current = clientId;
    writeLocalString(CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY, clientId);
  }, [clientId]);

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
  }

  async function validateOverride() {
    const reviewGeneration = captureReviewGeneration();
    const frozenTarget = singleTarget;
    const frozenToml = overrideToml.trim();
    const frozenBaseHash = baseHash;
    setApplySnapshot(null);
    setConfirmOpen(false);
    setReviewStatus("Validating one-VPS override");
    await runAction(async () => {
      await waitForReviewRender();
      if (!frozenTarget) {
        throw new Error("Select one VPS before validating an override");
      }
      if (!frozenBaseHash) {
        throw new Error("Read the current VPS config before validating an override");
      }
      if (!frozenToml) {
        throw new Error("Paste a one-VPS runtime config override");
      }
      const sections = inferTomlSections(frozenToml);
      const payloadHashHex = await sha256Hex(new TextEncoder().encode(frozenToml));
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setOverrideValidation({ sections, payloadHashHex });
      setReviewStatus(`Validated ${sections.join(", ")} against base ${shortId(frozenBaseHash)}`);
    });
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
        throw new Error("Read the current VPS config before applying an override");
      }
      if (!frozenToml) {
        throw new Error("Paste a one-VPS runtime config override");
      }
      const selectorExpression = selectorExpressionForClientIds([frozenTarget.id]);
      const patchSections = inferTomlSections(frozenToml);
      const payloadHashHex = await sha256Hex(new TextEncoder().encode(frozenToml));
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
        throw new Error("One-VPS override snapshot is missing; review the apply again");
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
    const frozenPrivilegeMaterial = privilegeMaterial;
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
    await runAction(async () => {
      if (!frozenTarget || !frozenPrivilegeMaterial) {
        throw new Error("Select one VPS and unlock privilege");
      }
      const operation: JobOperation = { type: "config_read" };
      const selectorExpressionForTarget = selectorExpressionForClientIds([frozenTarget.id]);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [frozenTarget.id],
        commandType: "config_read",
        operation,
        privilegeMaterial: frozenPrivilegeMaterial,
        selectorExpression: selectorExpressionForTarget,
        maxTimeoutSecs: boundedMaxTimeoutSecs,
      });
      const response = await onCreateJob({
        argv: [],
        command: "config_read",
        confirmed: true,
        destructive: false,
        force_unprivileged: false,
        job_id: crypto.randomUUID(),
        operation,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
        selector_expression: selectorExpressionForTarget,
        target_client_ids: [frozenTarget.id],
        max_timeout_secs: boundedMaxTimeoutSecs,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        onProgress: setProgress,
        targets: [frozenTarget],
        maxTimeoutSecs: boundedMaxTimeoutSecs,
      });
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
    });
  }

  return (
    <div className="configApplyGrid">
      <div className="compactForm">
        <ConfigHelpLabel help={CONFIG_HELP.targetSelector} label="VPS target" strong />
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
            {singleTarget ? formatVpsName(singleTarget, vpsNameDisplayMode) : clientId ? "Select a listed VPS" : "no target selected"}
          </span>
          <span>{runtimeConfigApplyStateSummary(runtimeApplyState)}</span>
        </div>
        <div className="inlinePrivilege">
          <label>
            <ConfigHelpLabel help={CONFIG_HELP.maxTimeout} label="Max timeout seconds" />
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
        </div>
        <PrivilegeVaultBox
          labelPrefix="Runtime config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={(material) => {
            clearSingleConfigReview();
            setPrivilegeMaterial(material);
          }}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Open Privilege Vault for runtime config"
        />
        <button
          className="secondaryAction"
          disabled={pending || !singleTarget || !privilegeMaterial}
          onClick={readConfig}
          title={
            pending
              ? "Wait for the current config operation to finish before reading runtime config."
              : !singleTarget
                ? "Select one VPS before reading runtime config."
                : !privilegeMaterial
                  ? "Unlock privilege material before reading runtime config."
                  : "Read redacted runtime config from the selected VPS."
          }
          type="button"
        >
          <ServerCog size={16} />
          Read runtime config
        </button>
        {lastJobId && (
          <button className="secondaryAction" onClick={() => onOpenJobDetails(lastJobId)} type="button">
            Open job {shortId(lastJobId)}
          </button>
        )}
      </div>
      <div className="compactForm configTomlEditor">
        <ConfigHelpLabel help={CONFIG_HELP.redactedRuntimeToml} label="Redacted runtime TOML" strong />
        <span>{baseHash ? `base ${shortId(baseHash)} / immutable bootstrap view` : "Read one VPS config before viewing"}</span>
        <textarea
          aria-label="VPS redacted runtime config TOML"
          readOnly
          rows={22}
          value={redactedToml}
        />
        <span className="formHint">
          This is the current redacted base used to review a one-VPS override.
        </span>
      </div>
      <div className="compactForm configTomlEditor configOverrideEditor">
        <ConfigHelpLabel help={CONFIG_HELP.guardedOverride} label="Guarded one-VPS override" strong />
        <span>
          Applies one incremental TOML patch to the selected VPS only after the
          current config base and privilege assertion are reviewed.
        </span>
        <div className="configOverrideSummary" aria-label="One-VPS config override guard">
          <span>
            <strong>Exact target</strong>
            <small>{singleTarget ? formatVpsName(singleTarget, vpsNameDisplayMode) : "Select one VPS"}</small>
          </span>
          <span>
            <strong title={CONFIG_HELP.currentBase}>Current base</strong>
            <small>{baseHash ? shortId(baseHash) : "Read required"}</small>
          </span>
          <span>
            <strong title={CONFIG_HELP.sections}>Sections</strong>
            <small>{overrideValidation?.sections.join(", ") || "Validate required"}</small>
          </span>
          <span>
            <strong title={CONFIG_HELP.payload}>Payload</strong>
            <small>{overrideValidation?.payloadHashHex ? shortId(overrideValidation.payloadHashHex) : "Not reviewed"}</small>
          </span>
        </div>
        <textarea
          aria-label="One-VPS runtime config override TOML"
          onChange={(event) => {
            clearSingleConfigReview();
            setOverrideToml(event.target.value);
          }}
          placeholder="[update]\n# one incremental override for this VPS"
          rows={12}
          value={overrideToml}
        />
        {reviewStatus && <span className="formHint">{reviewStatus}</span>}
        <div className="configOverrideActions">
          <button
            className="secondaryAction"
            disabled={pending || !singleTarget || !baseHash || !overrideToml.trim()}
            onClick={() => void validateOverride()}
            title={
              pending
                ? "Wait for the current config operation to finish before validating the override."
                : !singleTarget
                  ? "Select one VPS before validating the override."
                  : !baseHash
                    ? "Read the selected VPS runtime config before validating the override."
                    : !overrideToml.trim()
                      ? "Enter one incremental TOML override before validation."
                      : "Validate the override against the current base config."
            }
            type="button"
          >
            Validate override
          </button>
          <button
            className="primaryAction"
            disabled={pending || !overrideReady}
            onClick={() => void reviewOverrideApply()}
            title={
              pending
                ? "Wait for the current config operation to finish before opening review."
                : !overrideReady
                  ? "Validate the override and unlock privilege material before review."
                  : "Open the reviewed one-VPS config apply prompt."
            }
            type="button"
          >
            <FileSliders size={16} />
            Review one-VPS apply
          </button>
        </div>
      </div>
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
          { label: "Selector", value: applySnapshot?.selectorExpression ?? "-" },
          { label: "Base hash", value: applySnapshot?.baseHash ? shortId(applySnapshot.baseHash) : "-" },
          { label: "Sections", value: applySnapshot?.patchSections.join(", ") ?? "-" },
          { label: "Payload", value: applySnapshot?.payloadHashHex ? shortId(applySnapshot.payloadHashHex) : "-" },
          { label: "Timeout", value: `${applySnapshot?.maxTimeoutSecs ?? maxTimeoutSecs}s` },
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
  preview: VpsRulesDryRunResponse;
};

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
  onBulkUnset: (request: VpsRulesBulkUnsetRequest) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsert: (request: VpsRulesBulkUpsertRequest) => Promise<VpsRulesDryRunResponse>;
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
  const [preview, setPreview] = useState<VpsRulesDryRunResponse | null>(null);
  const [reviewSnapshot, setReviewSnapshot] =
    useState<VpsRulesReviewSnapshot | null>(null);
  const [pending, setPending] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const agentNameById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, formatVpsName(agent, "name_id_suffix")])),
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
      .filter((row) => row.state === "incomplete" || row.incomplete_reasons.length > 0)
      .map((row) => row.client_id),
  );
  const editedRuleKeys = useMemo(
    () => vpsRuleEditKeys(valuesText, unsetKeys),
    [unsetKeys, valuesText],
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
        searchValue: (row) => `${row.client_id} ${agentNameById.get(row.client_id) ?? ""}`,
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
        searchValue: (row) => `${row.validation} ${row.validation_errors.join(" ")}`,
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
    setStatus(operation === "upsert" ? "dry-running set values" : "dry-running unset values");
    try {
      const values = operation === "upsert" ? parseSetValues() : {};
      const keys = operation === "unset" ? unsetKeys : [];
      if (operation === "unset" && keys.length === 0) {
        throw new Error("Select at least one VPS rule key to unset");
      }
      const nextPreview = await onDryRun({
        operation,
        selector_expression: selectorExpression.trim(),
        values,
        keys,
      });
      setPreview(nextPreview);
      setReviewSnapshot({
        operation,
        selectorExpression: selectorExpression.trim(),
        values,
        keys,
        preview: nextPreview,
      });
      setStatus(
        `${operation === "upsert" ? "set" : "unset"} dry-run matched ${nextPreview.matched_vps_count} VPSs`,
      );
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "VPS rules dry-run failed");
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
    setPending(true);
    try {
      const nextPreview =
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
      setPreview(nextPreview);
      setReviewSnapshot(null);
      setStatus(`applied ${nextPreview.changed_row_count} VPS rule rows`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "VPS rules apply failed");
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
          <select value={keyFilter} onChange={(event) => setKeyFilter(event.target.value)}>
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
          <select value={stateFilter} onChange={(event) => setStateFilter(event.target.value)}>
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
      <section className="consoleDetailPanel vpsRulesAlertImpact" aria-label="Affected alert policy context">
        <div className="consoleDetailPanelHeader">
          <span>
            <strong>Affected alert policies</strong>
            <small>
              Policies whose rule conditions reference the current edit keys:
              {" "}
              <span className="monoValue">{editedRuleKeys.join(", ") || "traffic.*"}</span>
            </small>
          </span>
          <button className="secondaryAction compactAction" onClick={onOpenAlerts} type="button">
            Open Observability alerts
          </button>
        </div>
        <div className="configRiskList">
          {affectedPolicyRules.slice(0, 6).map((impact) => (
            <div className="configRiskRow" key={`${impact.policyId}:${impact.ruleId}`}>
              <span>
                <strong>{impact.policyName} / {impact.ruleName}</strong>
                <small className="monoValue">{impact.conditionExpression}</small>
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
              <span>{accountingByClient.get(row.client_id)?.state ?? "unknown"}</span>
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
            <ConfigHelpLabel help={CONFIG_HELP.vpsRules} label="Bulk rule editor" strong />
            <small>Dry-run matched VPSs and changed keys before applying.</small>
          </span>
        </div>
        <div className="vpsRulesBulkEditor">
          <section className="vpsRulesEditorSection">
            <div className="sectionHeader compactHeader">
              <div>
                <h4 title={CONFIG_HELP.ruleSelector}>Target VPS selector</h4>
                <span className="monoValue">{selectorExpression || "unset"}</span>
              </div>
              <div className="consoleOperationsActions">
                <button
                  className="secondaryAction compactAction"
                  disabled={pending}
                  onClick={() => void dryRun("upsert")}
                  title={
                    pending
                      ? "Wait for the current VPS rule operation to finish before dry-run."
                      : "Preview matched VPSs before applying rule changes."
                  }
                  type="button"
                >
                  Dry-run matched VPSs
                </button>
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
                <span className="tokenChip">No dry-run preview</span>
              ) : (
                matchedPreviewClients.map(([clientId, displayName]) => (
                  <span className="tokenChip" key={clientId} title={clientId}>
                    {displayName}
                  </span>
                ))
              )}
            </div>
          </section>
          <section className="vpsRulesEditorSection">
            <div className="sectionHeader compactHeader">
              <div>
                <h4 title={CONFIG_HELP.ruleSetValues}>Set values</h4>
                <span title={CONFIG_HELP.ruleSetValues}>key=value lines</span>
              </div>
              <button
                className="secondaryAction compactAction"
                disabled={pending}
                onClick={() => void dryRun("upsert")}
                title={
                  pending
                    ? "Wait for the current VPS rule operation to finish before dry-run."
                    : "Dry-run typed VPS rule values before applying them."
                }
                type="button"
              >
                Dry-run set values
              </button>
            </div>
            <textarea
              aria-label="VPS rule set values"
              value={valuesText}
              onChange={(event) => setValuesText(event.target.value)}
            />
          </section>
          <section className="vpsRulesEditorSection">
            <div className="sectionHeader compactHeader">
              <div>
                <h4 title={CONFIG_HELP.ruleUnsetValues}>Unset values</h4>
                <span title={CONFIG_HELP.ruleUnsetValues}>Explicit key checklist</span>
              </div>
              <button
                className="secondaryAction compactAction"
                disabled={pending}
                onClick={() => void dryRun("unset")}
                title={
                  pending
                    ? "Wait for the current VPS rule operation to finish before dry-run."
                    : "Dry-run explicit VPS rule removals before applying them."
                }
                type="button"
              >
                Dry-run unset values
              </button>
            </div>
            <div className="checkListPanel compactChecklist">
              {VPS_RULE_KEYS.map((key) => (
                <label className="checkLine" key={key}>
                  <input
                    checked={unsetKeys.includes(key)}
                    onChange={(event) => toggleUnsetKey(key, event.target.checked)}
                    type="checkbox"
                  />
                  <span className="monoValue">{key}</span>
                </label>
              ))}
            </div>
          </section>
        </div>
        {preview ? (
          <VpsRulesPreviewTable columns={previewColumns} preview={preview} />
        ) : null}
        {status && <small className="fleetPolicyStatus">{status}</small>}
      </section>
      <ConfirmationPrompt
        confirmLabel="Apply VPS rules"
        detail="Applies the reviewed dry-run preview by selector and preview hash."
        items={[
          { label: "Selector", value: reviewSnapshot?.selectorExpression ?? "-" },
          { label: "Operation", value: reviewSnapshot?.operation ?? "-" },
          {
            label: "Set keys",
            value: Object.keys(reviewSnapshot?.values ?? {}).join(", ") || "-",
          },
          {
            label: "Unset keys",
            value: reviewSnapshot?.keys.join(", ") || "-",
          },
          { label: "Matched VPSs", value: reviewSnapshot?.preview.matched_vps_count ?? 0 },
          { label: "Changed rows", value: reviewSnapshot?.preview.changed_row_count ?? 0 },
          { label: "Preview hash", value: reviewSnapshot?.preview.preview_hash ?? "-", title: CONFIG_HELP.previewHash },
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

function VpsRulesPreviewTable({
  columns,
  preview,
}: {
  columns: ConsoleDataGridColumn<VpsRuleChangePreview>[];
  preview: VpsRulesDryRunResponse;
}) {
  return (
    <div className="vpsRulesPreviewBlock">
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Matched VPSs</strong>
          <span>{preview.matched_vps_count}</span>
        </span>
        <span>
          <strong>Changed rows</strong>
          <span>{preview.changed_row_count}</span>
        </span>
        <span>
          <strong>Invalid rows</strong>
          <span>{preview.invalid_row_count}</span>
        </span>
        <span>
          <strong title={CONFIG_HELP.previewHash}>Preview hash</strong>
          <span className="monoValue">{preview.preview_hash}</span>
        </span>
      </div>
      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={10}
        empty="No changed rows in dry-run preview."
        getRowId={(change) => `${change.client_id}:${change.key}:${change.action}`}
        itemLabel="changes"
        rows={preview.changes}
        searchPlaceholder="Search dry-run changes"
        selectable={false}
        storageKey="vpsman.grid.config.vpsRules.preview"
        title="Dry-run changed rows"
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
      return "Templates";
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
      return "Persistent runtime config sources and assignments";
    default:
      return "Runtime config workflows";
  }
}

function normalizeConfigSubpage(value: string): "overview" | "bulk" | "single" | "rules" | "templates" {
  const base = value.split(":")[0];
  if (base === "per_vps") {
    return "single";
  }
  if (base === "bulk_patch") {
    return "bulk";
  }
  if (base === "bulk" || base === "single" || base === "rules" || base === "templates") {
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

function exampleValuesForGenerator(generator: RuntimeConfigPatchGeneratorRecord): Record<string, JsonValue> {
  const schema = asRecord(generator.field_schema) ?? {};
  const fields = asRecord(schema.fields) ?? asRecord(schema.properties) ?? {};
  const values: Record<string, JsonValue> = {};
  for (const [field, specValue] of Object.entries(fields)) {
    values[field] = exampleValueFromSchema(asRecord(specValue));
  }
  return values;
}

function exampleValueFromSchema(schema: Record<string, unknown> | null): JsonValue {
  if (!schema) {
    return "";
  }
  if (isJsonValue(schema.default)) {
    return schema.default;
  }
  if (Array.isArray(schema.enum) && schema.enum.length > 0 && isJsonValue(schema.enum[0])) {
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
  if (value === null || ["string", "number", "boolean"].includes(typeof value)) {
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

function extractConfigRead(outputs: JobOutputRecord[]): { toml: string; baseHash: string } {
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    const value = JSON.parse(base64ToText(output.data_base64)) as {
      type?: string;
      toml?: string;
      base_config_sha256_hex?: string;
    };
    if (value.type === "config_read" && value.toml && value.base_config_sha256_hex) {
      return { toml: value.toml, baseHash: value.base_config_sha256_hex };
    }
  }
  throw new Error("Config read output was not available yet");
}

function runtimeConfigApplyStateSummary(state: RuntimeConfigApplyStateRecord | null): string {
  if (!state) {
    return "No server-applied runtime sync recorded";
  }
  if (state.pending_status === "failed") {
    const job = state.pending_job_id ? ` job ${shortId(state.pending_job_id)}` : "";
    const error = state.pending_error ? `: ${state.pending_error}` : "";
    return `Runtime sync failed${job}${error}`;
  }
  if (state.pending_status === "queued") {
    const job = state.pending_job_id ? ` job ${shortId(state.pending_job_id)}` : "";
    const version = state.pending_version ? ` v${state.pending_version}` : "";
    return `Runtime sync pending${version}${job}`;
  }
  if (state.applied_content_hash) {
    const version = state.applied_version ? ` v${state.applied_version}` : "";
    const job = state.applied_job_id ? ` job ${shortId(state.applied_job_id)}` : "";
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
  const storedClientId = readLocalString(CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY).trim();
  if (storedClientId) {
    return storedClientId;
  }
  return clientIdFromLegacySelector(readLocalString(CONFIG_SINGLE_SELECTOR_STORAGE_KEY));
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
