import { useEffect, useLayoutEffect, useMemo, useState, type FormEvent } from "react";
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
  AssignSourceTemplateRequest,
  AssignSourceTemplateResponse,
  BulkResolveResponse,
  RuntimeConfigPatchRequest,
  RuntimeConfigPatchResponse,
  CloneSourceTemplateRequest,
  CreateSourceTemplateRequest,
  CreateJobRequest,
  CreateJobResponse,
  TemplateRuntimeConfigResponse,
  SourceTemplateAssignmentRecord,
  SourceTemplateDiffRequest,
  SourceTemplateDiffResponse,
  SourceTemplateRecord,
  SourceTemplateTestRequest,
  SourceTemplateTestResponse,
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
  UpdateSourceTemplateRequest,
  UpdateSourceTemplateResponse,
  UpsertRuntimeConfigPatchGeneratorRequest,
} from "../types";
import { formatTime, formatVpsName, runPanelAction, shortId } from "../utils";
import { SourceTemplatePanel } from "./SourceTemplatesPanel";

const CONFIG_BULK_SELECTOR_STORAGE_KEY = "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY = "vpsman.config.single.selectorExpression";
const CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY = "vpsman.config.single.clientId";
const CONFIG_VPS_RULES_SELECTOR_STORAGE_KEY = "vpsman.config.vpsRules.selectorExpression";
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
  jobs,
  loading,
  onAssignSourceTemplate,
  onSubmitRuntimeConfigPatch,
  onCloneSourceTemplate,
  onCreateJob,
  onCreateSourceTemplate,
  onDiffSourceTemplate,
  onLoadJobOutputs,
  onLoadJobTargets,
  onDeleteRuntimeConfigPatchGenerator,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRefresh,
  onBulkUnsetVpsRules,
  onBulkUpsertVpsRules,
  onDryRunVpsRules,
  onRenderTemplateRuntimeConfig,
  onRenderRuntimeConfigPatchGenerator,
  onResolveBulk,
  onSelectSubpage,
  onTestSourceTemplate,
  onUpdateSourceTemplate,
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
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  loading: boolean;
  onAssignSourceTemplate: (request: AssignSourceTemplateRequest) => Promise<AssignSourceTemplateResponse>;
  onSubmitRuntimeConfigPatch: (request: RuntimeConfigPatchRequest) => Promise<RuntimeConfigPatchResponse>;
  onCloneSourceTemplate: (templateId: string, request: CloneSourceTemplateRequest) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateSourceTemplate: (request: CreateSourceTemplateRequest) => Promise<void>;
  onDiffSourceTemplate: (templateId: string, request: SourceTemplateDiffRequest) => Promise<SourceTemplateDiffResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onDeleteRuntimeConfigPatchGenerator: (
    generatorId: string,
    request: DeleteRuntimeConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => void;
  onBulkUnsetVpsRules: (request: VpsRulesBulkUnsetRequest) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsertVpsRules: (request: VpsRulesBulkUpsertRequest) => Promise<VpsRulesDryRunResponse>;
  onDryRunVpsRules: (request: VpsRulesDryRunRequest) => Promise<VpsRulesDryRunResponse>;
  onRenderTemplateRuntimeConfig: (clientId: string) => Promise<TemplateRuntimeConfigResponse>;
  onRenderRuntimeConfigPatchGenerator: (generatorId: string, request: { values: JsonValue }) => Promise<RuntimeConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onSelectSubpage: (subpage: string) => void;
  onTestSourceTemplate: (templateId: string, request: SourceTemplateTestRequest) => Promise<SourceTemplateTestResponse>;
  onUpdateSourceTemplate: (templateId: string, request: UpdateSourceTemplateRequest) => Promise<UpdateSourceTemplateResponse>;
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

  if (subpage === "templates") {
    return (
      <section className="workspace singleColumn">
        <SourceTemplatePanel
          activeSubpage="templates"
          agents={agents}
          assignments={sourceTemplateAssignments}
          sourceStatus={sourceStatus}
          onAssignTemplate={onAssignSourceTemplate}
          onCloneTemplate={onCloneSourceTemplate}
          onCreateTemplate={onCreateSourceTemplate}
          onDiffTemplate={onDiffSourceTemplate}
          onRenderTemplateRuntimeConfig={onRenderTemplateRuntimeConfig}
          onResolveBulk={onResolveBulk}
          onTestTemplate={onTestSourceTemplate}
          onUpdateTemplate={onUpdateSourceTemplate}
          templates={sourceTemplates}
        />
      </section>
    );
  }

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
            sourceTemplateAssignments={sourceTemplateAssignments}
            sourceTemplates={sourceTemplates}
            sourceStatus={sourceStatus}
            runtimeConfigApplyStates={runtimeConfigApplyStates}
            runtimeConfigPatchGenerators={runtimeConfigPatchGenerators}
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
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        )}
        {subpage === "rules" && (
          <VpsRulesPanel
            agents={agents}
            initialSelectorExpression={rulesSelectorPrefill}
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
  sourceTemplateAssignments,
  sourceTemplates,
  sourceStatus,
  runtimeConfigApplyStates,
  runtimeConfigPatchGenerators,
  jobs,
  onSelectSubpage,
}: {
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceTemplates: SourceTemplateRecord[];
  sourceStatus: SourceStatusRecord[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  runtimeConfigPatchGenerators: RuntimeConfigPatchGeneratorRecord[];
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  onSelectSubpage: (subpage: string) => void;
}) {
  const configJobs = jobs
    .filter((job) => ["config_read", "runtime_config_sync"].includes(job.command_type))
    .slice(0, 5);
  const sourceIssues = sourceStatus.filter((row) => row.status !== "ok").length;
  const pendingSyncs = runtimeConfigApplyStates.filter((state) => state.pending_status === "queued").length;
  const failedSyncs = runtimeConfigApplyStates.filter((state) => state.pending_status === "failed").length;
  const appliedSyncs = runtimeConfigApplyStates.filter((state) => state.applied_content_hash).length;
  const workflowCards = [
    {
      action: "Read VPS config",
      detail: "Bootstrap-safe runtime view",
      subpage: "single",
      title: "VPS config",
      value: "Read one VPS runtime config without exposing mutable bootstrap credentials.",
    },
    {
      action: "Apply incremental patch",
      detail: "Selector-resolved VPS targets",
      subpage: "bulk",
      title: "Bulk patch",
      value: "Apply one temporary incremental patch to many reviewed VPSs.",
    },
    {
      action: "Manage patch generators",
      detail: `${runtimeConfigPatchGenerators.length} saved generators`,
      subpage: "bulk",
      title: "Patch generators",
      value: "Reusable generators that render temporary incremental patches.",
    },
    {
      action: "Manage templates",
      detail: `${sourceTemplates.length} templates / ${sourceTemplateAssignments.length} assignments`,
      subpage: "templates",
      title: "Templates",
      value: "Persistent runtime inputs assigned to VPSs; changes push immediately after confirmation.",
    },
    {
      action: "Edit VPS rules",
      detail: "Traffic reset, quotas, and selectors",
      subpage: "rules",
      title: "VPS Rules",
      value: "Server-side per-VPS traffic rule values used by accounting and alert policies.",
    },
    {
      action: "Inspect template status",
      detail: `${sourceIssues} needing review`,
      subpage: "templates",
      title: "Template status",
      value: "Current template selection, source kind, readiness, and evidence per VPS.",
    },
  ];
  const termMap = [
    {
      term: "Patch",
      meaning: "A temporary incremental TOML change applied to reviewed VPS targets.",
    },
    {
      term: "Patch generators",
      meaning: "Reusable generators that render temporary patches; they do not bind to VPSs.",
    },
    {
      term: "Templates",
      meaning: "Persistent runtime inputs assigned to VPSs; confirmed changes push immediately.",
    },
    {
      term: "Command templates",
      meaning: "Saved job payloads managed under Jobs and Schedules.",
    },
    {
      term: "Rules",
      meaning: "VPS Rules are per-VPS traffic values; Alert policies under Fleet evaluate them.",
    },
    {
      term: "Suite config",
      meaning: "Control-plane service config managed under System.",
    },
  ];
  return (
    <>
      <div className="metricGrid">
        <div className="metricCard">
          <strong>{runtimeConfigPatchGenerators.length}</strong>
          <span>patch generators</span>
        </div>
        <div className="metricCard">
          <strong>{sourceTemplates.length}</strong>
          <span>templates</span>
        </div>
        <div className="metricCard">
          <strong>{sourceTemplateAssignments.length}</strong>
          <span>template assignments</span>
        </div>
        <div className="metricCard">
          <strong>{sourceIssues}</strong>
          <span>template checks needing review</span>
        </div>
        <div className="metricCard">
          <strong>{pendingSyncs}</strong>
          <span>runtime syncs pending apply</span>
        </div>
        <div className="metricCard">
          <strong>{failedSyncs}</strong>
          <span>runtime syncs failed apply</span>
        </div>
        <div className="metricCard">
          <strong>{appliedSyncs}</strong>
          <span>VPSs with applied runtime state</span>
        </div>
      </div>
      <div className="configWorkflowGrid">
        {workflowCards.map((card) => (
          <button className="configWorkflowCard" key={card.subpage} onClick={() => onSelectSubpage(card.subpage)} type="button">
            <span>
              <strong>{card.title}</strong>
              <small>{card.detail}</small>
            </span>
            <em>{card.value}</em>
            <b>{card.action}</b>
          </button>
        ))}
      </div>
      <div className="configTermMap" aria-label="Runtime config terminology">
        {termMap.map((item) => (
          <span key={item.term} title={item.meaning}>
            <strong>{item.term}</strong>
            <small>{item.meaning}</small>
          </span>
        ))}
      </div>
      <div className="table hierarchyTable">
        <div className="historyRow heading configJobGrid">
          <span>Job</span>
          <span>Operation</span>
          <span>Status</span>
          <span>Created</span>
        </div>
        {configJobs.map((job) => (
          <div className="historyRow configJobGrid" key={job.id}>
            <span>{shortId(job.id)}</span>
            <span>{job.command_type}</span>
            <span>{job.status}</span>
            <span>{formatTime(job.created_at)}</span>
          </div>
        ))}
        {configJobs.length === 0 && <div className="emptyState compactEmpty">No recent config jobs.</div>}
      </div>
    </>
  );
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
        <strong>Incremental patch</strong>
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
            <select
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
            <button className="secondaryAction" disabled={pending || !selectedGenerator} onClick={renderPatch} type="button">
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
        <strong>Targets</strong>
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
        <button className="secondaryAction" disabled={pending || !selectorExpression.trim()} onClick={previewTargets} type="button">
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
            <span>Max timeout seconds</span>
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
          unlockRedirectLabel="Unlock runtime config privilege"
        />
        {reviewStatus && <span className="formHint">{reviewStatus}</span>}
        <button className="primaryAction" disabled={pending || !ready} onClick={() => void reviewApply()} type="button">
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
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [clientId, setClientId] = useState(() => readSingleConfigClientId());
  const [redactedToml, setRedactedToml] = useState("");
  const [baseHash, setBaseHash] = useState("");
  const [lastJobId, setLastJobId] = useState<string | null>(null);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(DEFAULT_MAX_JOB_TIMEOUT_SECS);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
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

  useEffect(() => writeLocalString(CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY, clientId), [clientId]);

  function clearSingleConfigReview() {
    invalidateReviewGeneration();
  }

  function selectClientId(value: string) {
    clearSingleConfigReview();
    setClientId(value);
    setRedactedToml("");
    setBaseHash("");
    setProgress(null);
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
        <strong>VPS target</strong>
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
            <span>Max timeout seconds</span>
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
          unlockRedirectLabel="Unlock runtime config privilege"
        />
        <button className="secondaryAction" disabled={pending || !singleTarget || !privilegeMaterial} onClick={readConfig} type="button">
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
        <strong>Redacted runtime TOML</strong>
        <span>{baseHash ? `base ${shortId(baseHash)} / immutable bootstrap view` : "Read one VPS config before viewing"}</span>
        <textarea
          aria-label="VPS redacted runtime config TOML"
          readOnly
          rows={22}
          value={redactedToml}
        />
        <span className="formHint">
          Runtime changes are made through Bulk patch or template assignment and then pushed as runtime config sync jobs.
        </span>
      </div>
      {progress && (
        <ExecutionResultPanel
          loading={pending}
          onClearResults={() => setProgress(null)}
          onOpenJobDetails={onOpenJobDetails}
          progress={progress}
        />
      )}
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

function VpsRulesPanel({
  agents,
  initialSelectorExpression,
  onBulkUnset,
  onBulkUpsert,
  onDryRun,
  trafficAccounting,
  vpsRuleValues,
}: {
  agents: AgentView[];
  initialSelectorExpression: string | null;
  onBulkUnset: (request: VpsRulesBulkUnsetRequest) => Promise<VpsRulesDryRunResponse>;
  onBulkUpsert: (request: VpsRulesBulkUpsertRequest) => Promise<VpsRulesDryRunResponse>;
  onDryRun: (request: VpsRulesDryRunRequest) => Promise<VpsRulesDryRunResponse>;
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
            <strong>Bulk rule editor</strong>
            <small>Dry-run matched VPSs and changed keys before applying.</small>
          </span>
        </div>
        <div className="vpsRulesBulkEditor">
          <section className="vpsRulesEditorSection">
            <div className="sectionHeader compactHeader">
              <div>
                <h4>Target VPS selector</h4>
                <span className="monoValue">{selectorExpression || "unset"}</span>
              </div>
              <div className="consoleOperationsActions">
                <button
                  className="secondaryAction compactAction"
                  disabled={pending}
                  onClick={() => void dryRun("upsert")}
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
                <h4>Set values</h4>
                <span>key=value lines</span>
              </div>
              <button
                className="secondaryAction compactAction"
                disabled={pending}
                onClick={() => void dryRun("upsert")}
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
                <h4>Unset values</h4>
                <span>Explicit key checklist</span>
              </div>
              <button
                className="secondaryAction compactAction"
                disabled={pending}
                onClick={() => void dryRun("unset")}
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
          { label: "Preview hash", value: reviewSnapshot?.preview.preview_hash ?? "-" },
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
          <strong>Preview hash</strong>
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
      return "VPS config";
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
