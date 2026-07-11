import type { FormEvent } from "react";
import { DatabaseBackup, Play } from "lucide-react";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  BACKUP_PATH_PLACEHOLDER,
  BACKUP_PATH_PRESETS,
  presetPathsText,
} from "../../presets/backupPathPresets";
import { PathPresetButtons } from "./PathPresetButtons";
import type { AgentView, BackupMissingPathPolicy } from "../../types";

type BackupRequestFormProps = {
  agents: AgentView[];
  clientId: string;
  confirmationOpen: boolean;
  followSymlinks: boolean;
  includeConfig: boolean;
  missingPathPolicy: BackupMissingPathPolicy;
  note: string;
  onClientIdChange: (value: string) => void;
  onFollowSymlinksChange: (value: boolean) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onMissingPathPolicyChange: (value: BackupMissingPathPolicy) => void;
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
  followSymlinks,
  includeConfig,
  missingPathPolicy,
  note,
  onClientIdChange,
  onFollowSymlinksChange,
  onIncludeConfigChange,
  onMissingPathPolicyChange,
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
            onApply={(preset) => {
              onPathsTextChange(presetPathsText(preset.paths));
              onMissingPathPolicyChange(preset.missingPathPolicy);
            }}
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
        <div className="backupOptionStrip" aria-label="Backup collection options">
          <label className="checkLine inlineCheck">
            <input
              checked={includeConfig}
              onChange={(event) => onIncludeConfigChange(event.target.checked)}
              type="checkbox"
            />
            <span>Agent config</span>
          </label>
          <label
            className="checkLine inlineCheck"
            title="Default is off. Enable only when the backup should archive symlink target contents."
          >
            <input
              checked={followSymlinks}
              onChange={(event) => onFollowSymlinksChange(event.target.checked)}
              type="checkbox"
            />
            <span>Follow symlinks</span>
          </label>
          <label
            className="checkLine inlineCheck"
            title="Continue only when a selected root does not exist on this VPS. Unreadable paths and collection errors still fail the backup."
          >
            <input
              checked={missingPathPolicy === "skip"}
              onChange={(event) =>
                onMissingPathPolicyChange(event.target.checked ? "skip" : "fail")
              }
              type="checkbox"
            />
            <span>Skip missing roots</span>
          </label>
        </div>
        <div className="backupScopeList">
          <DatabaseBackup size={18} />
          <span>{includeConfig ? "config" : "no config"}</span>
          <span>{followSymlinks ? "follows symlinks" : "no symlink follow"}</span>
          <span>{missingPathPolicy === "skip" ? "optional roots" : "strict roots"}</span>
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
