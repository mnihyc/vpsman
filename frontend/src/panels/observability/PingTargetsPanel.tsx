import {
  Pencil,
  Plus,
  Power,
  PowerOff,
  RefreshCw,
  Star,
  Target,
  Trash2,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { apiGet, apiPost, apiPut } from "../../api";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  ConsoleActionDrawer,
  ConsoleStatusBadge,
} from "../../components/ConsoleLayout";
import { SearchExpressionInput } from "../../components/SearchExpressionInput";
import { useReviewGenerationGuard } from "../../hooks/useReviewGenerationGuard";
import { scrollIntoViewWithMotion } from "../../motion";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../../searchExpression";
import type {
  AgentView,
  BulkPingTargetLifecycleRequest,
  BulkPingTargetLifecycleResponse,
  BulkResolveResponse,
  BulkUpdatePingTargetsResponse,
  JobTargetSelection,
  MakePrimaryPingTargetRequest,
  PingTargetAssignmentChangeView,
  PingTargetAssignmentView,
  PingTargetDetailView,
  PingTargetMutationRequest,
  PingTargetMutationResponse,
  PingTargetView,
  RuntimeConfigDispatchRecord,
} from "../../types";
import {
  dispatchFailureReason,
  formatCompactTime,
  formatTime,
  formatVpsName,
  runPanelAction,
} from "../../utils";
import { LocalTargetPreview } from "../TargetImpactPreview";

type EditorState =
  | { mode: "create" }
  | {
      assignments: PingTargetAssignmentView[];
      mode: "edit";
      original: PingTargetView;
    }
  | null;

type SaveReview = {
  assignmentCount: number;
  kind: "create" | "update";
  probeChanged: boolean;
  request: PingTargetMutationRequest;
  targetId: string | null;
  targetName: string;
};

type LifecycleReview = {
  action: "enable" | "disable" | "delete";
  targets: PingTargetView[];
};

type UpdateTargetsReview = {
  preview: BulkUpdatePingTargetsResponse;
  targetIds: string[];
};

type Feedback = {
  message: string;
  tone: ActionFeedbackTone;
};

type RuntimeEvidence = {
  label: string;
  title: string;
  tone: "neutral" | "ok" | "warning";
};

export function PingTargetsPanel({
  agents,
  apiToken,
  onResolveTargets,
}: {
  agents: AgentView[];
  apiToken: string;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [targets, setTargets] = useState<PingTargetView[]>([]);
  const [details, setDetails] = useState<Record<string, PingTargetDetailView>>(
    {},
  );
  const [detailErrors, setDetailErrors] = useState<Record<string, string>>({});
  const [detailLoading, setDetailLoading] = useState<Record<string, boolean>>(
    {},
  );
  const [runtimeEvidence, setRuntimeEvidence] = useState<
    Record<string, RuntimeEvidence>
  >({});
  const [expandedTargetId, setExpandedTargetId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [editor, setEditor] = useState<EditorState>(null);
  const [saveReview, setSaveReview] = useState<SaveReview | null>(null);
  const [updateTargetsReview, setUpdateTargetsReview] =
    useState<UpdateTargetsReview | null>(null);
  const [lifecycleReview, setLifecycleReview] =
    useState<LifecycleReview | null>(null);
  const [name, setName] = useState("");
  const [host, setHost] = useState("");
  const [probeKind, setProbeKind] = useState<"icmp" | "tcp">("icmp");
  const [port, setPort] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [selectorExpression, setSelectorExpression] = useState("*");
  const pageFeedbackRef = useRef<HTMLDivElement | null>(null);
  const editorFeedbackRef = useRef<HTMLDivElement | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const pageFeedbackMessage =
    editor === null ? (error ?? feedback?.message) : null;
  const pageFeedbackTone = error && editor === null ? "danger" : feedback?.tone;

  useEffect(() => {
    if (!pageFeedbackMessage) return;
    const frame = window.requestAnimationFrame(() => {
      if (pageFeedbackRef.current) {
        scrollIntoViewWithMotion(pageFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [pageFeedbackMessage]);

  useEffect(() => {
    if (!editor || !error) return;
    const frame = window.requestAnimationFrame(() => {
      if (editorFeedbackRef.current) {
        scrollIntoViewWithMotion(editorFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [editor, error]);

  useEffect(() => {
    let active = true;
    invalidateReviewGeneration();
    setReviewPending(false);
    setLoading(true);
    setError(null);
    setTargets([]);
    setDetails({});
    setDetailErrors({});
    setRuntimeEvidence({});
    setExpandedTargetId(null);
    setEditor(null);
    setSaveReview(null);
    setUpdateTargetsReview(null);
    setLifecycleReview(null);
    void apiGet<PingTargetView[]>("/api/v1/ping-targets", apiToken)
      .then((records) => {
        if (!active) return;
        setTargets(records);
      })
      .catch((cause) => {
        if (!active) return;
        setError(errorMessage(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [apiToken, invalidateReviewGeneration]);

  const parsedSelector = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const selectorChanged =
    editor?.mode === "edit" &&
    selectorExpression.trim() !== editor.original.selector_expression.trim();
  const localTargets = useMemo(() => {
    if (!editor || parsedSelector.error || !selectorExpression.trim()) {
      return [];
    }
    if (editor.mode === "edit" && !selectorChanged) {
      return editor.assignments.map((assignment) => assignment.client);
    }
    return agentsMatchingExpression(agents, selectorExpression);
  }, [
    agents,
    editor,
    parsedSelector.error,
    selectorChanged,
    selectorExpression,
  ]);
  const editorReady = Boolean(
    editor &&
    name.trim() &&
    host.trim() &&
    selectorExpression.trim() &&
    !parsedSelector.error &&
    (probeKind === "icmp" || validPort(port)),
  );

  function changeEditorDraft(change: () => void) {
    invalidateReviewGeneration();
    setReviewPending(false);
    setSaveReview(null);
    setError(null);
    change();
  }

  function enterReviewWorkflow(workflow: "editor" | "targets" | "lifecycle") {
    invalidateReviewGeneration();
    setReviewPending(false);
    setSaveReview(null);
    setUpdateTargetsReview(null);
    setLifecycleReview(null);
    if (workflow !== "editor") {
      setEditor(null);
    }
    setError(null);
    setFeedback(null);
  }

  async function refreshTargets() {
    setLoading(true);
    setError(null);
    try {
      const records = await apiGet<PingTargetView[]>(
        "/api/v1/ping-targets",
        apiToken,
      );
      setTargets(records);
      setRuntimeEvidence({});
      setDetails({});
      setDetailErrors({});
      if (
        expandedTargetId &&
        records.some((target) => target.id === expandedTargetId)
      ) {
        await fetchDetail(expandedTargetId);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }

  async function fetchDetail(targetId: string): Promise<PingTargetDetailView> {
    setDetailLoading((current) => ({ ...current, [targetId]: true }));
    setDetailErrors((current) => omitKey(current, targetId));
    try {
      const detail = await apiGet<PingTargetDetailView>(
        `/api/v1/ping-targets/${encodeURIComponent(targetId)}`,
        apiToken,
      );
      setDetails((current) => ({ ...current, [targetId]: detail }));
      setTargets((current) => replaceTarget(current, detail.target));
      return detail;
    } catch (cause) {
      const message = errorMessage(cause);
      setDetailErrors((current) => ({ ...current, [targetId]: message }));
      throw cause;
    } finally {
      setDetailLoading((current) => ({ ...current, [targetId]: false }));
    }
  }

  function openCreate() {
    enterReviewWorkflow("editor");
    setEditor({ mode: "create" });
    setName("");
    setHost("");
    setProbeKind("icmp");
    setPort("");
    setEnabled(true);
    setSelectorExpression("*");
  }

  async function openEdit(target: PingTargetView) {
    enterReviewWorkflow("editor");
    await runPanelAction(setPending, setError, async () => {
      const detail = details[target.id] ?? (await fetchDetail(target.id));
      setEditor({
        assignments: detail.assignments,
        mode: "edit",
        original: detail.target,
      });
      setName(detail.target.name);
      setHost(detail.target.host);
      setProbeKind(detail.target.probe_kind === "tcp" ? "tcp" : "icmp");
      setPort(detail.target.port === null ? "" : String(detail.target.port));
      setEnabled(detail.target.enabled);
      setSelectorExpression(detail.target.selector_expression);
      setFeedback(null);
    });
  }

  async function reviewEditorSave(event: FormEvent) {
    event.preventDefault();
    if (!editorReady || !editor) return;
    const reviewGeneration = captureReviewGeneration();
    const frozenEditor = editor;
    const frozenSelectorChanged = selectorChanged;
    const frozenName = name;
    const frozenHost = host;
    const frozenProbeKind = probeKind;
    const frozenPort = port;
    const frozenEnabled = enabled;
    const frozenSelectorExpression = selectorExpression;
    setReviewPending(true);
    setSaveReview(null);
    setError(null);
    try {
      const resolved =
        frozenEditor.mode === "edit" && !frozenSelectorChanged
          ? frozenEditor.assignments.map((assignment) => assignment.client)
          : (
              await onResolveTargets({
                selector_expression: frozenSelectorExpression.trim(),
              })
            ).targets;
      if (!isReviewGenerationCurrent(reviewGeneration)) return;
      const targetClientIds = uniqueSorted(resolved.map((agent) => agent.id));
      const request = editorRequest({
        enabled: frozenEnabled,
        host: frozenHost,
        name: frozenName,
        port: frozenPort,
        probeKind: frozenProbeKind,
        selectorExpression: frozenSelectorExpression,
        targetClientIds,
      });
      const original =
        frozenEditor.mode === "edit" ? frozenEditor.original : null;
      setSaveReview({
        assignmentCount: targetClientIds.length,
        kind: original ? "update" : "create",
        probeChanged: Boolean(
          original &&
          (original.host !== request.host ||
            original.probe_kind !== request.probe_kind ||
            original.port !== (request.port ?? null) ||
            original.enabled !== request.enabled),
        ),
        request,
        targetId: original?.id ?? null,
        targetName: request.name,
      });
    } catch (cause) {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setError(errorMessage(cause));
      }
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewPending(false);
      }
    }
  }

  async function confirmSave() {
    if (!saveReview) return;
    await runPanelAction(setPending, setError, async () => {
      const response = saveReview.targetId
        ? await apiPut<PingTargetMutationResponse>(
            `/api/v1/ping-targets/${encodeURIComponent(saveReview.targetId)}`,
            apiToken,
            saveReview.request,
          )
        : await apiPost<PingTargetMutationResponse>(
            "/api/v1/ping-targets",
            apiToken,
            saveReview.request,
          );
      const saved = response.target.target;
      setTargets((current) => replaceTarget(current, saved));
      setDetails((current) => ({ ...current, [saved.id]: response.target }));
      setRuntimeEvidence((current) => ({
        ...current,
        [saved.id]: runtimeEvidenceFor(response.runtime_sync),
      }));
      setFeedback(
        mutationFeedback(
          response.runtime_sync,
          saveReview.kind === "create"
            ? `${saved.name} created with ${saved.assigned_count} frozen VPS assignment${saved.assigned_count === 1 ? "" : "s"}`
            : `${saved.name} updated with ${saved.assigned_count} frozen VPS assignment${saved.assigned_count === 1 ? "" : "s"}`,
        ),
      );
      setSaveReview(null);
      setEditor(null);
    });
  }

  async function reviewTargetUpdates(rows: PingTargetView[]) {
    if (rows.length === 0) return;
    enterReviewWorkflow("targets");
    await runPanelAction(setPending, setError, async () => {
      const targetIds = uniqueSorted(rows.map((row) => row.id));
      const preview = await apiPost<BulkUpdatePingTargetsResponse>(
        "/api/v1/ping-targets/update-targets",
        apiToken,
        { target_ids: targetIds, confirmed: false },
      );
      if (!pingTargetChangesPresent(preview.changes)) {
        setFeedback({
          message:
            "Frozen assignments already match the saved selector for every selected Ping target. Nothing was changed.",
          tone: "info",
        });
        return;
      }
      setUpdateTargetsReview({ preview, targetIds });
    });
  }

  async function confirmTargetUpdates() {
    if (!updateTargetsReview) return;
    await runPanelAction(setPending, setError, async () => {
      const response = await apiPost<BulkUpdatePingTargetsResponse>(
        "/api/v1/ping-targets/update-targets",
        apiToken,
        {
          target_ids: updateTargetsReview.targetIds,
          preview_hash: updateTargetsReview.preview.preview_hash,
          confirmed: true,
        },
      );
      const evidence = runtimeEvidenceFor(response.runtime_sync);
      setRuntimeEvidence((current) => ({
        ...current,
        ...Object.fromEntries(
          updateTargetsReview.targetIds.map((targetId) => [targetId, evidence]),
        ),
      }));
      setDetails((current) =>
        Object.fromEntries(
          Object.entries(current).filter(
            ([targetId]) => !updateTargetsReview.targetIds.includes(targetId),
          ),
        ),
      );
      setFeedback(
        mutationFeedback(
          response.runtime_sync,
          `Updated frozen assignments for ${response.changes.length} Ping target${response.changes.length === 1 ? "" : "s"}`,
        ),
      );
      setUpdateTargetsReview(null);
      await refreshTargets();
    });
  }

  async function confirmLifecycle() {
    if (!lifecycleReview) return;
    await runPanelAction(setPending, setError, async () => {
      const request: BulkPingTargetLifecycleRequest = {
        action: lifecycleReview.action,
        confirmed: true,
        target_ids: uniqueSorted(
          lifecycleReview.targets.map((target) => target.id),
        ),
      };
      const response = await apiPost<BulkPingTargetLifecycleResponse>(
        "/api/v1/ping-targets/lifecycle",
        apiToken,
        request,
      );
      const count = response.affected_target_ids.length;
      const evidence = runtimeEvidenceFor(response.runtime_sync);
      setRuntimeEvidence((current) => ({
        ...current,
        ...Object.fromEntries(
          response.affected_target_ids.map((targetId) => [targetId, evidence]),
        ),
      }));
      setDetails((current) =>
        Object.fromEntries(
          Object.entries(current).filter(
            ([targetId]) => !response.affected_target_ids.includes(targetId),
          ),
        ),
      );
      setLifecycleReview(null);
      setFeedback(
        mutationFeedback(
          response.runtime_sync,
          `${count} Ping target${count === 1 ? "" : "s"} ${lifecyclePastTense(response.action)}${response.action === "disable" ? "; primary assignments remain explicit and inactive" : response.action === "delete" ? "; assignments and retained Ping history were removed" : ""}`,
        ),
      );
      await refreshTargets();
    });
  }

  async function makePrimary(
    target: PingTargetView,
    assignments: PingTargetAssignmentView[],
  ) {
    setFeedback(null);
    await runPanelAction(setPending, setError, async () => {
      const request: MakePrimaryPingTargetRequest = {
        client_ids: uniqueSorted(
          assignments.map((assignment) => assignment.client.id),
        ),
      };
      const response = await apiPost<PingTargetMutationResponse>(
        `/api/v1/ping-targets/${encodeURIComponent(target.id)}/primary`,
        apiToken,
        request,
      );
      setTargets((current) => replaceTarget(current, response.target.target));
      setDetails({ [target.id]: response.target });
      let refreshFailure: string | null = null;
      try {
        const records = await apiGet<PingTargetView[]>(
          "/api/v1/ping-targets",
          apiToken,
        );
        setTargets(records);
      } catch (cause) {
        refreshFailure = errorMessage(cause);
      }
      setFeedback(
        refreshFailure
          ? {
              message: `${target.name} is now primary for ${assignments.length} VPS${assignments.length === 1 ? "" : "s"}, but the other target counts could not be refreshed: ${refreshFailure}`,
              tone: "warning",
            }
          : {
              message: `${target.name} is now the primary Ping target for ${assignments.length} VPS${assignments.length === 1 ? "" : "s"}.`,
              tone: "success",
            },
      );
    });
  }

  const columns = useMemo<ConsoleDataGridColumn<PingTargetView>[]>(
    () => [
      {
        id: "target",
        header: "Ping target",
        mobilePrimary: true,
        cell: (target) => (
          <span className="historyPrimary">
            <strong>{target.name}</strong>
            <small>
              {target.probe_kind.toUpperCase()} · {target.host}
              {target.port === null ? "" : `:${target.port}`}
            </small>
          </span>
        ),
        searchValue: (target) =>
          `${target.name} ${target.host} ${target.port ?? ""} ${target.probe_kind}`,
        sortValue: (target) => target.name,
        minSize: 220,
        size: 260,
      },
      {
        id: "state",
        header: "State",
        mobileState: true,
        cell: (target) => (
          <ConsoleStatusBadge tone={target.enabled ? "ok" : "neutral"}>
            {target.enabled ? "Enabled" : "Disabled"}
          </ConsoleStatusBadge>
        ),
        searchValue: (target) => (target.enabled ? "enabled" : "disabled"),
        sortValue: (target) => Number(target.enabled),
        size: 112,
      },
      {
        id: "selector",
        header: "Saved selector",
        cell: (target) => <code>{target.selector_expression}</code>,
        searchValue: (target) => target.selector_expression,
        sortValue: (target) => target.selector_expression,
        minSize: 180,
        size: 240,
      },
      {
        id: "assigned",
        header: "Assigned",
        align: "end",
        cell: (target) => target.assigned_count,
        searchValue: (target) => target.assigned_count,
        sortValue: (target) => target.assigned_count,
        size: 105,
      },
      {
        id: "primary",
        header: "Primary",
        align: "end",
        cell: (target) => target.primary_count,
        searchValue: (target) => target.primary_count,
        sortValue: (target) => target.primary_count,
        size: 100,
      },
      {
        id: "generation",
        header: "Generation",
        align: "end",
        cell: (target) => target.generation,
        searchValue: (target) => target.generation,
        sortValue: (target) => target.generation,
        size: 112,
      },
      {
        id: "runtime",
        header: "Runtime sync",
        cell: (target) => {
          const evidence =
            runtimeEvidence[target.id] ?? runtimeEvidenceForTarget(target);
          return (
            <span title={evidence.title}>
              <ConsoleStatusBadge tone={evidence.tone}>
                {evidence.label}
              </ConsoleStatusBadge>
            </span>
          );
        },
        searchValue: (target) =>
          (runtimeEvidence[target.id] ?? runtimeEvidenceForTarget(target))
            .title,
        sortValue: (target) =>
          (runtimeEvidence[target.id] ?? runtimeEvidenceForTarget(target))
            .label,
        minSize: 160,
        size: 180,
      },
      {
        id: "updated",
        header: "Updated",
        cell: (target) => formatCompactTime(target.updated_at),
        searchValue: (target) => formatTime(target.updated_at),
        sortValue: (target) => target.updated_at,
        size: 150,
      },
    ],
    [runtimeEvidence],
  );

  const actions: ConsoleDataGridAction<PingTargetView>[] = [
    {
      description: (rows) =>
        rows.length === 1
          ? `Edit ${rows[0].name}. Its frozen VPS assignments change only when the selector expression changes.`
          : "Edit supports one Ping target at a time.",
      disabled: (rows) => pending || rows.length !== 1,
      icon: <Pencil size={14} />,
      label: "Edit",
      onSelect: (rows) => rows[0] && void openEdit(rows[0]),
    },
    {
      description: (rows) =>
        rows.length
          ? `Review and enable ${rows.length} selected Ping target${rows.length === 1 ? "" : "s"} in one transaction without changing frozen assignments.`
          : "Select one or more Ping targets to enable.",
      disabled: (rows) =>
        pending || rows.length === 0 || rows.every((row) => row.enabled),
      icon: <Power size={14} />,
      label: "Enable",
      onSelect: (rows) => {
        enterReviewWorkflow("lifecycle");
        setLifecycleReview({ action: "enable", targets: rows });
      },
    },
    {
      description: (rows) =>
        rows.length
          ? `Review and disable ${rows.length} selected Ping target${rows.length === 1 ? "" : "s"} in one transaction. Primary assignments remain explicit and inactive.`
          : "Select one or more Ping targets to disable.",
      disabled: (rows) =>
        pending || rows.length === 0 || rows.every((row) => !row.enabled),
      icon: <PowerOff size={14} />,
      label: "Disable",
      onSelect: (rows) => {
        enterReviewWorkflow("lifecycle");
        setLifecycleReview({ action: "disable", targets: rows });
      },
    },
    {
      description: (rows) =>
        rows.length
          ? `Re-resolve the saved selector for ${rows.length} Ping target${rows.length === 1 ? "" : "s"}, then preview exact additions and removals before one transactional apply.`
          : "Select one or more Ping targets to check their frozen assignments.",
      disabled: (rows) =>
        pending ||
        rows.length === 0 ||
        rows.every((row) => !row.target_update_available),
      icon: <Target size={14} />,
      label: "Update targets",
      onSelect: (rows) => void reviewTargetUpdates(rows),
    },
    {
      description: (rows) =>
        rows.length
          ? `Delete ${rows.length} selected Ping target${rows.length === 1 ? "" : "s"}, frozen assignments, and retained Ping history in one transaction.`
          : "Select one or more Ping targets to delete.",
      disabled: (rows) => pending || rows.length === 0,
      icon: <Trash2 size={14} />,
      label: "Delete",
      onSelect: (rows) => {
        enterReviewWorkflow("lifecycle");
        setLifecycleReview({ action: "delete", targets: rows });
      },
      separatorBefore: true,
      tone: "danger",
    },
  ];

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Ping targets</h2>
            <span>
              Reusable ICMP and TCP checks with frozen VPS assignments and one
              explicit primary target per VPS.
            </span>
          </div>
        </div>
        <ActionFeedback
          className="localActionFeedback"
          message={pageFeedbackMessage}
          ref={pageFeedbackRef}
          tone={pageFeedbackTone}
        />
        <ConsoleDataGrid
          actions={actions}
          columns={columns}
          defaultPageSize={100}
          empty={
            <div className="emptyState">
              <strong>
                {loading ? "Loading Ping targets" : "No Ping targets"}
              </strong>
              <span>
                {loading
                  ? "Reading reusable target definitions and assignment counts."
                  : "Create an ICMP or TCP target; its selector is resolved into a frozen VPS list when saved."}
              </span>
            </div>
          }
          getRowId={(target) => target.id}
          itemLabel="Ping targets"
          onExpandedRowChange={(target) => {
            setExpandedTargetId(target?.id ?? null);
            if (target && !details[target.id] && !detailLoading[target.id]) {
              void fetchDetail(target.id).catch(() => undefined);
            }
          }}
          openRowOnClick={false}
          renderExpandedRow={(target) =>
            renderTargetAssignments({
              detail: details[target.id] ?? null,
              error: detailErrors[target.id] ?? null,
              loading: Boolean(detailLoading[target.id]),
              onMakePrimary: (assignments) =>
                void makePrimary(target, assignments),
              pending,
              target,
              vpsNameDisplayMode,
            })
          }
          rows={targets}
          searchPlaceholder="Search name, host, protocol, selector, or state"
          singleExpandedRow
          storageKey="vpsman.observability.pingTargets"
          title="Ping targets"
          toolbarActions={
            <div className="previewMeta">
              <button
                className="secondaryAction compactAction"
                disabled={loading}
                onClick={() => void refreshTargets()}
                title={
                  loading
                    ? "Ping target definitions are already loading"
                    : "Refresh Ping target definitions, frozen assignments, and runtime evidence"
                }
                type="button"
              >
                <RefreshCw size={14} />
                Refresh
              </button>
              <button
                className="primaryAction compactAction"
                disabled={pending || reviewPending}
                onClick={openCreate}
                title={
                  pending || reviewPending
                    ? "Wait for the current Ping target review or operation to finish"
                    : "Create a reusable ICMP or TCP Ping target"
                }
                type="button"
              >
                <Plus size={14} />
                Create Ping target
              </button>
            </div>
          }
        />
      </div>

      <ConsoleActionDrawer
        description="The selector is resolved once when created or changed. Use Update targets later to re-resolve the saved expression."
        onClose={() => {
          if (pending) return;
          invalidateReviewGeneration();
          setReviewPending(false);
          setEditor(null);
          setSaveReview(null);
          setError(null);
        }}
        open={editor !== null}
        title={
          editor?.mode === "edit"
            ? `Edit ${editor.original.name}`
            : "Create Ping target"
        }
      >
        <form className="compactForm" onSubmit={reviewEditorSave}>
          <ActionFeedback
            message={editor ? error : null}
            ref={editorFeedbackRef}
            tone="danger"
          />
          <div className="formRow">
            <label
              className="actionDrawerInitialFocus"
              title={
                pending
                  ? "Ping target name editing is disabled while an operation is pending"
                  : "Unique operator-facing Ping target name"
              }
            >
              <span>Name</span>
              <input
                aria-label="Ping target name"
                data-tooltip-disabled-reason="Wait for the current Ping target operation to finish before editing the name."
                disabled={pending}
                maxLength={128}
                onChange={(event) =>
                  changeEditorDraft(() => setName(event.target.value))
                }
                placeholder="Frankfurt gateway"
                required
                value={name}
              />
            </label>
            <label
              title={
                pending
                  ? "Probe selection is disabled while a Ping target operation is pending"
                  : "ICMP sends echo probes; TCP opens a connection to the configured port"
              }
            >
              <span>Probe</span>
              <select
                aria-label="Ping target probe"
                data-tooltip-disabled-reason="Wait for the current Ping target operation to finish before changing probe type."
                disabled={pending}
                onChange={(event) => {
                  const next = event.target.value === "tcp" ? "tcp" : "icmp";
                  changeEditorDraft(() => {
                    setProbeKind(next);
                    if (next === "icmp") setPort("");
                  });
                }}
                value={probeKind}
              >
                <option value="icmp">ICMP</option>
                <option value="tcp">TCP</option>
              </select>
            </label>
          </div>
          <div className="formRow">
            <label
              title={
                pending
                  ? "Host editing is disabled while a Ping target operation is pending"
                  : "Hostname or literal IPv4/IPv6 address probed by assigned VPSs"
              }
            >
              <span>Host or IP</span>
              <input
                aria-label="Ping target host or IP"
                data-tooltip-disabled-reason="Wait for the current Ping target operation to finish before editing the host."
                disabled={pending}
                maxLength={253}
                onChange={(event) =>
                  changeEditorDraft(() => setHost(event.target.value))
                }
                placeholder="edge.example.net or 2001:db8::1"
                required
                value={host}
              />
            </label>
            {probeKind === "tcp" && (
              <label
                title={
                  pending
                    ? "TCP port editing is disabled while a Ping target operation is pending"
                    : "TCP destination port opened by each probe"
                }
              >
                <span>TCP port</span>
                <input
                  aria-label="Ping target TCP port"
                  data-tooltip-disabled-reason="Wait for the current Ping target operation to finish before editing the TCP port."
                  disabled={pending}
                  max={65_535}
                  min={1}
                  onChange={(event) =>
                    changeEditorDraft(() => setPort(event.target.value))
                  }
                  required
                  type="number"
                  value={port}
                />
              </label>
            )}
          </div>
          <label
            className="inlineCheck tightCheck"
            title={
              pending
                ? "Enabled state is disabled while a Ping target operation is pending"
                : enabled
                  ? "This target dispatches probes and can be selected as primary."
                  : "This target remains saved without dispatching probes."
            }
          >
            <input
              checked={enabled}
              disabled={pending}
              onChange={(event) =>
                changeEditorDraft(() => setEnabled(event.target.checked))
              }
              type="checkbox"
            />
            <span>Enabled</span>
          </label>
          <div
            className="targetSelector"
            title={
              pending
                ? "VPS selector editing is disabled while a Ping target operation is pending"
                : "The server resolves this expression to a frozen VPS assignment list during review"
            }
          >
            <div className="targetSelectorHeader">
              <strong>VPS selector</strong>
              <span>
                {editor?.mode === "edit" && !selectorChanged
                  ? `${localTargets.length} frozen assignment${localTargets.length === 1 ? "" : "s"}; editing other fields does not re-resolve it`
                  : `${localTargets.length} local match${localTargets.length === 1 ? "" : "es"}; server resolution is reviewed before save`}
              </span>
            </div>
            <SearchExpressionInput
              agents={agents}
              ariaLabel="Ping target VPS selector"
              className="targetExpressionBar"
              disabled={pending}
              onChange={(value) =>
                changeEditorDraft(() => setSelectorExpression(value))
              }
              placeholder="* or provider:hetzner && country:DE"
              showMatchCount
              value={selectorExpression}
              verification={
                parsedSelector.error
                  ? "invalid"
                  : selectorExpression.trim()
                    ? "valid"
                    : "neutral"
              }
              verificationMessage={
                parsedSelector.error ??
                (editor?.mode === "edit" && !selectorChanged
                  ? `${localTargets.length} frozen`
                  : `${localTargets.length}/${agents.length} local`)
              }
            />
            <LocalTargetPreview
              agents={localTargets}
              ariaLabel={
                editor?.mode === "edit" && !selectorChanged
                  ? "Frozen Ping target VPS assignments"
                  : "Ping target local VPS preview"
              }
            />
            {localTargets.length === 0 && !parsedSelector.error && (
              <span className="formHint">
                This selector currently resolves no VPSs. The target can still
                be saved and assigned later with Update targets.
              </span>
            )}
          </div>
          <button
            className="primaryAction"
            disabled={
              pending || reviewPending || !editorReady || saveReview !== null
            }
            title={
              pending
                ? "Wait for the current Ping target operation to finish"
                : reviewPending
                  ? "The VPS selector is already being resolved for review"
                  : saveReview !== null
                    ? "Finish or cancel the current Ping target review"
                    : !name.trim()
                      ? "Enter a Ping target name"
                      : !host.trim()
                        ? "Enter a Ping target host or IP address"
                        : probeKind === "tcp" && !validPort(port)
                          ? "Enter a valid TCP port from 1 to 65535"
                          : parsedSelector.error
                            ? `Fix the VPS selector: ${parsedSelector.error}`
                            : !selectorExpression.trim()
                              ? "Enter a VPS selector expression"
                              : editor?.mode === "edit"
                                ? "Resolve the selector if changed and review the Ping target update"
                                : "Resolve the selector and review the new Ping target"
            }
            type="submit"
          >
            {reviewPending
              ? "Reviewing targets…"
              : editor?.mode === "edit"
                ? "Review changes"
                : "Review create"}
          </button>
          <ConfirmationPrompt
            confirmLabel={saveReviewConfirmLabel(saveReview)}
            detail={saveReviewDetail(saveReview)}
            error={error}
            items={saveReviewItems(saveReview)}
            onCancel={() => {
              if (pending) return;
              setSaveReview(null);
              setError(null);
            }}
            onConfirm={() => void confirmSave()}
            open={saveReview !== null}
            pending={pending}
            title="Confirm Ping target change"
            tone="normal"
          />
        </form>
      </ConsoleActionDrawer>

      <ConfirmationPrompt
        confirmLabel="Update frozen targets"
        detail="Apply the exact additions and removals resolved from each saved selector. All selected target definitions are updated transactionally."
        error={error}
        items={updateTargetReviewItems(updateTargetsReview, agents)}
        onCancel={() => {
          if (pending) return;
          setUpdateTargetsReview(null);
          setError(null);
        }}
        onConfirm={() => void confirmTargetUpdates()}
        open={updateTargetsReview !== null}
        pending={pending}
        title="Review frozen target updates"
        tone="warning"
      />

      <ConfirmationPrompt
        confirmLabel={
          lifecycleReview
            ? `${lifecycleVerb(lifecycleReview.action)} selected`
            : "Apply"
        }
        detail={lifecycleReviewDetail(lifecycleReview)}
        error={error}
        items={lifecycleReviewItems(lifecycleReview)}
        onCancel={() => {
          if (pending) return;
          setLifecycleReview(null);
          setError(null);
        }}
        onConfirm={() => void confirmLifecycle()}
        open={lifecycleReview !== null}
        pending={pending}
        title={
          lifecycleReview
            ? `${lifecycleVerb(lifecycleReview.action)} Ping targets`
            : "Ping target lifecycle"
        }
        tone={
          lifecycleReview?.action === "delete"
            ? "danger"
            : lifecycleReview?.action === "disable"
              ? "warning"
              : "normal"
        }
        typedConfirmationLabel={
          lifecycleReview?.action === "delete"
            ? "Type the confirmation phrase"
            : undefined
        }
        typedConfirmationText={
          lifecycleReview?.action === "delete"
            ? `DELETE ${lifecycleReview.targets.length}`
            : undefined
        }
      />
    </section>
  );
}

function renderTargetAssignments({
  detail,
  error,
  loading,
  onMakePrimary,
  pending,
  target,
  vpsNameDisplayMode,
}: {
  detail: PingTargetDetailView | null;
  error: string | null;
  loading: boolean;
  onMakePrimary: (assignments: PingTargetAssignmentView[]) => void;
  pending: boolean;
  target: PingTargetView;
  vpsNameDisplayMode: "name" | "name_id_suffix";
}) {
  if (error) {
    return <ActionFeedback message={error} tone="danger" />;
  }
  if (loading || !detail) {
    return (
      <div className="emptyState compactEmpty" role="status">
        Reading frozen VPS assignments…
      </div>
    );
  }
  const assignmentColumns: ConsoleDataGridColumn<PingTargetAssignmentView>[] = [
    {
      id: "vps",
      header: "Assigned VPS",
      mobilePrimary: true,
      cell: (assignment) => (
        <span className="historyPrimary">
          <strong title={assignment.client.id}>
            {formatVpsName(assignment.client, vpsNameDisplayMode)}
          </strong>
          <small>{assignment.client.id}</small>
        </span>
      ),
      searchValue: (assignment) =>
        `${formatVpsName(assignment.client, vpsNameDisplayMode)} ${assignment.client.id}`,
      sortValue: (assignment) =>
        formatVpsName(assignment.client, vpsNameDisplayMode),
      minSize: 220,
      size: 280,
    },
    {
      id: "state",
      header: "VPS state",
      mobileState: true,
      cell: (assignment) => (
        <ConsoleStatusBadge tone={agentStatusTone(assignment.client.status)}>
          {assignment.client.status}
        </ConsoleStatusBadge>
      ),
      searchValue: (assignment) => assignment.client.status,
      sortValue: (assignment) => assignment.client.status,
      size: 130,
    },
    {
      id: "role",
      header: "Card role",
      cell: (assignment) =>
        assignment.is_primary ? (
          <ConsoleStatusBadge tone={target.enabled ? "ok" : "warning"}>
            {target.enabled ? "Primary" : "Primary · disabled"}
          </ConsoleStatusBadge>
        ) : (
          "Assigned"
        ),
      searchValue: (assignment) =>
        assignment.is_primary ? "primary" : "assigned",
      sortValue: (assignment) => Number(assignment.is_primary),
      minSize: 150,
      size: 170,
    },
    {
      id: "assigned_at",
      header: "Frozen at",
      cell: (assignment) => formatTime(assignment.assigned_at),
      searchValue: (assignment) => formatTime(assignment.assigned_at),
      sortValue: (assignment) => assignment.assigned_at,
      size: 190,
    },
  ];
  const assignmentActions: ConsoleDataGridAction<PingTargetAssignmentView>[] = [
    {
      description: (rows) =>
        !target.enabled
          ? "Enable this Ping target before selecting it as primary."
          : rows.every((assignment) => assignment.is_primary)
            ? "This target is already primary for every selected VPS."
            : `Make ${target.name} the primary card Ping for ${rows.length} selected VPS${rows.length === 1 ? "" : "s"}.`,
      disabled: (rows) =>
        pending ||
        !target.enabled ||
        rows.length === 0 ||
        rows.every((assignment) => assignment.is_primary),
      icon: <Star size={14} />,
      label: "Make primary",
      onSelect: onMakePrimary,
    },
  ];
  return (
    <div className="compactForm">
      <div
        className="targetSelectorHeader"
        title="Assignments stay frozen until an operator explicitly resolves the saved selector again."
      >
        <strong>Frozen VPS assignments</strong>
        <span
          title={`${detail.assignments.length} assigned · ${detail.target.primary_count} primary · selector ${detail.target.selector_expression}`}
        >
          {detail.assignments.length} assigned · {detail.target.primary_count}{" "}
          primary · selector <code>{detail.target.selector_expression}</code>
        </span>
      </div>
      <ConsoleDataGrid
        actions={assignmentActions}
        columns={assignmentColumns}
        defaultPageSize={100}
        empty={
          <div className="emptyState compactEmpty">
            <strong>No assigned VPSs</strong>
            <span>
              Use Update targets after the saved selector matches registered
              VPSs.
            </span>
          </div>
        }
        getRowId={(assignment) => assignment.client.id}
        itemLabel="VPS assignments"
        openRowOnClick={false}
        rows={detail.assignments}
        searchPlaceholder="Search assigned VPSs"
        storageKey={`vpsman.observability.pingTargets.assignments.${target.id}`}
        title={`${target.name} assignments`}
      />
    </div>
  );
}

function editorRequest({
  enabled,
  host,
  name,
  port,
  probeKind,
  selectorExpression,
  targetClientIds,
}: {
  enabled: boolean;
  host: string;
  name: string;
  port: string;
  probeKind: "icmp" | "tcp";
  selectorExpression: string;
  targetClientIds: string[];
}): PingTargetMutationRequest {
  return {
    name: name.trim(),
    host: host.trim(),
    probe_kind: probeKind,
    port: probeKind === "tcp" ? Number(port) : null,
    enabled,
    selector_expression: selectorExpression.trim(),
    target_client_ids: targetClientIds,
    confirmed: true,
  };
}

function saveReviewConfirmLabel(review: SaveReview | null): string {
  if (!review) return "Save Ping target";
  return review.kind === "create" ? "Create Ping target" : "Save Ping target";
}

function saveReviewDetail(review: SaveReview | null): ReactNode {
  if (!review) return "Review the exact Ping target change before applying.";
  return review.probeChanged
    ? "Save this definition. Probe behavior or enabled state changed, so a new result generation begins and prior results are not mixed with it."
    : "Save this definition and the exact frozen VPS list shown below.";
}

function saveReviewItems(
  review: SaveReview | null,
): Array<{ label: string; value: ReactNode }> {
  if (!review) return [];
  return [
    { label: "Ping target", value: review.targetName },
    {
      label: "Endpoint",
      value: `${review.request.probe_kind.toUpperCase()} ${review.request.host}${review.request.port == null ? "" : `:${review.request.port}`}`,
    },
    {
      label: "State",
      value: review.request.enabled ? "Enabled" : "Disabled",
    },
    {
      label: "Saved selector",
      value: review.request.selector_expression ?? "",
    },
    {
      label: "Frozen VPSs",
      value: String(review.assignmentCount),
    },
    ...(review.probeChanged
      ? [{ label: "History boundary", value: "New generation" }]
      : []),
  ];
}

function lifecycleVerb(action: LifecycleReview["action"]) {
  if (action === "enable") return "Enable";
  if (action === "disable") return "Disable";
  return "Delete";
}

function lifecyclePastTense(action: LifecycleReview["action"]) {
  if (action === "enable") return "enabled";
  if (action === "disable") return "disabled";
  return "deleted";
}

function lifecycleReviewDetail(review: LifecycleReview | null) {
  if (!review) return "Review the exact lifecycle change before applying.";
  if (review.action === "enable") {
    return "Enable every selected definition in one transaction without changing its frozen VPS assignments. Each changed target starts a new result generation.";
  }
  if (review.action === "disable") {
    return "Disable every selected definition in one transaction. Existing assignments and primary choices remain explicit but inactive; each changed target starts a new result generation.";
  }
  return "Delete every selected definition, frozen assignment, and retained Ping history in one transaction. This cannot be undone.";
}

function lifecycleReviewItems(
  review: LifecycleReview | null,
): Array<{ label: string; title?: string; value: ReactNode }> {
  if (!review) return [];
  const names = review.targets.map((target) => target.name);
  return [
    { label: "Action", value: lifecycleVerb(review.action) },
    {
      label: `Ping targets (${review.targets.length})`,
      title: names.join(", "),
      value:
        names.length <= 8
          ? names.join(", ")
          : `${names.slice(0, 8).join(", ")} · +${names.length - 8} more`,
    },
    {
      label: "Frozen assignments",
      value: String(
        review.targets.reduce(
          (total, target) => total + target.assigned_count,
          0,
        ),
      ),
    },
    {
      label: "Primary assignments",
      value: String(
        review.targets.reduce(
          (total, target) => total + target.primary_count,
          0,
        ),
      ),
    },
    ...(review.action === "delete"
      ? []
      : [{ label: "History boundary", value: "New generation" }]),
  ];
}

function updateTargetReviewItems(
  review: UpdateTargetsReview | null,
  agents: AgentView[],
): Array<{ label: string; title?: string; value: ReactNode }> {
  if (!review) return [];
  const agentsById = new Map(agents.map((agent) => [agent.id, agent]));
  return review.preview.changes.flatMap((change) => [
    {
      label: `${change.target_name} · selector`,
      value: <code>{change.selector_expression}</code>,
    },
    {
      label: `${change.target_name} · add (${change.added_client_ids.length})`,
      title: change.added_client_ids.join(", "),
      value: exactClientList(change.added_client_ids, agentsById),
    },
    {
      label: `${change.target_name} · remove (${change.removed_client_ids.length})`,
      title: change.removed_client_ids.join(", "),
      value: exactClientList(change.removed_client_ids, agentsById),
    },
    {
      label: `${change.target_name} · unchanged`,
      value: String(change.unchanged_count),
    },
  ]);
}

function exactClientList(
  clientIds: string[],
  agentsById: Map<string, AgentView>,
): ReactNode {
  if (clientIds.length === 0) return "None";
  return (
    <span>
      {clientIds.map((clientId, index) => {
        const agent = agentsById.get(clientId);
        return (
          <span key={clientId} title={clientId}>
            {index > 0 ? ", " : ""}
            {agent?.display_name?.trim() || "Unknown VPS"} ({clientId})
          </span>
        );
      })}
    </span>
  );
}

function pingTargetChangesPresent(
  changes: PingTargetAssignmentChangeView[],
): boolean {
  return changes.some(
    (change) =>
      change.added_client_ids.length > 0 ||
      change.removed_client_ids.length > 0,
  );
}

function runtimeEvidenceFor(
  sync: RuntimeConfigDispatchRecord[],
): RuntimeEvidence {
  if (sync.length === 0) {
    return {
      label: "No dispatch needed",
      title: "The saved change required no agent runtime-config dispatch.",
      tone: "neutral",
    };
  }
  const failures = sync.filter((record) => record.status !== "queued");
  if (failures.length > 0) {
    return {
      label: `${sync.length - failures.length} queued · ${failures.length} failed`,
      title: failures
        .map(
          (record) =>
            `${record.client_id}: ${dispatchFailureReason(record.error, record.status, "Ping runtime sync")}`,
        )
        .join("; "),
      tone: "warning",
    };
  }
  return {
    label: `${sync.length} queued`,
    title: `Runtime configuration queued for ${sync.length} VPS${sync.length === 1 ? "" : "s"}.`,
    tone: "ok",
  };
}

function runtimeEvidenceForTarget(target: PingTargetView): RuntimeEvidence {
  const state = target.runtime_sync.state;
  return {
    label:
      state === "not_applicable"
        ? "Not applicable"
        : humanizeRuntimeState(state),
    title: target.runtime_sync.reason,
    tone:
      state === "applied"
        ? "ok"
        : state === "failed" || state === "stale"
          ? "warning"
          : "neutral",
  };
}

function humanizeRuntimeState(value: string): string {
  const normalized = value.trim().replace(/_/g, " ");
  return normalized
    ? normalized[0].toLocaleUpperCase() + normalized.slice(1)
    : "Unknown";
}

function mutationFeedback(
  sync: RuntimeConfigDispatchRecord[],
  success: string,
): Feedback {
  const evidence = runtimeEvidenceFor(sync);
  if (evidence.tone === "warning") {
    return {
      message: `${success}. Desired state was saved, but runtime sync was incomplete: ${evidence.title}`,
      tone: "warning",
    };
  }
  return {
    message: sync.length > 0 ? `${success}. ${evidence.title}` : `${success}.`,
    tone: sync.length > 0 ? "progress" : "success",
  };
}

function replaceTarget(
  current: PingTargetView[],
  next: PingTargetView,
): PingTargetView[] {
  const found = current.some((target) => target.id === next.id);
  const records = found
    ? current.map((target) => (target.id === next.id ? next : target))
    : [...current, next];
  return records.sort((left, right) => left.name.localeCompare(right.name));
}

function omitKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  return Object.fromEntries(
    Object.entries(record).filter(([candidate]) => candidate !== key),
  ) as Record<string, T>;
}

function uniqueSorted(values: string[]): string[] {
  return Array.from(new Set(values)).sort();
}

function validPort(value: string): boolean {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= 1 && parsed <= 65_535;
}

function agentStatusTone(
  status: string,
): "critical" | "warning" | "ok" | "neutral" {
  const normalized = status.trim().toLowerCase();
  if (normalized === "online" || normalized === "connected") return "ok";
  if (normalized === "offline") return "critical";
  if (normalized === "stale" || normalized === "degraded") return "warning";
  return "neutral";
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error
    ? cause.message
    : "The Ping target action returned no diagnostic detail. No success is assumed; refresh current state before retrying.";
}
