import { RotateCcw } from "lucide-react";
import { MAX_CONFIGURABLE_JOB_TIMEOUT_SECS } from "../../jobMaxTimeout";
import { VpsCombobox } from "../../components/VpsCombobox";
import type { AgentView } from "../../types";
import { TargetImpactPreview } from "../TargetImpactPreview";

type RestoreRollbackFormProps = {
  agents: AgentView[];
  confirmationOpen: boolean;
  forceUnprivileged: boolean;
  onForceUnprivilegedChange: (value: boolean) => void;
  onRestoreJobIdChange: (value: string) => void;
  onRestoreRollbackMaxTimeoutSecsChange: (value: number) => void;
  onRunRestoreRollback: () => void;
  onTargetClientIdChange: (value: string) => void;
  pending: boolean;
  privilegeReady: boolean;
  restoreJobId: string;
  restoreRollbackMaxTimeoutSecs: number;
  targetAgent: AgentView | null;
  targetClientId: string;
};

export function RestoreRollbackForm({
  agents,
  confirmationOpen,
  forceUnprivileged,
  onForceUnprivilegedChange,
  onRestoreJobIdChange,
  onRestoreRollbackMaxTimeoutSecsChange,
  onRunRestoreRollback,
  onTargetClientIdChange,
  pending,
  privilegeReady,
  restoreJobId,
  restoreRollbackMaxTimeoutSecs,
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
          <VpsCombobox
            agents={agents}
            ariaLabel="Restore rollback target VPS ID"
            onChange={onTargetClientIdChange}
            placeholder="Search rollback VPS"
            value={targetClientId}
          />
        </label>
        <label>
          <span>Max timeout seconds</span>
          <input
            aria-label="Restore rollback max timeout seconds"
            max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
            min={1}
            onChange={(event) => onRestoreRollbackMaxTimeoutSecsChange(Number(event.target.value))}
            type="number"
            value={restoreRollbackMaxTimeoutSecs}
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
            Review rollback
          </button>
        )}
      </form>
    </>
  );
}
