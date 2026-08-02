import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import {
  ChevronDown,
  ClipboardList,
  Clock3,
  Pencil,
  Play,
  Plus,
  Power,
  PowerOff,
  RefreshCcw,
  Save,
  ShieldCheck,
  Target,
  Trash2,
} from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConsoleCollapsibleSection } from "../components/ConsoleLayout";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import {
  buildPrivilegeAssertion,
  canonicalSchedulePrivilegeIntent,
  operationPayloadHashHex,
  parseCommandArgv,
  type PrivilegeMaterial,
} from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../searchExpression";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { useReviewGenerationGuard } from "../hooks/useReviewGenerationGuard";
import { formatLowerBoundCount } from "../constants";
import type {
  AgentView,
  BulkResolveResponse,
  CommandTemplateRecord,
  CreateJobResponse,
  CreateScheduleRequest,
  DeferScheduleRequest,
  JobTargetSelection,
  JobOperation,
  SchedulePrivilegeMutationRequest,
  ScheduleRecord,
  UpdateScheduleRequest,
  UpdateScheduleTargetsRequest,
} from "../types";
import {
  formatCompactTime,
  formatTime,
  runPanelAction,
  shortId,
} from "../utils";
import { LocalTargetPreview } from "./TargetImpactPreview";
import { buildScheduleTargetUpdatePrivilegeAssertion } from "../scheduleTargetMaintenance";
import { scrollIntoViewWithMotion } from "../motion";

const SCHEDULE_SELECTOR_STORAGE_KEY = "vpsman.schedules.selectorExpression";

function ScheduleFieldLabel({ help, label }: { help: string; label: string }) {
  return (
    <span className="fieldLabelWithHelp">
      <span>{label}</span>
      <span
        aria-label={`${label} help`}
        className="fieldHelpIcon"
        role="img"
        tabIndex={0}
        title={help}
      >
        ?
      </span>
    </span>
  );
}

export function SchedulesPanel({
  activeSubpage: _activeSubpage,
  agents,
  commandTemplates,
  commandTemplatesTruncated,
  error,
  loading,
  onApplyScheduleNow,
  onCreateSchedule,
  onDeferSchedule,
  onDeleteSchedule,
  onDisableSchedule,
  onEnableSchedule,
  onOpenPrivilegeUnlock,
  onOpenScheduledRuns,
  onRefresh,
  onResolveTargets,
  onUpdateSchedule,
  onUpdateScheduleTargets,
  privilegeMaterial,
  schedules,
  schedulesTruncated,
}: {
  activeSubpage: string;
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  commandTemplatesTruncated: boolean;
  error: string | null;
  loading: boolean;
  onApplyScheduleNow: (
    scheduleId: string,
    request: SchedulePrivilegeMutationRequest,
  ) => Promise<CreateJobResponse>;
  onCreateSchedule: (request: CreateScheduleRequest) => Promise<void>;
  onDeferSchedule: (
    scheduleId: string,
    request: DeferScheduleRequest,
  ) => Promise<void>;
  onDeleteSchedule: (
    scheduleId: string,
    request: SchedulePrivilegeMutationRequest,
  ) => Promise<void>;
  onDisableSchedule: (
    scheduleId: string,
    request: SchedulePrivilegeMutationRequest,
  ) => Promise<void>;
  onEnableSchedule: (
    scheduleId: string,
    request: SchedulePrivilegeMutationRequest,
  ) => Promise<void>;
  onOpenPrivilegeUnlock: () => void;
  onOpenScheduledRuns?: () => void;
  onRefresh: () => Promise<void>;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
  onUpdateSchedule: (
    scheduleId: string,
    request: UpdateScheduleRequest,
  ) => Promise<void>;
  onUpdateScheduleTargets: (
    scheduleId: string,
    request: UpdateScheduleTargetsRequest,
  ) => Promise<void>;
  privilegeMaterial: PrivilegeMaterial | null;
  schedules: ScheduleRecord[];
  schedulesTruncated: boolean;
}) {
  const [name, setName] = useState("");
  const [selectedTemplateId, setSelectedTemplateId] = useState("");
  const [commandText, setCommandText] = useState("");
  const [cronExpr, setCronExpr] = useState("0 * * * *");
  const [enabled, setEnabled] = useState(false);
  const [catchUpPolicy, setCatchUpPolicy] = useState("skip_missed");
  const [catchUpLimit, setCatchUpLimit] = useState(1);
  const [retryDelaySecs, setRetryDelaySecs] = useState(300);
  const [maxFailures, setMaxFailures] = useState(3);
  const [selectorExpression, setSelectorExpression] = useState(() =>
    readLocalString(SCHEDULE_SELECTOR_STORAGE_KEY, ""),
  );
  const [confirmationOpen, setConfirmationOpen] = useState(false);
  const [pendingScheduleSnapshot, setPendingScheduleSnapshot] =
    useState<ScheduleDraftSnapshot | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionSuccess, setActionSuccess] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [editingScheduleId, setEditingScheduleId] = useState<string | null>(
    null,
  );
  const [composerRevealRequest, setComposerRevealRequest] = useState(0);
  const scheduleComposerRef = useRef<HTMLElement | null>(null);
  const scheduleNameRef = useRef<HTMLInputElement | null>(null);
  const scheduleLifecycleFeedbackRef = useRef<HTMLDivElement | null>(null);
  const preserveNextComposerSuccessRef = useRef(false);
  const [scheduleAction, setScheduleAction] = useState<ScheduleAction | null>(
    null,
  );
  const [scheduleActionError, setScheduleActionError] = useState<string | null>(
    null,
  );
  const [scheduleLifecycleFeedback, setScheduleLifecycleFeedback] = useState<{
    message: string;
    tone: ActionFeedbackTone;
  } | null>(null);
  const [deferDraft, setDeferDraft] = useState<{
    schedule: ScheduleRecord;
    deferredUntil: string;
    reason: string;
  } | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();

  useEffect(() => {
    if (composerRevealRequest === 0) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      if (scheduleComposerRef.current) {
        scrollIntoViewWithMotion(scheduleComposerRef.current, {
          block: "start",
        });
        scheduleNameRef.current?.focus({ preventScroll: true });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [composerRevealRequest]);

  useEffect(() => {
    if (!scheduleLifecycleFeedback?.message) return;
    const frame = window.requestAnimationFrame(() => {
      if (scheduleLifecycleFeedbackRef.current) {
        scrollIntoViewWithMotion(scheduleLifecycleFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [scheduleLifecycleFeedback?.message]);

  const argv = useMemo(() => {
    try {
      return parseCommandArgv(commandText);
    } catch {
      return [];
    }
  }, [commandText]);
  const selectedTemplate = useMemo(
    () =>
      commandTemplates.find((template) => template.id === selectedTemplateId) ??
      null,
    [commandTemplates, selectedTemplateId],
  );
  const builtinTemplates = useMemo(
    () => commandTemplates.filter((template) => template.built_in),
    [commandTemplates],
  );
  const userTemplates = useMemo(
    () => commandTemplates.filter((template) => !template.built_in),
    [commandTemplates],
  );
  const scheduleOperation = useMemo<JobOperation | null>(
    () =>
      selectedTemplate?.operation ??
      (argv.length > 0 ? { type: "shell", argv, pty: false } : null),
    [argv, selectedTemplate],
  );
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const selectedTargets = useMemo(
    () =>
      selectorParse.error || !selectorExpression.trim()
        ? []
        : agentsMatchingExpression(agents, selectorExpression),
    [agents, selectorExpression, selectorParse.error],
  );
  const selectedTargetIds = useMemo(
    () => selectedTargets.map((agent) => agent.id),
    [selectedTargets],
  );
  const selectedTargetCount = selectedTargetIds.length;
  const cronShapeValid = useMemo(() => hasCronFieldShape(cronExpr), [cronExpr]);
  const nextRuns = useMemo(() => previewNextCronRuns(cronExpr, 5), [cronExpr]);
  const ready =
    name.trim().length > 0 &&
    scheduleOperation !== null &&
    cronShapeValid &&
    selectorExpression.trim().length > 0 &&
    !selectorParse.error;
  const status = schedulesTruncated
    ? `${formatLowerBoundCount(schedules.length, true)} loaded schedules`
    : countPhrase(schedules.length, "schedule");
  const schedulesPageFeedbackMessage =
    error ?? (loading ? "Loading schedules" : null);
  const schedulesPageFeedbackTone = error ? "danger" : "progress";
  const schedulesActionFeedbackMessage = actionError ?? actionSuccess;
  const schedulesActionFeedbackTone: ActionFeedbackTone = actionError
    ? "danger"
    : "success";
  const confirmationNextRun =
    pendingScheduleSnapshot?.nextRun ?? nextRuns[0] ?? null;

  const confirmationItems = [
    {
      label: "Name",
      value: (pendingScheduleSnapshot?.name ?? name.trim()) || "-",
    },
    {
      label: "Audit selector",
      value:
        (pendingScheduleSnapshot?.selectorExpression ??
          selectorExpression.trim()) ||
        "-",
    },
    {
      label: "Fixed targets",
      value: `${pendingScheduleSnapshot?.targetClientIds.length ?? selectedTargetCount} resolved and saved`,
    },
    {
      label: "Target preview",
      value:
        formatScheduleTargetPreview(
          pendingScheduleSnapshot?.targetClientIds ?? selectedTargetIds,
          agents,
        ) || "-",
    },
    {
      label: "Operation",
      value: pendingScheduleSnapshot
        ? (pendingScheduleSnapshot.selectedTemplateName ??
          operationSummary(pendingScheduleSnapshot.operation))
        : selectedTemplate
          ? selectedTemplate.name
          : operationSummary(scheduleOperation),
    },
    {
      label: "Cron",
      value: `${pendingScheduleSnapshot?.cronExpr ?? cronExpr.trim()} UTC`,
    },
    {
      label: "Next",
      value: confirmationNextRun
        ? formatTime(confirmationNextRun)
        : "Server calculates after save",
    },
    {
      label: "Catch-up",
      value: formatCatchUpPolicy(
        pendingScheduleSnapshot?.catchUpPolicy ?? catchUpPolicy,
      ),
    },
    {
      label: "Retry",
      value: formatInterval(
        pendingScheduleSnapshot?.retryDelaySecs ??
          clampInteger(retryDelaySecs, 1, 86_400),
      ),
    },
    {
      label: "State",
      value:
        (pendingScheduleSnapshot?.enabled ?? enabled) ? "Enabled" : "Disabled",
    },
  ];

  const scheduleColumns = useMemo<ConsoleDataGridColumn<ScheduleRecord>[]>(
    () => [
      {
        id: "name",
        header: "Name",
        size: 170,
        minSize: 130,
        sortValue: (schedule) => schedule.name,
        searchValue: (schedule) => `${schedule.name} ${schedule.id}`,
        cell: (schedule) => (
          <span className="historyPrimary">
            <strong>{schedule.name}</strong>
            <small title={schedule.id}>{shortId(schedule.id)}</small>
          </span>
        ),
      },
      {
        id: "operation",
        header: "Operation",
        size: 150,
        minSize: 120,
        sortValue: (schedule) => schedule.command_type,
        searchValue: (schedule) =>
          `${schedule.command_type} ${operationSummary(schedule.operation)} ${schedule.operation_error ?? ""}`,
        cell: (schedule) => (
          <span className="historyPrimary">
            <strong
              className={
                scheduleOperationInvalid(schedule) ? "status warn" : undefined
              }
            >
              {scheduleOperationInvalid(schedule)
                ? "Invalid saved operation"
                : operationSummary(schedule.operation)}
            </strong>
            <small>
              {scheduleOperationInvalid(schedule)
                ? "Full edit required before execution"
                : scheduleCommandTypeLabel(schedule.command_type)}
            </small>
          </span>
        ),
      },
      {
        id: "targets",
        header: "Targets",
        size: 155,
        minSize: 125,
        sortValue: (schedule) => fixedTargetIds(schedule).length,
        searchValue: (schedule) =>
          `${schedule.selector_expression} ${fixedTargetIds(schedule).join(" ")}`,
        cell: (schedule) => {
          const fixedIds = fixedTargetIds(schedule);
          return (
            <span className="historyPrimary">
              <strong>
                {countPhrase(fixedIds.length, "fixed VPS", "fixed VPSs")}
              </strong>
              <small className="mutedText">
                {schedule.selector_expression.trim()
                  ? "audit selector retained"
                  : "no audit selector"}
              </small>
            </span>
          );
        },
      },
      {
        id: "cron",
        header: "Human cadence",
        size: 165,
        minSize: 140,
        sortValue: (schedule) => schedule.cron_expr,
        searchValue: (schedule) =>
          `${schedule.cron_expr} ${describeCronExpression(schedule.cron_expr)} ${schedule.timezone} ${schedule.cadence_error ?? ""}`,
        cell: (schedule) => {
          const cadenceError = scheduleCadenceErrorDetail(schedule);
          return (
            <span
              className="historyPrimary scheduleCadenceCell"
              title={`${schedule.cron_expr} ${schedule.timezone}`}
            >
              {cadenceError ? (
                <strong className="status warn">Invalid cadence</strong>
              ) : (
                <strong>{describeCronExpression(schedule.cron_expr)}</strong>
              )}
              <small>
                {schedule.cron_expr} · {schedule.timezone}
              </small>
              {cadenceError ? <small>{cadenceError}</small> : null}
            </span>
          );
        },
      },
      {
        id: "nextRun",
        header: "Next run / Overdue",
        size: 170,
        minSize: 145,
        sortValue: (schedule) => schedule.next_run_at,
        searchValue: (schedule) =>
          `${schedule.next_run_at} ${schedule.next_runs.join(" ")} ${schedule.last_run_at ?? ""}`,
        cell: (schedule) => {
          const timing = scheduleRunTiming(schedule);
          return (
            <span className="scheduleRunsCell">
              {timing.futureRuns.length > 0 ? (
                <span className="historyPrimary">
                  <strong title={formatTime(timing.futureRuns[0])}>
                    {formatCompactTime(timing.futureRuns[0])}
                  </strong>
                  {timing.futureRuns.length > 1 ? (
                    <small>
                      {Math.min(5, timing.futureRuns.length)} scheduled runs in
                      details
                    </small>
                  ) : null}
                </span>
              ) : (
                <span className={`status ${timing.tone}`}>{timing.label}</span>
              )}
              <small>{timing.detail}</small>
            </span>
          );
        },
      },
      {
        id: "lastResult",
        header: "Last result",
        size: 130,
        minSize: 115,
        sortValue: (schedule) => schedule.last_run_at ?? "",
        searchValue: (schedule) =>
          `${schedule.last_run_at ?? ""} ${schedule.last_error ?? ""} ${schedule.failure_count}`,
        cell: (schedule) => (
          <span className="historyPrimary">
            <span className={`status ${scheduleLastResultTone(schedule)}`}>
              {scheduleLastResultLabel(schedule)}
            </span>
            <small>
              {schedule.last_run_at
                ? formatCompactTime(schedule.last_run_at)
                : "No execution yet"}
            </small>
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        size: 135,
        minSize: 120,
        sortValue: (schedule) =>
          `${schedule.enabled ? "enabled" : "disabled"} ${schedule.failure_count}`,
        searchValue: (schedule) =>
          `${schedule.enabled ? "enabled" : "disabled"} ${schedule.last_error ?? ""} ${schedule.cadence_error ?? ""} ${schedule.operation_error ?? ""}`,
        cell: (schedule) => {
          const cadenceError = scheduleCadenceErrorDetail(schedule);
          const operationInvalid = scheduleOperationInvalid(schedule);
          return (
            <span className="historyPrimary">
              <span
                className={
                  operationInvalid || cadenceError
                    ? "status warn"
                    : schedule.enabled
                      ? "status ok"
                      : "status neutral"
                }
              >
                {operationInvalid
                  ? "Invalid operation"
                  : cadenceError
                    ? "Invalid cadence"
                    : schedule.enabled
                      ? "Enabled"
                      : "Disabled"}
              </span>
              <small>
                {operationInvalid
                  ? "Run, enable, defer, and retarget are blocked"
                  : cadenceError
                    ? "Edit required; automatic runs blocked"
                    : schedule.enabled
                      ? "Automatic runs authorized"
                      : "Automatic runs paused"}
              </small>
              {schedule.failure_count > 0 && (
                <small>
                  {schedule.failure_count}/{schedule.max_failures} failures
                </small>
              )}
              {schedule.last_error && <small>{schedule.last_error}</small>}
            </span>
          );
        },
      },
    ],
    [agents, commandTemplates],
  );

  async function submitSchedule(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setActionError(null);
    setActionSuccess(null);
    if (!ready) {
      setActionError("Schedule is incomplete");
      return;
    }
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    setPending(true);
    try {
      const snapshot = await buildScheduleDraftSnapshot();
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingScheduleSnapshot(snapshot);
      blurActiveElement();
      window.setTimeout(() => {
        if (isReviewGenerationCurrent(reviewGeneration)) {
          setConfirmationOpen(true);
        }
      }, 140);
    } catch (error) {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setActionError(
          error instanceof Error
            ? error.message
            : "Schedule review failed without diagnostic detail",
        );
      }
    } finally {
      setPending(false);
    }
  }

  async function saveScheduleNow() {
    setActionError(null);
    setActionSuccess(null);
    await runPanelAction(setPending, setActionError, async () => {
      const snapshot = pendingScheduleSnapshot;
      if (!snapshot) {
        throw new Error("Schedule is incomplete");
      }
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required");
      }
      const operationHash = await operationPayloadHashHex(snapshot.operation);
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalSchedulePrivilegeIntent({
          action: snapshot.editingScheduleId
            ? "schedule.update"
            : "schedule.create",
          scheduleId: snapshot.editingScheduleId,
          name: snapshot.name,
          commandType: snapshot.commandType,
          operationPayloadHash: operationHash,
          selectorExpression: snapshot.selectorExpression,
          resolvedTargets: snapshot.targetClientIds,
          cronExpr: snapshot.cronExpr,
          timezone: "UTC",
          enabled: snapshot.enabled,
          catchUpPolicy: snapshot.catchUpPolicy,
          catchUpLimit: snapshot.catchUpLimit,
          retryDelaySecs: snapshot.retryDelaySecs,
          maxFailures: snapshot.maxFailures,
          deferredUntil: null,
          deleted: false,
        }),
        privilegeMaterial,
      });
      const request: CreateScheduleRequest = {
        name: snapshot.name,
        operation: snapshot.operation,
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.targetClientIds,
        cron_expr: snapshot.cronExpr,
        timezone: "UTC",
        enabled: snapshot.enabled,
        catch_up_policy: snapshot.catchUpPolicy,
        catch_up_limit: snapshot.catchUpLimit,
        retry_delay_secs: snapshot.retryDelaySecs,
        max_failures: snapshot.maxFailures,
        confirmed: true,
        privilege_assertion: privilegeAssertion,
      };
      if (snapshot.editingScheduleId) {
        await onUpdateSchedule(snapshot.editingScheduleId, {
          ...request,
          expected_selector_expression:
            snapshot.expectedSelectorExpression ?? "",
          expected_target_client_ids: snapshot.expectedTargetClientIds ?? [],
        });
      } else {
        await onCreateSchedule(request);
      }
      setConfirmationOpen(false);
      setActionSuccess(
        snapshot.editingScheduleId
          ? `${snapshot.name} updated`
          : `${snapshot.name} saved ${snapshot.enabled ? "and enabled; automatic runs authorized" : "as disabled; automatic runs paused"}`,
      );
      preserveNextComposerSuccessRef.current = true;
      resetScheduleComposer();
      setPendingScheduleSnapshot(null);
    });
  }

  function resetScheduleComposer() {
    invalidateReviewGeneration();
    setName("");
    setSelectedTemplateId("");
    setCommandText("");
    setCronExpr("0 * * * *");
    setEnabled(false);
    setCatchUpPolicy("skip_missed");
    setCatchUpLimit(1);
    setRetryDelaySecs(300);
    setMaxFailures(3);
    setSelectorExpression("");
    setEditingScheduleId(null);
    setConfirmationOpen(false);
    setPendingScheduleSnapshot(null);
    setActionError(null);
  }

  async function buildScheduleDraftSnapshot(): Promise<ScheduleDraftSnapshot> {
    if (!ready || !scheduleOperation) {
      throw new Error("Schedule is incomplete");
    }
    const selector = selectorExpression.trim();
    const draft = {
      editingScheduleId,
      name: name.trim(),
      operation: scheduleOperation,
      commandType: commandTypeForApi(scheduleOperation),
      selectorExpression: selector,
      cronExpr: cronExpr.trim(),
      enabled,
      catchUpPolicy,
      catchUpLimit: clampInteger(catchUpLimit, 1, 25),
      retryDelaySecs: clampInteger(retryDelaySecs, 1, 86_400),
      maxFailures: clampInteger(maxFailures, 1, 100),
      nextRun: nextRuns[0] ?? null,
      selectedTemplateName: selectedTemplate?.name ?? null,
    };
    const editingSchedule = editingScheduleId
      ? (schedules.find((schedule) => schedule.id === editingScheduleId) ??
        null)
      : null;
    const selectorChanged =
      editingSchedule !== null &&
      selector !== editingSchedule.selector_expression.trim();
    const targetClientIds =
      editingSchedule && !selectorChanged
        ? fixedTargetIds(editingSchedule)
        : (
            await onResolveTargets({ selector_expression: selector })
          ).targets.map((target) => target.id);
    if (!targetClientIds.length) {
      throw new Error("Schedule confirmation resolved no VPSs");
    }
    return {
      ...draft,
      expectedSelectorExpression: editingSchedule?.selector_expression ?? null,
      expectedTargetClientIds: editingSchedule?.target_client_ids ?? null,
      targetClientIds,
    };
  }

  function editSchedule(schedule: ScheduleRecord) {
    if (pending) return;
    setPendingScheduleSnapshot(null);
    setScheduleLifecycleFeedback(null);
    const matchingTemplate = schedule.operation
      ? commandTemplates.find(
          (template) =>
            JSON.stringify(template.operation) ===
            JSON.stringify(schedule.operation),
        )
      : undefined;
    if (
      schedule.operation &&
      schedule.operation.type !== "shell" &&
      !matchingTemplate
    ) {
      setScheduleLifecycleFeedback({
        message: commandTemplatesTruncated
          ? "This schedule's non-shell template is not in the loaded template page; older templates may exist. Modification stays disabled until that template is loaded."
          : "Non-shell schedules can be modified from their command template",
        tone: commandTemplatesTruncated ? "warning" : "danger",
      });
      return;
    }
    if (!schedule.operation) {
      setScheduleLifecycleFeedback({
        message:
          "The saved operation is invalid. Choose a replacement command or template, then review the full schedule update.",
        tone: "warning",
      });
    }
    setEditingScheduleId(schedule.id);
    setName(schedule.name);
    setSelectedTemplateId(matchingTemplate?.id ?? "");
    setCommandText(
      schedule.operation?.type === "shell"
        ? operationToCommandText(schedule.operation)
        : "",
    );
    setCronExpr(schedule.cron_expr);
    setEnabled(schedule.enabled);
    setCatchUpPolicy(schedule.catch_up_policy);
    setCatchUpLimit(schedule.catch_up_limit);
    setRetryDelaySecs(schedule.retry_delay_secs);
    setMaxFailures(schedule.max_failures);
    setSelectorExpression(schedule.selector_expression);
    setComposerRevealRequest((current) => current + 1);
  }

  function startDefer(schedule: ScheduleRecord) {
    if (pending) return;
    setScheduleLifecycleFeedback(null);
    const nextHour = new Date(Date.now() + 60 * 60 * 1000);
    setDeferDraft({
      schedule,
      deferredUntil: toDatetimeLocal(nextHour),
      reason: "",
    });
  }

  function openScheduleAction(action: ScheduleAction) {
    setScheduleLifecycleFeedback(null);
    setScheduleActionError(null);
    setScheduleAction(action);
  }

  function reviewApplyNow(schedule: ScheduleRecord) {
    if (pending) return;
    setScheduleActionError(null);
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      setScheduleLifecycleFeedback({
        message: "Privilege unlock is required",
        tone: "danger",
      });
      return;
    }
    openScheduleAction({ type: "applyNow", schedule });
  }

  async function runScheduleAction(action: ScheduleAction) {
    if (pending) return;
    setPending(true);
    setScheduleLifecycleFeedback(null);
    setScheduleActionError(null);
    let completedTargetUpdates = 0;
    try {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required");
      }
      if (action.type === "targetUpdate") {
        const reviewedUpdates = await Promise.all(
          action.updates.map(async (update) => ({
            ...update,
            privilegeAssertion:
              await buildScheduleTargetUpdatePrivilegeAssertion({
                privilegeMaterial,
                schedule: update.schedule,
                selectorExpression: update.selectorExpression,
                targetClientIds: update.targetClientIds,
              }),
          })),
        );
        for (const update of reviewedUpdates) {
          await onUpdateScheduleTargets(update.schedule.id, {
            confirmed: true,
            privilege_assertion: update.privilegeAssertion,
          });
          completedTargetUpdates += 1;
        }
        setScheduleAction(null);
        setScheduleLifecycleFeedback({
          message: `Updated fixed targets for ${countPhrase(
            completedTargetUpdates,
            "schedule",
          )}`,
          tone: "success",
        });
        return;
      }
      const nextEnabled =
        action.type === "enable"
          ? true
          : action.type === "disable" || action.type === "delete"
            ? false
            : action.schedule.enabled;
      const deferredUntil =
        action.type === "defer"
          ? action.deferredUntil
          : action.schedule.deferred_until;
      const privilegeAssertion = await buildSchedulePrivilege(
        action.schedule,
        actionName(action),
        nextEnabled,
        deferredUntil,
        action.type === "delete",
        fixedTargetIds(action.schedule),
        action.schedule.selector_expression,
      );
      let successMessage: string;
      if (action.type === "enable") {
        await onEnableSchedule(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
        successMessage = `${action.schedule.name} enabled; automatic runs resumed`;
      } else if (action.type === "disable") {
        await onDisableSchedule(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
        successMessage = `${action.schedule.name} disabled; automatic runs paused`;
      } else if (action.type === "defer") {
        await onDeferSchedule(action.schedule.id, {
          deferred_until: action.deferredUntil,
          reason: action.reason || null,
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
        successMessage = `${action.schedule.name} deferred until ${formatCompactTime(action.deferredUntil)}`;
      } else if (action.type === "delete") {
        await onDeleteSchedule(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
        successMessage = `${action.schedule.name} deleted`;
      } else {
        const response = await onApplyScheduleNow(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
        successMessage = `Manual run ${shortId(response.job_id)} dispatched to ${countPhrase(response.target_count, "VPS")}; track it in Jobs / History`;
      }
      setScheduleAction(null);
      setScheduleLifecycleFeedback({
        message: successMessage,
        tone: "success",
      });
    } catch (error) {
      const detail =
        error instanceof Error ? error.message : "Schedule action failed";
      if (action.type === "targetUpdate" && completedTargetUpdates > 0) {
        setScheduleAction({
          ...action,
          selectedCount: action.updates.length - completedTargetUpdates,
          updates: action.updates.slice(completedTargetUpdates),
        });
        setScheduleActionError(
          `${completedTargetUpdates} of ${action.updates.length} target snapshots updated before the failure: ${detail}`,
        );
      } else {
        setScheduleActionError(detail);
      }
    } finally {
      setPending(false);
    }
  }

  async function reviewScheduleTargetUpdates(selected: ScheduleRecord[]) {
    if (pending) return;
    const candidates = selected.filter(
      (schedule) =>
        !scheduleOperationInvalid(schedule) &&
        scheduleTargetsNeedUpdate(schedule, agents),
    );
    if (candidates.length === 0) {
      setScheduleLifecycleFeedback({
        message:
          "Selected schedules already match their current audit selector resolution",
        tone: "info",
      });
      return;
    }
    setPending(true);
    setScheduleLifecycleFeedback({
      message: `Resolving saved audit selectors for ${countPhrase(
        candidates.length,
        "schedule",
      )}`,
      tone: "progress",
    });
    setScheduleActionError(null);
    try {
      const resolvedBySelector = new Map<string, string[]>();
      const updates: ScheduleTargetUpdate[] = [];
      for (const schedule of candidates) {
        const selectorExpressionForIntent = schedule.selector_expression.trim();
        let targetClientIds = resolvedBySelector.get(
          selectorExpressionForIntent,
        );
        if (!targetClientIds) {
          const resolved = await onResolveTargets({
            selector_expression: selectorExpressionForIntent,
          });
          targetClientIds = resolved.targets.map((target) => target.id);
          resolvedBySelector.set(selectorExpressionForIntent, targetClientIds);
        }
        if (sameStringSet(fixedTargetIds(schedule), targetClientIds)) {
          continue;
        }
        updates.push({
          schedule,
          selectorExpression: selectorExpressionForIntent,
          targetClientIds,
        });
      }
      if (updates.length === 0) {
        throw new Error("Server resolution found no changed target snapshots");
      }
      openScheduleAction({
        type: "targetUpdate",
        selectedCount: selected.length,
        updates,
      });
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Target update review failed";
      setScheduleLifecycleFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function buildSchedulePrivilege(
    schedule: ScheduleRecord,
    action: string,
    nextEnabled: boolean,
    deferredUntil: string | null,
    deleted: boolean,
    targetIds: string[],
    selectorExpressionForIntent: string,
  ) {
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Privilege unlock is required");
    }
    if (!targetIds.length && action !== "schedule.targets.update") {
      throw new Error("Schedule has no fixed VPS targets");
    }
    const operationHash =
      schedule.operation_payload_hash?.trim() ||
      (schedule.operation
        ? await operationPayloadHashHex(schedule.operation)
        : "");
    if (!operationHash) {
      throw new Error("Schedule operation evidence is unavailable");
    }
    return buildPrivilegeAssertion({
      intent: canonicalSchedulePrivilegeIntent({
        action,
        scheduleId: schedule.id,
        name: schedule.name,
        commandType: schedule.command_type,
        operationPayloadHash: operationHash,
        selectorExpression: selectorExpressionForIntent,
        resolvedTargets: targetIds,
        cronExpr: schedule.cron_expr,
        timezone: schedule.timezone,
        enabled: nextEnabled,
        catchUpPolicy: schedule.catch_up_policy,
        catchUpLimit: schedule.catch_up_limit,
        retryDelaySecs: schedule.retry_delay_secs,
        maxFailures: schedule.max_failures,
        deferredUntil,
        deleted,
      }),
      privilegeMaterial,
    });
  }

  function selectCommandTemplate(templateId: string) {
    setSelectedTemplateId(templateId);
    const template = commandTemplates.find(
      (candidate) => candidate.id === templateId,
    );
    if (template && !name.trim()) {
      setName(`${template.name} schedule`);
    }
  }

  function actionDetail(action: ScheduleAction): string {
    if (action.type === "targetUpdate") {
      return `Re-resolves each saved audit selector and replaces only the ${
        action.updates.length === 1
          ? "fixed target snapshot"
          : "fixed target snapshots"
      }. No other schedule setting changes.`;
    }
    if (action.type === "applyNow") {
      return "Dispatches a normal job from the saved fixed target snapshot without changing the next scheduled run.";
    }
    if (action.type === "defer") {
      return `Pauses automatic execution until ${formatCompactTime(action.deferredUntil)}.`;
    }
    return `${actionConfirmLabel(action.type)} ${action.schedule.name}.`;
  }

  function actionConfirmationItems(action: ScheduleAction) {
    if (action.type === "targetUpdate") {
      return [
        {
          label: "Selected schedules",
          value: `${action.selectedCount}`,
        },
        {
          label: "Changed snapshots",
          value: `${action.updates.length}`,
        },
        {
          label: "Only change",
          value: "Saved fixed target IDs",
        },
        {
          label: "Target updates",
          value: (
            <div className="configurationReviewList">
              {action.updates.map((update) => {
                const delta = scheduleTargetDelta(
                  fixedTargetIds(update.schedule),
                  update.targetClientIds,
                );
                return (
                  <span key={update.schedule.id}>
                    <strong>
                      {update.schedule.name}:{" "}
                      {vpsCountLabel(fixedTargetIds(update.schedule).length)} →{" "}
                      {vpsCountLabel(update.targetClientIds.length)}
                    </strong>
                    <small>
                      Added:{" "}
                      {formatScheduleTargetPreview(delta.added, agents) ||
                        "None"}
                    </small>
                    <small>
                      Removed:{" "}
                      {formatScheduleTargetPreview(delta.removed, agents) ||
                        "None"}
                    </small>
                    <small>Selector: {update.selectorExpression}</small>
                  </span>
                );
              })}
            </div>
          ),
        },
      ];
    }
    const items = [
      {
        label: "Schedule",
        title: action.schedule.id,
        value: `${action.schedule.name} (${shortId(action.schedule.id)})`,
      },
      {
        label: "Operation",
        value: scheduleOperationInvalid(action.schedule)
          ? "Invalid saved operation · repair required"
          : `${operationSummary(action.schedule.operation)} · ${scheduleCommandTypeLabel(action.schedule.command_type)}`,
      },
      {
        label: "Fixed targets",
        value: `${vpsCountLabel(fixedTargetIds(action.schedule).length)} saved`,
      },
      {
        label: "Audit selector",
        value: action.schedule.selector_expression || "-",
      },
      {
        label: "State",
        value: scheduleOperationInvalid(action.schedule)
          ? "Invalid operation — execution blocked"
          : action.schedule.cadence_error
            ? "Invalid cadence — automatic runs blocked"
            : action.schedule.enabled
              ? "Enabled"
              : "Disabled",
      },
    ];
    if (action.type === "defer") {
      items.push({ label: "Deferred until", value: action.deferredUntil });
      if (action.reason.trim()) {
        items.push({ label: "Reason", value: action.reason.trim() });
      }
    }
    return items;
  }

  useEffect(() => {
    writeLocalString(SCHEDULE_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  useLayoutEffect(() => {
    invalidateReviewGeneration();
    setConfirmationOpen(false);
    setPendingScheduleSnapshot(null);
    if (preserveNextComposerSuccessRef.current) {
      preserveNextComposerSuccessRef.current = false;
    } else {
      setActionSuccess(null);
    }
  }, [
    catchUpLimit,
    catchUpPolicy,
    commandText,
    cronExpr,
    editingScheduleId,
    enabled,
    invalidateReviewGeneration,
    maxFailures,
    name,
    retryDelaySecs,
    selectedTemplateId,
    selectorExpression,
  ]);

  const scheduleActions: ConsoleDataGridAction<ScheduleRecord>[] = [
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Run",
          "Dispatches one job from the saved fixed target snapshot.",
          " now",
        ),
      label: "Review run now",
      disabled: (rows) =>
        pending ||
        rows.length !== 1 ||
        Boolean(rows[0] && scheduleOperationInvalid(rows[0])),
      icon: <Play size={14} />,
      onSelect: (rows) => rows[0] && reviewApplyNow(rows[0]),
    },
    {
      description: (rows) =>
        describeScheduleAction(rows, "Enable", "Automatic runs will resume."),
      label: "Review enable",
      disabled: (rows) =>
        pending ||
        rows.length !== 1 ||
        rows[0]?.enabled === true ||
        Boolean(rows[0]?.cadence_error) ||
        Boolean(rows[0] && scheduleOperationInvalid(rows[0])),
      icon: <Power size={14} />,
      onSelect: (rows) =>
        rows[0] && openScheduleAction({ type: "enable", schedule: rows[0] }),
    },
    {
      description: (rows) =>
        describeScheduleAction(rows, "Disable", "Automatic runs will stop."),
      label: "Review disable",
      disabled: (rows) =>
        pending || rows.length !== 1 || rows[0]?.enabled === false,
      icon: <PowerOff size={14} />,
      onSelect: (rows) =>
        rows[0] && openScheduleAction({ type: "disable", schedule: rows[0] }),
    },
    {
      description: (rows) =>
        describeScheduleAction(rows, "Edit", "Opens the schedule composer."),
      disabled: (rows) => pending || rows.length !== 1,
      icon: <Pencil size={14} />,
      label: "Edit",
      onSelect: (rows) => rows[0] && editSchedule(rows[0]),
    },
    {
      description: (rows) => describeScheduleTargetUpdate(rows, agents),
      label: "Update targets",
      disabled: (rows) =>
        pending ||
        rows.length === 0 ||
        !rows.some(
          (schedule) =>
            !scheduleOperationInvalid(schedule) &&
            scheduleTargetsNeedUpdate(schedule, agents),
        ),
      icon: <Target size={14} />,
      onSelect: (rows) => void reviewScheduleTargetUpdates(rows),
    },
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Defer",
          "Opens a defer form before confirmation.",
        ),
      label: "Defer",
      disabled: (rows) =>
        pending ||
        rows.length !== 1 ||
        Boolean(rows[0] && scheduleOperationInvalid(rows[0])),
      icon: <Clock3 size={14} />,
      onSelect: (rows) => rows[0] && startDefer(rows[0]),
    },
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Delete",
          "Permanently removes this schedule.",
        ),
      label: "Review deletion",
      disabled: (rows) => pending || rows.length !== 1,
      icon: <Trash2 size={14} />,
      onSelect: (rows) =>
        rows[0] && openScheduleAction({ type: "delete", schedule: rows[0] }),
      tone: "danger",
    },
    {
      label: "Copy schedule IDs",
      onSelect: (rows) =>
        void copyText(rows.map((schedule) => schedule.id).join("\n")),
    },
    {
      label: "Copy fixed target IDs",
      onSelect: (rows) =>
        void copyText(
          rows.flatMap((schedule) => fixedTargetIds(schedule)).join("\n"),
        ),
    },
    {
      label: "Copy audit selectors",
      onSelect: (rows) =>
        void copyText(
          rows.map((schedule) => schedule.selector_expression).join("\n"),
        ),
    },
  ];
  return (
    <div className="workspace singleColumn">
      <section className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Schedules</h2>
            <span>{status}</span>
          </div>
          <div className="headerActionStack">
            <div className="inlineActions">
              <button
                className="secondaryAction compactAction"
                disabled={pending || !onOpenScheduledRuns}
                onClick={onOpenScheduledRuns}
                title="Open worker-created schedule execution history in Jobs / Scheduled runs"
                type="button"
              >
                <ClipboardList size={17} />
                Scheduled runs
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={loading || pending}
                onClick={onRefresh}
                type="button"
              >
                <RefreshCcw size={17} />
                Refresh
              </button>
            </div>
            <ActionFeedback
              message={schedulesPageFeedbackMessage}
              tone={schedulesPageFeedbackTone}
            />
          </div>
        </div>
        <div
          className="scheduleExecutionPolicy"
          aria-label="Schedule execution policy"
        >
          <Clock3 size={16} />
          <span>
            Enabled schedules with a valid cadence automatically dispatch future
            jobs from their saved target snapshot. Use <strong>Run now</strong>{" "}
            for one manual dispatch; approval work is separate in Jobs /
            Approvals.
          </span>
        </div>
        <ActionFeedback
          className="localActionFeedback scheduleLifecycleFeedback"
          message={scheduleLifecycleFeedback?.message}
          ref={scheduleLifecycleFeedbackRef}
          tone={scheduleLifecycleFeedback?.tone}
        />
        <ConsoleDataGrid
          actions={scheduleActions}
          columns={scheduleColumns}
          defaultPageSize={10}
          expandOnRowClick
          empty={
            schedules.length === 0 ? (
              <div className="emptyState compactEmpty">
                <Clock3 size={22} />
                <strong>No schedules yet</strong>
                <span>
                  Create a schedule below to run a command template on a fixed
                  target snapshot.
                </span>
              </div>
            ) : (
              <div className="emptyState compactEmpty">
                No schedules match the current search.
              </div>
            )
          }
          getRowId={(schedule) => schedule.id}
          itemLabel="schedules"
          renderExpandedRow={(schedule) => (
            <ScheduleExpandedDetail agents={agents} schedule={schedule} />
          )}
          rowActions={scheduleActions.slice(0, 7)}
          rows={schedules}
          rowsTruncated={schedulesTruncated}
          showMobileRowActions={false}
          singleExpandedRow
          storageKey="vpsman.grid.schedules"
          title="Schedule records"
          toolbarActions={
            <button
              className="primaryAction compactAction"
              disabled={pending || editingScheduleId !== null}
              onClick={() => setComposerRevealRequest((current) => current + 1)}
              title={
                editingScheduleId
                  ? "Finish or cancel the current schedule edit before starting another schedule."
                  : "Open the existing Create schedule form below."
              }
              type="button"
            >
              <Plus size={14} />
              Create schedule
            </button>
          }
        />
        <div
          className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`}
          aria-label="Schedule lifecycle privilege gate"
        >
          <ShieldCheck size={16} />
          <span>
            {privilegeMaterial
              ? "Privilege unlocked for schedule lifecycle actions"
              : "Unlock privilege to enable apply now, target updates, enable, disable, and delete"}
          </span>
          {!privilegeMaterial && (
            <button
              className="secondaryAction compactAction"
              onClick={onOpenPrivilegeUnlock}
              type="button"
            >
              Unlock privilege
            </button>
          )}
        </div>
        {deferDraft && (
          <form
            className="inlineOpsForm"
            onSubmit={(event) => {
              event.preventDefault();
              openScheduleAction({
                type: "defer",
                schedule: deferDraft.schedule,
                deferredUntil: datetimeLocalToRfc3339(deferDraft.deferredUntil),
                reason: deferDraft.reason,
              });
            }}
          >
            <label>
              <span>Defer until</span>
              <input
                aria-label="Schedule defer until"
                onChange={(event) =>
                  setDeferDraft({
                    ...deferDraft,
                    deferredUntil: event.target.value,
                  })
                }
                type="datetime-local"
                value={deferDraft.deferredUntil}
              />
            </label>
            <label>
              <span>Reason</span>
              <input
                aria-label="Schedule defer reason"
                onChange={(event) =>
                  setDeferDraft({ ...deferDraft, reason: event.target.value })
                }
                value={deferDraft.reason}
              />
            </label>
            <button
              className="primaryAction"
              disabled={pending || !deferDraft.deferredUntil}
              type="submit"
            >
              <Clock3 size={17} />
              Review defer
            </button>
            <button
              className="secondaryAction"
              onClick={() => setDeferDraft(null)}
              type="button"
            >
              Cancel
            </button>
          </form>
        )}
        <ConfirmationPrompt
          confirmLabel={
            scheduleAction
              ? actionConfirmLabel(scheduleAction.type)
              : "Run schedule action"
          }
          detail={scheduleAction ? actionDetail(scheduleAction) : ""}
          error={scheduleActionError}
          items={scheduleAction ? actionConfirmationItems(scheduleAction) : []}
          onCancel={() => {
            setScheduleActionError(null);
            setScheduleAction(null);
          }}
          onConfirm={() => {
            if (scheduleAction) {
              const action = scheduleAction;
              if (action.type === "defer") {
                setDeferDraft(null);
              }
              void runScheduleAction(action);
            }
          }}
          open={scheduleAction !== null}
          pending={pending}
          title={
            scheduleAction
              ? actionTitle(scheduleAction.type)
              : "Confirm schedule action"
          }
          tone={scheduleAction?.type === "delete" ? "danger" : "normal"}
        />
      </section>

      <section ref={scheduleComposerRef}>
        <ConsoleCollapsibleSection
          forceOpenKey={
            editingScheduleId ??
            (composerRevealRequest > 0
              ? `create:${composerRevealRequest}`
              : null)
          }
          defaultOpen={schedules.length === 0}
          storageKey="vpsman.panel.schedules.create"
          summary={
            schedules.length === 0
              ? "Create the first recurring job"
              : `${countPhrase(selectedTargetCount, "matching VPS", "matching VPSs")} in local preview; server resolves before save`
          }
          title={editingScheduleId ? "Modify schedule" : "Create schedule"}
        >
          <form className="dispatchForm" onSubmit={submitSchedule}>
            <label>
              <span>Name</span>
              <input
                aria-label="Schedule name"
                onChange={(event) => setName(event.target.value)}
                ref={scheduleNameRef}
                value={name}
              />
            </label>
            <label>
              <span>Template</span>
              <select
                aria-label="Schedule job template"
                onChange={(event) => selectCommandTemplate(event.target.value)}
                value={selectedTemplateId}
              >
                <option value="">One-off shell argv</option>
                {builtinTemplates.length > 0 && (
                  <optgroup label="Built-in templates">
                    {builtinTemplates.map((template) => (
                      <option key={template.id} value={template.id}>
                        {template.name}
                      </option>
                    ))}
                  </optgroup>
                )}
                {userTemplates.length > 0 && (
                  <>
                    <option disabled value="__user_template_separator">
                      ────────── User-defined templates ──────────
                    </option>
                    <optgroup label="User-defined templates">
                      {userTemplates.map((template) => (
                        <option key={template.id} value={template.id}>
                          {template.name} · {template.command_type}
                        </option>
                      ))}
                    </optgroup>
                  </>
                )}
              </select>
              {commandTemplatesTruncated && (
                <small>
                  {commandTemplates.length} templates loaded; older templates
                  may not appear.
                </small>
              )}
            </label>
            <label>
              <span>Command argv</span>
              <textarea
                aria-label="Schedule job argv"
                disabled={selectedTemplate !== null}
                onChange={(event) => setCommandText(event.target.value)}
                rows={3}
                value={
                  selectedTemplate
                    ? operationSummary(selectedTemplate.operation)
                    : commandText
                }
              />
            </label>
            <div className="dispatchControls">
              <label>
                <ScheduleFieldLabel
                  help="Five-field cron expression evaluated in UTC. For example, 0 2 * * * runs every day at 02:00 UTC."
                  label="UTC cron"
                />
                <input
                  aria-label="Schedule cron expression"
                  onChange={(event) => setCronExpr(event.target.value)}
                  placeholder="0 2 * * *"
                  value={cronExpr}
                />
              </label>
              <label className="checkLine inlineCheck">
                <input
                  checked={enabled}
                  onChange={(event) => setEnabled(event.target.checked)}
                  type="checkbox"
                />
                <span>Enabled</span>
              </label>
            </div>
            <div className="dispatchControls">
              <label>
                <ScheduleFieldLabel
                  help="Controls missed runs after downtime: skip them, dispatch one missed run, or dispatch a bounded backlog."
                  label="Catch-up"
                />
                <select
                  aria-label="Schedule catch-up policy"
                  onChange={(event) => setCatchUpPolicy(event.target.value)}
                  value={catchUpPolicy}
                >
                  <option value="skip_missed">Skip missed</option>
                  <option value="run_once">Run one missed</option>
                  <option value="run_all_limited">Run limited backlog</option>
                </select>
              </label>
              <label>
                <ScheduleFieldLabel
                  help="Maximum missed runs dispatched when Catch-up is Run limited backlog. It is ignored by the other policies."
                  label="Catch-up limit"
                />
                <input
                  aria-label="Schedule catch-up limit"
                  disabled={catchUpPolicy !== "run_all_limited"}
                  min={1}
                  max={25}
                  onChange={(event) =>
                    setCatchUpLimit(Number(event.target.value))
                  }
                  type="number"
                  value={catchUpLimit}
                />
              </label>
            </div>
            <div className="dispatchControls">
              <label>
                <ScheduleFieldLabel
                  help="Delay before the scheduler retries after a failed run."
                  label="Retry delay (seconds)"
                />
                <input
                  aria-label="Schedule retry delay seconds"
                  min={1}
                  max={86_400}
                  onChange={(event) =>
                    setRetryDelaySecs(Number(event.target.value))
                  }
                  type="number"
                  value={retryDelaySecs}
                />
              </label>
              <label>
                <ScheduleFieldLabel
                  help="Consecutive failed runs allowed before the scheduler disables this schedule."
                  label="Max failures"
                />
                <input
                  aria-label="Schedule max failures"
                  min={1}
                  max={100}
                  onChange={(event) =>
                    setMaxFailures(Number(event.target.value))
                  }
                  type="number"
                  value={maxFailures}
                />
              </label>
            </div>
            <div className="targetSelector">
              <div className="targetSelectorHeader">
                <strong>Target selector (required)</strong>
                <span>
                  {selectorExpression.trim()
                    ? `${vpsCountLabel(selectedTargetCount)} in local preview; server resolves before save`
                    : "Enter an explicit selector; schedules never imply the entire fleet"}
                </span>
              </div>
              <SearchExpressionInput
                agents={agents}
                ariaLabel="Schedule target expression"
                className="targetExpressionBar"
                onChange={setSelectorExpression}
                placeholder="id:edge-sfo-01 || provider:hetzner && country:US"
                showMatchCount
                value={selectorExpression}
                verification={
                  selectorParse.error
                    ? "invalid"
                    : selectorExpression.trim()
                      ? "valid"
                      : "neutral"
                }
                verificationMessage={
                  selectorParse.error ??
                  (selectorExpression.trim()
                    ? `${selectedTargetCount}/${agents.length}`
                    : "required")
                }
              />
              <LocalTargetPreview
                agents={selectedTargets}
                ariaLabel="Schedule local VPS preview"
              />
            </div>
            <div className="schedulePreview">
              <strong>Next runs</strong>
              <span>
                {nextRuns.length
                  ? `${describeCronExpression(cronExpr)}. Times shown in browser timezone.`
                  : cronShapeValid
                    ? "No run appears in the short local preview; the server validates this cadence when saved."
                    : "Cron must use five fields; the server validates it when saved."}
              </span>
              <div className="targetChipList">
                {nextRuns.map((run) => (
                  <span
                    className="targetChip"
                    key={run}
                    title={formatTime(run)}
                  >
                    {formatSchedulePreviewTime(run)}
                  </span>
                ))}
              </div>
              <small>
                {selectorExpression.trim()
                  ? `${countPhrase(selectedTargetCount, "matching VPS", "matching VPSs")} in local preview; server resolves before save; `
                  : "No target selector; "}
                {selectedTemplate
                  ? selectedTemplate.name
                  : operationSummary(scheduleOperation)}
              </small>
            </div>
            {!confirmationOpen && (
              <ActionFeedback
                className="localActionFeedback scheduleActionFeedback"
                message={schedulesActionFeedbackMessage}
                tone={schedulesActionFeedbackTone}
              />
            )}
            {!confirmationOpen && (
              <div className="consoleFormActions">
                {editingScheduleId ? (
                  <button
                    className="secondaryAction"
                    disabled={pending}
                    onClick={resetScheduleComposer}
                    type="button"
                  >
                    Cancel edit
                  </button>
                ) : null}
                <button
                  className="primaryAction"
                  disabled={pending || !ready}
                  type="submit"
                >
                  <Save size={17} />
                  {editingScheduleId ? "Review update" : "Review save"}
                </button>
              </div>
            )}
            <ConfirmationPrompt
              confirmLabel={
                pendingScheduleSnapshot?.editingScheduleId
                  ? "Update schedule"
                  : "Save schedule"
              }
              detail={`Recurring ${
                pendingScheduleSnapshot?.selectedTemplateName ??
                operationSummary(
                  pendingScheduleSnapshot?.operation ?? scheduleOperation,
                )
              } on ${vpsCountLabel(
                pendingScheduleSnapshot?.targetClientIds.length ??
                  selectedTargetCount,
              )}. The resolved target list is saved as a fixed snapshot; the selector is retained for audit and the table's manual Update targets action.`}
              error={actionError}
              items={confirmationItems}
              onCancel={() => {
                setActionError(null);
                setConfirmationOpen(false);
                setPendingScheduleSnapshot(null);
              }}
              onConfirm={() => void saveScheduleNow()}
              open={confirmationOpen}
              pending={pending}
              title={
                pendingScheduleSnapshot?.editingScheduleId
                  ? "Confirm schedule update"
                  : "Confirm schedule"
              }
            />
          </form>
        </ConsoleCollapsibleSection>
      </section>
    </div>
  );
}

function commandTypeForApi(operation: JobOperation): string {
  if (operation.type === "shell") {
    return operation.pty ? "shell_pty" : "shell_argv";
  }
  return operation.type;
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
}

type ScheduleAction =
  | { type: "enable"; schedule: ScheduleRecord }
  | { type: "disable"; schedule: ScheduleRecord }
  | { type: "applyNow"; schedule: ScheduleRecord }
  | {
      type: "targetUpdate";
      selectedCount: number;
      updates: ScheduleTargetUpdate[];
    }
  | { type: "delete"; schedule: ScheduleRecord }
  | {
      type: "defer";
      schedule: ScheduleRecord;
      deferredUntil: string;
      reason: string;
    };

type ScheduleTargetUpdate = {
  schedule: ScheduleRecord;
  selectorExpression: string;
  targetClientIds: string[];
};

type ScheduleDraftSnapshot = {
  editingScheduleId: string | null;
  name: string;
  operation: JobOperation;
  commandType: string;
  selectorExpression: string;
  targetClientIds: string[];
  cronExpr: string;
  enabled: boolean;
  catchUpPolicy: string;
  catchUpLimit: number;
  retryDelaySecs: number;
  maxFailures: number;
  nextRun: string | null;
  selectedTemplateName: string | null;
  expectedSelectorExpression: string | null;
  expectedTargetClientIds: string[] | null;
};

function actionName(action: ScheduleAction): string {
  switch (action.type) {
    case "enable":
      return "schedule.enable";
    case "disable":
      return "schedule.disable";
    case "delete":
      return "schedule.delete";
    case "targetUpdate":
      return "schedule.targets.update";
    case "defer":
      return "schedule.defer";
    case "applyNow":
      return "schedule.apply_now";
  }
}

function actionTitle(type: ScheduleAction["type"]): string {
  switch (type) {
    case "enable":
      return "Enable schedule";
    case "disable":
      return "Disable schedule";
    case "defer":
      return "Defer schedule";
    case "applyNow":
      return "Run schedule now";
    case "targetUpdate":
      return "Update schedule targets";
    case "delete":
      return "Delete schedule";
  }
}

function actionConfirmLabel(type: ScheduleAction["type"]): string {
  switch (type) {
    case "enable":
      return "Enable";
    case "disable":
      return "Disable";
    case "defer":
      return "Defer";
    case "applyNow":
      return "Run now";
    case "targetUpdate":
      return "Update targets";
    case "delete":
      return "Delete schedule";
  }
}

function describeScheduleAction(
  rows: ScheduleRecord[],
  verb: string,
  consequence: string,
  suffix = "",
): string {
  const scheduleName = rows[0]?.name ?? "selected schedule";
  return `${verb} schedule ${scheduleName}${suffix}. ${consequence}`;
}

function describeScheduleTargetUpdate(
  rows: ScheduleRecord[],
  agents: AgentView[],
): string {
  if (rows.length === 0) {
    return "Select schedules to update their saved targets.";
  }
  const changed = rows.filter(
    (schedule) =>
      !scheduleOperationInvalid(schedule) &&
      scheduleTargetsNeedUpdate(schedule, agents),
  );
  if (changed.length > 0) {
    return `Update ${changed.length} of ${rows.length} selected ${
      rows.length === 1 ? "schedule" : "schedules"
    }. Each saved audit selector is re-resolved; only fixed target IDs change.`;
  }
  const schedule = rows[0];
  if (scheduleOperationInvalid(schedule)) {
    return rows.length === 1
      ? `Repair the saved operation for ${schedule.name} before updating its targets.`
      : "None of the selected schedules has an eligible changed target snapshot.";
  }
  const resolution = currentScheduleTargetIds(schedule, agents);
  if (resolution === null) {
    return rows.length === 1
      ? `Edit ${schedule.name}; its saved audit selector is missing or invalid.`
      : "None of the selected schedules has a valid changed audit selector resolution.";
  }
  if (resolution.length === 0) {
    return rows.length === 1
      ? `${schedule.name}'s saved audit selector currently matches no VPSs.`
      : "The selected audit selectors have no changed non-empty target snapshots.";
  }
  return rows.length === 1
    ? `${schedule.name}'s saved fixed targets already match its current audit selector resolution.`
    : "All selected schedules already match their current audit selector resolution.";
}

function currentScheduleTargetIds(
  schedule: ScheduleRecord,
  agents: AgentView[],
): string[] | null {
  const selector = schedule.selector_expression.trim();
  if (!selector || parseSearchExpression(selector).error) {
    return null;
  }
  return agentsMatchingExpression(agents, selector).map((agent) => agent.id);
}

function scheduleTargetsNeedUpdate(
  schedule: ScheduleRecord,
  agents: AgentView[],
): boolean {
  const currentTargetIds = currentScheduleTargetIds(schedule, agents);
  return Boolean(
    currentTargetIds &&
    currentTargetIds.length > 0 &&
    !sameStringSet(fixedTargetIds(schedule), currentTargetIds),
  );
}

function fixedTargetIds(schedule: ScheduleRecord): string[] {
  return Array.isArray(schedule.target_client_ids)
    ? schedule.target_client_ids
    : [];
}

function formatScheduleTargetPreview(
  targetIds: string[],
  agents: AgentView[],
): string {
  const previewLimit = 6;
  const labels = targetIds.slice(0, previewLimit).map((targetId) => {
    const agent = agents.find((candidate) => candidate.id === targetId);
    const name = agent?.display_name?.trim();
    return name ? `${name} (${targetId})` : targetId;
  });
  const remaining = targetIds.length - labels.length;
  return `${labels.join(", ")}${remaining > 0 ? `, +${remaining} more` : ""}`;
}

function scheduleTargetDelta(
  currentTargetIds: string[],
  nextTargetIds: string[],
): { added: string[]; removed: string[] } {
  const current = new Set(currentTargetIds);
  const next = new Set(nextTargetIds);
  return {
    added: nextTargetIds.filter((targetId) => !current.has(targetId)),
    removed: currentTargetIds.filter((targetId) => !next.has(targetId)),
  };
}

function sameStringSet(left: string[], right: string[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  const normalizedLeft = [...left].sort();
  const normalizedRight = [...right].sort();
  return normalizedLeft.every(
    (value, index) => value === normalizedRight[index],
  );
}

function operationToCommandText(operation: JobOperation): string {
  if (operation.type === "shell") {
    return operation.argv.join(" ");
  }
  return operationSummary(operation);
}

function toDatetimeLocal(date: Date): string {
  const offsetMs = date.getTimezoneOffset() * 60 * 1000;
  return new Date(date.getTime() - offsetMs).toISOString().slice(0, 16);
}

function datetimeLocalToRfc3339(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toISOString();
}

function formatInterval(seconds: number): string {
  if (seconds % 86_400 === 0) {
    return `${seconds / 86_400}d`;
  }
  if (seconds % 3600 === 0) {
    return `${seconds / 3600}h`;
  }
  if (seconds % 60 === 0) {
    return `${seconds / 60}m`;
  }
  return `${seconds}s`;
}

type ParsedCronExpression = {
  domAny: boolean;
  domValues: Set<number> | null;
  dowAny: boolean;
  dowValues: Set<number> | null;
  hours: Set<number>;
  minutes: Set<number>;
  months: Set<number>;
};

function hasCronFieldShape(expr: string): boolean {
  return expr.trim().split(/\s+/).length === 5;
}

function parseCronExpression(expr: string): ParsedCronExpression | null {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) {
    return null;
  }
  const [minuteExpr, hourExpr, domExpr, monthExpr, dowExpr] = fields;
  const minutes = parseCronField(minuteExpr, 0, 59);
  const hours = parseCronField(hourExpr, 0, 23);
  const months = parseCronField(monthExpr, 1, 12);
  if (!minutes || !hours || !months) {
    return null;
  }
  const domAny = domExpr === "*";
  const dowAny = dowExpr === "*";
  const domValues = domAny ? null : parseCronField(domExpr, 1, 31);
  const dowValues = dowAny ? null : parseCronField(dowExpr, 0, 7);
  if ((!domAny && !domValues) || (!dowAny && !dowValues)) {
    return null;
  }
  return {
    domAny,
    domValues,
    dowAny,
    dowValues,
    hours,
    minutes,
    months,
  };
}

function previewNextCronRuns(expr: string, count: number): string[] {
  const parsed = parseCronExpression(expr);
  if (!parsed) {
    return [];
  }
  const { domAny, domValues, dowAny, dowValues, hours, minutes, months } =
    parsed;
  const result: string[] = [];
  const cursor = new Date();
  cursor.setUTCSeconds(0, 0);
  cursor.setUTCMinutes(cursor.getUTCMinutes() + 1);
  const maxMinuteChecks = 32 * 24 * 60;
  for (
    let checkedMinutes = 0;
    result.length < count && checkedMinutes < maxMinuteChecks;
    checkedMinutes += 1
  ) {
    const month = cursor.getUTCMonth() + 1;
    const minute = cursor.getUTCMinutes();
    const hour = cursor.getUTCHours();
    const dom = cursor.getUTCDate();
    const dow = cursor.getUTCDay();
    const dowMatches =
      dowAny || dowValues?.has(dow) || (dow === 0 && dowValues?.has(7));
    const domMatches = domAny || domValues?.has(dom);
    const dayMatches =
      domAny || dowAny ? domMatches && dowMatches : domMatches || dowMatches;
    if (
      months.has(month) &&
      hours.has(hour) &&
      minutes.has(minute) &&
      dayMatches
    ) {
      result.push(cursor.toISOString());
    }
    cursor.setUTCMinutes(cursor.getUTCMinutes() + 1);
  }
  return result;
}

function parseCronField(
  expr: string,
  min: number,
  max: number,
): Set<number> | null {
  const values = new Set<number>();
  for (const part of expr.split(",")) {
    if (!part) {
      return null;
    }
    const [rangePart, stepPart] = part.split("/");
    const step = stepPart ? Number(stepPart) : 1;
    if (!Number.isInteger(step) || step < 1) {
      return null;
    }
    let start: number;
    let end: number;
    if (rangePart === "*") {
      start = min;
      end = max;
    } else if (rangePart.includes("-")) {
      const [left, right] = rangePart.split("-").map(Number);
      start = left;
      end = right;
    } else {
      start = Number(rangePart);
      end = start;
    }
    if (
      !Number.isInteger(start) ||
      !Number.isInteger(end) ||
      start < min ||
      end > max ||
      start > end
    ) {
      return null;
    }
    for (let value = start; value <= end; value += step) {
      values.add(value);
    }
  }
  return values;
}

function formatCatchUpPolicy(policy: string): string {
  if (policy === "run_all_limited") {
    return "limited backlog";
  }
  if (policy === "run_once") {
    return "one missed";
  }
  return "skip missed";
}

function scheduleCommandTypeLabel(commandType: string): string {
  switch (commandType) {
    case "shell_argv":
      return "Argv command";
    case "scheduled_shell_argv":
      return "Scheduled shell command";
    case "backup":
      return "Backup";
    default:
      return commandType.replace(/_/g, " ");
  }
}

function describeSchedulePolicy(schedule: ScheduleRecord): string {
  const retry = `retry after ${formatInterval(schedule.retry_delay_secs)}`;
  if (schedule.catch_up_policy === "run_all_limited") {
    return `Run up to ${schedule.catch_up_limit} missed runs; ${retry}`;
  }
  if (schedule.catch_up_policy === "run_once") {
    return `Run only one missed run; ${retry}`;
  }
  return `Skip missed runs; ${retry}`;
}

function scheduleLastResultTone(
  schedule: ScheduleRecord,
): "neutral" | "ok" | "warn" {
  if (schedule.last_error || schedule.failure_count > 0) {
    return "warn";
  }
  if (schedule.last_run_at) {
    return "ok";
  }
  return "neutral";
}

function scheduleLastResultLabel(schedule: ScheduleRecord): string {
  if (schedule.last_error || schedule.failure_count > 0) {
    return "Needs review";
  }
  if (schedule.last_run_at) {
    return "Succeeded";
  }
  return "Never run";
}

function ScheduleFutureRunsMenu({ runs }: { runs: string[] }) {
  const boundedRuns = runs.slice(0, 5);
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          className="scheduleNextRunsTrigger"
          title="Show the next scheduled run times"
          type="button"
        >
          <Clock3 size={13} />
          <span>View {boundedRuns.length}</span>
          <ChevronDown size={12} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          className="consoleMenu scheduleRunMenu"
          collisionPadding={12}
          sideOffset={6}
        >
          <DropdownMenu.Label className="consoleMenuLabel">
            Next runs
          </DropdownMenu.Label>
          <div className="scheduleRunMenuList" role="list">
            {boundedRuns.map((run, index) => (
              <div className="scheduleRunMenuItem" key={run} role="listitem">
                <strong>{index === 0 ? "Next" : `#${index + 1}`}</strong>
                <span title={formatTime(run)}>{formatCompactTime(run)}</span>
              </div>
            ))}
          </div>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function ScheduleExpandedDetail({
  agents,
  schedule,
}: {
  agents: AgentView[];
  schedule: ScheduleRecord;
}) {
  const timing = scheduleRunTiming(schedule);
  return (
    <div className="consoleInlineDetailGrid scheduleExpandedDetail">
      <span>
        <strong>Operation</strong>
        <span>
          {scheduleOperationInvalid(schedule)
            ? "Invalid saved operation"
            : operationSummary(schedule.operation)}
        </span>
        <span>
          {scheduleOperationInvalid(schedule)
            ? "Full edit required before execution"
            : scheduleCommandTypeLabel(schedule.command_type)}
        </span>
      </span>
      <span>
        <strong>Targets</strong>
        <span>
          {countPhrase(
            fixedTargetIds(schedule).length,
            "fixed VPS",
            "fixed VPSs",
          )}
        </span>
        <span>
          {formatScheduleTargetPreview(fixedTargetIds(schedule), agents) ||
            "No fixed VPS IDs"}
        </span>
        <span>
          Audit selector: {schedule.selector_expression || "not retained"}
        </span>
      </span>
      <span>
        <strong>Future runs</strong>
        <span>
          {timing.futureRuns.length > 0
            ? timing.futureRuns
                .slice(0, 5)
                .map((run) => `${formatCompactTime(run)} (${formatTime(run)})`)
                .join(" · ")
            : timing.label}
        </span>
        <span>{timing.detail}</span>
      </span>
      <span>
        <strong>Last result</strong>
        <span>
          {schedule.last_run_at ? formatTime(schedule.last_run_at) : "Never"}
        </span>
        <span>{schedule.last_error || "No error reported"}</span>
      </span>
      <span>
        <strong>Execution policy</strong>
        <span>
          {schedule.enabled
            ? scheduleOperationInvalid(schedule)
              ? "Invalid operation — automatic and manual runs are blocked"
              : schedule.cadence_error
                ? "Invalid cadence — edit required; automatic runs are blocked"
                : "Enabled schedules authorize future runs automatically"
            : "Disabled schedules do not dispatch future runs"}
        </span>
        <span>{describeSchedulePolicy(schedule)}</span>
      </span>
    </div>
  );
}

type ScheduleRunTiming = {
  detail: string;
  futureRuns: string[];
  label: string;
  staleRuns: string[];
  tone: "info" | "neutral" | "ok" | "warn";
};

function scheduleRunTiming(schedule: ScheduleRecord): ScheduleRunTiming {
  if (scheduleOperationInvalid(schedule)) {
    return {
      detail:
        "Automatic and manual runs are blocked until a full reviewed edit replaces the saved operation.",
      futureRuns: [],
      label: "Invalid operation",
      staleRuns: [],
      tone: "warn",
    };
  }
  const cadenceError = scheduleCadenceErrorDetail(schedule);
  if (cadenceError) {
    return {
      detail: `Invalid cadence — edit required. ${cadenceError}`,
      futureRuns: [],
      label: "Invalid cadence",
      staleRuns: [],
      tone: "warn",
    };
  }
  const runs = parseScheduleRuns(schedule);
  const now = Date.now();
  const futureRuns = runs
    .filter((run) => run.time > now)
    .map((run) => run.value);
  const staleRuns = runs
    .filter((run) => run.time <= now)
    .map((run) => run.value);
  if (futureRuns.length > 0) {
    const staleDetail =
      staleRuns.length > 0
        ? `; ${staleRuns.length} stale ${staleRuns.length === 1 ? "time hidden" : "times hidden"}`
        : "";
    return {
      detail: `${futureRuns.length} future ${futureRuns.length === 1 ? "run" : "runs"} returned${staleDetail}`,
      futureRuns,
      label: "Scheduled",
      staleRuns,
      tone: "ok",
    };
  }
  if (staleRuns.length > 0) {
    const latestStale = staleRuns[staleRuns.length - 1];
    const overdueAge = scheduleOverdueAge(latestStale);
    return {
      detail: `${overdueAge}; schedule calculation stale; latest returned time was ${formatCompactTime(latestStale)}`,
      futureRuns,
      label: schedule.enabled ? "Overdue" : "No future runs",
      staleRuns,
      tone: schedule.enabled ? "warn" : "neutral",
    };
  }
  return {
    detail: schedule.enabled
      ? "Schedule calculation stale; no valid future run returned"
      : "No future runs while disabled",
    futureRuns,
    label: schedule.enabled ? "Schedule stale" : "Disabled",
    staleRuns,
    tone: schedule.enabled ? "warn" : "neutral",
  };
}

function scheduleOperationInvalid(schedule: ScheduleRecord): boolean {
  return !schedule.operation || Boolean(schedule.operation_error);
}

function scheduleCadenceErrorDetail(schedule: ScheduleRecord): string | null {
  if (!schedule.cadence_error) {
    return null;
  }
  if (schedule.cadence_error === "schedule_cron_invalid") {
    return "The saved cron expression is invalid.";
  }
  if (schedule.cadence_error === "schedule_cron_no_future_occurrence") {
    return "The saved cron expression has no future occurrence.";
  }
  return `The API reported ${schedule.cadence_error.replace(/_/g, " ")}.`;
}

function scheduleOverdueAge(value: string): string {
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) {
    return "Overdue age unknown";
  }
  const deltaMs = Math.max(0, Date.now() - ms);
  const totalHours = Math.max(1, Math.round(deltaMs / 3_600_000));
  const weeks = Math.floor(totalHours / (24 * 7));
  const days = Math.floor((totalHours % (24 * 7)) / 24);
  const hours = totalHours % 24;
  if (weeks > 0) {
    return days > 0 ? `Overdue by ${weeks}w ${days}d` : `Overdue by ${weeks}w`;
  }
  if (days > 0) {
    return hours > 0 ? `Overdue by ${days}d ${hours}h` : `Overdue by ${days}d`;
  }
  return `Overdue by ${hours}h`;
}

function nextRunList(schedule: ScheduleRecord): string[] {
  return parseScheduleRuns(schedule).map((run) => run.value);
}

function parseScheduleRuns(
  schedule: ScheduleRecord,
): Array<{ time: number; value: string }> {
  const runs = Array.isArray(schedule.next_runs) ? schedule.next_runs : [];
  const unique = new Set<string>();
  if (schedule.next_run_at) {
    unique.add(schedule.next_run_at);
  }
  for (const run of runs) {
    if (run) {
      unique.add(run);
    }
  }
  return Array.from(unique)
    .map((value) => ({ time: Date.parse(value), value }))
    .filter((run) => Number.isFinite(run.time))
    .sort((left, right) => left.time - right.time);
}

function describeCronExpression(expr: string): string {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) {
    return "Invalid schedule";
  }
  const [minute, hour, dom, month, dow] = fields;
  if (
    minute.startsWith("*/") &&
    hour === "*" &&
    dom === "*" &&
    month === "*" &&
    dow === "*"
  ) {
    const interval = Number(minute.slice(2));
    return Number.isInteger(interval) && interval > 0
      ? `Every ${interval} minutes`
      : "Custom cron schedule";
  }
  if (hour === "*" && dom === "*" && month === "*" && dow === "*") {
    return `Hourly at ${minuteLabel(minute)}`;
  }
  if (dom === "*" && month === "*" && dow === "*") {
    return `Daily at ${timeLabel(hour, minute)} UTC`;
  }
  if (dom === "*" && month === "*" && dow !== "*") {
    return `Weekly ${weekdayLabel(dow)} at ${timeLabel(hour, minute)} UTC`;
  }
  if (month === "*" && dow === "*") {
    return `Monthly on day ${dom} at ${timeLabel(hour, minute)} UTC`;
  }
  return "Custom cron schedule";
}

function formatSchedulePreviewTime(value: string): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) {
    return value;
  }
  const now = new Date();
  const sameLocalDay =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  return new Intl.DateTimeFormat(undefined, {
    ...(sameLocalDay ? {} : { weekday: "short" as const }),
    hour: "numeric",
    minute: "2-digit",
  }).format(date);
}

function minuteLabel(value: string): string {
  if (/^\d+$/.test(value)) {
    return `minute ${Number(value)}`;
  }
  return `minutes ${value}`;
}

function timeLabel(hour: string, minute: string): string {
  if (/^\d+$/.test(hour) && /^\d+$/.test(minute)) {
    return `${String(Number(hour)).padStart(2, "0")}:${String(Number(minute)).padStart(2, "0")}`;
  }
  return `${hour}:${minute}`;
}

function weekdayLabel(value: string): string {
  const names = new Map([
    ["0", "Sunday"],
    ["7", "Sunday"],
    ["1", "Monday"],
    ["2", "Tuesday"],
    ["3", "Wednesday"],
    ["4", "Thursday"],
    ["5", "Friday"],
    ["6", "Saturday"],
  ]);
  return value
    .split(",")
    .map((part) => names.get(part) ?? `weekday ${part}`)
    .join(", ");
}

function operationSummary(operation: JobOperation | null): string {
  if (!operation) {
    return "command";
  }
  switch (operation.type) {
    case "shell":
      return operation.argv.join(" ") || "shell";
    case "shell_script":
      return "shell script";
    case "terminal_open":
      return `terminal ${operation.argv.join(" ") || "session"}`;
    case "backup":
      return `backup ${operation.include_config ? "config" : "paths"}${
        operation.follow_symlinks ? ", follows symlinks" : ""
      }`;
    default:
      return operation.type;
  }
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function countPhrase(
  count: number,
  singular: string,
  plural = `${singular}s`,
): string {
  return `${count} ${count === 1 ? singular : plural}`;
}

function blurActiveElement() {
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }),
    );
    document.activeElement.blur();
  }
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function readLocalString(key: string, fallback: string): string {
  if (typeof window === "undefined") {
    return fallback;
  }
  return window.localStorage.getItem(key) ?? fallback;
}

function writeLocalString(key: string, value: string) {
  if (typeof window === "undefined") {
    return;
  }
  if (value.trim()) {
    window.localStorage.setItem(key, value);
  } else {
    window.localStorage.removeItem(key);
  }
}
