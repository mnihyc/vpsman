import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  AlertTriangle,
  ArrowUpCircle,
  Bell,
  Check,
  CircleCheck,
  History,
  Server,
  VolumeX,
} from "lucide-react";
import { ActionFeedback } from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { formatBoundedCount } from "../constants";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
  type ConsoleDataGridSearchState,
} from "../components/ConsoleDataGrid";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { searchFieldsForSearchValues } from "../components/searchSuggestions";
import { usePanelDisplaySettings } from "../panelDisplay";
import { filterBySearchExpression } from "../searchExpression";
import {
  alertCategoryLabel,
  presentFleetAlert,
  readableAlertToken,
} from "../alertPresentation";
import type {
  AgentView,
  FleetAlertBulkResolveRequest,
  FleetAlertBulkResolveResponse,
  FleetAlertRecord,
  FleetAlertStateBulkRequest,
  FleetAlertStateBulkResponse,
  FleetAlertStateRequest,
} from "../types";
import { formatCompactTime, formatFullTime, formatVpsName } from "../utils";

const MAX_ALERT_STATE_BULK_ITEMS = 1_000;

type FleetAlertsPanelProps = {
  agents: AgentView[];
  apiError: string | null;
  alerts: FleetAlertRecord[];
  alertsEvidenceAvailable: boolean;
  alertsTruncated: boolean;
  canManageAlertLifecycle: boolean;
  eventReviewError: string | null;
  eventReviewHasMore: boolean;
  eventReviewItems: FleetAlertRecord[];
  eventReviewLoading: boolean;
  eventReviewLimitNotice: string | null;
  eventReviewStarted: boolean;
  eventReviewVerified: boolean;
  eventSearchHasMore: boolean;
  eventSearchItems: FleetAlertRecord[];
  eventSearchQuery: string;
  eventSearchScannedCount: number;
  history: FleetAlertRecord[];
  historyEvidenceAvailable: boolean;
  historyTruncated: boolean;
  onOpenAlertPolicies: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  onActivateEvents: () => Promise<void>;
  onDeactivateEvents: () => void;
  onLoadOlderEvents: () => Promise<void>;
  onSearchOlderEvents: (
    query: string,
    matchingItems: (items: FleetAlertRecord[]) => FleetAlertRecord[],
    scan: boolean,
  ) => Promise<void>;
  onSyncEvents: () => Promise<void>;
  onResolve: (
    request: FleetAlertBulkResolveRequest,
  ) => Promise<FleetAlertBulkResolveResponse>;
  onUpdateBulk: (
    request: FleetAlertStateBulkRequest,
  ) => Promise<FleetAlertStateBulkResponse>;
};

export function FleetAlertsPanel({
  agents,
  apiError,
  alerts,
  alertsEvidenceAvailable,
  alertsTruncated,
  canManageAlertLifecycle,
  eventReviewError,
  eventReviewHasMore,
  eventReviewItems,
  eventReviewLoading,
  eventReviewLimitNotice,
  eventReviewStarted,
  eventReviewVerified,
  eventSearchHasMore,
  eventSearchItems,
  eventSearchQuery,
  eventSearchScannedCount,
  history,
  historyEvidenceAvailable,
  historyTruncated,
  onOpenAlertPolicies,
  onOpenVpsDetail,
  onActivateEvents,
  onDeactivateEvents,
  onLoadOlderEvents,
  onSearchOlderEvents,
  onSyncEvents,
  onResolve,
  onUpdateBulk,
}: FleetAlertsPanelProps) {
  useEffect(() => {
    void onActivateEvents();
    return onDeactivateEvents;
  }, [onActivateEvents, onDeactivateEvents]);
  const currentConditions = useMemo(
    () => alerts.filter((alert) => alert.record_kind !== "event"),
    [alerts],
  );
  const currentAlerts = useMemo(
    // Conditions remain owned by the full Fleet snapshot. Occurrences have one
    // separate current-feed owner and participate only after it verifies them.
    () =>
      dedupeFleetAlertsById([
        ...(eventReviewVerified ? eventReviewItems : []),
        ...(eventReviewVerified ? eventSearchItems : []),
        ...currentConditions,
      ]),
    [
      currentConditions,
      eventReviewItems,
      eventReviewVerified,
      eventSearchItems,
    ],
  );
  const currentTriagedCount = currentAlerts.filter((alert) => {
    const presentation = presentFleetAlert(alert);
    return (
      presentation.current &&
      presentation.operatorState !== null &&
      presentation.operatorState !== "open"
    );
  }).length;
  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Fleet alerts</h2>
            <span>
              {!alertsEvidenceAvailable
                ? eventReviewStarted && eventReviewVerified
                  ? `Current alert snapshot is unavailable; ${currentAlerts.length} incident review row${currentAlerts.length === 1 ? " is" : "s are"} loaded separately.`
                  : "Current alert evidence is unavailable."
                : `${formatBoundedCount(currentAlerts.length, alertsTruncated)} current alert episode${currentAlerts.length === 1 ? "" : "s"}${alertsTruncated ? " in the loaded evidence" : ""}`}
            </span>
          </div>
          <div className="sectionActions">
            <span className="sectionContext">
              {alertsEvidenceAvailable
                ? `${formatBoundedCount(currentTriagedCount, alertsTruncated)} current triaged episode${currentTriagedCount === 1 ? "" : "s"}`
                : "Current triage evidence unavailable"}
            </span>
            <button
              className="secondaryAction compactAction"
              onClick={onOpenAlertPolicies}
              title={
                canManageAlertLifecycle
                  ? "Open Alert Policies to tune every alert's trigger, confirmation, and automatic resolution conditions."
                  : "View Alert Policies. Alert lifecycle changes require Operator or Admin role with fleet:read, backups:read, and integrations:write."
              }
              type="button"
            >
              <Bell size={14} />
              <span>
                {canManageAlertLifecycle
                  ? "Open alert policies"
                  : "View alert policies"}
              </span>
            </button>
          </div>
        </div>
        <ConsoleFreshnessBanner
          currentAvailable={alertsEvidenceAvailable}
          error={apiError}
          historyAvailable={historyEvidenceAvailable}
        />
        <FleetAlertList
          agents={agents}
          alerts={currentAlerts}
          alertsEvidenceAvailable={alertsEvidenceAvailable}
          alertsTruncated={alertsTruncated}
          canManageAlertLifecycle={canManageAlertLifecycle}
          eventReviewAdditionalCount={Math.max(
            0,
            eventReviewVerified
              ? currentAlerts.filter((alert) => alert.record_kind === "event")
                  .length
              : 0,
          )}
          eventReviewError={eventReviewError}
          eventReviewHasMore={eventReviewHasMore}
          eventReviewLoading={eventReviewLoading}
          eventReviewLimitNotice={eventReviewLimitNotice}
          eventReviewRetainedCount={eventReviewItems.length}
          eventReviewStarted={eventReviewStarted}
          eventReviewVerified={eventReviewVerified}
          eventSearchHasMore={eventSearchHasMore}
          eventSearchQuery={eventSearchQuery}
          eventSearchScannedCount={eventSearchScannedCount}
          history={history}
          historyEvidenceAvailable={historyEvidenceAvailable}
          historyTruncated={historyTruncated}
          onOpenAlertPolicies={onOpenAlertPolicies}
          onOpenVpsDetail={onOpenVpsDetail}
          onLoadOlderEvents={onLoadOlderEvents}
          onSearchOlderEvents={onSearchOlderEvents}
          onSyncEvents={onSyncEvents}
          onResolve={onResolve}
          onUpdateBulk={onUpdateBulk}
        />
      </div>
    </section>
  );
}

function ConsoleFreshnessBanner({
  currentAvailable,
  error,
  historyAvailable,
}: {
  currentAvailable: boolean;
  error: string | null;
  historyAvailable: boolean;
}) {
  if (!error && currentAvailable && historyAvailable) {
    return null;
  }
  return (
    <div className="consoleFreshnessBanner">
      <span>
        {!currentAvailable
          ? "Current alert evidence is unavailable; no stale episode is presented as current. "
          : ""}
        {!historyAvailable
          ? "Alert history is unavailable; any retained history is stale. "
          : ""}
        {error ? `Refresh detail: ${error}` : ""}
      </span>
    </div>
  );
}

function FleetAlertList({
  agents,
  alerts,
  alertsEvidenceAvailable,
  alertsTruncated,
  canManageAlertLifecycle,
  eventReviewAdditionalCount,
  eventReviewError,
  eventReviewHasMore,
  eventReviewLoading,
  eventReviewLimitNotice,
  eventReviewRetainedCount,
  eventReviewStarted,
  eventReviewVerified,
  eventSearchHasMore,
  eventSearchQuery,
  eventSearchScannedCount,
  history,
  historyEvidenceAvailable,
  historyTruncated,
  onOpenAlertPolicies,
  onOpenVpsDetail,
  onLoadOlderEvents,
  onSearchOlderEvents,
  onSyncEvents,
  onResolve,
  onUpdateBulk,
}: {
  agents: AgentView[];
  alerts: FleetAlertRecord[];
  alertsEvidenceAvailable: boolean;
  alertsTruncated: boolean;
  canManageAlertLifecycle: boolean;
  eventReviewAdditionalCount: number;
  eventReviewError: string | null;
  eventReviewHasMore: boolean;
  eventReviewLoading: boolean;
  eventReviewLimitNotice: string | null;
  eventReviewRetainedCount: number;
  eventReviewStarted: boolean;
  eventReviewVerified: boolean;
  eventSearchHasMore: boolean;
  eventSearchQuery: string;
  eventSearchScannedCount: number;
  history: FleetAlertRecord[];
  historyEvidenceAvailable: boolean;
  historyTruncated: boolean;
  onOpenAlertPolicies: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  onLoadOlderEvents: () => Promise<void>;
  onSearchOlderEvents: (
    query: string,
    matchingItems: (items: FleetAlertRecord[]) => FleetAlertRecord[],
    scan: boolean,
  ) => Promise<void>;
  onSyncEvents: () => Promise<void>;
  onResolve: (
    request: FleetAlertBulkResolveRequest,
  ) => Promise<FleetAlertBulkResolveResponse>;
  onUpdateBulk: (
    request: FleetAlertStateBulkRequest,
  ) => Promise<FleetAlertStateBulkResponse>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [reviewSnapshot, setReviewSnapshot] = useState<{
    action: FleetAlertStateRequest["action"];
    request: FleetAlertStateBulkRequest;
    rows: FleetAlertRecord[];
  } | null>(null);
  const [resolveSnapshot, setResolveSnapshot] = useState<
    FleetAlertRecord[] | null
  >(null);
  const [resolveReason, setResolveReason] = useState("");
  const [resolvePending, setResolvePending] = useState(false);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolveSuccess, setResolveSuccess] = useState<string | null>(null);
  const [alertSearchState, setAlertSearchState] =
    useState<ConsoleDataGridSearchState>({
      error: null,
      matchingCount: 0,
      pageSize: 10,
      query: "",
    });
  const [historySearchState, setHistorySearchState] =
    useState<ConsoleDataGridSearchState>({
      error: null,
      matchingCount: 0,
      pageSize: 10,
      query: "",
    });
  const agentById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent])),
    [agents],
  );
  const nameById = useMemo(
    () =>
      new Map(
        agents.map((agent) => [
          agent.id,
          formatVpsName(agent, vpsNameDisplayMode),
        ]),
      ),
    [agents, vpsNameDisplayMode],
  );
  const activeAlerts = alerts.filter(
    (alert) => presentFleetAlert(alert).active,
  );
  const criticalCount = activeAlerts.filter(
    (alert) => alert.severity === "critical",
  ).length;
  const warningCount = activeAlerts.filter(
    (alert) => alert.severity === "warning",
  ).length;
  const infoCount = activeAlerts.length - criticalCount - warningCount;
  const unknownCount = alerts.filter(
    (alert) => presentFleetAlert(alert).lifecycleState === "unknown",
  ).length;
  const malformedCount = alerts.filter(
    (alert) => presentFleetAlert(alert).malformed,
  ).length;
  const actionableCount = alerts.filter(
    (alert) => presentFleetAlert(alert).actionable,
  ).length;

  const alertColumns = useMemo<ConsoleDataGridColumn<FleetAlertRecord>[]>(
    () => [
      {
        id: "severity",
        header: "Severity",
        size: 115,
        minSize: 95,
        sortValue: (alert) => alert.severity,
        searchValue: (alert) => alert.severity,
        cell: (alert) => (
          <ConsoleStatusBadge tone={alertTone(alert.severity)}>
            {alert.severity}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "alert",
        header: "Summary",
        size: 390,
        minSize: 240,
        sortValue: (alert) => alert.title,
        searchValue: (alert) => {
          const presentation = presentFleetAlert(alert);
          return `${alert.title} ${alert.detail} ${alertCategoryLabel(alert.category)} ${presentation.recordKindLabel} ${presentation.lifecycleLabel} ${presentation.operatorLabel} ${alertSourceStatusLabel(alert)}`;
        },
        cell: (alert) => {
          const presentation = presentFleetAlert(alert);
          return (
            <span className="historyPrimary fleetAlertSummary">
              <strong>{alert.title}</strong>
              <small>{alert.detail}</small>
              <small>
                {alertCategoryLabel(alert.category)} ·{" "}
                {presentation.recordKindLabel} · {alertSourceStatusLabel(alert)}
              </small>
            </span>
          );
        },
      },
      {
        id: "target",
        header: "VPS",
        size: 210,
        minSize: 150,
        sortValue: (alert) =>
          alert.client_id
            ? (nameById.get(alert.client_id) ?? alert.client_id)
            : alertTargetLabel(alert),
        searchValue: (alert) =>
          `${alert.target_kind} ${alert.target_id} ${alert.client_id ?? ""} ${
            alert.client_id ? (nameById.get(alert.client_id) ?? "") : ""
          }`,
        cell: (alert) => {
          const label = alert.client_id
            ? (nameById.get(alert.client_id) ?? alert.client_id)
            : alertTargetLabel(alert);
          return (
            <span
              className="historyPrimary"
              title={`${alert.target_kind}:${alert.target_id}`}
            >
              <strong>{label}</strong>
              <small>{alertTargetScopeLabel(alert)}</small>
            </span>
          );
        },
      },
      {
        id: "state",
        header: "State",
        size: 190,
        minSize: 160,
        sortValue: (alert) => presentFleetAlert(alert).lifecycleLabel,
        searchValue: (alert) => {
          const presentation = presentFleetAlert(alert);
          return `${presentation.lifecycleLabel} ${presentation.operatorLabel} ${alert.state_reason ?? ""}`;
        },
        cell: (alert) => {
          const presentation = presentFleetAlert(alert);
          return (
            <span className="fleetAlertStateStack">
              <ConsoleStatusBadge tone={presentation.lifecycleTone}>
                {presentation.lifecycleLabel}
              </ConsoleStatusBadge>
              <small>Operator triage: {presentation.operatorLabel}</small>
              {alert.state_reason && <small>{alert.state_reason}</small>}
            </span>
          );
        },
      },
      {
        id: "observed",
        header: "Triggered",
        size: 140,
        minSize: 110,
        sortValue: (alert) =>
          presentFleetAlert(alert).malformed
            ? ""
            : alert.lifecycle.triggered_at,
        cell: (alert) => {
          const presentation = presentFleetAlert(alert);
          return presentation.malformed ? (
            "Lifecycle unavailable"
          ) : (
            <time
              dateTime={alert.lifecycle.triggered_at}
              title={formatFullTime(alert.lifecycle.triggered_at)}
            >
              {formatCompactTime(alert.lifecycle.triggered_at)}
            </time>
          );
        },
      },
      {
        id: "resolution",
        header: "Resolution",
        size: 230,
        minSize: 170,
        sortValue: (alert) => alert.lifecycle?.resolved_at ?? "",
        searchValue: (alert) =>
          `${alert.lifecycle?.resolution_reason ?? ""} ${alert.lifecycle?.resolution_note ?? ""} ${alert.lifecycle?.resolution_actor_id ?? ""}`,
        cell: (alert) =>
          presentFleetAlert(alert).lifecycleState === "resolved" ? (
            <span className="historyPrimary">
              <strong>
                {formatLifecycleTime(alert.lifecycle.resolved_at)}
              </strong>
              <small>{alertResolutionLabel(alert)}</small>
            </span>
          ) : (
            "—"
          ),
      },
    ],
    [nameById],
  );

  useEffect(() => {
    setReviewSnapshot((current) => {
      if (!current) {
        return current;
      }
      const currentAlerts = new Map(alerts.map((alert) => [alert.id, alert]));
      const reviewIsCurrent = current.rows.every((reviewedAlert) => {
        const latestAlert = currentAlerts.get(reviewedAlert.id);
        const reviewed = presentFleetAlert(reviewedAlert);
        const latest = latestAlert ? presentFleetAlert(latestAlert) : null;
        return (
          latestAlert != null &&
          !reviewed.malformed &&
          latest != null &&
          !latest.malformed &&
          latest.operatorState === reviewed.operatorState &&
          latest.lifecycleState === reviewed.lifecycleState &&
          latestAlert.state_revision === reviewedAlert.state_revision &&
          latestAlert.lifecycle.trigger_generation ===
            reviewedAlert.lifecycle.trigger_generation
        );
      });
      return reviewIsCurrent ? current : null;
    });
  }, [alerts]);

  useEffect(() => {
    if (!resolveSnapshot) {
      return;
    }
    const latestById = new Map(alerts.map((alert) => [alert.id, alert]));
    const reviewIsCurrent = resolveSnapshot.every((reviewed) => {
      const latest = latestById.get(reviewed.id);
      return (
        latest != null &&
        presentFleetAlert(latest).resolvableIncident &&
        latest.lifecycle.trigger_generation ===
          reviewed.lifecycle.trigger_generation
      );
    });
    if (!reviewIsCurrent) {
      setResolveSnapshot(null);
      setResolveReason("");
      setResolveError(null);
    }
  }, [alerts, resolveSnapshot]);

  function reviewAlertUpdate(
    rows: FleetAlertRecord[],
    action: FleetAlertStateRequest["action"],
  ) {
    if (
      !canManageAlertLifecycle ||
      rows.length === 0 ||
      rows.length > MAX_ALERT_STATE_BULK_ITEMS ||
      pending
    ) {
      if (rows.length > MAX_ALERT_STATE_BULK_ITEMS) {
        setActionError(
          `Select at most ${MAX_ALERT_STATE_BULK_ITEMS.toLocaleString()} alerts for one triage update.`,
        );
      }
      return;
    }
    setActionError(null);
    setReviewSnapshot({
      action,
      rows,
      request: {
        action,
        items: rows.map((alert) => ({
          alert_id: alert.id,
          expected_revision: alert.state_revision ?? 0,
        })),
        muted_for_secs: action === "mute" ? 4 * 60 * 60 : null,
        reason:
          action === "mute"
            ? "panel mute"
            : action === "acknowledge"
              ? "panel acknowledgement"
              : action === "escalate"
                ? "panel escalation"
                : "panel triage reset to open",
        confirmed: true,
      },
    });
  }

  async function updateReviewedAlerts() {
    const snapshot = reviewSnapshot;
    if (!canManageAlertLifecycle || !snapshot || pending) {
      return;
    }
    setPending(`${snapshot.action}:${snapshot.rows.length}`);
    try {
      await onUpdateBulk(snapshot.request);
      setActionError(null);
      setReviewSnapshot(null);
    } catch (error) {
      const detail =
        error instanceof Error
          ? error.message
          : "The alert update failed without diagnostic detail.";
      setActionError(
        `The reviewed batch is atomic (all or none). A current-state refresh was attempted after the request failed: ${detail}`,
      );
    } finally {
      setPending(null);
    }
  }

  function reviewIncidentResolution(rows: FleetAlertRecord[]) {
    if (
      rows.length === 0 ||
      rows.length > MAX_ALERT_STATE_BULK_ITEMS ||
      rows.some((alert) => !presentFleetAlert(alert).resolvableIncident) ||
      !canManageAlertLifecycle ||
      pending ||
      resolvePending
    ) {
      return;
    }
    setResolveSnapshot(rows);
    setResolveReason("");
    setResolveError(null);
    setResolveSuccess(null);
  }

  async function resolveReviewedIncident() {
    const snapshot = resolveSnapshot;
    const reason = resolveReason.trim();
    if (
      !canManageAlertLifecycle ||
      !snapshot ||
      !reason ||
      pending ||
      resolvePending
    ) {
      return;
    }
    setResolvePending(true);
    setResolveError(null);
    try {
      await onResolve({
        confirmed: true,
        items: snapshot.map((alert) => ({
          alert_id: alert.id,
          expected_trigger_generation: alert.lifecycle.trigger_generation,
        })),
        reason,
      });
      setResolveSuccess(
        snapshot.length === 1
          ? `Resolved occurrence ${snapshot[0].title}, generation ${snapshot[0].lifecycle.trigger_generation}. Operator triage was not changed.`
          : `Resolved ${snapshot.length.toLocaleString()} occurrences atomically. Operator triage was not changed.`,
      );
      setResolveSnapshot(null);
      setResolveReason("");
    } catch (error) {
      setResolveError(
        error instanceof Error
          ? error.message
          : "The incident could not be resolved without diagnostic detail.",
      );
    } finally {
      setResolvePending(false);
    }
  }

  const openRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => {
      const presentation = presentFleetAlert(alert);
      return presentation.current && presentation.operatorState === "open";
    });
  const triagedRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => {
      const presentation = presentFleetAlert(alert);
      return (
        presentation.current &&
        presentation.operatorState !== null &&
        presentation.operatorState !== "open"
      );
    });
  const resolvableRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => presentFleetAlert(alert).resolvableIncident);
  const handleAlertSearchState = useCallback(
    (next: ConsoleDataGridSearchState) => {
      setAlertSearchState((current) =>
        current.error === next.error &&
        current.matchingCount === next.matchingCount &&
        current.pageSize === next.pageSize &&
        current.query === next.query
          ? current
          : next,
      );
    },
    [],
  );
  const handleHistorySearchState = useCallback(
    (next: ConsoleDataGridSearchState) => {
      setHistorySearchState((current) =>
        current.error === next.error &&
        current.matchingCount === next.matchingCount &&
        current.pageSize === next.pageSize &&
        current.query === next.query
          ? current
          : next,
      );
    },
    [],
  );
  const normalizedAlertSearch = alertSearchState.query.trim();
  const validAlertSearchNeedsMore =
    normalizedAlertSearch.length > 0 &&
    alertSearchState.error === null &&
    alertSearchState.matchingCount < alertSearchState.pageSize;
  const searchSequenceIsCurrent = eventSearchQuery === normalizedAlertSearch;
  const searchingOlderOccurrences =
    validAlertSearchNeedsMore &&
    (!searchSequenceIsCurrent || eventSearchHasMore);
  const matchingOlderOccurrenceItems = useCallback(
    (items: FleetAlertRecord[]) => {
      const result = filterBySearchExpression(
        items,
        normalizedAlertSearch,
        (alert) =>
          searchFieldsForSearchValues(
            alertColumns.map(
              (column) =>
                column.searchValue?.(alert) ?? column.sortValue?.(alert),
            ),
          ),
      );
      return result.error ? [] : result.items;
    },
    [alertColumns, normalizedAlertSearch],
  );
  useEffect(() => {
    if (
      !eventReviewVerified ||
      eventReviewLoading ||
      eventReviewError ||
      (searchSequenceIsCurrent && !searchingOlderOccurrences)
    ) {
      return;
    }
    void onSearchOlderEvents(
      normalizedAlertSearch,
      matchingOlderOccurrenceItems,
      validAlertSearchNeedsMore,
    );
  }, [
    eventReviewError,
    eventReviewLoading,
    eventReviewVerified,
    matchingOlderOccurrenceItems,
    normalizedAlertSearch,
    onSearchOlderEvents,
    searchSequenceIsCurrent,
    searchingOlderOccurrences,
    validAlertSearchNeedsMore,
  ]);

  return (
    <div className="fleetAlertList" aria-label="Fleet alerts">
      <div className="fleetAlertHeader">
        <span>
          <AlertTriangle size={17} />
          <strong>Fleet alerts</strong>
        </span>
        <small>
          {!alertsEvidenceAvailable
            ? eventReviewStarted && eventReviewVerified
              ? `Current condition evidence unavailable · ${alerts.length} verified occurrence${alerts.length === 1 ? "" : "s"}`
              : "Current alert evidence unavailable"
            : alerts.length === 0 && !alertsTruncated
              ? "No current alert episodes"
              : `${alertsTruncated ? "Capped condition evidence: " : ""}${criticalCount} critical / ${warningCount} warning / ${infoCount} info active · ${actionableCount} actionable${unknownCount ? ` · ${unknownCount} Unknown` : ""}${malformedCount ? ` · ${malformedCount} malformed` : ""} · ${formatBoundedCount(triagedRows(alerts).length, alertsTruncated)} current triaged`}
        </small>
      </div>
      {!canManageAlertLifecycle ? (
        <div
          aria-label="Fleet alert read-only boundary"
          className="consoleInlineNotice"
        >
          <strong>Read-only alert review</strong>
          <small>
            Operator or Admin role with fleet:read, backups:read, and
            integrations:write is required to acknowledge, mute, escalate, reset
            triage, or resolve an occurrence. Lifecycle and operator triage
            remain visible.
          </small>
        </div>
      ) : null}
      {alertsTruncated ||
      !alertsEvidenceAvailable ||
      !eventReviewVerified ||
      eventReviewStarted ||
      eventReviewError ||
      eventReviewLimitNotice ? (
        <div
          aria-label="Older current incident review"
          className="consoleInlineNotice"
        >
          <span>
            <strong>Unresolved occurrence review</strong>
            <small>
              {eventReviewLoading
                ? searchingOlderOccurrences
                  ? `Searching the next indexed 200-row occurrence page; ${eventSearchScannedCount.toLocaleString()} older rows scanned for this expression.`
                  : "Synchronizing the newest occurrences and revalidating every retained occurrence."
                : eventReviewError
                  ? `Current occurrence evidence is unverified: ${eventReviewError} ${eventReviewRetainedCount.toLocaleString()} retained row${eventReviewRetainedCount === 1 ? " is" : "s are"} hidden until synchronization succeeds.`
                  : eventReviewLimitNotice
                    ? eventReviewLimitNotice
                    : !eventReviewVerified
                      ? "Current occurrence evidence has not been verified yet. Retained occurrence rows are not presented as current."
                      : !eventReviewStarted
                        ? "Synchronizing the authoritative current occurrence head."
                        : eventReviewHasMore
                          ? `${eventReviewAdditionalCount} additional verified current occurrence${eventReviewAdditionalCount === 1 ? "" : "s"} loaded. Load older extends only from the current tail cursor.`
                          : `${eventReviewAdditionalCount} additional verified current occurrence${eventReviewAdditionalCount === 1 ? "" : "s"} loaded. The occurrence feed is complete${alertsTruncated ? "; current condition evidence remains capped" : ""}.`}
            </small>
          </span>
          {eventReviewError ||
          !eventReviewVerified ||
          (eventReviewHasMore &&
            !eventReviewLimitNotice &&
            !searchingOlderOccurrences) ? (
            <button
              className="secondaryAction compactAction"
              disabled={eventReviewLoading}
              onClick={() =>
                void (eventReviewError || !eventReviewVerified
                  ? onSyncEvents()
                  : onLoadOlderEvents())
              }
              type="button"
            >
              <History size={14} />
              <span>
                {eventReviewLoading
                  ? "Synchronizing occurrences"
                  : eventReviewError || !eventReviewVerified
                    ? "Retry current occurrence sync"
                    : "Load older current incidents"}
              </span>
            </button>
          ) : null}
        </div>
      ) : null}
      <ActionFeedback
        className="localActionFeedback"
        message={reviewSnapshot ? null : actionError}
        tone="danger"
      />
      <ActionFeedback
        className="localActionFeedback"
        message={resolveSuccess}
        tone="success"
      />
      <ConsoleDataGrid
        actions={[
          {
            label: "Acknowledge Open triage",
            description: (rows) =>
              bulkActionDescription(
                "Acknowledge",
                openRows(rows),
                "whose operator triage is Open",
              ),
            disabled: (rows) =>
              pending != null || !validAlertStateBulk(openRows(rows)),
            hidden: () => !canManageAlertLifecycle,
            icon: <Check size={14} />,
            onSelect: (rows) =>
              reviewAlertUpdate(openRows(rows), "acknowledge"),
          },
          {
            label: "Mute Open triage 4h",
            description: (rows) =>
              bulkActionDescription(
                "Mute",
                openRows(rows),
                "whose operator triage is Open for four hours",
              ),
            disabled: (rows) =>
              pending != null || !validAlertStateBulk(openRows(rows)),
            hidden: () => !canManageAlertLifecycle,
            icon: <VolumeX size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "mute"),
          },
          {
            label: "Escalate Open triage",
            description: (rows) =>
              bulkActionDescription(
                "Escalate",
                openRows(rows),
                "whose operator triage is Open",
              ),
            disabled: (rows) =>
              pending != null || !validAlertStateBulk(openRows(rows)),
            hidden: () => !canManageAlertLifecycle,
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "escalate"),
          },
          {
            label: "Reset triage to Open",
            description: (rows) =>
              bulkActionDescription(
                "Reset operator triage to Open for",
                triagedRows(rows),
                "without resolving their lifecycle",
              ),
            disabled: (rows) =>
              pending != null || !validAlertStateBulk(triagedRows(rows)),
            hidden: () => !canManageAlertLifecycle,
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => reviewAlertUpdate(triagedRows(rows), "clear"),
          },
          {
            label: "Resolve incident",
            description: (rows) =>
              bulkActionDescription(
                "Resolve",
                resolvableRows(rows),
                "whose occurrence lifecycles are current, using one required operator reason",
              ),
            disabled: (rows) =>
              pending != null ||
              resolvePending ||
              !validAlertStateBulk(resolvableRows(rows)),
            hidden: () => !canManageAlertLifecycle,
            icon: <AlertTriangle size={14} />,
            onSelect: (rows) => reviewIncidentResolution(resolvableRows(rows)),
          },
        ]}
        columns={alertColumns}
        defaultPageSize={10}
        empty={
          !alertsEvidenceAvailable
            ? "Current alert evidence is unavailable."
            : alertsTruncated
              ? "Current condition evidence is capped and this loaded page is empty; narrow the fleet scope, severity, or category."
              : "No current alert episodes."
        }
        expandOnRowClick
        getRowId={(alert) => alert.id}
        itemLabel="alerts"
        renderExpandedRow={(alert) => {
          const presentation = presentFleetAlert(alert);
          return (
            <div className="consoleGridDetails fleetAlertDetail">
              <div className="consoleInlineDetailGrid">
                <span>Operator state</span>
                <strong>{presentation.operatorLabel}</strong>
                <span>Lifecycle</span>
                <strong>{presentation.lifecycleLabel}</strong>
                <span>Record kind</span>
                <strong>{presentation.recordKindLabel}</strong>
                <span>Generation</span>
                <strong>
                  {presentation.malformed
                    ? "Lifecycle unavailable"
                    : alert.lifecycle.trigger_generation}
                </strong>
                <span>Source status</span>
                <strong>{alertSourceStatusLabel(alert)}</strong>
                <span>Category</span>
                <strong>{alertCategoryLabel(alert.category)}</strong>
                <span>Target</span>
                <strong>
                  {alert.target_kind}:{alert.target_id}
                </strong>
                <span>Observed</span>
                <strong>{formatFullTime(alert.observed_at)}</strong>
                <span>Triggered</span>
                <strong>
                  {presentation.malformed
                    ? "Lifecycle unavailable"
                    : formatLifecycleTime(alert.lifecycle.triggered_at)}
                </strong>
                <span>Last confirmed</span>
                <strong>
                  {presentation.malformed
                    ? "Lifecycle unavailable"
                    : formatLifecycleTime(alert.lifecycle.last_confirmed_at)}
                </strong>
                <span>Resolved</span>
                <strong>
                  {presentation.malformed
                    ? "Lifecycle unavailable"
                    : formatLifecycleTime(alert.lifecycle.resolved_at)}
                </strong>
                <span>Resolution</span>
                <strong>{alertResolutionLabel(alert)}</strong>
                <span>Resolution actor</span>
                <strong>{alertResolutionActorLabel(alert)}</strong>
                {alert.muted_until_unix && (
                  <>
                    <span>Muted until</span>
                    <strong>{formatUnixTime(alert.muted_until_unix)}</strong>
                  </>
                )}
                <span>Escalation</span>
                <strong>{alert.escalation_level ?? 0}</strong>
              </div>
              {policyNameFromAlert(alert) && (
                <span className="fleetAlertPolicyHint">
                  Policy: <strong>{policyNameFromAlert(alert)}</strong>
                </span>
              )}
              {!policyNameFromAlert(alert) && (
                <span className="fleetAlertPolicyHint">
                  Policy: <strong>Policy metadata unavailable</strong>
                </span>
              )}
              <pre>{JSON.stringify(alert.evidence, null, 2)}</pre>
            </div>
          );
        }}
        rowActions={[
          {
            label: "Acknowledge Open triage",
            description: (rows) =>
              actionTargetDescription(
                "Acknowledge",
                "fleet alert",
                rows[0]?.title,
                "Marks the operator triage as Acknowledged.",
              ),
            disabled: (rows) => pending != null || openRows(rows).length !== 1,
            hidden: (rows) =>
              !canManageAlertLifecycle || openRows(rows).length !== 1,
            icon: <Check size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "acknowledge"),
          },
          {
            label: "Open VPS",
            description: (rows) =>
              actionTargetDescription(
                "Open",
                "VPS detail for alert",
                rows[0]?.title,
              ),
            disabled: (rows) =>
              !rows[0]?.client_id || !agentById.has(rows[0].client_id),
            icon: <Server size={14} />,
            onSelect: (rows) => {
              const clientId = rows[0]?.client_id;
              const agent = clientId ? agentById.get(clientId) : null;
              if (agent) {
                onOpenVpsDetail(agent);
              }
            },
          },
          {
            label: "Reset triage to Open",
            description: (rows) =>
              actionTargetDescription(
                "Reset triage to Open for",
                "fleet alert",
                rows[0]?.title,
                "This removes operator triage only; it does not resolve the lifecycle.",
              ),
            disabled: (rows) =>
              pending != null || triagedRows(rows).length !== 1,
            hidden: (rows) =>
              !canManageAlertLifecycle || triagedRows(rows).length !== 1,
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "clear"),
          },
          {
            label: "Resolve incident",
            description: (rows) =>
              actionTargetDescription(
                "Resolve",
                "occurrence",
                rows[0]?.title,
                "Closes the event lifecycle with a required operator reason; triage is unchanged.",
              ),
            disabled: (rows) =>
              pending != null ||
              resolvePending ||
              rows.length !== 1 ||
              resolvableRows(rows).length !== 1,
            hidden: (rows) =>
              !canManageAlertLifecycle || resolvableRows(rows).length !== 1,
            icon: <AlertTriangle size={14} />,
            onSelect: reviewIncidentResolution,
          },
          {
            label: "Mute Open triage 4h",
            description: (rows) =>
              actionTargetDescription(
                "Mute",
                "fleet alert",
                rows[0]?.title,
                "Mutes the Open operator triage for four hours.",
              ),
            disabled: (rows) => pending != null || openRows(rows).length !== 1,
            hidden: (rows) =>
              !canManageAlertLifecycle || openRows(rows).length !== 1,
            icon: <VolumeX size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "mute"),
            separatorBefore: true,
          },
          {
            label: "Escalate Open triage",
            description: (rows) =>
              actionTargetDescription(
                "Escalate",
                "fleet alert",
                rows[0]?.title,
                "Raises the Open operator-triage escalation level.",
              ),
            disabled: (rows) => pending != null || openRows(rows).length !== 1,
            hidden: (rows) =>
              !canManageAlertLifecycle || openRows(rows).length !== 1,
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "escalate"),
          },
          {
            label: "Policies",
            description: (rows) =>
              actionTargetDescription(
                "Open",
                "alert policy context for",
                rows[0]?.title,
              ),
            icon: <Bell size={14} />,
            onSelect: () => onOpenAlertPolicies(),
          },
        ]}
        renderSelectionPanel={(rows) => {
          const selectedOpen = openRows(rows).length;
          const selectedTriaged = triagedRows(rows).length;
          const selectedResolvable = resolvableRows(rows).length;
          const exceedsBulkLimit =
            selectedOpen > MAX_ALERT_STATE_BULK_ITEMS ||
            selectedTriaged > MAX_ALERT_STATE_BULK_ITEMS ||
            selectedResolvable > MAX_ALERT_STATE_BULK_ITEMS;
          return (
            <span>
              {rows.length} selected · {selectedOpen} current with Open triage ·{" "}
              {selectedTriaged} current triaged · {selectedResolvable}{" "}
              resolvable occurrence{selectedResolvable === 1 ? "" : "s"}
              {exceedsBulkLimit
                ? ` · Atomic action limit is ${MAX_ALERT_STATE_BULK_ITEMS.toLocaleString()}; narrow the selection.`
                : ""}
            </span>
          );
        }}
        rows={alerts}
        rowsTruncated={alertsTruncated || eventReviewHasMore}
        onSearchStateChange={handleAlertSearchState}
        searchPlaceholder="Search alerts"
        selectable={canManageAlertLifecycle}
        storageKey="vpsman.grid.fleet.alerts.v1"
        title="Current alert episodes"
        truncatedEmpty={
          eventReviewError
            ? "Current occurrence synchronization failed. Retry the sync above before trusting this view."
            : !eventReviewVerified || eventReviewLoading
              ? "Synchronizing the authoritative current occurrence head."
              : eventReviewHasMore
                ? "No verified current occurrence is loaded yet. Load older current incidents above, or narrow the fleet scope, severity, or category."
                : alertsTruncated
                  ? "Current condition evidence is capped and this loaded page is empty; narrow the fleet scope, severity, or category."
                  : undefined
        }
        truncatedSearchEmpty={
          eventReviewError
            ? "Search is paused because current occurrence synchronization failed. Retry the current occurrence sync above."
            : eventReviewLimitNotice && !normalizedAlertSearch
              ? eventReviewLimitNotice
              : searchingOlderOccurrences || eventReviewLoading
                ? `Searching older current occurrences; ${eventSearchScannedCount.toLocaleString()} indexed rows scanned for this expression.`
                : validAlertSearchNeedsMore &&
                    searchSequenceIsCurrent &&
                    !eventSearchHasMore
                  ? alertsTruncated
                    ? "No verified occurrence matches after reaching the current occurrence-feed end. Current condition evidence is capped; narrow by VPS, severity, or category."
                    : "No current alert matches after reaching the authoritative occurrence-feed end."
                  : alertsTruncated
                    ? "No loaded alert matches. Current condition evidence is capped; narrow by VPS, severity, or category, or review the alert policies."
                    : "No current alert matches this search."
        }
        truncatedStatus={
          alertSearchState.query.trim()
            ? eventReviewError
              ? `${alertSearchState.matchingCount} matching · occurrence search paused; retry above`
              : eventReviewLimitNotice && !normalizedAlertSearch
                ? `${alertSearchState.matchingCount} matching · retained-review limit reached; narrow or export`
                : searchingOlderOccurrences || eventReviewLoading
                  ? `${alertSearchState.matchingCount} matching · ${eventSearchScannedCount.toLocaleString()} older occurrence rows scanned`
                  : eventSearchHasMore &&
                      alertSearchState.matchingCount >=
                        alertSearchState.pageSize
                    ? `${alertSearchState.matchingCount} matching · visible page filled; narrow the expression to inspect later occurrence matches`
                    : validAlertSearchNeedsMore &&
                        searchSequenceIsCurrent &&
                        !eventSearchHasMore
                      ? alertsTruncated
                        ? `${alertSearchState.matchingCount} matching · occurrence feed complete; condition evidence capped`
                        : `${alertSearchState.matchingCount} matching · authoritative occurrence feed complete`
                      : alertsTruncated
                        ? `${alertSearchState.matchingCount} matching · current condition evidence is capped`
                        : `${alertSearchState.matchingCount} matching current alerts`
            : eventReviewHasMore
              ? `${alerts.length} verified current rows loaded · Load older extends the occurrence tail`
              : alertsTruncated
                ? `${alerts.length} current rows loaded · current condition evidence is capped`
                : `${alerts.length} current rows loaded · authoritative current evidence complete`
        }
      />
      <ConsoleDataGrid
        columns={alertColumns}
        defaultPageSize={10}
        empty={
          !historyEvidenceAvailable
            ? "Alert episode history is unavailable."
            : historyTruncated
              ? "The newest retained-history page is empty; use alert export with an explicit time window for authoritative older evidence."
              : "No retained alert episode history."
        }
        getRowId={(alert) => alert.id}
        itemLabel="episodes"
        onSearchStateChange={handleHistorySearchState}
        rows={history}
        rowsTruncated={historyTruncated}
        searchPlaceholder="Search alert episode history"
        selectable={false}
        storageKey="vpsman.grid.fleet.alert-history.v1"
        title="Alert episode history"
        truncatedEmpty="The newest retained-history page is empty. Use alert export with an explicit time window for authoritative older evidence."
        truncatedSearchEmpty="No newest retained episode matches. Use the alert export with an explicit time window for complete retained-history evidence."
        truncatedStatus={
          historySearchState.query.trim()
            ? `${historySearchState.matchingCount} matching in the newest ${history.length} retained episodes · use alert export for the complete time window`
            : `Newest ${history.length} retained episodes loaded · use alert export for older retained evidence`
        }
      />
      <ConfirmationPrompt
        confirmLabel={fleetAlertActionLabel(reviewSnapshot?.action)}
        detail="Applies the reviewed operator triage updates atomically. It does not change alert lifecycles."
        items={[
          {
            label: "Action",
            value: fleetAlertActionLabel(reviewSnapshot?.action),
          },
          {
            label: "Batch",
            value: `${(reviewSnapshot?.rows.length ?? 0).toLocaleString()} alerts`,
          },
          {
            label: "Commit",
            value: "Atomic · all or none",
          },
          {
            label: "Alerts",
            value: selectedRecordSummary(
              reviewSnapshot?.rows ?? null,
              "alert",
              "alerts",
              (row) => row.title,
              (row) => row.id,
            ),
          },
        ]}
        error={actionError ?? undefined}
        onCancel={() => {
          setActionError(null);
          setReviewSnapshot(null);
        }}
        onConfirm={() => void updateReviewedAlerts()}
        open={reviewSnapshot !== null}
        pending={pending !== null}
        title="Confirm fleet alert triage"
        tone={reviewSnapshot?.action === "clear" ? "normal" : "danger"}
      />
      <ConfirmationPrompt
        confirmDisabled={!resolveReason.trim()}
        confirmLabel="Resolve incident"
        detail="Closes the reviewed occurrence lifecycles atomically and emits one resolved edge per occurrence. Operator triage remains separate and unchanged."
        error={resolveError ?? undefined}
        items={[
          {
            label: "Batch",
            value: `${(resolveSnapshot?.length ?? 0).toLocaleString()} occurrence${resolveSnapshot?.length === 1 ? "" : "s"}`,
          },
          {
            label: "Commit",
            value: "Atomic · all or none",
          },
          {
            label: "Occurrences",
            value: selectedRecordSummary(
              resolveSnapshot,
              "occurrence",
              "occurrences",
              (row) => row.title,
              (row) => row.id,
            ),
          },
        ]}
        onCancel={() => {
          setResolveSnapshot(null);
          setResolveReason("");
          setResolveError(null);
        }}
        onConfirm={() => void resolveReviewedIncident()}
        open={resolveSnapshot !== null}
        pending={resolvePending}
        title="Confirm incident resolution"
        tone="warning"
      >
        <label className="confirmationTypedInput">
          <span>Resolution reason</span>
          <textarea
            aria-label="Incident resolution reason"
            autoFocus
            maxLength={1024}
            onChange={(event) => setResolveReason(event.target.value)}
            placeholder="Describe why this occurrence is being closed"
            required
            value={resolveReason}
          />
        </label>
      </ConfirmationPrompt>
    </div>
  );
}

function formatUnixTime(value: number): string {
  return formatCompactTime(new Date(value * 1000).toISOString());
}

function fleetAlertActionLabel(
  action: FleetAlertStateRequest["action"] | undefined,
): string {
  switch (action) {
    case "acknowledge":
      return "Acknowledge";
    case "mute":
      return "Mute";
    case "escalate":
      return "Escalate";
    case "clear":
      return "Reset triage to Open";
    default:
      return "Confirm";
  }
}

function validAlertStateBulk(rows: FleetAlertRecord[]): boolean {
  return rows.length > 0 && rows.length <= MAX_ALERT_STATE_BULK_ITEMS;
}

function bulkActionDescription(
  action: string,
  rows: FleetAlertRecord[],
  detail: string,
): string {
  if (rows.length > MAX_ALERT_STATE_BULK_ITEMS) {
    return `${rows.length.toLocaleString()} alerts match. Select at most ${MAX_ALERT_STATE_BULK_ITEMS.toLocaleString()} for one atomic update.`;
  }
  return `${action} ${rows.length.toLocaleString()} selected alert${rows.length === 1 ? "" : "s"} ${detail}. The update commits all or none.`;
}

function alertTone(severity: string): "critical" | "warning" | "info" {
  if (severity === "critical") {
    return "critical";
  }
  if (severity === "warning") {
    return "warning";
  }
  return "info";
}

function alertTargetLabel(alert: FleetAlertRecord) {
  return alert.target_id.trim() || "Target unavailable";
}

function dedupeFleetAlertsById(alerts: FleetAlertRecord[]): FleetAlertRecord[] {
  const byId = new Map<string, FleetAlertRecord>();
  for (const alert of alerts) {
    byId.set(alert.id, alert);
  }
  return Array.from(byId.values());
}

function alertSourceStatusLabel(alert: FleetAlertRecord): string {
  if (
    alert.record_kind === "condition" &&
    alert.target_kind === "policy_rule"
  ) {
    return "Policy condition evidence";
  }
  switch (alert.status) {
    case "tunnel_adapter_degraded":
      return "Tunnel adapter degraded";
    case "stale":
      return "Agent stale";
    default:
      return readableAlertToken(alert.status);
  }
}

function alertTargetScopeLabel(alert: FleetAlertRecord): string {
  switch (alert.target_kind) {
    case "agent":
    case "client":
      return "VPS";
    case "tunnel":
      return "Tunnel";
    case "configuration_preset":
      return "Configuration preset";
    case "configuration_source":
      return "Configuration source";
    case "policy_rule":
      return "Policy alert";
    default:
      return readableAlertToken(alert.target_kind);
  }
}

function formatLifecycleTime(value: string | null | undefined): string {
  return value && Number.isFinite(Date.parse(value))
    ? formatFullTime(value)
    : "Not recorded";
}

function alertResolutionLabel(alert: FleetAlertRecord): string {
  const presentation = presentFleetAlert(alert);
  if (presentation.lifecycleState !== "resolved") {
    return presentation.malformed ? "Lifecycle unavailable" : "Not resolved";
  }
  const reason = alert.lifecycle.resolution_reason
    ? readableAlertToken(alert.lifecycle.resolution_reason)
    : "Reason unavailable";
  const note = alert.lifecycle.resolution_note?.trim();
  const actor = alert.lifecycle.resolution_actor_id;
  const provenance = actor ? `Actor ${actor}` : "Automatic resolution";
  return `${note ? `${reason} · ${note}` : reason} · ${provenance}`;
}

function alertResolutionActorLabel(alert: FleetAlertRecord): string {
  const presentation = presentFleetAlert(alert);
  if (presentation.malformed) {
    return "Lifecycle unavailable";
  }
  if (presentation.lifecycleState !== "resolved") {
    return "Not resolved";
  }
  return alert.lifecycle.resolution_actor_id ?? "Automatic";
}

function policyNameFromAlert(alert: FleetAlertRecord): string | null {
  const evidence = alert.evidence;
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    return null;
  }
  const policy = (evidence as { policy?: unknown }).policy;
  if (!policy || typeof policy !== "object" || Array.isArray(policy)) {
    return null;
  }
  const name = (policy as { name?: unknown }).name;
  return typeof name === "string" && name.trim() ? name : null;
}

function actionTargetDescription(
  action: string,
  kind: string,
  name: string | undefined,
  detail?: string,
): string {
  const target = name ? `${kind} ${name}` : kind;
  return detail ? `${action} ${target}. ${detail}` : `${action} ${target}.`;
}

function selectedRecordSummary<T>(
  rows: T[] | null,
  singularLabel: string,
  pluralLabel: string,
  getName: (row: T) => string,
  getId: (row: T) => string,
): ReactNode {
  const selectedRows = rows ?? [];
  if (selectedRows.length === 0) {
    return `0 ${pluralLabel}`;
  }
  const visibleRows = selectedRows.slice(0, 8);
  const remaining = selectedRows.length - visibleRows.length;
  return (
    <div
      className="configurationReviewList"
      aria-label={`${selectedRows.length} ${selectedRows.length === 1 ? singularLabel : pluralLabel} in the atomic selection`}
      tabIndex={0}
    >
      <span>
        <strong>
          {selectedRows.length.toLocaleString()}{" "}
          {selectedRows.length === 1 ? singularLabel : pluralLabel}
        </strong>
        <small>
          One exact atomic selection; the update commits all or none.
        </small>
      </span>
      {visibleRows.map((row) => (
        <span key={getId(row)}>
          <strong>{getName(row)}</strong>
          <small>{getId(row)}</small>
        </span>
      ))}
      {remaining > 0 && (
        <span>
          <strong>{remaining.toLocaleString()} more alerts</strong>
          <small>Included in this exact reviewed selection.</small>
        </span>
      )}
    </div>
  );
}
