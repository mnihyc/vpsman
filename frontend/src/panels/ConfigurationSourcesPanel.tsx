import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  Copy,
  FileJson,
  Plus,
  RefreshCw,
  RotateCcw,
  Settings2,
  Trash2,
  X,
} from "lucide-react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { handleTabListKeyDown, tabId } from "../components/AccessibleTabs";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConsoleActionDrawer } from "../components/ConsoleLayout";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../searchExpression";
import type {
  AgentView,
  ApplyConfigurationSourceOverrideRequest,
  ApplyConfigurationSourceOverrideResponse,
  CloneConfigurationPresetRequest,
  ConfigurationBehavior,
  ConfigurationPresetPreview,
  ConfigurationPresetRecord,
  ConfigurationSourceOverridePreview,
  ConfigurationSourceOverrideRequest,
  ConfigurationSourceView,
  CreateConfigurationPresetRequest,
  EffectiveAgentConfigResponse,
  JsonValue,
  UpdateConfigurationPresetRequest,
  UpdateConfigurationPresetResponse,
} from "../types";
import {
  formatTime,
  formatVpsName,
  runPanelAction,
  type VpsNameDisplayMode,
} from "../utils";
import { usePanelDisplaySettings } from "../panelDisplay";

const BEHAVIORS: readonly ConfigurationBehavior[] = [
  "host_metrics",
  "tunnel_traffic",
  "latency_probe",
  "ospf_update_command",
  "process_inventory",
  "user_sessions",
  "command_execution",
];

type DrawerState =
  | { kind: "assign"; source: ConfigurationSourceView | null }
  | { kind: "create"; behavior: ConfigurationBehavior }
  | { kind: "clone"; preset: ConfigurationPresetRecord }
  | { kind: "edit"; preset: ConfigurationPresetRecord }
  | null;

type PendingConfirmation =
  | {
      kind: "override";
      request: ConfigurationSourceOverrideRequest;
      preview: ConfigurationSourceOverridePreview;
    }
  | {
      kind: "preset";
      preset: ConfigurationPresetRecord;
      preview: ConfigurationPresetPreview;
      request: {
        description: string | null;
        definition: Record<string, JsonValue>;
      };
    }
  | {
      kind: "delete";
      preset: ConfigurationPresetRecord;
    }
  | null;

type LocalFeedback = {
  message: string;
  tone: ActionFeedbackTone;
};

type EditorTextDrafts = Record<string, string>;

export function ConfigurationSourcesPanel({
  agents,
  error,
  loading,
  onApplyOverride,
  onClonePreset,
  onCreatePreset,
  onDeletePreset,
  onLoadEffectiveConfig,
  onPreviewOverride,
  onPreviewPreset,
  onRefresh,
  onUpdatePreset,
  presets,
  privilegeMaterial,
  setPrivilegeMaterial,
  sources,
}: {
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onApplyOverride: (
    request: ApplyConfigurationSourceOverrideRequest,
  ) => Promise<ApplyConfigurationSourceOverrideResponse>;
  onClonePreset: (
    presetId: string,
    request: CloneConfigurationPresetRequest,
  ) => Promise<ConfigurationPresetRecord>;
  onCreatePreset: (
    request: CreateConfigurationPresetRequest,
  ) => Promise<ConfigurationPresetRecord>;
  onDeletePreset: (presetId: string) => Promise<void>;
  onLoadEffectiveConfig: (
    clientId: string,
  ) => Promise<EffectiveAgentConfigResponse>;
  onPreviewOverride: (
    request: ConfigurationSourceOverrideRequest,
  ) => Promise<ConfigurationSourceOverridePreview>;
  onPreviewPreset: (
    presetId: string,
    request: {
      description?: string | null;
      definition: JsonValue;
    },
  ) => Promise<ConfigurationPresetPreview>;
  onRefresh: () => Promise<void>;
  onUpdatePreset: (
    presetId: string,
    request: UpdateConfigurationPresetRequest,
  ) => Promise<UpdateConfigurationPresetResponse>;
  presets: ConfigurationPresetRecord[];
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  sources: ConfigurationSourceView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [view, setView] = useState<"effective" | "presets">("effective");
  const [drawer, setDrawer] = useState<DrawerState>(null);
  const [confirmation, setConfirmation] = useState<PendingConfirmation>(null);
  const [pending, setPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<LocalFeedback | null>(null);
  const [targetCandidate, setTargetCandidate] = useState("");
  const [directTargetIds, setDirectTargetIds] = useState<string[]>([]);
  const [selectorExpression, setSelectorExpression] = useState("");
  const [assignBehavior, setAssignBehavior] =
    useState<ConfigurationBehavior>("host_metrics");
  const [assignPresetId, setAssignPresetId] = useState("");
  const [renderedConfig, setRenderedConfig] =
    useState<EffectiveAgentConfigResponse | null>(null);
  const [editorName, setEditorName] = useState("");
  const [editorDescription, setEditorDescription] = useState("");
  const [editorBehavior, setEditorBehavior] =
    useState<ConfigurationBehavior>("host_metrics");
  const [editorDefinition, setEditorDefinition] = useState<
    Record<string, JsonValue>
  >(() => defaultDefinition("host_metrics"));
  const [editorTextDrafts, setEditorTextDrafts] = useState<EditorTextDrafts>(
    () => textDraftsForDefinition(defaultDefinition("host_metrics")),
  );
  const inspectionGeneration = useRef(0);
  const assignmentReviewGeneration = useRef(0);

  const agentById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent])),
    [agents],
  );
  const parsedAssignmentSelector = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const selectorTargetIds = useMemo(
    () =>
      selectorExpression.trim() && !parsedAssignmentSelector.error
        ? agentsMatchingExpression(agents, selectorExpression).map(
            (agent) => agent.id,
          )
        : [],
    [agents, parsedAssignmentSelector.error, selectorExpression],
  );
  const assignmentTargetIds = useMemo(() => {
    const direct = [...new Set(directTargetIds)];
    const directSet = new Set(direct);
    const matched = selectorTargetIds
      .filter((clientId) => !directSet.has(clientId))
      .sort((left, right) => {
        const leftAgent = agentById.get(left);
        const rightAgent = agentById.get(right);
        const leftName = leftAgent
          ? formatVpsName(leftAgent, vpsNameDisplayMode)
          : left;
        const rightName = rightAgent
          ? formatVpsName(rightAgent, vpsNameDisplayMode)
          : right;
        return leftName.localeCompare(rightName) || left.localeCompare(right);
      });
    return [...direct, ...matched];
  }, [
    agentById,
    directTargetIds,
    selectorTargetIds,
    vpsNameDisplayMode,
  ]);
  const assignablePresets = useMemo(
    () =>
      presets.filter(
        (preset) => preset.behavior === assignBehavior && !preset.is_default,
      ),
    [assignBehavior, presets],
  );
  const effectiveAssignPresetId = assignPresetId;
  const assignmentReviewBaseBlockedReason = pending
    ? "Wait for the current operation to finish."
    : parsedAssignmentSelector.error
      ? `Fix the target selector before review: ${parsedAssignmentSelector.error}`
      : assignmentTargetIds.length === 0
        ? "Choose at least one VPS directly or with a matching selector."
        : null;
  const openedAssignmentSource =
    drawer?.kind === "assign" ? drawer.source : null;
  const assignmentIsExactOpenedRow =
    openedAssignmentSource !== null &&
    openedAssignmentSource.behavior === assignBehavior &&
    directTargetIds.length === 1 &&
    directTargetIds[0] === openedAssignmentSource.client_id &&
    !selectorExpression.trim();
  const setAssignmentNoOp =
    assignmentIsExactOpenedRow &&
    Boolean(effectiveAssignPresetId) &&
    openedAssignmentSource?.selection_origin === "explicit_override" &&
    effectiveAssignPresetId === openedAssignmentSource.effective_preset_id;
  const resetAssignmentNoOp =
    assignmentIsExactOpenedRow &&
    openedAssignmentSource?.selection_origin === "system_default";
  const setAssignmentReviewBlockedReason =
    assignmentReviewBaseBlockedReason ??
    (setAssignmentNoOp
      ? "Choose a different preset or target; this VPS already has that explicit preset."
      : null);
  const resetAssignmentReviewBlockedReason =
    assignmentReviewBaseBlockedReason ??
    (resetAssignmentNoOp
      ? "Choose a different target; this VPS already inherits the system default."
      : null);
  const primaryAssignmentReviewBlockedReason = effectiveAssignPresetId
    ? setAssignmentReviewBlockedReason
    : resetAssignmentReviewBlockedReason;
  const sourceAttentionCount = sources.filter(
    (source) =>
      ["failed", "stale"].includes(source.runtime_sync.state) ||
      ["degraded", "failed", "invalid"].includes(source.readiness.state),
  ).length;
  const sourceUnconfiguredCount = sources.filter(
    (source) => source.readiness.state === "unconfigured",
  ).length;
  const sourceVpsCount = new Set(sources.map((source) => source.client_id))
    .size;

  useEffect(() => {
    void runPanelAction(setPending, setActionError, onRefresh);
  }, [onRefresh]);

  function resetMessages() {
    setActionError(null);
    setFeedback(null);
  }

  function clearEffectiveConfigInspection() {
    inspectionGeneration.current += 1;
    setRenderedConfig(null);
  }

  function replaceEditorDefinition(definition: Record<string, JsonValue>) {
    setEditorDefinition(definition);
    setEditorTextDrafts(textDraftsForDefinition(definition));
  }

  function openAssignment(source: ConfigurationSourceView | null = null) {
    assignmentReviewGeneration.current += 1;
    setPending(false);
    resetMessages();
    const behavior = source?.behavior ?? "host_metrics";
    setAssignBehavior(behavior);
    setAssignPresetId(
      source?.selection_origin === "explicit_override"
        ? source.effective_preset_id
        : "",
    );
    setDirectTargetIds(source ? [source.client_id] : []);
    setTargetCandidate("");
    setSelectorExpression("");
    clearEffectiveConfigInspection();
    setDrawer({ kind: "assign", source });
  }

  function openCreate(behavior: ConfigurationBehavior = "host_metrics") {
    assignmentReviewGeneration.current += 1;
    setPending(false);
    resetMessages();
    setEditorBehavior(behavior);
    setEditorName("");
    setEditorDescription("");
    replaceEditorDefinition(defaultDefinition(behavior));
    setDrawer({ kind: "create", behavior });
  }

  function openClone(preset: ConfigurationPresetRecord) {
    assignmentReviewGeneration.current += 1;
    setPending(false);
    resetMessages();
    setEditorName(`${preset.name} copy`);
    setEditorDescription(preset.description ?? "");
    setDrawer({ kind: "clone", preset });
  }

  function openEdit(preset: ConfigurationPresetRecord) {
    assignmentReviewGeneration.current += 1;
    setPending(false);
    resetMessages();
    setEditorBehavior(preset.behavior);
    setEditorName(preset.name);
    setEditorDescription(preset.description ?? "");
    replaceEditorDefinition(asObject(preset.definition));
    setDrawer({ kind: "edit", preset });
  }

  async function reviewOverride(action: "set" | "reset") {
    const generation = assignmentReviewGeneration.current + 1;
    assignmentReviewGeneration.current = generation;
    setPending(true);
    setActionError(null);
    try {
      const selectedDirectIds = [...new Set<string>(directTargetIds)].sort();
      if (parsedAssignmentSelector.error) {
        throw new Error(
          `Fix the target selector before review: ${parsedAssignmentSelector.error}`,
        );
      }
      if (selectedDirectIds.length === 0 && !selectorExpression.trim()) {
        throw new Error(
          "Choose at least one VPS directly or enter a selector that matches a VPS",
        );
      }
      if (action === "set" && !effectiveAssignPresetId) {
        throw new Error("Choose a configuration preset");
      }
      const request: ConfigurationSourceOverrideRequest = {
        action,
        behavior: assignBehavior,
        preset_id: action === "set" ? effectiveAssignPresetId : null,
        selector_expression: selectorExpression.trim(),
        target_client_ids: selectedDirectIds,
      };
      const preview = await onPreviewOverride(request);
      if (preview.target_count === 0) {
        throw new Error("The selected targets did not match any VPS");
      }
      if (
        preview.targets.every(
          (target) =>
            target.before_preset_id === target.after_preset_id &&
            target.before_origin === target.after_origin,
        )
      ) {
        throw new Error(
          "Every reviewed VPS already has this configuration selection; nothing would change.",
        );
      }
      if (assignmentReviewGeneration.current !== generation) {
        return;
      }
      setConfirmation({
        kind: "override",
        preview,
        request: {
          ...request,
          selector_expression: preview.selector_expression,
        },
      });
      setFeedback({
        message: `Reviewed ${preview.target_count} ${preview.target_count === 1 ? "VPS" : "VPSs"}; no change has been applied`,
        tone: "info",
      });
    } catch (reviewError) {
      if (assignmentReviewGeneration.current === generation) {
        setActionError(
          reviewError instanceof Error
            ? reviewError.message
            : "Configuration review failed without diagnostic detail.",
        );
      }
    } finally {
      if (assignmentReviewGeneration.current === generation) {
        setPending(false);
      }
    }
  }

  async function confirmAction() {
    if (!confirmation) return;
    await runPanelAction(setPending, setActionError, async () => {
      if (confirmation.kind === "override") {
        if (!privilegeMaterial) {
          throw new Error("Unlock privilege before saving this selection");
        }
        const { preview, request } = confirmation;
        const target =
          request.action === "set"
            ? `configuration_preset:${request.preset_id}`
            : `configuration_behavior:${request.behavior}`;
        const privilegeAssertion = await buildPrivilegeAssertion({
          intent: canonicalDbPrivilegeIntent({
            action: "configuration_source_override.apply",
            confirmed: true,
            payloadHash: preview.preview_hash,
            resolvedTargets: preview.targets.map((target) => target.client_id),
            selectorExpression: request.selector_expression || null,
            target,
          }),
          privilegeMaterial,
        });
        const response = await onApplyOverride({
          ...request,
          target_client_ids: preview.targets.map((target) => target.client_id),
          preview_hash: preview.preview_hash,
          privilege_assertion: privilegeAssertion,
        });
        setFeedback(overrideApplyFeedback(response));
        setConfirmation(null);
        setDrawer(null);
        return;
      }
      if (confirmation.kind === "preset") {
        const { preset, preview, request } = confirmation;
        let privilegeAssertion = null;
        if (preview.affected_client_count > 0) {
          if (!privilegeMaterial) {
            throw new Error("Unlock privilege before updating this preset");
          }
          privilegeAssertion = await buildPrivilegeAssertion({
            intent: canonicalDbPrivilegeIntent({
              action: "configuration_preset.update",
              confirmed: true,
              payloadHash: preview.preview_hash,
              resolvedTargets: preview.affected_client_ids,
              selectorExpression: null,
              target: `configuration_preset:${preset.id}`,
            }),
            privilegeMaterial,
          });
        }
        const response = await onUpdatePreset(preset.id, {
          description: request.description,
          definition: request.definition,
          preview_hash: preview.preview_hash,
          privilege_assertion: privilegeAssertion,
        });
        setFeedback(presetUpdateFeedback(response));
        setConfirmation(null);
        setDrawer(null);
        return;
      }
      await onDeletePreset(confirmation.preset.id);
      setFeedback({
        message: `Deleted ${confirmation.preset.name}`,
        tone: "success",
      });
      setConfirmation(null);
      setDrawer(null);
    });
  }

  async function submitPresetEditor(event: FormEvent) {
    event.preventDefault();
    const previewGeneration =
      drawer?.kind === "edit"
        ? assignmentReviewGeneration.current + 1
        : assignmentReviewGeneration.current;
    if (drawer?.kind === "edit") {
      assignmentReviewGeneration.current = previewGeneration;
    }
    const setSubmitPending = (value: boolean) => {
      if (
        drawer?.kind !== "edit" ||
        assignmentReviewGeneration.current === previewGeneration
      ) {
        setPending(value);
      }
    };
    const setSubmitError = (value: string | null) => {
      if (
        drawer?.kind !== "edit" ||
        assignmentReviewGeneration.current === previewGeneration
      ) {
        setActionError(value);
      }
    };
    await runPanelAction(setSubmitPending, setSubmitError, async () => {
      if (!editorName.trim()) {
        throw new Error("Preset name is required");
      }
      let candidateDefinition = editorDefinition;
      if (drawer?.kind !== "clone") {
        const materialized = materializeDefinition(
          editorBehavior,
          editorDefinition,
          editorTextDrafts,
        );
        if (materialized.definition === null) {
          throw new Error(materialized.error);
        }
        candidateDefinition = materialized.definition;
        const definitionError = validatePresetDefinition(
          editorBehavior,
          candidateDefinition,
        );
        if (definitionError) {
          throw new Error(definitionError);
        }
      }
      if (drawer?.kind === "create") {
        await onCreatePreset({
          behavior: editorBehavior,
          name: editorName.trim(),
          description: editorDescription.trim() || null,
          definition: candidateDefinition,
        });
        setFeedback({
          message: `Created ${editorName.trim()}`,
          tone: "success",
        });
        setDrawer(null);
        return;
      }
      if (drawer?.kind === "clone") {
        await onClonePreset(drawer.preset.id, {
          name: editorName.trim(),
          description: editorDescription.trim() || null,
        });
        setFeedback({
          message: `Cloned ${drawer.preset.name} as ${editorName.trim()}`,
          tone: "success",
        });
        setDrawer(null);
        return;
      }
      if (drawer?.kind === "edit") {
        const candidateDescription = editorDescription.trim() || null;
        const preview = await onPreviewPreset(drawer.preset.id, {
          description: candidateDescription,
          definition: candidateDefinition,
        });
        if (assignmentReviewGeneration.current !== previewGeneration) {
          return;
        }
        if (preview.changed_keys.length === 0) {
          setFeedback({
            message: "No changes to review",
            tone: "info",
          });
          return;
        }
        setConfirmation({
          kind: "preset",
          preset: drawer.preset,
          preview,
          request: {
            description: candidateDescription,
            definition: candidateDefinition,
          },
        });
        setFeedback({
          message: `Reviewed ${preview.changed_keys.length} changed ${preview.changed_keys.length === 1 ? "field" : "fields"}; no change has been applied`,
          tone: "info",
        });
      }
    });
  }

  async function inspectEffectiveConfig(clientId: string) {
    const generation = inspectionGeneration.current + 1;
    inspectionGeneration.current = generation;
    const setInspectionPending = (value: boolean) => {
      if (inspectionGeneration.current === generation) {
        setPending(value);
      }
    };
    const setInspectionError = (value: string | null) => {
      if (inspectionGeneration.current === generation) {
        setActionError(value);
      }
    };
    await runPanelAction(setInspectionPending, setInspectionError, async () => {
      const response = await onLoadEffectiveConfig(clientId);
      if (inspectionGeneration.current === generation) {
        setRenderedConfig(response);
      }
    });
  }

  const sourceColumns = useMemo<
    ConsoleDataGridColumn<ConfigurationSourceView>[]
  >(
    () => [
      {
        id: "vps",
        header: "VPS",
        cell: (source) => {
          const agent = agentById.get(source.client_id);
          return (
            <span className="historyPrimary">
              <strong>
                {agent
                  ? formatVpsName(agent, vpsNameDisplayMode)
                  : source.client_id}
              </strong>
              <small>{agent?.status ?? "not in current fleet"}</small>
            </span>
          );
        },
        searchValue: (source) => {
          const agent = agentById.get(source.client_id);
          return `${agent?.display_name ?? ""} ${source.client_id} ${agent?.status ?? ""}`;
        },
        sortValue: (source) =>
          agentById.get(source.client_id)?.display_name ?? source.client_id,
      },
      {
        id: "behavior",
        header: "Behavior",
        cell: (source) => behaviorLabel(source.behavior),
        searchValue: (source) =>
          `${behaviorLabel(source.behavior)} ${source.behavior}`,
        sortValue: (source) => behaviorLabel(source.behavior),
      },
      {
        id: "preset",
        header: "Effective preset",
        cell: (source) => (
          <span className="historyPrimary">
            <strong>{source.effective_preset_name}</strong>
            <small>
              {source.selection_origin === "explicit_override"
                ? "Explicit override"
                : "Inherited system default"}
            </small>
          </span>
        ),
        searchValue: (source) =>
          `${source.effective_preset_name} ${source.selection_origin}`,
        sortValue: (source) => source.effective_preset_name,
      },
      {
        id: "sync",
        header: "Runtime sync",
        cell: (source) => (
          <span
            className={`status ${syncTone(source.runtime_sync.state)}`}
            title={source.runtime_sync.reason}
          >
            {tokenLabel(source.runtime_sync.state)}
          </span>
        ),
        searchValue: (source) =>
          `${source.runtime_sync.state} ${source.runtime_sync.reason}`,
        sortValue: (source) => source.runtime_sync.state,
      },
      {
        id: "readiness",
        header: "Readiness",
        cell: (source) => (
          <span
            className={`status ${readinessTone(source.readiness.state)}`}
            title={source.readiness.reason}
          >
            {tokenLabel(source.readiness.state)}
          </span>
        ),
        searchValue: (source) =>
          `${source.readiness.state} ${source.readiness.reason}`,
        sortValue: (source) => source.readiness.state,
      },
    ],
    [agentById, vpsNameDisplayMode],
  );

  const sourceActions: ConsoleDataGridAction<ConfigurationSourceView>[] = [
    {
      label: "Change",
      onSelect: (rows) => openAssignment(rows[0]),
      disabled: (rows) => rows.length !== 1,
    },
    {
      label: "Reset to system default",
      onSelect: (rows) => {
        openAssignment(rows[0]);
        void reviewOverrideForRow(rows[0], "reset");
      },
      disabled: (rows) =>
        rows.length !== 1 || rows[0].selection_origin !== "explicit_override",
    },
  ];

  async function reviewOverrideForRow(
    source: ConfigurationSourceView,
    action: "reset",
  ) {
    setAssignBehavior(source.behavior);
    setAssignPresetId(source.effective_preset_id);
    setDirectTargetIds([source.client_id]);
    setSelectorExpression("");
    const generation = assignmentReviewGeneration.current + 1;
    assignmentReviewGeneration.current = generation;
    setPending(true);
    setActionError(null);
    try {
      const request: ConfigurationSourceOverrideRequest = {
        action,
        behavior: source.behavior,
        preset_id: null,
        selector_expression: "",
        target_client_ids: [source.client_id],
      };
      const preview = await onPreviewOverride(request);
      if (preview.target_count === 0) {
        throw new Error("The selected VPS no longer exists");
      }
      if (
        preview.targets.every(
          (target) =>
            target.before_preset_id === target.after_preset_id &&
            target.before_origin === target.after_origin,
        )
      ) {
        throw new Error(
          "This VPS already inherits the system default; nothing would change.",
        );
      }
      if (assignmentReviewGeneration.current !== generation) {
        return;
      }
      setConfirmation({
        kind: "override",
        request: {
          ...request,
          selector_expression: preview.selector_expression,
        },
        preview,
      });
      setFeedback({
        message: "Reset reviewed; no change has been applied",
        tone: "info",
      });
    } catch (reviewError) {
      if (assignmentReviewGeneration.current === generation) {
        setActionError(
          reviewError instanceof Error
            ? reviewError.message
            : "Configuration reset review failed without diagnostic detail.",
        );
      }
    } finally {
      if (assignmentReviewGeneration.current === generation) {
        setPending(false);
      }
    }
  }

  const presetColumns = useMemo<
    ConsoleDataGridColumn<ConfigurationPresetRecord>[]
  >(
    () => [
      {
        id: "preset",
        header: "Preset",
        cell: (preset) => (
          <span className="historyPrimary">
            <strong>{preset.name}</strong>
            <small>{preset.description ?? "No description"}</small>
          </span>
        ),
        searchValue: (preset) => `${preset.name} ${preset.description ?? ""}`,
        sortValue: (preset) => preset.name,
      },
      {
        id: "behavior",
        header: "Behavior",
        cell: (preset) => behaviorLabel(preset.behavior),
        searchValue: (preset) => preset.behavior,
        sortValue: (preset) => behaviorLabel(preset.behavior),
      },
      {
        id: "kind",
        header: "Type",
        cell: (preset) => (
          <span
            className={`status ${preset.is_default ? "info" : preset.kind === "custom" ? "ok" : "neutral"}`}
          >
            {preset.is_default
              ? "System default"
              : preset.kind === "system"
                ? "System option"
                : "Custom"}
          </span>
        ),
        searchValue: (preset) =>
          `${preset.kind} ${preset.is_default ? "system default" : ""}`,
        sortValue: (preset) =>
          `${preset.is_default ? "0" : "1"}:${preset.kind}`,
      },
      {
        id: "use",
        header: "VPS use",
        cell: (preset) => (
          <span className="historyPrimary">
            <strong>{preset.effective_vps_count} effective</strong>
            <small>{preset.override_vps_count} explicit</small>
          </span>
        ),
        searchValue: (preset) =>
          `${preset.effective_vps_count} ${preset.override_vps_count}`,
        sortValue: (preset) => preset.effective_vps_count,
      },
      {
        id: "updated",
        header: "Updated",
        cell: (preset) => formatTime(preset.updated_at),
        searchValue: (preset) => formatTime(preset.updated_at),
        sortValue: (preset) => preset.updated_at,
      },
    ],
    [],
  );

  const presetActions: ConsoleDataGridAction<ConfigurationPresetRecord>[] = [
    {
      label: "Use preset",
      onSelect: (rows) => {
        openAssignment(null);
        setAssignBehavior(rows[0].behavior);
        setAssignPresetId(rows[0].id);
      },
      description: (rows) =>
        rows[0]?.is_default
          ? "System defaults are inherited; use Inherit system default instead."
          : "Choose VPS targets that should explicitly use this preset.",
      disabled: (rows) => rows.length !== 1 || rows[0].is_default,
    },
    {
      label: "Inherit system default",
      icon: <RotateCcw size={14} />,
      onSelect: (rows) => {
        openAssignment(null);
        setAssignBehavior(rows[0].behavior);
        setAssignPresetId("");
      },
      description: () =>
        "Remove explicit overrides so the selected VPSs inherit this behavior's system default.",
      disabled: (rows) => rows.length !== 1 || !rows[0].is_default,
    },
    {
      label: "Clone to customize",
      icon: <Copy size={14} />,
      onSelect: (rows) => openClone(rows[0]),
      disabled: (rows) => rows.length !== 1,
    },
    {
      label: "Edit",
      icon: <Settings2 size={14} />,
      onSelect: (rows) => openEdit(rows[0]),
      disabled: (rows) => rows.length !== 1 || rows[0].kind !== "custom",
    },
    {
      label: "Delete",
      icon: <Trash2 size={14} />,
      tone: "danger",
      onSelect: (rows) => {
        resetMessages();
        setConfirmation({ kind: "delete", preset: rows[0] });
      },
      disabled: (rows) =>
        rows.length !== 1 ||
        rows[0].kind !== "custom" ||
        rows[0].override_vps_count > 0,
    },
  ];

  const confirmationNeedsPrivilege =
    confirmation?.kind === "override" ||
    (confirmation?.kind === "preset" &&
      confirmation.preview.affected_client_count > 0);
  const confirmationPreviewHash =
    confirmation?.kind === "override" || confirmation?.kind === "preset"
      ? confirmation.preview.preview_hash
      : null;
  const editorDefinitionPreview = materializeDefinition(
    editorBehavior,
    editorDefinition,
    editorTextDrafts,
  );

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel configurationSourcesPanel">
        <div className="sectionHeader">
          <div>
            <h2>Configuration sources</h2>
            <span>
              {sourceVpsCount} VPS{sourceVpsCount === 1 ? "" : "s"} ·{" "}
              {loading || (!error && presets.length === 0)
                ? "Checking evidence"
                : error
                  ? "Evidence unavailable"
                  : configurationSourceSummary(
                      sourceAttentionCount,
                      sourceUnconfiguredCount,
                    )}
            </span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction"
              disabled={loading || pending}
              onClick={() =>
                void runPanelAction(setPending, setActionError, onRefresh)
              }
              type="button"
            >
              <RefreshCw size={15} />
              Refresh
            </button>
          </div>
        </div>

        <div
          aria-label="Configuration source views"
          className="consoleRegistryTabs"
          onKeyDown={handleTabListKeyDown}
          role="tablist"
        >
          <button
            aria-controls="configuration-sources-tabpanel"
            aria-selected={view === "effective"}
            className={view === "effective" ? "active" : ""}
            id={tabId("configuration-sources", "effective")}
            onClick={() => setView("effective")}
            role="tab"
            tabIndex={view === "effective" ? 0 : -1}
            type="button"
          >
            Effective configuration
          </button>
          <button
            aria-controls="configuration-sources-tabpanel"
            aria-selected={view === "presets"}
            className={view === "presets" ? "active" : ""}
            id={tabId("configuration-sources", "presets")}
            onClick={() => setView("presets")}
            role="tab"
            tabIndex={view === "presets" ? 0 : -1}
            type="button"
          >
            Configuration presets
          </button>
        </div>

        <ActionFeedback
          className="localActionFeedback"
          message={error}
          tone="danger"
        />
        {!drawer && !confirmation ? (
          <ActionFeedback
            className="localActionFeedback"
            message={actionError}
            tone="danger"
          />
        ) : null}
        <ActionFeedback
          className="localActionFeedback"
          message={feedback?.message}
          tone={feedback?.tone}
        />

        <div
          aria-labelledby={tabId("configuration-sources", view)}
          id="configuration-sources-tabpanel"
          role="tabpanel"
        >
          {view === "effective" ? (
            <ConsoleDataGrid
              actions={sourceActions}
              columns={sourceColumns}
              defaultPageSize={12}
              empty={
                <div className="emptyState">
                  <Settings2 size={22} />
                  <strong>No effective configuration</strong>
                  <span>
                    No source rows were returned. Refresh before assuming a
                    default or override.
                  </span>
                </div>
              }
              getRowId={(source) => `${source.client_id}:${source.behavior}`}
              itemLabel="configuration sources"
              onOpenRow={openAssignment}
              openRowLabel="Change"
              showMobileOpenRowAction={false}
              renderExpandedRow={(source) => (
                <div className="consoleInlineDetailGrid">
                  <span>Selection</span>
                  <strong>
                    {source.selection_origin === "explicit_override"
                      ? `Explicit override${source.override_updated_at ? ` · ${formatTime(source.override_updated_at)}` : ""}`
                      : "Inherited system default"}
                  </strong>
                  <span>Runtime sync</span>
                  <strong>
                    {tokenLabel(source.runtime_sync.state)} —{" "}
                    {source.runtime_sync.reason}
                  </strong>
                  <span>Readiness</span>
                  <strong>
                    {tokenLabel(source.readiness.state)} —{" "}
                    {source.readiness.reason}
                  </strong>
                  <span>Evidence</span>
                  <strong>
                    <pre>
                      {JSON.stringify(source.readiness.evidence, null, 2)}
                    </pre>
                  </strong>
                </div>
              )}
              rows={sources}
              searchPlaceholder="Search VPS, behavior, preset, or state"
              storageKey="vpsman.configurationSources.effective"
              title="Effective configuration"
              toolbarActions={
                <button
                  className="primaryAction compactAction"
                  onClick={() => openAssignment()}
                  type="button"
                >
                  <Plus size={15} />
                  Change configuration
                </button>
              }
            />
          ) : (
            <ConsoleDataGrid
              actions={presetActions}
              columns={presetColumns}
              defaultPageSize={12}
              empty={
                <div className="emptyState">
                  <FileJson size={22} />
                  <strong>No configuration presets</strong>
                  <span>
                    System defaults could not be loaded. Refresh before creating
                    an alternative.
                  </span>
                </div>
              }
              getRowId={(preset) => preset.id}
              itemLabel="configuration presets"
              onOpenRow={(preset) =>
                preset.kind === "custom" ? openEdit(preset) : openClone(preset)
              }
              openRowLabel="Open"
              showMobileOpenRowAction={false}
              renderExpandedRow={(preset) => {
                const effectiveSources = sources.filter(
                  (source) => source.effective_preset_id === preset.id,
                );
                const explicitSources = effectiveSources.filter(
                  (source) => source.selection_origin === "explicit_override",
                );
                return (
                  <div className="compactForm">
                    <div className="consoleInlineDetailGrid">
                      <span>Effective on</span>
                      <strong>
                        {sourceVpsList(
                          effectiveSources,
                          agentById,
                          vpsNameDisplayMode,
                        )}
                      </strong>
                      <span>Explicitly selected on</span>
                      <strong>
                        {sourceVpsList(
                          explicitSources,
                          agentById,
                          vpsNameDisplayMode,
                        )}
                      </strong>
                    </div>
                    <details>
                      <summary>Advanced definition</summary>
                      <pre>{JSON.stringify(preset.definition, null, 2)}</pre>
                    </details>
                  </div>
                );
              }}
              rows={presets}
              searchPlaceholder="Search preset or behavior"
              storageKey="vpsman.configurationSources.presets"
              title="Configuration presets"
              toolbarActions={
                <button
                  className="primaryAction compactAction"
                  onClick={() => openCreate()}
                  type="button"
                >
                  <Plus size={15} />
                  New preset
                </button>
              }
            />
          )}
        </div>
      </div>

      <ConfirmationPrompt
        confirmDisabled={confirmationNeedsPrivilege && !privilegeMaterial}
        confirmLabel={
          confirmation?.kind === "override"
            ? confirmation.request.action === "reset"
              ? "Reset to system default"
              : "Save selection"
            : confirmation?.kind === "preset"
              ? "Update preset"
              : "Delete preset"
        }
        detail={confirmationDetail(confirmation)}
        error={actionError}
        items={confirmationItems(
          confirmation,
          agentById,
          vpsNameDisplayMode,
        )}
        onCancel={() => {
          setActionError(null);
          setConfirmation(null);
        }}
        onConfirm={() => void confirmAction()}
        open={confirmation !== null}
        pending={pending}
        title={
          confirmation?.kind === "override"
            ? "Review effective configuration change"
            : confirmation?.kind === "preset"
              ? "Review preset update"
              : "Delete custom preset"
        }
        tone={confirmation?.kind === "delete" ? "danger" : "normal"}
      >
        {confirmationNeedsPrivilege && !privilegeMaterial ? (
          <PrivilegeVaultBox
            labelPrefix="Configuration sources"
            lastPayloadHash={confirmationPreviewHash}
            onPrivilegeMaterialChange={setPrivilegeMaterial}
            privilegeMaterial={privilegeMaterial}
            usePrivilegeLabel="Unlock configuration apply"
          />
        ) : null}
      </ConfirmationPrompt>

      <ConsoleActionDrawer
        description={drawerDescription(drawer)}
        onClose={() => {
          assignmentReviewGeneration.current += 1;
          setPending(false);
          setActionError(null);
          clearEffectiveConfigInspection();
          setDrawer(null);
        }}
        open={drawer !== null}
        title={drawerTitle(drawer)}
      >
        {drawer?.kind === "assign" ? (
          <form
            className="compactForm structuredDefinitionForm"
            onSubmit={(event) => {
              event.preventDefault();
              void reviewOverride(effectiveAssignPresetId ? "set" : "reset");
            }}
          >
            <fieldset
              className="configurationDrawerFields"
              disabled={pending}
            >
              <strong>Choose behavior and preset</strong>
            <div className="formRow">
              <label>
                <span>Behavior</span>
                <select
                  aria-label="Configuration behavior"
                  onChange={(event) => {
                    const behavior = event.target
                      .value as ConfigurationBehavior;
                    setAssignBehavior(behavior);
                    setAssignPresetId("");
                    clearEffectiveConfigInspection();
                  }}
                  value={assignBehavior}
                >
                  {BEHAVIORS.map((behavior) => (
                    <option key={behavior} value={behavior}>
                      {behaviorLabel(behavior)}
                    </option>
                  ))}
                </select>
                {assignablePresets.length === 0 ? (
                  <span className="formHint">
                    No alternative preset exists for this behavior. Keep the
                    system default, or{" "}
                    <button
                      className="linkButton"
                      onClick={() => openCreate(assignBehavior)}
                      type="button"
                    >
                      create a preset
                    </button>
                    .
                  </span>
                ) : null}
              </label>
              <label>
                <span>Preset</span>
                <select
                  aria-label="Configuration preset"
                  onChange={(event) => {
                    setAssignPresetId(event.target.value);
                    clearEffectiveConfigInspection();
                  }}
                  value={effectiveAssignPresetId}
                >
                  <option value="">Inherit system default</option>
                  {assignablePresets.map((preset) => (
                    <option key={preset.id} value={preset.id}>
                      {preset.name} ·{" "}
                      {preset.kind === "system" ? "System option" : "Custom"}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            <div className="targetSelector">
              <div className="targetSelectorHeader">
                <strong>Targets</strong>
                <span>
                  {parsedAssignmentSelector.error
                    ? "Fix the selector before review"
                    : `${assignmentTargetIds.length} ${assignmentTargetIds.length === 1 ? "VPS" : "VPSs"} selected locally`}
                </span>
              </div>
              <VpsCombobox
                agents={agents}
                ariaLabel="Add configuration target VPS"
                excludeIds={directTargetIds}
                onChange={(clientId) => {
                  if (!clientId) {
                    setTargetCandidate("");
                    return;
                  }
                  setDirectTargetIds((current) => [
                    ...new Set([...current, clientId]),
                  ]);
                  setTargetCandidate("");
                  clearEffectiveConfigInspection();
                }}
                placeholder="Add an individual VPS"
                value={targetCandidate}
              />
              <details>
                <summary>
                  {selectorExpression.trim()
                    ? `Edit target selector · ${selectorExpression.trim()}`
                    : "Add targets by selector"}
                </summary>
                <SearchExpressionInput
                  agents={agents}
                  ariaLabel="Configuration target selector"
                  onChange={(value) => {
                    setSelectorExpression(value);
                    clearEffectiveConfigInspection();
                  }}
                  placeholder="For example tag:edge"
                  showMatchCount
                  value={selectorExpression}
                  verification={
                    parsedAssignmentSelector.error
                      ? "invalid"
                      : selectorExpression.trim()
                        ? "valid"
                        : "neutral"
                  }
                />
              </details>
              <AssignmentTargetPreview
                agentById={agentById}
                directTargetIds={directTargetIds}
                onRemoveDirectTarget={(clientId) => {
                  clearEffectiveConfigInspection();
                  setDirectTargetIds((current) =>
                    current.filter((id) => id !== clientId),
                  );
                }}
                targetIds={assignmentTargetIds}
                vpsNameDisplayMode={vpsNameDisplayMode}
              />
              <span className="formHint">
                Direct choices and selector matches form one list. Review
                verifies it on the server and freezes the exact VPSs; later tag
                changes do not alter this operation.
              </span>
            </div>

            {!confirmation ? (
              <ActionFeedback message={actionError} tone="danger" />
            ) : null}
            <div className="formRow">
              <button
                className="primaryAction"
                disabled={Boolean(primaryAssignmentReviewBlockedReason)}
                title={primaryAssignmentReviewBlockedReason ?? "Review the exact VPS list and configuration change."}
                type="submit"
              >
                {effectiveAssignPresetId
                  ? "Review change"
                  : "Review reset to system default"}
              </button>
              {effectiveAssignPresetId ? (
                <button
                  className="secondaryAction"
                  disabled={Boolean(resetAssignmentReviewBlockedReason)}
                  onClick={() => void reviewOverride("reset")}
                  title={resetAssignmentReviewBlockedReason ?? "Review resetting the exact VPS list to the system default."}
                  type="button"
                >
                  <RotateCcw size={15} />
                  Reset to system default
                </button>
              ) : null}
            </div>

            {directTargetIds.length === 1 && !selectorExpression.trim() ? (
              <button
                className="secondaryAction compactAction"
                disabled={pending}
                onClick={() => void inspectEffectiveConfig(directTargetIds[0])}
                type="button"
              >
                Inspect current effective config
              </button>
            ) : null}
            {renderedConfig ? (
              <details className="configPreview" open>
                <summary>Current effective config (TOML)</summary>
                <div className="previewMeta">
                  <span>
                    {reviewedVpsLabel(
                      renderedConfig.client_id,
                      agentById,
                      vpsNameDisplayMode,
                    )}
                  </span>
                  <span>{renderedConfig.sources.length} effective sources</span>
                  <span>{formatTime(renderedConfig.generated_at)}</span>
                </div>
                <span className="formHint">
                  Current server-rendered configuration. A preset choice above
                  is only a candidate until you review and apply it.
                </span>
                <textarea
                  aria-label="Effective agent config TOML"
                  readOnly
                  value={renderedConfig.toml}
                />
              </details>
            ) : null}
            </fieldset>
          </form>
        ) : null}

        {drawer?.kind === "create" ||
        drawer?.kind === "edit" ||
        drawer?.kind === "clone" ? (
          <form
            className="compactForm structuredDefinitionForm"
            onSubmit={submitPresetEditor}
          >
            <fieldset
              className="configurationDrawerFields"
              disabled={pending}
            >
            {drawer.kind === "create" ? (
              <span className="formHint">
                Create a reusable alternative for one behavior. VPSs keep
                inheriting the system default until you explicitly assign this
                preset.
              </span>
            ) : null}
            {drawer.kind === "clone" ? (
              <>
                <label>
                  <span>Name</span>
                  <input
                    aria-label="Cloned preset name"
                    onChange={(event) => setEditorName(event.target.value)}
                    value={editorName}
                  />
                </label>
                <label>
                  <span>Description</span>
                  <input
                    aria-label="Cloned preset description"
                    onChange={(event) =>
                      setEditorDescription(event.target.value)
                    }
                    value={editorDescription}
                  />
                </label>
                <details>
                  <summary>Definition copied from selected preset</summary>
                  <pre>{JSON.stringify(drawer.preset.definition, null, 2)}</pre>
                </details>
              </>
            ) : (
              <>
                <div className="formRow">
                  <label>
                    <span>Behavior</span>
                    <select
                      aria-label="Preset behavior"
                      disabled={drawer.kind === "edit"}
                      onChange={(event) => {
                        const behavior = event.target
                          .value as ConfigurationBehavior;
                        setEditorBehavior(behavior);
                        replaceEditorDefinition(defaultDefinition(behavior));
                      }}
                      value={editorBehavior}
                    >
                      {BEHAVIORS.map((behavior) => (
                        <option key={behavior} value={behavior}>
                          {behaviorLabel(behavior)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    <span>Name</span>
                    <input
                      aria-label="Preset name"
                      disabled={drawer.kind === "edit"}
                      onChange={(event) => setEditorName(event.target.value)}
                      value={editorName}
                    />
                  </label>
                </div>
                <label>
                  <span>Description</span>
                  <input
                    aria-label="Preset description"
                    onChange={(event) =>
                      setEditorDescription(event.target.value)
                    }
                    value={editorDescription}
                  />
                </label>
                <PresetDefinitionEditor
                  behavior={editorBehavior}
                  definition={editorDefinition}
                  onChange={setEditorDefinition}
                  onReplace={replaceEditorDefinition}
                  onTextDraftChange={(key, value) =>
                    setEditorTextDrafts((current) => ({
                      ...current,
                      [key]: value,
                    }))
                  }
                  textDrafts={editorTextDrafts}
                />
                <details>
                  <summary>Advanced definition preview</summary>
                  <pre>
                    {editorDefinitionPreview.error
                      ? editorDefinitionPreview.error
                      : JSON.stringify(
                          editorDefinitionPreview.definition,
                          null,
                          2,
                        )}
                  </pre>
                </details>
              </>
            )}
            {!confirmation ? (
              <ActionFeedback message={actionError} tone="danger" />
            ) : null}
            <button
              className="primaryAction"
              disabled={pending || !editorName.trim()}
              type="submit"
            >
              {drawer.kind === "create"
                ? "Create preset"
                : drawer.kind === "clone"
                  ? "Clone preset"
                  : "Review preset update"}
            </button>
            </fieldset>
          </form>
        ) : null}
      </ConsoleActionDrawer>
    </section>
  );
}

function AssignmentTargetPreview({
  agentById,
  directTargetIds,
  onRemoveDirectTarget,
  targetIds,
  vpsNameDisplayMode,
}: {
  agentById: Map<string, AgentView>;
  directTargetIds: string[];
  onRemoveDirectTarget: (clientId: string) => void;
  targetIds: string[];
  vpsNameDisplayMode: VpsNameDisplayMode;
}) {
  const [expanded, setExpanded] = useState(false);
  const directTargets = new Set(directTargetIds);
  const visibleTargetIds = expanded ? targetIds : targetIds.slice(0, 8);
  const remaining = targetIds.length - visibleTargetIds.length;

  return (
    <div aria-label="Configuration target preview" className="targetChipList">
      {targetIds.length === 0 ? (
        <span className="formHint">No VPS selected yet.</span>
      ) : (
        visibleTargetIds.map((clientId) => {
          const agent = agentById.get(clientId);
          const label = agent
            ? formatVpsName(agent, vpsNameDisplayMode)
            : clientId;
          return directTargets.has(clientId) ? (
            <button
              aria-label={`Remove ${label}`}
              className="targetChip"
              key={clientId}
              onClick={() => onRemoveDirectTarget(clientId)}
              title={`${clientId} · selected directly`}
              type="button"
            >
              <span>{label}</span>
              <X size={13} />
            </button>
          ) : (
            <span className="targetChip" key={clientId} title={clientId}>
              {label}
            </span>
          );
        })
      )}
      {remaining > 0 ? (
        <button
          className="targetChip mutedChip showMoreChip"
          onClick={() => setExpanded(true)}
          type="button"
        >
          Show {remaining} more
        </button>
      ) : expanded && targetIds.length > 8 ? (
        <button
          className="targetChip mutedChip showMoreChip"
          onClick={() => setExpanded(false)}
          type="button"
        >
          Show fewer
        </button>
      ) : null}
    </div>
  );
}

function PresetDefinitionEditor({
  behavior,
  definition,
  onChange,
  onReplace,
  onTextDraftChange,
  textDrafts,
}: {
  behavior: ConfigurationBehavior;
  definition: Record<string, JsonValue>;
  onChange: (definition: Record<string, JsonValue>) => void;
  onReplace: (definition: Record<string, JsonValue>) => void;
  onTextDraftChange: (key: string, value: string) => void;
  textDrafts: EditorTextDrafts;
}) {
  const setField = (key: string, value: JsonValue) =>
    onChange({ ...definition, [key]: value });
  const changeSource = (source: string) =>
    onReplace(defaultDefinitionForSource(behavior, source));

  if (behavior === "ospf_update_command") {
    return (
      <div className="compactForm">
        <strong>OSPF updater command</strong>
        <label>
          <span>Contract version</span>
          <input
            aria-label="OSPF updater contract version"
            readOnly
            type="number"
            value={1}
          />
        </label>
        <BoundedCommandEditor
          command={asObject(definition.status_command)}
          commandKey="status_command"
          label="Read current OSPF cost"
          onChange={(command) => setField("status_command", command)}
          onTextDraftChange={onTextDraftChange}
          textDrafts={textDrafts}
        />
        <BoundedCommandEditor
          command={asObject(definition.update_command)}
          commandKey="update_command"
          label="Update OSPF cost"
          onChange={(command) => setField("update_command", command)}
          onTextDraftChange={onTextDraftChange}
          textDrafts={textDrafts}
        />
      </div>
    );
  }

  if (behavior === "command_execution") {
    return (
      <div className="compactForm">
        <strong>Command execution</strong>
        <ArgvField
          label="Shell command arguments"
          onChange={(value) => onTextDraftChange("shell_script_argv", value)}
          value={textDrafts.shell_script_argv ?? ""}
        />
        <label>
          <span>Working directory</span>
          <input
            aria-label="Command working directory"
            onChange={(event) =>
              setField("working_directory", event.target.value.trim() || null)
            }
            placeholder="Use agent working directory"
            value={asString(definition.working_directory)}
          />
        </label>
        <div className="formRow">
          <label>
            <span>Environment</span>
            <select
              aria-label="Command environment policy"
              onChange={(event) =>
                setField("environment_policy", event.target.value)
              }
              value={asString(definition.environment_policy)}
            >
              <option value="inherit">Inherit</option>
              <option value="minimal_path">Minimal PATH</option>
              <option value="clean">Clean</option>
            </select>
          </label>
          <label>
            <span>Terminal</span>
            <select
              aria-label="Command terminal policy"
              onChange={(event) => setField("pty_policy", event.target.value)}
              value={asString(definition.pty_policy)}
            >
              <option value="native_pty">Native PTY</option>
              <option value="disabled">Disabled</option>
            </select>
          </label>
          <label>
            <span>Process cleanup</span>
            <select
              aria-label="Command process cleanup"
              onChange={(event) =>
                setField("process_cleanup", event.target.value)
              }
              value={asString(definition.process_cleanup)}
            >
              <option value="process_group">Process group</option>
              <option value="direct_child">Direct child</option>
            </select>
          </label>
        </div>
        <StringListField
          label="Environment names to keep"
          onChange={(value) => onTextDraftChange("environment_keep", value)}
          value={textDrafts.environment_keep ?? ""}
        />
        <label>
          <span>Environment values (KEY=value, one per line)</span>
          <textarea
            aria-label="Command environment values"
            onChange={(event) =>
              onTextDraftChange("environment_set", event.target.value)
            }
            value={textDrafts.environment_set ?? ""}
          />
        </label>
      </div>
    );
  }

  const source = asString(definition.source);
  return (
    <div className="compactForm">
      <strong>{behaviorLabel(behavior)} source</strong>
      <label>
        <span>Source type</span>
        <select
          aria-label={`${behaviorLabel(behavior)} source type`}
          onChange={(event) => changeSource(event.target.value)}
          value={source}
        >
          {sourceOptions(behavior).map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </label>

      {behavior === "host_metrics" &&
      ["linux_procfs", "linux_procfs_and_custom_command"].includes(source) ? (
        <div className="formRow">
          <PathField
            field="proc_root"
            label="Proc filesystem"
            onChange={setField}
            value={definition.proc_root}
          />
          <PathField
            field="sys_class_net_dir"
            label="Network devices"
            onChange={setField}
            value={definition.sys_class_net_dir}
          />
          <PathField
            field="hostname_file"
            label="Hostname file"
            onChange={setField}
            value={definition.hostname_file}
          />
          <PathField
            field="os_release_file"
            label="OS release file"
            onChange={setField}
            value={definition.os_release_file}
          />
        </div>
      ) : null}

      {behavior === "process_inventory" && source === "linux_procfs" ? (
        <PathField
          field="proc_root"
          label="Proc filesystem"
          onChange={setField}
          value={definition.proc_root}
        />
      ) : null}

      {behavior === "tunnel_traffic" && source === "vnstat" ? (
        <ArgvField
          label="vnStat arguments"
          onChange={(value) => onTextDraftChange("vnstat_argv", value)}
          value={textDrafts.vnstat_argv ?? ""}
        />
      ) : null}

      {behavior === "latency_probe" && source === "configured_ping_argv" ? (
        <ArgvField
          label="Ping arguments"
          onChange={(value) => onTextDraftChange("probe_ping_argv", value)}
          value={textDrafts.probe_ping_argv ?? ""}
        />
      ) : null}

      {commandFieldFor(behavior, source) ? (
        <BoundedCommandEditor
          command={asObject(definition[commandFieldFor(behavior, source)!])}
          commandKey={commandFieldFor(behavior, source)!}
          label={commandLabel(behavior)}
          onChange={(command) =>
            setField(commandFieldFor(behavior, source)!, command)
          }
          onTextDraftChange={onTextDraftChange}
          textDrafts={textDrafts}
        />
      ) : null}
    </div>
  );
}

function PathField({
  field,
  label,
  onChange,
  value,
}: {
  field: string;
  label: string;
  onChange: (field: string, value: JsonValue) => void;
  value: JsonValue | undefined;
}) {
  return (
    <label>
      <span>{label}</span>
      <input
        aria-label={label}
        onChange={(event) => onChange(field, event.target.value)}
        placeholder="/absolute/path"
        value={asString(value)}
      />
    </label>
  );
}

function ArgvField({
  label,
  onChange,
  value,
}: {
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <label>
      <span>{label} (one argument per line)</span>
      <textarea
        aria-label={label}
        onChange={(event) => onChange(event.target.value)}
        placeholder={"/absolute/path/to/executable\n--argument"}
        value={value}
      />
    </label>
  );
}

function StringListField({
  label,
  onChange,
  value,
}: {
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <label>
      <span>{label} (one per line)</span>
      <textarea
        aria-label={label}
        onChange={(event) => onChange(event.target.value)}
        value={value}
      />
    </label>
  );
}

function BoundedCommandEditor({
  command,
  commandKey,
  label,
  onChange,
  onTextDraftChange,
  textDrafts,
}: {
  command: Record<string, JsonValue>;
  commandKey: string;
  label: string;
  onChange: (command: Record<string, JsonValue>) => void;
  onTextDraftChange: (key: string, value: string) => void;
  textDrafts: EditorTextDrafts;
}) {
  return (
    <div className="compactForm">
      <strong>{label}</strong>
      <ArgvField
        label={`${label} arguments`}
        onChange={(value) => onTextDraftChange(`${commandKey}.argv`, value)}
        value={textDrafts[`${commandKey}.argv`] ?? ""}
      />
      <div className="formRow">
        <label>
          <span>Timeout seconds</span>
          <input
            aria-label={`${label} timeout seconds`}
            max={120}
            min={1}
            onChange={(event) =>
              onChange({
                ...command,
                max_timeout_secs: Number(event.target.value),
              })
            }
            type="number"
            value={asNumber(command.max_timeout_secs, 5)}
          />
        </label>
        <label>
          <span>Maximum output bytes</span>
          <input
            aria-label={`${label} maximum output bytes`}
            max={65536}
            min={1024}
            onChange={(event) =>
              onChange({
                ...command,
                max_output_bytes: Number(event.target.value),
              })
            }
            step={1024}
            type="number"
            value={asNumber(command.max_output_bytes, 16384)}
          />
        </label>
      </div>
    </div>
  );
}

function defaultDefinition(
  behavior: ConfigurationBehavior,
): Record<string, JsonValue> {
  switch (behavior) {
    case "host_metrics":
      return {
        source: "linux_procfs",
        proc_root: "/proc",
        sys_class_net_dir: "/sys/class/net",
        hostname_file: "/etc/hostname",
        os_release_file: "/etc/os-release",
      };
    case "tunnel_traffic":
      return { source: "interface_counters" };
    case "latency_probe":
      return { source: "linux_ping_preset" };
    case "ospf_update_command":
      return {
        contract_version: 1,
        status_command: defaultCommand(),
        update_command: defaultCommand(),
      };
    case "process_inventory":
      return { source: "linux_procfs", proc_root: "/proc" };
    case "user_sessions":
      return { source: "linux_w_who_preset" };
    case "command_execution":
      return {
        shell_script_argv: ["/bin/sh", "-lc"],
        working_directory: null,
        environment_policy: "inherit",
        environment_keep: [],
        environment_set: {},
        pty_policy: "native_pty",
        process_cleanup: "process_group",
      };
  }
}

function defaultDefinitionForSource(
  behavior: ConfigurationBehavior,
  source: string,
): Record<string, JsonValue> {
  if (behavior === "host_metrics") {
    const paths = {
      proc_root: "/proc",
      sys_class_net_dir: "/sys/class/net",
      hostname_file: "/etc/hostname",
      os_release_file: "/etc/os-release",
    };
    if (source === "linux_procfs") return { source, ...paths };
    if (source === "custom_command") {
      return {
        source,
        custom_metrics_command: defaultCommand(),
      };
    }
    return {
      source,
      ...paths,
      custom_metrics_command: defaultCommand(),
    };
  }
  if (behavior === "tunnel_traffic") {
    return source === "vnstat"
      ? { source, vnstat_argv: ["/usr/bin/vnstat"] }
      : { source };
  }
  if (behavior === "latency_probe") {
    return source === "configured_ping_argv"
      ? { source, probe_ping_argv: ["/usr/bin/ping"] }
      : { source };
  }
  if (behavior === "process_inventory") {
    return source === "custom_command"
      ? {
          source,
          process_inventory_command: defaultCommand(),
        }
      : { source, proc_root: "/proc" };
  }
  if (behavior === "user_sessions") {
    return source === "custom_command"
      ? {
          source,
          user_sessions_command: defaultCommand(),
        }
      : { source };
  }
  return defaultDefinition(behavior);
}

function defaultCommand(): Record<string, JsonValue> {
  return {
    argv: [],
    max_timeout_secs: 5,
    max_output_bytes: 16384,
  };
}

function sourceOptions(behavior: ConfigurationBehavior) {
  switch (behavior) {
    case "host_metrics":
      return [
        { value: "linux_procfs", label: "Linux procfs and sysfs" },
        { value: "custom_command", label: "Custom metrics command" },
        {
          value: "linux_procfs_and_custom_command",
          label: "Linux procfs plus custom command",
        },
      ];
    case "tunnel_traffic":
      return [
        { value: "interface_counters", label: "Interface counters" },
        { value: "vnstat", label: "vnStat" },
      ];
    case "latency_probe":
      return [
        { value: "linux_ping_preset", label: "Linux ping" },
        { value: "configured_ping_argv", label: "Configured ping executable" },
      ];
    case "ospf_update_command":
      return [];
    case "process_inventory":
      return [
        { value: "linux_procfs", label: "Linux procfs" },
        { value: "custom_command", label: "Custom process command" },
      ];
    case "user_sessions":
      return [
        { value: "linux_w_who_preset", label: "Linux w/who" },
        { value: "custom_command", label: "Custom session command" },
      ];
    case "command_execution":
      return [];
  }
}

function commandFieldFor(
  behavior: ConfigurationBehavior,
  source: string,
): string | null {
  if (
    behavior === "host_metrics" &&
    ["custom_command", "linux_procfs_and_custom_command"].includes(source)
  ) {
    return "custom_metrics_command";
  }
  if (behavior === "process_inventory" && source === "custom_command") {
    return "process_inventory_command";
  }
  if (behavior === "user_sessions" && source === "custom_command") {
    return "user_sessions_command";
  }
  return null;
}

function commandLabel(behavior: ConfigurationBehavior): string {
  if (behavior === "host_metrics") return "Metrics command";
  if (behavior === "process_inventory") return "Process inventory command";
  return "User sessions command";
}

export function behaviorLabel(behavior: ConfigurationBehavior): string {
  switch (behavior) {
    case "host_metrics":
      return "Host metrics";
    case "tunnel_traffic":
      return "Tunnel traffic accounting";
    case "latency_probe":
      return "Latency checks";
    case "ospf_update_command":
      return "OSPF updater command";
    case "process_inventory":
      return "Process inventory";
    case "user_sessions":
      return "User sessions";
    case "command_execution":
      return "Command execution";
  }
}

function drawerTitle(drawer: DrawerState): string {
  if (!drawer) return "Configuration sources";
  if (drawer.kind === "assign") return "Change effective configuration";
  if (drawer.kind === "create") return "New configuration preset";
  if (drawer.kind === "clone") return "Clone to customize";
  return `Edit ${drawer.preset.name}`;
}

function drawerDescription(drawer: DrawerState): string {
  if (!drawer) return "";
  if (drawer.kind === "assign") {
    return "Choose targets, review the frozen VPS set, then save the selection.";
  }
  if (drawer.kind === "create") {
    return "Create one reusable preset for a behavior.";
  }
  if (drawer.kind === "clone") {
    return `${behaviorLabel(drawer.preset.behavior)} · copied as a custom preset`;
  }
  return `${behaviorLabel(drawer.preset.behavior)} · ${drawer.preset.effective_vps_count} effective VPSs`;
}

function confirmationDetail(confirmation: PendingConfirmation): string {
  if (!confirmation) return "";
  if (confirmation.kind === "override") {
    return confirmation.request.action === "reset"
      ? "Remove the explicit override. Each VPS will inherit the current system default for this behavior."
      : "Save the reviewed preset selection for this exact, frozen VPS set. Runtime sync is queued separately.";
  }
  if (confirmation.kind === "preset") {
    return "Update the custom preset and synchronize every VPS that currently uses it.";
  }
  return "Delete this unused custom preset. This cannot delete a system preset or a preset with explicit overrides.";
}

function confirmationItems(
  confirmation: PendingConfirmation,
  agentById: Map<string, AgentView>,
  vpsNameDisplayMode: VpsNameDisplayMode,
) {
  if (!confirmation) return [];
  if (confirmation.kind === "override") {
    const changedTargets = confirmation.preview.targets.filter(
      (target) =>
        target.before_preset_id !== target.after_preset_id ||
        target.before_origin !== target.after_origin,
    );
    const unchangedTargets = confirmation.preview.targets.filter(
      (target) =>
        target.before_preset_id === target.after_preset_id &&
        target.before_origin === target.after_origin,
    );
    return [
      {
        label: "Behavior",
        value: behaviorLabel(confirmation.request.behavior),
      },
      {
        label: "Frozen targets",
        value: (
          <ConfigurationReviewList
            items={confirmation.preview.targets.map((target) =>
              reviewedVpsLabel(
                target.client_id,
                agentById,
                vpsNameDisplayMode,
              ),
            )}
          />
        ),
      },
      {
        label: "Audit selector",
        value:
          confirmation.preview.selector_expression || "Direct VPS choices only",
      },
      ...(changedTargets.length > 0
        ? [
            {
              label:
                changedTargets.length === 1 ? "Preset change" : "Preset changes",
              value: (
                <ConfigurationReviewList
                  items={changedTargets.map(
                    (target) =>
                      `${reviewedVpsLabel(
                        target.client_id,
                        agentById,
                        vpsNameDisplayMode,
                      )}: ${target.before_preset_name} → ${target.after_preset_name}`,
                  )}
                />
              ),
            },
          ]
        : []),
      ...(unchangedTargets.length > 0
        ? [
            {
              label: "Unchanged targets",
              value: (
                <ConfigurationReviewList
                  items={unchangedTargets.map(
                    (target) =>
                      `${reviewedVpsLabel(
                        target.client_id,
                        agentById,
                        vpsNameDisplayMode,
                      )}: ${target.after_preset_name} already selected; included for runtime resync`,
                  )}
                />
              ),
            },
          ]
        : []),
    ];
  }
  if (confirmation.kind === "preset") {
    return [
      {
        label: "Preset",
        value: confirmation.preset.name,
      },
      {
        label: "Changed fields",
        value: confirmation.preview.changed_keys.join(", ") || "None",
      },
      {
        label: "Affected VPSs",
        value:
          confirmation.preview.affected_client_count === 0
            ? "0 · None"
            : (
                <ConfigurationReviewList
                  items={confirmation.preview.affected_client_ids.map(
                    (clientId) =>
                      reviewedVpsLabel(
                        clientId,
                        agentById,
                        vpsNameDisplayMode,
                      ),
                  )}
                  summary={`${confirmation.preview.affected_client_count} total`}
                />
              ),
      },
    ];
  }
  return [
    { label: "Preset", value: confirmation.preset.name },
    {
      label: "Behavior",
      value: behaviorLabel(confirmation.preset.behavior),
    },
  ];
}

function overrideApplyFeedback(
  response: ApplyConfigurationSourceOverrideResponse,
): LocalFeedback {
  const attentionCount = runtimeSyncAttentionCount(
    response.sync,
    response.target_count,
  );
  if (attentionCount > 0) {
    return {
      message: `Selection saved for ${response.target_count} ${response.target_count === 1 ? "VPS" : "VPSs"}; runtime sync needs attention on ${attentionCount}`,
      tone: "warning",
    };
  }
  return {
    message: `Selection saved and runtime sync queued for ${response.target_count} ${response.target_count === 1 ? "VPS" : "VPSs"}`,
    tone: "success",
  };
}

function presetUpdateFeedback(
  response: UpdateConfigurationPresetResponse,
): LocalFeedback {
  const affectedCount = response.preview.affected_client_count;
  const attentionCount = runtimeSyncAttentionCount(
    response.sync,
    affectedCount,
  );
  if (attentionCount > 0) {
    return {
      message: `Updated ${response.preset.name}; runtime sync needs attention on ${attentionCount}`,
      tone: "warning",
    };
  }
  if (affectedCount === 0) {
    return {
      message: `Updated ${response.preset.name}; no VPS runtime sync was needed`,
      tone: "success",
    };
  }
  return {
    message: `Updated ${response.preset.name}; runtime sync queued for ${affectedCount} ${affectedCount === 1 ? "VPS" : "VPSs"}`,
    tone: "success",
  };
}

function runtimeSyncAttentionCount(
  sync: UpdateConfigurationPresetResponse["sync"],
  expectedCount: number,
): number {
  const failedCount = sync.filter(
    (outcome) => outcome.status !== "queued",
  ).length;
  return failedCount + Math.max(expectedCount - sync.length, 0);
}

function sourceVpsList(
  rows: ConfigurationSourceView[],
  agentById: Map<string, AgentView>,
  vpsNameDisplayMode: VpsNameDisplayMode,
): string {
  if (rows.length === 0) return "None";
  return rows
    .map((row) =>
      reviewedVpsLabel(row.client_id, agentById, vpsNameDisplayMode),
    )
    .sort()
    .join(", ");
}

function reviewedVpsLabel(
  clientId: string,
  agentById: Map<string, AgentView>,
  vpsNameDisplayMode: VpsNameDisplayMode,
): string {
  const agent = agentById.get(clientId);
  return `${agent ? formatVpsName(agent, vpsNameDisplayMode) : "VPS not in current fleet"} · ${clientId}`;
}

function ConfigurationReviewList({
  items,
  summary,
}: {
  items: string[];
  summary?: string;
}) {
  return (
    <span className="configurationReviewList">
      {summary ? <strong>{summary}</strong> : null}
      {items.map((item) => (
        <span key={item} title={item}>
          {item}
        </span>
      ))}
    </span>
  );
}

function readinessIsReady(state: string): boolean {
  return state === "ready";
}

function configurationSourceSummary(
  attentionCount: number,
  unconfiguredCount: number,
): string {
  const parts = [];
  if (attentionCount > 0) {
    parts.push(
      `${attentionCount} ${attentionCount === 1 ? "setting needs" : "settings need"} attention`,
    );
  }
  if (unconfiguredCount > 0) {
    parts.push(
      `${unconfiguredCount} setting${unconfiguredCount === 1 ? "" : "s"} unconfigured`,
    );
  }
  return parts.join(" · ") || "No reported blockers";
}

function readinessRequiresAttention(state: string): boolean {
  return ["degraded", "failed", "invalid", "unconfigured"].includes(state);
}

function readinessTone(state: string): "ok" | "warn" | "neutral" {
  if (readinessIsReady(state)) return "ok";
  return readinessRequiresAttention(state) ? "warn" : "neutral";
}

function syncTone(
  state: ConfigurationSourceView["runtime_sync"]["state"],
): "ok" | "warn" | "danger" | "neutral" | "info" {
  if (state === "applied") return "ok";
  if (state === "failed") return "danger";
  if (state === "queued") return "info";
  if (state === "stale") return "warn";
  return "neutral";
}

function tokenLabel(value: string): string {
  return value
    .split("_")
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

function asObject(value: JsonValue | undefined): Record<string, JsonValue> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...value }
    : {};
}

function asString(value: JsonValue | undefined): string {
  return typeof value === "string" ? value : "";
}

function asStringArray(value: JsonValue | undefined): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

function asNumber(value: JsonValue | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function formatKeyValues(value: JsonValue | undefined): string {
  return Object.entries(asObject(value))
    .map(([key, item]) => `${key}=${typeof item === "string" ? item : ""}`)
    .join("\n");
}

function textDraftsForDefinition(
  definition: Record<string, JsonValue>,
): EditorTextDrafts {
  return {
    "custom_metrics_command.argv": asStringArray(
      asObject(definition.custom_metrics_command).argv,
    ).join("\n"),
    environment_keep: asStringArray(definition.environment_keep).join("\n"),
    environment_set: formatKeyValues(definition.environment_set),
    "process_inventory_command.argv": asStringArray(
      asObject(definition.process_inventory_command).argv,
    ).join("\n"),
    "status_command.argv": asStringArray(
      asObject(definition.status_command).argv,
    ).join("\n"),
    probe_ping_argv: asStringArray(definition.probe_ping_argv).join("\n"),
    shell_script_argv: asStringArray(definition.shell_script_argv).join("\n"),
    "user_sessions_command.argv": asStringArray(
      asObject(definition.user_sessions_command).argv,
    ).join("\n"),
    "update_command.argv": asStringArray(
      asObject(definition.update_command).argv,
    ).join("\n"),
    vnstat_argv: asStringArray(definition.vnstat_argv).join("\n"),
  };
}

type MaterializedDefinition =
  | {
      definition: Record<string, JsonValue>;
      error: null;
    }
  | {
      definition: null;
      error: string;
    };

function materializeDefinition(
  behavior: ConfigurationBehavior,
  definition: Record<string, JsonValue>,
  textDrafts: EditorTextDrafts,
): MaterializedDefinition {
  const candidate = { ...definition };
  if (behavior === "ospf_update_command") {
    candidate.status_command = {
      ...defaultCommand(),
      ...asObject(candidate.status_command),
      argv: parseMultilineDraft(textDrafts["status_command.argv"] ?? ""),
    };
    candidate.update_command = {
      ...defaultCommand(),
      ...asObject(candidate.update_command),
      argv: parseMultilineDraft(textDrafts["update_command.argv"] ?? ""),
    };
    return { definition: candidate, error: null };
  }
  if (behavior === "command_execution") {
    candidate.shell_script_argv = parseMultilineDraft(
      textDrafts.shell_script_argv ?? "",
    );
    candidate.environment_keep = parseMultilineDraft(
      textDrafts.environment_keep ?? "",
    );
    const environmentSet = parseEnvironmentSetDraft(
      textDrafts.environment_set ?? "",
    );
    if (environmentSet.error) {
      return { definition: null, error: environmentSet.error };
    }
    candidate.environment_set = environmentSet.value;
    return { definition: candidate, error: null };
  }

  const source = asString(candidate.source);
  if (behavior === "tunnel_traffic" && source === "vnstat") {
    candidate.vnstat_argv = parseMultilineDraft(textDrafts.vnstat_argv ?? "");
  }
  if (behavior === "latency_probe" && source === "configured_ping_argv") {
    candidate.probe_ping_argv = parseMultilineDraft(
      textDrafts.probe_ping_argv ?? "",
    );
  }
  const commandField = commandFieldFor(behavior, source);
  if (commandField) {
    candidate[commandField] = {
      ...asObject(candidate[commandField]),
      argv: parseMultilineDraft(textDrafts[`${commandField}.argv`] ?? ""),
    };
  }
  return { definition: candidate, error: null };
}

function parseMultilineDraft(value: string): string[] {
  return value === "" ? [] : value.split("\n");
}

function parseEnvironmentSetDraft(
  value: string,
):
  | { error: null; value: Record<string, JsonValue> }
  | { error: string; value: null } {
  if (value === "") {
    return { error: null, value: {} };
  }
  const entries: Record<string, JsonValue> = {};
  const seen = new Set<string>();
  const lines = value.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const lineNumber = index + 1;
    if (line === "") {
      return {
        error: `Environment values line ${lineNumber} is empty; remove it or enter KEY=value`,
        value: null,
      };
    }
    const separator = line.indexOf("=");
    if (separator <= 0) {
      return {
        error: `Environment values line ${lineNumber} must use KEY=value`,
        value: null,
      };
    }
    const key = line.slice(0, separator);
    if (seen.has(key)) {
      return {
        error: `Environment name ${key} is repeated on line ${lineNumber}`,
        value: null,
      };
    }
    seen.add(key);
    entries[key] = line.slice(separator + 1);
  }
  return { error: null, value: entries };
}

function validatePresetDefinition(
  behavior: ConfigurationBehavior,
  definition: Record<string, JsonValue>,
): string | null {
  if (behavior === "ospf_update_command") {
    if (definition.contract_version !== 1) {
      return "OSPF updater contract version must be 1";
    }
    return (
      validateBoundedCommand(
        definition.status_command,
        "Read current OSPF cost",
      ) ??
      validateBoundedCommand(definition.update_command, "Update OSPF cost")
    );
  }
  if (behavior === "command_execution") {
    const argvError = validateArgv(
      definition.shell_script_argv,
      "Shell command arguments",
    );
    if (argvError) return argvError;
    const workingDirectory = definition.working_directory;
    if (workingDirectory !== null && typeof workingDirectory !== "string") {
      return "Working directory must be an absolute path or left empty";
    }
    if (typeof workingDirectory === "string") {
      const pathError = validateAbsolutePath(
        workingDirectory,
        "Working directory",
      );
      if (pathError) return pathError;
    }
    if (
      !["inherit", "clean", "minimal_path"].includes(
        asString(definition.environment_policy),
      )
    ) {
      return "Choose a command environment policy";
    }
    if (!["native_pty", "disabled"].includes(asString(definition.pty_policy))) {
      return "Choose a terminal policy";
    }
    if (
      !["process_group", "direct_child"].includes(
        asString(definition.process_cleanup),
      )
    ) {
      return "Choose a process cleanup policy";
    }
    const environmentKeep = definition.environment_keep;
    if (
      !Array.isArray(environmentKeep) ||
      environmentKeep.some((key) => typeof key !== "string")
    ) {
      return "Environment names to keep must contain only names";
    }
    if (environmentKeep.length > 64) {
      return "Environment names to keep may contain at most 64 entries";
    }
    for (const key of environmentKeep as string[]) {
      const keyError = validateEnvironmentKey(key);
      if (keyError) return keyError;
    }
    const environmentSet = definition.environment_set;
    if (
      !environmentSet ||
      typeof environmentSet !== "object" ||
      Array.isArray(environmentSet)
    ) {
      return "Environment values must use KEY=value entries";
    }
    const environmentEntries = Object.entries(environmentSet);
    if (environmentEntries.length > 64) {
      return "Environment values may contain at most 64 entries";
    }
    for (const [key, value] of environmentEntries) {
      const keyError = validateEnvironmentKey(key);
      if (keyError) return keyError;
      if (
        typeof value !== "string" ||
        utf8Length(value) > 4096 ||
        value.includes("\0")
      ) {
        return `Environment value ${key} must be text no longer than 4096 bytes`;
      }
    }
    return null;
  }

  const source = asString(definition.source);
  if (!sourceOptions(behavior).some((option) => option.value === source)) {
    return `Choose a ${behaviorLabel(behavior).toLowerCase()} source type`;
  }
  const requiredPaths =
    behavior === "host_metrics" &&
    ["linux_procfs", "linux_procfs_and_custom_command"].includes(source)
      ? [
          ["proc_root", "Proc filesystem"],
          ["sys_class_net_dir", "Network devices"],
          ["hostname_file", "Hostname file"],
          ["os_release_file", "OS release file"],
        ]
      : behavior === "process_inventory" && source === "linux_procfs"
        ? [["proc_root", "Proc filesystem"]]
        : [];
  for (const [field, label] of requiredPaths) {
    const pathError = validateAbsolutePath(asString(definition[field]), label);
    if (pathError) return pathError;
  }
  if (behavior === "tunnel_traffic" && source === "vnstat") {
    return validateArgv(definition.vnstat_argv, "vnStat arguments");
  }
  if (behavior === "latency_probe" && source === "configured_ping_argv") {
    return validateArgv(definition.probe_ping_argv, "Ping arguments");
  }
  const commandField = commandFieldFor(behavior, source);
  if (commandField) {
    return validateBoundedCommand(
      definition[commandField],
      commandLabel(behavior),
    );
  }
  return null;
}

function validateBoundedCommand(
  value: JsonValue | undefined,
  label: string,
): string | null {
  const command = asObject(value);
  const argvError = validateArgv(command.argv, `${label} arguments`);
  if (argvError) return argvError;
  const timeout = command.max_timeout_secs;
  if (
    typeof timeout !== "number" ||
    !Number.isInteger(timeout) ||
    timeout < 1 ||
    timeout > 120
  ) {
    return `${label} timeout must be a whole number from 1 to 120 seconds`;
  }
  const output = command.max_output_bytes;
  if (
    typeof output !== "number" ||
    !Number.isInteger(output) ||
    output < 1024 ||
    output > 65536
  ) {
    return `${label} maximum output must be 1024 to 65536 bytes`;
  }
  return null;
}

function validateArgv(
  value: JsonValue | undefined,
  label: string,
): string | null {
  if (
    !Array.isArray(value) ||
    value.some((argument) => typeof argument !== "string")
  ) {
    return `${label} must contain only text arguments`;
  }
  const argv = value as string[];
  if (argv.length === 0) {
    return `${label} require an executable`;
  }
  if (argv.length > 32) {
    return `${label} may contain at most 32 arguments`;
  }
  const invalidArgument = argv.find(
    (argument) =>
      argument.length === 0 ||
      utf8Length(argument) > 4096 ||
      argument.includes("\0"),
  );
  if (invalidArgument !== undefined) {
    return `${label} arguments must be non-empty and no longer than 4096 bytes`;
  }
  if (!argv[0].startsWith("/")) {
    return `${label} must start with an absolute executable path`;
  }
  if (hasControlCharacter(argv[0])) {
    return `${label} executable path cannot contain control characters`;
  }
  return null;
}

function validateAbsolutePath(value: string, label: string): string | null {
  if (!value.startsWith("/")) {
    return `${label} must be an absolute path`;
  }
  if (utf8Length(value) > 4096) {
    return `${label} must be no longer than 4096 bytes`;
  }
  if (hasControlCharacter(value)) {
    return `${label} cannot contain control characters`;
  }
  if (value.split("/").some((segment) => segment === "." || segment === "..")) {
    return `${label} cannot contain . or .. path segments`;
  }
  return null;
}

function validateEnvironmentKey(key: string): string | null {
  if (!/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(key)) {
    return `Environment name ${key || "(empty)"} must start with a letter or underscore and contain only letters, numbers, or underscores (128 characters maximum)`;
  }
  return null;
}

function hasControlCharacter(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
  });
}

function utf8Length(value: string): number {
  return new TextEncoder().encode(value).length;
}
