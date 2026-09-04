import { RefreshCw, Target } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { apiGet, apiPost, buildListPath } from "../api";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { scrollIntoViewWithMotion } from "../motion";
import {
  agentsMatchingExpression,
  type AgentSearchContext,
  parseSearchExpression,
  VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE,
  vpsRuleSearchUnavailable,
} from "../searchExpression";
import { useVpsRuleSearchContext } from "../vpsRuleSearchContext";
import { buildScheduleTargetUpdatePrivilegeAssertion } from "../scheduleTargetMaintenance";
import type {
  AgentView,
  ArtifactCleanupPreviewRecord,
  BulkResolveManyRequest,
  BulkResolveManyResponse,
  BulkUpdateScheduleTargetsResponse,
  BulkUpdateMonitoringShareTargetsResponse,
  BulkUpdatePingTargetsResponse,
  MonitoringShareTargetChangeView,
  MonitoringShareView,
  PingTargetAssignmentChangeView,
  PingTargetView,
  ScheduleRecord,
  ServerJobRecord,
} from "../types";
import { formatCompactTime, shortId } from "../utils";
import type { PrivilegeMaterial } from "../privilege";
import { ServerJobsPanel } from "./jobs/ServerJobsPanel";

type MaintenanceTab = "selectors" | "artifacts" | "jobs";

type StaleSelectorRow = {
  canUpdate: boolean;
  frozenTargetIds: string[];
  id: string;
  kind: "Ping target" | "Schedule" | "Shared view";
  name: string;
  reason: string;
  resolvedTargetIds: string[] | null;
  resourceId: string;
  selectorExpression: string;
  source: MonitoringShareView | PingTargetView | ScheduleRecord;
  updatedAt: string;
};

type ScheduleTargetReview = {
  nextTargetIds: string[];
  schedule: ScheduleRecord;
  selectorExpression: string;
};

type SelectorUpdateReview = {
  pingPreview: BulkUpdatePingTargetsResponse | null;
  pingTargetIds: string[];
  schedules: ScheduleTargetReview[];
  selectedCount: number;
  sharePreview: BulkUpdateMonitoringShareTargetsResponse | null;
  shareIds: string[];
};

type Feedback = {
  message: string;
  tone: ActionFeedbackTone;
};

const SCHEDULE_TARGET_UPDATE_BATCH_LIMIT = 1_000;
const PING_TARGET_UPDATE_BATCH_LIMIT = 1_000;
const MONITORING_SHARE_UPDATE_BATCH_LIMIT = 1_000;

const maintenanceTabs: Array<{
  id: MaintenanceTab;
  label: string;
  route: string;
}> = [
  { id: "selectors", label: "Stale selectors", route: "maintenance:selectors" },
  {
    id: "artifacts",
    label: "Artifact cleanup",
    route: "maintenance:artifacts",
  },
  { id: "jobs", label: "Maintenance jobs", route: "maintenance:jobs" },
];

const SELECTOR_PAGE_SIZE = 1_000;
const MAX_SELECTOR_PAGES = 100;
const SCHEDULE_TARGET_PRIVILEGE_REQUIRED =
  "Privilege unlock is required to update schedule target snapshots.";

export function SystemMaintenancePanel({
  activeSubpage,
  agents,
  apiToken,
  jobs,
  jobsError,
  jobsLoading,
  onCancelJob,
  onCreateCleanupJob,
  onOpenPrivilegeUnlock,
  onPreviewCleanup,
  onRefreshJobs,
  onRefreshSchedules,
  onResolveManyTargets,
  onSelectSubpage,
  privilegeMaterial,
  requestsEnabled,
}: {
  activeSubpage: string;
  agents: AgentView[];
  apiToken: string;
  jobs: ServerJobRecord[];
  jobsError: string | null;
  jobsLoading: boolean;
  onCancelJob: (jobId: string) => Promise<ServerJobRecord>;
  onCreateCleanupJob: (
    expression: string,
    domains: string[],
    previewHash: string,
  ) => Promise<ServerJobRecord>;
  onOpenPrivilegeUnlock: () => void;
  onPreviewCleanup: (
    expression: string,
    domains: string[],
  ) => Promise<ArtifactCleanupPreviewRecord>;
  onRefreshJobs: () => void;
  onRefreshSchedules: () => Promise<void>;
  onResolveManyTargets: (
    request: BulkResolveManyRequest,
  ) => Promise<BulkResolveManyResponse>;
  onSelectSubpage: (subpage: string) => void;
  privilegeMaterial: PrivilegeMaterial | null;
  requestsEnabled: boolean;
}) {
  const activeTab = maintenanceTabFromSubpage(activeSubpage);

  return (
    <section className="workspace singleColumn systemMaintenanceWorkspace">
      <div className="fleetPanel maintenanceNavigationPanel">
        <div className="sectionHeader compactSectionHeader">
          <div>
            <h2>System maintenance</h2>
            <span>
              Repair saved target snapshots, review artifact cleanup, and
              inspect maintenance jobs.
            </span>
          </div>
        </div>
        <nav
          aria-label="System maintenance subpanels"
          className="subpanelTabs accessTabs maintenanceTabs"
        >
          {maintenanceTabs.map((tab) => (
            <button
              className={activeTab === tab.id ? "active" : ""}
              key={tab.id}
              onClick={() => onSelectSubpage(tab.route)}
              type="button"
            >
              {tab.label}
            </button>
          ))}
        </nav>
      </div>

      {activeTab === "selectors" ? (
        <StaleSelectorMaintenancePanel
          agents={agents}
          apiToken={apiToken}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          onRefreshSchedules={onRefreshSchedules}
          onResolveManyTargets={onResolveManyTargets}
          privilegeMaterial={privilegeMaterial}
          requestsEnabled={requestsEnabled}
        />
      ) : null}

      {activeTab !== "selectors" ? (
        <ServerJobsPanel
          error={jobsError}
          jobs={jobs}
          loading={jobsLoading}
          onCancelJob={onCancelJob}
          onCreateCleanupJob={onCreateCleanupJob}
          onPreviewCleanup={onPreviewCleanup}
          onRefresh={onRefreshJobs}
          section={activeTab === "artifacts" ? "cleanup" : "jobs"}
        />
      ) : null}
    </section>
  );
}

function StaleSelectorMaintenancePanel({
  agents,
  apiToken,
  onOpenPrivilegeUnlock,
  onRefreshSchedules,
  onResolveManyTargets,
  privilegeMaterial,
  requestsEnabled,
}: {
  agents: AgentView[];
  apiToken: string;
  onOpenPrivilegeUnlock: () => void;
  onRefreshSchedules: () => Promise<void>;
  onResolveManyTargets: (
    request: BulkResolveManyRequest,
  ) => Promise<BulkResolveManyResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
  requestsEnabled: boolean;
}) {
  const vpsRuleSearch = useVpsRuleSearchContext();
  const [schedules, setSchedules] = useState<ScheduleRecord[]>([]);
  const [pingTargets, setPingTargets] = useState<PingTargetView[]>([]);
  const [shares, setShares] = useState<MonitoringShareView[]>([]);
  const [scheduleLoadError, setScheduleLoadError] = useState<string | null>(
    null,
  );
  const [pingLoadError, setPingLoadError] = useState<string | null>(null);
  const [shareLoadError, setShareLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [review, setReview] = useState<SelectorUpdateReview | null>(null);
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  const previousFeedbackRef = useRef<string | null>(null);
  const loadedApiTokenRef = useRef<string | null>(null);
  const resourceLoadGenerationRef = useRef(0);
  const resourceApiTokenRef = useRef(apiToken);
  if (resourceApiTokenRef.current !== apiToken) {
    resourceApiTokenRef.current = apiToken;
    resourceLoadGenerationRef.current += 1;
  }

  const refreshResources = useCallback(async () => {
    const generation = ++resourceLoadGenerationRef.current;
    setLoading(true);
    setScheduleLoadError(null);
    setPingLoadError(null);
    setShareLoadError(null);
    const [scheduleResult, pingResult, shareResult] = await Promise.allSettled([
      loadAllSchedules(apiToken),
      apiGet<PingTargetView[]>("/api/v1/ping-targets", apiToken),
      loadAllMonitoringShares(apiToken),
    ]);
    if (
      resourceLoadGenerationRef.current !== generation ||
      resourceApiTokenRef.current !== apiToken
    ) {
      return;
    }
    if (scheduleResult.status === "fulfilled") {
      setSchedules(scheduleResult.value);
    } else {
      setSchedules([]);
      setScheduleLoadError(errorMessage(scheduleResult.reason));
    }
    if (pingResult.status === "fulfilled") {
      setPingTargets(pingResult.value);
    } else {
      setPingTargets([]);
      setPingLoadError(errorMessage(pingResult.reason));
    }
    if (shareResult.status === "fulfilled") {
      setShares(shareResult.value);
    } else {
      setShares([]);
      setShareLoadError(errorMessage(shareResult.reason));
    }
    setLoading(false);
  }, [apiToken]);

  useEffect(() => {
    if (!requestsEnabled) return;
    if (loadedApiTokenRef.current === apiToken) return;
    loadedApiTokenRef.current = apiToken;
    void refreshResources();
  }, [apiToken, refreshResources, requestsEnabled]);

  useEffect(() => {
    if (privilegeMaterial && error === SCHEDULE_TARGET_PRIVILEGE_REQUIRED) {
      setError(null);
    }
  }, [error, privilegeMaterial]);

  const rows = useMemo(
    () =>
      staleSelectorRows(schedules, pingTargets, shares, agents, vpsRuleSearch),
    [agents, pingTargets, schedules, shares, vpsRuleSearch],
  );
  const updateableRows = rows.filter((row) => row.canUpdate);
  const blockedRows = rows.length - updateableRows.length;
  const pageFeedback = [
    scheduleLoadError ? `Schedules: ${scheduleLoadError}` : null,
    pingLoadError ? `Ping targets: ${pingLoadError}` : null,
    shareLoadError ? `Shared views: ${shareLoadError}` : null,
  ]
    .filter(Boolean)
    .join(" ");
  const outcomeMessage = error ?? feedback?.message ?? null;
  const outcomeTone: ActionFeedbackTone = error
    ? "danger"
    : (feedback?.tone ?? "info");

  useEffect(() => {
    if (!outcomeMessage) {
      previousFeedbackRef.current = null;
      return;
    }
    if (previousFeedbackRef.current === outcomeMessage) {
      return;
    }
    previousFeedbackRef.current = outcomeMessage;
    const frame = window.requestAnimationFrame(() => {
      if (feedbackRef.current) {
        scrollIntoViewWithMotion(feedbackRef.current, { block: "nearest" });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [outcomeMessage]);

  const columns = useMemo<ConsoleDataGridColumn<StaleSelectorRow>[]>(
    () => [
      {
        id: "resource",
        header: "Resource",
        mobilePrimary: true,
        size: 220,
        minSize: 180,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.name}</strong>
            <small title={row.resourceId}>
              {row.kind} · {shortId(row.resourceId)}
            </small>
          </span>
        ),
        searchValue: (row) => `${row.kind} ${row.name} ${row.resourceId}`,
        sortValue: (row) => `${row.kind}:${row.name}`,
      },
      {
        id: "selector",
        header: "Saved selector",
        size: 270,
        minSize: 190,
        cell: (row) => (
          <code title={row.selectorExpression}>{row.selectorExpression}</code>
        ),
        searchValue: (row) => row.selectorExpression,
        sortValue: (row) => row.selectorExpression,
      },
      {
        id: "frozen",
        header: "Frozen targets",
        size: 145,
        minSize: 125,
        cell: (row) => (
          <span title={targetIdTitle(row.frozenTargetIds)}>
            {targetCountLabel(row.frozenTargetIds.length)}
          </span>
        ),
        searchValue: (row) => row.frozenTargetIds.join(" "),
        sortValue: (row) => row.frozenTargetIds.length,
      },
      {
        id: "current",
        header: "Current resolution",
        size: 170,
        minSize: 145,
        cell: (row) =>
          row.resolvedTargetIds === null
            ? "Unavailable"
            : targetDeltaLabel(row.frozenTargetIds, row.resolvedTargetIds),
        searchValue: (row) => row.resolvedTargetIds?.join(" ") ?? row.reason,
        sortValue: (row) => row.resolvedTargetIds?.length ?? -1,
      },
      {
        id: "state",
        header: "State",
        mobileState: true,
        size: 150,
        minSize: 130,
        cell: (row) => (
          <span title={row.reason}>
            <ConsoleStatusBadge tone={row.canUpdate ? "warning" : "critical"}>
              {row.canUpdate ? "Update available" : "Repair required"}
            </ConsoleStatusBadge>
          </span>
        ),
        searchValue: (row) => row.reason,
        sortValue: (row) => Number(row.canUpdate),
      },
      {
        id: "updated",
        header: "Saved",
        size: 150,
        minSize: 130,
        cell: (row) => formatCompactTime(row.updatedAt),
        searchValue: (row) => row.updatedAt,
        sortValue: (row) => row.updatedAt,
      },
    ],
    [],
  );

  async function reviewRows(selectedRows: StaleSelectorRow[]) {
    if (pending) return;
    const selected = selectedRows.filter((row) => row.canUpdate);
    if (selected.length === 0) {
      setError(
        "No selected stale selector can be updated until its saved definition is repaired.",
      );
      return;
    }
    const limitMessage = selectorUpdateBatchLimitMessage(selected);
    if (limitMessage) {
      setError(limitMessage);
      return;
    }
    const selectedSchedules = selected.filter(
      (row): row is StaleSelectorRow & { source: ScheduleRecord } =>
        row.kind === "Schedule",
    );
    const selectedShares = selected.filter(
      (row): row is StaleSelectorRow & { source: MonitoringShareView } =>
        row.kind === "Shared view",
    );
    if (selectedSchedules.length > 0 && !privilegeMaterial) {
      setError(SCHEDULE_TARGET_PRIVILEGE_REQUIRED);
      onOpenPrivilegeUnlock();
      return;
    }
    setPending(true);
    setError(null);
    setFeedback({
      message: "Resolving saved selectors for review",
      tone: "progress",
    });
    try {
      const selectors = Array.from(
        new Set(selectedSchedules.map((row) => row.selectorExpression)),
      );
      const resolution =
        selectors.length > 0
          ? await onResolveManyTargets({
              items: selectors.map((selector_expression) => ({
                selector_expression,
              })),
            })
          : { outcomes: [] };
      assertOrderedSelectorResolution(selectors, resolution);
      const resolvedBySelector = new Map(
        resolution.outcomes.map((outcome) => [
          outcome.selector_expression,
          uniqueSorted(outcome.target_client_ids),
        ]),
      );
      const scheduleUpdates: ScheduleTargetReview[] = [];
      for (const row of selectedSchedules) {
        const targetIds = resolvedBySelector.get(row.selectorExpression) ?? [];
        if (!sameStringSet(row.frozenTargetIds, targetIds)) {
          scheduleUpdates.push({
            nextTargetIds: targetIds,
            schedule: row.source,
            selectorExpression: row.selectorExpression,
          });
        }
      }

      const pingTargetIds = selected
        .filter((row) => row.kind === "Ping target")
        .map((row) => row.resourceId);
      const pingPreview =
        pingTargetIds.length > 0
          ? await apiPost<BulkUpdatePingTargetsResponse>(
              "/api/v1/ping-targets/update-targets",
              apiToken,
              { confirmed: false, target_ids: pingTargetIds },
            )
          : null;
      const pingChanges = pingPreview?.changes.filter(pingChangeHasDelta) ?? [];
      const shareIds = selectedShares.map((row) => row.resourceId);
      const sharePreview =
        shareIds.length > 0
          ? await apiPost<BulkUpdateMonitoringShareTargetsResponse>(
              "/api/v1/monitoring-shares/update-targets",
              apiToken,
              { confirmed: false, share_ids: shareIds },
            )
          : null;
      const shareChanges =
        sharePreview?.changes.filter(shareChangeHasDelta) ?? [];
      if (
        scheduleUpdates.length === 0 &&
        pingChanges.length === 0 &&
        shareChanges.length === 0
      ) {
        setFeedback({
          message:
            "Server resolution confirms every selected frozen target list is current.",
          tone: "info",
        });
        await refreshResources();
        return;
      }
      setReview({
        pingPreview: pingPreview
          ? { ...pingPreview, changes: pingChanges }
          : null,
        pingTargetIds,
        schedules: scheduleUpdates,
        selectedCount: selected.length,
        sharePreview: sharePreview
          ? { ...sharePreview, changes: shareChanges }
          : null,
        shareIds,
      });
      setFeedback(null);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setPending(false);
    }
  }

  async function confirmUpdates() {
    const snapshot = review;
    if (!snapshot || pending) return;
    if (snapshot.schedules.length > 0 && !privilegeMaterial) {
      setError(
        "Privilege unlock expired before schedule target updates were applied.",
      );
      onOpenPrivilegeUnlock();
      return;
    }
    setPending(true);
    setError(null);
    setFeedback({
      message: "Updating reviewed frozen target snapshots",
      tone: "progress",
    });
    const failures: string[] = [];
    let updatedSchedules = 0;
    let updatedPingTargets = 0;
    let updatedShares = 0;
    try {
      const preparedSchedules = await Promise.all(
        snapshot.schedules.map(async (update) => ({
          ...update,
          privilegeAssertion: await buildScheduleTargetUpdatePrivilegeAssertion(
            {
              privilegeMaterial: privilegeMaterial!,
              schedule: update.schedule,
              selectorExpression: update.selectorExpression,
              targetClientIds: update.nextTargetIds,
            },
          ),
        })),
      );
      if (preparedSchedules.length > 0) {
        try {
          const response = await apiPost<BulkUpdateScheduleTargetsResponse>(
            "/api/v1/schedules/update-targets",
            apiToken,
            {
              confirmed: true,
              items: preparedSchedules.map((update) => ({
                schedule_id: update.schedule.id,
                expected_definition_revision:
                  update.schedule.definition_revision,
                privilege_assertion: update.privilegeAssertion,
              })),
            },
          );
          response.outcomes.forEach((outcome, index) => {
            if (outcome.status === "updated") {
              updatedSchedules += 1;
            } else {
              failures.push(
                `${preparedSchedules[index]?.schedule.name ?? outcome.schedule_id}: ${outcome.error_code ?? "schedule target update rejected"}`,
              );
            }
          });
        } catch (cause) {
          failures.push(`Schedules: ${errorMessage(cause)}`);
        }
      }

      if (snapshot.pingPreview && snapshot.pingTargetIds.length > 0) {
        try {
          const response = await apiPost<BulkUpdatePingTargetsResponse>(
            "/api/v1/ping-targets/update-targets",
            apiToken,
            {
              confirmed: true,
              preview_hash: snapshot.pingPreview.preview_hash,
              target_ids: snapshot.pingTargetIds,
            },
          );
          updatedPingTargets =
            response.changes.filter(pingChangeHasDelta).length;
          for (const sync of response.runtime_sync) {
            if (sync.status !== "queued") {
              failures.push(
                `Ping runtime sync for ${sync.client_id}: ${sync.error ?? sync.status}`,
              );
            }
          }
        } catch (cause) {
          failures.push(`Ping targets: ${errorMessage(cause)}`);
        }
      }
      if (snapshot.sharePreview && snapshot.shareIds.length > 0) {
        try {
          const response =
            await apiPost<BulkUpdateMonitoringShareTargetsResponse>(
              "/api/v1/monitoring-shares/update-targets",
              apiToken,
              {
                confirmed: true,
                preview_hash: snapshot.sharePreview.preview_hash,
                share_ids: snapshot.shareIds,
              },
            );
          updatedShares = response.changes.filter(shareChangeHasDelta).length;
        } catch (cause) {
          failures.push(`Shared views: ${errorMessage(cause)}`);
        }
      }
    } catch (cause) {
      failures.push(errorMessage(cause));
    }

    setReview(null);
    await Promise.allSettled([refreshResources(), onRefreshSchedules()]);
    const successParts = [
      updatedSchedules > 0
        ? `${updatedSchedules} schedule${updatedSchedules === 1 ? "" : "s"}`
        : null,
      updatedPingTargets > 0
        ? `${updatedPingTargets} Ping target${updatedPingTargets === 1 ? "" : "s"}`
        : null,
      updatedShares > 0
        ? `${updatedShares} shared view${updatedShares === 1 ? "" : "s"}`
        : null,
    ].filter(Boolean);
    if (failures.length > 0 && successParts.length === 0) {
      setError(`No target snapshot was updated. ${failures.join(" ")}`);
      setFeedback(null);
    } else {
      setFeedback({
        message: `${successParts.length > 0 ? `Updated ${successParts.join(" and ")}.` : "No target snapshot required a write."}${failures.length > 0 ? ` Remaining problems: ${failures.join(" ")}` : ""}`,
        tone: failures.length > 0 ? "warning" : "success",
      });
    }
    setPending(false);
  }

  const actions: ConsoleDataGridAction<StaleSelectorRow>[] = [
    {
      description: (selected) => {
        const updateable = selected.filter((row) => row.canUpdate);
        return (
          selectorUpdateBatchLimitMessage(updateable) ??
          (selected.length > 0
            ? `Resolve and review ${updateable.length} updateable saved selector${updateable.length === 1 ? "" : "s"}.`
            : "Select one or more stale saved selectors.")
        );
      },
      disabled: (selected) =>
        pending ||
        !selected.some((row) => row.canUpdate) ||
        selectorUpdateBatchLimitMessage(
          selected.filter((row) => row.canUpdate),
        ) !== null,
      icon: <Target size={14} />,
      label: "Update targets",
      onSelect: (selected) => void reviewRows(selected),
    },
  ];

  return (
    <div className="fleetPanel staleSelectorMaintenancePanel">
      <div className="sectionHeader">
        <div>
          <h2>Stale frozen selectors</h2>
          <span>
            {loading
              ? "Checking saved selectors"
              : rows.length === 0
                ? "All mutable frozen target lists match their saved selectors"
                : `${rows.length} stale or invalid saved selector${rows.length === 1 ? "" : "s"}; ${updateableRows.length} updateable${blockedRows > 0 ? `, ${blockedRows} require repair` : ""}`}
          </span>
        </div>
      </div>
      <ActionFeedback
        className="localActionFeedback"
        message={pageFeedback || null}
        tone="danger"
      />
      <ActionFeedback
        className="localActionFeedback"
        message={outcomeMessage}
        ref={feedbackRef}
        tone={outcomeTone}
      />
      <div className="scheduleExecutionPolicy selectorMaintenancePolicy">
        <Target size={16} />
        <span>
          Schedules include backup policies. Ping assignments and shared-view
          targets each update transactionally; schedules keep their native
          per-schedule review and audit boundary. Approval records remain
          immutable evidence and never appear here.
        </span>
      </div>
      <ConsoleDataGrid
        actions={actions}
        columns={columns}
        defaultPageSize={100}
        empty={
          <div className="emptyState compactEmpty">
            <Target size={22} />
            <strong>
              {loading ? "Checking selectors" : "No stale selectors"}
            </strong>
            <span>
              {loading
                ? "Loading schedules, Ping assignments, and active shared views."
                : "Mutable frozen target lists match their saved selector expressions."}
            </span>
          </div>
        }
        getRowId={(row) => row.id}
        itemLabel="selectors"
        renderExpandedRow={(row) => (
          <div className="consoleInlineDetailGrid">
            <span>
              <strong>Resource ID</strong>
              <span className="monoValue">{row.resourceId}</span>
            </span>
            <span>
              <strong>Reason</strong>
              <span>{row.reason}</span>
            </span>
            <span>
              <strong>Frozen target IDs</strong>
              <span className="monoValue">
                {row.frozenTargetIds.join(", ") || "None"}
              </span>
            </span>
            <span>
              <strong>Current local resolution</strong>
              <span className="monoValue">
                {row.resolvedTargetIds?.join(", ") ?? "Unavailable"}
              </span>
            </span>
          </div>
        )}
        rowActions={actions}
        rows={rows}
        searchPlaceholder="Search resource, selector, or frozen VPS ID"
        singleExpandedRow
        storageKey="vpsman.grid.system.staleSelectors"
        title="Stale selector records"
        toolbarActions={
          <div className="previewMeta">
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                pending
                  ? "Stale selector records cannot refresh while an update review is in progress."
                  : "Stale selector records are already refreshing."
              }
              disabled={loading || pending}
              onClick={() => void refreshResources()}
              type="button"
            >
              <RefreshCw size={14} />
              Refresh
            </button>
            <button
              className="primaryAction compactAction"
              data-tooltip-disabled-reason={
                pending
                  ? "A stale selector update review is already in progress."
                  : selectorUpdateBatchLimitMessage(updateableRows) ??
                    "No resolvable stale target snapshots are available to update."
              }
              disabled={
                pending ||
                updateableRows.length === 0 ||
                selectorUpdateBatchLimitMessage(updateableRows) !== null
              }
              onClick={() => void reviewRows(updateableRows)}
              title={
                pending
                  ? "A stale selector update review is already in progress."
                  : updateableRows.length === 0
                    ? "No resolvable stale target snapshots are available to update."
                    : selectorUpdateBatchLimitMessage(updateableRows) ??
                      (blockedRows > 0
                        ? `Update every resolvable stale snapshot; ${blockedRows} invalid definition${blockedRows === 1 ? " remains" : "s remain"} for repair.`
                        : "Resolve and review every stale mutable target snapshot.")
              }
              type="button"
            >
              <Target size={14} />
              Update all
            </button>
          </div>
        }
      />
      <ConfirmationPrompt
        confirmLabel="Update targets"
        detail="Replace only the reviewed frozen target IDs. Saved selector text and every other resource setting remain unchanged. Schedules update separately; Ping and shared-view batches are each atomic, not cross-resource."
        error={error}
        items={selectorReviewItems(review)}
        onCancel={() => setReview(null)}
        onConfirm={() => void confirmUpdates()}
        open={review !== null}
        pending={pending}
        title="Confirm stale target updates"
      />
    </div>
  );
}

function selectorUpdateBatchLimitMessage(
  rows: StaleSelectorRow[],
): string | null {
  const scheduleCount = rows.filter((row) => row.kind === "Schedule").length;
  if (scheduleCount > SCHEDULE_TARGET_UPDATE_BATCH_LIMIT) {
    return `Schedule target updates accept at most ${SCHEDULE_TARGET_UPDATE_BATCH_LIMIT} records in one reviewed batch; narrow the selection by ${scheduleCount - SCHEDULE_TARGET_UPDATE_BATCH_LIMIT} or more.`;
  }
  const pingCount = rows.filter((row) => row.kind === "Ping target").length;
  if (pingCount > PING_TARGET_UPDATE_BATCH_LIMIT) {
    return `Ping target updates accept at most ${PING_TARGET_UPDATE_BATCH_LIMIT} records in one reviewed batch; narrow the selection by ${pingCount - PING_TARGET_UPDATE_BATCH_LIMIT} or more.`;
  }
  const shareCount = rows.filter((row) => row.kind === "Shared view").length;
  if (shareCount > MONITORING_SHARE_UPDATE_BATCH_LIMIT) {
    return `Shared-view target updates accept at most ${MONITORING_SHARE_UPDATE_BATCH_LIMIT} records in one reviewed batch; narrow the selection by ${shareCount - MONITORING_SHARE_UPDATE_BATCH_LIMIT} or more.`;
  }
  return null;
}

function staleSelectorRows(
  schedules: ScheduleRecord[],
  pingTargets: PingTargetView[],
  shares: MonitoringShareView[],
  agents: AgentView[],
  context: AgentSearchContext,
): StaleSelectorRow[] {
  const rows: StaleSelectorRow[] = [];
  for (const schedule of schedules) {
    const selectorExpression = schedule.selector_expression.trim();
    const parsed = parseSearchExpression(selectorExpression);
    const ruleEvidenceUnavailable = vpsRuleSearchUnavailable(
      selectorExpression,
      context,
    );
    const frozenTargetIds = uniqueSorted(schedule.target_client_ids ?? []);
    const resolvedTargetIds =
      selectorExpression && !parsed.error && !ruleEvidenceUnavailable
        ? uniqueSorted(
            agentsMatchingExpression(agents, selectorExpression, context).map(
              (agent) => agent.id,
            ),
          )
        : null;
    if (
      resolvedTargetIds !== null &&
      sameStringSet(frozenTargetIds, resolvedTargetIds)
    ) {
      continue;
    }
    const operationInvalid =
      Boolean(schedule.operation_error) ||
      (schedule.trigger_kind === "cron" && !schedule.operation);
    const reason = parsed.error
      ? `Invalid saved selector: ${parsed.error}`
      : ruleEvidenceUnavailable
        ? "Selector resolution evidence is unavailable; frozen targets remain unchanged. Refresh rule evidence or retry with the required access."
        : operationInvalid
          ? "Saved operation is invalid; repair the schedule before updating targets"
          : resolvedTargetIds?.length === 0
            ? "Saved selector currently matches no visible VPS; update will freeze that exact empty result"
            : "Current selector resolution differs from the frozen target IDs";
    rows.push({
      canUpdate:
        !parsed.error &&
        !ruleEvidenceUnavailable &&
        !operationInvalid &&
        resolvedTargetIds !== null,
      frozenTargetIds,
      id: `schedule:${schedule.id}`,
      kind: "Schedule",
      name: schedule.name,
      reason,
      resolvedTargetIds,
      resourceId: schedule.id,
      selectorExpression,
      source: schedule,
      updatedAt: schedule.updated_at,
    });
  }
  for (const target of pingTargets) {
    const selectorExpression = target.selector_expression.trim();
    const parsed = parseSearchExpression(selectorExpression);
    const ruleEvidenceUnavailable =
      !target.target_update_evidence_available ||
      vpsRuleSearchUnavailable(selectorExpression, context);
    const frozenTargetIds = uniqueSorted(target.target_client_ids ?? []);
    const resolvedTargetIds =
      selectorExpression && !parsed.error && !ruleEvidenceUnavailable
        ? uniqueSorted(
            agentsMatchingExpression(agents, selectorExpression, context).map(
              (agent) => agent.id,
            ),
          )
        : null;
    if (
      !ruleEvidenceUnavailable &&
      !target.target_update_available &&
      (resolvedTargetIds === null ||
        sameStringSet(frozenTargetIds, resolvedTargetIds))
    ) {
      continue;
    }
    rows.push({
      canUpdate: !parsed.error && !ruleEvidenceUnavailable,
      frozenTargetIds,
      id: `ping:${target.id}`,
      kind: "Ping target",
      name: target.name,
      reason: parsed.error
        ? `Invalid saved selector: ${parsed.error}`
        : ruleEvidenceUnavailable
          ? "Target refresh evidence is unavailable; frozen assignments remain unchanged. Repair or retry with the required access."
          : "Current selector resolution differs from the frozen assignments",
      resolvedTargetIds,
      resourceId: target.id,
      selectorExpression,
      source: target,
      updatedAt: target.updated_at,
    });
  }
  for (const share of shares) {
    if (share.status !== "active") {
      continue;
    }
    const selectorExpression = share.selector_expression.trim();
    const parsed = parseSearchExpression(selectorExpression);
    const ruleEvidenceUnavailable =
      !share.target_update_evidence_available ||
      vpsRuleSearchUnavailable(selectorExpression, context);
    if (!ruleEvidenceUnavailable && !share.target_update_available) {
      continue;
    }
    const frozenTargetIds = uniqueSorted(share.target_client_ids);
    const resolvedTargetIds =
      selectorExpression && !parsed.error && !ruleEvidenceUnavailable
        ? uniqueSorted(
            agentsMatchingExpression(agents, selectorExpression, context).map(
              (agent) => agent.id,
            ),
          )
        : null;
    rows.push({
      canUpdate: !parsed.error && !ruleEvidenceUnavailable,
      frozenTargetIds,
      id: `share:${share.id}`,
      kind: "Shared view",
      name: share.name,
      reason: parsed.error
        ? `Invalid saved selector: ${parsed.error}`
        : ruleEvidenceUnavailable
          ? "Target refresh evidence is unavailable; frozen targets remain unchanged. Repair or retry with the required access."
          : "Current selector resolution differs from the frozen shared-view targets",
      resolvedTargetIds,
      resourceId: share.id,
      selectorExpression,
      source: share,
      updatedAt: share.updated_at,
    });
  }
  return rows.sort(
    (left, right) =>
      left.kind.localeCompare(right.kind) ||
      left.name.localeCompare(right.name),
  );
}

async function loadAllSchedules(apiToken: string): Promise<ScheduleRecord[]> {
  const schedules: ScheduleRecord[] = [];
  for (let page = 0; page < MAX_SELECTOR_PAGES; page += 1) {
    const records = await apiGet<ScheduleRecord[]>(
      buildListPath("/api/v1/schedules", {
        dir: "asc",
        limit: SELECTOR_PAGE_SIZE,
        offset: page * SELECTOR_PAGE_SIZE,
        sort: "name",
      }),
      apiToken,
    );
    schedules.push(...records);
    if (records.length < SELECTOR_PAGE_SIZE) {
      return schedules;
    }
  }
  throw new Error(
    `Schedule maintenance scan reached its explicit ${MAX_SELECTOR_PAGES * SELECTOR_PAGE_SIZE}-record boundary; narrow or remove old schedules before using Update all.`,
  );
}

async function loadAllMonitoringShares(
  apiToken: string,
): Promise<MonitoringShareView[]> {
  const shares: MonitoringShareView[] = [];
  for (let page = 0; page < MAX_SELECTOR_PAGES; page += 1) {
    const records = await apiGet<MonitoringShareView[]>(
      `${buildListPath("/api/v1/monitoring-shares", {
        limit: SELECTOR_PAGE_SIZE,
        offset: page * SELECTOR_PAGE_SIZE,
      })}&status=active`,
      apiToken,
    );
    shares.push(...records);
    if (records.length < SELECTOR_PAGE_SIZE) {
      return shares;
    }
  }
  throw new Error(
    `Shared-view maintenance scan reached its explicit ${MAX_SELECTOR_PAGES * SELECTOR_PAGE_SIZE}-record boundary; narrow or revoke old shared views before using Update all.`,
  );
}

function selectorReviewItems(review: SelectorUpdateReview | null) {
  const pingChanges = review?.pingPreview?.changes ?? [];
  const schedules = review?.schedules ?? [];
  const shareChanges = review?.sharePreview?.changes ?? [];
  return [
    { label: "Selected stale records", value: review?.selectedCount ?? 0 },
    { label: "Schedule snapshots", value: schedules.length },
    { label: "Ping assignments", value: pingChanges.length },
    { label: "Shared-view targets", value: shareChanges.length },
    {
      label: "Only change",
      value: "Frozen VPS target IDs",
    },
    {
      label: "Write boundary",
      value: "Not cross-resource atomic",
    },
    {
      label: "Target deltas",
      value: (
        <div
          className="configurationReviewList"
          aria-label="Exact added and removed VPS target IDs"
          tabIndex={0}
        >
          {schedules.map((update) => {
            const delta = targetDelta(
              update.schedule.target_client_ids,
              update.nextTargetIds,
            );
            return (
              <span key={`schedule:${update.schedule.id}`}>
                <strong>
                  Schedule · {update.schedule.name} ·{" "}
                  {targetCountLabel(update.nextTargetIds.length)} now
                </strong>
                <small>Added: {delta.added.join(", ") || "None"}</small>
                <small>Removed: {delta.removed.join(", ") || "None"}</small>
              </span>
            );
          })}
          {pingChanges.map((change) => (
            <span key={`ping:${change.target_id}`}>
              <strong>Ping target · {change.target_name}</strong>
              <small>
                Added: {change.added_client_ids.join(", ") || "None"}
              </small>
              <small>
                Removed: {change.removed_client_ids.join(", ") || "None"}
              </small>
            </span>
          ))}
          {shareChanges.map((change) => (
            <span key={`share:${change.share_id}`}>
              <strong>Shared view · {change.share_name}</strong>
              <small>
                Added: {change.added_client_ids.join(", ") || "None"}
              </small>
              <small>
                Removed: {change.removed_client_ids.join(", ") || "None"}
              </small>
            </span>
          ))}
        </div>
      ),
    },
  ];
}

function maintenanceTabFromSubpage(subpage: string): MaintenanceTab {
  const detail = subpage.split(":")[1];
  return detail === "artifacts" || detail === "jobs" ? detail : "selectors";
}

function pingChangeHasDelta(change: PingTargetAssignmentChangeView): boolean {
  return (
    change.added_client_ids.length > 0 || change.removed_client_ids.length > 0
  );
}

function shareChangeHasDelta(change: MonitoringShareTargetChangeView): boolean {
  return (
    change.added_client_ids.length > 0 || change.removed_client_ids.length > 0
  );
}

function targetDeltaLabel(current: string[], next: string[]): string {
  const currentSet = new Set(current);
  const nextSet = new Set(next);
  const added = next.filter((id) => !currentSet.has(id)).length;
  const removed = current.filter((id) => !nextSet.has(id)).length;
  return `${targetCountLabel(next.length)} now · +${added} / -${removed}`;
}

function targetDelta(current: string[], next: string[]) {
  const currentSet = new Set(current);
  const nextSet = new Set(next);
  return {
    added: next.filter((id) => !currentSet.has(id)),
    removed: current.filter((id) => !nextSet.has(id)),
  };
}

function targetCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function targetIdTitle(ids: string[]): string {
  return ids.length > 0 ? ids.join(", ") : "No frozen target IDs";
}

function assertOrderedSelectorResolution(
  selectors: string[],
  response: BulkResolveManyResponse,
): void {
  if (
    response.outcomes.length !== selectors.length ||
    response.outcomes.some(
      (outcome, index) =>
        outcome.selector_expression !== selectors[index] ||
        outcome.target_count !== outcome.target_client_ids.length,
    )
  ) {
    throw new Error("Target resolution returned an invalid ordered result set");
  }
}

function uniqueSorted(values: string[]): string[] {
  return Array.from(new Set(values)).sort();
}

function sameStringSet(left: string[], right: string[]): boolean {
  const normalizedLeft = uniqueSorted(left);
  const normalizedRight = uniqueSorted(right);
  return (
    normalizedLeft.length === normalizedRight.length &&
    normalizedLeft.every((value, index) => value === normalizedRight[index])
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
