import { Play } from "lucide-react";
import type { AgentView } from "../../types";
import { TargetImpactPreview } from "../TargetImpactPreview";

type RestoreRunFormProps = {
  confirmationOpen: boolean;
  forceUnprivileged: boolean;
  onForceUnprivilegedChange: (value: boolean) => void;
  onArchivePathChange: (value: string) => void;
  onArchiveSizeBytesChange: (value: string) => void;
  onArchiveSha256HexChange: (value: string) => void;
  onDryRunChange: (value: boolean) => void;
  onPostRestoreArgvChange: (value: string) => void;
  onRestoreTimeoutSecsChange: (value: number) => void;
  onRunRestore: () => void;
  pending: boolean;
  privilegeReady: boolean;
  restoreArchivePath: string;
  restoreArchiveSizeBytes: string;
  restoreArchiveSha256Hex: string;
  restoreDryRun: boolean;
  restorePostRestoreArgv: string;
  restoreSourceId: string;
  restoreTarget: AgentView | null;
  restoreTargetId: string;
  restoreTimeoutSecs: number;
};

export function RestoreRunForm({
  confirmationOpen,
  forceUnprivileged,
  onForceUnprivilegedChange,
  onArchivePathChange,
  onArchiveSizeBytesChange,
  onArchiveSha256HexChange,
  onDryRunChange,
  onPostRestoreArgvChange,
  onRestoreTimeoutSecsChange,
  onRunRestore,
  pending,
  privilegeReady,
  restoreArchivePath,
  restoreArchiveSizeBytes,
  restoreArchiveSha256Hex,
  restoreDryRun,
  restorePostRestoreArgv,
  restoreSourceId,
  restoreTarget,
  restoreTargetId,
  restoreTimeoutSecs,
}: RestoreRunFormProps) {
  const archiveReady = Boolean(
    restoreArchivePath.trim() &&
      restoreArchiveSizeBytes.trim() &&
      /^[0-9a-fA-F]{64}$/.test(restoreArchiveSha256Hex.trim()),
  );
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Run restore</h2>
        <span>Runs agent-local archive restores with rehearsal support</span>
      </div>
      <form className="dispatchForm" onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>Agent-local archive path</span>
          <input
            aria-label="Agent-local restore archive path"
            onChange={(event) => onArchivePathChange(event.target.value)}
            placeholder="/var/lib/vpsman/restore/source.tar.zst"
            value={restoreArchivePath}
          />
        </label>
        <label>
          <span>Archive size bytes</span>
          <input
            aria-label="Agent-local restore archive size bytes"
            min={1}
            onChange={(event) => onArchiveSizeBytesChange(event.target.value)}
            placeholder="1048576"
            type="number"
            value={restoreArchiveSizeBytes}
          />
        </label>
        <label>
          <span>Archive SHA-256</span>
          <input
            aria-label="Agent-local restore archive SHA-256"
            onChange={(event) => onArchiveSha256HexChange(event.target.value)}
            placeholder="64 hex characters"
            value={restoreArchiveSha256Hex}
          />
        </label>
        <label>
          <span>Post-restore argv</span>
          <input
            aria-label="Post-restore argv"
            onChange={(event) => onPostRestoreArgvChange(event.target.value)}
            placeholder="/usr/local/sbin/post-restore-check --json"
            value={restorePostRestoreArgv}
          />
        </label>
        <label>
          <span>Timeout seconds</span>
          <input
            aria-label="Restore timeout seconds"
            max={3600}
            min={1}
            onChange={(event) => onRestoreTimeoutSecsChange(Number(event.target.value))}
            type="number"
            value={restoreTimeoutSecs}
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
