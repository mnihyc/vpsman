import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  DatabaseZap,
  FileText,
  Pencil,
  Plus,
  SlidersHorizontal,
  UserPlus,
} from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
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
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import { sourceReadinessStatusBadgeClass } from "../jobStatusPresentation";
import { scrollIntoViewWithMotion } from "../motion";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../searchExpression";
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
  UpdateSourceTemplateRequest,
  UpdateSourceTemplateResponse,
} from "../types";
import { formatTime, formatVpsName, runPanelAction, shortId } from "../utils";

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
  "routing_daemon_adapter",
  "backup_object_store",
  "restore_path_mapping",
  "update_artifact_source",
  "update_restart_policy",
  "update_rollback_heartbeat_source",
];

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
};

type SourceTemplateLifecycleUpdateSnapshot = {
  assignedClientCount: number;
  description: string | null;
  definition: JsonValue;
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
  onRenderTemplateRuntimeConfig,
  onResolveBulk,
  onTestTemplate,
  onUpdateTemplate,
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
  templates: SourceTemplateRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const createFormRef = useRef<HTMLFormElement | null>(null);
  const assignmentFormRef = useRef<HTMLFormElement | null>(null);
  const lifecycleFormRef = useRef<HTMLFormElement | null>(null);
  const [drawerMode, setDrawerMode] = useState<SourceTemplateDrawerMode>(null);
  const [detailTab, setDetailTab] = useState<SourceTemplateDetailTab>("assign");
  const [createDomain, setCreateDomain] = useState(SOURCE_TEMPLATE_DOMAINS[1]);
  const [createName, setCreateName] = useState("");
  const [createScope, setCreateScope] = useState("shared");
  const [ownerClientId, setOwnerClientId] = useState("");
  const [description, setDescription] = useState("");
  const [definitionText, setDefinitionText] = useState(DEFAULT_DEFINITION);
  const [assignDomain, setAssignDomain] = useState(SOURCE_TEMPLATE_DOMAINS[1]);
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
  const [actionError, setActionError] = useState<string | null>(null);
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
  const assignmentSelectorParse = useMemo(
    () => parseSearchExpression(assignmentSelectorExpression),
    [assignmentSelectorExpression],
  );
  const assignmentTargetCount = useMemo(
    () =>
      assignmentSelectorParse.error
        ? 0
        : agentsMatchingExpression(agents, assignmentSelectorExpression).length,
    [agents, assignmentSelectorExpression, assignmentSelectorParse.error],
  );
  const lifecycleStatus = lastUpdate?.confirmation_required
    ? `${lastUpdate.affected_client_count} VPSs inherit this template; confirmation required`
    : lastUpdate
      ? `${lastUpdate.affected_client_count} VPSs inherited the template update`
      : lastTest
        ? lastTest.valid
          ? `${lastTest.renderable ? "Renderable" : "Workflow"} template test passed for ${lastTest.domain}`
          : `Template test failed: ${lastTest.error ?? "invalid definition"}`
        : lastDiff
          ? `${lastDiff.changed_keys.length} keys changed; ${lastDiff.affected_client_count} VPSs affected`
          : null;
  const status =
    actionError ??
    reviewStatus ??
    lifecycleStatus ??
    (sourceStatus.length > 0 ? sourceStatusSummary : null) ??
    (lastAssignment
      ? `${lastAssignment.target_count} template assignments evaluated`
      : `${templates.length} templates across ${new Set(templates.map((template) => template.domain)).size} domains`);
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
        header: "Assigned",
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
        label: "Open template",
        description: (rows) =>
          rows.length === 1
            ? `Open ${rows[0].name} template detail.`
            : "Select exactly one template to open.",
        disabled: (rows) => rows.length !== 1,
        icon: <FileText size={14} />,
        onSelect: (rows) => openTemplateDetail(rows[0], "assign"),
      },
      {
        label: "Assign template",
        description: (rows) =>
          rows.length === 1
            ? `Load ${rows[0].name} into the assignment form.`
            : "Select exactly one template to assign.",
        disabled: (rows) => rows.length !== 1,
        icon: <UserPlus size={14} />,
        onSelect: (rows) => prepareTemplateAssignment(rows[0]),
      },
      {
        label: "Edit / test template",
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
    await runPanelAction(setPending, setActionError, async () => {
      await onCreateTemplate({
        definition: parseDefinition(definitionText),
        description: description.trim() || null,
        domain: createDomain,
        name: createName.trim(),
        owner_client_id:
          createScope === "vps_local" ? ownerClientId || null : null,
        scope: createScope,
      });
      setCreateName("");
      setDescription("");
      setDefinitionText(DEFAULT_DEFINITION);
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
        setLastAssignment(preview);
        setAssignmentSnapshot({
          assignments: preview.assignments,
          domain: frozenDomain,
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
      const response = await onAssignTemplate({
        confirmed: true,
        domain: snapshot.domain,
        template_id: snapshot.templateId,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.targetClientIds,
      });
      setLastAssignment(response);
      setAssignmentSnapshot(null);
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
      setLastDiff(null);
      setLastUpdate(null);
    });
  }

  async function cloneLifecycleTemplate() {
    if (!lifecycleTemplate || !lifecycleCloneName.trim()) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      await onCloneTemplate(lifecycleTemplate.id, {
        description:
          lifecycleDescription.trim() || lifecycleTemplate.description,
        name: lifecycleCloneName.trim(),
        owner_client_id: null,
        scope: "shared",
      });
      setLastDiff(null);
      setLastTest(null);
      setLastUpdate(null);
    });
  }

  function updateLifecycleTemplate() {
    if (!lifecycleTemplate || lifecycleTemplate.built_in) {
      return;
    }
    setLifecycleUpdateSnapshot({
      assignedClientCount: lifecycleTemplate.assigned_client_count,
      description: lifecycleDescription.trim() || null,
      definition: parseDefinition(lifecycleDefinitionText),
      templateId: lifecycleTemplate.id,
      templateName: lifecycleTemplate.name,
    });
    setPendingConfirmation("lifecycle-update");
  }

  async function executeLifecycleTemplateUpdate(
    snapshot: SourceTemplateLifecycleUpdateSnapshot,
  ) {
    await runPanelAction(setPending, setActionError, async () => {
      const response = await onUpdateTemplate(snapshot.templateId, {
        confirmed: true,
        definition: snapshot.definition,
        description: snapshot.description,
      });
      setLastUpdate(response);
      setLastDiff(response.diff);
      setLastTest(null);
      setLifecycleUpdateSnapshot(null);
    });
  }

  async function confirmSourceTemplateAction() {
    const action = pendingConfirmation;
    if (!action) {
      return;
    }
    setPendingConfirmation(null);
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
            value: assignmentSnapshot?.domain ?? assignDomain,
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

  function changeAssignDomain(domain: string) {
    clearAssignmentConfirmation();
    setAssignDomain(domain);
    setAssignTemplateId("");
    setLastAssignment(null);
  }

  function prepareTemplateAssignment(template: SourceTemplateRecord) {
    clearAssignmentConfirmation();
    setAssignDomain(template.domain);
    setAssignTemplateId(template.id);
    setLastAssignment(null);
    setLifecycleTemplateId(template.id);
    setDetailTab("assign");
    setDrawerMode("detail");
  }

  function prepareNewTemplate() {
    setCreateDomain(SOURCE_TEMPLATE_DOMAINS[1]);
    setCreateName("");
    setCreateScope("shared");
    setOwnerClientId("");
    setDescription("");
    setDefinitionText(DEFAULT_DEFINITION);
    setActionError(null);
    setDrawerMode("create");
  }

  function prepareTemplateLifecycle(template: SourceTemplateRecord) {
    clearLifecycleUpdateConfirmation();
    setLifecycleTemplateId(template.id);
    setLastDiff(null);
    setLastTest(null);
    setLastUpdate(null);
    setAssignDomain(template.domain);
    setAssignTemplateId(template.id);
    setDetailTab("lifecycle");
    setDrawerMode("detail");
  }

  function openTemplateDetail(
    template: SourceTemplateRecord,
    tab: SourceTemplateDetailTab,
  ) {
    setLifecycleTemplateId(template.id);
    setAssignDomain(template.domain);
    setAssignTemplateId(template.id);
    setDetailTab(tab);
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
        />
      )}

      {showTemplateManagement && (
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
              <span>
                {actionError ?? "No template records match the current search."}
              </span>
            </div>
          }
          renderExpandedRow={(template) => (
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
              <span>Assigned VPSs</span>
              <strong>{template.assigned_client_count}</strong>
              <span>Description</span>
              <strong>{template.description ?? "None"}</strong>
            </div>
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
                    (total, template) => total + template.assigned_client_count,
                    0,
                  )}
                </strong>
                assigned VPSs
              </span>
            </div>
          )}
          rowActions={templateActions}
          onOpenRow={(template) => openTemplateDetail(template, "assign")}
          rows={templates}
          searchPlaceholder="Search templates"
          storageKey="vpsman.sourceTemplates.registry"
          title="Template registry"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              onClick={prepareNewTemplate}
              type="button"
            >
              <Plus size={15} />
              <span>New template</span>
            </button>
          }
        />
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
              ? `${sourceDomainLabel(lifecycleTemplate.domain)} · ${lifecycleTemplate.assigned_client_count} assigned VPSs`
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
            <div className="formRow templateFormRow">
              <label>
                <span>Domain</span>
                <select
                  aria-label="Template domain"
                  onChange={(event) => setCreateDomain(event.target.value)}
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
                  onChange={(event) => setCreateName(event.target.value)}
                  placeholder="shared:vnstat-json"
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
                onChange={(event) => setDescription(event.target.value)}
                placeholder="description"
                value={description}
              />
            </label>
            <label>
              <span>Definition JSON</span>
              <textarea
                aria-label="Template definition JSON"
                onChange={(event) => setDefinitionText(event.target.value)}
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
              {[
                ["assign", "Assign"],
                ["render", "Render"],
                ["lifecycle", "Test / update"],
              ].map(([value, label]) => (
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
            <div className="timeline templateAssignmentSummary">
              <SlidersHorizontal size={18} />
              <div>
                <strong>
                  {assignments.length} template assignment records
                </strong>
                <span>{assignmentSummary(assignments, lastAssignment)}</span>
              </div>
            </div>

            {detailTab === "assign" && (
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
                      {SOURCE_TEMPLATE_DOMAINS.map((domain) => (
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
                      {assignmentSelectorParse.error ??
                        `${assignmentTargetCount}/${agents.length} matching VPSs`}
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
                    showMatchCount
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

            {detailTab === "render" && (
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
                      <span>
                        {
                          renderedTemplateRuntimeConfig.unsupported_domains
                            .length
                        }{" "}
                        notes
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
                  Diff, test, clone, or update a saved template. Updates report
                  affected VPS count before commit.
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
                      onChange={(event) =>
                        setLifecycleCloneName(event.target.value)
                      }
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
                      clearLifecycleUpdateConfirmation();
                    }}
                    placeholder="description"
                    value={lifecycleDescription}
                  />
                </label>
                <label>
                  <span>Definition JSON</span>
                  <textarea
                    aria-label="Lifecycle template definition JSON"
                    onChange={(event) => {
                      setLifecycleDefinitionText(event.target.value);
                      clearLifecycleUpdateConfirmation();
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
                    type="button"
                  >
                    Test
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
                      onClick={updateLifecycleTemplate}
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
                        <span>
                          {lastDiff.changed_keys.length
                            ? lastDiff.changed_keys.join(", ")
                            : "no definition changes"}
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
    return `shared:${name.slice("builtin:".length)}`;
  }
  return `${name}.copy`;
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
    ? "No VPS template assignments loaded"
    : `${domains.size} domains with explicit VPS source selections`;
}

function sourceEvidenceSummary(row: SourceStatusRecord): string {
  const evidence = row.evidence;
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    return row.status_reason;
  }
  const sampleCount =
    typeof evidence.sample_count === "number" ? evidence.sample_count : null;
  const promotionRequired =
    typeof evidence.promotion_required === "number"
      ? evidence.promotion_required
      : null;
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
  if (promotionRequired !== null && promotionRequired > 0) {
    parts.push(`${promotionRequired} promotion`);
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
