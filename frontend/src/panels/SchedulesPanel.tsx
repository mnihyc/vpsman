import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Clock3, Pencil, Play, RefreshCcw, Save, Trash2 } from "lucide-react";
import {
  ConsoleDataGrid,
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
  CommandTemplateRecord,
  CreateScheduleRequest,
  DeferScheduleRequest,
  JobOperation,
  ScheduleRecord,
  SchedulePrivilegeMutationRequest,
  UpdateScheduleRequest,
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
  onRefresh,
  onUpdateSchedule,
  privilegeMaterial,
  schedules,
}: {
  activeSubpage: string;
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  error: string | null;
  loading: boolean;
  onApplyScheduleNow: (scheduleId: string) => Promise<void>;
  onCreateSchedule: (request: CreateScheduleRequest) => Promise<void>;
  onDeferSchedule: (scheduleId: string, request: DeferScheduleRequest) => Promise<void>;
  onDeleteSchedule: (scheduleId: string, request: SchedulePrivilegeMutationRequest) => Promise<void>;
  onDisableSchedule: (scheduleId: string, request: SchedulePrivilegeMutationRequest) => Promise<void>;
  onEnableSchedule: (scheduleId: string, request: SchedulePrivilegeMutationRequest) => Promise<void>;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => Promise<void>;
  onUpdateSchedule: (scheduleId: string, request: UpdateScheduleRequest) => Promise<void>;
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
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [editingScheduleId, setEditingScheduleId] = useState<string | null>(null);
  const [scheduleAction, setScheduleAction] = useState<ScheduleAction | null>(null);
  const [deferDraft, setDeferDraft] = useState<{ schedule: ScheduleRecord; deferredUntil: string; reason: string } | null>(null);

  const argv = useMemo(() => {
    try {
      return parseCommandArgv(commandText);
    } catch {
      return [];
    }
  }, [commandText]);
  const selectedTemplate = useMemo(
    () => commandTemplates.find((template) => template.id === selectedTemplateId) ?? null,
    [commandTemplates, selectedTemplateId],
  );
  const scheduleOperation = useMemo<JobOperation | null>(
    () => selectedTemplate?.operation ?? (argv.length > 0 ? { type: "shell", argv, pty: false } : null),
    [argv, selectedTemplate],
  );
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const nextRuns = useMemo(() => previewNextCronRuns(cronExpr, 5), [cronExpr]);
  const selectedTargetCount = useMemo(
    () =>
      selectorParse.error
        ? 0
        : agentsMatchingExpression(agents, selectorExpression).length,
    [agents, selectorExpression, selectorParse.error],
  );
  const selectedTargetIds = useMemo(
    () =>
      selectorParse.error
        ? []
        : agentsMatchingExpression(agents, selectorExpression).map((agent) => agent.id),
    [agents, selectorExpression, selectorParse.error],
  );
  const ready =
    name.trim().length > 0 &&
    scheduleOperation !== null &&
    nextRuns.length > 0 &&
    selectorExpression.trim().length > 0 &&
    !selectorParse.error &&
    selectedTargetCount > 0;
  const status =
    actionError ??
    error ??
    (loading ? "Loading" : `${schedules.length} schedules`);
  const confirmationItems = [
    { label: "Name", value: name.trim() || "-" },
    { label: "Selector", value: selectorExpression.trim() || "-" },
    { label: "Targets", value: `${selectedTargetCount} resolved` },
    { label: "Operation", value: selectedTemplate ? selectedTemplate.name : operationSummary(scheduleOperation) },
    { label: "Cron", value: `${cronExpr.trim()} UTC` },
    { label: "Next", value: nextRuns[0] ? formatCompactTime(nextRuns[0]) : "invalid" },
    { label: "Catch-up", value: formatCatchUpPolicy(catchUpPolicy) },
    { label: "Retry", value: formatInterval(clampInteger(retryDelaySecs, 1, 86_400)) },
    { label: "State", value: enabled ? "Enabled" : "Disabled" },
  ];
  const scheduleColumns = useMemo<ConsoleDataGridColumn<ScheduleRecord>[]>(
    () => [
      {
        id: "actions",
        header: "",
        size: 210,
        minSize: 180,
        cell: (schedule) => (
          <div className="rowActionGroup">
            <button aria-label={`Edit ${schedule.name}`} onClick={() => editSchedule(schedule)} type="button">
              <Pencil size={15} />
            </button>
            <button
              aria-label={schedule.enabled ? `Disable ${schedule.name}` : `Enable ${schedule.name}`}
              onClick={() => setScheduleAction({ type: schedule.enabled ? "disable" : "enable", schedule })}
              type="button"
            >
              {schedule.enabled ? "Disable" : "Enable"}
            </button>
            <button
              aria-label={`Apply ${schedule.name} now`}
              disabled={!schedule.enabled}
              onClick={() => setScheduleAction({ type: "applyNow", schedule })}
              type="button"
            >
              <Play size={15} />
            </button>
            <button aria-label={`Defer ${schedule.name}`} onClick={() => startDefer(schedule)} type="button">
              <Clock3 size={15} />
            </button>
            <button aria-label={`Delete ${schedule.name}`} onClick={() => setScheduleAction({ type: "delete", schedule })} type="button">
              <Trash2 size={15} />
            </button>
          </div>
        ),
      },
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
        size: 100,
        minSize: 90,
        sortValue: (schedule) => schedule.command_type,
        searchValue: (schedule) => schedule.command_type,
        cell: (schedule) => schedule.command_type,
      },
      {
        id: "targets",
        header: "Targets",
        size: 85,
        minSize: 75,
        align: "end",
        sortValue: (schedule) => schedule.selector_expression,
        searchValue: (schedule) => schedule.selector_expression,
        cell: (schedule) => (
          <span className="monoCell compactSelectorText">
            {schedule.selector_expression}
          </span>
        ),
      },
      {
        id: "cron",
        header: "Cron",
        size: 90,
        minSize: 80,
        sortValue: (schedule) => schedule.cron_expr,
        searchValue: (schedule) => schedule.cron_expr,
        cell: (schedule) => (
          <span className="historyPrimary">
            <strong>{schedule.cron_expr}</strong>
            <small>{schedule.timezone}</small>
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
          <span className="historyPrimary">
            <strong>{formatCatchUpPolicy(schedule.catch_up_policy)}</strong>
            <small>
              {schedule.catch_up_policy === "run_all_limited"
                ? `limit ${schedule.catch_up_limit}`
                : `retry ${formatInterval(schedule.retry_delay_secs)}`}
            </small>
          </span>
        ),
      },
      {
        id: "nextRun",
        header: "Next run",
        size: 160,
        minSize: 140,
        sortValue: (schedule) => schedule.next_run_at,
        searchValue: (schedule) => schedule.next_run_at,
        cell: (schedule) => formatCompactTime(schedule.next_run_at),
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
    [commandTemplates],
  );

  async function submitSchedule(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setActionError(null);
    if (!ready) {
      setActionError("Schedule is incomplete");
      return;
    }
    blurActiveElement();
    window.setTimeout(() => setConfirmationOpen(true), 140);
  }

  async function saveScheduleNow() {
    setConfirmationOpen(false);
    await runPanelAction(setPending, setActionError, async () => {
      if (!ready || !scheduleOperation) {
        throw new Error("Schedule is incomplete");
      }
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required");
      }
      const operationHash = await operationPayloadHashHex(scheduleOperation);
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalSchedulePrivilegeIntent({
          action: editingScheduleId ? "schedule.update" : "schedule.create",
          scheduleId: editingScheduleId,
          name: name.trim(),
          commandType: commandTypeForApi(scheduleOperation),
          operationPayloadHash: operationHash,
          selectorExpression: selectorExpression.trim(),
          resolvedTargets: selectedTargetIds,
          cronExpr: cronExpr.trim(),
          timezone: "UTC",
          enabled,
          catchUpPolicy,
          catchUpLimit: clampInteger(catchUpLimit, 1, 25),
          retryDelaySecs: clampInteger(retryDelaySecs, 1, 86_400),
          maxFailures: clampInteger(maxFailures, 1, 100),
          deferredUntil: null,
          deleted: false,
        }),
        privilegeMaterial,
      });
      const request: CreateScheduleRequest = {
        name: name.trim(),
        operation: scheduleOperation,
        selector_expression: selectorExpression.trim(),
        cron_expr: cronExpr.trim(),
        timezone: "UTC",
        enabled,
        catch_up_policy: catchUpPolicy,
        catch_up_limit: clampInteger(catchUpLimit, 1, 25),
        retry_delay_secs: clampInteger(retryDelaySecs, 1, 86_400),
        max_failures: clampInteger(maxFailures, 1, 100),
        privilege_assertion: privilegeAssertion,
      };
      if (editingScheduleId) {
        await onUpdateSchedule(editingScheduleId, request);
      } else {
        await onCreateSchedule(request);
      }
      setName("");
      setCommandText("");
      setSelectedTemplateId("");
      setEditingScheduleId(null);
    });
  }

  function editSchedule(schedule: ScheduleRecord) {
    const matchingTemplate = commandTemplates.find(
      (template) => JSON.stringify(template.operation) === JSON.stringify(schedule.operation),
    );
    if (schedule.operation.type !== "shell" && !matchingTemplate) {
      setActionError("Non-shell schedules can be modified from their command template");
      return;
    }
    setEditingScheduleId(schedule.id);
    setName(schedule.name);
    setSelectedTemplateId(matchingTemplate?.id ?? "");
    setCommandText(schedule.operation.type === "shell" ? operationToCommandText(schedule.operation) : "");
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

  async function runScheduleAction(action: ScheduleAction) {
    setScheduleAction(null);
    await runPanelAction(setPending, setActionError, async () => {
      if (action.type === "applyNow") {
        await onApplyScheduleNow(action.schedule.id);
        return;
      }
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required");
      }
      const enabledForIntent =
        action.type === "enable" ? true : action.type === "disable" || action.type === "delete" ? false : action.schedule.enabled;
      const deferredUntil = action.type === "defer" ? action.deferredUntil : action.schedule.deferred_until;
      const privilegeAssertion = await buildSchedulePrivilege(action.schedule, actionName(action), enabledForIntent, deferredUntil, action.type === "delete");
      if (action.type === "enable") {
        await onEnableSchedule(action.schedule.id, { privilege_assertion: privilegeAssertion });
      } else if (action.type === "disable") {
        await onDisableSchedule(action.schedule.id, { privilege_assertion: privilegeAssertion });
      } else if (action.type === "defer") {
        await onDeferSchedule(action.schedule.id, {
          deferred_until: action.deferredUntil,
          reason: action.reason || null,
          privilege_assertion: privilegeAssertion,
        });
      } else if (action.type === "delete") {
        await onDeleteSchedule(action.schedule.id, { privilege_assertion: privilegeAssertion });
      }
    });
  }

  async function buildSchedulePrivilege(
    schedule: ScheduleRecord,
    action: string,
    nextEnabled: boolean,
    deferredUntil: string | null,
    deleted: boolean,
  ) {
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Privilege unlock is required");
    }
    const resolvedTargets = agentsMatchingExpression(agents, schedule.selector_expression).map((agent) => agent.id);
    if (!resolvedTargets.length) {
      throw new Error("Schedule target selector resolves no VPSs");
    }
    const operationHash = await operationPayloadHashHex(schedule.operation);
    return buildPrivilegeAssertion({
      intent: canonicalSchedulePrivilegeIntent({
        action,
        scheduleId: schedule.id,
        name: schedule.name,
        commandType: schedule.command_type,
        operationPayloadHash: operationHash,
        selectorExpression: schedule.selector_expression,
        resolvedTargets,
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
    const template = commandTemplates.find((candidate) => candidate.id === templateId);
    if (!template) {
      return;
    }
    if (!name.trim()) {
      setName(`${template.name} schedule`);
    }
  }

  useEffect(() => {
    writeLocalString(SCHEDULE_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  return (
    <div className="workspace singleColumn">
      <section className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Schedules</h2>
            <span>{status}</span>
          </div>
          <button
            className="secondaryAction"
            disabled={loading || pending}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCcw size={17} />
            Refresh
          </button>
        </div>
        <ConsoleDataGrid
          actions={[
            {
              label: "Copy schedule IDs",
              onSelect: (rows) =>
                void copyText(rows.map((schedule) => schedule.id).join("\n")),
            },
            {
              label: "Copy target selectors",
              onSelect: (rows) =>
                void copyText(
                  rows
                    .map((schedule) => schedule.selector_expression)
                    .join("\n"),
                ),
            },
          ]}
          columns={scheduleColumns}
          defaultPageSize={10}
          empty={
            <div className="emptyState compactEmpty">
              No schedules match the current search.
            </div>
          }
          getRowId={(schedule) => schedule.id}
          itemLabel="schedules"
          renderExpandedRow={(schedule) => (
            <div className="gridDetailLine">
              <strong>{schedule.command_type}</strong>
              <span className="monoCell">{schedule.selector_expression}</span>
              <span>
                last{" "}
                {schedule.last_run_at
                  ? formatTime(schedule.last_run_at)
                  : "never"}
              </span>
            </div>
          )}
          rows={schedules}
          storageKey="vpsman.grid.schedules"
          title="Schedule records"
        />
        {deferDraft && (
          <form
            className="inlineOpsForm"
            onSubmit={(event) => {
              event.preventDefault();
              setScheduleAction({
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
                onChange={(event) => setDeferDraft({ ...deferDraft, deferredUntil: event.target.value })}
                type="datetime-local"
                value={deferDraft.deferredUntil}
              />
            </label>
            <label>
              <span>Reason</span>
              <input
                aria-label="Schedule defer reason"
                onChange={(event) => setDeferDraft({ ...deferDraft, reason: event.target.value })}
                value={deferDraft.reason}
              />
            </label>
            <button className="primaryAction" disabled={pending || !deferDraft.deferredUntil} type="submit">
              <Clock3 size={17} />
              Defer
            </button>
            <button className="secondaryAction" onClick={() => setDeferDraft(null)} type="button">
              Cancel
            </button>
          </form>
        )}
        <ConfirmationPrompt
          confirmLabel={scheduleAction ? actionConfirmLabel(scheduleAction.type) : "Confirm"}
          detail={scheduleAction ? actionDetail(scheduleAction) : ""}
          items={scheduleAction ? actionConfirmationItems(scheduleAction) : []}
          onCancel={() => setScheduleAction(null)}
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
          title={scheduleAction ? actionTitle(scheduleAction.type) : "Confirm schedule action"}
        />
      </section>

      <section className="scheduleComposer">
        <ConsoleCollapsibleSection
          defaultOpen={false}
          forceOpenKey={editingScheduleId}
          storageKey="vpsman.panel.schedules.create"
          summary={`${selectedTargetCount} selected targets`}
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
                aria-label="Schedule command template"
                onChange={(event) => selectCommandTemplate(event.target.value)}
                value={selectedTemplateId}
              >
                <option value="">One-off shell argv</option>
                {commandTemplates.map((template) => (
                  <option key={template.id} value={template.id}>
                    {template.name} · {template.command_type}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Command argv</span>
              <textarea
                aria-label="Schedule command argv"
                disabled={selectedTemplate !== null}
                onChange={(event) => setCommandText(event.target.value)}
                rows={3}
                value={selectedTemplate ? operationSummary(selectedTemplate.operation) : commandText}
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
                <strong>Targets</strong>
                <span>{selectedTargetCount} matching VPSs</span>
              </div>
              <SearchExpressionInput
                agents={agents}
                ariaLabel="Schedule target expression"
                className="targetExpressionBar"
                onChange={setSelectorExpression}
                placeholder="id:edge-sfo-01 || provider:hetzner && country:US"
                showMatchCount
                value={selectorExpression}
                verification={selectorParse.error ? "invalid" : selectorExpression.trim() ? "valid" : "neutral"}
                verificationMessage={selectorParse.error ?? (selectorExpression.trim() ? `${selectedTargetCount}/${agents.length}` : "no selector")}
              />
            </div>
            <div className="schedulePreview">
              <strong>Next runs</strong>
              <span>{nextRuns.length ? "UTC schedule, displayed in browser timezone" : "Invalid or unsupported cron expression"}</span>
              <div className="targetChipList">
                {nextRuns.map((run) => (
                  <span className="targetChip" key={run}>
                    {formatCompactTime(run)}
                  </span>
                ))}
              </div>
              <small>
                {selectedTargetCount} targets; {selectedTemplate ? selectedTemplate.name : operationSummary(scheduleOperation)}
              </small>
            </div>
            {!confirmationOpen && (
              <button
                className="primaryAction"
                disabled={pending || !ready}
                type="submit"
              >
                <Save size={17} />
                {editingScheduleId ? "Update" : "Save"}
              </button>
            )}
            <ConfirmationPrompt
              confirmLabel={editingScheduleId ? "Update schedule" : "Save schedule"}
              detail={`Recurring ${selectedTemplate ? selectedTemplate.name : operationSummary(scheduleOperation)} on ${vpsCountLabel(selectedTargetCount)}.`}
              items={confirmationItems}
              onCancel={() => setConfirmationOpen(false)}
              onConfirm={() => void saveScheduleNow()}
              open={confirmationOpen}
              pending={pending}
              title={editingScheduleId ? "Confirm schedule update" : "Confirm schedule"}
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
  | { type: "delete"; schedule: ScheduleRecord }
  | { type: "defer"; schedule: ScheduleRecord; deferredUntil: string; reason: string };

function actionName(action: ScheduleAction): string {
  switch (action.type) {
    case "enable":
      return "schedule.enable";
    case "disable":
      return "schedule.disable";
    case "delete":
      return "schedule.delete";
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
    case "delete":
      return "Delete";
  }
}

function actionDetail(action: ScheduleAction): string {
  if (action.type === "applyNow") {
    return "Dispatches a normal job from the saved schedule without changing the next scheduled run.";
  }
  if (action.type === "defer") {
    return `Pauses automatic execution until ${formatCompactTime(action.deferredUntil)}.`;
  }
  return `${actionConfirmLabel(action.type)} ${action.schedule.name}.`;
}

function actionConfirmationItems(action: ScheduleAction) {
  const items = [
    { label: "Schedule", value: `${action.schedule.name} (${shortId(action.schedule.id)})` },
    { label: "Operation", value: action.schedule.command_type },
    { label: "Targets", value: action.schedule.selector_expression },
    { label: "State", value: action.schedule.enabled ? "enabled" : "disabled" },
  ];
  if (action.type === "defer") {
    items.push({ label: "Deferred until", value: action.deferredUntil });
    if (action.reason.trim()) {
      items.push({ label: "Reason", value: action.reason.trim() });
    }
  }
  return items;
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
    const dowMatches = dowAny || dowValues?.has(dow) || (dow === 0 && dowValues?.has(7));
    const domMatches = domAny || domValues?.has(dom);
    const dayMatches = domAny || dowAny ? domMatches && dowMatches : domMatches || dowMatches;
    if (months.has(month) && hours.has(hour) && minutes.has(minute) && dayMatches) {
      result.push(cursor.toISOString());
    }
    cursor.setUTCMinutes(cursor.getUTCMinutes() + 1);
  }
  return result;
}

function parseCronField(expr: string, min: number, max: number): Set<number> | null {
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
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < min || end > max || start > end) {
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
      return `backup ${operation.include_config ? "config" : "paths"}`;
    default:
      return operation.type;
  }
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function blurActiveElement() {
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
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
