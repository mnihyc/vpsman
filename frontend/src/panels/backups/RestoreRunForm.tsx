import { Play } from "lucide-react";
import { MAX_CONFIGURABLE_JOB_TIMEOUT_SECS } from "../../jobMaxTimeout";
import type { AgentView } from "../../types";
import { TargetImpactPreview } from "../TargetImpactPreview";
import {
  RestoreArchiveTransferSelect,
  type RestoreArchiveTransferOption,
} from "./RestoreArchiveTransferSelect";

type RestoreRunFormProps = {
  archiveEmptyMessage: string;
  archiveTransferKey: string;
  archiveTransferOptions: RestoreArchiveTransferOption[];
  confirmationOpen: boolean;
  forceUnprivileged: boolean;
  onArchiveTransferChange: (value: string) => void;
  onDryRunChange: (value: boolean) => void;
  onForceUnprivilegedChange: (value: boolean) => void;
  onPostRestoreArgvChange: (value: string) => void;
  onRestoreMaxTimeoutSecsChange: (value: number) => void;
  onRunRestore: () => void;
  pending: boolean;
  privilegeReady: boolean;
  restoreDryRun: boolean;
  restorePostRestoreArgv: string;
  restoreSourceId: string;
  restoreTarget: AgentView | null;
  restoreTargetId: string;
  restoreMaxTimeoutSecs: number;
};

export function RestoreRunForm({
  archiveEmptyMessage,
  archiveTransferKey,
  archiveTransferOptions,
  confirmationOpen,
  forceUnprivileged,
  onArchiveTransferChange,
  onDryRunChange,
  onForceUnprivilegedChange,
  onPostRestoreArgvChange,
  onRestoreMaxTimeoutSecsChange,
  onRunRestore,
  pending,
  privilegeReady,
  restoreDryRun,
  restorePostRestoreArgv,
  restoreSourceId,
  restoreTarget,
  restoreTargetId,
  restoreMaxTimeoutSecs,
}: RestoreRunFormProps) {
  const archiveReady = Boolean(
    archiveTransferOptions.some((option) => option.key === archiveTransferKey),
  );
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Run restore</h2>
        <span>Runs agent-local archive restores with rehearsal support</span>
      </div>
      <form className="dispatchForm" onSubmit={(event) => event.preventDefault()}>
        <RestoreArchiveTransferSelect
          emptyMessage={archiveEmptyMessage}
          onChange={onArchiveTransferChange}
          options={archiveTransferOptions}
          value={archiveTransferKey}
        />
        <label>
          <span>Post-restore argv</span>
          <input
            aria-label="Post-restore argv"
            onChange={(event) => onPostRestoreArgvChange(event.target.value)}
            placeholder="/usr/local/sbin/post-restore-check --json"
            title="Command and arguments to run after restore, separated by spaces"
            value={restorePostRestoreArgv}
          />
        </label>
        <label>
          <span>Max timeout seconds</span>
          <input
            aria-label="Restore max timeout seconds"
            max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
            min={1}
            onChange={(event) => onRestoreMaxTimeoutSecsChange(Number(event.target.value))}
            type="number"
            value={restoreMaxTimeoutSecs}
          />
        </label>
        <label className="checkLine">
          <input checked={restoreDryRun} onChange={(event) => onDryRunChange(event.target.checked)} type="checkbox" />
          <span>Dry-run rehearsal</span>
        </label>
        <TargetImpactPreview
          forceUnprivileged={forceUnprivileged}
          mode="restore"
          targets={restoreTarget ? [restoreTarget] : []}
          title="Restore target impact"
        />
        <label className="checkLine">
          <input
            aria-label="Force unprivileged restore best effort"
            checked={forceUnprivileged}
            onChange={(event) => onForceUnprivilegedChange(event.target.checked)}
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
        {!confirmationOpen && (
          <button
            className="primaryAction dangerPrimary"
            disabled={
              pending ||
              !privilegeReady ||
              !restoreSourceId ||
              !restoreTargetId ||
              !archiveReady
            }
            onClick={onRunRestore}
            type="button"
          >
            <Play size={17} />
            Review restore
          </button>
        )}
      </form>
    </>
  );
}
