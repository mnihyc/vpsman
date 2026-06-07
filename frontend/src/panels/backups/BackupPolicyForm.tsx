import type { FormEvent } from "react";
import { CalendarClock, Save } from "lucide-react";
import { SearchExpressionInput } from "../../components/SearchExpressionInput";
import { BACKUP_PATH_PLACEHOLDER } from "../../presets/backupPathPresets";
import type { AgentView } from "../../types";

type BackupPolicyFormProps = {
  agents: AgentView[];
  includeConfig: boolean;
  intervalSecs: number;
  keepLast: number;
  name: string;
  onConfirmedChange: (value: boolean) => void;
  onEnabledChange: (value: boolean) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onIntervalSecsChange: (value: number) => void;
  onKeepLastChange: (value: number) => void;
  onNameChange: (value: string) => void;
  onPathsTextChange: (value: string) => void;
  onRecipientPublicKeyHexChange: (value: string) => void;
  onRetentionDaysChange: (value: number) => void;
  onRotationGenerationChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onTargetsTextChange: (value: string) => void;
  pathsCount: number;
  pathsText: string;
  pending: boolean;
  policyConfirmed: boolean;
  policyEnabled: boolean;
  recipientPublicKeyHex: string;
  retentionDays: number;
  rotationGeneration: string;
  targetCount: number;
  targetExpressionMessage: string;
  targetExpressionValid: boolean;
  targetsText: string;
};

export function BackupPolicyForm({
  agents,
  includeConfig,
  intervalSecs,
  keepLast,
  name,
  onConfirmedChange,
  onEnabledChange,
  onIncludeConfigChange,
  onIntervalSecsChange,
  onKeepLastChange,
  onNameChange,
  onPathsTextChange,
  onRecipientPublicKeyHexChange,
  onRetentionDaysChange,
  onRotationGenerationChange,
  onSubmit,
  onTargetsTextChange,
  pathsCount,
  pathsText,
  pending,
  policyConfirmed,
  policyEnabled,
  recipientPublicKeyHex,
  retentionDays,
  rotationGeneration,
  targetCount,
  targetExpressionMessage,
  targetExpressionValid,
  targetsText,
}: BackupPolicyFormProps) {
  return (
    <>
      <div className="sectionHeader compact">
        <h2>Backup policy</h2>
        <span>{targetCount} matching VPS{targetCount === 1 ? "" : "s"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Name</span>
          <input aria-label="Backup policy name" onChange={(event) => onNameChange(event.target.value)} value={name} />
        </label>
        <div className="targetSelector">
          <div className="targetSelectorHeader">
            <strong>Targets</strong>
            <span>{targetExpressionMessage}</span>
          </div>
          <SearchExpressionInput
            agents={agents}
            ariaLabel="Backup policy target expression"
            className="targetExpressionBar"
            onChange={onTargetsTextChange}
            placeholder="id:edge-01 || provider:alpha && country:us"
            showMatchCount
            value={targetsText}
            verification={targetExpressionValid ? "valid" : "invalid"}
            verificationMessage={targetExpressionMessage}
          />
        </div>
        <label>
          <span>Selected paths</span>
          <textarea
            aria-label="Backup policy selected paths"
            onChange={(event) => onPathsTextChange(event.target.value)}
            placeholder={BACKUP_PATH_PLACEHOLDER}
            rows={4}
            value={pathsText}
          />
        </label>
        <label>
          <span>Recipient public key</span>
          <input
            aria-label="Backup recipient public key hex"
            onChange={(event) => onRecipientPublicKeyHexChange(event.target.value)}
            placeholder="optional 32-byte hex"
            value={recipientPublicKeyHex}
          />
        </label>
        <div className="dispatchControls">
          <label>
            <span>Interval</span>
            <input
              aria-label="Backup policy interval seconds"
              max={31_536_000}
              min={1}
              onChange={(event) => onIntervalSecsChange(Number(event.target.value))}
              type="number"
              value={intervalSecs}
            />
          </label>
          <label>
            <span>Retain days</span>
            <input
              aria-label="Backup policy retention days"
              max={3650}
              min={1}
              onChange={(event) => onRetentionDaysChange(Number(event.target.value))}
              type="number"
              value={retentionDays}
            />
          </label>
          <label>
            <span>Keep last</span>
            <input
              aria-label="Backup policy keep last"
              max={1000}
              min={1}
              onChange={(event) => onKeepLastChange(Number(event.target.value))}
              type="number"
              value={keepLast}
            />
          </label>
        </div>
        <label>
          <span>Rotation generation</span>
          <input
            aria-label="Backup policy rotation generation"
            onChange={(event) => onRotationGenerationChange(event.target.value)}
            placeholder="keyring/v2"
            value={rotationGeneration}
          />
        </label>
        <label className="checkLine inlineCheck">
          <input checked={includeConfig} onChange={(event) => onIncludeConfigChange(event.target.checked)} type="checkbox" />
          <span>Include agent config</span>
        </label>
        <label className="checkLine inlineCheck">
          <input checked={policyEnabled} onChange={(event) => onEnabledChange(event.target.checked)} type="checkbox" />
          <span>Enabled</span>
        </label>
        <label className="checkLine">
          <input checked={policyConfirmed} onChange={(event) => onConfirmedChange(event.target.checked)} type="checkbox" />
          <span>Confirmed</span>
        </label>
        <div className="backupScopeList">
          <CalendarClock size={18} />
          <span>{includeConfig ? "config" : "no config"}</span>
          <span>{pathsCount} path{pathsCount === 1 ? "" : "s"}</span>
        </div>
        <button className="primaryAction" disabled={pending || !name.trim() || targetCount === 0 || !targetExpressionValid} type="submit">
          <Save size={17} />
          Save policy
        </button>
      </form>
    </>
  );
}
