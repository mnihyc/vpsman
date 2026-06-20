import type { FormEvent } from "react";
import { CalendarClock, Save } from "lucide-react";
import { SearchExpressionInput } from "../../components/SearchExpressionInput";
import {
  BACKUP_PATH_PLACEHOLDER,
  BACKUP_PATH_PRESETS,
} from "../../presets/backupPathPresets";
import { PathPresetButtons } from "./PathPresetButtons";
import type { AgentView } from "../../types";

type BackupPolicyFormProps = {
  agents: AgentView[];
  cronExpr: string;
  includeConfig: boolean;
  keepLast: number;
  name: string;
  confirmationOpen: boolean;
  onCronExprChange: (value: string) => void;
  onEnabledChange: (value: boolean) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onKeepLastChange: (value: number) => void;
  onNameChange: (value: string) => void;
  onPathsTextChange: (value: string) => void;
  onRetentionDaysChange: (value: number) => void;
  onRotationGenerationChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onTargetsTextChange: (value: string) => void;
  pathsCount: number;
  pathsText: string;
  pending: boolean;
  policyEnabled: boolean;
  retentionDays: number;
  rotationGeneration: string;
  targetCount: number;
  targetExpressionMessage: string;
  targetExpressionValid: boolean;
  targetsText: string;
};

export function BackupPolicyForm({
  agents,
  cronExpr,
  includeConfig,
  keepLast,
  name,
  confirmationOpen,
  onCronExprChange,
  onEnabledChange,
  onIncludeConfigChange,
  onKeepLastChange,
  onNameChange,
  onPathsTextChange,
  onRetentionDaysChange,
  onRotationGenerationChange,
  onSubmit,
  onTargetsTextChange,
  pathsCount,
  pathsText,
  pending,
  policyEnabled,
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
        <span>
          {targetCount} fixed VPS target{targetCount === 1 ? "" : "s"} after confirmation
        </span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Name</span>
          <input
            aria-label="Backup policy name"
            onChange={(event) => onNameChange(event.target.value)}
            value={name}
          />
        </label>
        <div className="targetSelector">
          <div className="targetSelectorHeader">
            <strong>Audit selector</strong>
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
          <small className="formHint">
            The confirmation saves the resolved VPS list as fixed targets; the selector remains for audit and future manual target updates.
          </small>
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
          <PathPresetButtons
            onApply={onPathsTextChange}
            presets={BACKUP_PATH_PRESETS}
          />
        </label>
        <div className="dispatchControls">
          <label>
            <span>UTC cron</span>
            <input
              aria-label="Backup policy UTC cron expression"
              onChange={(event) => onCronExprChange(event.target.value)}
              placeholder="0 3 * * *"
              value={cronExpr}
            />
          </label>
          <label>
            <span>Retain days</span>
            <input
              aria-label="Backup policy retention days"
              max={3650}
              min={1}
              onChange={(event) =>
                onRetentionDaysChange(Number(event.target.value))
              }
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
          <input
            checked={includeConfig}
            onChange={(event) => onIncludeConfigChange(event.target.checked)}
            type="checkbox"
          />
          <span>Include agent config</span>
        </label>
        <label className="checkLine inlineCheck">
          <input
            checked={policyEnabled}
            onChange={(event) => onEnabledChange(event.target.checked)}
            type="checkbox"
          />
          <span>Enabled</span>
        </label>
        <div className="backupScopeList">
          <CalendarClock size={18} />
          <span>{cronExpr.trim() || "cron required"}</span>
          <span>{includeConfig ? "config" : "no config"}</span>
          <span>
            {pathsCount} path{pathsCount === 1 ? "" : "s"}
          </span>
        </div>
        {!confirmationOpen && (
          <button
            className="primaryAction"
            disabled={
              pending ||
              !name.trim() ||
              !cronExpr.trim() ||
              !targetsText.trim() ||
              !targetExpressionValid
            }
            type="submit"
          >
            <Save size={17} />
            Review policy
          </button>
        )}
      </form>
    </>
  );
}
