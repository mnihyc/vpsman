import { RefreshCw, ShieldCheck, Trash2, XCircle } from "lucide-react";
import { useState } from "react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { CrudPager } from "../../components/CrudPager";
import { useReviewGenerationGuard, waitForReviewRender } from "../../hooks/useReviewGenerationGuard";
import { serverJobStatusBadgeClass } from "../../jobStatusPresentation";
import type {
  ArtifactCleanupPreviewRecord,
  ServerJobRecord,
} from "../../types";
import { formatTime, shortHash, shortId } from "../../utils";

type ArtifactCleanupDomain = "job_output" | "file_transfer" | "backup_artifact";

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
  const [expression, setExpression] = useState('artifact.domain = "job_output"');
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
  const summary =
    error ??
    previewStatus ??
    (preview
      ? `${preview.matched_count} artifacts, ${formatBytes(preview.matched_bytes)}`
      : `${jobs.length} server jobs`);

  async function previewCleanup() {
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
          <label className="artifactCleanupExpression">
            <span>Expression</span>
            <textarea
              rows={3}
              title="Expression filters artifacts inside the selected domains. Domain authority is selected separately."
              value={expression}
              onChange={(event) => {
                invalidateReviewGeneration();
                setExpression(event.target.value);
                setPreview(null);
                setConfirmOpen(false);
                setPreviewStatus(null);
              }}
            />
          </label>
          <div className="artifactCleanupDomains">
            <span>Domains</span>
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
          <label>
            <span>Preview hash</span>
            <input
              readOnly
              title={preview?.preview_hash ?? "Preview before queueing cleanup"}
              value={preview?.preview_hash ?? ""}
            />
          </label>
          <label>
            <span>Matched</span>
            <input
              readOnly
              title={
                preview
                  ? `${preview.matched_count} artifacts, ${formatBytes(preview.matched_bytes)}`
                  : "Preview before queueing cleanup"
              }
              value={
                preview
                  ? `${preview.matched_count} / ${formatBytes(preview.matched_bytes)}`
                  : ""
              }
            />
          </label>
          <div className="retentionActions">
            <button
              className="secondaryAction"
              disabled={pending || !expression.trim() || domains.length === 0}
              onClick={() => void previewCleanup()}
              title="Build a reviewed cleanup snapshot for the selected domains"
              type="button"
            >
              <ShieldCheck size={16} />
              Preview
            </button>
            <button
              className="dangerAction"
              disabled={pending || !preview}
              onClick={() => setConfirmOpen(true)}
              title="Queue cleanup using the reviewed expression, domains, and preview hash"
              type="button"
            >
              <Trash2 size={16} />
              Queue cleanup
            </button>
          </div>
        </div>
        <ConfirmationPrompt
          confirmLabel="Queue cleanup"
          detail="Queues a server-side cleanup job for the reviewed artifact set."
          error={error}
          items={[
            { label: "Expression", value: preview?.expression ?? expression },
            {
              label: "Domains",
              title: formatCleanupDomains(preview?.domains ?? domains),
              value: formatCleanupDomains(preview?.domains ?? domains),
            },
            { label: "Artifacts", value: preview?.matched_count ?? 0 },
            {
              label: "Bytes",
              value: preview ? formatBytes(preview.matched_bytes) : "0B",
            },
            {
              label: "Preview hash",
              title: preview?.preview_hash,
              value: preview ? shortHash(preview.preview_hash) : "-",
            },
          ]}
          onCancel={() => setConfirmOpen(false)}
          onConfirm={() => void queueCleanup()}
          open={confirmOpen}
          pending={pending}
          title="Confirm artifact cleanup"
          tone="danger"
        />
      </div>
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Server jobs</h2>
            <span>{jobs.length} retained control-plane jobs</span>
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
        <CrudPager
          fields={[
            { label: "Type", value: (job) => job.job_type },
            { label: "Status", value: (job) => job.status },
            { label: "Matched", value: (job) => job.matched_count },
            { label: "Deleted", value: (job) => job.deleted_count },
            { label: "Created", value: (job) => formatTime(job.created_at) },
          ]}
          itemLabel="jobs"
          items={jobs}
          pageSize={10}
          title="Server job records"
          empty={
            <div className="emptyState">
              <Trash2 size={22} />
              <strong>No server jobs</strong>
              <span>Artifact cleanup jobs appear here.</span>
            </div>
          }
        >
          {(rows) => (
            <div className="table historyTable">
              <div className="historyRow serverJobGrid heading">
                <span>Job</span>
                <span>Status</span>
                <span>Matched</span>
                <span>Deleted</span>
                <span>Created</span>
                <span>Action</span>
              </div>
              {rows.map((job) => (
                <div className="historyRow serverJobGrid" key={job.id}>
                  <span className="historyPrimary">
                    <strong>{displayToken(job.job_type)}</strong>
                    <small>{shortId(job.id)}</small>
                  </span>
                  <span className="historyPrimary">
                    <span className={`status ${serverJobStatusBadgeClass(job.status)}`}>
                      {displayToken(job.status)}
                    </span>
                    <small>{job.error ?? job.expression ?? "no details"}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{job.matched_count}</strong>
                    <small>{formatBytes(job.matched_bytes)}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{job.deleted_count}</strong>
                    <small>{formatBytes(job.deleted_bytes)}</small>
                  </span>
                  <span>{formatTime(job.created_at)}</span>
                  <span>
                    <button
                      className="secondaryAction compactAction dangerAction"
                      disabled={pendingJobId === job.id || job.status !== "queued"}
                      onClick={() => reviewCancelJob(job)}
                      title="Cancel queued server job"
                      type="button"
                    >
                      <XCircle size={14} />
                      Cancel
                    </button>
                  </span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
        <ConfirmationPrompt
          confirmLabel="Cancel job"
          detail="Cancel the reviewed queued server-side maintenance job."
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
          title="Confirm server job cancellation"
          tone="danger"
        />
      </div>
    </div>
  );
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}

function formatCleanupDomains(domains: string[]): string {
  const labels = new Map(
    artifactCleanupDomainOptions.map((option) => [option.value, option.label]),
  );
  return domains.map((domain) => labels.get(domain as ArtifactCleanupDomain) ?? domain).join(", ");
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)}G`;
  }
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)}M`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)}K`;
  }
  return `${value}B`;
}
