import { Play } from "lucide-react";
import type { AgentView } from "../../types";
import { TargetImpactPreview } from "../TargetImpactPreview";

type RestoreRunFormProps = {
  forceUnprivileged: boolean;
  onForceUnprivilegedChange: (value: boolean) => void;
  onArtifactFileChange: (value: File | null) => void;
  onArchivePathChange: (value: string) => void;
  onArchiveSha256HexChange: (value: string) => void;
  onDryRunChange: (value: boolean) => void;
  onPrivateKeyHexChange: (value: string) => void;
  onPostRestoreArgvChange: (value: string) => void;
  onRestoreRunConfirmedChange: (value: boolean) => void;
  onRestoreTimeoutSecsChange: (value: number) => void;
  onRunRestore: () => void;
  pending: boolean;
  proofReady: boolean;
  restoreArchivePath: string;
  restoreArchiveSha256Hex: string;
  restoreArtifactFile: File | null;
  restoreDryRun: boolean;
  restorePrivateKeyHex: string;
  restorePostRestoreArgv: string;
  restoreRunConfirmed: boolean;
  restoreSourceId: string;
  restoreTarget: AgentView | null;
  restoreTargetId: string;
  restoreTimeoutSecs: number;
};

export function RestoreRunForm({
  forceUnprivileged,
  onForceUnprivilegedChange,
  onArtifactFileChange,
  onArchivePathChange,
  onArchiveSha256HexChange,
  onDryRunChange,
  onPrivateKeyHexChange,
  onPostRestoreArgvChange,
  onRestoreRunConfirmedChange,
  onRestoreTimeoutSecsChange,
  onRunRestore,
  pending,
  proofReady,
  restoreArchivePath,
  restoreArchiveSha256Hex,
  restoreArtifactFile,
  restoreDryRun,
  restorePrivateKeyHex,
  restorePostRestoreArgv,
  restoreRunConfirmed,
  restoreSourceId,
  restoreTarget,
  restoreTargetId,
  restoreTimeoutSecs,
}: RestoreRunFormProps) {
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Run restore</h2>
        <span>Runs browser-decrypted or agent-local archive restores with rehearsal support</span>
      </div>
      <form className="dispatchForm" onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>Artifact file (optional)</span>
          <input aria-label="Restore artifact file" onChange={(event) => onArtifactFileChange(event.target.files?.[0] ?? null)} type="file" />
        </label>
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
          <span>Archive SHA-256</span>
          <input
            aria-label="Agent-local restore archive SHA-256"
            onChange={(event) => onArchiveSha256HexChange(event.target.value)}
            placeholder="optional 64 hex characters"
            value={restoreArchiveSha256Hex}
          />
        </label>
        <label>
          <span>Backup private key hex</span>
          <input
            aria-label="Backup private key hex"
            onChange={(event) => onPrivateKeyHexChange(event.target.value)}
            placeholder={restoreArchivePath ? "not needed for agent-local archive" : "64 hex characters"}
            type="password"
            value={restorePrivateKeyHex}
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
          <input checked={restoreRunConfirmed} onChange={(event) => onRestoreRunConfirmedChange(event.target.checked)} type="checkbox" />
          <span>Confirmed executable restore</span>
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
        <button
          className="primaryAction dangerPrimary"
          disabled={pending || !proofReady || !restoreSourceId || !restoreTargetId}
          onClick={onRunRestore}
          type="button"
        >
          <Play size={17} />
          Run restore
        </button>
      </form>
    </>
  );
}
