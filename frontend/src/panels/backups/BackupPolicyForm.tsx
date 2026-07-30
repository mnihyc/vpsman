import type { FormEvent } from "react";
import { CalendarClock, Save } from "lucide-react";
import { SearchExpressionInput } from "../../components/SearchExpressionInput";
import {
  BACKUP_PATH_PLACEHOLDER,
  BACKUP_PATH_PRESETS,
  presetPathsText,
} from "../../presets/backupPathPresets";
import { PathPresetButtons } from "./PathPresetButtons";
import type { AgentView, BackupMissingPathPolicy } from "../../types";
import { LocalTargetPreview } from "../TargetImpactPreview";

type BackupPolicyFormProps = {
  agents: AgentView[];
  cronExpr: string;
  followSymlinks: boolean;
  includeConfig: boolean;
  missingPathPolicy: BackupMissingPathPolicy;
  keepLast: number;
  mode: "create" | "edit";
  name: string;
  confirmationOpen: boolean;
  onCronExprChange: (value: string) => void;
  onEnabledChange: (value: boolean) => void;
  onFollowSymlinksChange: (value: boolean) => void;
  onIncludeConfigChange: (value: boolean) => void;
  onMissingPathPolicyChange: (value: BackupMissingPathPolicy) => void;
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
  targetAgents: AgentView[];
  targetExpressionMessage: string;
  targetExpressionValid: boolean;
  targetsText: string;
};

export function BackupPolicyForm({
  agents,
  cronExpr,
  followSymlinks,
  includeConfig,
  missingPathPolicy,
  keepLast,
  mode,
  name,
  confirmationOpen,
  onCronExprChange,
  onEnabledChange,
  onFollowSymlinksChange,
  onIncludeConfigChange,
  onMissingPathPolicyChange,
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
  targetAgents,
  targetExpressionMessage,
  targetExpressionValid,
  targetsText,
}: BackupPolicyFormProps) {
  return (
    <>
      <div className="sectionHeader compact">
        <h2>{mode === "edit" ? "Edit backup policy" : "Backup policy"}</h2>
        <span>
          {targetCount} fixed VPS target{targetCount === 1 ? "" : "s"} after
          confirmation
        </span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Name</span>
          <input
            aria-label="Backup policy name"
            onChange={(event) => onNameChange(event.target.value)}
            placeholder="nightly system backup"
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
            verification={
              targetsText.trim()
                ? targetExpressionValid
                  ? "valid"
                  : "invalid"
                : "neutral"
            }
            verificationMessage={targetExpressionMessage}
          />
          <LocalTargetPreview
            agents={targetAgents}
            ariaLabel="Backup policy local VPS preview"
          />
          <small className="formHint">
            The confirmation saves the resolved VPS list as fixed targets; the
            selector remains for audit and future manual target updates.
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
            onApply={(preset) => {
              onPathsTextChange(presetPathsText(preset.paths));
              onMissingPathPolicyChange(preset.missingPathPolicy);
            }}
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
        <div className="backupOptionStrip" aria-label="Backup policy options">
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
            title="Default is off. Enable only when scheduled backups should archive symlink target contents."
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
            title="Continue only when a selected root does not exist on a target VPS. Unreadable paths and collection errors still fail that target."
          >
            <input
              checked={missingPathPolicy === "skip"}
              onChange={(event) =>
                onMissingPathPolicyChange(
                  event.target.checked ? "skip" : "fail",
                )
              }
              type="checkbox"
            />
            <span>Skip missing roots</span>
          </label>
          <label className="checkLine inlineCheck">
            <input
              checked={policyEnabled}
              onChange={(event) => onEnabledChange(event.target.checked)}
              type="checkbox"
            />
            <span>Enabled</span>
          </label>
        </div>
        <div className="backupScopeList">
          <CalendarClock size={18} />
          <span>{cronExpr.trim() || "cron required"}</span>
          <span>{includeConfig ? "config" : "no config"}</span>
          <span>
            {followSymlinks ? "follows symlinks" : "no symlink follow"}
          </span>
          <span>
            {missingPathPolicy === "skip" ? "optional roots" : "strict roots"}
          </span>
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
              !targetExpressionValid ||
              targetCount === 0
            }
            title={
              !name.trim()
                ? "Enter a policy name"
                : !targetsText.trim()
                  ? "Enter a target selector"
                  : !targetExpressionValid
                    ? targetExpressionMessage
                    : targetCount === 0
                      ? "Choose a selector that matches at least one VPS"
                      : !cronExpr.trim()
                        ? "Enter a UTC cron expression"
                        : "Review the fixed targets and backup policy"
            }
            type="submit"
          >
            <Save size={17} />
            {mode === "edit" ? "Review policy update" : "Review policy"}
          </button>
        )}
      </form>
    </>
  );
}
