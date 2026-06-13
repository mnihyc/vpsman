import { RefreshCw, ShieldCheck, Trash2, XCircle } from "lucide-react";
import { useState } from "react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { CrudPager } from "../../components/CrudPager";
import { serverJobStatusBadgeClass } from "../../jobStatusPresentation";
import type {
  ArtifactCleanupPreviewRecord,
  ServerJobRecord,
} from "../../types";
import { formatTime, shortHash, shortId } from "../../utils";

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
    previewHash: string,
  ) => Promise<ServerJobRecord>;
  onPreviewCleanup: (
    expression: string,
  ) => Promise<ArtifactCleanupPreviewRecord>;
  onRefresh: () => void;
}) {
  const [expression, setExpression] = useState('artifact.domain = "job_output"');
  const [preview, setPreview] = useState<ArtifactCleanupPreviewRecord | null>(
    null,
  );
  const [pending, setPending] = useState(false);
  const [pendingJobId, setPendingJobId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const summary =
    error ??
    (preview
      ? `${preview.matched_count} artifacts, ${formatBytes(preview.matched_bytes)}`
      : `${jobs.length} server jobs`);

  async function previewCleanup() {
    setPending(true);
    setError(null);
    try {
      setPreview(await onPreviewCleanup(expression));
      setConfirmOpen(false);
    } catch (previewError) {
      setPreview(null);
      setError(
        previewError instanceof Error
          ? previewError.message
          : "Cleanup preview failed",
      );
    } finally {
      setPending(false);
    }
  }

  async function queueCleanup() {
    if (!preview) {
      return;
    }
    setPending(true);
    setError(null);
    try {
      await onCreateCleanupJob(preview.expression, preview.preview_hash);
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

  async function cancelJob(job: ServerJobRecord) {
    setPendingJobId(job.id);
    setError(null);
    try {
      await onCancelJob(job.id);
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
          <label>
            <span>Expression</span>
            <textarea
              rows={3}
              value={expression}
              onChange={(event) => {
                setExpression(event.target.value);
                setPreview(null);
              }}
            />
          </label>
          <label>
            <span>Preview hash</span>
            <input readOnly value={preview?.preview_hash ?? ""} />
          </label>
          <label>
            <span>Matched</span>
            <input
              readOnly
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
              disabled={pending || !expression.trim()}
              onClick={() => void previewCleanup()}
              type="button"
            >
              <ShieldCheck size={16} />
              Preview
            </button>
            <button
              className="dangerAction"
              disabled={pending || !preview}
              onClick={() => setConfirmOpen(true)}
              type="button"
            >
              <Trash2 size={16} />
              Queue cleanup
            </button>
          </div>
        </div>
        <ConfirmationPrompt
          confirmLabel="Queue cleanup"
          detail="Queues a server-side cleanup job for the current preview hash."
          error={error}
          items={[
            { label: "Expression", value: preview?.expression ?? expression },
            { label: "Artifacts", value: preview?.matched_count ?? 0 },
            {
              label: "Bytes",
              value: preview ? formatBytes(preview.matched_bytes) : "0B",
            },
            {
              label: "Preview hash",
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
                      onClick={() => void cancelJob(job)}
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
      </div>
    </div>
  );
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
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
