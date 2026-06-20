import { CheckCircle2, CircleDashed, TriangleAlert } from "lucide-react";
import { restorePlanStatusBadgeClass } from "../../jobStatusPresentation";
import type { MigrationLinkRecord, RestorePlanRecord } from "../../types";
import { shortId } from "../../utils";
import {
  RestoreArchiveTransferSelect,
  type RestoreArchiveTransferOption,
} from "./RestoreArchiveTransferSelect";

type MigrationLinkFormProps = {
  archiveEmptyMessage: string;
  archiveTransferKey: string;
  archiveTransferOptions: RestoreArchiveTransferOption[];
  forceUnprivileged: boolean;
  lastMigrationLink: MigrationLinkRecord | null;
  linkConfirmationOpen: boolean;
  migrationNote: string;
  migrationRestorePlanId: string;
  onArchiveTransferChange: (value: string) => void;
  onMigrationNoteChange: (value: string) => void;
  onMigrationRestorePlanIdChange: (value: string) => void;
  onRunMigrationRestore: () => void | Promise<void>;
  onSubmit: () => void | Promise<void>;
  pending: boolean;
  clientLabel: (clientId: string) => string;
  postRestoreArgv: string;
  privilegeReady: boolean;
  restoreDryRun: boolean;
  restorePlans: RestorePlanRecord[];
  runConfirmationOpen: boolean;
  selectedPlan: RestorePlanRecord | null;
};

export function MigrationLinkForm({
  archiveEmptyMessage,
  archiveTransferKey,
  archiveTransferOptions,
  forceUnprivileged,
  lastMigrationLink,
  linkConfirmationOpen,
  migrationNote,
  migrationRestorePlanId,
  onArchiveTransferChange,
  onMigrationNoteChange,
  onMigrationRestorePlanIdChange,
  onRunMigrationRestore,
  onSubmit,
  pending,
  clientLabel,
  postRestoreArgv,
  privilegeReady,
  restoreDryRun,
  restorePlans,
  runConfirmationOpen,
  selectedPlan,
}: MigrationLinkFormProps) {
  const archiveReady = Boolean(
    archiveTransferOptions.some((option) => option.key === archiveTransferKey),
  );
  const checklist = [
    {
      label: "Restore plan",
      detail: selectedPlan
        ? `${clientLabel(selectedPlan.source_client_id)} to ${clientLabel(selectedPlan.target_client_id)}`
        : "Select an approved restore plan",
      ready: Boolean(selectedPlan),
      required: true,
    },
    {
      label: "Archive metadata",
      detail: archiveReady
        ? "Completed upload transfer selected"
        : "Required for migration restore; not needed for link-only",
      ready: archiveReady,
      required: false,
    },
    {
      label: "Privilege",
      detail: privilegeReady ? "Ready" : "Unlock privilege before running the restore",
      ready: privilegeReady,
      required: true,
    },
    {
      label: "Rehearsal mode",
      detail: restoreDryRun ? "Dry run enabled" : "Live restore selected",
      ready: restoreDryRun,
      required: false,
    },
    {
      label: "Post-restore",
      detail: postRestoreArgv.trim() || "No post-restore command configured",
      ready: Boolean(postRestoreArgv.trim()),
      required: false,
    },
    {
      label: "Privilege policy",
      detail: forceUnprivileged ? "Forced best-effort/unprivileged restore" : "Use client capability policy",
      ready: !forceUnprivileged,
      required: false,
    },
  ];

  return (
    <section className="backupActionPanel">
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Migration assistant</h2>
        <span>Rebuilt VPS identity map and executable restore run</span>
      </div>
      <div className="formGrid">
        <label>
          Restore plan
          <select
            aria-label="Migration restore plan"
            onChange={(event) => onMigrationRestorePlanIdChange(event.target.value)}
            value={migrationRestorePlanId}
          >
            <option value="">Select restore plan</option>
            {restorePlans.map((plan) => (
              <option key={plan.id} value={plan.id}>
                {clientLabel(plan.source_client_id)} to {clientLabel(plan.target_client_id)} ({plan.status})
              </option>
            ))}
          </select>
        </label>
        {selectedPlan ? (
          <div className="migrationPlanSummary" aria-live="polite">
            <div>
              <span>Plan</span>
              <strong>{shortId(selectedPlan.id)}</strong>
            </div>
            <div>
              <span>Source</span>
              <strong>{clientLabel(selectedPlan.source_client_id)}</strong>
            </div>
            <div>
              <span>Target</span>
              <strong>{clientLabel(selectedPlan.target_client_id)}</strong>
            </div>
            <div>
              <span>Scope</span>
              <strong>{restoreScopeLabel(selectedPlan)}</strong>
            </div>
            <div>
              <span>Status</span>
              <strong className={`status ${restorePlanStatusBadgeClass(selectedPlan.status)}`}>{selectedPlan.status}</strong>
            </div>
            <div>
              <span>Last link</span>
              <strong>{lastMigrationLink ? `${shortId(lastMigrationLink.id)} ${lastMigrationLink.status}` : "none"}</strong>
            </div>
          </div>
        ) : null}
        <RestoreArchiveTransferSelect
          emptyMessage={archiveEmptyMessage}
          label="Migration staged archive"
          onChange={onArchiveTransferChange}
          options={archiveTransferOptions}
          value={archiveTransferKey}
        />
        <div className="migrationChecklist">
          {checklist.map((item) => (
            <div className={`migrationCheckItem ${item.ready ? "ready" : item.required ? "blocked" : "optional"}`} key={item.label}>
              {item.ready ? <CheckCircle2 size={16} /> : item.required ? <TriangleAlert size={16} /> : <CircleDashed size={16} />}
              <span>
                <strong>{item.label}</strong>
                <small>{item.detail}</small>
              </span>
            </div>
          ))}
        </div>
        <label>
          Migration note
          <input
            aria-label="Migration note"
            onChange={(event) => onMigrationNoteChange(event.target.value)}
            placeholder="rebuilt VPS cutover"
            value={migrationNote}
          />
        </label>
      </div>
      <div className="actionRow">
        {!linkConfirmationOpen && (
          <button
            className="primaryAction"
            disabled={pending || !migrationRestorePlanId || !privilegeReady}
            onClick={() => void onSubmit()}
            type="button"
          >
            Review link
          </button>
        )}
        {!runConfirmationOpen && (
          <button
            className="secondaryAction"
            disabled={pending || !migrationRestorePlanId || !privilegeReady || !archiveReady}
            onClick={() => void onRunMigrationRestore()}
            type="button"
          >
            Review migration restore
          </button>
        )}
      </div>
    </section>
  );
}

function restoreScopeLabel(plan: RestorePlanRecord): string {
  const parts = [];
  if (plan.include_config) {
    parts.push("config");
  }
  if (plan.paths.length > 0) {
    parts.push(`${plan.paths.length} path${plan.paths.length === 1 ? "" : "s"}`);
  }
  return parts.join(", ") || "metadata only";
}
