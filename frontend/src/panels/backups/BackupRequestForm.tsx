import type { FormEvent } from "react";
import { DatabaseBackup, Play } from "lucide-react";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  BACKUP_PATH_PLACEHOLDER,
  BACKUP_PATH_PRESETS,
} from "../../presets/backupPathPresets";
import { PathPresetButtons } from "./PathPresetButtons";
import type { AgentView } from "../../types";

type BackupRequestFormProps = {
  agents: AgentView[];
  clientId: string;
  confirmationOpen: boolean;
  includeConfig: boolean;
  note: string;
  onClientIdChange: (value: string) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onNoteChange: (value: string) => void;
  onPathsTextChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  pathsCount: number;
  pathsText: string;
  pending: boolean;
  privilegeReady: boolean;
  selectedAgentName: string | null;
};

export function BackupRequestForm({
  agents,
  clientId,
  confirmationOpen,
  includeConfig,
  note,
  onClientIdChange,
  onIncludeConfigChange,
  onNoteChange,
  onPathsTextChange,
  onSubmit,
  pathsCount,
  pathsText,
  pending,
  privilegeReady,
  selectedAgentName,
}: BackupRequestFormProps) {
  return (
    <>
      <div className="sectionHeader compact">
        <h2>Request backup</h2>
        <span>{selectedAgentName ?? "Single-client metadata request"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>VPS</span>
          <VpsCombobox
            agents={agents}
            ariaLabel="Backup client"
            onChange={onClientIdChange}
            placeholder="Search backup VPS"
            value={clientId}
          />
        </label>
        <label>
          <span>Selected paths</span>
          <textarea
            aria-label="Backup selected paths"
            onChange={(event) => onPathsTextChange(event.target.value)}
            placeholder={BACKUP_PATH_PLACEHOLDER}
            rows={5}
            value={pathsText}
          />
          <PathPresetButtons
            onApply={onPathsTextChange}
            presets={BACKUP_PATH_PRESETS}
          />
        </label>
        <label>
          <span>Note</span>
          <input
            aria-label="Backup note"
            onChange={(event) => onNoteChange(event.target.value)}
            placeholder="pre-migration snapshot"
            value={note}
          />
        </label>
        <div className="dispatchControls">
          <label className="checkLine inlineCheck">
            <input
              checked={includeConfig}
              onChange={(event) => onIncludeConfigChange(event.target.checked)}
              type="checkbox"
            />
            <span>Include agent config</span>
          </label>
        </div>
        <div className="backupScopeList">
          <DatabaseBackup size={18} />
          <span>{includeConfig ? "config" : "no config"}</span>
          <span>
            {pathsCount} path{pathsCount === 1 ? "" : "s"}
          </span>
        </div>
        {!confirmationOpen && (
          <button
            className="primaryAction"
            disabled={pending || !privilegeReady || !clientId}
            type="submit"
          >
            <Play size={17} />
            Review backup
          </button>
        )}
      </form>
    </>
  );
}
