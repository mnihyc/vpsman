import type { FormEvent } from "react";
import { RotateCcw } from "lucide-react";
import { VpsCombobox } from "../../components/VpsCombobox";
import type { AgentView, BackupRequestRecord } from "../../types";
import { shortId } from "../../utils";

type RestorePlanFormProps = {
  agents: AgentView[];
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  confirmationOpen: boolean;
  onNoteChange: (value: string) => void;
  onSourceIdChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onTargetIdChange: (value: string) => void;
  pending: boolean;
  privilegeReady: boolean;
  restoreDestinationRoot: string;
  restoreIncludeConfig: boolean;
  restoreNote: string;
  restorePaths: string[];
  restoreSourceId: string;
  restoreTargetId: string;
  restoreTargetName: string | null;
};

export function RestorePlanForm({
  agents,
  backups,
  clientLabel,
  confirmationOpen,
  onNoteChange,
  onSourceIdChange,
  onSubmit,
  onTargetIdChange,
  pending,
  privilegeReady,
  restoreDestinationRoot,
  restoreIncludeConfig,
  restoreNote,
  restorePaths,
  restoreSourceId,
  restoreTargetId,
  restoreTargetName,
}: RestorePlanFormProps) {
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Plan restore</h2>
        <span>{restoreTargetName ?? "Metadata-only restore plan"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Source backup</span>
          <select
            aria-label="Restore source backup request"
            onChange={(event) => onSourceIdChange(event.target.value)}
            value={restoreSourceId}
          >
            <option value="">Select backup request</option>
            {backups.map((backup) => (
              <option key={backup.id} value={backup.id}>
                {shortId(backup.id)} from {clientLabel(backup.client_id)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Target VPS</span>
          <VpsCombobox
            agents={agents}
            ariaLabel="Restore target client"
            onChange={onTargetIdChange}
            placeholder="Search target VPS"
            value={restoreTargetId}
          />
        </label>
        <div className="restoreReadOnlyField">
          <span>Source scope</span>
          <strong title={restoreScopeTitle(restoreIncludeConfig, restorePaths)}>
            {restoreScopeLabel(restoreIncludeConfig, restorePaths)}
          </strong>
        </div>
        <div className="restoreReadOnlyField">
          <span>Generated destination root</span>
          <strong title={restoreDestinationRoot || "Select source and target"}>
            {restoreDestinationRoot || "Select source and target"}
          </strong>
        </div>
        <label>
          <span>Note</span>
          <input
            aria-label="Restore note"
            onChange={(event) => onNoteChange(event.target.value)}
            placeholder="restore rehearsal"
            value={restoreNote}
          />
        </label>
        <div className="backupScopeList">
          <RotateCcw size={18} />
          <span>{restoreIncludeConfig ? "config" : "no config"}</span>
          <span>
            {restorePaths.length} path{restorePaths.length === 1 ? "" : "s"}
          </span>
        </div>
        {!confirmationOpen && (
          <button
            className="primaryAction"
            disabled={
              pending ||
              !privilegeReady ||
              !restoreSourceId ||
              !restoreTargetId ||
              !restoreDestinationRoot
            }
            type="submit"
          >
            <RotateCcw size={17} />
            Review plan
          </button>
        )}
      </form>
    </>
  );
}

function restoreScopeLabel(includeConfig: boolean, paths: string[]): string {
  const parts = [];
  if (includeConfig) {
    parts.push("agent config");
  }
  if (paths.length > 0) {
    parts.push(`${paths.length} path${paths.length === 1 ? "" : "s"}`);
  }
  return parts.join(", ") || "metadata only";
}

function restoreScopeTitle(includeConfig: boolean, paths: string[]): string {
  const parts = includeConfig ? ["agent config"] : [];
  parts.push(...paths);
  return parts.join("\n") || "No paths recorded";
}
