import { RotateCcw } from "lucide-react";
import type { AgentView } from "../../types";
import { TargetImpactPreview } from "../TargetImpactPreview";

type RestoreRollbackFormProps = {
  confirmationOpen: boolean;
  forceUnprivileged: boolean;
  onForceUnprivilegedChange: (value: boolean) => void;
  onRestoreJobIdChange: (value: string) => void;
  onRestoreRollbackTimeoutSecsChange: (value: number) => void;
  onRunRestoreRollback: () => void;
  onTargetClientIdChange: (value: string) => void;
  pending: boolean;
  privilegeReady: boolean;
  restoreJobId: string;
  restoreRollbackTimeoutSecs: number;
  targetAgent: AgentView | null;
  targetClientId: string;
};

export function RestoreRollbackForm({
  confirmationOpen,
  forceUnprivileged,
  onForceUnprivilegedChange,
  onRestoreJobIdChange,
  onRestoreRollbackTimeoutSecsChange,
  onRunRestoreRollback,
  onTargetClientIdChange,
  pending,
  privilegeReady,
  restoreJobId,
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
        {!confirmationOpen && (
          <button
            className="secondaryAction dangerAction"
            disabled={pending || !privilegeReady || !restoreJobId || !targetClientId}
            onClick={onRunRestoreRollback}
            type="button"
          >
            <RotateCcw size={17} />
            Rollback restore
          </button>
        )}
      </form>
    </>
  );
}
