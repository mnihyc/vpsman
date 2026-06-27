import { ClipboardCheck, ExternalLink, FileText, Link2, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  jobStatusBadgeClass,
  jobTargetStatusBadgeClass,
} from "../../jobStatusPresentation";
import type {
  AgentView,
  AuditLogRecord,
  JobHistoryRecord,
  JobOutputRecord,
  JobTargetRecord,
  JsonValue,
} from "../../types";
import {
  decodeOutputPreview,
  formatTime,
  metadataOperator,
  shortHash,
  shortId,
} from "../../utils";

type EvidenceRecord = {
  auditMatches: AuditLogRecord[];
  job: JobHistoryRecord;
};

type EvidenceStateTone = "neutral" | "ok" | "warn";

type EvidenceStateLabel = {
  detail: string;
  label: string;
  searchText: string;
  tone: EvidenceStateTone;
};

type EvidenceLoadState = {
  error: string | null;
  loading: boolean;
  outputs: JobOutputRecord[];
  targets: JobTargetRecord[];
};

const EMPTY_EVIDENCE_STATE: EvidenceLoadState = {
  error: null,
  loading: false,
  outputs: [],
  targets: [],
};

export function JobEvidencePanel({
  agents,
  audits,
  error,
  jobs,
  loading,
  onLoadJobOutputs,
  onLoadJobTargets,
  onOpenJobDetails,
  onRefresh,
}: {
  agents: AgentView[];
  audits: AuditLogRecord[];
  error: string | null;
  jobs: JobHistoryRecord[];
  loading: boolean;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadJobTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onRefresh: () => void;
}) {
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [evidenceByJob, setEvidenceByJob] = useState<Record<string, EvidenceLoadState>>({});

  useEffect(() => {
    if (!selectedJobId && jobs.length > 0) {
      setSelectedJobId(jobs[0].id);
    }
  }, [jobs, selectedJobId]);

  const agentNameById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent.display_name || agent.id])),
    [agents],
  );

  const evidenceRows = useMemo<EvidenceRecord[]>(
    () =>
      jobs.map((job) => ({
        auditMatches: audits
          .filter((audit) => auditMatchesJob(audit, job))
          .sort((left, right) => right.created_at.localeCompare(left.created_at)),
        job,
      })),
    [audits, jobs],
  );

  const selectedRecord = useMemo(
    () =>
      evidenceRows.find((row) => row.job.id === selectedJobId) ??
      evidenceRows[0] ??
      null,
    [evidenceRows, selectedJobId],
  );

  const selectedEvidence =
    (selectedRecord ? evidenceByJob[selectedRecord.job.id] : null) ??
    EMPTY_EVIDENCE_STATE;

  useEffect(() => {
    const jobId = selectedRecord?.job.id;
    if (!jobId || evidenceByJob[jobId]) {
      return;
    }
    setEvidenceByJob((current) => ({
      ...current,
      [jobId]: {
        error: null,
        loading: true,
        outputs: current[jobId]?.outputs ?? [],
        targets: current[jobId]?.targets ?? [],
      },
    }));
    Promise.all([onLoadJobTargets(jobId), onLoadJobOutputs(jobId)])
      .then(([targets, outputs]) => {
        setEvidenceByJob((current) => ({
          ...current,
          [jobId]: {
            error: null,
            loading: false,
            outputs,
            targets,
          },
        }));
      })
      .catch((loadError: unknown) => {
        setEvidenceByJob((current) => ({
          ...current,
          [jobId]: {
            error:
              loadError instanceof Error
                ? loadError.message
                : "Unable to load job evidence",
            loading: false,
            outputs: current[jobId]?.outputs ?? [],
            targets: current[jobId]?.targets ?? [],
          },
        }));
      });
  }, [evidenceByJob, onLoadJobOutputs, onLoadJobTargets, selectedRecord]);

  const privilegedJobs = useMemo(
    () => jobs.filter((job) => job.privileged).length,
    [jobs],
  );
  const matchedJobs = useMemo(
    () => evidenceRows.filter((row) => row.auditMatches.length > 0).length,
    [evidenceRows],
  );
  const auditGapCount = useMemo(
    () => evidenceRows.filter((row) => row.auditMatches.length === 0).length,
    [evidenceRows],
  );

  const columns = useMemo<ConsoleDataGridColumn<EvidenceRecord>[]>(
    () => [
      {
        id: "job",
        header: "Job",
        minSize: 190,
        searchValue: (row) => `${row.job.id} ${row.job.command_type}`,
        size: 230,
        sortValue: (row) => row.job.created_at,
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{commandLabel(row.job.command_type)}</strong>
            <small>{shortId(row.job.id)}</small>
          </span>
        ),
      },
      {
        id: "actor",
        header: "Actor",
        minSize: 130,
        searchValue: (row) => jobActorLabel(row.job, row.auditMatches),
        size: 150,
        sortValue: (row) => jobActorLabel(row.job, row.auditMatches),
        cell: (row) => jobActorLabel(row.job, row.auditMatches),
      },
      {
        id: "privilege",
        header: "Privilege",
        minSize: 110,
        searchValue: (row) => (row.job.privileged ? "privileged" : "standard"),
        size: 120,
        sortValue: (row) => (row.job.privileged ? 1 : 0),
        cell: (row) => (
          <span className={`status ${row.job.privileged ? "warn" : "neutral"}`}>
            {row.job.privileged ? "privileged" : "standard"}
          </span>
        ),
      },
      {
        id: "targets",
        align: "end",
        header: "Targets",
        minSize: 90,
        searchValue: (row) => row.job.target_count,
        size: 100,
        sortValue: (row) => row.job.target_count,
        cell: (row) => row.job.target_count,
      },
      {
        id: "status",
        header: "Result",
        minSize: 120,
        searchValue: (row) => row.job.status,
        size: 130,
        sortValue: (row) => row.job.status,
        cell: (row) => (
          <span className={`status ${jobStatusBadgeClass(row.job.status)}`}>
            {row.job.status}
          </span>
        ),
      },
      {
        id: "audit",
        header: "Audit",
        minSize: 130,
        searchValue: (row) =>
          auditEvidenceState(row).searchText,
        size: 150,
        sortValue: (row) => (row.auditMatches.length > 0 ? 1 : 0),
        cell: (row) => {
          const state = auditEvidenceState(row);
          return <span className={`status ${state.tone}`}>{state.label}</span>;
        },
      },
      {
        id: "output",
        header: "Output",
        minSize: 120,
        searchValue: (row) => outputEvidenceState(row.job, evidenceByJob[row.job.id]).searchText,
        size: 150,
        sortValue: (row) => outputEvidenceState(row.job, evidenceByJob[row.job.id]).label,
        cell: (row) => {
          const state = outputEvidenceState(row.job, evidenceByJob[row.job.id]);
          return <span className={`status ${state.tone}`}>{state.label}</span>;
        },
      },
    ],
    [evidenceByJob],
  );

  return (
    <section className="fleetPanel auditJobEvidencePanel" aria-label="Audit job evidence">
      <div className="sectionHeader">
        <span>
          <h2>Job audit evidence</h2>
          <small>
            Read-only correlation of job history, audit rows, target results, and retained output artifacts.
          </small>
        </span>
        <button className="secondaryAction compactAction" onClick={onRefresh} type="button">
          Refresh
        </button>
      </div>

      {error && <div className="errorBanner">{error}</div>}

      <div className="metricGrid" aria-label="Job evidence summary">
        <div className="metricCard">
          <ClipboardCheck size={18} />
          <span>
            <strong>{jobs.length}</strong>
            <small>Jobs in ledger</small>
          </span>
        </div>
        <div className="metricCard">
          <ShieldCheck size={18} />
          <span>
            <strong>{privilegedJobs}</strong>
            <small>Privileged jobs</small>
          </span>
        </div>
        <div className="metricCard">
          <Link2 size={18} />
          <span>
            <strong>{matchedJobs}</strong>
            <small>Jobs with audit rows</small>
          </span>
        </div>
        <div className={`metricCard ${auditGapCount > 0 ? "attention" : ""}`}>
          <FileText size={18} />
          <span>
            <strong>{auditGapCount}</strong>
            <small>Audit gaps</small>
          </span>
        </div>
      </div>

      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={10}
        empty={
          <div className="emptyState">
            <ClipboardCheck size={22} />
            <strong>No job evidence returned</strong>
            <span>
              Dispatch history is required before Audit can prove execution.
            </span>
          </div>
        }
        getRowId={(row) => row.job.id}
        itemLabel="jobs"
        onOpenRow={(row) => setSelectedJobId(row.job.id)}
        openRowLabel="Select proof"
        openRowTitle={(row) => `Show evidence proof for job ${row.job.id}.`}
        rows={evidenceRows}
        searchPlaceholder="Search job ID, actor, status, hash, command, or audit action"
        selectable={false}
        storageKey="audit-job-evidence-grid"
        title="Job evidence ledger"
      />

      {loading && (
        <div className="dashboardWidgetEmpty">Loading job and audit evidence...</div>
      )}

      {selectedRecord && (
        <JobEvidenceDetail
          agentNameById={agentNameById}
          evidence={selectedEvidence}
          onOpenJobDetails={onOpenJobDetails}
          record={selectedRecord}
        />
      )}
    </section>
  );
}

function JobEvidenceDetail({
  agentNameById,
  evidence,
  onOpenJobDetails,
  record,
}: {
  agentNameById: Map<string, string>;
  evidence: EvidenceLoadState;
  onOpenJobDetails?: (jobId: string) => void;
  record: EvidenceRecord;
}) {
  const targetSummary = targetScopeLabel(record.job, evidence.targets, agentNameById);
  const outputArtifactCount = evidence.outputs.filter((output) =>
    Boolean(output.artifact_object_key || output.artifact_sha256_hex),
  ).length;
  const streams = Array.from(new Set(evidence.outputs.map((output) => output.stream))).sort();
  const approvalLabel = approvalStateLabel(record.job, record.auditMatches);
  const auditState = auditEvidenceState(record);
  const outputState = outputEvidenceState(record.job, evidence);

  return (
    <section className="consoleDetailPanel jobEvidenceDetailPanel" aria-label="Selected job evidence detail">
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>Selected job proof</strong>
          <small>
            {commandLabel(record.job.command_type)} · {shortId(record.job.id)}
          </small>
        </span>
        {onOpenJobDetails ? (
          <button
            className="secondaryAction compactAction"
            onClick={() => onOpenJobDetails(record.job.id)}
            type="button"
          >
            <ExternalLink size={14} />
            <span>Open in Jobs / History</span>
          </button>
        ) : null}
      </div>

      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Actor</strong>
          <span>{jobActorLabel(record.job, record.auditMatches)}</span>
        </span>
        <span>
          <strong>Privilege</strong>
          <span>{record.job.privileged ? "privileged command" : "standard command"}</span>
        </span>
        <span>
          <strong>Target scope</strong>
          <span>{targetSummary}</span>
        </span>
        <span>
          <strong>Audit</strong>
          <span className={`status ${auditState.tone}`}>{auditState.label}</span>
        </span>
        <span>
          <strong>Output</strong>
          <span className={`status ${outputState.tone}`}>{outputState.label}</span>
        </span>
        <span>
          <strong>Output artifact</strong>
          <span>
            {outputArtifactCount > 0
              ? `${outputArtifactCount} retained artifact${outputArtifactCount === 1 ? "" : "s"}`
              : evidence.outputs.length > 0
                ? `${evidence.outputs.length} retained output row${evidence.outputs.length === 1 ? "" : "s"}`
                : evidence.loading
                  ? "loading retained output"
                  : "no output rows returned"}
          </span>
        </span>
        <span>
          <strong>Approval</strong>
          <span>{approvalLabel}</span>
        </span>
        <span>
          <strong>Payload hash</strong>
          <span>{record.job.payload_hash}</span>
        </span>
      </div>

      {evidence.error && <div className="errorBanner">{evidence.error}</div>}

      <div className="jobEvidenceSections">
        <section className="dashboardWidgetTable" aria-label="Audit context for selected job">
          <div className="dashboardWidgetHeader">
            <strong>Audit context</strong>
            <small>
              {record.auditMatches.length > 0
                ? `${record.auditMatches.length} row${record.auditMatches.length === 1 ? "" : "s"}`
                : "Audit event missing"}
            </small>
          </div>
          {record.auditMatches.length > 0 ? (
            record.auditMatches.slice(0, 6).map((audit) => (
              <div className="dashboardWidgetRow auditEvidenceRow" key={audit.id}>
                <strong>{audit.action}</strong>
                <span>{audit.target}</span>
                <small>{audit.command_hash ? shortHash(audit.command_hash) : "no hash"}</small>
                <small>{formatTime(audit.created_at)}</small>
              </div>
            ))
          ) : (
            <div className="dashboardWidgetEmpty">
              Audit event missing. No matching audit row was returned for this payload hash or job ID; job, target, and output evidence remains visible here.
            </div>
          )}
        </section>

        <section className="dashboardWidgetTable" aria-label="Job targets for selected job">
          <div className="dashboardWidgetHeader">
            <strong>Job targets</strong>
            <small>
              {evidence.loading
                ? "loading"
                : `${evidence.targets.length} target${evidence.targets.length === 1 ? "" : "s"}`}
            </small>
          </div>
          {evidence.targets.length > 0 ? (
            evidence.targets.map((target) => (
              <div className="dashboardWidgetRow auditEvidenceRow" key={`${target.job_id}-${target.client_id}`}>
                <strong>{agentNameById.get(target.client_id) ?? target.client_id}</strong>
                <span className={`status ${jobTargetStatusBadgeClass(target.status)}`}>
                  {target.status}
                </span>
                <small>{target.exit_code == null ? "exit -" : `exit ${target.exit_code}`}</small>
                <small>{target.completed_at ? formatTime(target.completed_at) : "not completed"}</small>
              </div>
            ))
          ) : (
            <div className="dashboardWidgetEmpty">
              {evidence.loading ? "Loading target status evidence..." : "No target rows returned for this job."}
            </div>
          )}
        </section>

        <section className="dashboardWidgetTable wideWidget" aria-label="Job outputs for selected job">
          <div className="dashboardWidgetHeader">
            <strong>Job outputs</strong>
            <small>
              {evidence.loading
                ? "loading"
                : streams.length > 0
                  ? streams.join(", ")
                  : "no streams"}
            </small>
          </div>
          {evidence.outputs.length > 0 ? (
            evidence.outputs.slice(0, 8).map((output) => (
              <div className="jobEvidenceOutputRow" key={`${output.job_id}-${output.client_id}-${output.seq}-${output.stream}`}>
                <span>
                  <strong>{agentNameById.get(output.client_id) ?? output.client_id}</strong>
                  <small>{output.stream} · seq {output.seq}</small>
                </span>
                <span>
                  {output.artifact_object_key ? (
                    <strong>{output.artifact_object_key}</strong>
                  ) : output.artifact_sha256_hex ? (
                    <strong>{shortHash(output.artifact_sha256_hex)}</strong>
                  ) : (
                    <strong>inline output</strong>
                  )}
                  <small>
                    {output.artifact_size_bytes != null
                      ? formatBytes(output.artifact_size_bytes)
                      : output.done
                        ? "complete"
                        : "streaming"}
                  </small>
                </span>
                <pre>{decodeOutputPreview(output.data_base64).slice(0, 640) || "no preview"}</pre>
              </div>
            ))
          ) : (
            <div className="dashboardWidgetEmpty">
              {outputState.detail}
            </div>
          )}
        </section>
      </div>
    </section>
  );
}

function auditMatchesJob(audit: AuditLogRecord, job: JobHistoryRecord): boolean {
  if (audit.command_hash && audit.command_hash === job.payload_hash) {
    return true;
  }
  const metadata = JSON.stringify(audit.metadata).toLowerCase();
  return (
    metadata.includes(job.id.toLowerCase()) ||
    metadata.includes(job.payload_hash.toLowerCase())
  );
}

function auditEvidenceState(record: EvidenceRecord): EvidenceStateLabel {
  if (record.auditMatches.length > 0) {
    const actions = record.auditMatches.map((audit) => audit.action).join(" ");
    return {
      detail: `${record.auditMatches.length} audit row${record.auditMatches.length === 1 ? "" : "s"} matched`,
      label: `${record.auditMatches.length} matched`,
      searchText: `${actions} matched audit linked`,
      tone: "ok",
    };
  }
  return {
    detail: "No audit row matched this job ID or payload hash",
    label: "Audit event missing",
    searchText: "audit event missing audit gap",
    tone: "warn",
  };
}

function outputEvidenceState(
  job: JobHistoryRecord,
  evidence: EvidenceLoadState | undefined,
): EvidenceStateLabel {
  if (!evidence || evidence.loading) {
    return {
      detail: evidence?.loading
        ? "Loading retained output evidence..."
        : "Select the row to load retained output evidence.",
      label: "Not loaded",
      searchText: "not loaded output pending",
      tone: "neutral",
    };
  }
  if (evidence.error) {
    const lower = evidence.error.toLowerCase();
    const retentionExpired =
      lower.includes("retention") ||
      lower.includes("expired") ||
      lower.includes("gone");
    return retentionExpired
      ? {
          detail: "Retention expired. The job remains in the ledger, but retained output is no longer available.",
          label: "Retention expired",
          searchText: "retention expired output unavailable",
          tone: "warn",
        }
      : {
          detail: `Output unavailable. ${evidence.error}`,
          label: "Output unavailable",
          searchText: "output unavailable load error",
          tone: "warn",
        };
  }
  if (evidence.outputs.length === 0) {
    return {
      detail:
        job.status === "completed"
          ? "Output unavailable. No output artifact or inline output row was returned for this completed job."
          : "Output unavailable. No retained output row is available yet for this job.",
      label: "Output unavailable",
      searchText: "output unavailable no rows",
      tone: "warn",
    };
  }
  const hasArtifact = evidence.outputs.some((output) =>
    Boolean(output.artifact_object_key || output.artifact_sha256_hex),
  );
  const hasNonEmptyOutput = evidence.outputs.some((output) =>
    decodeOutputPreview(output.data_base64).trim().length > 0,
  );
  if (!hasArtifact && !hasNonEmptyOutput) {
    return {
      detail: "Empty output. The retained output stream contains no visible text or artifact reference.",
      label: "Empty output",
      searchText: "empty output retained",
      tone: "neutral",
    };
  }
  return {
    detail: hasArtifact
      ? `${evidence.outputs.length} output row${evidence.outputs.length === 1 ? "" : "s"} with retained artifact evidence`
      : `${evidence.outputs.length} inline output row${evidence.outputs.length === 1 ? "" : "s"} loaded`,
    label: hasArtifact ? "Retained output" : "Inline output",
    searchText: `${hasArtifact ? "retained output artifact" : "inline output"} loaded`,
    tone: "ok",
  };
}

function jobActorLabel(job: JobHistoryRecord, audits: AuditLogRecord[]): string {
  const auditActor = audits
    .map((audit) => metadataOperator(audit.metadata) ?? metadataText(audit.metadata, ["operator_username", "username", "operator_id", "actor_id"]))
    .find(Boolean);
  if (auditActor) {
    return auditActor;
  }
  if (job.actor_id) {
    return shortId(job.actor_id);
  }
  if (job.command_type.startsWith("scheduled_")) {
    return "system scheduler";
  }
  return "system / automation";
}

function approvalStateLabel(job: JobHistoryRecord, audits: AuditLogRecord[]): string {
  const approvalAudit = audits.find((audit) =>
    `${audit.action} ${JSON.stringify(audit.metadata)}`.toLowerCase().includes("approval"),
  );
  if (approvalAudit) {
    return `${approvalAudit.action} · ${formatTime(approvalAudit.created_at)}`;
  }
  return job.privileged
    ? "no approval record exposed"
    : "approval not required by job record";
}

function targetScopeLabel(
  job: JobHistoryRecord,
  targets: JobTargetRecord[],
  agentNameById: Map<string, string>,
): string {
  if (targets.length === 0) {
    return `${job.target_count} target${job.target_count === 1 ? "" : "s"} declared`;
  }
  const names = targets
    .slice(0, 3)
    .map((target) => agentNameById.get(target.client_id) ?? target.client_id);
  const suffix = targets.length > names.length ? ` +${targets.length - names.length}` : "";
  return `${targets.length} target${targets.length === 1 ? "" : "s"}: ${names.join(", ")}${suffix}`;
}

function metadataText(metadata: JsonValue, keys: string[]): string | null {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) {
    return null;
  }
  for (const key of keys) {
    const value = metadata[key];
    if (typeof value === "string" && value.trim()) {
      return value;
    }
  }
  return null;
}

function commandLabel(value: string): string {
  return value.replace(/^scheduled_/, "").replace(/_/g, " ");
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
