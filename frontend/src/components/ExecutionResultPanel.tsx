import { ExternalLink, X } from "lucide-react";
import {
  bulkOutcomeSummary,
  bulkProgressLabel,
  type BulkFailureReason,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { shortId } from "../utils";

export function ExecutionResultPanel({
  label = "Execution result",
  loading = false,
  onClearResults,
  onOpenJobDetails,
  progress,
}: {
  label?: string;
  loading?: boolean;
  onClearResults?: () => void;
  onOpenJobDetails?: (jobId: string) => void;
  progress: BulkJobProgress;
}) {
  return (
    <section className="executionResultPanel" aria-label={label}>
      <div className="executionResultHeader">
        <div>
          <strong>{label}</strong>
          <span>{loading ? "Polling job status" : bulkOutcomeSummary(progress)}</span>
        </div>
        {(onOpenJobDetails || onClearResults) && (
          <div className="executionResultActions">
            {onOpenJobDetails && (
              <button className="secondaryAction compactAction" onClick={() => onOpenJobDetails(progress.jobId)} type="button">
                <ExternalLink size={15} />
                <span>Open job details</span>
              </button>
            )}
            {onClearResults && (
              <button className="secondaryAction compactAction" disabled={loading} onClick={onClearResults} type="button">
                <X size={15} />
                <span>Clear results</span>
              </button>
            )}
          </div>
        )}
      </div>
      <div className="executionResultStats">
        <span>
          <strong>{shortId(progress.jobId)}</strong>
          job
        </span>
        <span>
          <strong>{progress.accepted}/{progress.expected}</strong>
          queued
        </span>
        <span>
          <strong>{progress.doing}</strong>
          doing
        </span>
        <span>
          <strong>{progress.retrieved}</strong>
          retrieved
        </span>
        <span>
          <strong>{progress.completed}</strong>
          done
        </span>
        <span>
          <strong>{progress.unavailable}</strong>
          unavailable
        </span>
        <span>
          <strong>{progress.failed}</strong>
          failed
        </span>
      </div>
      <p>{bulkProgressLabel(progress)}</p>
      <FailureReasonGroups reasons={progress.failureReasons ?? []} />
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
            <strong>{group.targets.length} failed</strong>
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
    const reason = failure.reason.trim() || "failed";
    const target = failure.target.trim() || "target";
    const targets = groups.get(reason) ?? [];
    targets.push(target);
    groups.set(reason, targets);
  }
  return Array.from(groups.entries())
    .map(([reason, targets]) => ({ reason, targets }))
    .sort((left, right) => right.targets.length - left.targets.length || left.reason.localeCompare(right.reason));
}
