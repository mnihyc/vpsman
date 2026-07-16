import { CheckCircle2, CircleDashed, TriangleAlert } from "lucide-react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { MAX_CONFIGURABLE_JOB_TIMEOUT_SECS } from "../../jobMaxTimeout";
import {
  migrationLinkStatusLabel,
  restorePlanStatusBadgeClass,
  restorePlanStatusLabel,
} from "../../jobStatusPresentation";
import type {
  AgentView,
  MigrationLinkRecord,
  RestorePlanRecord,
} from "../../types";
import { shortId } from "../../utils";
import { TargetImpactPreview } from "../TargetImpactPreview";
import {
  RestoreArchiveTransferSelect,
  type RestoreArchiveTransferOption,
} from "./RestoreArchiveTransferSelect";

type MigrationLinkFormProps = {
  archiveEmptyMessage: string;
  archiveTransferKey: string;
  archiveTransferOptions: RestoreArchiveTransferOption[];
  forceUnprivileged: boolean;
  existingLink: MigrationLinkRecord | null;
  linkConfirmationOpen: boolean;
  migrationNote: string;
  migrationRestorePlanId: string;
  onArchiveTransferChange: (value: string) => void;
  onDownloadPackage?: () => void;
  onDryRunChange: (value: boolean) => void;
  onForceUnprivilegedChange: (value: boolean) => void;
  onMigrationNoteChange: (value: string) => void;
  onMigrationRestorePlanIdChange: (value: string) => void;
  onOpenRestore: () => void;
  onOpenTransfers?: () => void;
  onPostRestoreArgvChange: (value: string) => void;
  onRestoreMaxTimeoutSecsChange: (value: number) => void;
  onRunMigrationRestore: () => void | Promise<void>;
  onSubmit: () => void | Promise<void>;
  pending: boolean;
  clientLabel: (clientId: string) => string;
  postRestoreArgv: string;
  privilegeReady: boolean;
  restoreDryRun: boolean;
  restoreMaxTimeoutSecs: number;
  restorePlans: RestorePlanRecord[];
  runConfirmationOpen: boolean;
  sameVpsRestoreDraftCount: number;
  selectedPlan: RestorePlanRecord | null;
  targetAgent: AgentView | null;
};

export function MigrationLinkForm({
  archiveEmptyMessage,
  archiveTransferKey,
  archiveTransferOptions,
  forceUnprivileged,
  existingLink,
  linkConfirmationOpen,
  migrationNote,
  migrationRestorePlanId,
  onArchiveTransferChange,
  onDownloadPackage,
  onDryRunChange,
  onForceUnprivilegedChange,
  onMigrationNoteChange,
  onMigrationRestorePlanIdChange,
  onOpenRestore,
  onOpenTransfers,
  onPostRestoreArgvChange,
  onRestoreMaxTimeoutSecsChange,
  onRunMigrationRestore,
  onSubmit,
  pending,
  clientLabel,
  postRestoreArgv,
  privilegeReady,
  restoreDryRun,
  restoreMaxTimeoutSecs,
  restorePlans,
  runConfirmationOpen,
  sameVpsRestoreDraftCount,
  selectedPlan,
  targetAgent,
}: MigrationLinkFormProps) {
  const routeValid = Boolean(
    selectedPlan &&
      selectedPlan.source_client_id !== selectedPlan.target_client_id,
  );
  const archiveReady = Boolean(
    archiveTransferOptions.some((option) => option.key === archiveTransferKey),
  );
  const checklist = [
    {
      label: "Source -> replacement",
      detail: selectedPlan
        ? routeValid
          ? `${clientLabel(selectedPlan.source_client_id)} to ${clientLabel(selectedPlan.target_client_id)}`
          : "Source and replacement are the same VPS. Use Restore for same-VPS recovery."
        : "Select a draft restore that defines both VPSs",
      ready: routeValid,
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
          Source VPS/artifact to replacement VPS, with optional mapping notes
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
            disabled={restorePlans.length === 0}
            onChange={(event) =>
              onMigrationRestorePlanIdChange(event.target.value)
            }
            title={
              selectedPlan
                ? `${clientLabel(selectedPlan.source_client_id)} to ${clientLabel(selectedPlan.target_client_id)} (${restorePlanStatusLabel(selectedPlan.status)})`
                : "Select a draft restore with a replacement VPS"
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
        {restorePlans.length === 0 && (
          <div className="restoreArtifactWarning" role="status">
            <strong>Replacement VPS required</strong>
            <span>
              Create a draft restore whose destination differs from its source,
              then return here.
              {sameVpsRestoreDraftCount > 0
                ? ` ${sameVpsRestoreDraftCount} same-VPS draft${sameVpsRestoreDraftCount === 1 ? " remains" : "s remain"} available on Restore.`
                : ""}
            </span>
            <button
              className="secondaryAction compactAction"
              onClick={onOpenRestore}
              type="button"
            >
              Plan restore
            </button>
          </div>
        )}
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
                {existingLink
                  ? `${shortId(existingLink.id)} · ${migrationLinkStatusLabel(existingLink.status)}`
                  : "none"}
              </strong>
            </div>
          </div>
        ) : null}
        <label>
          <span>Mapping notes</span>
          <input
            aria-label="Migration mapping notes"
            onChange={(event) => onMigrationNoteChange(event.target.value)}
            placeholder="rebuilt VPS cutover"
            readOnly={Boolean(existingLink)}
            title={
              existingLink
                ? "Saved mapping notes are immutable for this draft restore"
                : "Optional notes saved with this migration mapping"
            }
            value={existingLink?.note ?? migrationNote}
          />
        </label>
        {existingLink ? (
          <ActionFeedback
            message={`Mapping ${shortId(existingLink.id)} is saved. Cutover reuses this exact mapping.`}
            tone="success"
          />
        ) : !linkConfirmationOpen ? (
          <button
            className="primaryAction"
            disabled={pending || !migrationRestorePlanId || !routeValid}
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
        ) : null}
        <div className="sectionHeader compact restoreFormHeader">
          <h3>Cutover restore</h3>
          <span>Stage the package, rehearse, then explicitly select live mode</span>
        </div>
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
        <label>
          <span>Post-restore check argv</span>
          <input
            aria-label="Migration post-restore argv"
            onChange={(event) => onPostRestoreArgvChange(event.target.value)}
            placeholder="/usr/local/sbin/post-restore-check --json"
            title="Command and arguments to run after restore, separated by spaces"
            value={postRestoreArgv}
          />
        </label>
        <label>
          <span>Max timeout seconds</span>
          <input
            aria-label="Migration restore max timeout seconds"
            max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
            min={1}
            onChange={(event) =>
              onRestoreMaxTimeoutSecsChange(Number(event.target.value))
            }
            type="number"
            value={restoreMaxTimeoutSecs}
          />
        </label>
        <label className="checkLine">
          <input
            aria-label="Migration dry-run rehearsal"
            checked={restoreDryRun}
            onChange={(event) => onDryRunChange(event.target.checked)}
            type="checkbox"
          />
          <span>Dry-run rehearsal</span>
        </label>
        <TargetImpactPreview
          forceUnprivileged={forceUnprivileged}
          mode="restore"
          targets={targetAgent ? [targetAgent] : []}
          title="Cutover target impact"
        />
        <label className="checkLine">
          <input
            aria-label="Force unprivileged migration restore best effort"
            checked={forceUnprivileged}
            onChange={(event) =>
              onForceUnprivilegedChange(event.target.checked)
            }
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
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
                <small title={item.detail}>{item.detail}</small>
              </span>
            </div>
          ))}
        </div>
        {!runConfirmationOpen && (
          <button
            className={
              restoreDryRun ? "primaryAction" : "primaryAction dangerPrimary"
            }
            disabled={
              pending || !migrationRestorePlanId || !routeValid || !archiveReady
            }
            onClick={() => void onRunMigrationRestore()}
            title={
              privilegeReady
                ? "Review the frozen cutover restore"
                : "Opens privilege unlock before preparing the cutover review"
            }
            type="button"
          >
            {restoreDryRun ? "Review dry run" : "Review live cutover"}
          </button>
        )}
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
