import type { FormEvent } from "react";
import { Upload } from "lucide-react";
import type { BackupRequestRecord } from "../../types";
import { shortId } from "../../utils";

type ArtifactUploadFormProps = {
  artifactBackupId: string;
  artifactConfirmationOpen: boolean;
  artifactFile: File | null;
  artifactObjectKey: string;
  artifactUploadMode: "inline" | "chunked";
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  handoffConfirmationOpen: boolean;
  onArtifactBackupIdChange: (value: string) => void;
  onArtifactFileChange: (value: File | null) => void;
  onArtifactObjectKeyChange: (value: string) => void;
  onArtifactUploadModeChange: (value: "inline" | "chunked") => void;
  onHandoffJobIdChange: (value: string) => void;
  onHandoffSubmit: () => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  handoffJobId: string;
  pending: boolean;
};

export function ArtifactUploadForm({
  artifactBackupId,
  artifactConfirmationOpen,
  artifactFile,
  artifactObjectKey,
  artifactUploadMode,
  backups,
  clientLabel,
  handoffConfirmationOpen,
  onArtifactBackupIdChange,
  onArtifactFileChange,
  onArtifactObjectKeyChange,
  onArtifactUploadModeChange,
  onHandoffJobIdChange,
  onHandoffSubmit,
  onSubmit,
  handoffJobId,
  pending,
}: ArtifactUploadFormProps) {
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Upload artifact</h2>
        <span>Plain tar artifact bytes for a recorded backup request</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Backup request</span>
          <select aria-label="Artifact backup request" onChange={(event) => onArtifactBackupIdChange(event.target.value)} value={artifactBackupId}>
            <option value="">Select backup request</option>
            {backups.map((backup) => (
              <option key={backup.id} value={backup.id}>
                {shortId(backup.id)} from {clientLabel(backup.client_id)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Object key</span>
          <input
            aria-label="Artifact object key"
            onChange={(event) => onArtifactObjectKeyChange(event.target.value)}
            placeholder="backups/client/id.tar"
            value={artifactObjectKey}
          />
        </label>
        <label>
          <span>Artifact file</span>
          <input aria-label="Backup artifact file" onChange={(event) => onArtifactFileChange(event.target.files?.[0] ?? null)} type="file" />
        </label>
        <label>
          <span>Upload mode</span>
          <select
            aria-label="Backup artifact upload mode"
            onChange={(event) => onArtifactUploadModeChange(event.target.value === "chunked" ? "chunked" : "inline")}
            value={artifactUploadMode}
          >
            <option value="inline">Inline</option>
            <option value="chunked">Chunked session</option>
          </select>
        </label>
        {!artifactConfirmationOpen && (
          <button className="primaryAction" disabled={pending || !artifactBackupId || !artifactFile} type="submit">
            <Upload size={17} />
            Review upload
          </button>
        )}
      </form>
      <div className="dispatchForm inlineRestoreAction">
        <label>
          <span>Source job ID</span>
          <input
            aria-label="Backup artifact handoff source job ID"
            onChange={(event) => onHandoffJobIdChange(event.target.value)}
            placeholder="optional completed backup job"
            value={handoffJobId}
          />
        </label>
        {!handoffConfirmationOpen && (
          <button className="secondaryAction" disabled={pending || !artifactBackupId} onClick={onHandoffSubmit} type="button">
            <Upload size={17} />
            Review promotion
          </button>
        )}
      </div>
    </>
  );
}
