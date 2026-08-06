import { ExternalLink, X } from "lucide-react";
import { useEffect, useRef, type ReactNode } from "react";
import {
  bulkOutcomeSummary,
  bulkProgressLabel,
  type BulkFailureReason,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { scrollIntoViewWithMotion } from "../motion";
import { shortId } from "../utils";

export function ExecutionResultPanel({
  context,
  label = "Execution result",
  loading = false,
  onClearResults,
  onOpenJobDetails,
  onOpenJobHistory,
  progress,
  children,
}: {
  children?: ReactNode;
  context?: string;
  label?: string;
  loading?: boolean;
  onClearResults?: () => void;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenJobHistory?: () => void;
  progress: BulkJobProgress;
}) {
  const panelRef = useRef<HTMLElement | null>(null);
  const jobCount = progress.jobIds.length;
  const hasMultipleJobs = jobCount > 1;

  useEffect(() => {
    window.requestAnimationFrame(() => {
      const panel = panelRef.current;
      if (!panel) return;
      scrollIntoViewWithMotion(panel, { block: "start" });
      panel.focus({ preventScroll: true });
    });
  }, []);

  return (
    <section
      aria-label={label}
      className="executionResultPanel"
      ref={panelRef}
      tabIndex={-1}
    >
      <div className="executionResultHeader">
        <div>
          <strong>{label}</strong>
          <span title={context}>
            {context ? `${context} · ` : ""}
            {loading ? "Polling job status" : bulkOutcomeSummary(progress)}
          </span>
        </div>
        {(onOpenJobDetails || onOpenJobHistory || onClearResults) && (
          <div className="executionResultActions">
            {hasMultipleJobs && onOpenJobHistory ? (
              <button
                className="secondaryAction compactAction"
                onClick={onOpenJobHistory}
                title={`Open all ${jobCount} target-specific jobs in Jobs / History.`}
                type="button"
              >
                <ExternalLink size={15} />
                <span>Open job history</span>
              </button>
            ) : onOpenJobDetails ? (
              <button
                className="secondaryAction compactAction"
                onClick={() => onOpenJobDetails(progress.jobId)}
                title="Open this job in Jobs / History."
                type="button"
              >
                <ExternalLink size={15} />
                <span>Open job details</span>
              </button>
            ) : null}
            {onClearResults && (
              <button
                className="secondaryAction compactAction"
                disabled={loading}
                onClick={onClearResults}
                title="Clear these bulk operation results from the panel."
                type="button"
              >
                <X size={15} />
                <span>Clear results</span>
              </button>
            )}
          </div>
        )}
      </div>
      <div className="executionResultStats">
        <span>
          <strong>
            {hasMultipleJobs ? jobCount : shortId(progress.jobId)}
          </strong>
          {hasMultipleJobs ? "jobs" : "job"}
        </span>
        <span>
          <strong>{progress.terminal}/{progress.total}</strong>
          {hasMultipleJobs ? "target actions" : "targets"}
        </span>
        <span>
          <strong>{progress.in_progress}</strong>
          in progress
        </span>
        <span>
          <strong>{progress.retrieved}</strong>
          reported
        </span>
        <span>
          <strong>{progress.completed}</strong>
          completed
        </span>
        <span>
          <strong>{progress.skipped}</strong>
          skipped
        </span>
        <span>
          <strong>{progress.unavailable}</strong>
          pre-run unavailable
        </span>
        <span>
          <strong>{progress.unsuccessful}</strong>
          unsuccessful
        </span>
      </div>
      <p>{bulkProgressLabel(progress)}</p>
      <FailureReasonGroups reasons={progress.failureReasons ?? []} />
      {children}
    </section>
  );
}

export function FailureReasonGroups({ reasons }: { reasons: BulkFailureReason[] }) {
  const groups = groupFailureReasons(reasons);
  if (groups.length === 0) {
    return null;
  }
  return (
    <div className="executionFailureReasons" aria-label="Failed target reasons">
      {groups.map((group) => {
        const visibleTargets = group.targets.slice(0, 6);
        const more = group.targets.length - visibleTargets.length;
        return (
          <div className="executionFailureReason" key={group.reason}>
            <strong>{group.targets.length} unsuccessful</strong>
            <span title={group.reason}>{group.reason}</span>
            <small title={group.targets.join("\n")}>
              {visibleTargets.join(", ")}
              {more > 0 ? `, +${more} more` : ""}
            </small>
          </div>
        );
      })}
    </div>
  );
}

function groupFailureReasons(reasons: BulkFailureReason[]): Array<{ reason: string; targets: string[] }> {
  const groups = new Map<string, string[]>();
  for (const failure of reasons) {
    const reason =
      failure.reason.trim() ||
      "Target was unsuccessful without retained output detail. Open the job and inspect target evidence before retrying.";
    const target = failure.target.trim() || "target";
    const targets = groups.get(reason) ?? [];
    targets.push(target);
    groups.set(reason, targets);
  }
  return Array.from(groups.entries())
    .map(([reason, targets]) => ({ reason, targets }))
    .sort((left, right) => right.targets.length - left.targets.length || left.reason.localeCompare(right.reason));
}
