import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  DatabaseZap,
  FileText,
  Pencil,
  Plus,
  Route,
  SlidersHorizontal,
  UserPlus,
} from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ActionFeedback } from "../components/ActionFeedback";
import { ConsoleActionDrawer } from "../components/ConsoleLayout";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import { sourceReadinessStatusBadgeClass } from "../jobStatusPresentation";
import { scrollIntoViewWithMotion } from "../motion";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../searchExpression";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../privilege";
import type {
  AgentView,
  AssignSourceTemplateRequest,
  AssignSourceTemplateResponse,
  BulkResolveResponse,
  CloneSourceTemplateRequest,
  CreateSourceTemplateRequest,
  TemplateRuntimeConfigResponse,
  SourceTemplateAssignmentRecord,
  SourceTemplateDiffRequest,
  SourceTemplateDiffResponse,
  SourceTemplateRecord,
  SourceTemplateTestRequest,
  SourceTemplateTestResponse,
  SourceStatusRecord,
  JsonValue,
  RuntimeConfigDispatchRecord,
  UpdateSourceTemplateRequest,
  UpdateSourceTemplateResponse,
} from "../types";
import {
  dispatchFailureReason,
  formatTime,
  formatVpsName,
  runPanelAction,
  shortId,
} from "../utils";

const SOURCE_TEMPLATE_DOMAINS = [
  "telemetry_metrics_source",
  "runtime_traffic_accounting_source",
  "latency_probe_source",
  "speed_test_provider",
  "process_inventory_source",
  "user_session_inventory_source",
  "command_execution_policy",
  "process_supervisor_policy",
  "runtime_tunnel_adapter",
  "traffic_limit_status_source",
  "routing_cost_adapter",
  "backup_object_store",
  "restore_path_mapping",
  "update_artifact_source",
  "update_restart_policy",
  "update_rollback_heartbeat_source",
];
const ASSIGNABLE_SOURCE_TEMPLATE_DOMAINS = SOURCE_TEMPLATE_DOMAINS.filter(
  (domain) => !isPlanBoundAdapterDomain(domain),
);

const DEFAULT_DEFINITION = '{\n  "source": "custom"\n}';
const SOURCE_TEMPLATE_SELECTOR_STORAGE_KEY =
  "vpsman.sourceTemplates.assignmentSelectorExpression";
type SourceTemplateConfirmationAction = "assignment" | "lifecycle-update";
type SourceTemplateDrawerMode = "create" | "detail" | null;
type SourceTemplateDetailTab = "assign" | "render" | "lifecycle";

type SourceTemplateAssignmentSnapshot = {
  domain: string;
  templateId: string;
  templateName: string;
  selectorExpression: string;
  targetClientIds: string[];
  targets: AgentView[];
  assignments: AssignSourceTemplateResponse["assignments"];
  previewHash: string;
};

type SourceTemplateLifecycleUpdateSnapshot = {
  assignedClientCount: number;
  affectedClientIds: string[];
  description: string | null;
  definition: JsonValue;
  previewHash: string;
  templateId: string;
  templateName: string;
};

export function SourceTemplatePanel({
  activeSubpage,
  agents,
  assignments,
  sourceStatus,
  onAssignTemplate,
  onCloneTemplate,
  onCreateTemplate,
  onDiffTemplate,
  initialCreateDomain,
  onInitialCreateDomainConsumed,
  onOpenTunnelPlans,
  onRenderTemplateRuntimeConfig,
  onResolveBulk,
  onTestTemplate,
  onUpdateTemplate,
  privilegeMaterial,
  setPrivilegeMaterial,
  templates,
}: {
  activeSubpage: "templates";
  agents: AgentView[];
  assignments: SourceTemplateAssignmentRecord[];
  sourceStatus: SourceStatusRecord[];
  onAssignTemplate: (
    request: AssignSourceTemplateRequest,
  ) => Promise<AssignSourceTemplateResponse>;
  onCloneTemplate: (
    templateId: string,
    request: CloneSourceTemplateRequest,
  ) => Promise<void>;
  onCreateTemplate: (request: CreateSourceTemplateRequest) => Promise<void>;
  onDiffTemplate: (
    templateId: string,
    request: SourceTemplateDiffRequest,
  ) => Promise<SourceTemplateDiffResponse>;
  initialCreateDomain: string | null;
  onInitialCreateDomainConsumed: () => void;
  onOpenTunnelPlans: () => void;
  onRenderTemplateRuntimeConfig: (
    clientId: string,
  ) => Promise<TemplateRuntimeConfigResponse>;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onTestTemplate: (
    templateId: string,
    request: SourceTemplateTestRequest,
  ) => Promise<SourceTemplateTestResponse>;
  onUpdateTemplate: (
    templateId: string,
    request: UpdateSourceTemplateRequest,
  ) => Promise<UpdateSourceTemplateResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  templates: SourceTemplateRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const createFormRef = useRef<HTMLFormElement | null>(null);
  const assignmentFormRef = useRef<HTMLFormElement | null>(null);
  const lifecycleFormRef = useRef<HTMLFormElement | null>(null);
  const createDomainIntentHandledRef = useRef(false);
  const [drawerMode, setDrawerMode] = useState<SourceTemplateDrawerMode>(null);
  const [detailTab, setDetailTab] = useState<SourceTemplateDetailTab>("assign");
  const [createDomain, setCreateDomain] = useState(SOURCE_TEMPLATE_DOMAINS[1]);
  const [createName, setCreateName] = useState("");
  const [createScope, setCreateScope] = useState("shared");
  const [ownerClientId, setOwnerClientId] = useState("");
  const [description, setDescription] = useState("");
  const [definitionText, setDefinitionText] = useState(() =>
    defaultDefinitionForDomain(SOURCE_TEMPLATE_DOMAINS[1], templates),
  );
  const [assignDomain, setAssignDomain] = useState(
    ASSIGNABLE_SOURCE_TEMPLATE_DOMAINS[1],
  );
  const [assignTemplateId, setAssignTemplateId] = useState("");
  const [assignmentSelectorExpression, setAssignmentSelectorExpression] =
    useState(() => readLocalString(SOURCE_TEMPLATE_SELECTOR_STORAGE_KEY, ""));
  const [renderClientId, setRenderClientId] = useState("");
  const [renderedTemplateRuntimeConfig, setRenderedTemplateRuntimeConfig] =
    useState<TemplateRuntimeConfigResponse | null>(null);
  const [lifecycleTemplateId, setLifecycleTemplateId] = useState("");
  const [lifecycleDescription, setLifecycleDescription] = useState("");
  const [lifecycleDefinitionText, setLifecycleDefinitionText] =
    useState(DEFAULT_DEFINITION);
  const [lifecycleCloneName, setLifecycleCloneName] = useState("");
  const [lastDiff, setLastDiff] = useState<SourceTemplateDiffResponse | null>(
    null,
  );
  const [lastTest, setLastTest] = useState<SourceTemplateTestResponse | null>(
    null,
  );
  const [lastUpdate, setLastUpdate] =
    useState<UpdateSourceTemplateResponse | null>(null);
  const [lastAssignment, setLastAssignment] =
    useState<AssignSourceTemplateResponse | null>(null);
  const [lastCloneName, setLastCloneName] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [createFeedback, setCreateFeedback] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] =
    useState<SourceTemplateConfirmationAction | null>(null);
  const [assignmentSnapshot, setAssignmentSnapshot] =
    useState<SourceTemplateAssignmentSnapshot | null>(null);
  const [lifecycleUpdateSnapshot, setLifecycleUpdateSnapshot] =
    useState<SourceTemplateLifecycleUpdateSnapshot | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();

  const assignableTemplates = useMemo(
    () => templates.filter((template) => template.domain === assignDomain),
    [assignDomain, templates],
  );
  const sourceStatusSummary = useMemo(() => {
    const attention = sourceStatus.filter(
      (row) => sourceStatusTone(row.status) === "warning",
    ).length;
    const ready = sourceStatus.filter(
      (row) => sourceStatusTone(row.status) === "ok",
    ).length;
    return `${ready} ready source checks, ${attention} need review`;
  }, [sourceStatus]);
  const effectiveTemplateId =
    assignTemplateId || assignableTemplates[0]?.id || "";
  const effectiveLifecycleTemplateId =
    lifecycleTemplateId || templates[0]?.id || "";
  const showTemplateManagement = activeSubpage === "templates";
  const showSourceStatus = activeSubpage === "templates";
  const lifecycleTemplate = useMemo(
    () =>
      templates.find(
        (template) => template.id === effectiveLifecycleTemplateId,
      ) ?? null,
    [effectiveLifecycleTemplateId, templates],
  );
  const planBoundAdapter =
    lifecycleTemplate !== null &&
    isPlanBoundAdapterDomain(lifecycleTemplate.domain);
  const assignmentSelectorParse = useMemo(
    () => parseSearchExpression(assignmentSelectorExpression),
    [assignmentSelectorExpression],
  );
  const assignmentHasSelector = Boolean(assignmentSelectorExpression.trim());
  const assignmentTargetCount = useMemo(
    () =>
      !assignmentSelectorExpression.trim() || assignmentSelectorParse.error
        ? 0
        : agentsMatchingExpression(agents, assignmentSelectorExpression).length,
    [agents, assignmentSelectorExpression, assignmentSelectorParse.error],
  );
  const lifecycleStatus = lastUpdate?.confirmation_required
    ? `${sourceTemplateDiffSummary(lastUpdate.diff)}; ${lastUpdate.affected_client_count} ${sourceTemplateUsageLabel(planBoundAdapter, lastUpdate.affected_client_count)} require confirmation`
    : lastUpdate
      ? `${sourceTemplateDiffSummary(lastUpdate.diff)}; desired state updated for ${lastUpdate.affected_client_count} ${sourceTemplateUsageLabel(planBoundAdapter, lastUpdate.affected_client_count)}; ${runtimeDispatchSummary(lastUpdate.sync)}`
      : lastTest
        ? lastTest.valid
          ? `${lastTest.renderable ? "Renderable" : "Workflow"} template test passed for ${lastTest.domain}`
          : `Template test failed: ${lastTest.error ?? "invalid definition"}`
        : lastDiff
          ? `${sourceTemplateDiffSummary(lastDiff)}; ${lastDiff.affected_client_count} ${sourceTemplateUsageLabel(planBoundAdapter, lastDiff.affected_client_count)} ${sourceTemplateDiffHasChanges(lastDiff) ? "affected" : "in scope"}`
          : null;
  const status =
    (sourceStatus.length > 0 ? sourceStatusSummary : null) ??
    `${templates.length} templates across ${new Set(templates.map((template) => template.domain)).size} domains`;
  const assignmentStatus = lastAssignment
    ? lastAssignment.confirmation_required
      ? `Reviewed ${lastAssignment.target_count} ${lastAssignment.target_count === 1 ? "VPS" : "VPSs"}; confirmation required`
      : `Template assignment saved for ${lastAssignment.target_count} ${lastAssignment.target_count === 1 ? "VPS" : "VPSs"}; ${runtimeDispatchSummary(lastAssignment.sync)}`
    : null;
  const sourceWorkflowDispatch = lifecycleStatus && lastUpdate
    ? lastUpdate.sync
    : assignmentStatus && lastAssignment
      ? lastAssignment.sync
      : [];
  const sourceWorkflowFeedbackMessage =
    actionError ??
    reviewStatus ??
    lifecycleStatus ??
    (lastCloneName ? `Cloned template as ${lastCloneName}` : null) ??
    assignmentStatus;
  const sourceWorkflowFeedbackTone =
    actionError || lifecycleStatus?.startsWith("Template test failed")
      ? "danger"
      : sourceWorkflowDispatch.some((outcome) => outcome.status !== "queued")
        ? "warning"
        : sourceWorkflowDispatch.length > 0
          ? "progress"
          : lifecycleStatus ||
          lastCloneName ||
          (lastAssignment && !lastAssignment.confirmation_required)
            ? "success"
            : "progress";
  const sourceTemplateListFeedbackMessage =
    drawerMode === null ? actionError : null;
  const sourceStatusColumns = useMemo<
    ConsoleDataGridColumn<SourceStatusRecord>[]
  >(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatVpsName(row, vpsNameDisplayMode)}</strong>
            <small>{sourceTokenLabel(row.client_status)}</small>
          </span>
        ),
        header: "VPS",
        id: "vps",
        searchValue: (row) =>
          `${formatVpsName(row, vpsNameDisplayMode)} ${row.client_id} ${row.client_status}`,
        sortValue: (row) => formatVpsName(row, vpsNameDisplayMode),
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.module}</strong>
            <small>{sourceDomainLabel(row.domain)}</small>
          </span>
        ),
        header: "Module",
        id: "module",
        searchValue: (row) => `${row.module} ${row.domain}`,
        sortValue: (row) => row.module,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.template_name}</strong>
            <small>{sourceTokenLabel(row.template_scope)}</small>
          </span>
        ),
        header: "Template",
        id: "template",
        searchValue: (row) => `${row.template_name} ${row.template_scope}`,
        sortValue: (row) => row.template_name,
      },
      {
        cell: (row) => sourceTokenLabel(row.source_kind),
        header: "Source",
        id: "source",
        searchValue: (row) => row.source_kind,
        sortValue: (row) => row.source_kind,
      },
      {
        cell: (row) => (
          <span
            className={`status ${sourceReadinessStatusBadgeClass(row.status)}`}
            title={row.status_reason}
          >
            {sourceStatusLabel(row.status)}
          </span>
        ),
        header: "Readiness",
        id: "status",
        searchValue: (row) => `${row.status} ${row.status_reason}`,
        sortValue: (row) => row.status,
      },
      {
        cell: (row) => sourceEvidenceSummary(row),
        header: "Evidence",
        id: "evidence",
        searchValue: (row) => sourceEvidenceSummary(row),
        sortValue: (row) => sourceEvidenceSummary(row),
      },
    ],
    [vpsNameDisplayMode],
  );
  const templateColumns = useMemo<
    ConsoleDataGridColumn<SourceTemplateRecord>[]
  >(
    () => [
      {
        cell: (template) => (
          <span className="historyPrimary">
            <strong>{template.name}</strong>
            <small>
              {template.description ??
                (template.built_in ? "Built-in" : "Custom")}
            </small>
          </span>
        ),
        header: "Template",
        id: "template",
        searchValue: (template) =>
          `${template.name} ${template.description ?? ""}`,
        sortValue: (template) => template.name,
      },
      {
        cell: (template) => sourceDomainLabel(template.domain),
        header: "Domain",
        id: "domain",
        searchValue: (template) => template.domain,
        sortValue: (template) => template.domain,
      },
      {
        cell: (template) => (
          <span
            className={`status ${template.is_default ? "info" : template.built_in ? "neutral" : "ok"}`}
          >
            {template.is_default ? "Default" : sourceTokenLabel(template.scope)}
          </span>
        ),
        header: "Scope",
        id: "scope",
        searchValue: (template) =>
          `${template.scope} ${template.is_default ? "default" : ""} ${template.built_in ? "built-in" : "custom"}`,
        sortValue: (template) =>
          `${template.is_default ? "0" : "1"}:${template.scope}`,
      },
      {
        cell: (template) => template.assigned_client_count,
        header: "VPS use",
        id: "assigned",
        searchValue: (template) => template.assigned_client_count,
        sortValue: (template) => template.assigned_client_count,
      },
      {
        cell: (template) => formatTime(template.updated_at),
        header: "Updated",
        id: "updated",
        searchValue: (template) => formatTime(template.updated_at),
        sortValue: (template) => template.updated_at,
      },
    ],
    [],
  );
  const templateActions = useMemo<
    ConsoleDataGridAction<SourceTemplateRecord>[]
  >(
    () => [
      {
        label: "Open",
        description: (rows) =>
          rows.length === 1
            ? `Open ${rows[0].name} template detail.`
            : "Select exactly one template to open.",
        disabled: (rows) => rows.length !== 1,
        hidden: (rows) =>
          rows.length === 1 && isPlanBoundAdapterDomain(rows[0].domain),
        icon: <FileText size={14} />,
        onSelect: (rows) =>
          openTemplateDetail(
            rows[0],
            isPlanBoundAdapterDomain(rows[0].domain) ? "lifecycle" : "assign",
          ),
      },
      {
        label: "Assign",
        description: (rows) =>
          rows.length === 1 && isPlanBoundAdapterDomain(rows[0].domain)
            ? "Adapter templates are bound from Network > Tunnel plans."
            : rows.length === 1
              ? `Load ${rows[0].name} into the assignment form.`
              : "Select exactly one template to assign.",
        disabled: (rows) =>
          rows.length !== 1 || isPlanBoundAdapterDomain(rows[0].domain),
        hidden: (rows) =>
          rows.length === 1 && isPlanBoundAdapterDomain(rows[0].domain),
        icon: <UserPlus size={14} />,
        onSelect: (rows) => prepareTemplateAssignment(rows[0]),
      },
      {
        label: "Edit/test",
        description: (rows) =>
          rows.length === 1
            ? `Load ${rows[0].name} into the lifecycle form.`
            : "Select exactly one template to edit or test.",
        disabled: (rows) => rows.length !== 1,
        icon: <Pencil size={14} />,
        onSelect: (rows) => prepareTemplateLifecycle(rows[0]),
      },
    ],
    [],
  );

  useEffect(() => {
    if (!lifecycleTemplate) {
      return;
    }
    setLifecycleDescription(lifecycleTemplate.description ?? "");
    setLifecycleDefinitionText(
      JSON.stringify(lifecycleTemplate.definition, null, 2),
    );
    setLifecycleCloneName(defaultCloneName(lifecycleTemplate.name));
    setLastDiff(null);
    setLastTest(null);
    setLastUpdate(null);
    setLastCloneName(null);
    clearLifecycleUpdateConfirmation();
  }, [lifecycleTemplate?.id]);

  useEffect(() => {
    writeLocalString(
      SOURCE_TEMPLATE_SELECTOR_STORAGE_KEY,
      assignmentSelectorExpression,
    );
  }, [assignmentSelectorExpression]);

  async function submitCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const templateName = createName.trim();
    setCreateFeedback(null);
    await runPanelAction(setPending, setActionError, async () => {
      await onCreateTemplate({
        definition: parseDefinition(definitionText),
        description: description.trim() || null,
        domain: createDomain,
        name: templateName,
        owner_client_id:
          createScope === "vps_local" ? ownerClientId || null : null,
        scope: createScope,
      });
      setCreateName("");
      setDescription("");
      setDefinitionText(defaultDefinitionForDomain(createDomain, templates));
      setCreateFeedback(`Created source template ${templateName}`);
    });
  }

  async function submitAssignment(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    clearAssignmentConfirmation();
    const reviewGeneration = captureReviewGeneration();
    const frozenDomain = assignDomain;
    const frozenTemplateId = effectiveTemplateId;
    const frozenSelector = assignmentSelectorExpression.trim();
    setReviewStatus("Preparing template assignment review");
    try {
      await runPanelAction(setPending, setActionError, async () => {
        await waitForReviewRender();
        if (assignmentSelectorParse.error) {
          throw new Error(
            `Invalid target expression: ${assignmentSelectorParse.error}`,
          );
        }
        if (!frozenSelector) {
          throw new Error("Add at least one target selector");
        }
        if (!frozenTemplateId) {
          throw new Error("Select a template");
        }
        const resolved = await onResolveBulk(frozenSelector);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const targetClientIds = resolved.targets.map((target) => target.id);
        if (!targetClientIds.length) {
          throw new Error("Template assignment confirmation resolved no VPSs");
        }
        const preview = await onAssignTemplate({
          confirmed: false,
          domain: frozenDomain,
          template_id: frozenTemplateId,
          selector_expression: frozenSelector,
          target_client_ids: targetClientIds,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setLastCloneName(null);
        setLastDiff(null);
        setLastTest(null);
        setLastUpdate(null);
        setLastAssignment(preview);
        setAssignmentSnapshot({
          assignments: preview.assignments,
          domain: frozenDomain,
          previewHash: requirePreviewHash(
            preview.preview_hash,
            "Template assignment",
          ),
          templateId: frozenTemplateId,
          templateName: preview.template.name,
          selectorExpression: frozenSelector,
          targetClientIds,
          targets: resolved.targets,
        });
        setPendingConfirmation("assignment");
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
  }

  async function executeAssignment() {
    await runPanelAction(setPending, setActionError, async () => {
      const snapshot = assignmentSnapshot;
      if (!snapshot) {
        throw new Error(
          "Template assignment confirmation snapshot is missing; review the assignment again",
        );
      }
      const privilegeAssertion = await buildSourceTemplatePrivilegeAssertion({
        action: "source_template.assign",
        previewHash: snapshot.previewHash,
        selectorExpression: snapshot.selectorExpression,
        targetClientIds: snapshot.targetClientIds,
        templateId: snapshot.templateId,
      });
      const response = await onAssignTemplate({
        confirmed: true,
        domain: snapshot.domain,
        preview_hash: snapshot.previewHash,
        privilege_assertion: privilegeAssertion,
        template_id: snapshot.templateId,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.targetClientIds,
      });
      setLastCloneName(null);
      setLastDiff(null);
      setLastTest(null);
      setLastUpdate(null);
      setLastAssignment(response);
      setAssignmentSnapshot(null);
      setPendingConfirmation(null);
      await waitForReviewRender();
      scrollIntoViewSoon(
        assignmentFormRef.current?.closest<HTMLElement>(".actionDrawer") ??
          assignmentFormRef.current,
      );
    });
  }

  async function previewTemplateRuntimeConfig(
    event: FormEvent<HTMLFormElement>,
  ) {
    event.preventDefault();
    clearApplyConfirmation();
    const reviewGeneration = captureReviewGeneration();
    const frozenClientId = renderClientId;
    setReviewStatus("Rendering template runtime config");
    try {
      await runPanelAction(setPending, setActionError, async () => {
        await waitForReviewRender();
        const rendered = await onRenderTemplateRuntimeConfig(frozenClientId);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setRenderedTemplateRuntimeConfig(rendered);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
  }

  async function diffLifecycleTemplate() {
    if (!lifecycleTemplate) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      setLastDiff(
        await onDiffTemplate(lifecycleTemplate.id, {
          definition: parseDefinition(lifecycleDefinitionText),
          description: lifecycleDescription.trim() || null,
        }),
      );
      setLastAssignment(null);
      setLastCloneName(null);
      setLastTest(null);
      setLastUpdate(null);
    });
  }

  async function testLifecycleTemplate() {
    if (!lifecycleTemplate) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      setLastTest(
        await onTestTemplate(lifecycleTemplate.id, {
          definition: parseDefinition(lifecycleDefinitionText),
        }),
      );
      setLastAssignment(null);
      setLastCloneName(null);
      setLastDiff(null);
      setLastUpdate(null);
    });
  }

  async function cloneLifecycleTemplate() {
    if (!lifecycleTemplate || !lifecycleCloneName.trim()) {
      return;
    }
    const cloneName = lifecycleCloneName.trim();
    await runPanelAction(setPending, setActionError, async () => {
      await onCloneTemplate(lifecycleTemplate.id, {
        description:
          lifecycleDescription.trim() || lifecycleTemplate.description,
        name: cloneName,
        owner_client_id: null,
        scope: "shared",
      });
      setLastDiff(null);
      setLastTest(null);
      setLastUpdate(null);
      setLastAssignment(null);
      setLastCloneName(cloneName);
      await waitForReviewRender();
      scrollIntoViewSoon(
        lifecycleFormRef.current?.closest<HTMLElement>(".actionDrawer") ??
          lifecycleFormRef.current,
      );
    });
  }

  async function updateLifecycleTemplate() {
    if (!lifecycleTemplate || lifecycleTemplate.built_in) {
      return;
    }
    const template = lifecycleTemplate;
    await runPanelAction(setPending, setActionError, async () => {
      const description = lifecycleDescription.trim() || null;
      const definition = parseDefinition(lifecycleDefinitionText);
      const preview = await onUpdateTemplate(template.id, {
        confirmed: false,
        definition,
        description,
      });
      setLastAssignment(null);
      setLastCloneName(null);
      setLastUpdate(preview);
      setLastDiff(preview.diff);
      setLastTest(null);
      if (!preview.confirmation_required) {
        setLifecycleUpdateSnapshot(null);
        setPendingConfirmation(null);
        return;
      }
      setLifecycleUpdateSnapshot({
        affectedClientIds: preview.affected_client_ids,
        assignedClientCount: preview.affected_client_count,
        description,
        definition,
        previewHash: requirePreviewHash(
          preview.preview_hash,
          "Template update",
        ),
        templateId: template.id,
        templateName: template.name,
      });
      setPendingConfirmation("lifecycle-update");
    });
  }

  async function executeLifecycleTemplateUpdate(
    snapshot: SourceTemplateLifecycleUpdateSnapshot,
  ) {
    await runPanelAction(setPending, setActionError, async () => {
      const privilegeAssertion = snapshot.affectedClientIds.length
        ? await buildSourceTemplatePrivilegeAssertion({
            action: "source_template.update",
            previewHash: snapshot.previewHash,
            targetClientIds: snapshot.affectedClientIds,
            templateId: snapshot.templateId,
          })
        : null;
      const response = await onUpdateTemplate(snapshot.templateId, {
        confirmed: true,
        definition: snapshot.definition,
        description: snapshot.description,
        preview_hash: snapshot.previewHash,
        privilege_assertion: privilegeAssertion,
      });
      setLastAssignment(null);
      setLastCloneName(null);
      setLastUpdate(response);
      setLastDiff(response.diff);
      setLastTest(null);
      setLifecycleUpdateSnapshot(null);
      setPendingConfirmation(null);
      await waitForReviewRender();
      scrollIntoViewSoon(
        lifecycleFormRef.current?.closest<HTMLElement>(".actionDrawer") ??
          lifecycleFormRef.current,
      );
    });
  }

  async function confirmSourceTemplateAction() {
    const action = pendingConfirmation;
    if (!action) {
      return;
    }
    if (action === "assignment") {
      if (!assignmentSnapshot) {
        setActionError(
          "Template assignment confirmation snapshot is missing; review the assignment again",
        );
        return;
      }
      await executeAssignment();
    } else {
      if (!lifecycleUpdateSnapshot) {
        setActionError(
          "Template update confirmation snapshot is missing; review the update again",
        );
        return;
      }
      await executeLifecycleTemplateUpdate(lifecycleUpdateSnapshot);
    }
  }

  const sourceTemplateConfirmationTitle =
    pendingConfirmation === "assignment"
      ? "Confirm template assignment"
      : "Update template";
  const sourceTemplateConfirmationDetail =
    pendingConfirmation === "assignment"
      ? "Confirm the chosen template and resolved VPS assignment set."
      : "Confirm updating this template for assigned VPSs.";
  const sourceTemplateConfirmationItems =
    pendingConfirmation === "assignment"
      ? [
          {
            label: "Domain",
            value: sourceDomainLabel(
              assignmentSnapshot?.domain ?? assignDomain,
            ),
          },
          {
            label: "Template",
            value:
              assignmentSnapshot?.templateName ??
              (effectiveTemplateId ? shortId(effectiveTemplateId) : "none"),
          },
          {
            label: "Targets",
            value: assignmentSnapshot
              ? `${assignmentSnapshot.targetClientIds.length} resolved and frozen`
              : `${assignmentTargetCount}/${agents.length}`,
          },
          {
            label: "Preview",
            value: assignmentSnapshot
              ? assignmentSnapshot.targets
                  .slice(0, 4)
                  .map((target) => formatVpsName(target, vpsNameDisplayMode))
                  .join(", ") +
                (assignmentSnapshot.targets.length > 4
                  ? `, +${assignmentSnapshot.targets.length - 4} more`
                  : "")
              : "Review assignment to freeze targets",
          },
        ]
      : [
          {
            label: "Template",
            value: lifecycleUpdateSnapshot?.templateName ?? "none",
          },
          {
            label: "Assigned",
            value: `${lifecycleUpdateSnapshot?.assignedClientCount ?? 0} VPSs`,
          },
        ];
  const sourceTemplateConfirmationRequiresPrivilege =
    pendingConfirmation === "assignment" ||
    (pendingConfirmation === "lifecycle-update" &&
      (lifecycleUpdateSnapshot?.affectedClientIds.length ?? 0) > 0);
  const sourceTemplateConfirmationPreviewHash =
    pendingConfirmation === "assignment"
      ? (assignmentSnapshot?.previewHash ?? null)
      : pendingConfirmation === "lifecycle-update"
        ? (lifecycleUpdateSnapshot?.previewHash ?? null)
        : null;

  function requirePreviewHash(hash: string | null | undefined, action: string) {
    if (!hash) {
      throw new Error(
        `${action} preview expired; review again before applying`,
      );
    }
    return hash;
  }

  async function buildSourceTemplatePrivilegeAssertion({
    action,
    previewHash,
    selectorExpression,
    targetClientIds,
    templateId,
  }: {
    action: "source_template.assign" | "source_template.update";
    previewHash: string;
    selectorExpression?: string | null;
    targetClientIds: string[];
    templateId: string;
  }) {
    if (!privilegeMaterial) {
      throw new Error("Privilege unlock is required before final apply");
    }
    return buildPrivilegeAssertion({
      intent: canonicalDbPrivilegeIntent({
        action,
        confirmed: true,
        payloadHash: previewHash,
        resolvedTargets: targetClientIds,
        selectorExpression,
        target: sourceTemplatePrivilegeTarget(templateId),
      }),
      privilegeMaterial,
    });
  }

  function clearApplyConfirmation() {
    invalidateReviewGeneration();
    setReviewStatus(null);
  }

  function clearAssignmentConfirmation() {
    invalidateReviewGeneration();
    setAssignmentSnapshot(null);
    setPendingConfirmation((current) =>
      current === "assignment" ? null : current,
    );
    setReviewStatus(null);
  }

  function clearLifecycleUpdateConfirmation() {
    setLifecycleUpdateSnapshot(null);
    setPendingConfirmation((current) =>
      current === "lifecycle-update" ? null : current,
    );
  }

  function clearLifecycleCandidateResults() {
    clearLifecycleUpdateConfirmation();
    setLastCloneName(null);
    setLastDiff(null);
    setLastTest(null);
    setLastUpdate(null);
  }

  function changeAssignDomain(domain: string) {
    clearAssignmentConfirmation();
    setAssignDomain(domain);
    setAssignTemplateId("");
    setLastAssignment(null);
  }

  function prepareTemplateAssignment(template: SourceTemplateRecord) {
    if (isPlanBoundAdapterDomain(template.domain)) {
      prepareTemplateLifecycle(template);
      return;
    }
    clearAssignmentConfirmation();
    setAssignDomain(template.domain);
    setAssignTemplateId(template.id);
    setLastAssignment(null);
    setLifecycleTemplateId(template.id);
    setDetailTab("assign");
    setDrawerMode("detail");
  }

  function prepareNewTemplate(requestedDomain = SOURCE_TEMPLATE_DOMAINS[1]) {
    const domain = SOURCE_TEMPLATE_DOMAINS.includes(requestedDomain)
      ? requestedDomain
      : SOURCE_TEMPLATE_DOMAINS[1];
    setCreateDomain(domain);
    setCreateName("");
    setCreateScope("shared");
    setOwnerClientId("");
    setDescription("");
    setDefinitionText(defaultDefinitionForDomain(domain, templates));
    setActionError(null);
    setCreateFeedback(null);
    setDrawerMode("create");
  }

  useEffect(() => {
    if (initialCreateDomain === null) {
      createDomainIntentHandledRef.current = false;
      return;
    }
    if (createDomainIntentHandledRef.current) {
      return;
    }
    createDomainIntentHandledRef.current = true;
    prepareNewTemplate(initialCreateDomain);
    onInitialCreateDomainConsumed();
  }, [initialCreateDomain, onInitialCreateDomainConsumed]);

  function prepareTemplateLifecycle(template: SourceTemplateRecord) {
    clearLifecycleUpdateConfirmation();
    setLifecycleTemplateId(template.id);
    setLastAssignment(null);
    setLastCloneName(null);
    setLastDiff(null);
    setLastTest(null);
    setLastUpdate(null);
    if (!isPlanBoundAdapterDomain(template.domain)) {
      setAssignDomain(template.domain);
      setAssignTemplateId(template.id);
    }
    setDetailTab("lifecycle");
    setDrawerMode("detail");
  }

  function openTemplateDetail(
    template: SourceTemplateRecord,
    tab: SourceTemplateDetailTab,
  ) {
    setLifecycleTemplateId(template.id);
    if (!isPlanBoundAdapterDomain(template.domain)) {
      setAssignDomain(template.domain);
      setAssignTemplateId(template.id);
    }
    setDetailTab(isPlanBoundAdapterDomain(template.domain) ? "lifecycle" : tab);
    setDrawerMode("detail");
    setActionError(null);
  }

  return (
    <section className="fleetPanel sourceTemplatePanel">
      <div className="sectionHeader">
        <div>
          <h2>Source templates</h2>
          <span>{status}</span>
        </div>
      </div>

      {showTemplateManagement && (
        <ConfirmationPrompt
          confirmLabel={
            pendingConfirmation === "assignment"
              ? "Apply template assignment"
              : "Update template"
          }
          confirmDisabled={
            sourceTemplateConfirmationRequiresPrivilege && !privilegeMaterial
          }
          detail={sourceTemplateConfirmationDetail}
          items={sourceTemplateConfirmationItems}
          onCancel={() => {
            if (pendingConfirmation === "assignment") {
              setAssignmentSnapshot(null);
            } else if (pendingConfirmation === "lifecycle-update") {
              setLifecycleUpdateSnapshot(null);
            }
            setPendingConfirmation(null);
          }}
          onConfirm={() => void confirmSourceTemplateAction()}
          open={pendingConfirmation !== null}
          pending={pending}
          title={sourceTemplateConfirmationTitle}
          tone="normal"
        >
          {sourceTemplateConfirmationRequiresPrivilege &&
            !privilegeMaterial && (
              <PrivilegeVaultBox
                labelPrefix="Source templates"
                lastPayloadHash={sourceTemplateConfirmationPreviewHash}
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                privilegeMaterial={privilegeMaterial}
                usePrivilegeLabel="Unlock source template apply"
              />
            )}
        </ConfirmationPrompt>
      )}

      {showTemplateManagement && (
        <>
          <ActionFeedback
            className="localActionFeedback sourceTemplateListActionFeedback"
            message={sourceTemplateListFeedbackMessage}
            tone="danger"
          />
          <ConsoleDataGrid
            actions={templateActions}
            columns={templateColumns}
            defaultPageSize={10}
            getRowId={(template) => template.id}
            itemLabel="templates"
            empty={
              <div className="emptyState">
                <DatabaseZap size={22} />
                <strong>No templates</strong>
                <span>No template records match the current search.</span>
              </div>
            }
            renderExpandedRow={(template) => (
              <>
                <div className="consoleInlineDetailGrid">
                  <span>Template ID</span>
                  <strong>{template.id}</strong>
                  <span>Name</span>
                  <strong>{template.name}</strong>
                  <span>Domain</span>
                  <strong>{sourceDomainLabel(template.domain)}</strong>
                  <span>Scope</span>
                  <strong>{sourceTokenLabel(template.scope)}</strong>
                  <span>Default</span>
                  <strong>{template.is_default ? "Yes" : "No"}</strong>
                  <span>
                    {isPlanBoundAdapterDomain(template.domain)
                      ? "Bound endpoint VPSs"
                      : "Assigned VPSs"}
                  </span>
                  <strong>{template.assigned_client_count}</strong>
                  <span>Description</span>
                  <strong>{template.description ?? "None"}</strong>
                </div>
                <div
                  className="consoleInlineDetailActions"
                  aria-label={`Template workflow actions for ${template.name}`}
                >
                  {!isPlanBoundAdapterDomain(template.domain) && (
                    <>
                      <button
                        className="secondaryAction compactAction"
                        onClick={(event) => {
                          event.stopPropagation();
                          openTemplateDetail(template, "assign");
                        }}
                        type="button"
                      >
                        <UserPlus size={14} />
                        <span>Assign</span>
                      </button>
                      <button
                        className="secondaryAction compactAction"
                        onClick={(event) => {
                          event.stopPropagation();
                          openTemplateDetail(template, "render");
                        }}
                        type="button"
                      >
                        <FileText size={14} />
                        <span>Render</span>
                      </button>
                    </>
                  )}
                  <button
                    className="secondaryAction compactAction"
                    onClick={(event) => {
                      event.stopPropagation();
                      openTemplateDetail(template, "lifecycle");
                    }}
                    type="button"
                  >
                    <Pencil size={14} />
                    <span>Test/update</span>
                  </button>
                </div>
              </>
            )}
            renderSelectionPanel={(rows) => (
              <div className="gridSelectionSummary">
                <span>
                  <strong>{rows.length}</strong>
                  selected
                </span>
                <span>
                  <strong>
                    {new Set(rows.map((template) => template.domain)).size}
                  </strong>
                  domains
                </span>
                <span>
                  <strong>
                    {rows.filter((template) => template.built_in).length}
                  </strong>
                  built-in
                </span>
                <span>
                  <strong>
                    {rows.reduce(
                      (total, template) =>
                        total + template.assigned_client_count,
                      0,
                    )}
                  </strong>
                  VPS uses
                </span>
              </div>
            )}
            rowActions={templateActions}
            onOpenRow={(template) =>
              openTemplateDetail(
                template,
                isPlanBoundAdapterDomain(template.domain)
                  ? "lifecycle"
                  : "assign",
              )
            }
            openRowLabel="Open"
            openRowTitle={(template) =>
              `Open details for template ${template.name}.`
            }
            showMobileOpenRowAction={false}
            rows={templates}
            searchPlaceholder="Search templates"
            storageKey="vpsman.sourceTemplates.registry"
            title="Template registry"
            toolbarActions={
              <button
                className="primaryAction compactAction"
                onClick={() => prepareNewTemplate()}
                title="Create a new source template for runtime config, telemetry, or workflow policy."
                type="button"
              >
                <Plus size={15} />
                <span>New template</span>
              </button>
            }
          />
        </>
      )}

      {showSourceStatus && (
        <details className="sourceStatusSection">
          <summary>
            <strong>Active source status</strong>
            <span>{sourceStatusSummary}</span>
          </summary>
          <div className="sourceStatusGridWrap">
            <ConsoleDataGrid
              columns={sourceStatusColumns}
              defaultPageSize={10}
              expandOnRowClick
              getRowId={(row) => `${row.client_id}:${row.domain}`}
              itemLabel="sources"
              empty={
                <div className="emptyState">
                  <DatabaseZap size={22} />
                  <strong>Active source status</strong>
                  <span>
                    No active source records match the current search.
                  </span>
                </div>
              }
              renderExpandedRow={(row) => (
                <div className="consoleInlineDetailGrid">
                  <span>VPS</span>
                  <strong>{formatVpsName(row, vpsNameDisplayMode)}</strong>
                  <span>Client ID</span>
                  <strong>{row.client_id}</strong>
                  <span>Domain</span>
                  <strong>{sourceDomainLabel(row.domain)}</strong>
                  <span>Template</span>
                  <strong>{row.template_name}</strong>
                  <span>Source</span>
                  <strong>{sourceTokenLabel(row.source_kind)}</strong>
                  <span>Raw state</span>
                  <strong>{row.status}</strong>
                  <span>Reason</span>
                  <strong>{row.status_reason}</strong>
                  <span>Evidence</span>
                  <strong>{sourceEvidenceSummary(row)}</strong>
                </div>
              )}
              rows={sourceStatus}
              searchPlaceholder="Search active sources"
              selectable={false}
              storageKey="vpsman.sourceTemplates.activeSources"
              title="Active sources"
            />
          </div>
        </details>
      )}

      <ConsoleActionDrawer
        description={
          drawerMode === "create"
            ? "Create one reusable source template."
            : lifecycleTemplate
              ? `${sourceDomainLabel(lifecycleTemplate.domain)} · ${lifecycleTemplate.assigned_client_count} ${isPlanBoundAdapterDomain(lifecycleTemplate.domain) ? "bound endpoint VPSs" : "assigned VPSs"}`
              : "Select a template from the registry."
        }
        onClose={() => setDrawerMode(null)}
        open={drawerMode !== null}
        title={
          drawerMode === "create"
            ? "New source template"
            : (lifecycleTemplate?.name ?? "Source template detail")
        }
      >
        {drawerMode === "create" ? (
          <form
            className="compactForm templateForm"
            onSubmit={submitCreate}
            ref={createFormRef}
          >
            <strong>Template definition</strong>
            <span className="formHint">
              Create one reusable template. Scope decides whether it is shared
              or owned by one VPS.
            </span>
            <ActionFeedback
              className="localActionFeedback sourceTemplateActionFeedback"
              message={actionError}
              tone="danger"
            />
            <ActionFeedback
              className="localActionFeedback sourceTemplateActionFeedback"
              message={createFeedback}
              tone="success"
            />
            <div className="formRow templateFormRow">
              <label>
                <span>Domain</span>
                <select
                  aria-label="Template domain"
                  onChange={(event) => {
                    const domain = event.target.value;
                    setCreateDomain(domain);
                    setDefinitionText(
                      defaultDefinitionForDomain(domain, templates),
                    );
                    setCreateFeedback(null);
                  }}
                  title={sourceDomainLabel(createDomain)}
                  value={createDomain}
                >
                  {SOURCE_TEMPLATE_DOMAINS.map((domain) => (
                    <option key={domain} value={domain}>
                      {sourceDomainLabel(domain)}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>Name</span>
                <input
                  aria-label="Template name"
                  onChange={(event) => {
                    setCreateName(event.target.value);
                    setCreateFeedback(null);
                  }}
                  placeholder={sourceTemplateNamePlaceholder(createDomain)}
                  title={sourceTemplateNamePlaceholder(createDomain)}
                  value={createName}
                />
              </label>
              <label>
                <span>Scope</span>
                <select
                  aria-label="Template scope"
                  onChange={(event) => setCreateScope(event.target.value)}
                  value={createScope}
                >
                  <option value="shared">Shared</option>
                  <option value="vps_local">VPS-local</option>
                </select>
              </label>
            </div>
            {adapterDomainHelp(createDomain) && (
              <div
                className="operationNote sourceAdapterContract"
                role="note"
                title={adapterDomainContractTitle(createDomain) ?? undefined}
              >
                <Route size={17} />
                <span>{adapterDomainHelp(createDomain)}</span>
              </div>
            )}
            {createScope === "vps_local" && (
              <label>
                <span>Owner VPS</span>
                <VpsCombobox
                  agents={agents}
                  ariaLabel="VPS-local owner"
                  onChange={setOwnerClientId}
                  placeholder="Search owner VPS"
                  value={ownerClientId}
                />
              </label>
            )}
            <label>
              <span>Description</span>
              <input
                aria-label="Template description"
                onChange={(event) => {
                  setDescription(event.target.value);
                  setCreateFeedback(null);
                }}
                placeholder="description"
                value={description}
              />
            </label>
            <label>
              <span>Definition JSON</span>
              <textarea
                aria-label="Template definition JSON"
                onChange={(event) => {
                  setDefinitionText(event.target.value);
                  setCreateFeedback(null);
                }}
                value={definitionText}
              />
            </label>
            <button
              className="secondaryAction"
              disabled={
                pending ||
                !createName.trim() ||
                (createScope === "vps_local" && !ownerClientId)
              }
              type="submit"
            >
              Save template
            </button>
          </form>
        ) : (
          <div className="sourceTemplateDetailStack">
            <div
              className="templateDetailTabs"
              role="tablist"
              aria-label="Source template workflow"
            >
              {(planBoundAdapter
                ? [["lifecycle", "Test / update"]]
                : [
                    ["assign", "Assign"],
                    ["render", "Render"],
                    ["lifecycle", "Test / update"],
                  ]
              ).map(([value, label]) => (
                <button
                  aria-selected={detailTab === value}
                  className={detailTab === value ? "selected" : ""}
                  key={value}
                  onClick={() => setDetailTab(value as SourceTemplateDetailTab)}
                  role="tab"
                  type="button"
                >
                  {label}
                </button>
              ))}
            </div>
            {planBoundAdapter ? (
              <div
                className="operationNote sourceAdapterBindingNote"
                role="note"
              >
                <Route size={18} />
                <div>
                  <strong>Bound from tunnel plans</strong>
                  <span>
                    This definition is used only by explicit endpoint bindings;
                    it is never ambient VPS configuration.
                  </span>
                  <button
                    className="secondaryAction compactAction"
                    onClick={onOpenTunnelPlans}
                    type="button"
                  >
                    <Route size={14} />
                    <span>Open tunnel plans</span>
                  </button>
                </div>
              </div>
            ) : (
              <div className="timeline templateAssignmentSummary">
                <SlidersHorizontal size={18} />
                <div>
                  <strong>
                    {assignments.length} effective template{" "}
                    {assignments.length === 1 ? "assignment" : "assignments"}
                  </strong>
                  <span>{assignmentSummary(assignments, lastAssignment)}</span>
                </div>
              </div>
            )}
            <ActionFeedback
              className="localActionFeedback sourceTemplateActionFeedback"
              message={sourceWorkflowFeedbackMessage}
              tone={sourceWorkflowFeedbackTone}
            />

            {!planBoundAdapter && detailTab === "assign" && (
              <form
                className="compactForm templateForm"
                onSubmit={submitAssignment}
                ref={assignmentFormRef}
              >
                <strong>Assign template</strong>
                <span className="formHint">
                  Assign one template to a selector-resolved VPS set; preview
                  target count before confirmation.
                </span>
                <div className="formRow templateFormRow">
                  <label>
                    <span>Domain</span>
                    <select
                      aria-label="Assignment domain"
                      onChange={(event) =>
                        changeAssignDomain(event.target.value)
                      }
                      value={assignDomain}
                    >
                      {ASSIGNABLE_SOURCE_TEMPLATE_DOMAINS.map((domain) => (
                        <option key={domain} value={domain}>
                          {sourceDomainLabel(domain)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    <span>Template</span>
                    <select
                      aria-label="Template assignment template"
                      onChange={(event) => {
                        clearAssignmentConfirmation();
                        setAssignTemplateId(event.target.value);
                      }}
                      value={effectiveTemplateId}
                    >
                      {assignableTemplates.map((template) => (
                        <option key={template.id} value={template.id}>
                          {template.name}
                        </option>
                      ))}
                    </select>
                  </label>
                </div>
                <div className="targetSelector templateTargetSelector">
                  <div className="targetSelectorHeader">
                    <strong>Targets</strong>
                    <span>
                      {assignmentHasSelector
                        ? `${assignmentTargetCount}/${agents.length} matching VPSs`
                        : "Add a selector"}
                    </span>
                  </div>
                  <SearchExpressionInput
                    agents={agents}
                    ariaLabel="Template assignment target expression"
                    className="targetExpressionBar"
                    onChange={(value) => {
                      clearAssignmentConfirmation();
                      setAssignmentSelectorExpression(value);
                    }}
                    placeholder="id:edge-a || provider:alpha && country:us"
                    showMatchCount={assignmentHasSelector}
                    value={assignmentSelectorExpression}
                    verification={
                      assignmentSelectorParse.error
                        ? "invalid"
                        : assignmentSelectorExpression.trim()
                          ? "valid"
                          : "neutral"
                    }
                    verificationMessage={
                      assignmentSelectorParse.error ??
                      `${assignmentTargetCount}/${agents.length}`
                    }
                  />
                </div>
                {pendingConfirmation !== "assignment" && (
                  <button
                    className="secondaryAction"
                    disabled={
                      pending ||
                      !effectiveTemplateId ||
                      !assignmentSelectorExpression.trim() ||
                      Boolean(assignmentSelectorParse.error)
                    }
                    type="submit"
                  >
                    Review assignment
                  </button>
                )}
              </form>
            )}

            {!planBoundAdapter && detailTab === "render" && (
              <form
                className="compactForm templateForm"
                onSubmit={previewTemplateRuntimeConfig}
              >
                <strong>Render runtime config</strong>
                <span className="formHint">
                  Review the runtime config generated from one VPS's assigned
                  templates.
                </span>
                <label>
                  <span>Review VPS</span>
                  <VpsCombobox
                    agents={agents}
                    ariaLabel="Template runtime config preview VPS"
                    onChange={(value) => {
                      if (value === renderClientId) {
                        return;
                      }
                      setRenderClientId(value);
                      setRenderedTemplateRuntimeConfig(null);
                      clearApplyConfirmation();
                    }}
                    placeholder="Search review VPS"
                    value={renderClientId}
                  />
                </label>
                <button
                  className="secondaryAction"
                  disabled={pending || !renderClientId}
                  type="submit"
                >
                  Render config
                </button>
                {renderedTemplateRuntimeConfig && (
                  <div className="configPreview">
                    <div className="previewMeta">
                      <span>
                        {renderedTemplateRuntimeConfig.assignments.length}{" "}
                        resolved templates
                      </span>
                      <span
                        title={
                          renderedTemplateRuntimeConfig.unsupported_domains
                            .length
                            ? renderedTemplateRuntimeConfig.unsupported_domains.join(
                                "\n",
                              )
                            : "Every selected template contributes runtime config directly."
                        }
                      >
                        {
                          renderedTemplateRuntimeConfig.unsupported_domains
                            .length
                        }{" "}
                        workflow-managed
                      </span>
                      <span
                        title={
                          renderedTemplateRuntimeConfig.render_notes.length
                            ? renderedTemplateRuntimeConfig.render_notes.join(
                                "\n",
                              )
                            : "No additional render notes."
                        }
                      >
                        {renderedTemplateRuntimeConfig.render_notes.length}{" "}
                        render notes
                      </span>
                    </div>
                    <textarea
                      aria-label="Rendered template runtime config TOML"
                      readOnly
                      value={renderedTemplateRuntimeConfig.toml}
                    />
                  </div>
                )}
                <span className="formHint">
                  Template assignment and template updates are effective
                  immediately; this render is for inspection only.
                </span>
              </form>
            )}

            {detailTab === "lifecycle" && (
              <form
                className="compactForm templateForm"
                onSubmit={(event) => event.preventDefault()}
                ref={lifecycleFormRef}
              >
                <strong>Template lifecycle</strong>
                <span className="formHint">
                  {planBoundAdapter
                    ? "Diff, validate, clone, or update this operator-owned contract. Updates report every bound endpoint VPS before commit."
                    : "Diff, test, clone, or update a saved template. Updates report affected VPS count before commit."}
                </span>
                <div className="formRow templateFormRow">
                  <label>
                    <span>Template</span>
                    <select
                      aria-label="Lifecycle template"
                      onChange={(event) =>
                        setLifecycleTemplateId(event.target.value)
                      }
                      value={effectiveLifecycleTemplateId}
                    >
                      {templates.map((template) => (
                        <option key={template.id} value={template.id}>
                          {template.name}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    <span>Clone name</span>
                    <input
                      aria-label="Clone template name"
                      onChange={(event) => {
                        setLifecycleCloneName(event.target.value);
                        setLastCloneName(null);
                      }}
                      placeholder="shared:copy"
                      value={lifecycleCloneName}
                    />
                  </label>
                </div>
                <label>
                  <span>Description</span>
                  <input
                    aria-label="Lifecycle template description"
                    onChange={(event) => {
                      setLifecycleDescription(event.target.value);
                      clearLifecycleCandidateResults();
                    }}
                    placeholder="description"
                    value={lifecycleDescription}
                  />
                </label>
                {lifecycleTemplate &&
                  adapterDomainHelp(lifecycleTemplate.domain) && (
                    <div
                      className="operationNote sourceAdapterContract"
                      role="note"
                      title={
                        adapterDomainContractTitle(lifecycleTemplate.domain) ??
                        undefined
                      }
                    >
                      <Route size={17} />
                      <span>{adapterDomainHelp(lifecycleTemplate.domain)}</span>
                    </div>
                  )}
                <label>
                  <span>Definition JSON</span>
                  <textarea
                    aria-label="Lifecycle template definition JSON"
                    onChange={(event) => {
                      setLifecycleDefinitionText(event.target.value);
                      clearLifecycleCandidateResults();
                    }}
                    value={lifecycleDefinitionText}
                  />
                </label>
                <div className="formRow templateLifecycleActions">
                  <button
                    className="secondaryAction"
                    disabled={pending || !lifecycleTemplate}
                    onClick={diffLifecycleTemplate}
                    type="button"
                  >
                    Diff
                  </button>
                  <button
                    className="secondaryAction"
                    disabled={pending || !lifecycleTemplate}
                    onClick={testLifecycleTemplate}
                    title={
                      lifecycleTemplate &&
                      adapterDomainHelp(lifecycleTemplate.domain)
                        ? "Validates the saved contract shape. Endpoint status verifies that the operator-owned executable is present and working."
                        : undefined
                    }
                    type="button"
                  >
                    {lifecycleTemplate &&
                    adapterDomainHelp(lifecycleTemplate.domain)
                      ? "Validate definition"
                      : "Test"}
                  </button>
                  <button
                    className="secondaryAction"
                    disabled={
                      pending ||
                      !lifecycleTemplate ||
                      !lifecycleCloneName.trim()
                    }
                    onClick={cloneLifecycleTemplate}
                    type="button"
                  >
                    Clone
                  </button>
                  {pendingConfirmation !== "lifecycle-update" && (
                    <button
                      className="secondaryAction"
                      disabled={
                        pending ||
                        !lifecycleTemplate ||
                        lifecycleTemplate.built_in
                      }
                      onClick={() => void updateLifecycleTemplate()}
                      type="button"
                    >
                      Review update
                    </button>
                  )}
                </div>
                {(lastDiff || lastTest) && (
                  <div className="configPreview lifecyclePreview">
                    {lastDiff && (
                      <div className="previewMeta">
                        <span>
                          {lastDiff.affected_client_count} assigned VPSs
                        </span>
                        <span title={sourceTemplateDiffDetail(lastDiff)}>
                          {sourceTemplateDiffDetail(lastDiff)}
                        </span>
                      </div>
                    )}
                    {lastTest && (
                      <>
                        <div className="previewMeta">
                          <span>{lastTest.valid ? "valid" : "invalid"}</span>
                          <span>
                            {lastTest.renderable
                              ? "incremental patch renderable"
                              : "workflow-managed"}
                          </span>
                        </div>
                        {lastTest.toml && (
                          <textarea
                            aria-label="Tested template TOML"
                            readOnly
                            value={lastTest.toml}
                          />
                        )}
                        {lastTest.error && <span>{lastTest.error}</span>}
                      </>
                    )}
                  </div>
                )}
              </form>
            )}
          </div>
        )}
      </ConsoleActionDrawer>
    </section>
  );
}

function defaultCloneName(name: string): string {
  if (name.startsWith("builtin:")) {
    return `shared:${name.slice("builtin:".length)} (cloned)`;
  }
  return `${name} (cloned)`;
}

function sourceTemplatePrivilegeTarget(templateId: string) {
  return `source_template:${templateId}`;
}

function parseDefinition(value: string): JsonValue {
  const parsed = JSON.parse(value) as JsonValue;
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("Template definition must be a JSON object");
  }
  return parsed;
}

function assignmentSummary(
  assignments: SourceTemplateAssignmentRecord[],
  lastAssignment: AssignSourceTemplateResponse | null,
): string {
  if (lastAssignment?.confirmation_required) {
    return "Confirmation required before changing multiple VPS template selections";
  }
  const domains = new Set(assignments.map((assignment) => assignment.domain));
  return domains.size === 0
    ? "No effective VPS template assignments loaded"
    : `${domains.size} source domains active across scoped VPSs`;
}

function runtimeDispatchSummary(sync: RuntimeConfigDispatchRecord[]): string {
  if (sync.length === 0) return "no runtime apply required";
  const failures = sync.filter((outcome) => outcome.status !== "queued");
  if (failures.length === 0) {
    return `runtime apply queued for ${sync.length} endpoint${sync.length === 1 ? "" : "s"}`;
  }
  return `desired state saved; runtime apply was not queued for ${failures.map((outcome) => `${outcome.client_id}: ${dispatchFailureReason(outcome.error, outcome.status, "Runtime apply job")}`).join("; ")}`;
}

function sourceEvidenceSummary(row: SourceStatusRecord): string {
  const evidence = row.evidence;
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    return row.status_reason;
  }
  const sampleCount =
    typeof evidence.sample_count === "number" ? evidence.sample_count : null;
  const degradedCount =
    typeof evidence.degraded_count === "number"
      ? evidence.degraded_count
      : null;
  const objectStoreConfigured =
    typeof evidence.server_object_store_configured === "boolean"
      ? evidence.server_object_store_configured
      : null;
  const objectStoreKind =
    typeof evidence.server_object_store_kind === "string"
      ? evidence.server_object_store_kind
      : null;
  const artifactCount =
    typeof evidence.artifact_count === "number"
      ? evidence.artifact_count
      : null;
  const releaseCount =
    typeof evidence.release_count === "number" ? evidence.release_count : null;
  const externalReleaseCount =
    typeof evidence.external_release_count === "number"
      ? evidence.external_release_count
      : null;
  const backupRequestCount =
    typeof evidence.backup_request_count === "number"
      ? evidence.backup_request_count
      : null;
  const restoreSourceCount =
    typeof evidence.restore_source_count === "number"
      ? evidence.restore_source_count
      : null;
  const restoreTargetCount =
    typeof evidence.restore_target_count === "number"
      ? evidence.restore_target_count
      : null;
  const migrationSourceCount =
    typeof evidence.migration_source_count === "number"
      ? evidence.migration_source_count
      : null;
  const migrationTargetCount =
    typeof evidence.migration_target_count === "number"
      ? evidence.migration_target_count
      : null;
  const probeSampleCount =
    typeof evidence.probe_sample_count === "number"
      ? evidence.probe_sample_count
      : null;
  const speedSampleCount =
    typeof evidence.speed_sample_count === "number"
      ? evidence.speed_sample_count
      : null;
  const routingRecommendationCount =
    typeof evidence.routing_recommendation_count === "number"
      ? evidence.routing_recommendation_count
      : null;
  const ospfUpdateCandidateCount =
    typeof evidence.ospf_update_candidate_count === "number"
      ? evidence.ospf_update_candidate_count
      : null;
  const trafficLimitPlanCount =
    typeof evidence.traffic_limit_plan_count === "number"
      ? evidence.traffic_limit_plan_count
      : null;
  const workflow =
    typeof evidence.workflow === "string" ? evidence.workflow : null;
  const privilegeGated =
    typeof evidence.privilege_gated === "boolean"
      ? evidence.privilege_gated
      : null;
  const environmentPolicy =
    typeof evidence.environment_policy === "string"
      ? evidence.environment_policy
      : null;
  const ptyPolicy =
    typeof evidence.pty_policy === "string" ? evidence.pty_policy : null;
  const processCleanup =
    typeof evidence.process_cleanup === "string"
      ? evidence.process_cleanup
      : null;
  const configuredPing =
    typeof evidence.configured_ping_argv === "boolean"
      ? evidence.configured_ping_argv
      : null;
  const customCommand =
    typeof evidence.custom_command_configured === "boolean"
      ? evidence.custom_command_configured
      : null;
  const requiresTwoEndpoints =
    typeof evidence.requires_two_endpoints === "boolean"
      ? evidence.requires_two_endpoints
      : null;
  const privilegeMode =
    typeof evidence.privilege_mode === "string"
      ? evidence.privilege_mode
      : null;
  const processLimitsStatus =
    typeof evidence.process_limits_status === "string"
      ? evidence.process_limits_status
      : null;
  const canApplyProcessLimits =
    typeof evidence.can_apply_process_limits === "boolean"
      ? evidence.can_apply_process_limits
      : null;
  const parts = [];
  if (workflow) {
    parts.push(formatSourceToken(workflow));
  }
  if (privilegeGated) {
    parts.push("privilege-unlocked");
  }
  if (environmentPolicy) {
    parts.push(`${environmentPolicy} env`);
  }
  if (ptyPolicy) {
    parts.push(`${formatSourceToken(ptyPolicy)} PTY`);
  }
  if (processCleanup) {
    parts.push(`${formatSourceToken(processCleanup)} cleanup`);
  }
  if (configuredPing) {
    parts.push("configured ping");
  }
  if (customCommand) {
    parts.push("custom command");
  }
  if (requiresTwoEndpoints) {
    parts.push("paired endpoints");
  }
  if (privilegeMode) {
    parts.push(formatSourceToken(privilegeMode));
  }
  if (processLimitsStatus) {
    parts.push(
      canApplyProcessLimits === true
        ? "process limits available"
        : `${formatSourceToken(processLimitsStatus)} process limits`,
    );
  }
  if (objectStoreConfigured !== null) {
    parts.push(
      objectStoreConfigured
        ? `${objectStoreKind ?? "configured"} store`
        : "no server store",
    );
  }
  if (artifactCount !== null) {
    parts.push(`${artifactCount} artifacts`);
  }
  if (releaseCount !== null) {
    parts.push(`${releaseCount} releases`);
  }
  if (externalReleaseCount !== null && externalReleaseCount > 0) {
    parts.push(`${externalReleaseCount} external`);
  }
  if (backupRequestCount !== null && backupRequestCount > 0) {
    parts.push(`${backupRequestCount} backup requests`);
  }
  if (restoreSourceCount !== null && restoreSourceCount > 0) {
    parts.push(`${restoreSourceCount} source restores`);
  }
  if (restoreTargetCount !== null && restoreTargetCount > 0) {
    parts.push(`${restoreTargetCount} target restores`);
  }
  if (migrationSourceCount !== null && migrationSourceCount > 0) {
    parts.push(`${migrationSourceCount} source migrations`);
  }
  if (migrationTargetCount !== null && migrationTargetCount > 0) {
    parts.push(`${migrationTargetCount} target migrations`);
  }
  if (probeSampleCount !== null && probeSampleCount > 0) {
    parts.push(`${probeSampleCount} probe samples`);
  }
  if (speedSampleCount !== null && speedSampleCount > 0) {
    parts.push(`${speedSampleCount} speed samples`);
  }
  if (routingRecommendationCount !== null && routingRecommendationCount > 0) {
    parts.push(`${routingRecommendationCount} routing recommendations`);
  }
  if (ospfUpdateCandidateCount !== null && ospfUpdateCandidateCount > 0) {
    parts.push(`${ospfUpdateCandidateCount} OSPF updates`);
  }
  if (trafficLimitPlanCount !== null && trafficLimitPlanCount > 0) {
    parts.push(`${trafficLimitPlanCount} traffic limit plans`);
  }
  if (sampleCount !== null) {
    parts.push(`${sampleCount} samples`);
  }
  if (degradedCount !== null && degradedCount > 0) {
    parts.push(`${degradedCount} degraded`);
  }
  return parts.length > 0 ? parts.join(", ") : row.status_reason;
}

function sourceStatusTone(status: string): "ok" | "neutral" | "warning" {
  switch (status) {
    case "ok":
    case "ready":
    case "ready_on_demand":
      return "ok";
    case "selected":
    case "selected_workflow":
    case "metadata_only":
      return "neutral";
    default:
      return "warning";
  }
}

function sourceStatusLabel(status: string): string {
  switch (status) {
    case "ok":
      return "Ready";
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
      return "Agent offline";
    case "unknown_domain":
      return "Unknown source domain";
    case "selected_no_store":
      return "Source selected; server storage not configured";
    case "selected_no_artifacts":
      return "Source selected; no artifacts";
    case "selected_no_limits":
      return "Source selected; limits unavailable";
    case "selected_no_samples":
      return "Source selected; no samples";
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

function defaultDefinitionForDomain(
  domain: string,
  templates: SourceTemplateRecord[],
): string {
  if (domain === "runtime_tunnel_adapter") {
    return JSON.stringify(
      {
        manager: "external_managed_adapter",
        contract_version: 1,
        startup_command: {
          argv: [
            "/opt/operator/tunnel-adapter",
            "start",
            "--interface",
            "{interface}",
            "--kind",
            "{kind}",
            "--local-source",
            "{local_underlay}",
            "--remote-destination",
            "{remote_underlay}",
            "--local-address",
            "{local_address}",
            "--remote-address",
            "{remote_address}",
            "--prefix-len",
            "{prefix_len}",
          ],
          max_timeout_secs: 10,
          max_output_bytes: 16384,
        },
        cleanup_command: {
          argv: [
            "/opt/operator/tunnel-adapter",
            "cleanup",
            "--interface",
            "{interface}",
          ],
          max_timeout_secs: 10,
          max_output_bytes: 16384,
        },
        status_command: {
          argv: [
            "/opt/operator/tunnel-adapter",
            "status",
            "--interface",
            "{interface}",
          ],
          max_timeout_secs: 10,
          max_output_bytes: 16384,
        },
      },
      null,
      2,
    );
  }
  if (domain === "routing_cost_adapter") {
    return JSON.stringify(
      {
        contract_version: 1,
        status_command: {
          argv: ["/opt/operator/routing-cost-adapter", "status"],
          max_timeout_secs: 10,
          max_output_bytes: 16384,
        },
        update_command: {
          argv: ["/opt/operator/routing-cost-adapter", "apply"],
          max_timeout_secs: 10,
          max_output_bytes: 16384,
        },
      },
      null,
      2,
    );
  }
  const starter =
    templates.find(
      (template) => template.domain === domain && template.is_default,
    ) ?? templates.find((template) => template.domain === domain);
  if (starter) {
    return JSON.stringify(starter.definition, null, 2);
  }
  return DEFAULT_DEFINITION;
}

function adapterDomainHelp(domain: string): string | null {
  if (domain === "runtime_tunnel_adapter") {
    return "Direct absolute argv controls only the declared tunnel. vpsman substitutes plan placeholders and never installs, edits, or removes the executable.";
  }
  if (domain === "routing_cost_adapter") {
    return "Status and update receive contract-v1 JSON on stdin and must return contract-v1 JSON. vpsman never assumes or configures a routing daemon.";
  }
  return null;
}

function sourceTemplateNamePlaceholder(domain: string): string {
  if (domain === "runtime_tunnel_adapter") {
    return "shared:tunnel-adapter";
  }
  if (domain === "routing_cost_adapter") {
    return "shared:routing-cost-adapter";
  }
  return `shared:${domain.replace(/_/g, "-")}`;
}

function isPlanBoundAdapterDomain(domain: string): boolean {
  return (
    domain === "runtime_tunnel_adapter" || domain === "routing_cost_adapter"
  );
}

function adapterDomainContractTitle(domain: string): string | null {
  if (domain === "runtime_tunnel_adapter") {
    return "Supported placeholders include endpoint-specific {remote_underlay} and optional {local_underlay}; the latter is empty when OS source selection is requested. Tunnel-interface addresses, IPv4/IPv6 variants, FOU values, and traffic limits are also available. Status exits 0 when ready.";
  }
  if (domain === "routing_cost_adapter") {
    return "Request fields include operation, plan and endpoint identity, addresses, expected_current_cost, and desired_cost. Response fields include contract_version, interface_name, ready, current_cost, applied_cost, adapter_version, and message.";
  }
  return null;
}

function sourceTokenLabel(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    return "Not configured";
  }
  return trimmed
    .replace(/_/g, " ")
    .replace(/\b\w/g, (match) => match.toUpperCase())
    .replace(/\bJson\b/g, "JSON")
    .replace(/\bToml\b/g, "TOML")
    .replace(/\bSha\b/g, "SHA")
    .replace(/\bVps\b/g, "VPS")
    .replace(/\bOspf\b/g, "OSPF");
}

function formatSourceToken(value: string): string {
  return value.replace(/_/g, " ");
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.min(max, Math.max(min, Math.trunc(value)));
}

function sourceTemplateDiffHasChanges(
  diff: SourceTemplateDiffResponse,
): boolean {
  return diff.description_changed || diff.definition_changed;
}

function sourceTemplateDiffSummary(diff: SourceTemplateDiffResponse): string {
  const changes: string[] = [];
  if (diff.description_changed) {
    changes.push("Description");
  }
  if (diff.definition_changed) {
    const count = diff.changed_keys.length;
    changes.push(
      count > 0
        ? `${count} definition ${count === 1 ? "key" : "keys"}`
        : "Definition",
    );
  }
  return changes.length > 0
    ? `${changes.join(" and ")} changed`
    : "No template changes";
}

function sourceTemplateDiffDetail(diff: SourceTemplateDiffResponse): string {
  if (!sourceTemplateDiffHasChanges(diff)) {
    return "no template changes";
  }
  const changes: string[] = [];
  if (diff.description_changed) {
    changes.push("description changed");
  }
  if (diff.definition_changed) {
    changes.push(
      diff.changed_keys.length > 0
        ? `definition: ${diff.changed_keys.join(", ")}`
        : "definition changed",
    );
  } else {
    changes.push("no definition changes");
  }
  return changes.join(" · ");
}

function sourceTemplateUsageLabel(
  planBoundAdapter: boolean,
  count: number,
): string {
  const resource = count === 1 ? "VPS" : "VPSs";
  return planBoundAdapter
    ? `bound endpoint ${resource}`
    : `assigned ${resource}`;
}

function scrollIntoViewSoon(element: HTMLElement | null) {
  if (!element) {
    return;
  }
  window.requestAnimationFrame(() => {
    scrollIntoViewWithMotion(element, { block: "start" });
  });
}

function readLocalString(key: string, fallback: string): string {
  if (typeof window === "undefined") {
    return fallback;
  }
  return window.localStorage.getItem(key) ?? fallback;
}

function writeLocalString(key: string, value: string) {
  if (typeof window === "undefined") {
    return;
  }
  if (value.trim()) {
    window.localStorage.setItem(key, value);
  } else {
    window.localStorage.removeItem(key);
  }
}
