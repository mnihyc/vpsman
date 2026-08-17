import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  ChevronsUpDown,
  Code2,
  ExternalLink,
  FileSliders,
  FolderTree,
  LockKeyhole,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  ServerCog,
  Trash2,
} from "lucide-react";
import { parse, stringify, type TomlTable } from "smol-toml";
import { ActionFeedback } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ConsoleStatusBadge } from "../../components/ConsoleLayout";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  createJobTargetCount,
  waitForBulkJobTargets,
} from "../../bulkJobProgress";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../../privilege";
import type {
  AgentView,
  ApplyRuntimeConfigOverrideRequest,
  ApplyRuntimeConfigOverrideResponse,
  CreateJobRequest,
  CreateJobResponse,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JsonValue,
  PreviewRuntimeConfigOverrideRequest,
  RuntimeConfigClientWorkspace,
  RuntimeConfigFieldSchemaRecord,
  RuntimeConfigOverrideCandidate,
  RuntimeConfigOverridePreview,
  RuntimeConfigProvenanceRecord,
} from "../../types";
import { dispatchFailureReason, formatVpsName, shortId } from "../../utils";
import {
  clampJobMaxTimeoutSecs,
  DEFAULT_MAX_JOB_TIMEOUT_SECS,
} from "../jobDispatchModel";

const CLIENT_STORAGE_KEY = "vpsman.config.single.clientId";
const EXPANSION_STORAGE_KEY = "vpsman.config.single.tree.expanded";

type CandidateMode = "structured" | "toml";
type EditorMode = "tree" | "advanced";

type ReviewedOverride = {
  candidate: RuntimeConfigOverrideCandidate;
  preview: RuntimeConfigOverridePreview;
  privilegeAssertion: ApplyRuntimeConfigOverrideRequest["privilege_assertion"];
};

type LiveConfigEvidence = {
  comparison: "match" | "different" | "unavailable";
  jobId: string;
  runtimeConfig: JsonValue;
};

export function SingleVpsConfigWorkspace({
  actionError,
  agents,
  onApplyOverride,
  onCreateJob,
  onLoadJobOutputs,
  onLoadJobTargets,
  onLoadWorkspace,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onPreviewOverride,
  pending,
  privilegeMaterial,
  runAction,
}: {
  actionError: string | null;
  agents: AgentView[];
  onApplyOverride: (
    clientId: string,
    request: ApplyRuntimeConfigOverrideRequest,
  ) => Promise<ApplyRuntimeConfigOverrideResponse>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onLoadWorkspace: (clientId: string) => Promise<RuntimeConfigClientWorkspace>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onPreviewOverride: (
    clientId: string,
    request: PreviewRuntimeConfigOverrideRequest,
  ) => Promise<RuntimeConfigOverridePreview>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [clientId, setClientId] = useState(() =>
    readLocalString(CLIENT_STORAGE_KEY),
  );
  const [workspace, setWorkspace] =
    useState<RuntimeConfigClientWorkspace | null>(null);
  const [draftOverride, setDraftOverride] = useState<JsonValue>({});
  const [advancedToml, setAdvancedToml] = useState("");
  const [advancedError, setAdvancedError] = useState<string | null>(null);
  const [candidateMode, setCandidateMode] =
    useState<CandidateMode>("structured");
  const [editorMode, setEditorMode] = useState<EditorMode>("tree");
  const [resetRequested, setResetRequested] = useState(false);
  const [search, setSearch] = useState("");
  const initialExpansionState = useMemo(() => readExpansionState(), []);
  const expansionStateInitialized = useRef(initialExpansionState !== null);
  const [expanded, setExpanded] = useState<Set<string>>(
    () => initialExpansionState ?? new Set(),
  );
  const [preview, setPreview] = useState<RuntimeConfigOverridePreview | null>(
    null,
  );
  const [reviewed, setReviewed] = useState<ReviewedOverride | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [liveEvidence, setLiveEvidence] = useState<LiveConfigEvidence | null>(
    null,
  );
  const loadGeneration = useRef(0);
  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === clientId) ?? null,
    [agents, clientId],
  );
  const selectedAgentId = selectedAgent?.id ?? "";
  const savedParsed = asObject(workspace?.saved_override.parsed) ?? {};
  const draftDirty = useMemo(() => {
    if (!workspace) return false;
    if (resetRequested) return workspace.saved_override.exists;
    if (candidateMode === "toml") {
      return advancedToml !== workspace.saved_override.toml;
    }
    return !jsonEqual(draftOverride, savedParsed);
  }, [
    advancedToml,
    candidateMode,
    draftOverride,
    resetRequested,
    savedParsed,
    workspace,
  ]);
  const treePaused = Boolean(advancedError);
  const activeCandidate = useMemo<RuntimeConfigOverrideCandidate>(() => {
    if (resetRequested) return { type: "reset" };
    if (candidateMode === "toml") return { type: "toml", toml: advancedToml };
    return { type: "structured", value: draftOverride };
  }, [advancedToml, candidateMode, draftOverride, resetRequested]);
  const effectiveDraftOverride = useMemo<JsonValue>(
    () => (resetRequested ? {} : draftOverride),
    [draftOverride, resetRequested],
  );
  const draftDesired = useMemo(
    () => deepMerge(workspace?.inherited ?? {}, effectiveDraftOverride),
    [effectiveDraftOverride, workspace?.inherited],
  );
  const previewDeletesSavedOverride = Boolean(
    workspace?.saved_override.exists && preview?.canonical_toml === null,
  );
  const reviewedDeletesSavedOverride = Boolean(
    workspace?.saved_override.exists &&
    reviewed?.preview.canonical_toml === null,
  );

  useEffect(() => {
    const generation = ++loadGeneration.current;
    writeLocalString(CLIENT_STORAGE_KEY, clientId);
    setWorkspace(null);
    setLiveEvidence(null);
    setStatus(null);
    setPreview(null);
    setReviewed(null);
    setConfirmOpen(false);
    if (!selectedAgentId) return;
    void runAction(async () => {
      const next = await onLoadWorkspace(selectedAgentId);
      if (generation !== loadGeneration.current) return;
      initializeWorkspace(next);
    });
    // Loading is intentionally tied to the resolved target identity. This also
    // handles a browser-restored client id whose agent arrives asynchronously.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clientId, selectedAgentId, onLoadWorkspace]);

  useEffect(() => {
    if (expansionStateInitialized.current) writeExpansionState(expanded);
  }, [expanded]);

  const initializeWorkspace = useCallback(
    (next: RuntimeConfigClientWorkspace) => {
      const parsed = asObject(next.saved_override.parsed) ?? {};
      setWorkspace(next);
      setDraftOverride(parsed);
      setAdvancedToml(next.saved_override.toml);
      setAdvancedError(next.saved_override.diagnostic);
      setCandidateMode(next.saved_override.diagnostic ? "toml" : "structured");
      setEditorMode(next.saved_override.diagnostic ? "advanced" : "tree");
      setResetRequested(false);
      setPreview(null);
      setReviewed(null);
      setConfirmOpen(false);
      setReason(next.saved_override.reason ?? "");
      if (!expansionStateInitialized.current) {
        expansionStateInitialized.current = true;
        setExpanded(new Set(topLevelPointers(next.desired)));
      }
    },
    [],
  );

  async function reloadWorkspace(message?: string) {
    if (!selectedAgent) return;
    const targetId = selectedAgent.id;
    const generation = ++loadGeneration.current;
    await runAction(async () => {
      const next = await onLoadWorkspace(targetId);
      if (generation !== loadGeneration.current) return;
      initializeWorkspace(next);
      if (message) setStatus(message);
    });
  }

  function invalidateReview(nextStatus: string | null = null) {
    ++loadGeneration.current;
    setPreview(null);
    setReviewed(null);
    setConfirmOpen(false);
    setStatus(nextStatus);
  }

  function updateStructured(path: string[], value: JsonValue) {
    const next = setAtPath(
      resetRequested ? {} : (asObject(draftOverride) ?? {}),
      path,
      value,
    );
    setDraftOverride(next);
    setAdvancedToml(stringifyOverride(next));
    setAdvancedError(null);
    setCandidateMode("structured");
    setResetRequested(false);
    invalidateReview();
  }

  function useInherited(path: string[]) {
    const next = deleteAtPath(
      resetRequested ? {} : (asObject(draftOverride) ?? {}),
      path,
    );
    setDraftOverride(next);
    setAdvancedToml(stringifyOverride(next));
    setAdvancedError(null);
    setCandidateMode("structured");
    setResetRequested(false);
    invalidateReview();
  }

  function resetField(path: string[]) {
    if (hasAtPath(savedParsed, path)) {
      updateStructured(
        path,
        cloneJson(getAtPath(savedParsed, path) as JsonValue),
      );
    } else {
      useInherited(path);
    }
  }

  function resetDraft() {
    setDraftOverride(savedParsed);
    setAdvancedToml(workspace?.saved_override.toml ?? "");
    setAdvancedError(workspace?.saved_override.diagnostic ?? null);
    setCandidateMode(
      workspace?.saved_override.diagnostic ? "toml" : "structured",
    );
    setResetRequested(false);
    invalidateReview("Draft restored to the saved VPS override");
  }

  function requestOverrideReset() {
    setDraftOverride({});
    setAdvancedToml("");
    setCandidateMode("structured");
    setResetRequested(true);
    setAdvancedError(null);
    invalidateReview("VPS override reset is ready for review");
  }

  function syncAdvancedToTree(showSuccess: boolean) {
    const parsed = parseTomlDocument(advancedToml);
    if (!parsed.ok) {
      setAdvancedError(parsed.error);
      setCandidateMode("toml");
      invalidateReview(
        "Advanced TOML is preserved exactly; fix the parse error before using Tree",
      );
      return false;
    }
    const policyError = validateAdvancedOverride(
      parsed.value,
      workspace?.field_schema ?? [],
    );
    if (policyError) {
      setAdvancedError(policyError);
      setCandidateMode("toml");
      invalidateReview(
        "Advanced TOML is preserved exactly; repair the field before using Tree",
      );
      return false;
    }
    setDraftOverride(parsed.value);
    setAdvancedError(null);
    setCandidateMode("toml");
    setResetRequested(false);
    invalidateReview(showSuccess ? "Advanced TOML applied to the tree" : null);
    return true;
  }

  function selectEditorMode(next: EditorMode) {
    if (
      next === "tree" &&
      candidateMode === "toml" &&
      !syncAdvancedToTree(false)
    ) {
      setEditorMode("advanced");
      return;
    }
    setEditorMode(next);
  }

  async function reviewChanges() {
    if (!selectedAgent || !workspace) return;
    if (!draftDirty) {
      setStatus("No VPS override changes are waiting for review");
      return;
    }
    if (advancedError) {
      setStatus("Fix the exact Advanced TOML error before review");
      return;
    }
    const targetId = selectedAgent.id;
    const candidate = cloneJson(activeCandidate);
    const generation = ++loadGeneration.current;
    await runAction(async () => {
      setStatus("Building server preview");
      const next = await onPreviewOverride(targetId, {
        candidate,
        reason: reason.trim() || null,
      });
      if (generation !== loadGeneration.current) return;
      setPreview(next);
      setReviewed(null);
      setStatus(
        workspace.saved_override.exists && next.canonical_toml === null
          ? "Review ready: delete the saved VPS override"
          : next.storage_only
            ? "Review ready: stored TOML changes, effective runtime values do not"
            : `Review ready: ${next.changes.length} runtime value ${next.changes.length === 1 ? "change" : "changes"}`,
      );
    });
  }

  async function prepareApply() {
    if (!selectedAgent || !workspace || !preview) return;
    if (!privilegeMaterial) {
      setStatus("Unlock privilege to apply this reviewed VPS override");
      onOpenPrivilegeUnlock();
      return;
    }
    const targetId = selectedAgent.id;
    const reviewedPreview = preview;
    const candidate = cloneJson(activeCandidate);
    const generation = ++loadGeneration.current;
    await runAction(async () => {
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "runtime_config.override.apply",
          target: `client:${targetId}`,
          selectorExpression: null,
          resolvedTargets: [targetId],
          confirmed: true,
          payloadHash: reviewedPreview.preview_hash,
        }),
        privilegeMaterial,
      });
      if (generation !== loadGeneration.current) return;
      setReviewed({ candidate, preview: reviewedPreview, privilegeAssertion });
      setConfirmOpen(true);
    });
  }

  async function applyReviewed() {
    if (!selectedAgent || !reviewed) return;
    const targetId = selectedAgent.id;
    const reviewedSnapshot = reviewed;
    const generation = ++loadGeneration.current;
    setConfirmOpen(false);
    await runAction(async () => {
      const response = await onApplyOverride(targetId, {
        candidate: reviewedSnapshot.candidate,
        reason: reason.trim() || null,
        expected_override_revision: reviewedSnapshot.preview.override_revision,
        expected_desired_hash: reviewedSnapshot.preview.desired_hash,
        preview_hash: reviewedSnapshot.preview.preview_hash,
        confirmed: true,
        privilege_assertion: reviewedSnapshot.privilegeAssertion,
      });
      if (generation !== loadGeneration.current) return;
      const savedStatus =
        dispatchWarning(response, "VPS override saved") ??
        "VPS override saved and runtime sync queued";
      const appliedOverride =
        asObject(response.preview.candidate_override) ?? {};
      const overrideRecord = response.override_record;
      setWorkspace((current) =>
        current
          ? {
              ...current,
              desired: cloneJson(response.preview.desired),
              desired_toml: response.preview.desired_toml,
              provenance: response.preview.provenance,
              saved_override: {
                diagnostic: null,
                exists: Boolean(overrideRecord),
                parsed: cloneJson(appliedOverride),
                reason: overrideRecord?.reason ?? null,
                toml: response.preview.canonical_toml ?? "",
                updated_at: overrideRecord?.updated_at ?? null,
                updated_by: overrideRecord?.updated_by ?? null,
              },
            }
          : current,
      );
      setDraftOverride(cloneJson(appliedOverride));
      setAdvancedToml(response.preview.canonical_toml ?? "");
      setAdvancedError(null);
      setCandidateMode("structured");
      setEditorMode("tree");
      setResetRequested(false);
      setPreview(null);
      setReviewed(null);
      setConfirmOpen(false);
      setReason(overrideRecord?.reason ?? "");
      setStatus(savedStatus);
      try {
        const next = await onLoadWorkspace(targetId);
        if (generation !== loadGeneration.current) return;
        initializeWorkspace(next);
        setStatus(savedStatus);
      } catch {
        if (generation !== loadGeneration.current) return;
        setStatus(
          `${savedStatus}. Desired state refresh failed; use Refresh desired before editing again.`,
        );
      }
    });
  }

  async function refreshLiveConfig() {
    if (!selectedAgent || !workspace) return;
    const target = selectedAgent;
    const savedDesired = cloneJson(workspace.desired);
    const generation = ++loadGeneration.current;
    await runAction(async () => {
      setStatus("Reading current runtime config from the VPS");
      const operation: JobOperation = { type: "config_read" };
      const selectorExpression = `id:${target.id}`;
      const maxTimeoutSecs = clampJobMaxTimeoutSecs(
        DEFAULT_MAX_JOB_TIMEOUT_SECS,
      );
      const response = await onCreateJob({
        argv: [],
        command: "config_read",
        confirmed: false,
        destructive: false,
        force_unprivileged: true,
        job_id: crypto.randomUUID(),
        operation,
        privileged: false,
        selector_expression: selectorExpression,
        target_client_ids: [target.id],
        max_timeout_secs: maxTimeoutSecs,
      });
      await waitForBulkJobTargets(response.job_id, onLoadJobTargets, {
        targetCount: createJobTargetCount(response),
        targets: [target],
        maxTimeoutSecs,
      });
      const runtimeConfig = extractConfigRead(
        await onLoadJobOutputs(response.job_id),
      );
      if (generation !== loadGeneration.current) return;
      const comparison = compareLiveDesired(runtimeConfig, savedDesired);
      setLiveEvidence({
        comparison,
        jobId: response.job_id,
        runtimeConfig,
      });
      setStatus(
        comparison === "match"
          ? "Live config matches the saved desired runtime values"
          : comparison === "different"
            ? "Live config differs from the saved desired runtime values"
            : "Live config was read, but its runtime values could not be compared",
      );
    });
  }

  const normalizedSearch = search.trim().toLowerCase();
  const allContainerPointers = useMemo(() => {
    const pointers = new Set(collectContainerPointers(draftDesired));
    for (const field of workspace?.field_schema ?? []) {
      if (
        field.collection ||
        field.value_type === "array" ||
        field.value_type === "object" ||
        field.control === "section"
      ) {
        pointers.add(normalizePointer(field.pointer, field.path));
      }
    }
    pointers.delete("");
    return [...pointers];
  }, [draftDesired, workspace?.field_schema]);
  const allContainersExpanded =
    allContainerPointers.length > 0 &&
    allContainerPointers.every((pointer) => expanded.has(pointer));
  const desiredChangeCount = preview?.changes.length ?? 0;

  return (
    <div className="singleConfigWorkspace">
      <section
        className="singleConfigToolbar"
        aria-label="VPS runtime config workspace controls"
      >
        <div className="singleConfigTargetControl">
          <label htmlFor="single-vps-config-target">VPS</label>
          <VpsCombobox
            agents={agents}
            ariaLabel="VPS config target"
            className="configTargetCombobox"
            onChange={(nextClientId) => {
              ++loadGeneration.current;
              setClientId(nextClientId);
            }}
            placeholder="Search VPS config"
            value={clientId}
          />
          {selectedAgent ? (
            <span>{formatVpsName(selectedAgent, vpsNameDisplayMode)}</span>
          ) : (
            <span>
              Select one VPS to load its server-desired runtime configuration.
            </span>
          )}
        </div>
        <div className="singleConfigToolbarActions">
          <button
            className="secondaryAction compactAction"
            disabled={pending || !selectedAgent}
            onClick={() => void reloadWorkspace("Workspace refreshed")}
            type="button"
          >
            <RefreshCw size={15} />
            <span>Refresh desired</span>
          </button>
          <button
            className="secondaryAction compactAction"
            disabled={pending || !workspace}
            onClick={() => void refreshLiveConfig()}
            type="button"
          >
            <ServerCog size={15} />
            <span>Refresh live</span>
          </button>
        </div>
      </section>

      <ActionFeedback
        className="localActionFeedback singleConfigWorkspaceFeedback"
        message={actionError ?? status}
        tone={
          actionError
            ? "danger"
            : status?.includes("runtime sync was not queued") ||
                status?.includes("differs from the saved desired")
              ? "warning"
              : status?.startsWith("VPS override saved")
                ? "success"
                : "progress"
        }
      />

      {!selectedAgent ? (
        <section
          className="singleConfigWelcome"
          aria-label="Per-VPS config start"
        >
          <FolderTree size={22} />
          <div>
            <strong>One desired configuration, edited in place</strong>
            <span>
              Inherited values, VPS overrides, ownership, and locked fields
              appear in the same hierarchy.
            </span>
          </div>
        </section>
      ) : !workspace ? (
        <section className="singleConfigWelcome" aria-live="polite">
          <RefreshCw className="spin" size={22} />
          <div>
            <strong>Loading desired runtime config</strong>
            <span>
              The editor opens after server ownership and field schema are
              available.
            </span>
          </div>
        </section>
      ) : (
        <>
          <section
            className="singleConfigSummary"
            aria-label="VPS runtime config summary"
          >
            <SummaryItem
              label="Desired content"
              value={shortId(workspace.desired_content_hash)}
              title={workspace.desired_content_hash}
            />
            <SummaryItem
              label="Override"
              value={
                workspace.saved_override.exists
                  ? `revision ${shortId(workspace.override_revision)}`
                  : "inherited only"
              }
              title={
                workspace.saved_override.exists
                  ? workspace.override_revision
                  : undefined
              }
            />
            <SummaryItem
              label="Apply state"
              value={applyStateLabel(workspace)}
              tone={
                workspace.apply_state?.pending_status === "failed"
                  ? "critical"
                  : workspace.apply_state?.pending_status
                    ? "warning"
                    : "neutral"
              }
            />
            <SummaryItem
              label="Live evidence"
              value={liveComparisonLabel(liveEvidence)}
              tone={
                liveEvidence?.comparison === "different"
                  ? "warning"
                  : liveEvidence?.comparison === "match"
                    ? "ok"
                    : "neutral"
              }
            />
          </section>

          <details
            className="singleConfigLiveEvidence"
            aria-label="Saved desired runtime TOML"
          >
            <summary>
              <span>
                <strong>Saved desired TOML</strong>
                <small>
                  Read-only effective runtime config used for live comparison;
                  this is not the VPS override editor.
                </small>
              </span>
            </summary>
            <pre>{workspace.desired_toml}</pre>
          </details>

          {liveEvidence ? (
            <details className="singleConfigLiveEvidence">
              <summary>
                <span>
                  <strong>Live ConfigRead</strong>
                  <small>
                    {liveComparisonLabel(liveEvidence)} against saved desired
                    {draftDirty ? " · Draft will change saved desired" : ""}
                  </small>
                </span>
                <button
                  className="secondaryAction compactAction"
                  onClick={(event) => {
                    event.preventDefault();
                    onOpenJobDetails(liveEvidence.jobId);
                  }}
                  type="button"
                >
                  Open job {shortId(liveEvidence.jobId)}
                </button>
              </summary>
              <pre>{JSON.stringify(liveEvidence.runtimeConfig, null, 2)}</pre>
            </details>
          ) : null}

          <section
            className={`singleConfigStickyReview ${draftDirty ? "dirty" : ""}`}
            aria-label="VPS config sticky review"
          >
            <div>
              <strong>
                {previewDeletesSavedOverride || resetRequested
                  ? "Delete the saved VPS override"
                  : preview
                    ? preview.recovery_sync_required
                      ? "Repair saved override and resync"
                      : preview.storage_only
                        ? "Stored TOML only"
                        : `${desiredChangeCount} reviewed ${desiredChangeCount === 1 ? "change" : "changes"}`
                    : draftDirty
                      ? "Draft will change saved desired"
                      : "No draft changes"}
              </strong>
              <span>
                {preview
                  ? previewDeletesSavedOverride
                    ? "The reviewed replacement removes the saved override; values return to their inherited owners."
                    : preview.recovery_sync_required
                      ? "The stored override is invalid; saving this reviewed replacement forces an authoritative runtime sync."
                      : preview.storage_only
                        ? "Runtime values stay the same; only the exact replacement TOML changes."
                        : "Server diff is frozen until the draft changes."
                  : draftDirty
                    ? "Review creates the exact server diff before privilege confirmation."
                    : "Edit a field, list, or the replacement TOML to begin."}
              </span>
            </div>
            <div className="buttonCluster">
              {draftDirty ? (
                <button
                  className="secondaryAction compactAction"
                  onClick={resetDraft}
                  type="button"
                >
                  <RotateCcw size={15} />
                  <span>Discard draft</span>
                </button>
              ) : null}
              {!preview ? (
                <button
                  className="primaryAction compactAction"
                  disabled={pending || !draftDirty || Boolean(advancedError)}
                  onClick={() => void reviewChanges()}
                  type="button"
                >
                  <FileSliders size={15} />
                  <span>Review changes</span>
                </button>
              ) : !privilegeMaterial ? (
                <button
                  className="primaryAction compactAction"
                  onClick={onOpenPrivilegeUnlock}
                  type="button"
                >
                  <LockKeyhole size={15} />
                  <span>Unlock to apply</span>
                </button>
              ) : (
                <button
                  className="primaryAction compactAction"
                  disabled={pending}
                  onClick={() => void prepareApply()}
                  type="button"
                >
                  <FileSliders size={15} />
                  <span>
                    {previewDeletesSavedOverride
                      ? "Delete reviewed"
                      : "Apply reviewed"}
                  </span>
                </button>
              )}
            </div>
          </section>

          <section className="singleConfigEditorShell">
            <header className="singleConfigEditorHeader">
              <div>
                <strong>Desired runtime hierarchy</strong>
                <span>
                  Values not set by this VPS override stay inherited from their
                  named owner.
                </span>
              </div>
              <div
                className="singleConfigModeTabs"
                role="tablist"
                aria-label="VPS config editor mode"
              >
                <button
                  aria-selected={editorMode === "tree"}
                  className={editorMode === "tree" ? "active" : ""}
                  onClick={() => selectEditorMode("tree")}
                  role="tab"
                  type="button"
                >
                  <FolderTree size={15} /> Tree
                </button>
                <button
                  aria-selected={editorMode === "advanced"}
                  className={editorMode === "advanced" ? "active" : ""}
                  onClick={() => selectEditorMode("advanced")}
                  role="tab"
                  type="button"
                >
                  <Code2 size={15} /> Advanced
                </button>
              </div>
            </header>

            {editorMode === "tree" ? (
              <div className="singleConfigTreeMode" role="tabpanel">
                <div className="singleConfigTreeTools">
                  <label>
                    <Search size={15} />
                    <input
                      aria-label="Search VPS runtime config fields"
                      onChange={(event) => setSearch(event.target.value)}
                      placeholder="Search fields, paths, sources, or owners"
                      type="search"
                      value={search}
                    />
                  </label>
                  <button
                    aria-label={
                      normalizedSearch
                        ? "Search expands matching config sections"
                        : allContainersExpanded
                          ? "Collapse all config sections"
                          : "Expand all config sections"
                    }
                    className="singleConfigExpandToggle"
                    disabled={Boolean(normalizedSearch)}
                    onClick={() =>
                      setExpanded(
                        allContainersExpanded
                          ? new Set()
                          : new Set(allContainerPointers),
                      )
                    }
                    title={
                      normalizedSearch
                        ? "Clear the search to change remembered expansion"
                        : allContainersExpanded
                          ? "Collapse all config sections"
                          : "Expand all config sections"
                    }
                    type="button"
                  >
                    <ChevronsUpDown aria-hidden="true" size={15} />
                  </button>
                </div>
                {treePaused ? (
                  <div className="singleConfigTreePaused" role="alert">
                    <Code2 size={18} />
                    <span>
                      <strong>Tree paused</strong>
                      Advanced TOML is preserved exactly and does not parse:{" "}
                      {advancedError}
                    </span>
                    <button
                      className="secondaryAction compactAction"
                      onClick={() => setEditorMode("advanced")}
                      type="button"
                    >
                      Repair TOML
                    </button>
                  </div>
                ) : (
                  <ConfigTree
                    desired={draftDesired}
                    draftOverride={effectiveDraftOverride}
                    expanded={expanded}
                    inherited={workspace.inherited}
                    normalizedSearch={normalizedSearch}
                    onReset={resetField}
                    onSet={updateStructured}
                    onToggle={(pointer) =>
                      setExpanded((current) => toggleSetValue(current, pointer))
                    }
                    onUseInherited={useInherited}
                    provenance={workspace.provenance}
                    savedOverride={savedParsed}
                    schema={workspace.field_schema}
                  />
                )}
              </div>
            ) : (
              <div className="singleConfigAdvancedMode" role="tabpanel">
                <div className="singleConfigAdvancedNotice">
                  <div>
                    <strong>Complete VPS override replacement TOML</strong>
                    <span>
                      This is the entire sparse override. Removing a key means
                      Use inherited; an empty list remains explicitly empty.
                    </span>
                  </div>
                  <button
                    className="secondaryAction compactAction"
                    disabled={!workspace.saved_override.exists}
                    onClick={requestOverrideReset}
                    type="button"
                  >
                    <Trash2 size={15} />
                    <span>Delete override</span>
                  </button>
                </div>
                <textarea
                  aria-label="VPS replacement override TOML"
                  aria-invalid={advancedError ? "true" : undefined}
                  onBlur={() => syncAdvancedToTree(false)}
                  onChange={(event) => {
                    const nextToml = event.target.value;
                    setAdvancedToml(nextToml);
                    setCandidateMode("toml");
                    setResetRequested(false);
                    const parsed = parseTomlDocument(nextToml);
                    setAdvancedError(
                      parsed.ok
                        ? validateAdvancedOverride(
                            parsed.value,
                            workspace.field_schema,
                          )
                        : parsed.error,
                    );
                    invalidateReview();
                  }}
                  rows={26}
                  spellCheck={false}
                  value={advancedToml}
                />
                <div className="singleConfigAdvancedFooter">
                  <ActionFeedback message={advancedError} tone="danger" />
                  <button
                    className="secondaryAction compactAction"
                    onClick={() => syncAdvancedToTree(true)}
                    type="button"
                  >
                    <FolderTree size={15} />
                    <span>Apply to tree</span>
                  </button>
                </div>
              </div>
            )}
          </section>

          <section className="singleConfigReason">
            <label>
              <span>
                Change reason <small>optional</small>
              </span>
              <input
                aria-label="VPS runtime config change reason"
                maxLength={512}
                onChange={(event) => {
                  setReason(event.target.value);
                  invalidateReview();
                }}
                placeholder="Why this VPS needs a different runtime value"
                value={reason}
              />
            </label>
          </section>

          {preview ? (
            <OverridePreview
              deletesSavedOverride={previewDeletesSavedOverride}
              preview={preview}
            />
          ) : null}
        </>
      )}

      <ConfirmationPrompt
        confirmLabel={
          reviewedDeletesSavedOverride
            ? "Delete VPS override"
            : "Apply VPS override"
        }
        detail={
          reviewedDeletesSavedOverride
            ? `Delete the reviewed saved override from ${selectedAgent?.display_name ?? "one VPS"}.`
            : `Apply the reviewed replacement override to ${selectedAgent?.display_name ?? "one VPS"}.`
        }
        error={actionError}
        expiresAtUnix={reviewed?.privilegeAssertion.expires_unix}
        items={[
          {
            label: "VPS",
            value: selectedAgent?.display_name ?? "-",
            title: selectedAgent?.id,
          },
          {
            label: "Effect",
            value: reviewedDeletesSavedOverride
              ? "Delete saved override"
              : reviewed?.preview.recovery_sync_required
                ? "Repair and runtime resync"
                : reviewed?.preview.storage_only
                  ? "Stored TOML only"
                  : `${reviewed?.preview.changes.length ?? 0} runtime changes`,
          },
          {
            label: "Revision",
            value: reviewed ? shortId(reviewed.preview.override_revision) : "-",
            title: reviewed?.preview.override_revision,
          },
          {
            label: "Reviewed base",
            value: reviewed ? shortId(reviewed.preview.desired_hash) : "-",
            title: reviewed?.preview.desired_hash,
          },
          {
            label: "Preview",
            value: reviewed ? shortId(reviewed.preview.preview_hash) : "-",
            title: reviewed?.preview.preview_hash,
          },
        ]}
        onCancel={() => {
          setConfirmOpen(false);
          setReviewed(null);
        }}
        onConfirm={() => void applyReviewed()}
        open={confirmOpen}
        pending={pending}
        title={
          reviewedDeletesSavedOverride
            ? "Confirm VPS override deletion"
            : "Confirm VPS runtime override"
        }
        tone={reviewedDeletesSavedOverride ? "danger" : "normal"}
      />
    </div>
  );
}

function SummaryItem({
  label,
  title,
  tone = "neutral",
  value,
}: {
  label: string;
  title?: string;
  tone?: "critical" | "warning" | "ok" | "info" | "neutral";
  value: string;
}) {
  return (
    <span title={title}>
      <small>{label}</small>
      <ConsoleStatusBadge tone={tone}>{value}</ConsoleStatusBadge>
    </span>
  );
}

function ConfigTree({
  desired,
  draftOverride,
  expanded,
  inherited,
  normalizedSearch,
  onReset,
  onSet,
  onToggle,
  onUseInherited,
  provenance,
  savedOverride,
  schema,
}: {
  desired: JsonValue;
  draftOverride: JsonValue;
  expanded: Set<string>;
  inherited: JsonValue;
  normalizedSearch: string;
  onReset: (path: string[]) => void;
  onSet: (path: string[], value: JsonValue) => void;
  onToggle: (pointer: string) => void;
  onUseInherited: (path: string[]) => void;
  provenance: RuntimeConfigProvenanceRecord[];
  savedOverride: JsonValue;
  schema: RuntimeConfigFieldSchemaRecord[];
}) {
  const schemaByPointer = new Map(
    schema.map((row) => [normalizePointer(row.pointer, row.path), row]),
  );
  const provenanceByPointer = new Map(
    provenance.map((row) => [normalizePointer(row.pointer, row.path), row]),
  );
  const root = asObject(desired) ?? {};
  const keys = directChildKeys([], root, schema);
  const visibleKeys = keys.filter((key) =>
    nodeMatchesSearch(
      [key],
      root[key] ?? null,
      normalizedSearch,
      schemaByPointer,
      provenanceByPointer,
    ),
  );
  if (visibleKeys.length === 0) {
    return (
      <div className="singleConfigTreeEmpty">
        No runtime config fields match this search.
      </div>
    );
  }
  return (
    <div
      className="singleConfigTree"
      role="list"
      aria-label="Desired VPS runtime configuration"
    >
      {visibleKeys.map((key) => (
        <ConfigTreeNode
          desired={root[key] ?? null}
          draftOverride={draftOverride}
          expanded={expanded}
          inherited={getAtPath(inherited, [key]) as JsonValue | undefined}
          key={key}
          normalizedSearch={normalizedSearch}
          onReset={onReset}
          onSet={onSet}
          onToggle={onToggle}
          onUseInherited={onUseInherited}
          path={[key]}
          provenanceByPointer={provenanceByPointer}
          savedOverride={savedOverride}
          schema={schema}
          schemaByPointer={schemaByPointer}
        />
      ))}
    </div>
  );
}

function ConfigTreeNode({
  desired,
  draftOverride,
  expanded,
  inherited,
  normalizedSearch,
  onReset,
  onSet,
  onToggle,
  onUseInherited,
  path,
  provenanceByPointer,
  savedOverride,
  schema,
  schemaByPointer,
}: {
  desired: JsonValue;
  draftOverride: JsonValue;
  expanded: Set<string>;
  inherited: JsonValue | undefined;
  normalizedSearch: string;
  onReset: (path: string[]) => void;
  onSet: (path: string[], value: JsonValue) => void;
  onToggle: (pointer: string) => void;
  onUseInherited: (path: string[]) => void;
  path: string[];
  provenanceByPointer: Map<string, RuntimeConfigProvenanceRecord>;
  savedOverride: JsonValue;
  schema: RuntimeConfigFieldSchemaRecord[];
  schemaByPointer: Map<string, RuntimeConfigFieldSchemaRecord>;
}) {
  const pointer = pathToPointer(path);
  const field = schemaByPointer.get(pointer);
  const provenance = nearestProvenance(pointer, provenanceByPointer);
  const explicit = hasAtPath(draftOverride, path);
  const savedExplicit = hasAtPath(savedOverride, path);
  const changed =
    explicit !== savedExplicit ||
    !jsonEqual(
      explicit ? getAtPath(draftOverride, path) : null,
      savedExplicit ? getAtPath(savedOverride, path) : null,
    );
  const arrayContainer =
    Array.isArray(desired) || field?.value_type === "array";
  const childCount = Object.keys(asObject(desired) ?? {}).length;
  const container =
    isObject(desired) ||
    arrayContainer ||
    field?.value_type === "object" ||
    field?.collection;
  const locked = Boolean(
    provenance?.locked || (!container && field?.editable === false),
  );
  const open = normalizedSearch ? true : expanded.has(pointer);
  const label = field?.label || humanizeKey(path[path.length - 1]);
  const owner = provenance?.owner ?? field?.owner;
  const ownerLink = provenance?.owner_link ?? field?.owner_link;
  const sourceLabel =
    explicit && !provenance?.shadowed_override
      ? "VPS override"
      : humanizeSource(provenance?.source ?? "inherited");
  const canRemoveShadowedOverride = Boolean(
    locked && explicit && provenance?.shadowed_override,
  );
  const containerCountLabel = arrayContainer
    ? explicit && (!Array.isArray(desired) || desired.length === 0)
      ? "[] explicit"
      : `${Array.isArray(desired) ? desired.length : 0} ${Array.isArray(desired) && desired.length === 1 ? "item" : "items"}`
    : `${childCount} ${childCount === 1 ? "field" : "fields"}`;

  if (!container) {
    return (
      <div
        className={`singleConfigField ${changed ? "changed" : ""} ${locked ? "locked" : ""}`}
        role="listitem"
      >
        <FieldIdentity
          changed={changed}
          label={label}
          locked={locked}
          owner={owner}
          ownerLink={ownerLink}
          path={field?.path || path.join(".")}
          shadowed={Boolean(provenance?.shadowed_override)}
          source={sourceLabel}
        />
        <div className="singleConfigFieldEditor">
          <ScalarControl
            disabled={locked}
            field={field}
            label={label}
            onChange={(value) => onSet(path, value)}
            value={desired}
          />
          {!locked || canRemoveShadowedOverride ? (
            <FieldActions
              changed={changed}
              explicit={explicit}
              onReset={() => onReset(path)}
              onUseInherited={() => onUseInherited(path)}
            />
          ) : null}
        </div>
      </div>
    );
  }

  return (
    <section
      className={`singleConfigBranch ${changed ? "changed" : ""}`}
      role="listitem"
    >
      <header className="singleConfigBranchHeader">
        <button
          aria-expanded={open}
          aria-label={`${open ? "Collapse" : "Expand"} ${label}`}
          className="singleConfigDisclosure"
          disabled={Boolean(normalizedSearch)}
          onClick={() => onToggle(pointer)}
          title={
            normalizedSearch
              ? "Matching sections stay expanded while searching"
              : undefined
          }
          type="button"
        >
          {open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
        </button>
        <FieldIdentity
          changed={changed}
          count={containerCountLabel}
          label={label}
          locked={locked}
          owner={owner}
          ownerLink={ownerLink}
          path={field?.path || path.join(".")}
          shadowed={Boolean(provenance?.shadowed_override)}
          source={sourceLabel}
        />
        {!locked || canRemoveShadowedOverride ? (
          <FieldActions
            changed={changed}
            explicit={explicit}
            onReset={() => onReset(path)}
            onUseInherited={() => onUseInherited(path)}
          />
        ) : null}
      </header>
      {open ? (
        <div className="singleConfigBranchBody" role="list">
          {arrayContainer ? (
            <ArrayEditor
              disabled={locked}
              field={field}
              onChange={(value) => onSet(path, value)}
              value={Array.isArray(desired) ? desired : []}
            />
          ) : (
            <ObjectEditor
              allowAdd={field?.control === "map"}
              desired={asObject(desired) ?? {}}
              disabled={locked}
              draftOverride={draftOverride}
              expanded={expanded}
              inherited={asObject(inherited) ?? {}}
              normalizedSearch={normalizedSearch}
              onReset={onReset}
              onSet={onSet}
              onToggle={onToggle}
              onUseInherited={onUseInherited}
              path={path}
              provenanceByPointer={provenanceByPointer}
              savedOverride={savedOverride}
              schema={schema}
              schemaByPointer={schemaByPointer}
            />
          )}
        </div>
      ) : null}
    </section>
  );
}

function ObjectEditor({
  allowAdd,
  desired,
  disabled,
  draftOverride,
  expanded,
  inherited,
  normalizedSearch,
  onReset,
  onSet,
  onToggle,
  onUseInherited,
  path,
  provenanceByPointer,
  savedOverride,
  schema,
  schemaByPointer,
}: {
  allowAdd: boolean;
  desired: Record<string, JsonValue>;
  disabled: boolean;
  draftOverride: JsonValue;
  expanded: Set<string>;
  inherited: Record<string, JsonValue>;
  normalizedSearch: string;
  onReset: (path: string[]) => void;
  onSet: (path: string[], value: JsonValue) => void;
  onToggle: (pointer: string) => void;
  onUseInherited: (path: string[]) => void;
  path: string[];
  provenanceByPointer: Map<string, RuntimeConfigProvenanceRecord>;
  savedOverride: JsonValue;
  schema: RuntimeConfigFieldSchemaRecord[];
  schemaByPointer: Map<string, RuntimeConfigFieldSchemaRecord>;
}) {
  const [adding, setAdding] = useState(false);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");
  const [addError, setAddError] = useState<string | null>(null);
  const keys = directChildKeys(path, desired, schema).filter((key) =>
    nodeMatchesSearch(
      [...path, key],
      desired[key] ?? null,
      normalizedSearch,
      schemaByPointer,
      provenanceByPointer,
    ),
  );

  function addEntry() {
    const key = newKey.trim();
    if (!key || key.includes(".")) {
      setAddError("Enter one field name without dots");
      return;
    }
    let value: JsonValue = newValue;
    if (newValue.trim()) {
      try {
        value = JSON.parse(newValue) as JsonValue;
      } catch {
        value = newValue;
      }
    }
    onSet([...path, key], value);
    setNewKey("");
    setNewValue("");
    setAddError(null);
    setAdding(false);
  }

  return (
    <>
      {keys.map((key) => (
        <ConfigTreeNode
          desired={desired[key] ?? null}
          draftOverride={draftOverride}
          expanded={expanded}
          inherited={inherited[key]}
          key={key}
          normalizedSearch={normalizedSearch}
          onReset={onReset}
          onSet={onSet}
          onToggle={onToggle}
          onUseInherited={onUseInherited}
          path={[...path, key]}
          provenanceByPointer={provenanceByPointer}
          savedOverride={savedOverride}
          schema={schema}
          schemaByPointer={schemaByPointer}
        />
      ))}
      {!disabled && allowAdd ? (
        <div className="singleConfigAddField">
          {adding ? (
            <>
              <input
                aria-label={`New field name under ${path.join(".")}`}
                onChange={(event) => setNewKey(event.target.value)}
                placeholder="field_name"
                value={newKey}
              />
              <input
                aria-label={`New field value under ${path.join(".")}`}
                onChange={(event) => setNewValue(event.target.value)}
                placeholder='value or JSON, e.g. "text", 5, []'
                value={newValue}
              />
              <button
                className="primaryAction compactAction"
                onClick={addEntry}
                type="button"
              >
                Add
              </button>
              <button
                className="secondaryAction compactAction"
                onClick={() => setAdding(false)}
                type="button"
              >
                Cancel
              </button>
              {addError ? <small role="alert">{addError}</small> : null}
            </>
          ) : (
            <button
              className="secondaryAction compactAction"
              onClick={() => setAdding(true)}
              type="button"
            >
              <Plus size={14} /> Add field
            </button>
          )}
        </div>
      ) : null}
    </>
  );
}

function FieldIdentity({
  changed,
  count,
  label,
  locked,
  owner,
  ownerLink,
  path,
  shadowed,
  source,
}: {
  changed: boolean;
  count?: string;
  label: string;
  locked: boolean;
  owner: string | null | undefined;
  ownerLink: string | null | undefined;
  path: string;
  shadowed: boolean;
  source: string;
}) {
  const ownerLabel = owner ? humanizeSource(owner) : "Owner";
  return (
    <div className="singleConfigFieldIdentity">
      <span>
        <strong>{label}</strong>
        {changed ? <em>Will change</em> : null}
        {locked ? (
          <em className="locked">
            <LockKeyhole size={11} /> Locked
          </em>
        ) : null}
      </span>
      <span className="singleConfigFieldDetails">
        <small>{path}</small>
        {count ? (
          <span className="singleConfigBranchCount">{count}</span>
        ) : null}
        <span className="singleConfigFieldMeta">
          <span>{source}</span>
          {ownerLink ? (
            <a href={ownerRouteHref(ownerLink)} title={`Open ${ownerLabel}`}>
              {ownerLabel} <ExternalLink size={11} />
            </a>
          ) : owner ? (
            <span>{ownerLabel}</span>
          ) : null}
          {shadowed ? <span>Override shadowed</span> : null}
        </span>
      </span>
    </div>
  );
}

function FieldActions({
  changed,
  explicit,
  onReset,
  onUseInherited,
}: {
  changed: boolean;
  explicit: boolean;
  onReset: () => void;
  onUseInherited: () => void;
}) {
  if (!changed && !explicit) return null;

  return (
    <div className="singleConfigFieldActions">
      {changed ? (
        <button
          className="secondaryAction compactAction"
          onClick={onReset}
          title="Restore this field to its saved override state"
          type="button"
        >
          <RotateCcw size={13} /> Reset
        </button>
      ) : null}
      {explicit ? (
        <button
          aria-label="Use inherited"
          className="secondaryAction compactAction"
          onClick={onUseInherited}
          title="Remove this field from the VPS override"
          type="button"
        >
          Inherit
        </button>
      ) : null}
    </div>
  );
}

function ScalarControl({
  disabled,
  field,
  label,
  onChange,
  value,
}: {
  disabled: boolean;
  field: RuntimeConfigFieldSchemaRecord | undefined;
  label: string;
  onChange: (value: JsonValue) => void;
  value: JsonValue;
}) {
  const enumValues = field?.enum_values ?? [];
  if (field?.control === "toggle" || typeof value === "boolean") {
    return (
      <label className="singleConfigToggle">
        <input
          aria-label={label}
          checked={value === true}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked)}
          type="checkbox"
        />
        <span>{value === true ? "Enabled" : "Disabled"}</span>
      </label>
    );
  }
  if (enumValues.length > 0) {
    return (
      <select
        aria-label={label}
        disabled={disabled}
        onChange={(event) =>
          onChange(parseScalarForField(event.target.value, field))
        }
        value={scalarInputValue(value)}
      >
        {enumValues.map((entry) => (
          <option key={JSON.stringify(entry)} value={scalarInputValue(entry)}>
            {formatJsonValue(entry)}
          </option>
        ))}
      </select>
    );
  }
  return (
    <input
      aria-label={label}
      disabled={disabled}
      onChange={(event) =>
        onChange(parseScalarForField(event.target.value, field))
      }
      step={
        field?.value_type === "integer"
          ? 1
          : field?.value_type === "number"
            ? "any"
            : undefined
      }
      type={
        field?.control === "number" || typeof value === "number"
          ? "number"
          : "text"
      }
      value={scalarInputValue(value)}
    />
  );
}

function ArrayEditor({
  disabled,
  field,
  onChange,
  value,
}: {
  disabled: boolean;
  field: RuntimeConfigFieldSchemaRecord | undefined;
  onChange: (value: JsonValue[]) => void;
  value: JsonValue[];
}) {
  const [newItem, setNewItem] = useState("");
  const objectItems = field?.control === "object_list" || value.some(isObject);
  function update(index: number, next: JsonValue) {
    onChange(
      value.map((entry, entryIndex) => (entryIndex === index ? next : entry)),
    );
  }
  function move(index: number, direction: -1 | 1) {
    const target = index + direction;
    if (target < 0 || target >= value.length) return;
    const next = [...value];
    [next[index], next[target]] = [next[target], next[index]];
    onChange(next);
  }
  function add() {
    const trimmed = newItem.trim();
    let parsed: JsonValue =
      field?.control === "text_list" ? newItem : objectItems ? {} : trimmed;
    if (trimmed && field?.control !== "text_list") {
      try {
        parsed = JSON.parse(trimmed) as JsonValue;
      } catch {
        parsed = trimmed;
      }
    }
    onChange([...value, parsed]);
    setNewItem("");
  }
  return (
    <div className="singleConfigArrayEditor">
      {value.length === 0 ? (
        <span className="singleConfigEmptyArray">Explicit empty list []</span>
      ) : null}
      {value.map((entry, index) => (
        <div className="singleConfigArrayItem" key={index}>
          <span className="singleConfigArrayIndex">{index + 1}</span>
          {isObject(entry) || Array.isArray(entry) ? (
            <textarea
              aria-label={`Array item ${index + 1}`}
              disabled={disabled}
              onChange={(event) => {
                try {
                  update(index, JSON.parse(event.target.value) as JsonValue);
                } catch {
                  /* Preserve the last valid item until the textarea is valid. */
                }
              }}
              rows={Math.min(
                8,
                Math.max(2, JSON.stringify(entry, null, 2).split("\n").length),
              )}
              value={JSON.stringify(entry, null, 2)}
            />
          ) : (
            <ScalarControl
              disabled={disabled}
              field={undefined}
              label={`Array item ${index + 1}`}
              onChange={(next) => update(index, next)}
              value={entry}
            />
          )}
          {!disabled ? (
            <div className="singleConfigArrayActions">
              <button
                aria-label={`Move item ${index + 1} up`}
                className="iconButton"
                disabled={index === 0}
                onClick={() => move(index, -1)}
                type="button"
              >
                <ArrowUp size={14} />
              </button>
              <button
                aria-label={`Move item ${index + 1} down`}
                className="iconButton"
                disabled={index === value.length - 1}
                onClick={() => move(index, 1)}
                type="button"
              >
                <ArrowDown size={14} />
              </button>
              <button
                aria-label={`Delete item ${index + 1}`}
                className="iconButton danger"
                onClick={() =>
                  onChange(
                    value.filter((_, entryIndex) => entryIndex !== index),
                  )
                }
                type="button"
              >
                <Trash2 size={14} />
              </button>
            </div>
          ) : null}
        </div>
      ))}
      {!disabled ? (
        <div className="singleConfigArrayAdd">
          <input
            aria-label="New array item"
            onChange={(event) => setNewItem(event.target.value)}
            placeholder={objectItems ? '{"key":"value"}' : "New value"}
            value={newItem}
          />
          <button
            className="secondaryAction compactAction"
            onClick={add}
            type="button"
          >
            <Plus size={14} /> Add item
          </button>
          <button
            className="secondaryAction compactAction"
            disabled={value.length === 0}
            onClick={() => onChange([])}
            type="button"
          >
            Set []
          </button>
        </div>
      ) : null}
    </div>
  );
}

function OverridePreview({
  deletesSavedOverride,
  preview,
}: {
  deletesSavedOverride: boolean;
  preview: RuntimeConfigOverridePreview;
}) {
  return (
    <section
      className="singleConfigPreview"
      aria-label="Reviewed VPS config changes"
    >
      <header>
        <div>
          <strong>
            {deletesSavedOverride
              ? "Saved override deletion"
              : preview.recovery_sync_required
                ? "Stored override recovery"
                : preview.storage_only
                  ? "Storage-only review"
                  : "Server-reviewed runtime changes"}
          </strong>
          <span>
            {deletesSavedOverride
              ? "The reviewed replacement removes the saved VPS override and returns its values to inherited ownership."
              : preview.recovery_sync_required
                ? "The current stored override is invalid. This reviewed replacement repairs it and forces an authoritative runtime sync."
                : preview.storage_only
                  ? "The exact replacement TOML changes, but the resulting desired runtime values are identical."
                  : `${preview.changes.length} value ${preview.changes.length === 1 ? "change" : "changes"} will update saved desired config.`}
          </span>
        </div>
        <ConsoleStatusBadge
          tone={
            deletesSavedOverride
              ? "critical"
              : preview.recovery_sync_required || !preview.storage_only
                ? "warning"
                : "info"
          }
        >
          {deletesSavedOverride
            ? "delete override"
            : preview.recovery_sync_required
              ? "recovery sync"
              : preview.storage_only
                ? "stored text"
                : "runtime change"}
        </ConsoleStatusBadge>
      </header>
      {preview.changes.length > 0 ? (
        <div className="singleConfigChangeList">
          {preview.changes.map((change) => (
            <div key={`${change.pointer}-${change.kind}`}>
              <span>
                <strong>{change.path}</strong>
                <small>{change.kind}</small>
              </span>
              <code>{formatJsonValue(change.before)}</code>
              <ChevronRight size={14} />
              <code>{formatJsonValue(change.after)}</code>
            </div>
          ))}
        </div>
      ) : null}
      <details>
        <summary>Canonical replacement TOML</summary>
        <pre>
          {deletesSavedOverride
            ? "No replacement TOML; the saved override will be removed."
            : preview.canonical_toml}
        </pre>
      </details>
    </section>
  );
}

function dispatchWarning(
  response: ApplyRuntimeConfigOverrideResponse,
  prefix: string,
): string | null {
  const failures = response.sync.filter((entry) => entry.status !== "queued");
  if (failures.length === 0 && response.sync.length > 0) return null;
  if (failures.length === 0) return `${prefix}; no runtime sync was required`;
  return `${prefix}; runtime sync was not queued for ${failures
    .map(
      (entry) =>
        `${entry.client_id}: ${dispatchFailureReason(entry.error, entry.status, "Runtime sync")}`,
    )
    .join("; ")}`;
}

function applyStateLabel(workspace: RuntimeConfigClientWorkspace): string {
  const state = workspace.apply_state;
  if (!state) return "not reported";
  if (state.pending_status) return state.pending_status.replace(/_/g, " ");
  if (state.applied_content_hash === workspace.desired_content_hash)
    return "applied";
  return "saved / not confirmed";
}

function liveComparisonLabel(evidence: LiveConfigEvidence | null): string {
  if (!evidence) return "not refreshed";
  if (evidence.comparison === "match") return "matches saved desired";
  if (evidence.comparison === "different") return "differs from saved desired";
  return "comparison unavailable";
}

function compareLiveDesired(
  live: JsonValue,
  desired: JsonValue,
): LiveConfigEvidence["comparison"] {
  const subset = pickDesiredShape(live, desired);
  return jsonEqual(subset, desired) ? "match" : "different";
}

function pickDesiredShape(live: JsonValue, desired: JsonValue): JsonValue {
  if (Array.isArray(desired)) return live;
  if (!isObject(desired)) return live;
  const liveObject = asObject(live) ?? {};
  return Object.fromEntries(
    Object.entries(desired).map(([key, value]) => [
      key,
      pickDesiredShape(liveObject[key] ?? null, value),
    ]),
  );
}

function extractConfigRead(outputs: JobOutputRecord[]): JsonValue {
  for (const output of [...outputs].reverse()) {
    if (output.stream !== "status") continue;
    let value: { type?: string; runtime_config?: JsonValue };
    try {
      value = JSON.parse(base64ToText(output.data_base64)) as typeof value;
    } catch {
      continue;
    }
    if (value.type === "config_read" && isObject(value.runtime_config)) {
      return value.runtime_config;
    }
  }
  throw new Error(
    "Config read completed without structured runtime config output",
  );
}

function base64ToText(value: string): string {
  const binary = window.atob(value);
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

function parseTomlDocument(
  toml: string,
): { ok: true; value: JsonValue } | { ok: false; error: string } {
  try {
    const parsed = parse(toml || "") as TomlTable;
    return { ok: true, value: normalizeTomlValue(parsed) };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : "Invalid TOML",
    };
  }
}

function validateAdvancedOverride(
  value: JsonValue,
  schema: RuntimeConfigFieldSchemaRecord[],
): string | null {
  const byPointer = new Map(
    schema.map((field) => [normalizePointer(field.pointer, field.path), field]),
  );
  const leaves: string[][] = [];
  collectOverrideLeaves(value, [], leaves);
  for (const path of leaves) {
    const field = byPointer.get(pathToPointer(path));
    if (!field) return `Unknown runtime configuration field: ${path.join(".")}`;
    if (!field.editable)
      return `Locked runtime configuration field: ${field.path}`;
  }
  return null;
}

function collectOverrideLeaves(
  value: JsonValue,
  path: string[],
  output: string[][],
) {
  if (isObject(value)) {
    for (const [key, child] of Object.entries(value)) {
      collectOverrideLeaves(child, [...path, key], output);
    }
    return;
  }
  output.push(path);
}

function normalizeTomlValue(value: unknown): JsonValue {
  if (value instanceof Date) return value.toISOString();
  if (Array.isArray(value)) return value.map(normalizeTomlValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [
        key,
        normalizeTomlValue(entry),
      ]),
    );
  }
  if (
    value === null ||
    ["string", "number", "boolean"].includes(typeof value)
  ) {
    return value as JsonValue;
  }
  return String(value);
}

function stringifyOverride(value: JsonValue): string {
  const object = asObject(value) ?? {};
  try {
    return Object.keys(object).length ? stringify(object as TomlTable) : "";
  } catch {
    return JSON.stringify(object, null, 2);
  }
}

function deepMerge(inherited: JsonValue, override: JsonValue): JsonValue {
  if (!isObject(inherited) || !isObject(override)) return cloneJson(override);
  const next: Record<string, JsonValue> = { ...cloneJson(inherited) };
  for (const [key, value] of Object.entries(override)) {
    next[key] = key in next ? deepMerge(next[key], value) : cloneJson(value);
  }
  return next;
}

function setAtPath(
  root: Record<string, JsonValue>,
  path: string[],
  value: JsonValue,
): Record<string, JsonValue> {
  const next = cloneJson(root);
  let current = next;
  for (const part of path.slice(0, -1)) {
    const child = asObject(current[part]);
    current[part] = child ? cloneJson(child) : {};
    current = current[part] as Record<string, JsonValue>;
  }
  current[path[path.length - 1]] = cloneJson(value);
  return next;
}

function deleteAtPath(
  root: Record<string, JsonValue>,
  path: string[],
): Record<string, JsonValue> {
  const next = cloneJson(root);
  const parents: Array<[Record<string, JsonValue>, string]> = [];
  let current = next;
  for (const part of path.slice(0, -1)) {
    const child = asObject(current[part]);
    if (!child) return next;
    current[part] = cloneJson(child);
    parents.push([current, part]);
    current = current[part] as Record<string, JsonValue>;
  }
  delete current[path[path.length - 1]];
  for (const [parent, key] of parents.reverse()) {
    const child = asObject(parent[key]);
    if (child && Object.keys(child).length === 0) delete parent[key];
    else break;
  }
  return next;
}

function getAtPath(
  root: JsonValue | undefined,
  path: string[],
): JsonValue | undefined {
  let current = root;
  for (const part of path) {
    if (!isObject(current)) return undefined;
    current = current[part];
  }
  return current;
}

function hasAtPath(root: JsonValue | undefined, path: string[]): boolean {
  let current = root;
  for (const part of path) {
    if (
      !isObject(current) ||
      !Object.prototype.hasOwnProperty.call(current, part)
    )
      return false;
    current = current[part];
  }
  return true;
}

function directChildKeys(
  path: string[],
  desired: Record<string, JsonValue>,
  schema: RuntimeConfigFieldSchemaRecord[],
): string[] {
  const keys = new Set(Object.keys(desired));
  const prefix = pathToPointer(path);
  for (const row of schema) {
    const parts = pointerToPath(normalizePointer(row.pointer, row.path));
    if (
      parts.length === path.length + 1 &&
      path.every((part, index) => parts[index] === part)
    ) {
      keys.add(parts[parts.length - 1]);
    }
  }
  return [...keys].sort((left, right) =>
    left.localeCompare(right, undefined, { numeric: true }),
  );
}

function nodeMatchesSearch(
  path: string[],
  value: JsonValue,
  query: string,
  schema: Map<string, RuntimeConfigFieldSchemaRecord>,
  provenance: Map<string, RuntimeConfigProvenanceRecord>,
): boolean {
  if (!query) return true;
  const pointer = pathToPointer(path);
  const field = schema.get(pointer);
  const source = nearestProvenance(pointer, provenance);
  if (
    [path.join("."), field?.label, field?.owner, source?.source, source?.owner]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(query)
  )
    return true;
  if (isObject(value)) {
    return Object.entries(value).some(([key, child]) =>
      nodeMatchesSearch([...path, key], child, query, schema, provenance),
    );
  }
  if (Array.isArray(value))
    return JSON.stringify(value).toLowerCase().includes(query);
  return formatJsonValue(value).toLowerCase().includes(query);
}

function collectContainerPointers(
  value: JsonValue,
  path: string[] = [],
): string[] {
  const pointers: string[] = [];
  if (isObject(value)) {
    if (path.length) pointers.push(pathToPointer(path));
    for (const [key, child] of Object.entries(value)) {
      pointers.push(...collectContainerPointers(child, [...path, key]));
    }
  } else if (Array.isArray(value)) {
    pointers.push(pathToPointer(path));
  }
  return pointers;
}

function topLevelPointers(value: JsonValue): string[] {
  return Object.keys(asObject(value) ?? {}).map((key) => pathToPointer([key]));
}

function nearestProvenance(
  pointer: string,
  map: Map<string, RuntimeConfigProvenanceRecord>,
) {
  let current = pointer;
  while (current) {
    const found = map.get(current);
    if (found) return found;
    current = current.slice(0, current.lastIndexOf("/"));
  }
  return map.get("");
}

function normalizePointer(pointer: string, path: string): string {
  if (pointer?.startsWith("/")) return pointer;
  if (!path) return "";
  return pathToPointer(path.split(".").filter(Boolean));
}

function pathToPointer(path: string[]): string {
  return path.length
    ? `/${path.map((part) => part.replace(/~/g, "~0").replace(/\//g, "~1")).join("/")}`
    : "";
}

function pointerToPath(pointer: string): string[] {
  return pointer
    .split("/")
    .slice(1)
    .map((part) => part.replace(/~1/g, "/").replace(/~0/g, "~"));
}

function toggleSetValue(current: Set<string>, value: string): Set<string> {
  const next = new Set(current);
  if (next.has(value)) next.delete(value);
  else next.add(value);
  return next;
}

function parseScalarForField(
  value: string,
  field: RuntimeConfigFieldSchemaRecord | undefined,
): JsonValue {
  if (field?.value_type === "integer") {
    const parsed = Number.parseInt(value, 10);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  if (field?.value_type === "number") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return value;
}

function scalarInputValue(value: JsonValue): string {
  if (value === null) return "";
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : JSON.stringify(value);
}

function humanizeKey(value: string): string {
  return value
    .replace(/_/g, " ")
    .replace(/\b\w/g, (letter: string) => letter.toUpperCase());
}

function humanizeSource(value: string): string {
  return humanizeKey(value).replace(/\bVps\b/g, "VPS");
}

function formatJsonValue(value: JsonValue | undefined): string {
  if (value === undefined) return "unset";
  if (typeof value === "string") return value || '""';
  return JSON.stringify(value);
}

function asObject(
  value: JsonValue | null | undefined,
): Record<string, JsonValue> | null {
  return isObject(value) ? value : null;
}

function isObject(value: unknown): value is Record<string, JsonValue> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function cloneJson<T extends JsonValue>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function jsonEqual(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((entry, index) => jsonEqual(entry, right[index]))
    );
  }
  if (isObject(left) || isObject(right)) {
    if (!isObject(left) || !isObject(right)) return false;
    const leftKeys = Object.keys(left);
    const rightKeys = Object.keys(right);
    return (
      leftKeys.length === rightKeys.length &&
      leftKeys.every(
        (key) =>
          Object.prototype.hasOwnProperty.call(right, key) &&
          jsonEqual(left[key], right[key]),
      )
    );
  }
  return false;
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
    if (value) window.localStorage.setItem(key, value);
    else window.localStorage.removeItem(key);
  } catch {
    // Browser-local convenience must not block configuration work.
  }
}

function readExpansionState(): Set<string> | null {
  try {
    const stored = window.localStorage.getItem(EXPANSION_STORAGE_KEY);
    if (stored === null) return null;
    const value = JSON.parse(stored);
    if (!Array.isArray(value)) return null;
    return new Set(
      value.filter((entry): entry is string => typeof entry === "string"),
    );
  } catch {
    return null;
  }
}

function writeExpansionState(value: Set<string>) {
  try {
    window.localStorage.setItem(
      EXPANSION_STORAGE_KEY,
      JSON.stringify([...value].sort()),
    );
  } catch {
    // Expansion is a browser-local convenience only.
  }
}

function ownerRouteHref(ownerLink: string): string {
  if (ownerLink === "/config/presets") return "#/config/sources";
  if (ownerLink === "/network") return "#/network/overview";
  return ownerLink;
}
