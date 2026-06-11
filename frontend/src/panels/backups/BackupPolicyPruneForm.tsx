import type { FormEvent } from "react";
import { Scissors, SearchCheck } from "lucide-react";
import type { BackupPolicyPruneResponse, BackupPolicyRecord } from "../../types";
import { shortId } from "../../utils";

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
          matched: acc.matched + policy.matched_rows,
          pruned: acc.pruned + policy.pruned_rows,
          objects: acc.objects + policy.object_keys.length,
        }),
        { matched: 0, objects: 0, pruned: 0 },
      )
    : null;

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
        {result && totals && (
          <div className="backupScopeList">
            <span>{result.dry_run ? "dry run" : "applied"}</span>
            <span>{totals.matched} matched</span>
            <span>{totals.pruned} pruned</span>
            <span>{totals.objects} object{totals.objects === 1 ? "" : "s"}</span>
          </div>
        )}
        {!confirmationOpen && (
          <button className={dryRun ? "secondaryAction" : "dangerAction"} disabled={pending} type="submit">
            {dryRun ? <SearchCheck size={17} /> : <Scissors size={17} />}
            {dryRun ? "Review prune" : "Review prune"}
          </button>
        )}
      </form>
    </>
  );
}
