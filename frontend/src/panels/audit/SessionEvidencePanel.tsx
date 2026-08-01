import {
  History,
  KeyRound,
  Link2,
  LogOut,
  TerminalSquare,
  UserX,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { presentAudit } from "../../auditPresentation";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  formatLowerBoundCount,
} from "../../constants";
import type {
  AgentView,
  AuditLogRecord,
  JobHistoryRecord,
  JsonValue,
  OperatorAuthEventRecord,
  OperatorView,
  OperatorSessionRecord,
} from "../../types";
import type { TerminalSessionRecord } from "../../typesTerminal";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  operatorDbPayloadHashHex,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../../privilege";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../../hooks/useReviewGenerationGuard";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  shortHash,
  shortId,
} from "../../utils";

type TerminalEvidenceRecord = {
  audits: AuditLogRecord[];
  job: JobHistoryRecord | null;
  session: TerminalSessionRecord;
};

type EvidenceStateTone = "info" | "neutral" | "ok" | "warn";

type TerminalEvidenceState = {
  detail: string;
  label: string;
  open: boolean;
  tone: EvidenceStateTone;
};

type OperatorSessionEvidenceState = {
  detail: string;
  label: string;
  tone: EvidenceStateTone;
};

const TERMINAL_STALE_FLOOR_MS = 60 * 60 * 1000;

export function SessionEvidencePanel({
  agents,
  audits,
  auditsTruncated,
  jobs,
  jobsTruncated,
  loading,
  onClearSession,
  onOpenPrivilegeUnlock,
  onRefresh,
  onRevokeOperatorSession,
  operator,
  operatorAuthEvents,
  operatorAuthEventsTruncated,
  operatorSessions,
  operatorSessionsTruncated,
  privilegeMaterial,
  terminalSessions,
  terminalSessionsTruncated,
}: {
  agents: AgentView[];
  audits: AuditLogRecord[];
  auditsTruncated: boolean;
  jobs: JobHistoryRecord[];
  jobsTruncated: boolean;
  loading: boolean;
  onClearSession: () => void;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => void;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  operator: OperatorView | null;
  operatorAuthEvents: OperatorAuthEventRecord[];
  operatorAuthEventsTruncated: boolean;
  operatorSessions: OperatorSessionRecord[];
  operatorSessionsTruncated: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  terminalSessions: TerminalSessionRecord[];
  terminalSessionsTruncated: boolean;
}) {
  const canInspectOperatorAuthority = operator?.role === "admin";
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const authEventsTruncated = operatorAuthEventsTruncated;
  const auditCorrelationTruncated = auditsTruncated || jobsTruncated;
  const authCorrelationTruncated =
    authEventsTruncated || auditCorrelationTruncated;
  const operatorCorrelationTruncated =
    operatorSessionsTruncated || auditCorrelationTruncated;
  const agentNameById = useMemo(
    () =>
      new Map(
        agents.map((agent) => [agent.id, agent.display_name || agent.id]),
      ),
    [agents],
  );
  const jobsById = useMemo(
    () => new Map(jobs.map((job) => [job.id, job])),
    [jobs],
  );
  const authEventBySessionId = useMemo(
    () =>
      new Map(
        operatorAuthEvents
          .filter((event) => event.session_id)
          .map((event) => [event.session_id as string, event]),
      ),
    [operatorAuthEvents],
  );
  const evidenceRows = useMemo<TerminalEvidenceRecord[]>(
    () =>
      terminalSessions.map((session) => {
        const job = jobsById.get(session.job_id) ?? null;
        return {
          audits: audits
            .filter((audit) => auditMatchesTerminalSession(audit, session))
            .sort((left, right) =>
              right.created_at.localeCompare(left.created_at),
            ),
          job,
          session,
        };
      }),
    [audits, jobsById, terminalSessions],
  );
  useEffect(() => {
    if (!selectedKey && evidenceRows.length > 0) {
      setSelectedKey(terminalKey(evidenceRows[0].session));
    }
  }, [evidenceRows, selectedKey]);

  const selectedRecord = useMemo(
    () =>
      evidenceRows.find((row) => terminalKey(row.session) === selectedKey) ??
      evidenceRows[0] ??
      null,
    [evidenceRows, selectedKey],
  );
  const terminalStateByKey = useMemo(
    () =>
      new Map(
        terminalSessions.map((session) => [
          terminalKey(session),
          terminalEvidenceState(session),
        ]),
      ),
    [terminalSessions],
  );
  const operatorStateById = useMemo(
    () =>
      new Map(
        operatorSessions.map((session) => [
          session.id,
          operatorSessionEvidenceState(session),
        ]),
      ),
    [operatorSessions],
  );
  const openSessions = terminalSessions.filter(
    (session) => terminalStateByKey.get(terminalKey(session))?.open,
  ).length;
  const staleTerminalSessions = terminalSessions.filter(
    (session) =>
      terminalStateByKey.get(terminalKey(session))?.label === "Stale state",
  ).length;
  const replayableSessions = terminalSessions.filter(
    (session) => transcriptEvidenceState(session).replayable,
  ).length;
  const retainedBytes = terminalSessions.reduce(
    (total, session) => total + (session.output_retained_bytes ?? 0),
    0,
  );
  const matchedSessions = evidenceRows.filter(
    (row) => row.audits.length > 0,
  ).length;
  const expiredOperatorSessions = operatorSessions.filter(
    (session) => operatorStateById.get(session.id)?.label === "Expired",
  ).length;
  const demoAuthSignals = operatorAuthEvents.filter(isDemoAuthEvent).length;

  const columns = useMemo<ConsoleDataGridColumn<TerminalEvidenceRecord>[]>(
    () => [
      {
        id: "operator",
        header: "Operator",
        minSize: 140,
        searchValue: (row) => terminalActorLabel(row, authEventBySessionId),
        size: 150,
        sortValue: (row) => terminalActorLabel(row, authEventBySessionId),
        cell: (row) => (
          <span className="historyPrimary">
            <strong title={row.job?.actor_id ?? undefined}>
              {terminalActorLabel(row, authEventBySessionId)}
            </strong>
            <small title={terminalActorEvidenceTitle(row)}>
              {terminalActorDetail(row, authEventBySessionId)}
            </small>
          </span>
        ),
      },
      {
        id: "vps",
        header: "VPS",
        minSize: 130,
        searchValue: (row) =>
          `${row.session.client_id} ${agentNameById.get(row.session.client_id) ?? ""}`,
        size: 150,
        sortValue: (row) =>
          agentNameById.get(row.session.client_id) ?? row.session.client_id,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>
              {agentNameById.get(row.session.client_id) ??
                row.session.client_id}
            </strong>
            <small>{row.session.client_id}</small>
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        minSize: 120,
        searchValue: (row) => {
          const state =
            terminalStateByKey.get(terminalKey(row.session)) ??
            terminalEvidenceState(row.session);
          return `${state.label} ${state.detail} ${row.session.state} ${row.session.last_status}`;
        },
        size: 130,
        sortValue: (row) =>
          terminalStateSort(
            terminalStateByKey.get(terminalKey(row.session)) ??
              terminalEvidenceState(row.session),
          ),
        cell: (row) => {
          const state =
            terminalStateByKey.get(terminalKey(row.session)) ??
            terminalEvidenceState(row.session);
          return (
            <span className={`status ${state.tone}`} title={state.detail}>
              {state.label}
            </span>
          );
        },
      },
      {
        id: "started",
        header: "Started",
        minSize: 130,
        searchValue: (row) => terminalStartedLabel(row),
        size: 150,
        sortValue: (row) => terminalStartedAt(row) ?? row.session.observed_at,
        cell: (row) => {
          const startedAt = terminalStartedAt(row);
          return startedAt ? (
            <time dateTime={startedAt} title={formatFullTime(startedAt)}>
              {formatCompactTime(startedAt)}
            </time>
          ) : (
            "Terminal start not reported"
          );
        },
      },
      {
        id: "last_activity",
        header: "Last activity",
        minSize: 150,
        searchValue: (row) => row.session.observed_at,
        size: 170,
        sortValue: (row) => row.session.observed_at,
        cell: (row) => (
          <time
            dateTime={row.session.observed_at}
            title={formatFullTime(row.session.observed_at)}
          >
            {formatCompactTime(row.session.observed_at)}
          </time>
        ),
      },
      {
        id: "expiry",
        header: "Expiry",
        minSize: 150,
        searchValue: (row) =>
          canInspectOperatorAuthority
            ? terminalExpiryLabel(
                row,
                operatorSessions,
                operatorCorrelationTruncated,
              )
            : "Admin-only bearer evidence",
        size: 170,
        sortValue: (row) =>
          canInspectOperatorAuthority
            ? terminalExpirySort(row, operatorSessions)
            : row.session.observed_at,
        cell: (row) =>
          canInspectOperatorAuthority
            ? terminalExpiryLabel(
                row,
                operatorSessions,
                operatorCorrelationTruncated,
              )
            : "Admin-only bearer evidence",
      },
      {
        id: "transcript",
        header: "Transcript",
        minSize: 160,
        searchValue: (row) => transcriptLabel(row.session),
        size: 180,
        sortValue: (row) => row.session.output_retained_bytes ?? 0,
        cell: (row) => transcriptLabel(row.session),
      },
      {
        id: "audit",
        header: "Audit link",
        minSize: 130,
        searchValue: (row) => row.audits.map((audit) => audit.action).join(" "),
        size: 140,
        sortValue: (row) => row.audits.length,
        cell: (row) =>
          row.audits.length > 0 ? (
            <span className="status ok">{row.audits.length} matched</span>
          ) : (
            <span className="status neutral">session ledger</span>
          ),
      },
    ],
    [
      agentNameById,
      authEventBySessionId,
      canInspectOperatorAuthority,
      operatorSessions,
      operatorCorrelationTruncated,
      terminalStateByKey,
    ],
  );

  return (
    <section
      className="fleetPanel auditSessionEvidencePanel"
      aria-label="Audit session evidence"
    >
      <div className="sectionHeader">
        <span>
          <h2>Session evidence</h2>
          <small>
            {canInspectOperatorAuthority
              ? "Terminal and transcript evidence with a complete, revocable bearer-session inventory. Sign out ends only this browser session."
              : "Read-only terminal, transcript, and audit evidence. Sign out ends only this browser session; operator authority correlation is admin-only."}
          </small>
        </span>
        <div className="sectionActions">
          <button
            className="secondaryAction compactAction"
            onClick={onRefresh}
            type="button"
          >
            Refresh
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={onClearSession}
            type="button"
          >
            <LogOut size={16} />
            Sign out
          </button>
        </div>
      </div>

      <div className="metricGrid" aria-label="Session evidence summary">
        <div className="metricCard">
          <TerminalSquare size={18} />
          <strong>
            {formatLowerBoundCount(
              terminalSessions.length,
              terminalSessionsTruncated,
            )}
          </strong>
          <small>
            {terminalSessionsTruncated
              ? "Terminal sessions loaded"
              : "Terminal sessions"}
          </small>
        </div>
        <div className="metricCard">
          <Link2 size={18} />
          <strong>
            {formatLowerBoundCount(
              matchedSessions,
              terminalSessionsTruncated || auditCorrelationTruncated,
            )}
          </strong>
          <small>
            {terminalSessionsTruncated || auditCorrelationTruncated
              ? "Audit-linked in loaded evidence"
              : "Audit-linked terminals"}
          </small>
        </div>
        <div className="metricCard">
          <TerminalSquare size={18} />
          <strong>
            {formatLowerBoundCount(openSessions, terminalSessionsTruncated)}
          </strong>
          <small>
            {staleTerminalSessions > 0
              ? terminalSessionsTruncated
                ? `${formatLowerBoundCount(
                    staleTerminalSessions,
                    true,
                  )} stale states excluded among loaded terminals`
                : `${staleTerminalSessions} stale terminal ${
                    staleTerminalSessions === 1 ? "state" : "states"
                  } hidden from open count`
              : terminalSessionsTruncated
                ? "Open among loaded terminals"
                : "Open terminals"}
          </small>
        </div>
        <div className="metricCard">
          <History size={18} />
          <strong>
            {formatLowerBoundCount(
              replayableSessions,
              terminalSessionsTruncated,
            )}
          </strong>
          <small>
            {terminalSessionsTruncated
              ? "Replayable among loaded terminals"
              : "Replayable transcripts"}
          </small>
        </div>
        <div className="metricCard">
          <History size={18} />
          <strong>
            {terminalSessionsTruncated ? "≥" : ""}
            {formatBytes(retainedBytes)}
          </strong>
          <small>
            {terminalSessionsTruncated
              ? "Retained bytes among loaded terminals"
              : "Retained transcript bytes"}
          </small>
        </div>
        <div className="metricCard">
          <KeyRound size={18} />
          <strong>
            {canInspectOperatorAuthority
              ? formatLowerBoundCount(
                  operatorSessions.length,
                  operatorSessionsTruncated,
                )
              : "Admin only"}
          </strong>
          <small>
            {canInspectOperatorAuthority && expiredOperatorSessions > 0
              ? operatorSessionsTruncated
                ? `${formatLowerBoundCount(
                    expiredOperatorSessions,
                    true,
                  )} expired among loaded bearer sessions`
                : `${expiredOperatorSessions} expired bearer ${
                    expiredOperatorSessions === 1 ? "session" : "sessions"
                  }`
              : operatorSessionsTruncated
                ? "Bearer sessions loaded"
                : "Bearer-session inventory"}
          </small>
        </div>
        <div className="metricCard">
          <KeyRound size={18} />
          <strong>
            {canInspectOperatorAuthority
              ? formatLowerBoundCount(demoAuthSignals, authEventsTruncated)
              : "Admin only"}
          </strong>
          <small>
            {authEventsTruncated
              ? "Demo/test signals among loaded auth events"
              : "Authentication signals"}
          </small>
        </div>
      </div>

      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={10}
        empty={
          <div className="emptyState">
            <TerminalSquare size={22} />
            <strong>No terminal sessions returned</strong>
            <span>
              Terminal open, input, replay, and close evidence will appear here
              after remote operations run.
            </span>
          </div>
        }
        getRowId={(row) => terminalKey(row.session)}
        itemLabel="terminal sessions"
        onOpenRow={(row) => setSelectedKey(terminalKey(row.session))}
        openRowLabel="Select proof"
        openRowTitle={(row) => `Show terminal proof for session ${row.session.session_id}.`}
        rows={evidenceRows}
        rowsTruncated={terminalSessionsTruncated}
        searchPlaceholder="Search terminal session, actor, target, transcript, status, or audit event"
        selectable={false}
        storageKey="audit-terminal-session-evidence-grid"
        title="Terminal session evidence"
      />

      {loading && (
        <div className="dashboardWidgetEmpty">Loading session evidence...</div>
      )}

      {selectedRecord && (
        <SelectedSessionEvidence
          agentNameById={agentNameById}
          auditCorrelationTruncated={auditCorrelationTruncated}
          authCorrelationTruncated={authCorrelationTruncated}
          authEventBySessionId={authEventBySessionId}
          canInspectOperatorAuthority={canInspectOperatorAuthority}
          operatorSessions={operatorSessions}
          operatorCorrelationTruncated={operatorCorrelationTruncated}
          state={
            terminalStateByKey.get(terminalKey(selectedRecord.session)) ??
            terminalEvidenceState(selectedRecord.session)
          }
          record={selectedRecord}
        />
      )}

      <OperatorSessionEvidence
        authEventBySessionId={authEventBySessionId}
        canInspectOperatorAuthority={canInspectOperatorAuthority}
        onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
        onRevokeOperatorSession={onRevokeOperatorSession}
        operatorSessions={operatorSessions}
        operatorSessionsTruncated={operatorSessionsTruncated}
        privilegeMaterial={privilegeMaterial}
        stateById={operatorStateById}
      />
    </section>
  );
}

function SelectedSessionEvidence({
  agentNameById,
  auditCorrelationTruncated,
  authCorrelationTruncated,
  authEventBySessionId,
  canInspectOperatorAuthority,
  operatorSessions,
  operatorCorrelationTruncated,
  record,
  state,
}: {
  agentNameById: Map<string, string>;
  auditCorrelationTruncated: boolean;
  authCorrelationTruncated: boolean;
  authEventBySessionId: Map<string, OperatorAuthEventRecord>;
  canInspectOperatorAuthority: boolean;
  operatorSessions: OperatorSessionRecord[];
  operatorCorrelationTruncated: boolean;
  record: TerminalEvidenceRecord;
  state: TerminalEvidenceState;
}) {
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  const authEvent = operatorSessionId
    ? (authEventBySessionId.get(operatorSessionId) ?? null)
    : null;
  const transcriptPath = `/api/v1/terminal-sessions/${encodeURIComponent(record.session.client_id)}/${encodeURIComponent(record.session.session_id)}/replay`;

  return (
    <section
      className="consoleDetailPanel sessionEvidenceDetailPanel"
      aria-label="Selected terminal session evidence"
    >
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>Selected terminal proof</strong>
          <small title={record.session.session_id}>
            {agentNameById.get(record.session.client_id) ??
              record.session.client_id}{" "}
            · {shortId(record.session.session_id)}
          </small>
        </span>
      </div>

      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Actor</strong>
          <span title={record.job?.actor_id ?? undefined}>
            {terminalActorLabel(record, authEventBySessionId)}
          </span>
        </span>
        <span>
          <strong>Target</strong>
          <span>
            {agentNameById.get(record.session.client_id) ??
              record.session.client_id}
          </span>
        </span>
        <span>
          <strong>Lifecycle</strong>
          <span>
            {state.label}: {state.detail}
          </span>
        </span>
        <span>
          <strong>Started</strong>
          <span>{terminalStartedDetail(record)}</span>
        </span>
        <span>
          <strong>Last activity</strong>
          <span>{formatFullTime(record.session.observed_at)}</span>
        </span>
        <span>
          <strong>Expiry</strong>
          <span>
            {canInspectOperatorAuthority
              ? terminalExpiryDetail(
                  record,
                  operatorSessions,
                  operatorCorrelationTruncated,
                )
              : "Bearer-session expiry is visible to admins only"}
          </span>
        </span>
        <span>
          <strong>Transcript link</strong>
          <span>{transcriptLabel(record.session)}</span>
        </span>
        <span>
          <strong>Authorization job</strong>
          <span>{record.session.job_id}</span>
        </span>
        <span>
          <strong>Authorization command</strong>
          <span>terminal_open</span>
        </span>
      </div>

      <div className="jobEvidenceSections sessionEvidenceSections">
        <section
          className="dashboardWidgetTable"
          aria-label="Terminal audit events for selected session"
        >
          <div className="dashboardWidgetHeader">
            <strong>Terminal audit events</strong>
            <small>
              {record.audits.length > 0
                ? `${record.audits.length} matched`
                : auditCorrelationTruncated
                  ? "none in loaded audit history"
                  : "0 matched"}
            </small>
          </div>
          {record.audits.length > 0 ? (
            record.audits.slice(0, 6).map((audit) => (
              <div
                className="dashboardWidgetRow auditEvidenceRow"
                key={audit.id}
              >
                <strong>{presentAudit(audit).actionLabel}</strong>
                <span title={terminalAuditTargetTitle(audit)}>
                  {terminalAuditTargetLabel(audit)}
                </span>
                <small title={audit.command_hash ?? undefined}>
                  {audit.command_hash
                    ? shortHash(audit.command_hash)
                    : "no hash"}
                </small>
                <small title={formatFullTime(audit.created_at)}>
                  {formatCompactTime(audit.created_at)}
                </small>
              </div>
            ))
          ) : (
            <div className="dashboardWidgetEmpty">
              {auditCorrelationTruncated
                ? "No direct row matched in the loaded audit history. Older audit or job evidence may exist."
                : "No direct audit row returned for this terminal session ID."}{" "}
              Terminal inventory and transcript references remain visible.
            </div>
          )}
        </section>

        <section
          className="dashboardWidgetTable"
          aria-label="Transcript references for selected session"
        >
          <div className="dashboardWidgetHeader">
            <strong>Transcript references</strong>
            <small>{transcriptEvidenceState(record.session).label}</small>
          </div>
          <div className="sessionEvidenceReferenceGrid">
            <span>
              <strong>Replay range</strong>
              <small>{formatOutputRange(record.session)}</small>
            </span>
            <span>
              <strong>Retained bytes</strong>
              <small>
                {formatBytes(record.session.output_retained_bytes ?? 0)}
              </small>
            </span>
            <details className="wideReference sessionEvidenceAdvanced">
              <summary>Advanced replay path</summary>
              <small>{transcriptPath}</small>
            </details>
          </div>
        </section>

        <section
          className="dashboardWidgetTable"
          aria-label="Operator auth evidence for selected session"
        >
          <div className="dashboardWidgetHeader">
            <strong>Operator auth evidence</strong>
            <small title={operatorSessionId ?? undefined}>
              {operatorSessionId
                ? shortId(operatorSessionId)
                : auditCorrelationTruncated
                  ? "not linked in loaded audit history"
                  : "not linked"}
            </small>
          </div>
          {canInspectOperatorAuthority ? (
            <div className="sessionEvidenceReferenceGrid">
              <span>
                <strong>Operator session</strong>
                <small>
                  {operatorSessionId ??
                    (auditCorrelationTruncated
                      ? "not linked in loaded audit history"
                      : "not returned")}
                </small>
              </span>
              <span>
                <strong>Auth result</strong>
                <small>
                  {authEvent?.result ??
                    (authCorrelationTruncated
                      ? "not in loaded correlation evidence"
                      : "not matched")}
                </small>
              </span>
              <span>
                <strong>Remote IP</strong>
                <small>
                  {authEvent
                    ? formatAuthRemoteIp(authEvent)
                    : authCorrelationTruncated
                      ? "not in loaded correlation evidence"
                      : formatAuthRemoteIp(authEvent)}
                </small>
              </span>
              <span>
                <strong>User agent</strong>
                <small>
                  {authEvent
                    ? formatAuthUserAgent(authEvent)
                    : authCorrelationTruncated
                      ? "not in loaded correlation evidence"
                      : formatAuthUserAgent(authEvent)}
                </small>
              </span>
              <span>
                <strong>Auth source</strong>
                <small>
                  {authEvent
                    ? formatAuthEvidenceSource(authEvent)
                    : authCorrelationTruncated
                      ? "not in loaded correlation evidence"
                      : formatAuthEvidenceSource(authEvent)}
                </small>
              </span>
            </div>
          ) : (
            <div className="dashboardWidgetEmpty">
              Operator authentication correlation is visible to admins only.
              Terminal lifecycle, audit links, and transcript evidence remain
              available above.
            </div>
          )}
        </section>
      </div>
    </section>
  );
}

function OperatorSessionEvidence({
  authEventBySessionId,
  canInspectOperatorAuthority,
  onOpenPrivilegeUnlock,
  onRevokeOperatorSession,
  operatorSessions,
  operatorSessionsTruncated,
  privilegeMaterial,
  stateById,
}: {
  authEventBySessionId: Map<string, OperatorAuthEventRecord>;
  canInspectOperatorAuthority: boolean;
  onOpenPrivilegeUnlock: () => void;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  operatorSessions: OperatorSessionRecord[];
  operatorSessionsTruncated: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  stateById: Map<string, OperatorSessionEvidenceState>;
}) {
  const [pendingRevoke, setPendingRevoke] = useState<{
    adminRisk: boolean;
    privileges: Record<
      string,
      { payloadHashHex: string; privilegeAssertion: PrivilegeAssertion }
    >;
    sessions: OperatorSessionRecord[];
  } | null>(null);
  const [reviewPending, setReviewPending] = useState(false);
  const [revokePending, setRevokePending] = useState(false);
  const [feedback, setFeedback] = useState<{
    message: string;
    tone: "danger" | "progress" | "success";
  } | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const columns = useMemo<ConsoleDataGridColumn<OperatorSessionRecord>[]>(
    () => [
      {
        id: "operator",
        header: "Operator",
        cell: (session) => (
          <span className="historyPrimary">
            <strong>{session.operator_username}</strong>
            <small title={session.id}>{shortId(session.id)}</small>
          </span>
        ),
        searchValue: (session) =>
          `${session.operator_username} ${session.id}`,
        sortValue: (session) => session.operator_username,
      },
      {
        id: "role",
        header: "Role",
        cell: (session) => (
          <span
            className={`status ${session.operator_role === "admin" ? "warn" : "neutral"}`}
          >
            {session.operator_role}
          </span>
        ),
        searchValue: (session) => session.operator_role,
      },
      {
        id: "state",
        header: "State",
        cell: (session) => {
          const state =
            stateById.get(session.id) ??
            operatorSessionEvidenceState(session);
          return (
            <span className={`status ${state.tone}`} title={state.detail}>
              {state.label}
            </span>
          );
        },
        searchValue: (session) => {
          const state =
            stateById.get(session.id) ??
            operatorSessionEvidenceState(session);
          return `${state.label} ${state.detail}`;
        },
      },
      {
        id: "created",
        header: "Created",
        cell: (session) => (
          <time
            dateTime={session.created_at}
            title={formatFullTime(session.created_at)}
          >
            {formatCompactTime(session.created_at)}
          </time>
        ),
        sortValue: (session) => session.created_at,
      },
      {
        id: "access_expiry",
        header: "Access expires",
        cell: (session) => (
          <time
            dateTime={session.expires_at}
            title={formatFullTime(session.expires_at)}
          >
            {formatCompactTime(session.expires_at)}
          </time>
        ),
        sortValue: (session) => session.expires_at,
      },
      {
        id: "refresh_expiry",
        header: "Refresh expires",
        cell: (session) => (
          <time
            dateTime={session.refresh_expires_at}
            title={formatFullTime(session.refresh_expires_at)}
          >
            {formatCompactTime(session.refresh_expires_at)}
          </time>
        ),
        sortValue: (session) => session.refresh_expires_at,
      },
      {
        id: "source",
        header: "Authentication",
        cell: (session) =>
          formatAuthEvidenceSource(authEventBySessionId.get(session.id)),
        searchValue: (session) =>
          formatAuthEvidenceSource(authEventBySessionId.get(session.id)),
      },
    ],
    [authEventBySessionId, stateById],
  );

  useEffect(() => {
    invalidateReviewGeneration();
    setPendingRevoke(null);
  }, [invalidateReviewGeneration, operatorSessions]);

  async function requestRevoke(rows: OperatorSessionRecord[]) {
    const sessions = rows.filter(
      (session) => !session.current && isOperatorSessionRevokable(session),
    );
    if (sessions.length === 0) {
      setFeedback({
        message:
          "Select at least one active, non-current bearer session to revoke.",
        tone: "danger",
      });
      return;
    }
    if (!privilegeMaterial) {
      setFeedback({
        message: "Unlock privilege to review session revocation.",
        tone: "danger",
      });
      onOpenPrivilegeUnlock();
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const adminRisk = sessions.some(
      (session) => session.operator_role === "admin",
    );
    setReviewPending(true);
    setFeedback({
      message: "Preparing session revoke review",
      tone: "progress",
    });
    try {
      await waitForReviewRender();
      const privileges = Object.fromEntries(
        await Promise.all(
          sessions.map(async (session) => {
            const payloadHashHex = await operatorDbPayloadHashHex({
              action: "operator_session.revoke",
              target: session.id,
              adminRiskAcknowledged: adminRisk,
            });
            const privilegeAssertion = await buildPrivilegeAssertion({
              intent: canonicalDbPrivilegeIntent({
                action: "operator_session.revoke",
                confirmed: true,
                payloadHash: payloadHashHex,
                resolvedTargets: [session.id],
                target: session.id,
              }),
              privilegeMaterial,
            });
            return [session.id, { payloadHashHex, privilegeAssertion }];
          }),
        ),
      );
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingRevoke({ adminRisk, privileges, sessions });
      setFeedback(null);
    } catch (error) {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setFeedback({
          message:
            error instanceof Error
              ? error.message
              : "Session revoke review failed",
          tone: "danger",
        });
      }
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewPending(false);
      }
    }
  }

  async function confirmRevoke() {
    if (!pendingRevoke) {
      return;
    }
    setRevokePending(true);
    setFeedback({ message: "Revoking sessions", tone: "progress" });
    try {
      for (const session of pendingRevoke.sessions) {
        await onRevokeOperatorSession(
          session.id,
          pendingRevoke.adminRisk,
          pendingRevoke.privileges[session.id].privilegeAssertion,
        );
      }
      const count = pendingRevoke.sessions.length;
      setPendingRevoke(null);
      setFeedback({
        message: `Revoked ${count} bearer session${count === 1 ? "" : "s"}.`,
        tone: "success",
      });
    } catch (error) {
      setFeedback({
        message:
          error instanceof Error ? error.message : "Session revoke failed",
        tone: "danger",
      });
    } finally {
      setRevokePending(false);
    }
  }

  return (
    <section
      className="dashboardWidgetTable operatorSessionEvidenceTable"
      aria-label="Operator session evidence"
    >
      <div className="dashboardWidgetHeader">
        <strong>Operator session evidence</strong>
        <small>
          {canInspectOperatorAuthority
            ? `${formatLowerBoundCount(
                operatorSessions.length,
                operatorSessionsTruncated,
              )}${operatorSessionsTruncated ? " loaded" : ""} bearer session${operatorSessions.length === 1 ? "" : "s"} · select active non-current sessions to revoke`
            : "Admin only"}
        </small>
      </div>
      {!canInspectOperatorAuthority ? (
        <div className="dashboardWidgetEmpty">
          Bearer-session inventory and operator authentication records are
          visible to admins only. No empty-state conclusion is inferred.
        </div>
      ) : (
        <>
          <ActionFeedback
            className="localActionFeedback"
            message={feedback?.message ?? null}
            tone={feedback?.tone}
          />
          <ConsoleDataGrid
            actions={[
              {
                label: "Revoke",
                description: (rows) =>
                  rows.length === 1
                    ? `Revoke the bearer session for ${rows[0].operator_username}.`
                    : `Revoke ${rows.length} selected bearer sessions.`,
                disabled: (rows) =>
                  reviewPending ||
                  revokePending ||
                  rows.length === 0 ||
                  rows.some(
                    (session) =>
                      session.current ||
                      !isOperatorSessionRevokable(session),
                  ),
                icon: <UserX size={14} />,
                onSelect: (rows) => void requestRevoke(rows),
                tone: "danger",
              },
            ]}
            columns={columns}
            defaultPageSize={12}
            empty="No bearer session evidence returned by the operator-session API."
            expandOnRowClick
            getRowId={(session) => session.id}
            itemLabel="sessions"
            renderExpandedRow={(session) => (
              <div className="consoleInlineDetailGrid">
                <span>
                  <strong>Session ID</strong>
                  <span className="monoValue">{session.id}</span>
                </span>
                <span>
                  <strong>Operator ID</strong>
                  <span className="monoValue">{session.operator_id}</span>
                </span>
                <span>
                  <strong>Authentication source</strong>
                  <span>
                    {formatAuthEvidenceSource(
                      authEventBySessionId.get(session.id),
                    )}
                  </span>
                </span>
                <span>
                  <strong>Remote IP</strong>
                  <span>
                    {formatAuthRemoteIp(
                      authEventBySessionId.get(session.id) ?? null,
                    )}
                  </span>
                </span>
                <span>
                  <strong>User agent</strong>
                  <span>
                    {formatAuthUserAgent(
                      authEventBySessionId.get(session.id) ?? null,
                    )}
                  </span>
                </span>
                <span>
                  <strong>Current browser</strong>
                  <span>{session.current ? "Yes" : "No"}</span>
                </span>
              </div>
            )}
            rows={operatorSessions}
            rowsTruncated={operatorSessionsTruncated}
            searchPlaceholder="Search operator, role, state, session, or authentication evidence"
            singleExpandedRow
            storageKey="vpsman.audit.operatorSessions"
            title="Operator bearer sessions"
          />
          <ConfirmationPrompt
            confirmLabel={
              (pendingRevoke?.sessions.length ?? 0) === 1
                ? "Revoke session"
                : "Revoke sessions"
            }
            detail={
              pendingRevoke?.adminRisk
                ? "This revokes one or more admin bearer sessions. The current browser session cannot be selected here."
                : "This revokes the selected non-current bearer sessions."
            }
            items={[
              {
                label: "Sessions",
                value: pendingRevoke?.sessions.length ?? 0,
              },
              {
                label: "Operators",
                value:
                  pendingRevoke?.sessions
                    .map((session) => session.operator_username)
                    .join(", ") ?? "-",
              },
              {
                label: "Admin sessions",
                value:
                  pendingRevoke?.sessions.filter(
                    (session) => session.operator_role === "admin",
                  ).length ?? 0,
              },
            ]}
            onCancel={() => setPendingRevoke(null)}
            onConfirm={() => void confirmRevoke()}
            open={pendingRevoke !== null}
            pending={revokePending}
            title={
              pendingRevoke?.adminRisk
                ? "Confirm admin session revoke"
                : "Confirm session revoke"
            }
            tone="danger"
          />
        </>
      )}
    </section>
  );
}

function isOperatorSessionRevokable(
  session: OperatorSessionRecord,
): boolean {
  return !session.revoked && !isPast(session.refresh_expires_at);
}

function terminalEvidenceState(
  session: TerminalSessionRecord,
): TerminalEvidenceState {
  if (!isTerminalOpen(session)) {
    return {
      detail: session.close_reason
        ? `Closed by ${session.close_reason}; last event ${session.last_event}.`
        : `Closed; last event ${session.last_event}.`,
      label: "Closed",
      open: false,
      tone: "neutral",
    };
  }
  const observedMs = parseTimeMs(session.observed_at);
  if (observedMs === null) {
    return {
      detail: "No valid last-activity timestamp is available.",
      label: "State unknown",
      open: false,
      tone: "warn",
    };
  }
  const idleTimeoutMs = Math.max(0, session.idle_timeout_secs ?? 0) * 1000;
  const staleAfterMs = Math.max(idleTimeoutMs * 2, TERMINAL_STALE_FLOOR_MS);
  if (Date.now() - observedMs > staleAfterMs) {
    return {
      detail: `Last activity was ${formatTime(session.observed_at)}; reported state is ${session.state}.`,
      label: "Stale state",
      open: false,
      tone: "warn",
    };
  }
  return {
    detail: `Live terminal state reported at ${formatTime(session.observed_at)}.`,
    label: "Open",
    open: true,
    tone: "ok",
  };
}

function operatorSessionEvidenceState(
  session: OperatorSessionRecord,
): OperatorSessionEvidenceState {
  if (session.revoked) {
    return {
      detail: session.revoked_at
        ? `Revoked at ${formatTime(session.revoked_at)}.`
        : "This bearer session is revoked.",
      label: "Revoked",
      tone: "neutral",
    };
  }
  const accessExpired = isPast(session.expires_at);
  const refreshExpired = isPast(session.refresh_expires_at);
  if (refreshExpired) {
    return {
      detail: `Refresh expired at ${formatTime(session.refresh_expires_at)}.`,
      label: "Expired",
      tone: "warn",
    };
  }
  if (accessExpired) {
    return {
      detail: `Access expired at ${formatTime(session.expires_at)}; refresh remains available until ${formatTime(session.refresh_expires_at)}.`,
      label: "Refreshable",
      tone: "info",
    };
  }
  if (session.current) {
    return {
      detail: "This is the current console bearer session.",
      label: "Current",
      tone: "info",
    };
  }
  return {
    detail: `Access expires ${formatTime(session.expires_at)}.`,
    label: "Active",
    tone: "ok",
  };
}

function transcriptEvidenceState(session: TerminalSessionRecord): {
  label: string;
  replayable: boolean;
} {
  if (session.output_next_seq == null) {
    return { label: "No transcript range", replayable: false };
  }
  if (session.output_replay_truncated) {
    return { label: "Transcript truncated", replayable: true };
  }
  return { label: "Replayable transcript", replayable: true };
}

function terminalStateSort(state: TerminalEvidenceState): number {
  if (state.label === "Open") {
    return 0;
  }
  if (state.label === "Stale state") {
    return 1;
  }
  if (state.label === "State unknown") {
    return 2;
  }
  return 3;
}

function isPast(value: string): boolean {
  const timestamp = parseTimeMs(value);
  return timestamp !== null && timestamp <= Date.now();
}

function parseTimeMs(value: string): number | null {
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? timestamp : null;
}

function auditMatchesTerminalSession(
  audit: AuditLogRecord,
  session: TerminalSessionRecord,
): boolean {
  const presentation = presentAudit(audit);
  if (presentation.terminalSessionId === session.session_id) {
    return true;
  }
  if (
    presentation.evidenceReferences.some(
      (reference) =>
        reference.kind === "Job" && reference.value === session.job_id,
    )
  ) {
    return true;
  }
  return false;
}

function terminalActorLabel(
  record: TerminalEvidenceRecord,
  authEventBySessionId: Map<string, OperatorAuthEventRecord>,
): string {
  if (record.job?.actor_id) {
    const dispatchActor = record.audits.find(
      (audit) =>
        audit.actor_id === record.job?.actor_id &&
        audit.action === "job.dispatch_requested",
    );
    return dispatchActor
      ? presentAudit(dispatchActor).actorLabel
      : `Operator ${shortId(record.job.actor_id)}`;
  }
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  const authEvent = operatorSessionId
    ? authEventBySessionId.get(operatorSessionId)
    : null;
  return authEvent?.username ?? shortId(record.job?.actor_id) ?? "not reported";
}

function terminalActorDetail(
  record: TerminalEvidenceRecord,
  authEventBySessionId: Map<string, OperatorAuthEventRecord>,
): string {
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  const authEvent = operatorSessionId
    ? (authEventBySessionId.get(operatorSessionId) ?? null)
    : null;
  if (authEvent) {
    return formatAuthEvidenceSource(authEvent);
  }
  if (operatorSessionId) {
    return `bearer session ${shortId(operatorSessionId)}`;
  }
  if (record.job?.actor_id) {
    return `job actor ${shortId(record.job.actor_id)}`;
  }
  return "operator source not reported";
}

function terminalActorEvidenceTitle(
  record: TerminalEvidenceRecord,
): string | undefined {
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  if (operatorSessionId) {
    return operatorSessionId;
  }
  return record.job?.actor_id ?? undefined;
}

function terminalOperatorSessionId(audits: AuditLogRecord[]): string | null {
  for (const audit of audits) {
    const value = presentAudit(audit).operatorSessionId;
    if (value) {
      return value;
    }
  }
  return null;
}

function terminalAuditTargetLabel(audit: AuditLogRecord): string {
  return presentAudit(audit).targetLabel;
}

function terminalAuditTargetTitle(audit: AuditLogRecord): string {
  const presentation = presentAudit(audit);
  return `${presentation.targetLabel} · ${presentation.targetDetail}`;
}

function terminalStartedAt(record: TerminalEvidenceRecord): string | null {
  return (
    record.session.opened_at ??
    record.audits
      .filter((audit) => audit.action === "terminal.open")
      .map((audit) => audit.created_at)
      .sort((left, right) => left.localeCompare(right))[0] ?? null
  );
}

function terminalStartedLabel(record: TerminalEvidenceRecord): string {
  const startedAt = terminalStartedAt(record);
  return startedAt
    ? formatCompactTime(startedAt)
    : "Open time unavailable";
}

function terminalStartedDetail(record: TerminalEvidenceRecord): string {
  const startedAt = terminalStartedAt(record);
  return startedAt
    ? formatFullTime(startedAt)
    : "This retained session predates open-time evidence; last activity remains available.";
}

function terminalExpiryLabel(
  record: TerminalEvidenceRecord,
  operatorSessions: OperatorSessionRecord[],
  operatorSessionsTruncated = false,
): string {
  const operatorSession = operatorSessionForTerminal(record, operatorSessions);
  if (!operatorSession) {
    return operatorSessionsTruncated
      ? "Linked expiry unavailable in loaded evidence"
      : "Terminal expiry not reported";
  }
  const state = operatorSessionEvidenceState(operatorSession);
  return `${state.label} refresh ${formatCompactTime(operatorSession.refresh_expires_at)}`;
}

function terminalExpiryDetail(
  record: TerminalEvidenceRecord,
  operatorSessions: OperatorSessionRecord[],
  operatorSessionsTruncated = false,
): string {
  const operatorSession = operatorSessionForTerminal(record, operatorSessions);
  if (!operatorSession) {
    return operatorSessionsTruncated
      ? "Linked bearer expiry is outside the loaded correlation evidence"
      : "Terminal expiry and linked bearer expiry are unavailable";
  }
  const state = operatorSessionEvidenceState(operatorSession);
  return `${state.label} bearer session; access ${formatFullTime(operatorSession.expires_at)}; refresh ${formatFullTime(operatorSession.refresh_expires_at)}`;
}

function terminalExpirySort(
  record: TerminalEvidenceRecord,
  operatorSessions: OperatorSessionRecord[],
): string {
  return (
    operatorSessionForTerminal(record, operatorSessions)?.refresh_expires_at ??
    record.session.observed_at
  );
}

function operatorSessionForTerminal(
  record: TerminalEvidenceRecord,
  operatorSessions: OperatorSessionRecord[],
): OperatorSessionRecord | null {
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  if (!operatorSessionId) {
    return null;
  }
  return (
    operatorSessions.find((session) => session.id === operatorSessionId) ?? null
  );
}

function isDemoAuthEvent(event: OperatorAuthEventRecord): boolean {
  return (
    isLocalTestIp(event.remote_ip) ||
    isDocumentationTestIp(event.remote_ip) ||
    isTestAutomationUserAgent(event.user_agent)
  );
}

function formatAuthRemoteIp(
  event: OperatorAuthEventRecord | null | undefined,
): string {
  if (!event?.remote_ip) {
    return "not recorded";
  }
  if (isLocalTestIp(event.remote_ip)) {
    return `${event.remote_ip} (local test)`;
  }
  if (isDocumentationTestIp(event.remote_ip)) {
    return `${event.remote_ip} (documentation/test IP)`;
  }
  return event.remote_ip;
}

function formatAuthUserAgent(
  event: OperatorAuthEventRecord | null | undefined,
): string {
  if (!event?.user_agent) {
    return "not recorded";
  }
  if (isTestAutomationUserAgent(event.user_agent)) {
    return `${event.user_agent} (test automation)`;
  }
  return event.user_agent;
}

function formatAuthEvidenceSource(
  event: OperatorAuthEventRecord | null | undefined,
): string {
  if (!event) {
    return "not linked";
  }
  const labels = [
    isLocalTestIp(event.remote_ip) ? "local test IP" : null,
    isDocumentationTestIp(event.remote_ip) ? "documentation/test IP" : null,
    isTestAutomationUserAgent(event.user_agent) ? "test automation" : null,
  ].filter(Boolean);
  if (labels.length > 0) {
    return `Demo/test: ${labels.join(", ")}`;
  }
  return "Production-like auth signal";
}

function isLocalTestIp(value: string | null | undefined): boolean {
  if (!value) {
    return false;
  }
  const normalized = value.trim().toLowerCase();
  return (
    normalized === "localhost" ||
    normalized === "::1" ||
    normalized.startsWith("127.")
  );
}

function isDocumentationTestIp(value: string | null | undefined): boolean {
  if (!value) {
    return false;
  }
  const normalized = value.trim();
  return (
    normalized.startsWith("192.0.2.") ||
    normalized.startsWith("198.51.100.") ||
    normalized.startsWith("203.0.113.")
  );
}

function isTestAutomationUserAgent(value: string | null | undefined): boolean {
  return Boolean(value?.toLowerCase().includes("playwright"));
}

function metadataText(metadata: JsonValue, keys: string[]): string | null {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) {
    return null;
  }
  const record = metadata as Record<string, JsonValue>;
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) {
      return value;
    }
  }
  return null;
}

function terminalKey(session: TerminalSessionRecord): string {
  return `${session.client_id}:${session.session_id}`;
}

function isTerminalOpen(session: TerminalSessionRecord): boolean {
  return session.state === "opening" || session.state === "open";
}

function sessionLifecycleLabel(session: TerminalSessionRecord): string {
  if (isTerminalOpen(session)) {
    return `${session.state}; last ${session.last_event}`;
  }
  return `${session.state}; closed by ${session.close_reason ?? "not reported"}`;
}

function transcriptLabel(session: TerminalSessionRecord): string {
  const transcript = transcriptEvidenceState(session);
  if (!transcript.replayable) {
    return transcript.label;
  }
  return `${transcript.label}: ${formatOutputRange(session)} · ${formatBytes(session.output_retained_bytes ?? 0)}`;
}

function formatOutputRange(session: TerminalSessionRecord): string {
  const first = session.output_retained_first_seq ?? session.output_first_seq;
  const next = session.output_next_seq;
  if (first == null || next == null) {
    return "not retained";
  }
  return `seq ${first}-${Math.max(first, next - 1)}`;
}

function formatArgv(argv: string[]): string {
  return argv.join(" ");
}

function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}
