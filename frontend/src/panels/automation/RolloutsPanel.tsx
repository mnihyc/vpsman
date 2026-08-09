import { useEffect, useMemo, useRef, useState } from "react";
import {
  ExternalLink,
  Pause,
  Play,
  RefreshCw,
  ShieldCheck,
  XCircle,
} from "lucide-react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { ConsoleDetailPanel } from "../../components/ConsoleDetailPanel";
import { jobTargetStatusBadgeClass } from "../../jobStatusPresentation";
import { formatLowerBoundCount } from "../../constants";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  beginSubmission,
  createSubmissionGuard,
  finishSubmission,
} from "../../submissionGuard";
import type {
  AgentView,
  CancelJobResponse,
  JobHistoryRecord,
  JobRolloutRecord,
  JobRolloutTargetRecord,
  UpdateJobRolloutRequest,
} from "../../types";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatCompactTime,
  formatFullTime,
  shortId,
} from "../../utils";
import { pushHistoryEntry } from "../../historyEntryState";
import { scrollIntoViewWithMotion } from "../../motion";

type FeedbackState = {
  jobId?: string;
  message: string;
  tone: ActionFeedbackTone;
};

type ReviewAction = {
  kind: "abort" | "resume";
  rollout: JobRolloutRecord;
};

export function RolloutsPanel({
  agents,
  jobs,
  rollouts: initialRollouts,
  rolloutsTruncated,
  onCancelJob,
  onLoadRollouts,
  onOpenJobDetails,
  onUpdateRollout,
}: {
  agents: AgentView[];
  jobs: JobHistoryRecord[];
  rollouts: JobRolloutRecord[];
  rolloutsTruncated: boolean;
  onCancelJob: (jobId: string, reason: string) => Promise<CancelJobResponse>;
  onLoadRollouts: () => Promise<JobRolloutRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onUpdateRollout: (
    jobId: string,
    action: "pause" | "resume",
    request: UpdateJobRolloutRequest,
  ) => Promise<JobRolloutRecord>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [rollouts, setRollouts] = useState(initialRollouts);
  const [loading, setLoading] = useState(false);
  const loadingRef = useRef(false);
  const [pendingJobId, setPendingJobId] = useState<string | null>(null);
  const pendingJobIdRef = useRef<string | null>(null);
  const submissionGuardRef = useRef(createSubmissionGuard());
  const [feedback, setFeedback] = useState<FeedbackState | null>(null);
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  const [selectedJobId, setSelectedJobId] = useState(readRolloutJobRoute);
  const [reviewAction, setReviewAction] = useState<ReviewAction | null>(null);

  useEffect(() => setRollouts(initialRollouts), [initialRollouts]);

  useEffect(() => {
    void reload();
  }, []);

  useEffect(() => {
    const applyRoute = () => setSelectedJobId(readRolloutJobRoute());
    window.addEventListener("popstate", applyRoute);
    window.addEventListener("hashchange", applyRoute);
    return () => {
      window.removeEventListener("popstate", applyRoute);
      window.removeEventListener("hashchange", applyRoute);
    };
  }, []);

  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const jobById = useMemo(
    () => new Map(jobs.map((job) => [job.id, job])),
    [jobs],
  );
  const selected = selectedJobId
    ? (rollouts.find((rollout) => rollout.job_id === selectedJobId) ?? null)
    : null;

  useEffect(() => {
    if (!feedback || (feedback.jobId && feedback.jobId === selected?.job_id)) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      if (feedbackRef.current) {
        scrollIntoViewWithMotion(feedbackRef.current, { block: "nearest" });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [feedback, selected?.job_id]);

  async function reload() {
    if (loadingRef.current) return;
    loadingRef.current = true;
    setLoading(true);
    try {
      const next = await onLoadRollouts();
      setRollouts(next);
      setFeedback(null);
    } catch (error) {
      setFeedback({
        message:
          error instanceof Error
            ? error.message
            : "Rollout evidence is unavailable",
        tone: "danger",
      });
    } finally {
      loadingRef.current = false;
      setLoading(false);
    }
  }

  function openRollout(rollout: JobRolloutRecord) {
    setRolloutJobRoute(rollout.job_id);
    setSelectedJobId(rollout.job_id);
  }

  function closeRollout() {
    setRolloutJobRoute(null);
    setSelectedJobId(null);
    setReviewAction(null);
  }

  function replaceRollout(next: JobRolloutRecord) {
    setRollouts((current) => [
      next,
      ...current.filter((rollout) => rollout.job_id !== next.job_id),
    ]);
  }

  async function pauseRollout(rollout: JobRolloutRecord) {
    if (pendingJobIdRef.current || rollout.status !== "running") return;
    const submissionKey = `rollout-pause:${rollout.job_id}:${rollout.updated_at}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    pendingJobIdRef.current = rollout.job_id;
    setPendingJobId(rollout.job_id);
    setFeedback({
      jobId: rollout.job_id,
      message: `Pausing rollout ${shortId(rollout.job_id)} after already-dispatched work settles...`,
      tone: "progress",
    });
    try {
      const next = await onUpdateRollout(rollout.job_id, "pause", {
        confirmed: false,
        reason: "operator_requested",
      });
      replaceRollout(next);
      setFeedback({
        jobId: rollout.job_id,
        message: `Rollout ${shortId(rollout.job_id)} is paused. No new queued VPS will be released.`,
        tone: "success",
      });
      successful = true;
    } catch (error) {
      setFeedback({
        jobId: rollout.job_id,
        message:
          error instanceof Error ? error.message : "Rollout pause failed",
        tone: "danger",
      });
    } finally {
      pendingJobIdRef.current = null;
      finishSubmission(submissionGuardRef.current, submissionKey, successful);
      setPendingJobId(null);
    }
  }

  async function confirmReviewedAction() {
    const review = reviewAction;
    if (!review || pendingJobIdRef.current) return;
    const submissionKey = `rollout-${review.kind}:${review.rollout.job_id}:${review.rollout.updated_at}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    pendingJobIdRef.current = review.rollout.job_id;
    setPendingJobId(review.rollout.job_id);
    setFeedback({
      jobId: review.rollout.job_id,
      message:
        review.kind === "resume"
          ? `Resuming rollout ${shortId(review.rollout.job_id)} after the reviewed stage...`
          : `Aborting rollout ${shortId(review.rollout.job_id)} and canceling unreleased targets...`,
      tone: "progress",
    });
    try {
      if (review.kind === "resume") {
        const next = await onUpdateRollout(review.rollout.job_id, "resume", {
          confirmed: true,
          reason: "operator_reviewed_stage",
        });
        replaceRollout(next);
        successful = true;
        setFeedback({
          jobId: next.job_id,
          message: `Rollout ${shortId(next.job_id)} resumed. ${currentStageTargetCount(next)} VPS are eligible in the current stage.`,
          tone: "success",
        });
      } else {
        const result = await onCancelJob(
          review.rollout.job_id,
          "operator_aborted_rollout",
        );
        successful = true;
        setRollouts(await onLoadRollouts());
        const activeCancelCount = result.cancel_acks.length;
        setFeedback({
          jobId: result.job_id,
          message: `Rollout ${shortId(result.job_id)} aborted. ${result.pending_canceled} queued target${result.pending_canceled === 1 ? " was" : "s were"} canceled; ${activeCancelCount} active target${activeCancelCount === 1 ? "" : "s"} received cancellation requests.`,
          tone: result.cancel_acks.every((ack) => ack.applied)
            ? "success"
            : "warning",
        });
      }
      setReviewAction(null);
    } catch (error) {
      setFeedback({
        jobId: review.rollout.job_id,
        message:
          error instanceof Error ? error.message : "Rollout action failed",
        tone: "danger",
      });
    } finally {
      pendingJobIdRef.current = null;
      finishSubmission(submissionGuardRef.current, submissionKey, successful);
      setPendingJobId(null);
    }
  }

  function reviewRolloutAction(
    kind: ReviewAction["kind"],
    rollout: JobRolloutRecord,
  ) {
    setFeedback(null);
    setReviewAction({ kind, rollout });
  }

  const actions = useMemo<ConsoleDataGridAction<JobRolloutRecord>[]>(
    () => [
      {
        description: (rows) =>
          `Open batch and per-VPS evidence for rollout ${shortId(rows[0]?.job_id ?? "")}.`,
        icon: <ShieldCheck size={14} />,
        label: "Review rollout",
        onSelect: (rows) => rows[0] && openRollout(rows[0]),
      },
      {
        description: () =>
          "Stop releasing queued VPSs. Work already dispatched is allowed to settle.",
        disabled: (rows) =>
          Boolean(pendingJobId) || rows[0]?.status !== "running",
        icon: <Pause size={14} />,
        label: "Pause",
        onSelect: (rows) => rows[0] && void pauseRollout(rows[0]),
      },
      {
        description: () =>
          "Review and release the current stage after an operator or safety pause.",
        disabled: (rows) =>
          Boolean(pendingJobId) || rows[0]?.status !== "paused",
        icon: <Play size={14} />,
        label: "Resume",
        onSelect: (rows) => rows[0] && reviewRolloutAction("resume", rows[0]),
      },
      {
        description: () =>
          "Open the underlying job and complete output evidence.",
        icon: <ExternalLink size={14} />,
        label: "Open job",
        onSelect: (rows) => rows[0] && onOpenJobDetails(rows[0].job_id),
      },
      {
        tone: "danger",
        description: () =>
          "Cancel unreleased targets and request cancellation for active work.",
        disabled: (rows) =>
          Boolean(pendingJobId) || isTerminalRollout(rows[0]?.status),
        icon: <XCircle size={14} />,
        label: "Abort rollout",
        onSelect: (rows) => rows[0] && reviewRolloutAction("abort", rows[0]),
      },
    ],
    [pendingJobId],
  );

  const columns = useMemo<ConsoleDataGridColumn<JobRolloutRecord>[]>(
    () => [
      {
        cell: (rollout) => {
          const job = jobById.get(rollout.job_id);
          return (
            <span className="historyPrimary">
              <strong title={job?.command_type ?? "Unknown operation"}>
                {job ? readableToken(job.command_type) : "Unknown operation"}
              </strong>
              <small className="monoValue" title={rollout.job_id}>
                {shortId(rollout.job_id)}
              </small>
            </span>
          );
        },
        header: "Operation",
        id: "operation",
        minSize: 150,
        searchValue: (rollout) =>
          `${rollout.job_id} ${jobById.get(rollout.job_id)?.command_type ?? ""}`,
        size: 210,
        sortValue: (rollout) =>
          jobById.get(rollout.job_id)?.command_type ?? rollout.job_id,
      },
      {
        cell: (rollout) => {
          const progress = rolloutProgress(rollout);
          return (
            <span className="historyPrimary rolloutProgressCell">
              <strong>
                {progress.terminal} / {progress.total}
              </strong>
              <progress
                aria-label={`${progress.terminal} of ${progress.total} VPS complete`}
                max={Math.max(1, progress.total)}
                value={progress.terminal}
              />
            </span>
          );
        },
        header: "Progress",
        id: "progress",
        minSize: 120,
        searchValue: (rollout) => rolloutProgress(rollout).terminal,
        size: 150,
        sortValue: (rollout) => rolloutProgress(rollout).terminal,
      },
      {
        cell: (rollout) => (
          <span className="historyPrimary">
            <strong>
              {rollout.current_batch + 1} / {rollout.total_batches}
            </strong>
            <small>{currentStageTargetCount(rollout)} VPS in stage</small>
          </span>
        ),
        header: "Stage",
        id: "stage",
        minSize: 100,
        searchValue: (rollout) => rollout.current_batch,
        size: 126,
        sortValue: (rollout) => rollout.current_batch,
      },
      {
        cell: (rollout) => (
          <span className="historyPrimary">
            <strong>{rolloutFailureCount(rollout)}</strong>
            <small>Limit {rollout.max_failures}</small>
          </span>
        ),
        header: "Failures",
        id: "failures",
        minSize: 92,
        searchValue: rolloutFailureCount,
        size: 110,
        sortValue: rolloutFailureCount,
      },
      {
        cell: (rollout) => (
          <span className="historyPrimary">
            <span
              className={`status ${rolloutStatusClass(rollout.status)}`}
              title={rolloutStateDetail(rollout)}
            >
              {readableToken(rollout.status)}
            </span>
            <small title={rolloutStateDetail(rollout)}>
              {rolloutStateDetail(rollout)}
            </small>
          </span>
        ),
        header: "State",
        id: "state",
        minSize: 140,
        searchValue: (rollout) =>
          `${rollout.status} ${rollout.pause_reason ?? ""}`,
        size: 170,
        sortValue: (rollout) => rollout.status,
      },
      {
        cell: (rollout) => (
          <span title={formatFullTime(rollout.updated_at)}>
            {formatCompactTime(rollout.updated_at)}
          </span>
        ),
        header: "Updated",
        id: "updated",
        minSize: 110,
        searchValue: (rollout) => rollout.updated_at,
        size: 132,
        sortValue: (rollout) => rollout.updated_at,
      },
    ],
    [jobById],
  );

  const activeCount = rollouts.filter(
    (rollout) => rollout.status === "running",
  ).length;
  const pausedCount = rollouts.filter(
    (rollout) => rollout.status === "paused",
  ).length;
  const failurePausedCount = rollouts.filter(
    (rollout) => rollout.pause_reason === "failure_threshold",
  ).length;
  const completedCount = rollouts.filter(
    (rollout) => rollout.status === "completed",
  ).length;
  const abortedCount = rollouts.filter(
    (rollout) => rollout.status === "aborted",
  ).length;

  return (
    <div className="jobConsoleStack rolloutWorkspace">
      <section className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Staged rollouts</h2>
            <span>
              Durable canaries, bounded batches, safety pauses, and per-VPS
              evidence
            </span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                loading
                  ? "Rollout records are already loading"
                  : pendingJobId
                    ? "A rollout control action is already in progress"
                    : undefined
              }
              disabled={loading || Boolean(pendingJobId)}
              onClick={() => void reload()}
              title="Reload durable rollout state"
              type="button"
            >
              <RefreshCw size={14} />
              <span>{loading ? "Loading" : "Reload"}</span>
            </button>
            <ActionFeedback
              className="localActionFeedback"
              message={
                !feedback?.jobId || feedback.jobId !== selected?.job_id
                  ? feedback?.message
                  : null
              }
              ref={feedbackRef}
              tone={feedback?.tone}
            />
          </div>
        </div>

        <div
          aria-label="Rollout summary"
          className="processSupervisorSummaryStrip"
        >
          <span>
            <strong>
              {formatLowerBoundCount(activeCount, rolloutsTruncated)}
            </strong>
            <small>
              {rolloutsTruncated ? "Running in loaded page" : "Running"}
            </small>
          </span>
          <span className={pausedCount > 0 ? "attention" : undefined}>
            <strong>
              {formatLowerBoundCount(pausedCount, rolloutsTruncated)}
            </strong>
            <small>
              {rolloutsTruncated ? "Paused in loaded page" : "Paused"}
            </small>
          </span>
          <span className={failurePausedCount > 0 ? "attention" : undefined}>
            <strong>
              {formatLowerBoundCount(failurePausedCount, rolloutsTruncated)}
            </strong>
            <small>
              {rolloutsTruncated
                ? "Safety review in loaded page"
                : "Safety review"}
            </small>
          </span>
          <span>
            <strong>
              {formatLowerBoundCount(completedCount, rolloutsTruncated)}
            </strong>
            <small>
              {rolloutsTruncated ? "Completed in loaded page" : "Completed"}
            </small>
          </span>
          <span className={abortedCount > 0 ? "attention" : undefined}>
            <strong>
              {formatLowerBoundCount(abortedCount, rolloutsTruncated)}
            </strong>
            <small>
              {rolloutsTruncated ? "Aborted in loaded page" : "Aborted"}
            </small>
          </span>
        </div>

        <ConsoleDataGrid
          columns={columns}
          defaultPageSize={25}
          empty={
            <div className="emptyState compactEmpty">
              <ShieldCheck size={22} />
              <strong>
                {loading ? "Loading rollouts" : "No staged rollouts"}
              </strong>
              <span>
                Enable staged delivery while reviewing a multi-VPS dispatch.
              </span>
            </div>
          }
          expandOnRowClick
          getRowId={(rollout) => rollout.job_id}
          itemLabel="rollouts"
          renderExpandedRow={(rollout) => {
            const progress = rolloutProgress(rollout);
            return (
              <div className="consoleInlineDetailGrid">
                <span>Job</span>
                <strong className="monoValue" title={rollout.job_id}>
                  {rollout.job_id}
                </strong>
                <span>Stage</span>
                <strong>
                  {rollout.current_batch + 1} of {rollout.total_batches}
                </strong>
                <span>Completed targets</span>
                <strong>
                  {progress.terminal} of {progress.total}
                </strong>
                <span>Failure threshold</span>
                <strong>
                  {rolloutFailureCount(rollout)} observed /{" "}
                  {rollout.max_failures} tolerated
                </strong>
                <span>Canaries</span>
                <strong title={rollout.canary_client_ids.join(", ")}>
                  {rollout.canary_client_ids
                    .map((id) => clientLabel(id, agentNameById))
                    .join(", ")}
                </strong>
              </div>
            );
          }}
          rowActions={actions}
          rows={rollouts}
          rowsTruncated={rolloutsTruncated}
          searchPlaceholder="Search job, operation, state, or pause reason"
          showMobileRowActions={false}
          singleExpandedRow
          storageKey="vpsman.automation.rollouts"
          title="Rollout history"
        />
      </section>

      {selected && (
        <RolloutDetail
          agentNameById={agentNameById}
          feedback={
            !reviewAction && feedback?.jobId === selected.job_id
              ? feedback
              : null
          }
          job={jobById.get(selected.job_id) ?? null}
          onAbort={() => reviewRolloutAction("abort", selected)}
          onClose={closeRollout}
          onOpenJob={() => onOpenJobDetails(selected.job_id)}
          onPause={() => void pauseRollout(selected)}
          onResume={() => reviewRolloutAction("resume", selected)}
          pending={pendingJobId === selected.job_id}
          rollout={selected}
        />
      )}

      <ConfirmationPrompt
        confirmLabel={
          reviewAction?.kind === "abort" ? "Abort rollout" : "Resume stage"
        }
        detail={
          reviewAction
            ? reviewDetail(reviewAction)
            : "Review the rollout action."
        }
        error={
          feedback?.tone === "danger" &&
          (!reviewAction || feedback.jobId === reviewAction.rollout.job_id)
            ? feedback.message
            : null
        }
        items={reviewAction ? reviewItems(reviewAction) : []}
        onCancel={() => setReviewAction(null)}
        onConfirm={() => void confirmReviewedAction()}
        open={Boolean(reviewAction)}
        pending={Boolean(
          reviewAction && pendingJobId === reviewAction.rollout.job_id,
        )}
        title={
          reviewAction?.kind === "abort"
            ? "Confirm rollout abort"
            : "Confirm stage release"
        }
        tone={reviewAction?.kind === "abort" ? "danger" : "warning"}
      >
        {reviewAction &&
        feedback?.jobId === reviewAction.rollout.job_id &&
        feedback.tone !== "danger" ? (
          <ActionFeedback
            className="rolloutReviewFeedback"
            message={feedback.message}
            tone={feedback.tone}
          />
        ) : null}
      </ConfirmationPrompt>
    </div>
  );
}

function RolloutDetail({
  agentNameById,
  feedback,
  job,
  onAbort,
  onClose,
  onOpenJob,
  onPause,
  onResume,
  pending,
  rollout,
}: {
  agentNameById: Map<string, string>;
  feedback: FeedbackState | null;
  job: JobHistoryRecord | null;
  onAbort: () => void;
  onClose: () => void;
  onOpenJob: () => void;
  onPause: () => void;
  onResume: () => void;
  pending: boolean;
  rollout: JobRolloutRecord;
}) {
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!feedback) return;
    const frame = window.requestAnimationFrame(() => {
      if (feedbackRef.current) {
        scrollIntoViewWithMotion(feedbackRef.current, { block: "nearest" });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [feedback]);
  const targetColumns = useMemo<
    ConsoleDataGridColumn<JobRolloutTargetRecord>[]
  >(
    () => [
      {
        cell: (target) => (
          <span className="historyPrimary">
            <strong title={clientLabel(target.client_id, agentNameById)}>
              {clientLabel(target.client_id, agentNameById)}
            </strong>
            <small className="monoValue" title={target.client_id}>
              {target.client_id}
            </small>
          </span>
        ),
        header: "VPS",
        id: "vps",
        minSize: 160,
        searchValue: (target) =>
          `${target.client_id} ${clientLabel(target.client_id, agentNameById)}`,
        size: 240,
        sortValue: (target) => clientLabel(target.client_id, agentNameById),
      },
      {
        cell: (target) =>
          target.batch_index === 0 ? "Canary" : `Batch ${target.batch_index}`,
        header: "Stage",
        id: "stage",
        minSize: 90,
        searchValue: (target) => target.batch_index,
        size: 112,
        sortValue: (target) => target.batch_index,
      },
      {
        cell: (target) => (
          <span
            className={`status ${jobTargetStatusBadgeClass(target.status)}`}
            title={`Rollout target status: ${readableToken(target.status)}`}
          >
            {readableToken(target.status)}
          </span>
        ),
        header: "State",
        id: "state",
        minSize: 110,
        searchValue: (target) => `${target.status} ${target.message ?? ""}`,
        size: 132,
        sortValue: (target) => target.status,
      },
      {
        cell: (target) => (
          <span
            className="truncateValue"
            data-tooltip-empty-reason={
              target.message
                ? undefined
                : "This rollout target has no result message yet"
            }
          >
            {target.message ?? "-"}
          </span>
        ),
        header: "Latest result",
        id: "result",
        minSize: 180,
        searchValue: (target) => target.message ?? "",
        size: 320,
        sortValue: (target) => target.message ?? "",
      },
    ],
    [agentNameById],
  );
  const progress = rolloutProgress(rollout);
  return (
    <ConsoleDetailPanel
      actions={
        <>
          <button
            className="secondaryAction compactAction"
            onClick={onOpenJob}
            type="button"
          >
            <ExternalLink size={14} />
            <span>Open job</span>
          </button>
          {rollout.status === "running" && (
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                pending
                  ? "A rollout control action is already in progress"
                  : undefined
              }
              disabled={pending}
              onClick={onPause}
              type="button"
            >
              <Pause size={14} />
              <span>Pause</span>
            </button>
          )}
          {rollout.status === "paused" && (
            <button
              className="primaryAction compactAction"
              data-tooltip-disabled-reason={
                pending
                  ? "A rollout control action is already in progress"
                  : undefined
              }
              disabled={pending}
              onClick={onResume}
              type="button"
            >
              <Play size={14} />
              <span>Resume stage</span>
            </button>
          )}
          {!isTerminalRollout(rollout.status) && (
            <button
              className="dangerAction compactAction"
              data-tooltip-disabled-reason={
                pending
                  ? "A rollout control action is already in progress"
                  : undefined
              }
              disabled={pending}
              onClick={onAbort}
              type="button"
            >
              <XCircle size={14} />
              <span>Abort rollout</span>
            </button>
          )}
        </>
      }
      description={`${job ? readableToken(job.command_type) : "Unknown operation"} · ${progress.total} VPS · ${rollout.total_batches} stages`}
      onClose={onClose}
      title={`Rollout ${shortId(rollout.job_id)}`}
    >
      <ActionFeedback
        className="localActionFeedback rolloutActionFeedback"
        message={feedback?.message}
        ref={feedbackRef}
        tone={feedback?.tone}
      />
      <div
        aria-label="Selected rollout summary"
        className="processSupervisorSummaryStrip"
      >
        <span>
          <strong>{readableToken(rollout.status)}</strong>
          <small>{rolloutStateDetail(rollout)}</small>
        </span>
        <span>
          <strong>
            {progress.terminal} / {progress.total}
          </strong>
          <small>Targets complete</small>
        </span>
        <span>
          <strong>
            {rollout.current_batch + 1} / {rollout.total_batches}
          </strong>
          <small>Current stage</small>
        </span>
        <span
          className={
            rolloutFailureCount(rollout) > rollout.max_failures
              ? "attention"
              : undefined
          }
        >
          <strong>
            {rolloutFailureCount(rollout)} / {rollout.max_failures}
          </strong>
          <small>Failures / limit</small>
        </span>
        <span>
          <strong>{rollout.batch_delay_secs}s</strong>
          <small>Inter-stage delay</small>
        </span>
      </div>
      <ConsoleDataGrid
        columns={targetColumns}
        defaultPageSize={25}
        empty={
          <div className="emptyState compactEmpty">
            <strong>No target evidence</strong>
          </div>
        }
        getRowId={(target) => target.client_id}
        itemLabel="targets"
        rows={rollout.targets}
        searchPlaceholder="Search VPS or result"
        selectable={false}
        storageKey="vpsman.automation.rolloutTargets"
        title="Batch and target evidence"
      />
    </ConsoleDetailPanel>
  );
}

function rolloutProgress(rollout: JobRolloutRecord) {
  const terminal = rollout.targets.filter(
    (target) => !["queued", "dispatching", "running"].includes(target.status),
  ).length;
  return { terminal, total: rollout.targets.length };
}

function rolloutFailureCount(rollout: JobRolloutRecord) {
  return rollout.targets.filter(
    (target) =>
      !["queued", "dispatching", "running", "completed"].includes(
        target.status,
      ),
  ).length;
}

function currentStageTargetCount(rollout: JobRolloutRecord) {
  return rollout.targets.filter(
    (target) => target.batch_index === rollout.current_batch,
  ).length;
}

function rolloutStatusClass(status: JobRolloutRecord["status"]) {
  if (status === "completed") return "ok";
  if (status === "running") return "info";
  if (status === "paused" || status === "aborted") return "warn";
  return "neutral";
}

function rolloutStateDetail(rollout: JobRolloutRecord) {
  if (rollout.pause_reason === "canary_review") return "Canary review required";
  if (rollout.pause_reason === "failure_threshold")
    return "Failure threshold exceeded";
  if (rollout.pause_reason === "operator_requested")
    return "Paused by operator";
  if (rollout.pause_reason === "operator_aborted_rollout")
    return "Aborted by operator";
  if (rollout.pause_reason) return readableToken(rollout.pause_reason);
  if (rollout.status === "running") return "Releasing reviewed stage";
  if (rollout.status === "completed") return "All stages settled";
  if (rollout.status === "aborted")
    return "No further targets will be released";
  return "Awaiting operator action";
}

function reviewDetail(review: ReviewAction) {
  return review.kind === "abort"
    ? "Unreleased targets will be canceled immediately. Cancellation will be requested for work already running on an agent."
    : "Release the current reviewed stage. The rollout will pause again if its failure threshold is exceeded.";
}

function reviewItems(review: ReviewAction) {
  const rollout = review.rollout;
  return [
    { label: "Job", title: rollout.job_id, value: shortId(rollout.job_id) },
    {
      label: "Current stage",
      value: `${rollout.current_batch + 1} of ${rollout.total_batches} · ${currentStageTargetCount(rollout)} VPS`,
    },
    {
      label: "Progress",
      value: `${rolloutProgress(rollout).terminal} of ${rollout.targets.length} targets complete`,
    },
    {
      label: "Failures",
      value: `${rolloutFailureCount(rollout)} observed · ${rollout.max_failures} tolerated after last review`,
    },
  ];
}

function isTerminalRollout(status: JobRolloutRecord["status"] | undefined) {
  return status === "completed" || status === "aborted";
}

function readableToken(value: string) {
  return value
    .split("_")
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

function clientLabel(clientId: string, names: Map<string, string>) {
  return clientDisplayNameFromMap(clientId, names);
}

function readRolloutJobRoute(): string | null {
  if (typeof window === "undefined") return null;
  return (
    new URLSearchParams(window.location.search).get("rollout_job")?.trim() ||
    null
  );
}

function setRolloutJobRoute(jobId: string | null) {
  if (typeof window === "undefined") return;
  const url = new URL(window.location.href);
  const current = url.searchParams.get("rollout_job")?.trim() || null;
  if (current === jobId) return;
  if (jobId) url.searchParams.set("rollout_job", jobId);
  else url.searchParams.delete("rollout_job");
  pushHistoryEntry(`${url.pathname}${url.search}${url.hash}`);
}
