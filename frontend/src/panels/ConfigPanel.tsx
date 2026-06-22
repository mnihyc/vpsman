import { useEffect, useLayoutEffect, useMemo, useState, type FormEvent } from "react";
import { FileSliders, Play, RefreshCw, Save, ServerCog, Trash2 } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { usePanelDisplaySettings } from "../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../privilege";
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
  CloneSourceTemplateRequest,
  CreateSourceTemplateRequest,
  CreateJobRequest,
  CreateJobResponse,
  SourceConfigPatchResponse,
  SourceTemplateAssignmentRecord,
  SourceTemplateDiffRequest,
  SourceTemplateDiffResponse,
  SourceTemplateRecord,
  SourceTemplateTestRequest,
  SourceTemplateTestResponse,
  SourceStatusRecord,
  DeleteHotConfigPatchGeneratorRequest,
  HotConfigPatchGeneratorRecord,
  HotConfigPatchGeneratorRenderResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JsonValue,
  PrivilegeAssertion,
  UpdateSourceTemplateRequest,
  UpdateSourceTemplateResponse,
  UpsertHotConfigPatchGeneratorRequest,
} from "../types";
import { formatTime, formatVpsName, runPanelAction, shortId } from "../utils";
import { SourceTemplatePanel } from "./SourceTemplatesPanel";

const CONFIG_BULK_SELECTOR_STORAGE_KEY = "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY = "vpsman.config.single.selectorExpression";
const CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY = "vpsman.config.single.clientId";

type BulkConfigApplySnapshot = {
  jobId: string;
  selectorExpression: string;
  clientIds: string[];
  targets: AgentView[];
  operation: JobOperation;
  patchName: string;
  patchSections: string[];
  patchSource: "generator" | "temporary";
  maxTimeoutSecs: number;
  privilegeAssertion: PrivilegeAssertion;
  payloadHashHex: string;
};

type SingleConfigApplySnapshot = {
  baseHash: string;
  clientId: string;
  jobId: string;
  operation: JobOperation;
  payloadHashHex: string;
  privilegeAssertion: PrivilegeAssertion;
  selectorExpression: string;
  target: AgentView;
  maxTimeoutSecs: number;
  toml: string;
};

export function ConfigPanel({
  activeSubpage,
  agents,
  sourceTemplateAssignments,
  sourceTemplates,
  sourceStatus,
  error,
  hotConfigPatchGenerators,
  jobs,
  loading,
  onAssignSourceTemplate,
  onCloneSourceTemplate,
  onCreateJob,
  onCreateSourceTemplate,
  onDiffSourceTemplate,
  onLoadJobOutputs,
  onLoadJobTargets,
  onDeleteHotConfigPatchGenerator,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRefresh,
  onRenderSourceConfigPatch,
  onRenderHotConfigPatchGenerator,
  onResolveBulk,
  onSelectSubpage,
  onTestSourceTemplate,
  onUpdateSourceTemplate,
  onUpsertHotConfigPatchGenerator,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceTemplates: SourceTemplateRecord[];
  sourceStatus: SourceStatusRecord[];
  error: string | null;
  hotConfigPatchGenerators: HotConfigPatchGeneratorRecord[];
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  loading: boolean;
  onAssignSourceTemplate: (request: AssignSourceTemplateRequest) => Promise<AssignSourceTemplateResponse>;
  onCloneSourceTemplate: (templateId: string, request: CloneSourceTemplateRequest) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateSourceTemplate: (request: CreateSourceTemplateRequest) => Promise<void>;
  onDiffSourceTemplate: (templateId: string, request: SourceTemplateDiffRequest) => Promise<SourceTemplateDiffResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onDeleteHotConfigPatchGenerator: (
    generatorId: string,
    request: DeleteHotConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => void;
  onRenderSourceConfigPatch: (clientId: string) => Promise<SourceConfigPatchResponse>;
  onRenderHotConfigPatchGenerator: (generatorId: string, request: { values: JsonValue }) => Promise<HotConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onSelectSubpage: (subpage: string) => void;
  onTestSourceTemplate: (templateId: string, request: SourceTemplateTestRequest) => Promise<SourceTemplateTestResponse>;
  onUpdateSourceTemplate: (templateId: string, request: UpdateSourceTemplateRequest) => Promise<UpdateSourceTemplateResponse>;
  onUpsertHotConfigPatchGenerator: (request: UpsertHotConfigPatchGeneratorRequest) => Promise<HotConfigPatchGeneratorRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const subpage = normalizeConfigSubpage(activeSubpage);

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
          onCreateJob={onCreateJob}
          onCreateTemplate={onCreateSourceTemplate}
          onDiffTemplate={onDiffSourceTemplate}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          onRenderHotConfig={onRenderSourceConfigPatch}
          onResolveBulk={onResolveBulk}
          onTestTemplate={onTestSourceTemplate}
          onUpdateTemplate={onUpdateSourceTemplate}
          privilegeMaterial={privilegeMaterial}
          templates={sourceTemplates}
          setPrivilegeMaterial={setPrivilegeMaterial}
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
            <span>{actionError ?? error ?? (loading ? "Refreshing agent config state" : "Agent config workflows")}</span>
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
            hotConfigPatchGenerators={hotConfigPatchGenerators}
            jobs={jobs}
            onSelectSubpage={onSelectSubpage}
          />
        )}
        {subpage === "bulk" && (
          <BulkConfigApply
            agents={agents}
            hotConfigPatchGenerators={hotConfigPatchGenerators}
            onDeleteHotConfigPatchGenerator={onDeleteHotConfigPatchGenerator}
            onCreateJob={onCreateJob}
            onLoadJobOutputs={onLoadJobOutputs}
            onLoadJobTargets={onLoadJobTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onRenderHotConfigPatchGenerator={onRenderHotConfigPatchGenerator}
            onResolveBulk={onResolveBulk}
            onUpsertHotConfigPatchGenerator={onUpsertHotConfigPatchGenerator}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        )}
        {subpage === "single" && (
          <SingleVpsConfig
            agents={agents}
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
      </div>
    </section>
  );
}

function ConfigOverview({
  sourceTemplateAssignments,
  sourceTemplates,
  sourceStatus,
  hotConfigPatchGenerators,
  jobs,
  onSelectSubpage,
}: {
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  sourceTemplates: SourceTemplateRecord[];
  sourceStatus: SourceStatusRecord[];
  hotConfigPatchGenerators: HotConfigPatchGeneratorRecord[];
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  onSelectSubpage: (subpage: string) => void;
}) {
  const configJobs = jobs
    .filter((job) => ["config_read", "hot_config", "source_config_patch"].includes(job.command_type))
    .slice(0, 5);
  const sourceIssues = sourceStatus.filter((row) => row.status !== "ok").length;
  const workflowCards = [
    {
      action: "Read / edit VPS config",
      detail: "Base-hash guarded full override",
      subpage: "single",
      title: "VPS config",
      value: "Read one VPS config, edit redacted TOML, then apply a guarded full override.",
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
      detail: `${hotConfigPatchGenerators.length} saved generators`,
      subpage: "bulk",
      title: "Patch generators",
      value: "Reusable generators that render temporary incremental patches.",
    },
    {
      action: "Manage source templates",
      detail: `${sourceTemplates.length} templates / ${sourceTemplateAssignments.length} assignments`,
      subpage: "templates",
      title: "Source templates",
      value: "Persistent source definitions bound to VPSs and manually applied.",
    },
    {
      action: "Inspect source status",
      detail: `${sourceIssues} needing review`,
      subpage: "templates",
      title: "Source status",
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
      meaning: "Persistent source definitions assigned to VPSs and applied after review.",
    },
    {
      term: "Command templates",
      meaning: "Saved job payloads managed under Jobs and Schedules.",
    },
    {
      term: "Rules",
      meaning: "Alert and webhook matching logic under Fleet, not agent config.",
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
          <strong>{hotConfigPatchGenerators.length}</strong>
          <span>patch generators</span>
        </div>
        <div className="metricCard">
          <strong>{sourceTemplates.length}</strong>
          <span>source templates</span>
        </div>
        <div className="metricCard">
          <strong>{sourceTemplateAssignments.length}</strong>
          <span>template assignments</span>
        </div>
        <div className="metricCard">
          <strong>{sourceIssues}</strong>
          <span>source checks needing review</span>
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
      <div className="configTermMap" aria-label="Agent config terminology">
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
  hotConfigPatchGenerators,
  onDeleteHotConfigPatchGenerator,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRenderHotConfigPatchGenerator,
  onResolveBulk,
  onUpsertHotConfigPatchGenerator,
  pending,
  privilegeMaterial,
  runAction,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  hotConfigPatchGenerators: HotConfigPatchGeneratorRecord[];
  onDeleteHotConfigPatchGenerator: (
    generatorId: string,
    request: DeleteHotConfigPatchGeneratorRequest,
  ) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRenderHotConfigPatchGenerator: (generatorId: string, request: { values: JsonValue }) => Promise<HotConfigPatchGeneratorRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onUpsertHotConfigPatchGenerator: (request: UpsertHotConfigPatchGeneratorRequest) => Promise<HotConfigPatchGeneratorRecord>;
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
  const [rendered, setRendered] = useState<HotConfigPatchGeneratorRenderResponse | null>(null);
  const [applySnapshot, setApplySnapshot] = useState<BulkConfigApplySnapshot | null>(null);
  const [deleteGenerator, setDeleteGenerator] = useState<HotConfigPatchGeneratorRecord | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(DEFAULT_MAX_JOB_TIMEOUT_SECS);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectedGenerator = hotConfigPatchGenerators.find((generator) => generator.id === (generatorId || hotConfigPatchGenerators[0]?.id));
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);
  const ready = Boolean(
    selectorExpression.trim() &&
      privilegeMaterial &&
      !selectorParse.error &&
      (patchMode === "temporary" ? temporaryToml.trim() : selectedGenerator),
  );
  const patchGeneratorColumns = useMemo<ConsoleDataGridColumn<HotConfigPatchGeneratorRecord>[]>(
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
  const patchGeneratorActions = useMemo<ConsoleDataGridAction<HotConfigPatchGeneratorRecord>[]>(
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

  function loadPatchGeneratorForApply(generator: HotConfigPatchGeneratorRecord) {
    setPatchMode("generator");
    setGeneratorId(generator.id);
    setValuesText(formatJsonObject(exampleValuesForGenerator(generator)));
    setRendered(null);
    clearBulkConfigReview();
  }

  async function clonePatchGenerator(generator: HotConfigPatchGeneratorRecord) {
    await runAction(async () => {
      await onUpsertHotConfigPatchGenerator({
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
      await onDeleteHotConfigPatchGenerator(generator.id, {
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
    setReviewStatus("Rendering agent config patch");
    try {
      await runAction(async () => {
        const frozenValues = parseJsonObject(frozenValuesText);
        await waitForReviewRender();
        const nextRendered = await onRenderHotConfigPatchGenerator(frozenGeneratorId, { values: frozenValues });
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
          const nextRendered = await onRenderHotConfigPatchGenerator(frozenGenerator!.id, { values: frozenValues });
          if (!isReviewGenerationCurrent(reviewGeneration)) {
            return;
          }
          toml = nextRendered.toml;
          patchName = frozenGenerator!.name;
          patchSections = nextRendered.affected_sections;
          setRendered(nextRendered);
        }
        const operation: JobOperation = {
          type: "source_config_patch",
          apply_mode: "incremental_patch",
          toml,
        };
        const built = await buildPrivilegeForJobOperation({
          clientIds,
          commandType: "source_config_patch",
          operation,
          privilegeMaterial: frozenPrivilegeMaterial,
          selectorExpression: frozenSelector,
          maxTimeoutSecs: boundedMaxTimeoutSecs,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(nextPreview);
        setApplySnapshot({
          clientIds,
          jobId: crypto.randomUUID(),
          operation,
          patchName,
          patchSections,
          patchSource: frozenPatchMode,
          payloadHashHex: built.payloadHashHex,
          privilegeAssertion: built.privilegeAssertion,
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
      const response = await onCreateJob({
        argv: [],
        command: "source_config_patch",
        confirmed: true,
        destructive: true,
        force_unprivileged: false,
        job_id: snapshot.jobId,
        operation: snapshot.operation,
        privileged: true,
        privilege_assertion: snapshot.privilegeAssertion,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.clientIds,
        max_timeout_secs: snapshot.maxTimeoutSecs,
      });
      const initial = buildBulkJobProgress({
        targetCount: createJobTargetCount(response),
        jobId: response.job_id,
        targetRecords: [],
        targets: snapshot.targets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      setProgress(initial);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        onProgress: setProgress,
        targets: snapshot.targets,
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      const outputs = await onLoadJobOutputs(response.job_id).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          targetCount: createJobTargetCount(response),
          jobId: response.job_id,
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
              {hotConfigPatchGenerators.map((generator) => (
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
            {rendered && <textarea aria-label="Rendered bulk agent patch TOML" readOnly rows={8} value={rendered.toml} />}
          </>
        ) : (
          <textarea
            aria-label="Temporary bulk agent patch TOML"
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
          labelPrefix="Agent config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={(material) => {
            setPrivilegeMaterial(material);
            clearBulkConfigReview();
          }}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Unlock agent config privilege"
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
        confirmLabel="Apply agent config patch"
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
        rows={hotConfigPatchGenerators}
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
  const [singleApplySnapshot, setSingleApplySnapshot] = useState<SingleConfigApplySnapshot | null>(null);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const singleTarget = useMemo(() => agents.find((agent) => agent.id === clientId) ?? null, [agents, clientId]);

  useEffect(() => writeLocalString(CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY, clientId), [clientId]);

  function clearSingleConfigReview() {
    invalidateReviewGeneration();
    setSingleApplySnapshot(null);
    setReviewStatus(null);
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
      setSingleApplySnapshot(null);
    });
  }

  async function reviewConfigApply() {
    const reviewGeneration = captureReviewGeneration();
    const frozenTarget = singleTarget;
    const frozenPrivilegeMaterial = privilegeMaterial;
    const frozenToml = redactedToml;
    const frozenBaseHash = baseHash;
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
    setReviewStatus("Preparing VPS config review");
    try {
      await runAction(async () => {
        await waitForReviewRender();
        if (!frozenTarget || !frozenPrivilegeMaterial || !frozenToml || !frozenBaseHash) {
          throw new Error("Read one VPS config before applying");
        }
        const operation: JobOperation = {
          type: "hot_config",
          apply_mode: "full_override",
          toml: frozenToml,
          preserve_redacted: true,
          base_config_sha256_hex: frozenBaseHash,
        };
        const selectorExpressionForTarget = selectorExpressionForClientIds([frozenTarget.id]);
        const built = await buildPrivilegeForJobOperation({
          clientIds: [frozenTarget.id],
          commandType: "hot_config",
          operation,
          privilegeMaterial: frozenPrivilegeMaterial,
          selectorExpression: selectorExpressionForTarget,
          maxTimeoutSecs: boundedMaxTimeoutSecs,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setSingleApplySnapshot({
          baseHash: frozenBaseHash,
          clientId: frozenTarget.id,
          jobId: crypto.randomUUID(),
          operation,
          payloadHashHex: built.payloadHashHex,
          privilegeAssertion: built.privilegeAssertion,
          selectorExpression: selectorExpressionForTarget,
          target: frozenTarget,
          maxTimeoutSecs: boundedMaxTimeoutSecs,
          toml: frozenToml,
        });
      });
    } finally {
      setReviewStatus(null);
    }
  }

  async function applyConfig(snapshot: SingleConfigApplySnapshot) {
    setSingleApplySnapshot(null);
    await runAction(async () => {
      const response = await onCreateJob({
        argv: [],
        command: "hot_config",
        confirmed: true,
        destructive: true,
        force_unprivileged: false,
        job_id: snapshot.jobId,
        operation: snapshot.operation,
        privileged: true,
        privilege_assertion: snapshot.privilegeAssertion,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: [snapshot.clientId],
        max_timeout_secs: snapshot.maxTimeoutSecs,
      });
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        onProgress: setProgress,
        targets: [snapshot.target],
        maxTimeoutSecs: snapshot.maxTimeoutSecs,
      });
      const outputs = await onLoadJobOutputs(response.job_id).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          targetCount: createJobTargetCount(response),
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: [snapshot.target],
          maxTimeoutSecs: snapshot.maxTimeoutSecs,
        }),
      );
    });
  }

  return (
    <div className="configApplyGrid">
      <div className="compactForm">
        <strong>VPS target</strong>
        <VpsCombobox
          agents={agents}
          ariaLabel="VPS config target"
          onChange={selectClientId}
          placeholder="Search VPS config"
          value={clientId}
        />
        <span>{singleTarget ? formatVpsName(singleTarget, vpsNameDisplayMode) : clientId ? "Select a listed VPS" : "no target selected"}</span>
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
          labelPrefix="Agent config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={(material) => {
            clearSingleConfigReview();
            setPrivilegeMaterial(material);
          }}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Unlock agent config privilege"
        />
        <button className="secondaryAction" disabled={pending || !singleTarget || !privilegeMaterial} onClick={readConfig} type="button">
          <ServerCog size={16} />
          Read agent config
        </button>
        {lastJobId && (
          <button className="secondaryAction" onClick={() => onOpenJobDetails(lastJobId)} type="button">
            Open job {shortId(lastJobId)}
          </button>
        )}
      </div>
      <div className="compactForm configTomlEditor">
        <strong>Redacted agent TOML</strong>
        <span>{baseHash ? `base ${shortId(baseHash)}` : "Read one VPS config before editing"}</span>
        <textarea
          aria-label="VPS redacted agent config TOML"
          onChange={(event) => {
            clearSingleConfigReview();
            setRedactedToml(event.target.value);
          }}
          rows={22}
          value={redactedToml}
        />
        <button
          className="primaryAction"
          disabled={pending || !singleTarget || !privilegeMaterial || !baseHash || !redactedToml || singleApplySnapshot !== null}
          onClick={() => void reviewConfigApply()}
          type="button"
        >
          <Save size={16} />
          Review apply
        </button>
        {reviewStatus && <span className="formHint">{reviewStatus}</span>}
      </div>
      <ConfirmationPrompt
        confirmLabel="Apply full config"
        detail="Apply the redacted-preserve agent TOML only if the VPS config hash still matches the read base."
        expiresAtUnix={singleApplySnapshot?.privilegeAssertion.expires_unix}
        items={[
          { label: "Target", value: singleApplySnapshot ? formatVpsName(singleApplySnapshot.target, vpsNameDisplayMode) : "-" },
          { label: "Base hash", value: singleApplySnapshot?.baseHash ?? "-" },
          { label: "Payload", value: singleApplySnapshot?.payloadHashHex ? shortId(singleApplySnapshot.payloadHashHex) : "-" },
          { label: "Policy", value: "preserve redacted fields" },
        ]}
        onCancel={() => setSingleApplySnapshot(null)}
        onConfirm={() => singleApplySnapshot && void applyConfig(singleApplySnapshot)}
        open={singleApplySnapshot !== null}
        pending={pending}
        title="Confirm full config override"
      />
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

function configTitle(subpage: string): string {
  switch (subpage) {
    case "bulk":
      return "Bulk patch";
    case "single":
      return "VPS config";
    default:
      return "Agent config overview";
  }
}

function normalizeConfigSubpage(value: string): "overview" | "bulk" | "single" | "templates" {
  if (value === "bulk" || value === "single" || value === "templates") {
    return value;
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

function exampleValuesForGenerator(generator: HotConfigPatchGeneratorRecord): Record<string, JsonValue> {
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
