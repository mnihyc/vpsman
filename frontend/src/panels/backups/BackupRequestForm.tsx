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
  onClientIdChange: (value: string) => void;
  onFollowSymlinksChange: (value: boolean) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onMissingPathPolicyChange: (value: BackupMissingPathPolicy) => void;
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
  onClientIdChange,
  onFollowSymlinksChange,
  onIncludeConfigChange,
  onMissingPathPolicyChange,
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
        <h2>Backup scope</h2>
        <span>{selectedAgentName ?? "Single VPS job and artifact collection"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>VPS</span>
          <VpsCombobox
            agents={agents}
            ariaLabel="Backup client"
            className="actionDrawerInitialFocus"
            onChange={onClientIdChange}
            placeholder="Select VPS"
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
            disabled={pending || !clientId}
            title={
              privilegeReady
                ? "Review the frozen backup target and scope"
                : "Opens privilege unlock before preparing the backup review"
            }
            type="submit"
          >
            <Play size={17} />
            Review backup run
          </button>
        )}
      </form>
    </>
  );
}
