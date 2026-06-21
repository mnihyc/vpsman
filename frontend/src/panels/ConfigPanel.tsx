import { useEffect, useLayoutEffect, useMemo, useState, type FormEvent } from "react";
import { FileSliders, Play, RefreshCw, Save, ServerCog, Trash2 } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
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
  clampCommandTimeoutSecs,
  clampInteger,
  MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
} from "./jobDispatchModel";
import type {
  AgentView,
  AssignDataSourcePresetRequest,
  AssignDataSourcePresetResponse,
  BulkResolveResponse,
  CloneDataSourcePresetRequest,
  CreateDataSourcePresetRequest,
  CreateJobRequest,
  CreateJobResponse,
  DataSourceHotConfigResponse,
  DataSourcePresetAssignmentRecord,
  DataSourcePresetDiffRequest,
  DataSourcePresetDiffResponse,
  DataSourcePresetRecord,
  DataSourcePresetTestRequest,
  DataSourcePresetTestResponse,
  DataSourceStatusRecord,
  DeleteHotConfigRuleTemplateRequest,
  HotConfigRuleTemplateRecord,
  HotConfigRuleTemplateRenderResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JsonValue,
  PrivilegeAssertion,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
  UpsertHotConfigRuleTemplateRequest,
} from "../types";
import { formatTime, formatVpsName, runPanelAction, shortId } from "../utils";
import { DataSourcePresetPanel } from "./DataSourcePresetPanel";

const CONFIG_BULK_SELECTOR_STORAGE_KEY = "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY = "vpsman.config.single.selectorExpression";
const CONFIG_SINGLE_CLIENT_ID_STORAGE_KEY = "vpsman.config.single.clientId";

type BulkConfigApplySnapshot = {
  jobId: string;
  selectorExpression: string;
  clientIds: string[];
  targets: AgentView[];
  rendered: HotConfigRuleTemplateRenderResponse;
  operation: JobOperation;
  templateName: string;
  timeoutSecs: number;
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
  timeoutSecs: number;
  toml: string;
};

export function ConfigPanel({
  activeSubpage,
  agents,
  dataSourceAssignments,
  dataSourcePresets,
  dataSourceStatus,
  error,
  hotConfigRuleTemplates,
  jobs,
  loading,
  onAssignDataSourcePreset,
  onCloneDataSourcePreset,
  onCreateJob,
  onCreateDataSourcePreset,
  onDiffDataSourcePreset,
  onLoadJobOutputs,
  onLoadJobTargets,
  onDeleteHotConfigRuleTemplate,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRefresh,
  onRenderDataSourceHotConfig,
  onRenderHotConfigRuleTemplate,
  onResolveBulk,
  onTestDataSourcePreset,
  onUpdateDataSourcePreset,
  onUpsertHotConfigRuleTemplate,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  dataSourceAssignments: DataSourcePresetAssignmentRecord[];
  dataSourcePresets: DataSourcePresetRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  error: string | null;
  hotConfigRuleTemplates: HotConfigRuleTemplateRecord[];
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
  loading: boolean;
  onAssignDataSourcePreset: (request: AssignDataSourcePresetRequest) => Promise<AssignDataSourcePresetResponse>;
  onCloneDataSourcePreset: (presetId: string, request: CloneDataSourcePresetRequest) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateDataSourcePreset: (request: CreateDataSourcePresetRequest) => Promise<void>;
  onDiffDataSourcePreset: (presetId: string, request: DataSourcePresetDiffRequest) => Promise<DataSourcePresetDiffResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onDeleteHotConfigRuleTemplate: (
    templateId: string,
    request: DeleteHotConfigRuleTemplateRequest,
  ) => Promise<void>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => void;
  onRenderDataSourceHotConfig: (clientId: string) => Promise<DataSourceHotConfigResponse>;
  onRenderHotConfigRuleTemplate: (templateId: string, request: { values: JsonValue }) => Promise<HotConfigRuleTemplateRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onTestDataSourcePreset: (presetId: string, request: DataSourcePresetTestRequest) => Promise<DataSourcePresetTestResponse>;
  onUpdateDataSourcePreset: (presetId: string, request: UpdateDataSourcePresetRequest) => Promise<UpdateDataSourcePresetResponse>;
  onUpsertHotConfigRuleTemplate: (request: UpsertHotConfigRuleTemplateRequest) => Promise<HotConfigRuleTemplateRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const subpage = ["overview", "rules", "bulk", "single", "templates", "status"].includes(activeSubpage)
    ? activeSubpage
    : "overview";

  if (subpage === "templates" || subpage === "status") {
    return (
      <section className="workspace singleColumn">
        <DataSourcePresetPanel
          activeSubpage={subpage === "status" ? "status" : "presets"}
          agents={agents}
          assignments={dataSourceAssignments}
          dataSourceStatus={dataSourceStatus}
          onAssignPreset={onAssignDataSourcePreset}
          onClonePreset={onCloneDataSourcePreset}
          onCreateJob={onCreateJob}
          onCreatePreset={onCreateDataSourcePreset}
          onDiffPreset={onDiffDataSourcePreset}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          onRenderHotConfig={onRenderDataSourceHotConfig}
          onResolveBulk={onResolveBulk}
          onTestPreset={onTestDataSourcePreset}
          onUpdatePreset={onUpdateDataSourcePreset}
          privilegeMaterial={privilegeMaterial}
          presets={dataSourcePresets}
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
            <span>{actionError ?? error ?? (loading ? "Refreshing config state" : "Config console")}</span>
          </div>
          <button className="secondaryAction" disabled={loading || pending} onClick={onRefresh} type="button">
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
        {subpage === "overview" && (
          <ConfigOverview
            dataSourceAssignments={dataSourceAssignments}
            dataSourcePresets={dataSourcePresets}
            dataSourceStatus={dataSourceStatus}
            hotConfigRuleTemplates={hotConfigRuleTemplates}
            jobs={jobs}
          />
        )}
        {subpage === "rules" && (
          <RuleTemplateWorkspace
            hotConfigRuleTemplates={hotConfigRuleTemplates}
            onDeleteHotConfigRuleTemplate={onDeleteHotConfigRuleTemplate}
            onRenderHotConfigRuleTemplate={onRenderHotConfigRuleTemplate}
            onUpsertHotConfigRuleTemplate={onUpsertHotConfigRuleTemplate}
            pending={pending}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
          />
        )}
        {subpage === "bulk" && (
          <BulkConfigApply
            agents={agents}
            hotConfigRuleTemplates={hotConfigRuleTemplates}
            onCreateJob={onCreateJob}
            onLoadJobOutputs={onLoadJobOutputs}
            onLoadJobTargets={onLoadJobTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onRenderHotConfigRuleTemplate={onRenderHotConfigRuleTemplate}
            onResolveBulk={onResolveBulk}
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
  dataSourceAssignments,
  dataSourcePresets,
  dataSourceStatus,
  hotConfigRuleTemplates,
  jobs,
}: {
  dataSourceAssignments: DataSourcePresetAssignmentRecord[];
  dataSourcePresets: DataSourcePresetRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  hotConfigRuleTemplates: HotConfigRuleTemplateRecord[];
  jobs: Array<{ id: string; command_type: string; status: string; created_at: string }>;
}) {
  const configJobs = jobs
    .filter((job) => ["config_read", "hot_config", "data_source_config_patch"].includes(job.command_type))
    .slice(0, 5);
  return (
    <>
      <div className="metricGrid">
        <div className="metricCard">
          <strong>{hotConfigRuleTemplates.length}</strong>
          <span>rule templates</span>
        </div>
        <div className="metricCard">
          <strong>{dataSourcePresets.length}</strong>
          <span>data-source presets</span>
        </div>
        <div className="metricCard">
          <strong>{dataSourceAssignments.length}</strong>
          <span>preset assignments</span>
        </div>
        <div className="metricCard">
          <strong>{dataSourceStatus.filter((row) => row.status !== "ok").length}</strong>
          <span>source checks needing review</span>
        </div>
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

function RuleTemplateWorkspace({
  hotConfigRuleTemplates,
  onDeleteHotConfigRuleTemplate,
  onRenderHotConfigRuleTemplate,
  onUpsertHotConfigRuleTemplate,
  pending,
  runAction,
}: {
  hotConfigRuleTemplates: HotConfigRuleTemplateRecord[];
  onDeleteHotConfigRuleTemplate: (
    templateId: string,
    request: DeleteHotConfigRuleTemplateRequest,
  ) => Promise<void>;
  onRenderHotConfigRuleTemplate: (templateId: string, request: { values: JsonValue }) => Promise<HotConfigRuleTemplateRenderResponse>;
  onUpsertHotConfigRuleTemplate: (request: UpsertHotConfigRuleTemplateRequest) => Promise<HotConfigRuleTemplateRecord>;
  pending: boolean;
  runAction: (action: () => Promise<void>) => Promise<void>;
}) {
  const [selectedId, setSelectedId] = useState("");
  const [valuesText, setValuesText] = useState("");
  const [rendered, setRendered] = useState<HotConfigRuleTemplateRenderResponse | null>(null);
  const [deleteTemplate, setDeleteTemplate] = useState<HotConfigRuleTemplateRecord | null>(null);
  const selected = hotConfigRuleTemplates.find((template) => template.id === (selectedId || hotConfigRuleTemplates[0]?.id));
  const categories = Array.from(new Set(hotConfigRuleTemplates.map((template) => template.category))).sort();

  useEffect(() => {
    if (selected) {
      setValuesText(formatJsonObject(exampleValuesForTemplate(selected)));
    }
    setDeleteTemplate(null);
  }, [selected?.id]);

  async function renderSelected() {
    if (!selected) {
      return;
    }
    await runAction(async () => {
      setRendered(await onRenderHotConfigRuleTemplate(selected.id, { values: parseJsonObject(valuesText) }));
    });
  }

  async function cloneSelected() {
    if (!selected) {
      return;
    }
    await runAction(async () => {
      await onUpsertHotConfigRuleTemplate({
        category: selected.category,
        description: selected.description,
        docs_metadata: selected.docs_metadata,
        domain: selected.domain,
        field_schema: selected.field_schema,
        name: `${selected.name}.copy`,
        raw_generator_body: selected.raw_generator_body,
        confirmed: true,
      });
    });
  }

  async function deleteSelected() {
    if (!deleteTemplate) {
      return;
    }
    const templateId = deleteTemplate.id;
    const reviewedName = deleteTemplate.name;
    await runAction(async () => {
      await onDeleteHotConfigRuleTemplate(templateId, {
        confirmed: true,
        reviewed_name: reviewedName,
      });
      setSelectedId("");
      setRendered(null);
      setDeleteTemplate(null);
    });
  }

  return (
    <div className="configRuleWorkspace">
      <div className="ruleCardGrid">
        {categories.map((category) => (
          <div className="ruleCategory" key={category}>
            <strong>{category}</strong>
            {hotConfigRuleTemplates
              .filter((template) => template.category === category)
              .map((template) => (
                <button
                  className={`ruleCard ${selected?.id === template.id ? "activeAction" : ""}`}
                  key={template.id}
                  onClick={() => {
                    setSelectedId(template.id);
                    setRendered(null);
                  }}
                  type="button"
                >
                  <span>
                    <strong>{template.name}</strong>
                    <small>{template.description}</small>
                  </span>
                  <em>{template.built_in ? "predefined" : "custom"}</em>
                </button>
              ))}
          </div>
        ))}
      </div>
      <div className="compactForm ruleEditor">
        <strong>{selected?.name ?? "Rule template"}</strong>
        {selected && (
          <details>
            <summary>Template docs</summary>
            <pre>{JSON.stringify({ schema: selected.field_schema, docs: selected.docs_metadata }, null, 2)}</pre>
          </details>
        )}
        <textarea aria-label="Rule render values JSON" onChange={(event) => setValuesText(event.target.value)} rows={8} value={valuesText} />
        <div className="formRow">
          <button className="secondaryAction" disabled={pending || !selected} onClick={renderSelected} type="button">
            <Play size={15} />
            Render patch
          </button>
          <button className="secondaryAction" disabled={pending || !selected} onClick={cloneSelected} type="button">
            Clone template
          </button>
          <button
            className="secondaryAction dangerAction"
            disabled={pending || !selected || selected.built_in}
            onClick={() => selected && setDeleteTemplate(selected)}
            type="button"
            title={
              selected?.built_in
                ? "Predefined templates are immutable; clone before editing or deleting"
                : "Review deletion"
            }
          >
            <Trash2 size={15} />
            Review deletion
          </button>
        </div>
        <ConfirmationPrompt
          confirmLabel="Delete template"
          detail="This removes the reviewed operator-managed rule template. Predefined templates are immutable; clone them before editing."
          items={[
            { label: "Template", value: deleteTemplate?.name ?? "" },
            { label: "Domain", value: deleteTemplate?.domain ?? "" },
          ]}
          onCancel={() => setDeleteTemplate(null)}
          onConfirm={() => void deleteSelected()}
          open={deleteTemplate !== null}
          pending={pending}
          title="Delete rule template"
          tone="danger"
        />
        {rendered && (
          <div className="configPreview">
            <div className="previewMeta">
              <span>{rendered.affected_sections.join(", ")}</span>
              <span>{formatTime(rendered.generated_at)}</span>
            </div>
            <textarea aria-label="Rendered rule patch TOML" readOnly rows={10} value={rendered.toml} />
          </div>
        )}
      </div>
    </div>
  );
}

function BulkConfigApply({
  agents,
  hotConfigRuleTemplates,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onRenderHotConfigRuleTemplate,
  onResolveBulk,
  pending,
  privilegeMaterial,
  runAction,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  hotConfigRuleTemplates: HotConfigRuleTemplateRecord[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onRenderHotConfigRuleTemplate: (templateId: string, request: { values: JsonValue }) => Promise<HotConfigRuleTemplateRenderResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY));
  const [templateId, setTemplateId] = useState("");
  const [valuesText, setValuesText] = useState("");
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [rendered, setRendered] = useState<HotConfigRuleTemplateRenderResponse | null>(null);
  const [applySnapshot, setApplySnapshot] = useState<BulkConfigApplySnapshot | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [timeoutSecs, setTimeoutSecs] = useState(30);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectedTemplate = hotConfigRuleTemplates.find((template) => template.id === (templateId || hotConfigRuleTemplates[0]?.id));
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);
  const ready = Boolean(selectedTemplate && selectorExpression.trim() && privilegeMaterial && !selectorParse.error);

  useEffect(() => writeLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  useLayoutEffect(() => {
    if (selectedTemplate) {
      setValuesText(formatJsonObject(exampleValuesForTemplate(selectedTemplate)));
      setRendered(null);
      clearBulkConfigReview();
    }
  }, [selectedTemplate?.id]);

  function clearBulkConfigReview() {
    invalidateReviewGeneration();
    setApplySnapshot(null);
    setConfirmOpen(false);
    setReviewStatus(null);
  }

  async function previewTargets() {
    clearBulkConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenSelector = selectorExpression.trim();
    setReviewStatus("Resolving config targets");
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
    if (!selectedTemplate) {
      return;
    }
    clearBulkConfigReview();
    const reviewGeneration = captureReviewGeneration();
    const frozenTemplateId = selectedTemplate.id;
    const frozenValuesText = valuesText;
    setReviewStatus("Rendering config patch");
    try {
      await runAction(async () => {
        const frozenValues = parseJsonObject(frozenValuesText);
        await waitForReviewRender();
        const nextRendered = await onRenderHotConfigRuleTemplate(frozenTemplateId, { values: frozenValues });
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
    const frozenTemplate = selectedTemplate;
    const frozenPrivilegeMaterial = privilegeMaterial;
    const frozenSelector = selectorExpression.trim();
    const frozenValuesText = valuesText;
    const boundedTimeoutSecs = clampCommandTimeoutSecs(timeoutSecs);
    setReviewStatus("Preparing bulk config review");
    try {
      await runAction(async () => {
        const frozenValues = parseJsonObject(frozenValuesText);
        await waitForReviewRender();
        if (!frozenTemplate || !frozenPrivilegeMaterial) {
          throw new Error("Bulk config apply is incomplete");
        }
        if (selectorParse.error) {
          throw new Error(selectorParse.error);
        }
        if (!frozenSelector) {
          throw new Error("Add at least one target selector");
        }
        const nextPreview = await onResolveBulk(frozenSelector);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const clientIds = nextPreview.targets.map((target) => target.id);
        if (!clientIds.length) {
          throw new Error("Bulk config confirmation resolved no VPSs");
        }
        const nextRendered = await onRenderHotConfigRuleTemplate(frozenTemplate.id, { values: frozenValues });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const operation: JobOperation = {
          type: "data_source_config_patch",
          apply_mode: "incremental_patch",
          toml: nextRendered.toml,
        };
        const built = await buildPrivilegeForJobOperation({
          clientIds,
          commandType: "data_source_config_patch",
          operation,
          privilegeMaterial: frozenPrivilegeMaterial,
          selectorExpression: frozenSelector,
          timeoutSecs: boundedTimeoutSecs,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(nextPreview);
        setRendered(nextRendered);
        setApplySnapshot({
          clientIds,
          jobId: crypto.randomUUID(),
          operation,
          payloadHashHex: built.payloadHashHex,
          privilegeAssertion: built.privilegeAssertion,
          rendered: nextRendered,
          selectorExpression: frozenSelector,
          targets: nextPreview.targets,
          templateName: frozenTemplate.name,
          timeoutSecs: boundedTimeoutSecs,
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
        throw new Error("Bulk config confirmation snapshot is missing; review the apply again");
      }
      const response = await onCreateJob({
        argv: [],
        command: "data_source_config_patch",
        confirmed: true,
        destructive: true,
        force_unprivileged: false,
        job_id: snapshot.jobId,
        operation: snapshot.operation,
        privileged: true,
        privilege_assertion: snapshot.privilegeAssertion,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.clientIds,
        timeout_secs: snapshot.timeoutSecs,
      });
      const initial = buildBulkJobProgress({
        targetCount: createJobTargetCount(response),
        jobId: response.job_id,
        targetRecords: [],
        targets: snapshot.targets,
        timeoutSecs: snapshot.timeoutSecs,
      });
      setProgress(initial);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        onProgress: setProgress,
        targets: snapshot.targets,
        timeoutSecs: snapshot.timeoutSecs,
      });
      const outputs = await onLoadJobOutputs(response.job_id).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          targetCount: createJobTargetCount(response),
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: snapshot.targets,
          timeoutSecs: snapshot.timeoutSecs,
        }),
      );
      setApplySnapshot(null);
    });
  }

  return (
    <div className="configApplyGrid">
      <div className="compactForm">
        <strong>Patch source</strong>
        <select
          aria-label="Rule template"
          onChange={(event) => {
            setTemplateId(event.target.value);
            clearBulkConfigReview();
          }}
          value={selectedTemplate?.id ?? ""}
        >
          {hotConfigRuleTemplates.map((template) => (
            <option key={template.id} value={template.id}>
              {template.name}
            </option>
          ))}
        </select>
        <textarea
          aria-label="Rule values JSON"
          onChange={(event) => {
            setValuesText(event.target.value);
            setRendered(null);
            clearBulkConfigReview();
          }}
          rows={7}
          value={valuesText}
        />
        <button className="secondaryAction" disabled={pending || !selectedTemplate} onClick={renderPatch} type="button">
          Render patch
        </button>
        {rendered && <textarea aria-label="Bulk rendered incremental config patch" readOnly rows={8} value={rendered.toml} />}
      </div>
      <div className="compactForm">
        <strong>Targets</strong>
        <SearchExpressionInput
          agents={agents}
          ariaLabel="Bulk config selector expression"
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
          <input
            aria-label="Config apply timeout seconds"
            max={MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS}
            min={1}
            onChange={(event) => {
              setTimeoutSecs(Number(event.target.value));
              clearBulkConfigReview();
            }}
            type="number"
            value={timeoutSecs}
          />
        </div>
        <PrivilegeVaultBox
          labelPrefix="Config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={(material) => {
            setPrivilegeMaterial(material);
            clearBulkConfigReview();
          }}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Unlock config privilege"
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
        confirmLabel="Apply config patch"
        detail={`Apply one generated partial patch to ${applySnapshot?.clientIds.length ?? 0} frozen VPS targets.`}
        expiresAtUnix={applySnapshot?.privilegeAssertion.expires_unix}
        items={[
          { label: "Selector", value: applySnapshot?.selectorExpression ?? "-" },
          { label: "Targets", value: `${applySnapshot?.clientIds.length ?? 0}` },
          { label: "Template", value: applySnapshot?.templateName ?? "-" },
          { label: "Sections", value: applySnapshot?.rendered.affected_sections.join(", ") ?? "-" },
          { label: "Payload", value: applySnapshot?.payloadHashHex ? shortId(applySnapshot.payloadHashHex) : "-" },
        ]}
        onCancel={() => {
          setConfirmOpen(false);
          setApplySnapshot(null);
        }}
        onConfirm={() => void applyPatch()}
        open={confirmOpen}
        pending={pending}
        title="Confirm bulk config apply"
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
  const [timeoutSecs, setTimeoutSecs] = useState(30);
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
    const boundedTimeoutSecs = clampCommandTimeoutSecs(timeoutSecs);
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
        timeoutSecs: boundedTimeoutSecs,
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
        timeout_secs: boundedTimeoutSecs,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        onProgress: setProgress,
        targets: [frozenTarget],
        timeoutSecs: boundedTimeoutSecs,
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
          timeoutSecs: boundedTimeoutSecs,
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
    const boundedTimeoutSecs = clampCommandTimeoutSecs(timeoutSecs);
    setReviewStatus("Preparing single config review");
    try {
      await runAction(async () => {
        await waitForReviewRender();
        if (!frozenTarget || !frozenPrivilegeMaterial || !frozenToml || !frozenBaseHash) {
          throw new Error("Read a single VPS config before applying");
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
          timeoutSecs: boundedTimeoutSecs,
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
          timeoutSecs: boundedTimeoutSecs,
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
        timeout_secs: snapshot.timeoutSecs,
      });
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        onProgress: setProgress,
        targets: [snapshot.target],
        timeoutSecs: snapshot.timeoutSecs,
      });
      const outputs = await onLoadJobOutputs(response.job_id).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          targetCount: createJobTargetCount(response),
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: [snapshot.target],
          timeoutSecs: snapshot.timeoutSecs,
        }),
      );
    });
  }

  return (
    <div className="configApplyGrid">
      <div className="compactForm">
        <strong>Single VPS target</strong>
        <VpsCombobox
          agents={agents}
          ariaLabel="Single VPS config target"
          onChange={selectClientId}
          placeholder="Search config VPS"
          value={clientId}
        />
        <span>{singleTarget ? formatVpsName(singleTarget, vpsNameDisplayMode) : clientId ? "Select a listed VPS" : "no target selected"}</span>
        <div className="inlinePrivilege">
          <input
            aria-label="Single config timeout seconds"
            max={MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS}
            min={1}
            onChange={(event) => {
              clearSingleConfigReview();
              setTimeoutSecs(Number(event.target.value));
            }}
            type="number"
            value={timeoutSecs}
          />
        </div>
        <PrivilegeVaultBox
          labelPrefix="Config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={(material) => {
            clearSingleConfigReview();
            setPrivilegeMaterial(material);
          }}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Unlock config privilege"
        />
        <button className="secondaryAction" disabled={pending || !singleTarget || !privilegeMaterial} onClick={readConfig} type="button">
          <ServerCog size={16} />
          Read config
        </button>
        {lastJobId && (
          <button className="secondaryAction" onClick={() => onOpenJobDetails(lastJobId)} type="button">
            Open job {shortId(lastJobId)}
          </button>
        )}
      </div>
      <div className="compactForm configTomlEditor">
        <strong>Redacted TOML</strong>
        <span>{baseHash ? `base ${shortId(baseHash)}` : "Read a single VPS config before editing"}</span>
        <textarea
          aria-label="Single VPS redacted config TOML"
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
        confirmLabel="Apply config"
        detail="Apply the redacted-preserve TOML only if the VPS config hash still matches the read base."
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
        title="Confirm single-VPS config apply"
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
    case "rules":
      return "Config rules";
    case "bulk":
      return "Bulk config patches";
    case "single":
      return "Single VPS config";
    default:
      return "Config overview";
  }
}

function parseJsonObject(value: string): JsonValue {
  const parsed = JSON.parse(value) as JsonValue;
  if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("Values must be a JSON object");
  }
  return parsed;
}

function exampleValuesForTemplate(template: HotConfigRuleTemplateRecord): Record<string, JsonValue> {
  const schema = asRecord(template.field_schema) ?? {};
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
