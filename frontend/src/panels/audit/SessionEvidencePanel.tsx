import { History, KeyRound, Link2, TerminalSquare } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  terminalSessionStateBadgeClass,
} from "../../jobStatusPresentation";
import type {
  AgentView,
  AuditLogRecord,
  JobHistoryRecord,
  JsonValue,
  OperatorAuthEventRecord,
  OperatorSessionRecord,
} from "../../types";
import type { TerminalSessionRecord } from "../../typesTerminal";
import { formatTime, metadataOperator, shortHash, shortId } from "../../utils";

type TerminalEvidenceRecord = {
  audits: AuditLogRecord[];
  job: JobHistoryRecord | null;
  session: TerminalSessionRecord;
};

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
    () => new Map(agents.map((agent) => [agent.id, agent.display_name || agent.id])),
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
            .sort((left, right) => right.created_at.localeCompare(left.created_at)),
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
  const openSessions = terminalSessions.filter((session) => isTerminalOpen(session)).length;
  const replayableSessions = terminalSessions.filter((session) => session.output_next_seq !== null).length;
  const retainedBytes = terminalSessions.reduce(
    (total, session) => total + (session.output_retained_bytes ?? 0),
    0,
  );
  const matchedSessions = evidenceRows.filter((row) => row.audits.length > 0).length;

  const columns = useMemo<ConsoleDataGridColumn<TerminalEvidenceRecord>[]>(
    () => [
      {
        id: "session",
        header: "Terminal session",
        minSize: 190,
        searchValue: (row) => `${row.session.session_id} ${row.session.argv.join(" ")}`,
        size: 230,
        sortValue: (row) => row.session.observed_at,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatArgv(row.session.argv) || row.session.last_command_type}</strong>
            <small>{shortId(row.session.session_id)}</small>
          </span>
        ),
      },
      {
        id: "actor",
        header: "Actor",
        minSize: 140,
        searchValue: (row) => terminalActorLabel(row, authEventBySessionId),
        size: 150,
        sortValue: (row) => terminalActorLabel(row, authEventBySessionId),
        cell: (row) => terminalActorLabel(row, authEventBySessionId),
      },
      {
        id: "target",
        header: "Target VPS",
        minSize: 130,
        searchValue: (row) => `${row.session.client_id} ${agentNameById.get(row.session.client_id) ?? ""}`,
        size: 150,
        sortValue: (row) => agentNameById.get(row.session.client_id) ?? row.session.client_id,
        cell: (row) => agentNameById.get(row.session.client_id) ?? row.session.client_id,
      },
      {
        id: "state",
        header: "State",
        minSize: 120,
        searchValue: (row) => `${row.session.state} ${row.session.last_status}`,
        size: 130,
        sortValue: (row) => row.session.state,
        cell: (row) => (
          <span className={`status ${terminalSessionStateBadgeClass(row.session.state)}`}>
            {row.session.state}
          </span>
        ),
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
      {
        id: "observed",
        header: "Observed",
        minSize: 150,
        searchValue: (row) => row.session.observed_at,
        size: 170,
        sortValue: (row) => row.session.observed_at,
        cell: (row) => formatTime(row.session.observed_at),
      },
    ],
    [agentNameById, authEventBySessionId],
  );

  return (
    <section className="fleetPanel auditSessionEvidencePanel" aria-label="Audit session evidence">
      <div className="sectionHeader">
        <span>
          <h2>Session evidence</h2>
          <small>
            Read-only terminal, transcript, operator-session, and authentication evidence for security review.
          </small>
        </span>
        <button className="secondaryAction compactAction" onClick={onRefresh} type="button">
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
            <small>Open terminals</small>
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
            <small>Bearer sessions</small>
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
            <span>Terminal open, input, replay, and close evidence will appear here after remote operations run.</span>
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
          record={selectedRecord}
        />
      )}

      <OperatorSessionEvidence
        authEventBySessionId={authEventBySessionId}
        operatorSessions={operatorSessions}
      />
    </section>
  );
}

function SelectedSessionEvidence({
  agentNameById,
  authEventBySessionId,
  record,
}: {
  agentNameById: Map<string, string>;
  authEventBySessionId: Map<string, OperatorAuthEventRecord>;
  record: TerminalEvidenceRecord;
}) {
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  const authEvent = operatorSessionId
    ? authEventBySessionId.get(operatorSessionId) ?? null
    : null;
  const transcriptPath = `/api/v1/terminal-sessions/${encodeURIComponent(record.session.client_id)}/${encodeURIComponent(record.session.session_id)}/replay`;

  return (
    <section className="consoleDetailPanel sessionEvidenceDetailPanel" aria-label="Selected terminal session evidence">
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>Selected terminal proof</strong>
          <small>
            {agentNameById.get(record.session.client_id) ?? record.session.client_id} · {shortId(record.session.session_id)}
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
          <span>{agentNameById.get(record.session.client_id) ?? record.session.client_id}</span>
        </span>
        <span>
          <strong>Lifecycle</strong>
          <span>{sessionLifecycleLabel(record.session)}</span>
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
        <section className="dashboardWidgetTable" aria-label="Terminal audit events for selected session">
          <div className="dashboardWidgetHeader">
            <strong>Terminal audit events</strong>
            <small>{record.audits.length} matched</small>
          </div>
          {record.audits.length > 0 ? (
            record.audits.slice(0, 6).map((audit) => (
              <div className="dashboardWidgetRow auditEvidenceRow" key={audit.id}>
                <strong>{audit.action}</strong>
                <span>{audit.target}</span>
                <small>{audit.command_hash ? shortHash(audit.command_hash) : "no hash"}</small>
                <small>{formatTime(audit.created_at)}</small>
              </div>
            ))
          ) : (
            <div className="dashboardWidgetEmpty">
              No direct audit row returned for this terminal session ID. Terminal inventory and transcript references remain visible.
            </div>
          )}
        </section>

        <section className="dashboardWidgetTable" aria-label="Transcript references for selected session">
          <div className="dashboardWidgetHeader">
            <strong>Transcript references</strong>
            <small>{record.session.output_replay_truncated ? "truncated" : "retained"}</small>
          </div>
          <div className="sessionEvidenceReferenceGrid">
            <span>
              <strong>Replay range</strong>
              <small>{formatOutputRange(record.session)}</small>
            </span>
            <span>
              <strong>Retained bytes</strong>
              <small>{formatBytes(record.session.output_retained_bytes ?? 0)}</small>
            </span>
            <span className="wideReference">
              <strong>Replay API</strong>
              <small>{transcriptPath}</small>
            </span>
          </div>
        </section>

        <section className="dashboardWidgetTable" aria-label="Operator auth evidence for selected session">
          <div className="dashboardWidgetHeader">
            <strong>Operator auth evidence</strong>
            <small>{operatorSessionId ? shortId(operatorSessionId) : "not linked"}</small>
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
              <small>{authEvent?.remote_ip ?? "not recorded"}</small>
            </span>
            <span>
              <strong>User agent</strong>
              <small>{authEvent?.user_agent ?? "not recorded"}</small>
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
}: {
  authEventBySessionId: Map<string, OperatorAuthEventRecord>;
  operatorSessions: OperatorSessionRecord[];
}) {
  return (
    <section className="dashboardWidgetTable operatorSessionEvidenceTable" aria-label="Operator session evidence">
      <div className="dashboardWidgetHeader">
        <strong>Operator session evidence</strong>
        <small>{operatorSessions.length} bearer sessions</small>
      </div>
      {operatorSessions.length > 0 ? (
        operatorSessions.slice(0, 6).map((session) => {
          const authEvent = authEventBySessionId.get(session.id);
          return (
            <div className="dashboardWidgetRow operatorSessionEvidenceRow" key={session.id}>
              <strong>{session.operator_username}</strong>
              <span>{session.operator_role}</span>
              <small>{session.current ? "current" : session.revoked ? "revoked" : "active"}</small>
              <small>{authEvent?.remote_ip ?? "IP not recorded"}</small>
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

function auditMatchesTerminalSession(
  audit: AuditLogRecord,
  session: TerminalSessionRecord,
  job: JobHistoryRecord | null,
): boolean {
  const text = `${audit.target} ${JSON.stringify(audit.metadata)}`.toLowerCase();
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
    .map((audit) =>
      metadataOperator(audit.metadata) ??
      metadataText(audit.metadata, ["operator_username", "username", "operator_id", "actor_id"]),
    )
    .find(Boolean);
  if (auditActor) {
    return auditActor;
  }
  const operatorSessionId = terminalOperatorSessionId(record.audits);
  const authEvent = operatorSessionId ? authEventBySessionId.get(operatorSessionId) : null;
  return authEvent?.username ?? shortId(record.job?.actor_id) ?? "not reported";
}

function terminalOperatorSessionId(audits: AuditLogRecord[]): string | null {
  for (const audit of audits) {
    const value = metadataText(audit.metadata, ["operator_session_id", "session_id"]);
    if (value) {
      return value;
    }
  }
  return null;
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
  if (session.output_next_seq == null) {
    return "no transcript range";
  }
  return `${formatOutputRange(session)} · ${formatBytes(session.output_retained_bytes ?? 0)}`;
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
