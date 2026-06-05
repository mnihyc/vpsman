import { RotateCcw } from "lucide-react";
import type { AgentView } from "../../types";
import { TargetImpactPreview } from "../TargetImpactPreview";

type RestoreRollbackFormProps = {
  forceUnprivileged: boolean;
  onForceUnprivilegedChange: (value: boolean) => void;
  onRestoreJobIdChange: (value: string) => void;
  onRestoreRollbackConfirmedChange: (value: boolean) => void;
  onRestoreRollbackTimeoutSecsChange: (value: number) => void;
  onRunRestoreRollback: () => void;
  onTargetClientIdChange: (value: string) => void;
  pending: boolean;
  proofReady: boolean;
  restoreJobId: string;
  restoreRollbackConfirmed: boolean;
  restoreRollbackTimeoutSecs: number;
  targetAgent: AgentView | null;
  targetClientId: string;
};

export function RestoreRollbackForm({
  forceUnprivileged,
  onForceUnprivilegedChange,
  onRestoreJobIdChange,
  onRestoreRollbackConfirmedChange,
  onRestoreRollbackTimeoutSecsChange,
  onRunRestoreRollback,
  onTargetClientIdChange,
  pending,
  proofReady,
  restoreJobId,
  restoreRollbackConfirmed,
  restoreRollbackTimeoutSecs,
  targetAgent,
  targetClientId,
}: RestoreRollbackFormProps) {
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Rollback restore</h2>
        <span>Uses retained restore output evidence to revert restored files</span>
      </div>
      <form className="dispatchForm" onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>Restore job ID</span>
          <input
            aria-label="Restore rollback source job id"
            onChange={(event) => onRestoreJobIdChange(event.target.value)}
            placeholder="completed restore job UUID"
            value={restoreJobId}
          />
        </label>
        <label>
          <span>Target VPS</span>
          <input
            aria-label="Restore rollback target VPS ID"
            onChange={(event) => onTargetClientIdChange(event.target.value)}
            placeholder="VPS ID from details"
            value={targetClientId}
          />
        </label>
        <label>
          <span>Timeout seconds</span>
          <input
            aria-label="Restore rollback timeout seconds"
            max={3600}
            min={1}
            onChange={(event) => onRestoreRollbackTimeoutSecsChange(Number(event.target.value))}
            type="number"
            value={restoreRollbackTimeoutSecs}
          />
        </label>
        <label className="checkLine">
          <input
            checked={restoreRollbackConfirmed}
            onChange={(event) => onRestoreRollbackConfirmedChange(event.target.checked)}
            type="checkbox"
          />
          <span>Confirmed restore rollback</span>
        </label>
        <TargetImpactPreview
          forceUnprivileged={forceUnprivileged}
          mode="restore"
          targets={targetAgent ? [targetAgent] : []}
          title="Rollback target impact"
        />
        <label className="checkLine">
          <input
            aria-label="Force unprivileged restore rollback best effort"
            checked={forceUnprivileged}
            onChange={(event) => onForceUnprivilegedChange(event.target.checked)}
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
        <button
          className="secondaryAction dangerAction"
          disabled={pending || !proofReady || !restoreJobId || !targetClientId}
          onClick={onRunRestoreRollback}
          type="button"
        >
          <RotateCcw size={17} />
          Rollback restore
        </button>
      </form>
    </>
  );
}
