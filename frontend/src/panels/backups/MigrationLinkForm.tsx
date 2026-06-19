import { CheckCircle2, CircleDashed, TriangleAlert } from "lucide-react";
import { restorePlanStatusBadgeClass } from "../../jobStatusPresentation";
import type { MigrationLinkRecord, RestorePlanRecord } from "../../types";
import { shortId } from "../../utils";

type MigrationLinkFormProps = {
  linkConfirmationOpen: boolean;
  runConfirmationOpen: boolean;
  migrationNote: string;
  migrationRestorePlanId: string;
  pending: boolean;
  privilegeReady: boolean;
  archivePath: string;
  archiveSizeBytes: string;
  archiveSha256Hex: string;
  clientLabel: (clientId: string) => string;
  forceUnprivileged: boolean;
  lastMigrationLink: MigrationLinkRecord | null;
  postRestoreArgv: string;
  restoreDryRun: boolean;
  restorePlans: RestorePlanRecord[];
  selectedPlan: RestorePlanRecord | null;
  onMigrationNoteChange: (value: string) => void;
  onMigrationRestorePlanIdChange: (value: string) => void;
  onRunMigrationRestore: () => void;
  onSubmit: () => void;
};

export function MigrationLinkForm({
  linkConfirmationOpen,
  runConfirmationOpen,
  migrationNote,
  migrationRestorePlanId,
  pending,
  privilegeReady,
  archivePath,
  archiveSizeBytes,
  archiveSha256Hex,
  clientLabel,
  forceUnprivileged,
  lastMigrationLink,
  postRestoreArgv,
  restoreDryRun,
  restorePlans,
  selectedPlan,
  onMigrationNoteChange,
  onMigrationRestorePlanIdChange,
  onRunMigrationRestore,
  onSubmit,
}: MigrationLinkFormProps) {
  const archiveReady = Boolean(
    archivePath.trim() &&
      archiveSizeBytes.trim() &&
      /^[0-9a-fA-F]{64}$/.test(archiveSha256Hex.trim()),
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
        ? "Agent-local archive path, size, and SHA-256 ready"
        : "Set the agent-local archive path, size, and SHA-256",
      ready: archiveReady,
      required: true,
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
            disabled={pending || !migrationRestorePlanId}
            onClick={onSubmit}
            type="button"
          >
            Review link
          </button>
        )}
        {!runConfirmationOpen && (
          <button
            className="secondaryAction"
            disabled={pending || !migrationRestorePlanId || !privilegeReady || !archiveReady}
            onClick={onRunMigrationRestore}
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
