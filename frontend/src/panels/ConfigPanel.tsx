import { useEffect, useMemo, useState, type FormEvent } from "react";
import { FileSliders, Play, RefreshCw, Save, ServerCog, Trash2 } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import {
  buildBulkJobProgress,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { usePanelDisplaySettings } from "../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../privilege";
import { parseSearchExpression, selectorExpressionForClientIds } from "../searchExpression";
import { clampInteger } from "./jobDispatchModel";
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
  HotConfigRuleTemplateRecord,
  HotConfigRuleTemplateRenderResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JsonValue,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
  UpsertHotConfigRuleTemplateRequest,
} from "../types";
import { formatTime, formatVpsName, runPanelAction, shortId } from "../utils";
import { DataSourcePresetPanel } from "./DataSourcePresetPanel";

const CONFIG_BULK_SELECTOR_STORAGE_KEY = "vpsman.config.bulk.selectorExpression";
const CONFIG_SINGLE_SELECTOR_STORAGE_KEY = "vpsman.config.single.selectorExpression";

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
  onDeleteHotConfigRuleTemplate: (templateId: string) => Promise<void>;
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
            <span>{actionError ?? error ?? (loading ? "Refreshing config state" : "Hot config console")}</span>
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
            onResolveBulk={onResolveBulk}
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
  onDeleteHotConfigRuleTemplate: (templateId: string) => Promise<void>;
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
      });
    });
  }

  async function deleteSelected() {
    if (!deleteTemplate) {
      return;
    }
    const templateId = deleteTemplate.id;
    await runAction(async () => {
      await onDeleteHotConfigRuleTemplate(templateId);
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
                  <em>{template.built_in ? "built-in" : "custom"}</em>
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
          >
            <Trash2 size={15} />
            Review deletion
          </button>
        </div>
        <ConfirmationPrompt
          confirmLabel="Delete template"
          detail="This removes the shared custom rule template. Built-in templates remain managed by the server."
          items={[
            { label: "Template", value: deleteTemplate?.name ?? "" },
            { label: "Domain", value: deleteTemplate?.domain ?? "" },
          ]}
          onCancel={() => setDeleteTemplate(null)}
          onConfirm={() => void deleteSelected()}
          open={deleteTemplate !== null}
          pending={pending}
          title="Delete custom rule template"
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
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [timeoutSecs, setTimeoutSecs] = useState(30);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const selectedTemplate = hotConfigRuleTemplates.find((template) => template.id === (templateId || hotConfigRuleTemplates[0]?.id));
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);
  const ready = Boolean(selectedTemplate && preview?.target_count && rendered && privilegeMaterial && !selectorParse.error);

  useEffect(() => writeLocalString(CONFIG_BULK_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  useEffect(() => {
    if (selectedTemplate) {
      setValuesText(formatJsonObject(exampleValuesForTemplate(selectedTemplate)));
      setRendered(null);
    }
  }, [selectedTemplate?.id]);

  async function previewTargets() {
    await runAction(async () => {
      if (selectorParse.error) {
        throw new Error(selectorParse.error);
      }
      setPreview(await onResolveBulk(selectorExpression.trim()));
    });
  }

  async function renderPatch() {
    if (!selectedTemplate) {
      return;
    }
    await runAction(async () => {
      setRendered(await onRenderHotConfigRuleTemplate(selectedTemplate.id, { values: parseJsonObject(valuesText) }));
    });
  }

  async function applyPatch() {
    setConfirmOpen(false);
    await runAction(async () => {
      if (!ready || !preview || !rendered || !privilegeMaterial) {
        throw new Error("Bulk config apply is incomplete");
      }
      const clientIds = preview.targets.map((target) => target.id);
      const operation: JobOperation = { type: "data_source_config_patch", toml: rendered.toml };
      const boundedTimeoutSecs = clampInteger(timeoutSecs, 1, 3600);
      const built = await buildPrivilegeForJobOperation({
        clientIds,
        commandType: "data_source_config_patch",
        operation,
        privilegeMaterial,
        selectorExpression: selectorExpression.trim(),
        timeoutSecs: boundedTimeoutSecs,
      });
      const response = await onCreateJob({
        argv: [],
            command: "data_source_config_patch",
        confirmed: true,
        destructive: true,
        force_unprivileged: false,
        operation,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
        selector_expression: selectorExpression.trim(),
        target_client_ids: clientIds,
        timeout_secs: boundedTimeoutSecs,
      });
      const initial = buildBulkJobProgress({
        acceptedTargets: response.target_count,
        jobId: response.job_id,
        targetRecords: [],
        targets: preview.targets,
      });
      setProgress(initial);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        acceptedTargets: response.target_count,
        onProgress: setProgress,
        targets: preview.targets,
      });
      const outputs = await onLoadJobOutputs(response.job_id).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          acceptedTargets: response.target_count,
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: preview.targets,
        }),
      );
    });
  }

  return (
    <div className="configApplyGrid">
      <div className="compactForm">
        <strong>Patch source</strong>
        <select aria-label="Rule template" onChange={(event) => setTemplateId(event.target.value)} value={selectedTemplate?.id ?? ""}>
          {hotConfigRuleTemplates.map((template) => (
            <option key={template.id} value={template.id}>
              {template.name}
            </option>
          ))}
        </select>
        <textarea aria-label="Rule values JSON" onChange={(event) => setValuesText(event.target.value)} rows={7} value={valuesText} />
        <button className="secondaryAction" disabled={pending || !selectedTemplate} onClick={renderPatch} type="button">
          Render patch
        </button>
        {rendered && <textarea aria-label="Bulk rendered hot-config patch" readOnly rows={8} value={rendered.toml} />}
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
          <input aria-label="Config apply timeout seconds" max={3600} min={1} onChange={(event) => setTimeoutSecs(Number(event.target.value))} type="number" value={timeoutSecs} />
        </div>
        <PrivilegeVaultBox
          labelPrefix="Config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={setPrivilegeMaterial}
          privilegeMaterial={privilegeMaterial}
          unlockRedirectLabel="Unlock config privilege"
        />
        <button className="primaryAction" disabled={pending || !ready} onClick={() => setConfirmOpen(true)} type="button">
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
        detail={`Apply one generated partial patch to ${preview?.target_count ?? 0} VPS targets.`}
        items={[
          { label: "Selector", value: selectorExpression || "-" },
          { label: "Targets", value: `${preview?.target_count ?? 0}` },
          { label: "Template", value: selectedTemplate?.name ?? "-" },
          { label: "Sections", value: rendered?.affected_sections.join(", ") ?? "-" },
        ]}
        onCancel={() => setConfirmOpen(false)}
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
  onResolveBulk,
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
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(CONFIG_SINGLE_SELECTOR_STORAGE_KEY));
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [redactedToml, setRedactedToml] = useState("");
  const [baseHash, setBaseHash] = useState("");
  const [lastJobId, setLastJobId] = useState<string | null>(null);
  const [timeoutSecs, setTimeoutSecs] = useState(30);
  const [confirmApplyOpen, setConfirmApplyOpen] = useState(false);
  const [progress, setProgress] = useState<BulkJobProgress | null>(null);
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);
  const singleTarget = preview?.target_count === 1 ? preview.targets[0] : null;

  useEffect(() => writeLocalString(CONFIG_SINGLE_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  async function previewSingle() {
    await runAction(async () => {
      if (selectorParse.error) {
        throw new Error(selectorParse.error);
      }
      setPreview(await onResolveBulk(selectorExpression.trim()));
    });
  }

  async function readConfig() {
    await runAction(async () => {
      if (!singleTarget || !privilegeMaterial) {
        throw new Error("Resolve exactly one VPS and unlock privilege");
      }
      const operation: JobOperation = { type: "config_read" };
      const selectorExpressionForTarget = selectorExpressionForClientIds([singleTarget.id]);
      const boundedTimeoutSecs = clampInteger(timeoutSecs, 1, 3600);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [singleTarget.id],
        commandType: "config_read",
        operation,
        privilegeMaterial,
        selectorExpression: selectorExpressionForTarget,
        timeoutSecs: boundedTimeoutSecs,
      });
      const response = await onCreateJob({
        argv: [],
            command: "config_read",
        confirmed: true,
        destructive: false,
        force_unprivileged: false,
        operation,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
        selector_expression: selectorExpressionForTarget,
        target_client_ids: [singleTarget.id],
        timeout_secs: boundedTimeoutSecs,
      });
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        acceptedTargets: response.target_count,
        onProgress: setProgress,
        targets: [singleTarget],
      });
      const outputs = await onLoadJobOutputs(response.job_id);
      setProgress(
        buildBulkJobProgress({
          acceptedTargets: response.target_count,
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: [singleTarget],
        }),
      );
      const config = extractConfigRead(outputs);
      setRedactedToml(config.toml);
      setBaseHash(config.baseHash);
    });
  }

  async function applyConfig() {
    setConfirmApplyOpen(false);
    await runAction(async () => {
      if (!singleTarget || !privilegeMaterial || !redactedToml || !baseHash) {
        throw new Error("Read a single VPS config before applying");
      }
      const operation: JobOperation = {
        type: "hot_config",
        toml: redactedToml,
        preserve_redacted: true,
        base_config_sha256_hex: baseHash,
      };
      const selectorExpressionForTarget = selectorExpressionForClientIds([singleTarget.id]);
      const boundedTimeoutSecs = clampInteger(timeoutSecs, 1, 3600);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [singleTarget.id],
        commandType: "hot_config",
        operation,
        privilegeMaterial,
        selectorExpression: selectorExpressionForTarget,
        timeoutSecs: boundedTimeoutSecs,
      });
      const response = await onCreateJob({
        argv: [],
            command: "hot_config",
        confirmed: true,
        destructive: true,
        force_unprivileged: false,
        operation,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
        selector_expression: selectorExpressionForTarget,
        target_client_ids: [singleTarget.id],
        timeout_secs: boundedTimeoutSecs,
      });
      setLastJobId(response.job_id);
      const waited = await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        acceptedTargets: response.target_count,
        onProgress: setProgress,
        targets: [singleTarget],
      });
      const outputs = await onLoadJobOutputs(response.job_id).catch(() => []);
      setProgress(
        buildBulkJobProgress({
          acceptedTargets: response.target_count,
          jobId: response.job_id,
          outputs,
          targetRecords: waited.targets,
          targets: [singleTarget],
        }),
      );
    });
  }

  return (
    <div className="configApplyGrid">
      <div className="compactForm">
        <strong>Single VPS target</strong>
        <SearchExpressionInput
          agents={agents}
          ariaLabel="Single VPS config selector expression"
          className="targetExpressionBar"
          onChange={(value) => {
            setSelectorExpression(value);
            setPreview(null);
          }}
          placeholder="id:edge-a"
          showMatchCount
          value={selectorExpression}
          verification={selectorParse.error ? "invalid" : selectorExpression.trim() ? "valid" : "neutral"}
          verificationMessage={selectorParse.error ?? (preview ? `${preview.target_count}/${agents.length}` : selectorExpression.trim() ? undefined : "no selector")}
        />
        <button className="secondaryAction" disabled={pending || !selectorExpression.trim()} onClick={previewSingle} type="button">
          Resolve target
        </button>
        <span>{singleTarget ? formatVpsName(singleTarget, vpsNameDisplayMode) : `${preview?.target_count ?? 0} targets resolved`}</span>
        <div className="inlinePrivilege">
          <input aria-label="Single config timeout seconds" max={3600} min={1} onChange={(event) => setTimeoutSecs(Number(event.target.value))} type="number" value={timeoutSecs} />
        </div>
        <PrivilegeVaultBox
          labelPrefix="Config"
          lastPayloadHash={null}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={setPrivilegeMaterial}
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
        <textarea aria-label="Single VPS redacted config TOML" onChange={(event) => setRedactedToml(event.target.value)} rows={22} value={redactedToml} />
        <button className="primaryAction" disabled={pending || !singleTarget || !privilegeMaterial || !baseHash || !redactedToml} onClick={() => setConfirmApplyOpen(true)} type="button">
          <Save size={16} />
          Review apply
        </button>
      </div>
      <ConfirmationPrompt
        confirmLabel="Apply config"
        detail="Apply the redacted-preserve TOML only if the VPS config hash still matches the read base."
        items={[
          { label: "Target", value: singleTarget ? formatVpsName(singleTarget, vpsNameDisplayMode) : "-" },
          { label: "Base hash", value: baseHash || "-" },
          { label: "Policy", value: "preserve redacted fields" },
        ]}
        onCancel={() => setConfirmApplyOpen(false)}
        onConfirm={() => void applyConfig()}
        open={confirmApplyOpen}
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
      return "Bulk hot config";
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
