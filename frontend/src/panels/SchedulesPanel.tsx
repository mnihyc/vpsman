import { useEffect, useMemo, useState, type FormEvent } from "react";
import {
  ClipboardList,
  Clock3,
  Pencil,
  Play,
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
import type {
  AgentView,
  BulkResolveResponse,
  CommandTemplateRecord,
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

const SCHEDULE_SELECTOR_STORAGE_KEY = "vpsman.schedules.selectorExpression";

export function SchedulesPanel({
  activeSubpage: _activeSubpage,
  agents,
  commandTemplates,
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
}: {
  activeSubpage: string;
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  error: string | null;
  loading: boolean;
  onApplyScheduleNow: (
    scheduleId: string,
    request: SchedulePrivilegeMutationRequest,
  ) => Promise<void>;
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
}) {
  const [name, setName] = useState("");
  const [selectedTemplateId, setSelectedTemplateId] = useState("");
  const [commandText, setCommandText] = useState("");
  const [cronExpr, setCronExpr] = useState("0 * * * *");
  const [enabled, setEnabled] = useState(true);
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
  const [pending, setPending] = useState(false);
  const [editingScheduleId, setEditingScheduleId] = useState<string | null>(
    null,
  );
  const [scheduleAction, setScheduleAction] = useState<ScheduleAction | null>(
    null,
  );
  const [scheduleActionError, setScheduleActionError] = useState<string | null>(
    null,
  );
  const [deferDraft, setDeferDraft] = useState<{
    schedule: ScheduleRecord;
    deferredUntil: string;
    reason: string;
  } | null>(null);

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
  const selectedTargetIds = useMemo(
    () =>
      selectorParse.error
        ? []
        : agentsMatchingExpression(agents, selectorExpression).map(
            (agent) => agent.id,
          ),
    [agents, selectorExpression, selectorParse.error],
  );
  const selectedTargetCount = selectedTargetIds.length;
  const nextRuns = useMemo(() => previewNextCronRuns(cronExpr, 5), [cronExpr]);
  const ready =
    name.trim().length > 0 &&
    scheduleOperation !== null &&
    nextRuns.length > 0 &&
    selectorExpression.trim().length > 0 &&
    !selectorParse.error;
  const status =
    actionError ??
    error ??
    (loading ? "Loading" : `${schedules.length} schedules`);
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
        pendingScheduleSnapshot?.targetClientIds.slice(0, 8).join(", ") ||
        selectedTargetIds.slice(0, 8).join(", ") ||
        "-",
    },
    {
      label: "Operation",
      value: pendingScheduleSnapshot
        ? pendingScheduleSnapshot.selectedTemplateName ??
          operationSummary(pendingScheduleSnapshot.operation)
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
        ? formatCompactTime(confirmationNextRun)
        : "invalid",
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
      value: (pendingScheduleSnapshot?.enabled ?? enabled)
        ? "Enabled"
        : "Disabled",
    },
  ];

  const scheduleColumns = useMemo<ConsoleDataGridColumn<ScheduleRecord>[]>(
    () => [
      {
        id: "name",
        header: "Name",
        size: 220,
        minSize: 160,
        sortValue: (schedule) => schedule.name,
        searchValue: (schedule) => `${schedule.name} ${schedule.id}`,
        cell: (schedule) => (
          <span className="historyPrimary">
            <strong>{schedule.name}</strong>
            <small>{shortId(schedule.id)}</small>
          </span>
        ),
      },
      {
        id: "operation",
        header: "Operation",
        size: 110,
        minSize: 95,
        sortValue: (schedule) => schedule.command_type,
        searchValue: (schedule) => schedule.command_type,
        cell: (schedule) => schedule.command_type,
      },
      {
        id: "targets",
        header: "Targets",
        size: 230,
        minSize: 190,
        sortValue: (schedule) => fixedTargetIds(schedule).length,
        searchValue: (schedule) =>
          `${schedule.selector_expression} ${fixedTargetIds(schedule).join(" ")}`,
        cell: (schedule) => {
          const fixedIds = fixedTargetIds(schedule);
          return (
            <span className="historyPrimary">
              <strong>{fixedIds.length} fixed VPSs</strong>
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
        header: "Schedule",
        size: 190,
        minSize: 160,
        sortValue: (schedule) => schedule.cron_expr,
        searchValue: (schedule) =>
          `${schedule.cron_expr} ${describeCronExpression(schedule.cron_expr)} ${schedule.timezone}`,
        cell: (schedule) => (
          <span className="historyPrimary scheduleCadenceCell" title={`${schedule.cron_expr} ${schedule.timezone}`}>
            <strong>{describeCronExpression(schedule.cron_expr)}</strong>
            <small>{schedule.cron_expr} · {schedule.timezone}</small>
          </span>
        ),
      },
      {
        id: "policy",
        header: "Policy",
        size: 170,
        minSize: 150,
        sortValue: (schedule) => schedule.catch_up_policy,
        searchValue: (schedule) =>
          `${schedule.catch_up_policy} ${schedule.retry_delay_secs}`,
        cell: (schedule) => (
          <span className="historyPrimary" title={describeSchedulePolicy(schedule)}>
            <strong>{formatCatchUpPolicy(schedule.catch_up_policy)}</strong>
            <small>{describeSchedulePolicy(schedule)}</small>
          </span>
        ),
      },
      {
        id: "nextRun",
        header: "Next runs",
        size: 230,
        minSize: 190,
        sortValue: (schedule) => schedule.next_run_at,
        searchValue: (schedule) =>
          `${schedule.next_run_at} ${schedule.next_runs.join(" ")} ${schedule.last_run_at ?? ""}`,
        cell: (schedule) => (
          <span className="scheduleRunsCell">
            <span className="scheduleRunChips">
              {nextRunList(schedule).slice(0, 5).map((run) => (
                <span className="targetChip" key={run}>{formatCompactTime(run)}</span>
              ))}
            </span>
            <small>
              Last {schedule.last_run_at ? formatCompactTime(schedule.last_run_at) : "never"}
            </small>
          </span>
        ),
      },
      {
        id: "state",
        header: "State",
        size: 120,
        minSize: 105,
        sortValue: (schedule) =>
          `${schedule.enabled ? "enabled" : "disabled"} ${schedule.failure_count}`,
        searchValue: (schedule) =>
          `${schedule.enabled ? "enabled" : "disabled"} ${schedule.last_error ?? ""}`,
        cell: (schedule) => (
          <span className="historyPrimary">
            <span className={schedule.enabled ? "status ok" : "status neutral"}>
              {schedule.enabled ? "enabled" : "disabled"}
            </span>
            {schedule.failure_count > 0 && (
              <small>
                {schedule.failure_count}/{schedule.max_failures} failures
              </small>
            )}
            {schedule.last_error && <small>{schedule.last_error}</small>}
          </span>
        ),
      },
    ],
    [agents, commandTemplates],
  );

  async function submitSchedule(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setActionError(null);
    if (!ready) {
      setActionError("Schedule is incomplete");
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      const snapshot = await buildScheduleDraftSnapshot();
      setPendingScheduleSnapshot(snapshot);
      blurActiveElement();
      window.setTimeout(() => setConfirmationOpen(true), 140);
    });
  }

  async function saveScheduleNow() {
    setConfirmationOpen(false);
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
        await onUpdateSchedule(snapshot.editingScheduleId, request);
      } else {
        await onCreateSchedule(request);
      }
      setName("");
      setCommandText("");
      setSelectedTemplateId("");
      setEditingScheduleId(null);
      setPendingScheduleSnapshot(null);
    });
  }

  async function buildScheduleDraftSnapshot(): Promise<ScheduleDraftSnapshot> {
    if (!ready || !scheduleOperation) {
      throw new Error("Schedule is incomplete");
    }
    const selector = selectorExpression.trim();
    const resolved = await onResolveTargets({ selector_expression: selector });
    const targetClientIds = resolved.targets.map((target) => target.id);
    if (!targetClientIds.length) {
      throw new Error("Schedule confirmation resolved no VPSs");
    }
    return {
      editingScheduleId,
      name: name.trim(),
      operation: scheduleOperation,
      commandType: commandTypeForApi(scheduleOperation),
      selectorExpression: selector,
      targetClientIds,
      cronExpr: cronExpr.trim(),
      enabled,
      catchUpPolicy,
      catchUpLimit: clampInteger(catchUpLimit, 1, 25),
      retryDelaySecs: clampInteger(retryDelaySecs, 1, 86_400),
      maxFailures: clampInteger(maxFailures, 1, 100),
      nextRun: nextRuns[0] ?? null,
      selectedTemplateName: selectedTemplate?.name ?? null,
    };
  }

  function editSchedule(schedule: ScheduleRecord) {
    setPendingScheduleSnapshot(null);
    const matchingTemplate = commandTemplates.find(
      (template) =>
        JSON.stringify(template.operation) ===
        JSON.stringify(schedule.operation),
    );
    if (schedule.operation.type !== "shell" && !matchingTemplate) {
      setActionError(
        "Non-shell schedules can be modified from their command template",
      );
      return;
    }
    setEditingScheduleId(schedule.id);
    setName(schedule.name);
    setSelectedTemplateId(matchingTemplate?.id ?? "");
    setCommandText(
      schedule.operation.type === "shell"
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
  }

  function startDefer(schedule: ScheduleRecord) {
    const nextHour = new Date(Date.now() + 60 * 60 * 1000);
    setDeferDraft({
      schedule,
      deferredUntil: toDatetimeLocal(nextHour),
      reason: "",
    });
  }

  function openScheduleAction(action: ScheduleAction) {
    setScheduleActionError(null);
    setScheduleAction(action);
  }

  function reviewApplyNow(schedule: ScheduleRecord) {
    setScheduleActionError(null);
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      setActionError("Privilege unlock is required");
      return;
    }
    openScheduleAction({ type: "applyNow", schedule });
  }

  async function runScheduleAction(action: ScheduleAction) {
    if (pending) return;
    setPending(true);
    setActionError(null);
    setScheduleActionError(null);
    try {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required");
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
      const targetIds =
        action.type === "targetUpdate"
          ? action.targetClientIds
          : fixedTargetIds(action.schedule);
      const selectorExpressionForIntent =
        action.type === "targetUpdate"
          ? action.selectorExpression
          : action.schedule.selector_expression;
      const privilegeAssertion = await buildSchedulePrivilege(
        action.schedule,
        actionName(action),
        nextEnabled,
        deferredUntil,
        action.type === "delete",
        targetIds,
        selectorExpressionForIntent,
      );
      if (action.type === "enable") {
        await onEnableSchedule(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
      } else if (action.type === "disable") {
        await onDisableSchedule(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
      } else if (action.type === "defer") {
        await onDeferSchedule(action.schedule.id, {
          deferred_until: action.deferredUntil,
          reason: action.reason || null,
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
      } else if (action.type === "delete") {
        await onDeleteSchedule(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
      } else if (action.type === "applyNow") {
        await onApplyScheduleNow(action.schedule.id, {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
      } else if (action.type === "targetUpdate") {
        await onUpdateScheduleTargets(action.schedule.id, {
          selector_expression: action.selectorExpression,
          target_client_ids: action.targetClientIds,
          confirmed: true,
          privilege_assertion: privilegeAssertion,
        });
      }
      setScheduleAction(null);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Schedule action failed";
      setActionError(message);
      setScheduleActionError(message);
    } finally {
      setPending(false);
    }
  }

  async function reviewScheduleTargetUpdate(schedule: ScheduleRecord) {
    if (pending) return;
    setPending(true);
    setActionError(null);
    setScheduleActionError(null);
    try {
      const selectorExpressionForIntent = schedule.selector_expression.trim();
      if (!selectorExpressionForIntent) {
        throw new Error("Schedule has no audit selector");
      }
      const resolved = await onResolveTargets({
        selector_expression: selectorExpressionForIntent,
      });
      const targetClientIds = resolved.targets.map((target) => target.id);
      if (!targetClientIds.length) {
        throw new Error("Schedule audit selector resolved no VPSs");
      }
      if (sameStringSet(fixedTargetIds(schedule), targetClientIds)) {
        throw new Error(
          "Saved fixed targets already match current audit selector resolution",
        );
      }
      openScheduleAction({
        type: "targetUpdate",
        schedule,
        selectorExpression: selectorExpressionForIntent,
        targetClientIds,
      });
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Target update review failed";
      setActionError(message);
      setScheduleActionError(message);
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
    if (!targetIds.length) {
      throw new Error("Schedule has no fixed VPS targets");
    }
    const operationHash = await operationPayloadHashHex(schedule.operation);
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
    if (action.type === "applyNow") {
      return "Dispatches a normal job from the saved fixed target snapshot without changing the next scheduled run.";
    }
    if (action.type === "defer") {
      return `Pauses automatic execution until ${formatCompactTime(action.deferredUntil)}.`;
    }
    if (action.type === "targetUpdate") {
      return "Replaces the saved fixed target snapshot with the audit selector's current resolution. Future runs use the new fixed target list.";
    }
    return `${actionConfirmLabel(action.type)} ${action.schedule.name}.`;
  }

  function actionConfirmationItems(action: ScheduleAction) {
    const items = [
      {
        label: "Schedule",
        value: `${action.schedule.name} (${shortId(action.schedule.id)})`,
      },
      { label: "Operation", value: action.schedule.command_type },
      {
        label: "Fixed targets",
        value: `${fixedTargetIds(action.schedule).length} saved`,
      },
      {
        label: "Audit selector",
        value: action.schedule.selector_expression || "-",
      },
      {
        label: "State",
        value: action.schedule.enabled ? "enabled" : "disabled",
      },
    ];
    if (action.type === "targetUpdate") {
      items.push({
        label: "Selector resolves now",
        value: `${action.targetClientIds.length} VPSs`,
      });
      items.push({
        label: "Target preview",
        value: action.targetClientIds.slice(0, 8).join(", ") || "-",
      });
    }
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

  const scheduleActions: ConsoleDataGridAction<ScheduleRecord>[] = [
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Edit",
          "Opens the schedule composer.",
        ),
      disabled: (rows) => rows.length !== 1,
      icon: <Pencil size={14} />,
      label: "Edit",
      onSelect: (rows) => rows[0] && editSchedule(rows[0]),
    },
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Enable",
          "Automatic runs will resume.",
        ),
      label: "Review enable",
      disabled: (rows) => rows.length !== 1 || rows[0]?.enabled === true,
      icon: <Power size={14} />,
      onSelect: (rows) =>
        rows[0] &&
        openScheduleAction({ type: "enable", schedule: rows[0] }),
    },
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Disable",
          "Automatic runs will stop.",
        ),
      label: "Review disable",
      disabled: (rows) => rows.length !== 1 || rows[0]?.enabled === false,
      icon: <PowerOff size={14} />,
      onSelect: (rows) =>
        rows[0] &&
        openScheduleAction({ type: "disable", schedule: rows[0] }),
    },
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Apply",
          "Dispatches one job from the saved fixed target snapshot.",
          " now",
        ),
      label: "Review apply",
      disabled: (rows) => rows.length !== 1 || rows[0]?.enabled !== true,
      icon: <Play size={14} />,
      onSelect: (rows) => rows[0] && reviewApplyNow(rows[0]),
    },
    {
      description: (rows) => describeScheduleTargetUpdate(rows),
      label: "Review target update",
      disabled: (rows) =>
        rows.length !== 1 || !rows[0]?.selector_expression.trim(),
      icon: <Target size={14} />,
      onSelect: (rows) => {
        const schedule = rows[0];
        if (!schedule) return;
        void reviewScheduleTargetUpdate(schedule);
      },
    },
    {
      description: (rows) =>
        describeScheduleAction(
          rows,
          "Defer",
          "Opens a defer form before confirmation.",
        ),
      label: "Defer",
      disabled: (rows) => rows.length !== 1,
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
      disabled: (rows) => rows.length !== 1,
      icon: <Trash2 size={14} />,
      onSelect: (rows) =>
        rows[0] &&
        openScheduleAction({ type: "delete", schedule: rows[0] }),
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
          rows
            .flatMap((schedule) => fixedTargetIds(schedule))
            .join("\n"),
        ),
    },
    {
      label: "Copy audit selectors",
      onSelect: (rows) =>
        void copyText(
          rows
            .map((schedule) => schedule.selector_expression)
            .join("\n"),
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
          <div className="inlineActions">
            <button
              className="secondaryAction compactAction"
              disabled={!onOpenScheduledRuns}
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
        </div>
        <ConsoleDataGrid
          actions={scheduleActions}
          columns={scheduleColumns}
          defaultPageSize={10}
          empty={
            schedules.length === 0 ? (
              <div className="emptyState compactEmpty">
                <Clock3 size={22} />
                <strong>No schedules yet</strong>
                <span>Create a schedule below to run a command template on a fixed target snapshot.</span>
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
            <div className="gridDetailLine">
              <strong>{schedule.command_type}</strong>
              <span>{fixedTargetIds(schedule).length} fixed targets</span>
              <span className="monoCell">
                audit: {schedule.selector_expression || "none"}
              </span>
              <span>
                next {formatCompactTime(schedule.next_run_at)}
              </span>
              <span>
                last{" "}
                {schedule.last_run_at
                  ? formatTime(schedule.last_run_at)
                  : "never"}
              </span>
              <span>{describeSchedulePolicy(schedule)}</span>
            </div>
          )}
          rowActions={scheduleActions}
          rows={schedules}
          storageKey="vpsman.grid.schedules"
          title="Schedule records"
        />
        <div className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`} aria-label="Schedule lifecycle privilege gate">
          <ShieldCheck size={16} />
          <span>
            {privilegeMaterial
              ? "Privilege unlocked for schedule lifecycle actions"
              : "Open Privilege Vault to enable apply now, target updates, enable, disable, and delete"}
          </span>
          {!privilegeMaterial && (
            <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
              Open Privilege Vault
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
            scheduleAction ? actionConfirmLabel(scheduleAction.type) : "Run schedule action"
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

      <section className="scheduleComposer">
        <ConsoleCollapsibleSection
          forceOpenKey={editingScheduleId}
          defaultOpen={schedules.length === 0}
          storageKey="vpsman.panel.schedules.create"
          summary={schedules.length === 0 ? "Create the first recurring job" : `${selectedTargetCount} matching VPSs in local preview`}
          title={editingScheduleId ? "Modify schedule" : "Create schedule"}
        >
          <form className="dispatchForm" onSubmit={submitSchedule}>
            <label>
              <span>Name</span>
              <input
                aria-label="Schedule name"
                onChange={(event) => setName(event.target.value)}
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
                <span>UTC cron</span>
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
                <span>Catch-up</span>
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
                <span>Catch-up limit</span>
                <input
                  aria-label="Schedule catch-up limit"
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
                <span>Retry delay</span>
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
                <span>Max failures</span>
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
                <strong>Audit selector</strong>
                <span>{selectedTargetCount} VPSs in local preview</span>
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
                    : "no selector")
                }
              />
            </div>
            <div className="schedulePreview">
              <strong>Next runs</strong>
              <span>
                {nextRuns.length
                  ? `${describeCronExpression(cronExpr)}. UTC schedule, displayed in browser timezone`
                  : "Invalid or unsupported cron expression"}
              </span>
              <div className="targetChipList">
                {nextRuns.map((run) => (
                  <span className="targetChip" key={run}>
                    {formatCompactTime(run)}
                  </span>
                ))}
              </div>
              <small>
                {selectedTargetCount} matching VPSs in local preview;{" "}
                {selectedTemplate
                  ? selectedTemplate.name
                  : operationSummary(scheduleOperation)}
              </small>
            </div>
            {!confirmationOpen && (
              <button
                className="primaryAction"
                disabled={pending || !ready}
                type="submit"
              >
                <Save size={17} />
                {editingScheduleId ? "Review update" : "Review save"}
              </button>
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
              )}. The resolved target list is saved as a fixed snapshot; the selector is retained for audit and future manual Target update.`}
              items={confirmationItems}
              onCancel={() => {
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
      schedule: ScheduleRecord;
      selectorExpression: string;
      targetClientIds: string[];
    }
  | { type: "delete"; schedule: ScheduleRecord }
  | {
      type: "defer";
      schedule: ScheduleRecord;
      deferredUntil: string;
      reason: string;
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
      return "Confirm schedule enable";
    case "disable":
      return "Confirm schedule disable";
    case "defer":
      return "Confirm schedule defer";
    case "applyNow":
      return "Confirm apply now";
    case "targetUpdate":
      return "Confirm target update";
    case "delete":
      return "Confirm schedule delete";
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
      return "Apply now";
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

function describeScheduleTargetUpdate(rows: ScheduleRecord[]): string {
  const scheduleName = rows[0]?.name ?? "selected schedule";
  return `Update targets for schedule ${scheduleName}. Replaces saved fixed targets with the current audit selector resolution.`;
}

function fixedTargetIds(schedule: ScheduleRecord): string[] {
  return Array.isArray(schedule.target_client_ids)
    ? schedule.target_client_ids
    : [];
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

function previewNextCronRuns(expr: string, count: number): string[] {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) {
    return [];
  }
  const [minuteExpr, hourExpr, domExpr, monthExpr, dowExpr] = fields;
  const minutes = parseCronField(minuteExpr, 0, 59);
  const hours = parseCronField(hourExpr, 0, 23);
  const months = parseCronField(monthExpr, 1, 12);
  if (!minutes || !hours || !months) {
    return [];
  }
  const domAny = domExpr === "*";
  const dowAny = dowExpr === "*";
  const domValues = domAny ? null : parseCronField(domExpr, 1, 31);
  const dowValues = dowAny ? null : parseCronField(dowExpr, 0, 7);
  if ((!domAny && !domValues) || (!dowAny && !dowValues)) {
    return [];
  }
  const result: string[] = [];
  const cursor = new Date();
  cursor.setUTCSeconds(0, 0);
  cursor.setUTCMinutes(cursor.getUTCMinutes() + 1);
  const deadline = cursor.getTime() + 366 * 24 * 60 * 60 * 1000;
  while (result.length < count && cursor.getTime() <= deadline) {
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

function nextRunList(schedule: ScheduleRecord): string[] {
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
  return Array.from(unique);
}

function describeCronExpression(expr: string): string {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) {
    return "Invalid schedule";
  }
  const [minute, hour, dom, month, dow] = fields;
  if (minute.startsWith("*/") && hour === "*" && dom === "*" && month === "*" && dow === "*") {
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
