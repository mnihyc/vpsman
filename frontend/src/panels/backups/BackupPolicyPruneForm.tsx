import type { FormEvent } from "react";
import { Scissors, SearchCheck } from "lucide-react";
import type { BackupPolicyPruneResponse, BackupPolicyRecord } from "../../types";
import { shortHash, shortId } from "../../utils";

type BackupPolicyPruneFormProps = {
  confirmationOpen: boolean;
  dryRun: boolean;
  metadataOnly: boolean;
  onDryRunChange: (value: boolean) => void;
  onMetadataOnlyChange: (value: boolean) => void;
  onScheduleIdChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  pending: boolean;
  policies: BackupPolicyRecord[];
  result: BackupPolicyPruneResponse | null;
  scheduleId: string;
};

export function BackupPolicyPruneForm({
  confirmationOpen,
  dryRun,
  metadataOnly,
  onDryRunChange,
  onMetadataOnlyChange,
  onScheduleIdChange,
  onSubmit,
  pending,
  policies,
  result,
  scheduleId,
}: BackupPolicyPruneFormProps) {
  const totals = result
    ? result.policies.reduce(
        (acc, policy) => ({
          errors: acc.errors + policy.object_delete_errors.length,
          matched: acc.matched + policy.matched_rows,
          pruned: acc.pruned + policy.pruned_rows,
          objects: acc.objects + policy.object_keys.length,
        }),
        { errors: 0, matched: 0, objects: 0, pruned: 0 },
      )
    : null;
  const partialError = result?.policies.some((policy) => policy.status === "partial_error") ?? false;

  return (
    <>
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Policy prune</h2>
        <span>{policies.length} polic{policies.length === 1 ? "y" : "ies"}</span>
      </div>
      <form className="dispatchForm" onSubmit={onSubmit}>
        <label>
          <span>Policy scope</span>
          <select aria-label="Backup policy prune scope" onChange={(event) => onScheduleIdChange(event.target.value)} value={scheduleId}>
            <option value="">All policies</option>
            {policies.map((policy) => (
              <option key={policy.schedule_id} value={policy.schedule_id}>
                {policy.name} ({shortId(policy.schedule_id)})
              </option>
            ))}
          </select>
        </label>
        <label className="checkLine inlineCheck">
          <input checked={dryRun} onChange={(event) => onDryRunChange(event.target.checked)} type="checkbox" />
          <span>Dry run</span>
        </label>
        <label className="checkLine inlineCheck">
          <input checked={metadataOnly} onChange={(event) => onMetadataOnlyChange(event.target.checked)} type="checkbox" />
          <span>Metadata only</span>
        </label>
        <div className="backupPruneReviewState" aria-label="Backup policy prune review state">
          <strong>{dryRun ? "Preview only" : "Preview required before apply"}</strong>
          <span>
            {dryRun
              ? "Runs a dry-run retention preview without deleting metadata or object files."
              : "Submits a fresh dry-run preview first, freezes its preview hash, then opens an apply confirmation."}
          </span>
          {result && totals && (
            <small>
              Last preview {shortHash(result.preview_hash)} reviewed {totals.matched} matched row{totals.matched === 1 ? "" : "s"}
              {result.dry_run ? "" : `; ${totals.pruned} pruned`}.
            </small>
          )}
        </div>
        {result && totals && (
          <div className="backupScopeList">
            <span>{result.dry_run ? "dry run" : partialError ? "partial error" : "applied"}</span>
            <span>{totals.matched} matched</span>
            <span>{totals.pruned} pruned</span>
            <span>{totals.objects} object{totals.objects === 1 ? "" : "s"}</span>
            {totals.errors > 0 && <span>{totals.errors} delete error{totals.errors === 1 ? "" : "s"}</span>}
          </div>
        )}
        {!confirmationOpen && (
          <button className={dryRun ? "secondaryAction" : "dangerAction"} disabled={pending} type="submit">
            {dryRun ? <SearchCheck size={17} /> : <Scissors size={17} />}
            {dryRun ? "Run prune preview" : "Review prune apply"}
          </button>
        )}
      </form>
    </>
  );
}
