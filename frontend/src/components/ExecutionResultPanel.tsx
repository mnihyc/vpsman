import { ExternalLink, X } from "lucide-react";
import { bulkOutcomeSummary, bulkProgressLabel, type BulkJobProgress } from "../bulkJobProgress";
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
          pushed
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
    </section>
  );
}
