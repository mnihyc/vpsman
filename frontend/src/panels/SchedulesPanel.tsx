import { useEffect, useMemo, useState, type FormEvent } from "react";
import { RefreshCcw, Save } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConsoleCollapsibleSection } from "../components/ConsoleLayout";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { parseCommandArgv } from "../proof";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../searchExpression";
import type {
  AgentView,
  CreateScheduleRequest,
  ScheduleRecord,
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
  error,
  loading,
  onCreateSchedule,
  onRefresh,
  schedules,
}: {
  activeSubpage: string;
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onCreateSchedule: (request: CreateScheduleRequest) => Promise<void>;
  onRefresh: () => Promise<void>;
  schedules: ScheduleRecord[];
}) {
  const [name, setName] = useState("");
  const [commandText, setCommandText] = useState("");
  const [intervalSecs, setIntervalSecs] = useState(3600);
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

  const argv = useMemo(() => {
    try {
      return parseCommandArgv(commandText);
    } catch {
      return [];
    }
  }, [commandText]);
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const selectedTargetCount = useMemo(
    () =>
      selectorParse.error
        ? 0
        : agentsMatchingExpression(agents, selectorExpression).length,
    [agents, selectorExpression, selectorParse.error],
  );
  const ready =
    name.trim().length > 0 &&
    argv.length > 0 &&
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
    { label: "Command", value: argv[0] ?? "-" },
    { label: "Interval", value: formatInterval(clampInteger(intervalSecs, 1, 31_536_000)) },
    { label: "Catch-up", value: formatCatchUpPolicy(catchUpPolicy) },
    { label: "Retry", value: formatInterval(clampInteger(retryDelaySecs, 1, 86_400)) },
    { label: "State", value: enabled ? "Enabled" : "Disabled" },
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
        id: "interval",
        header: "Interval",
        size: 90,
        minSize: 80,
        sortValue: (schedule) => schedule.interval_secs,
        searchValue: (schedule) => formatInterval(schedule.interval_secs),
        cell: (schedule) => formatInterval(schedule.interval_secs),
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
    [],
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
      if (!ready) {
        throw new Error("Schedule is incomplete");
      }
      await onCreateSchedule({
        name: name.trim(),
        operation: { type: "shell", argv, pty: false },
        selector_expression: selectorExpression.trim(),
        interval_secs: clampInteger(intervalSecs, 1, 31_536_000),
        start_at_unix: null,
        enabled,
        catch_up_policy: catchUpPolicy,
        catch_up_limit: clampInteger(catchUpLimit, 1, 25),
        retry_delay_secs: clampInteger(retryDelaySecs, 1, 86_400),
        max_failures: clampInteger(maxFailures, 1, 100),
      });
      setName("");
      setCommandText("");
    });
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
      </section>

      <section className="scheduleComposer">
        <ConsoleCollapsibleSection
          defaultOpen={false}
          storageKey="vpsman.panel.schedules.create"
          summary={`${selectedTargetCount} selected targets`}
          title="Create schedule"
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
              <span>Command argv</span>
              <textarea
                aria-label="Schedule command argv"
                onChange={(event) => setCommandText(event.target.value)}
                rows={3}
                value={commandText}
              />
            </label>
            <div className="dispatchControls">
              <label>
                <span>Interval</span>
                <input
                  aria-label="Schedule interval seconds"
                  min={1}
                  max={31_536_000}
                  onChange={(event) =>
                    setIntervalSecs(Number(event.target.value))
                  }
                  type="number"
                  value={intervalSecs}
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
                verificationMessage={selectorParse.error ?? `${selectedTargetCount}/${agents.length}`}
              />
            </div>
            {!confirmationOpen && (
              <button
                className="primaryAction"
                disabled={pending || !ready}
                type="submit"
              >
                <Save size={17} />
                Save
              </button>
            )}
            <ConfirmationPrompt
              confirmLabel="Save schedule"
              detail={`Recurring ${argv[0] ?? "command"} on ${vpsCountLabel(selectedTargetCount)}.`}
              items={confirmationItems}
              onCancel={() => setConfirmationOpen(false)}
              onConfirm={() => void saveScheduleNow()}
              open={confirmationOpen}
              pending={pending}
              title="Confirm schedule"
            />
          </form>
        </ConsoleCollapsibleSection>
      </section>
    </div>
  );
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
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

function formatCatchUpPolicy(policy: string): string {
  if (policy === "run_all_limited") {
    return "limited backlog";
  }
  if (policy === "run_once") {
    return "one missed";
  }
  return "skip missed";
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
