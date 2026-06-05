import type { FormEvent } from "react";
import { RotateCcw } from "lucide-react";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { RESTORE_PATH_PLACEHOLDER } from "../../presets/backupPathPresets";
import type { AgentView, BackupRequestRecord } from "../../types";
import { formatVpsName, shortId } from "../../utils";

type RestorePlanFormProps = {
  agents: AgentView[];
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  onDestinationRootChange: (value: string) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onNoteChange: (value: string) => void;
  onPathsTextChange: (value: string) => void;
  onRestoreConfirmedChange: (value: boolean) => void;
  onSourceIdChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onTargetIdChange: (value: string) => void;
  pending: boolean;
  proofReady: boolean;
  restoreConfirmed: boolean;
  restoreDestinationRoot: string;
  restoreIncludeConfig: boolean;
  restoreNote: string;
  restorePathsCount: number;
  restorePathsText: string;
  restoreSourceId: string;
  restoreTargetId: string;
  restoreTargetName: string | null;
};

export function RestorePlanForm({
  agents,
  backups,
  clientLabel,
  onDestinationRootChange,
  onIncludeConfigChange,
  onNoteChange,
  onPathsTextChange,
  onRestoreConfirmedChange,
  onSourceIdChange,
  onSubmit,
  onTargetIdChange,
  pending,
  proofReady,
  restoreConfirmed,
  restoreDestinationRoot,
  restoreIncludeConfig,
  restoreNote,
  restorePathsCount,
  restorePathsText,
  restoreSourceId,
  restoreTargetId,
  restoreTargetName,
}: RestorePlanFormProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Plan restore</h2>
        <span>{restoreTargetName ?? "Metadata-only restore plan"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Source backup</span>
          <select aria-label="Restore source backup request" onChange={(event) => onSourceIdChange(event.target.value)} value={restoreSourceId}>
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
          <select aria-label="Restore target client" onChange={(event) => onTargetIdChange(event.target.value)} value={restoreTargetId}>
            <option value="">Select VPS</option>
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {formatVpsName(agent, vpsNameDisplayMode)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Selected paths</span>
          <textarea
            aria-label="Restore selected paths"
            onChange={(event) => onPathsTextChange(event.target.value)}
            placeholder={RESTORE_PATH_PLACEHOLDER}
            rows={4}
            value={restorePathsText}
          />
        </label>
        <label>
          <span>Destination root</span>
          <input
            aria-label="Restore destination root"
            onChange={(event) => onDestinationRootChange(event.target.value)}
            placeholder="/restore"
            value={restoreDestinationRoot}
          />
        </label>
        <label>
          <span>Note</span>
          <input aria-label="Restore note" onChange={(event) => onNoteChange(event.target.value)} placeholder="restore rehearsal" value={restoreNote} />
        </label>
        <label className="checkLine inlineCheck">
          <input checked={restoreIncludeConfig} onChange={(event) => onIncludeConfigChange(event.target.checked)} type="checkbox" />
          <span>Include agent config</span>
        </label>
        <label className="checkLine">
          <input checked={restoreConfirmed} onChange={(event) => onRestoreConfirmedChange(event.target.checked)} type="checkbox" />
          <span>Confirmed metadata plan</span>
        </label>
        <div className="backupScopeList">
          <RotateCcw size={18} />
          <span>{restoreIncludeConfig ? "config" : "no config"}</span>
          <span>{restorePathsCount} path{restorePathsCount === 1 ? "" : "s"}</span>
        </div>
        <button className="primaryAction" disabled={pending || !proofReady || !restoreSourceId || !restoreTargetId} type="submit">
          <RotateCcw size={17} />
          Plan restore
        </button>
      </form>
    </>
  );
}
