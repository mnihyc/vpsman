import { CheckCircle2, CircleDashed, TriangleAlert } from "lucide-react";
import {
  migrationLinkStatusLabel,
  restorePlanStatusBadgeClass,
  restorePlanStatusLabel,
} from "../../jobStatusPresentation";
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
  onDownloadPackage?: () => void;
  onMigrationNoteChange: (value: string) => void;
  onMigrationRestorePlanIdChange: (value: string) => void;
  onOpenTransfers?: () => void;
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
  onDownloadPackage,
  onMigrationNoteChange,
  onMigrationRestorePlanIdChange,
  onOpenTransfers,
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
      label: "Source -> replacement",
      detail: selectedPlan
        ? `${clientLabel(selectedPlan.source_client_id)} to ${clientLabel(selectedPlan.target_client_id)}`
        : "Select a draft restore that defines both VPSs",
      ready: Boolean(selectedPlan),
      required: true,
    },
    {
      label: "Source artifact",
      detail: archiveReady
        ? "Completed upload transfer selected on replacement"
        : "Stage the source package on the replacement before cutover restore",
      ready: archiveReady,
      required: true,
    },
    {
      label: "Privilege",
      detail: privilegeReady
        ? "Ready"
        : "Unlock privilege before running the restore",
      ready: privilegeReady,
      required: true,
    },
    {
      label: "Cutover mode",
      detail: restoreDryRun ? "Dry run enabled" : "Live restore selected",
      ready: restoreDryRun,
      required: false,
    },
    {
      label: "Service check",
      detail: postRestoreArgv.trim() || "No post-restore command configured",
      ready: Boolean(postRestoreArgv.trim()),
      required: false,
    },
    {
      label: "Identity policy",
      detail: forceUnprivileged
        ? "Forced best-effort/unprivileged restore"
        : "Use client capability policy",
      ready: !forceUnprivileged,
      required: false,
    },
  ];

  return (
    <section className="backupActionPanel">
      <div className="sectionHeader compact restoreFormHeader">
        <h2>Migration mapping</h2>
        <span>
          Source VPS/artifact to replacement VPS, with optional cutover notes
        </span>
      </div>
      <form
        className="dispatchForm migrationLinkForm"
        onSubmit={(event) => event.preventDefault()}
      >
        <label>
          <span>Draft restore relationship</span>
          <select
            aria-label="Migration draft restore"
            onChange={(event) =>
              onMigrationRestorePlanIdChange(event.target.value)
            }
            value={migrationRestorePlanId}
          >
            <option value="">Select draft restore</option>
            {restorePlans.map((plan) => (
              <option key={plan.id} value={plan.id}>
                {clientLabel(plan.source_client_id)} to{" "}
                {clientLabel(plan.target_client_id)} (
                {restorePlanStatusLabel(plan.status)})
              </option>
            ))}
          </select>
        </label>
        {selectedPlan ? (
          <div className="migrationPlanSummary" aria-live="polite">
            <div>
              <span>Draft restore</span>
              <strong>{shortId(selectedPlan.id)}</strong>
            </div>
            <div>
              <span>Source VPS</span>
              <strong>{clientLabel(selectedPlan.source_client_id)}</strong>
            </div>
            <div>
              <span>Replacement VPS</span>
              <strong>{clientLabel(selectedPlan.target_client_id)}</strong>
            </div>
            <div>
              <span>Path behavior</span>
              <strong>{restoreScopeLabel(selectedPlan)}</strong>
            </div>
            <div>
              <span>Restore state</span>
              <strong
                className={`status ${restorePlanStatusBadgeClass(selectedPlan.status)}`}
              >
                {restorePlanStatusLabel(selectedPlan.status)}
              </strong>
            </div>
            <div>
              <span>Last mapping</span>
              <strong>
                {lastMigrationLink
                  ? `${shortId(lastMigrationLink.id)} · ${migrationLinkStatusLabel(lastMigrationLink.status)}`
                  : "none"}
              </strong>
            </div>
          </div>
        ) : null}
        <RestoreArchiveTransferSelect
          emptyMessage={archiveEmptyMessage}
          label="Migration staged archive"
          onChange={onArchiveTransferChange}
          onDownloadPackage={onDownloadPackage}
          onOpenTransfers={onOpenTransfers}
          options={archiveTransferOptions}
          pending={pending}
          value={archiveTransferKey}
        />
        <div className="migrationChecklist" aria-label="Cutover readiness">
          <div className="migrationChecklistHeader">
            <strong>Cutover readiness</strong>
            <span>
              {checklist.filter((item) => item.required && item.ready).length}/
              {checklist.filter((item) => item.required).length} required ready
            </span>
          </div>
          {checklist.map((item) => (
            <div
              className={`migrationCheckItem ${item.ready ? "ready" : item.required ? "blocked" : "optional"}`}
              key={item.label}
            >
              {item.ready ? (
                <CheckCircle2 size={16} />
              ) : item.required ? (
                <TriangleAlert size={16} />
              ) : (
                <CircleDashed size={16} />
              )}
              <span>
                <strong>{item.label}</strong>
                <small>{item.detail}</small>
              </span>
            </div>
          ))}
        </div>
        <label>
          <span>Cutover notes</span>
          <input
            aria-label="Migration cutover notes"
            onChange={(event) => onMigrationNoteChange(event.target.value)}
            placeholder="rebuilt VPS cutover"
            value={migrationNote}
          />
        </label>
        <div className="actionRow">
          {!linkConfirmationOpen && (
            <button
              className="primaryAction"
              disabled={pending || !migrationRestorePlanId}
              onClick={() => void onSubmit()}
              title={
                privilegeReady
                  ? "Review the frozen migration mapping"
                  : "Opens privilege unlock before preparing the mapping review"
              }
              type="button"
            >
              Review mapping
            </button>
          )}
          {!runConfirmationOpen && (
            <button
              className="secondaryAction"
              disabled={pending || !migrationRestorePlanId || !archiveReady}
              onClick={() => void onRunMigrationRestore()}
              title={
                privilegeReady
                  ? "Review the frozen cutover restore"
                  : "Opens privilege unlock before preparing the cutover review"
              }
              type="button"
            >
              Review cutover restore
            </button>
          )}
        </div>
      </form>
    </section>
  );
}

function restoreScopeLabel(plan: RestorePlanRecord): string {
  const parts = [];
  if (plan.include_config) {
    parts.push("config");
  }
  if (plan.paths.length > 0) {
    parts.push(
      `${plan.paths.length} path${plan.paths.length === 1 ? "" : "s"}`,
    );
  }
  return parts.join(", ") || "metadata only";
}
