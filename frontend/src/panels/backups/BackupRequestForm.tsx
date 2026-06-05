import type { FormEvent } from "react";
import { DatabaseBackup, Play } from "lucide-react";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { BACKUP_PATH_PLACEHOLDER } from "../../presets/backupPathPresets";
import type { AgentView } from "../../types";
import { formatVpsName } from "../../utils";

type BackupRequestFormProps = {
  agents: AgentView[];
  clientId: string;
  confirmed: boolean;
  includeConfig: boolean;
  note: string;
  onClientIdChange: (value: string) => void;
  onConfirmedChange: (value: boolean) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onNoteChange: (value: string) => void;
  onPathsTextChange: (value: string) => void;
  onProofTtlSecsChange: (value: number) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  pathsCount: number;
  pathsText: string;
  pending: boolean;
  proofReady: boolean;
  proofTtlSecs: number;
  selectedAgentName: string | null;
};

export function BackupRequestForm({
  agents,
  clientId,
  confirmed,
  includeConfig,
  note,
  onClientIdChange,
  onConfirmedChange,
  onIncludeConfigChange,
  onNoteChange,
  onPathsTextChange,
  onProofTtlSecsChange,
  onSubmit,
  pathsCount,
  pathsText,
  pending,
  proofReady,
  proofTtlSecs,
  selectedAgentName,
}: BackupRequestFormProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  return (
    <>
      <div className="sectionHeader compact">
        <h2>Request backup</h2>
        <span>{selectedAgentName ?? "Single-client metadata request"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>VPS</span>
          <select aria-label="Backup client" onChange={(event) => onClientIdChange(event.target.value)} value={clientId}>
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
            aria-label="Backup selected paths"
            onChange={(event) => onPathsTextChange(event.target.value)}
            placeholder={BACKUP_PATH_PLACEHOLDER}
            rows={5}
            value={pathsText}
          />
        </label>
        <label>
          <span>Note</span>
          <input aria-label="Backup note" onChange={(event) => onNoteChange(event.target.value)} placeholder="pre-migration snapshot" value={note} />
        </label>
        <div className="dispatchControls">
          <label>
            <span>Proof TTL</span>
            <input
              aria-label="Backup proof TTL seconds"
              max={3600}
              min={15}
              onChange={(event) => onProofTtlSecsChange(Number(event.target.value))}
              type="number"
              value={proofTtlSecs}
            />
          </label>
          <label className="checkLine inlineCheck">
            <input checked={includeConfig} onChange={(event) => onIncludeConfigChange(event.target.checked)} type="checkbox" />
            <span>Include agent config</span>
          </label>
        </div>
        <label className="checkLine">
          <input checked={confirmed} onChange={(event) => onConfirmedChange(event.target.checked)} type="checkbox" />
          <span>Confirmed</span>
        </label>
        <div className="backupScopeList">
          <DatabaseBackup size={18} />
          <span>{includeConfig ? "config" : "no config"}</span>
          <span>{pathsCount} path{pathsCount === 1 ? "" : "s"}</span>
        </div>
        <button className="primaryAction" disabled={pending || !proofReady || !clientId} type="submit">
          <Play size={17} />
          Request backup
        </button>
      </form>
    </>
  );
}
