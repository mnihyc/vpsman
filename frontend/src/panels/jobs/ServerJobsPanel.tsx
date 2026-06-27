import { RefreshCw, ShieldCheck, Trash2, XCircle } from "lucide-react";
import { useMemo, useState } from "react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { useReviewGenerationGuard, waitForReviewRender } from "../../hooks/useReviewGenerationGuard";
import { serverJobStatusBadgeClass } from "../../jobStatusPresentation";
import type {
  ArtifactCleanupPreviewRecord,
  ServerJobRecord,
} from "../../types";
import { formatTime, shortHash, shortId } from "../../utils";

type ArtifactCleanupDomain = "job_output" | "file_transfer" | "backup_artifact";
type CleanupArtifactState = "active" | "delete_failed" | "deleting" | "creating" | "any";

const artifactCleanupDomainOptions: Array<{
  description: string;
  label: string;
  value: ArtifactCleanupDomain;
}> = [
  {
    description: "Retained command output objects and downloadable job payloads.",
    label: "Job output",
    value: "job_output",
  },
  {
    description: "Uploaded transfer source files and promoted download handoff objects.",
    label: "File transfer",
    value: "file_transfer",
  },
  {
    description: "Backup artifact objects and metadata.",
    label: "Backup artifacts",
    value: "backup_artifact",
  },
];

export function ServerJobsPanel({
  jobs,
  loading,
  onCancelJob,
  onCreateCleanupJob,
  onPreviewCleanup,
  onRefresh,
}: {
  jobs: ServerJobRecord[];
  loading: boolean;
  onCancelJob: (jobId: string) => Promise<ServerJobRecord>;
  onCreateCleanupJob: (
    expression: string,
    domains: string[],
    previewHash: string,
  ) => Promise<ServerJobRecord>;
  onPreviewCleanup: (
    expression: string,
    domains: string[],
  ) => Promise<ArtifactCleanupPreviewRecord>;
  onRefresh: () => void;
}) {
  const [olderThanDays, setOlderThanDays] = useState("30");
  const [artifactState, setArtifactState] = useState<CleanupArtifactState>("active");
  const [objectPrefix, setObjectPrefix] = useState("");
  const [advancedExpression, setAdvancedExpression] = useState("");
  const [domains, setDomains] = useState<ArtifactCleanupDomain[]>([
    "job_output",
    "file_transfer",
  ]);
  const [preview, setPreview] = useState<ArtifactCleanupPreviewRecord | null>(
    null,
  );
  const [pending, setPending] = useState(false);
  const [pendingJobId, setPendingJobId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [previewStatus, setPreviewStatus] = useState<string | null>(null);
  const [cancelJobSnapshot, setCancelJobSnapshot] = useState<ServerJobRecord | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const expression = useMemo(
    () => buildCleanupExpression(olderThanDays, artifactState, objectPrefix, advancedExpression),
    [advancedExpression, artifactState, objectPrefix, olderThanDays],
  );
  const expressionValid = expression.trim().length > 0 && !expression.startsWith("__invalid__");
  const summary =
    error ??
    previewStatus ??
    (preview
      ? `${preview.matched_count} artifacts previewed, ${formatBytes(preview.matched_bytes)}`
      : `${jobs.length} maintenance jobs`);
  const previewExpressionMatches = preview?.expression === expression;
  const previewDomainsMatch = preview ? sameDomains(preview.domains, domains) : false;
  const previewFresh = Boolean(preview && previewExpressionMatches && previewDomainsMatch);
  const cleanupEvidence = cleanupPreviewEvidence(preview);
  const cleanupCanDelete = Boolean(
    previewFresh &&
      cleanupEvidence.complete &&
      preview &&
      preview.matched_count > 0,
  );
  const previewReadiness = cleanupCanDelete
    ? "Reviewed preview has deletion evidence and can open one final confirmation."
    : previewFresh
      ? "Preview is current, but deletion is blocked until the backend reports age range, retention/protection, and affected objects."
    : preview
      ? "Preview is stale; rerun Preview before deletion."
      : "Preview required before deletion.";
  const serverJobColumns = useMemo<ConsoleDataGridColumn<ServerJobRecord>[]>(
    () => [
      {
        cell: (job) => (
          <span className="historyPrimary">
            <strong>{displayToken(job.job_type)}</strong>
            <small>{shortId(job.id)}</small>
          </span>
        ),
        header: "Job",
        id: "job",
        searchValue: (job) => `${job.job_type} ${job.id}`,
        sortValue: (job) => job.job_type,
      },
      {
        cell: (job) => (
          <span className="historyPrimary">
            <span className={`status ${serverJobStatusBadgeClass(job.status)}`}>
              {displayToken(job.status)}
            </span>
            <small>{job.error ?? job.expression ?? "no details"}</small>
          </span>
        ),
        header: "Status",
        id: "status",
        searchValue: (job) => `${job.status} ${job.error ?? ""} ${job.expression ?? ""}`,
        sortValue: (job) => job.status,
      },
      {
        cell: (job) => (
          <span className="historyPrimary">
            <strong>{job.matched_count}</strong>
            <small>{formatBytes(job.matched_bytes)}</small>
          </span>
        ),
        header: "Matched",
        id: "matched",
        searchValue: (job) => `${job.matched_count} ${formatBytes(job.matched_bytes)}`,
        sortValue: (job) => job.matched_count,
      },
      {
        cell: (job) => (
          <span className="historyPrimary">
            <strong>{job.deleted_count}</strong>
            <small>{formatBytes(job.deleted_bytes)}</small>
          </span>
        ),
        header: "Deleted",
        id: "deleted",
        searchValue: (job) => `${job.deleted_count} ${formatBytes(job.deleted_bytes)}`,
        sortValue: (job) => job.deleted_count,
      },
      {
        cell: (job) => formatTime(job.created_at),
        header: "Created",
        id: "created",
        searchValue: (job) => formatTime(job.created_at),
        sortValue: (job) => job.created_at,
      },
      {
        cell: (job) => (
          <button
            className="secondaryAction compactAction dangerAction"
            disabled={pendingJobId === job.id || job.status !== "queued"}
            onClick={(event) => {
              event.stopPropagation();
              reviewCancelJob(job);
            }}
            title="Cancel queued maintenance job"
            type="button"
          >
            <XCircle size={14} />
            Cancel
          </button>
        ),
        enableHiding: false,
        header: "Action",
        id: "action",
      },
    ],
    [pendingJobId],
  );

  async function previewCleanup() {
    if (!expressionValid) {
      setError("Enter valid cleanup criteria before preview.");
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const frozenExpression = expression;
    const frozenDomains = [...domains];
    setPending(true);
    setError(null);
    setPreviewStatus("Preparing cleanup preview");
    try {
      await waitForReviewRender();
      const nextPreview = await onPreviewCleanup(frozenExpression, frozenDomains);
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPreview(nextPreview);
      setConfirmOpen(false);
    } catch (previewError) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPreview(null);
      setError(
        previewError instanceof Error
          ? previewError.message
          : "Cleanup preview failed",
      );
    } finally {
      setPending(false);
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setPreviewStatus(null);
      }
    }
  }

  async function queueCleanup() {
    if (!preview) {
      return;
    }
    if (!cleanupCanDelete) {
      setError("Deletion is blocked until cleanup preview includes age range, retention/protection, and affected-object evidence.");
      return;
    }
    setPending(true);
    setError(null);
    try {
      await onCreateCleanupJob(
        preview.expression,
        preview.domains,
        preview.preview_hash,
      );
      setConfirmOpen(false);
      setPreview(null);
    } catch (createError) {
      setError(
        createError instanceof Error
          ? createError.message
          : "Cleanup job creation failed",
      );
    } finally {
      setPending(false);
    }
  }

  function reviewCancelJob(job: ServerJobRecord) {
    setError(null);
    setCancelJobSnapshot(job);
  }

  function updateDomain(domain: ArtifactCleanupDomain, checked: boolean) {
    invalidateReviewGeneration();
    setDomains((current) => {
      if (checked) {
        return current.includes(domain) ? current : [...current, domain];
      }
      return current.filter((value) => value !== domain);
    });
    setPreview(null);
    setConfirmOpen(false);
    setPreviewStatus(null);
  }

  async function cancelJob(job: ServerJobRecord) {
    setPendingJobId(job.id);
    setError(null);
    try {
      await onCancelJob(job.id);
      setCancelJobSnapshot(null);
    } catch (cancelError) {
      setError(
        cancelError instanceof Error
          ? cancelError.message
          : "Server job cancellation failed",
      );
    } finally {
      setPendingJobId(null);
    }
  }

  return (
    <div className="jobConsoleStack">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Artifact cleanup</h2>
            <span>{summary}</span>
          </div>
          <button
            className="secondaryAction"
            disabled={loading}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCw size={14} />
            Refresh
          </button>
        </div>
        <div className="historyRetentionGrid">
          <div className="cleanupPreviewContract">
            <ShieldCheck size={18} />
            <div>
              <strong>Preview gate</strong>
              <span>
                Preview shows the current count and size. Delete stays blocked until the preview also proves object age, retention/protection, and affected-object evidence.
              </span>
            </div>
          </div>
          <div className="artifactCleanupDomains">
            <span>Artifact types</span>
            <div className="artifactDomainOptions">
              {artifactCleanupDomainOptions.map((option) => (
                <label
                  className="artifactDomainOption"
                  key={option.value}
                  title={option.description}
                >
                  <input
                    checked={domains.includes(option.value)}
                    onChange={(event) => updateDomain(option.value, event.target.checked)}
                    type="checkbox"
                  />
                  <span>
                    <strong>{option.label}</strong>
                    <small>{option.description}</small>
                  </span>
                </label>
              ))}
            </div>
          </div>
          <div className="cleanupCriteriaGrid">
            <label>
              <span>Older than</span>
              <div className="inlineUnitInput">
                <input
                  aria-label="Older than days"
                  inputMode="numeric"
                  min={0}
                  max={3650}
                  onChange={(event) => {
                    invalidateReviewGeneration();
                    setOlderThanDays(event.target.value);
                    setPreview(null);
                    setConfirmOpen(false);
                    setPreviewStatus(null);
                  }}
                  type="number"
                  value={olderThanDays}
                />
                <small>days</small>
              </div>
            </label>
            <label>
              <span>State</span>
              <select
                aria-label="Artifact state"
                onChange={(event) => {
                  invalidateReviewGeneration();
                  setArtifactState(event.target.value as CleanupArtifactState);
                  setPreview(null);
                  setConfirmOpen(false);
                  setPreviewStatus(null);
                }}
                value={artifactState}
              >
                <option value="active">Active</option>
                <option value="delete_failed">Delete failed</option>
                <option value="deleting">Deleting</option>
                <option value="creating">Creating</option>
                <option value="any">Any state</option>
              </select>
            </label>
            <label>
              <span>Object path/prefix</span>
              <input
                aria-label="Object path prefix"
                onChange={(event) => {
                  invalidateReviewGeneration();
                  setObjectPrefix(event.target.value);
                  setPreview(null);
                  setConfirmOpen(false);
                  setPreviewStatus(null);
                }}
                placeholder="Optional object key prefix"
                value={objectPrefix}
              />
            </label>
          </div>
          <details className="cleanupAdvanced">
            <summary>Advanced expression</summary>
            <label className="artifactCleanupExpression">
              <span>Additional filter expression</span>
              <textarea
                aria-label="Expression"
                rows={3}
                title="Advanced expression filters artifacts inside the selected artifact types. It is combined with the common criteria above."
                value={advancedExpression}
                onChange={(event) => {
                  invalidateReviewGeneration();
                  setAdvancedExpression(event.target.value);
                  setPreview(null);
                  setConfirmOpen(false);
                  setPreviewStatus(null);
                }}
              />
            </label>
            <div className="cleanupExpressionPreview" aria-label="Effective cleanup expression">
              <span>Effective expression</span>
              <code>{expressionValid ? expression : "Invalid criteria"}</code>
            </div>
          </details>
          <div className="cleanupPreviewFacts" aria-label="Cleanup preview result">
            <div>
              <span>Matched</span>
              <strong>
                {preview
                  ? `${preview.matched_count} artifacts / ${formatBytes(preview.matched_bytes)}`
                  : "Preview required"}
              </strong>
            </div>
            <div>
              <span>Age range</span>
              <strong>{cleanupEvidence.ageRangeLabel}</strong>
            </div>
            <div>
              <span>Retention/protection</span>
              <strong>{cleanupEvidence.retentionLabel}</strong>
            </div>
            <div>
              <span>Affected objects</span>
              <strong>{cleanupEvidence.objectsLabel}</strong>
            </div>
            <div>
              <span>Preview snapshot</span>
              <strong title={preview?.preview_hash}>
                {preview ? shortHash(preview.preview_hash) : "Not created"}
              </strong>
            </div>
          </div>
          {preview?.representative_objects?.length ? (
            <div className="cleanupObjectPreview" aria-label="Representative cleanup objects">
              <div>
                <span>Representative objects</span>
                <strong>{preview.representative_objects.length} shown from preview</strong>
              </div>
              <ul>
                {preview.representative_objects.slice(0, 5).map((object) => (
                  <li key={`${object.domain}:${object.object_key}`}>
                    <span className="cleanupObjectIdentity">
                      <strong>{displayToken(object.domain)}</strong>
                      <code title={object.object_key}>{object.object_key}</code>
                    </span>
                    <span className="cleanupObjectMeta">
                      {formatBytes(object.size_bytes)} · {object.created_at ? formatTime(object.created_at) : "age unavailable"} · {displayToken(object.status)}
                      {object.reference_protected ? " · protected" : ""}
                    </span>
                    {object.reason ? <span className="cleanupObjectReason">{object.reason}</span> : null}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          <div className="cleanupReadinessSummary" aria-label="Artifact cleanup readiness">
            <div className={cleanupCanDelete ? "ready" : "attention"}>
              <span>Delete status</span>
              <strong>
                {cleanupCanDelete
                  ? "Ready for confirmation"
                  : previewFresh
                    ? "Delete blocked"
                    : "Preview required"}
              </strong>
              <p>{previewReadiness}</p>
            </div>
            <div>
              <span>Scope</span>
              <strong>{formatCleanupDomains(preview?.domains ?? domains) || "No domains selected"}</strong>
              <p>{domains.length} selected artifact types. Type filters and object-prefix criteria narrow the preview.</p>
            </div>
            <div className={preview ? "attention" : undefined}>
              <span>Deletion impact</span>
              <strong>
                {preview
                  ? `${preview.matched_count} artifacts / ${formatBytes(preview.matched_bytes)}`
                  : "Unknown until preview"}
              </strong>
              <p>
                {preview
                  ? "Count and size are known, but deletion requires object-level evidence before confirmation."
                  : "Run Preview to calculate object count and total size."}
              </p>
            </div>
            <div>
              <span>Missing evidence</span>
              <strong>{cleanupEvidence.missingLabel}</strong>
              <p>Backend preview must expose oldest/newest object age, retained/reference-protected counts, and a representative object list or download.</p>
            </div>
          </div>
          <div className="retentionActions">
            <button
              className="secondaryAction"
              disabled={pending || !expressionValid || domains.length === 0}
              onClick={() => void previewCleanup()}
              title="Build a reviewed cleanup snapshot for the selected domains"
              type="button"
            >
              <ShieldCheck size={16} />
              Preview
            </button>
            <button
              className="secondaryAction dangerAction"
              disabled={pending || !cleanupCanDelete}
              onClick={() => setConfirmOpen(true)}
              title={
                cleanupCanDelete
                  ? "Delete artifacts using the reviewed expression, artifact types, preview hash, and object evidence"
                  : "Deletion blocked until preview includes age range, retention/protection, and affected-object evidence"
              }
              type="button"
            >
              <Trash2 size={16} />
              Delete artifacts
            </button>
          </div>
        </div>
        <ConfirmationPrompt
          confirmLabel="Delete artifacts"
          detail="Queues the reviewed artifact set for deletion. This operation is destructive and uses the preview hash plus object evidence as its guardrail."
          error={error}
          items={[
            { label: "Preview gate", value: cleanupCanDelete ? "Fresh preview evidence verified" : "Preview evidence incomplete" },
            { label: "Expression", value: preview?.expression ?? expression },
            {
              label: "Artifact types",
              title: formatCleanupDomains(preview?.domains ?? domains),
              value: formatCleanupDomains(preview?.domains ?? domains),
            },
            { label: "Artifacts", value: preview?.matched_count ?? 0 },
            {
              label: "Bytes",
              value: preview ? formatBytes(preview.matched_bytes) : "0 B",
            },
            {
              label: "Age range",
              value: cleanupEvidence.ageRangeLabel,
            },
            {
              label: "Affected objects",
              value: cleanupEvidence.objectsLabel,
            },
            {
              label: "Preview hash",
              title: preview?.preview_hash,
              value: preview ? shortHash(preview.preview_hash) : "-",
            },
            {
              label: "Retention detail",
              value: cleanupEvidence.retentionLabel,
            },
            {
              label: "Effect",
              value: "Irreversible artifact deletion request",
            },
          ]}
          onCancel={() => setConfirmOpen(false)}
          onConfirm={() => void queueCleanup()}
          open={confirmOpen}
          pending={pending}
          title="Confirm artifact deletion"
          tone="danger"
          typedConfirmationLabel="Type DELETE to confirm artifact deletion"
          typedConfirmationText="DELETE"
        />
      </div>
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Maintenance jobs</h2>
            <span>{jobs.length} retained control-plane maintenance jobs</span>
          </div>
          <button
            className="secondaryAction"
            disabled={loading}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCw size={14} />
            Refresh
          </button>
        </div>
        <ConsoleDataGrid
          columns={serverJobColumns}
          defaultPageSize={10}
          expandOnRowClick
          getRowId={(job) => job.id}
          itemLabel="jobs"
          empty={
            <div className="emptyState">
              <Trash2 size={22} />
              <strong>No maintenance jobs</strong>
              <span>Artifact cleanup jobs appear here after queueing.</span>
            </div>
          }
          renderExpandedRow={(job) => (
            <div className="consoleInlineDetailGrid">
              <span>Job ID</span>
              <strong>{job.id}</strong>
              <span>Type</span>
              <strong>{displayToken(job.job_type)}</strong>
              <span>Status</span>
              <strong>{displayToken(job.status)}</strong>
              <span>Expression</span>
              <strong>{job.expression ?? "Not recorded"}</strong>
              <span>Matched bytes</span>
              <strong>{formatBytes(job.matched_bytes)}</strong>
              <span>Deleted bytes</span>
              <strong>{formatBytes(job.deleted_bytes)}</strong>
              <span>Error</span>
              <strong>{job.error ?? "None"}</strong>
            </div>
          )}
          rows={jobs}
          searchPlaceholder="Search maintenance jobs"
          selectable={false}
          storageKey="vpsman.jobs.serverJobs"
          title="Maintenance job records"
        />
        <ConfirmationPrompt
          confirmLabel="Cancel job"
          detail="Cancel the reviewed queued control-plane maintenance job."
          error={error}
          items={[
            { label: "Job", value: cancelJobSnapshot ? shortId(cancelJobSnapshot.id) : "-" },
            { label: "Type", value: cancelJobSnapshot ? displayToken(cancelJobSnapshot.job_type) : "-" },
            { label: "Matched", value: cancelJobSnapshot?.matched_count ?? 0 },
          ]}
          onCancel={() => setCancelJobSnapshot(null)}
          onConfirm={() => {
            if (cancelJobSnapshot) {
              void cancelJob(cancelJobSnapshot);
            }
          }}
          open={cancelJobSnapshot !== null}
          pending={pendingJobId !== null}
          title="Confirm maintenance job cancellation"
          tone="danger"
        />
      </div>
    </div>
  );
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}

function buildCleanupExpression(
  olderThanDays: string,
  artifactState: CleanupArtifactState,
  objectPrefix: string,
  advancedExpression: string,
): string {
  const filters: string[] = [];
  const trimmedDays = olderThanDays.trim();
  if (trimmedDays) {
    const parsedDays = Number(trimmedDays);
    if (!Number.isFinite(parsedDays) || parsedDays < 0 || parsedDays > 3650) {
      return "__invalid__: older than must be 0-3650 days";
    }
    if (parsedDays > 0) {
      const cutoff = new Date(Date.now() - parsedDays * 24 * 60 * 60 * 1000).toISOString();
      filters.push(`artifact.created_at <= "${escapeExpressionLiteral(cutoff)}"`);
    }
  }
  if (artifactState !== "any") {
    filters.push(`artifact.status = "${escapeExpressionLiteral(artifactState)}"`);
  }
  const prefix = objectPrefix.trim();
  if (prefix) {
    filters.push(`artifact.object = "${escapeExpressionLiteral(prefix)}*"`);
  }
  const extra = advancedExpression.trim();
  if (extra) {
    filters.push(`(${extra})`);
  }
  return filters.join(" && ");
}

function escapeExpressionLiteral(value: string): string {
  return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function cleanupPreviewEvidence(preview: ArtifactCleanupPreviewRecord | null): {
  ageRangeLabel: string;
  complete: boolean;
  missingLabel: string;
  objectsLabel: string;
  retentionLabel: string;
} {
  if (!preview) {
    return {
      ageRangeLabel: "Not previewed",
      complete: false,
      missingLabel: "Preview required",
      objectsLabel: "Not previewed",
      retentionLabel: "Not previewed",
    };
  }
  const ageRangeReady = Boolean(preview.oldest_created_at && preview.newest_created_at);
  const retentionReady =
    typeof preview.retained_count === "number" &&
    typeof preview.reference_protected_count === "number";
  const representativeObjects = preview.representative_objects ?? [];
  const objectsReady =
    preview.matched_count === 0 ||
    representativeObjects.length > 0 ||
    Boolean(preview.full_list_download_url);
  const missing: string[] = [];
  if (!ageRangeReady) {
    missing.push("age range");
  }
  if (!retentionReady) {
    missing.push("retention/protection");
  }
  if (!objectsReady) {
    missing.push("object list");
  }
  return {
    ageRangeLabel: ageRangeReady
      ? `${formatTime(preview.oldest_created_at as string)} to ${formatTime(preview.newest_created_at as string)}`
      : "Not reported by API",
    complete: missing.length === 0,
    missingLabel: missing.length > 0 ? missing.join(", ") : "None",
    objectsLabel: objectsReady
      ? preview.full_list_download_url
        ? "Full list available"
        : `${representativeObjects.length} representative`
      : "Not reported by API",
    retentionLabel: retentionReady
      ? `${preview.retained_count ?? 0} eligible / ${preview.reference_protected_count ?? 0} protected`
      : "Not reported by API",
  };
}

function formatCleanupDomains(domains: string[]): string {
  const labels = new Map(
    artifactCleanupDomainOptions.map((option) => [option.value, option.label]),
  );
  return domains.map((domain) => labels.get(domain as ArtifactCleanupDomain) ?? domain).join(", ");
}

function sameDomains(left: string[], right: string[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  const normalizedLeft = [...left].sort();
  const normalizedRight = [...right].sort();
  return normalizedLeft.every((value, index) => value === normalizedRight[index]);
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
  }
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${value} B`;
}
