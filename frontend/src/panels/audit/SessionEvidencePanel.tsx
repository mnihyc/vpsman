import { History, KeyRound, Link2, TerminalSquare } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import type {
  AgentView,
  AuditLogRecord,
  JobHistoryRecord,
  JsonValue,
  OperatorAuthEventRecord,
  OperatorSessionRecord,
} from "../../types";
import type { TerminalSessionRecord } from "../../typesTerminal";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  metadataOperator,
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
  jobs,
  loading,
  onRefresh,
  operatorAuthEvents,
  operatorSessions,
  terminalSessions,
}: {
  agents: AgentView[];
  audits: AuditLogRecord[];
  jobs: JobHistoryRecord[];
  loading: boolean;
  onRefresh: () => void;
  operatorAuthEvents: OperatorAuthEventRecord[];
  operatorSessions: OperatorSessionRecord[];
  terminalSessions: TerminalSessionRecord[];
}) {
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
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
        const job = jobsById.get(session.last_job_id) ?? null;
        return {
          audits: audits
            .filter((audit) => auditMatchesTerminalSession(audit, session, job))
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
            <strong>{terminalActorLabel(row, authEventBySessionId)}</strong>
            <small>{terminalActorDetail(row, authEventBySessionId)}</small>
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
        searchValue: (row) => terminalExpiryLabel(row, operatorSessions),
        size: 170,
        sortValue: (row) => terminalExpirySort(row, operatorSessions),
        cell: (row) => terminalExpiryLabel(row, operatorSessions),
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
    [agentNameById, authEventBySessionId, operatorSessions, terminalStateByKey],
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
            Read-only terminal, transcript, operator-session, and authentication
            evidence for security review.
          </small>
        </span>
        <button
          className="secondaryAction compactAction"
          onClick={onRefresh}
          type="button"
        >
          Refresh
        </button>
      </div>

      <div className="metricGrid" aria-label="Session evidence summary">
        <div className="metricCard">
          <TerminalSquare size={18} />
          <span>
            <strong>{terminalSessions.length}</strong>
            <small>Terminal sessions</small>
          </span>
        </div>
        <div className="metricCard">
          <Link2 size={18} />
          <span>
            <strong>{matchedSessions}</strong>
            <small>Audit-linked terminals</small>
          </span>
        </div>
        <div className="metricCard">
          <TerminalSquare size={18} />
          <span>
            <strong>{openSessions}</strong>
            <small>
              {staleTerminalSessions > 0
                ? `${staleTerminalSessions} stale terminal states hidden from open count`
                : "Open terminals"}
            </small>
          </span>
        </div>
        <div className="metricCard">
          <History size={18} />
          <span>
            <strong>{replayableSessions}</strong>
            <small>Replayable transcripts</small>
          </span>
        </div>
        <div className="metricCard">
          <History size={18} />
          <span>
            <strong>{formatBytes(retainedBytes)}</strong>
            <small>Retained transcript bytes</small>
          </span>
        </div>
        <div className="metricCard">
          <KeyRound size={18} />
          <span>
            <strong>{operatorSessions.length}</strong>
            <small>
              {expiredOperatorSessions > 0
                ? `${expiredOperatorSessions} expired bearer sessions`
                : "Bearer sessions"}
            </small>
          </span>
        </div>
        <div className="metricCard">
          <KeyRound size={18} />
          <span>
            <strong>{demoAuthSignals}</strong>
            <small>Demo/test auth signals</small>
          </span>
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
        rows={evidenceRows}
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
          authEventBySessionId={authEventBySessionId}
          operatorSessions={operatorSessions}
          state={
            terminalStateByKey.get(terminalKey(selectedRecord.session)) ??
            terminalEvidenceState(selectedRecord.session)
          }
          record={selectedRecord}
        />
      )}

      <OperatorSessionEvidence
        authEventBySessionId={authEventBySessionId}
        operatorSessions={operatorSessions}
        stateById={operatorStateById}
      />
    </section>
  );
}

function SelectedSessionEvidence({
  agentNameById,
  authEventBySessionId,
  operatorSessions,
  record,
  state,
}: {
  agentNameById: Map<string, string>;
  authEventBySessionId: Map<string, OperatorAuthEventRecord>;
  operatorSessions: OperatorSessionRecord[];
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
          <small>
            {agentNameById.get(record.session.client_id) ??
              record.session.client_id}{" "}
            · {shortId(record.session.session_id)}
          </small>
        </span>
      </div>

      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Actor</strong>
          <span>{terminalActorLabel(record, authEventBySessionId)}</span>
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
          <span>{terminalExpiryDetail(record, operatorSessions)}</span>
        </span>
        <span>
          <strong>Transcript link</strong>
          <span>{transcriptLabel(record.session)}</span>
        </span>
        <span>
          <strong>Last job</strong>
          <span>{record.session.last_job_id}</span>
        </span>
        <span>
          <strong>Last command</strong>
          <span>{record.session.last_command_type}</span>
        </span>
      </div>

      <div className="jobEvidenceSections sessionEvidenceSections">
        <section
          className="dashboardWidgetTable"
          aria-label="Terminal audit events for selected session"
        >
          <div className="dashboardWidgetHeader">
            <strong>Terminal audit events</strong>
            <small>{record.audits.length} matched</small>
          </div>
          {record.audits.length > 0 ? (
            record.audits.slice(0, 6).map((audit) => (
              <div
                className="dashboardWidgetRow auditEvidenceRow"
                key={audit.id}
              >
                <strong>{audit.action}</strong>
                <span title={audit.target}>
                  {terminalAuditTargetLabel(audit)}
                </span>
                <small>
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
              No direct audit row returned for this terminal session ID.
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
            <small>
              {operatorSessionId ? shortId(operatorSessionId) : "not linked"}
            </small>
          </div>
          <div className="sessionEvidenceReferenceGrid">
            <span>
              <strong>Operator session</strong>
              <small>{operatorSessionId ?? "not returned"}</small>
            </span>
            <span>
              <strong>Auth result</strong>
              <small>{authEvent?.result ?? "not matched"}</small>
            </span>
            <span>
              <strong>Remote IP</strong>
              <small>{formatAuthRemoteIp(authEvent)}</small>
            </span>
            <span>
              <strong>User agent</strong>
              <small>{formatAuthUserAgent(authEvent)}</small>
            </span>
            <span>
              <strong>Auth source</strong>
              <small>{formatAuthEvidenceSource(authEvent)}</small>
            </span>
          </div>
        </section>
      </div>
    </section>
  );
}

function OperatorSessionEvidence({
  authEventBySessionId,
  operatorSessions,
  stateById,
}: {
  authEventBySessionId: Map<string, OperatorAuthEventRecord>;
  operatorSessions: OperatorSessionRecord[];
  stateById: Map<string, OperatorSessionEvidenceState>;
}) {
  return (
    <section
      className="dashboardWidgetTable operatorSessionEvidenceTable"
      aria-label="Operator session evidence"
    >
      <div className="dashboardWidgetHeader">
        <strong>Operator session evidence</strong>
        <small>
          {operatorSessions.length} bearer sessions · created and refresh expiry
          shown
        </small>
      </div>
      {operatorSessions.length > 0 ? (
        operatorSessions.slice(0, 6).map((session) => {
          const authEvent = authEventBySessionId.get(session.id);
          const state =
            stateById.get(session.id) ?? operatorSessionEvidenceState(session);
          return (
            <div
              className="dashboardWidgetRow operatorSessionEvidenceRow"
              key={session.id}
            >
              <strong>{session.operator_username}</strong>
              <span>{session.operator_role}</span>
              <small className={`status ${state.tone}`} title={state.detail}>
                {state.label}
              </small>
              <small title={formatFullTime(session.created_at)}>
                Created {formatCompactTime(session.created_at)}
              </small>
              <small title={formatFullTime(session.refresh_expires_at)}>
                Refresh expiry {formatCompactTime(session.refresh_expires_at)}
              </small>
              <small>{formatAuthEvidenceSource(authEvent)}</small>
            </div>
          );
        })
      ) : (
        <div className="dashboardWidgetEmpty">
          No bearer session evidence returned by the operator-session API.
        </div>
      )}
    </section>
  );
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
      detail: "The backend did not provide a valid last-activity timestamp.",
      label: "State unknown",
      open: false,
      tone: "warn",
    };
  }
  const idleTimeoutMs = Math.max(0, session.idle_timeout_secs ?? 0) * 1000;
  const staleAfterMs = Math.max(idleTimeoutMs * 2, TERMINAL_STALE_FLOOR_MS);
  if (Date.now() - observedMs > staleAfterMs) {
    return {
      detail: `Last activity was ${formatTime(session.observed_at)}; raw backend state is ${session.state}.`,
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
  if (accessExpired || refreshExpired) {
    return {
      detail: refreshExpired
        ? `Refresh expired at ${formatTime(session.refresh_expires_at)}.`
        : `Access expired at ${formatTime(session.expires_at)}.`,
      label: "Expired",
      tone: "warn",
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
  const retainedBytes = session.output_retained_bytes ?? 0;
  if (session.output_replay_truncated) {
    return { label: "Transcript truncated", replayable: true };
  }
  if (retainedBytes < 128) {
    return {
      label: "Trace only; small retained transcript",
      replayable: false,
    };
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
  job: JobHistoryRecord | null,
): boolean {
  const text =
    `${audit.target} ${JSON.stringify(audit.metadata)}`.toLowerCase();
  if (
    text.includes(session.session_id.toLowerCase()) ||
    text.includes(session.last_job_id.toLowerCase())
  ) {
    return true;
  }
  return Boolean(job?.payload_hash && audit.command_hash === job.payload_hash);
}

function terminalActorLabel(
  record: TerminalEvidenceRecord,
  authEventBySessionId: Map<string, OperatorAuthEventRecord>,
): string {
  const auditActor = record.audits
    .map(
      (audit) =>
        metadataOperator(audit.metadata) ??
        metadataText(audit.metadata, [
          "operator_username",
          "username",
          "operator_id",
          "actor_id",
        ]),
    )
    .find(Boolean);
  if (auditActor) {
    return auditActor;
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

function terminalOperatorSessionId(audits: AuditLogRecord[]): string | null {
  for (const audit of audits) {
    const value = metadataText(audit.metadata, [
      "operator_session_id",
      "session_id",
    ]);
    if (value) {
      return value;
    }
  }
  return null;
}

function terminalAuditTargetLabel(audit: AuditLogRecord): string {
  const clientId = metadataText(audit.metadata, ["client_id"]);
  const terminalSessionId = metadataText(audit.metadata, [
    "terminal_session_id",
  ]);
  if (clientId || terminalSessionId) {
    return [
      clientId ?? "terminal",
      terminalSessionId ? shortId(terminalSessionId) : null,
    ]
      .filter(Boolean)
      .join(" · ");
  }
  return audit.target.replace(/^terminal:/, "");
}

function terminalStartedAt(record: TerminalEvidenceRecord): string | null {
  return (
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
    : "Terminal start not reported";
}

function terminalStartedDetail(record: TerminalEvidenceRecord): string {
  const startedAt = terminalStartedAt(record);
  return startedAt
    ? formatFullTime(startedAt)
    : "Terminal start not reported by backend or audit ledger";
}

function terminalExpiryLabel(
  record: TerminalEvidenceRecord,
  operatorSessions: OperatorSessionRecord[],
): string {
  const operatorSession = operatorSessionForTerminal(record, operatorSessions);
  if (!operatorSession) {
    return "Terminal expiry not reported";
  }
  const state = operatorSessionEvidenceState(operatorSession);
  return `${state.label} refresh ${formatCompactTime(operatorSession.refresh_expires_at)}`;
}

function terminalExpiryDetail(
  record: TerminalEvidenceRecord,
  operatorSessions: OperatorSessionRecord[],
): string {
  const operatorSession = operatorSessionForTerminal(record, operatorSessions);
  if (!operatorSession) {
    return "Terminal expiry not reported by backend; linked bearer expiry unavailable";
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
  return !session.session_exited && session.state !== "closed";
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
