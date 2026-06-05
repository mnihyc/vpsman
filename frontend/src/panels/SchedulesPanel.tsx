import { useMemo, useState, type FormEvent } from "react";
import { CalendarClock, RefreshCcw, Save, Server, Tag } from "lucide-react";
import { CrudPager } from "../components/CrudPager";
import { usePanelDisplaySettings } from "../panelDisplay";
import { parseCommandArgv } from "../proof";
import type {
  AgentView,
  CreateScheduleRequest,
  ScheduleRecord,
  TagView,
} from "../types";
import { formatTime, formatVpsName, runPanelAction, shortId, toggleValue } from "../utils";

export function SchedulesPanel({
  agents,
  error,
  loading,
  onCreateSchedule,
  onRefresh,
  schedules,
  tags,
}: {
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onCreateSchedule: (request: CreateScheduleRequest) => Promise<void>;
  onRefresh: () => Promise<void>;
  schedules: ScheduleRecord[];
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [name, setName] = useState("");
  const [commandText, setCommandText] = useState("");
  const [intervalSecs, setIntervalSecs] = useState(3600);
  const [enabled, setEnabled] = useState(true);
  const [catchUpPolicy, setCatchUpPolicy] = useState("skip_missed");
  const [catchUpLimit, setCatchUpLimit] = useState(1);
  const [retryDelaySecs, setRetryDelaySecs] = useState(300);
  const [maxFailures, setMaxFailures] = useState(3);
  const [selectedClients, setSelectedClients] = useState<string[]>([]);
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const argv = useMemo(() => {
    try {
      return parseCommandArgv(commandText);
    } catch {
      return [];
    }
  }, [commandText]);
  const selectedTargetCount = selectedClients.length + selectedTags.length;
  const ready = name.trim().length > 0 && argv.length > 0 && selectedTargetCount > 0;
  const status = actionError ?? error ?? (loading ? "Loading" : `${schedules.length} schedules`);

  async function submitSchedule(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!ready) {
        throw new Error("Schedule is incomplete");
      }
      await onCreateSchedule({
        name: name.trim(),
        operation: { type: "shell", argv, pty: false },
        clients: selectedClients,
        tags: selectedTags,
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

  return (
    <div className="workspaceGrid">
      <section className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Schedules</h2>
            <span>{status}</span>
          </div>
          <button className="secondaryAction" disabled={loading || pending} onClick={onRefresh} type="button">
            <RefreshCcw size={17} />
            Refresh
          </button>
        </div>
        <CrudPager
          fields={[
            { label: "Name", value: (schedule) => `${schedule.name} ${schedule.id}` },
            { label: "Operation", value: (schedule) => schedule.command_type },
            { label: "Targets", value: (schedule) => [...schedule.clients, ...schedule.tags].join(" ") },
            { label: "Policy", value: (schedule) => schedule.catch_up_policy },
            { label: "State", value: (schedule) => `${schedule.enabled ? "enabled" : "disabled"} ${schedule.last_error ?? ""}` },
          ]}
          itemLabel="schedules"
          items={schedules}
          pageSize={10}
          title="Schedule records"
          empty={<div className="emptyState compactEmpty">No schedules match the current search.</div>}
        >
          {(scheduleRows) => (
            <div className="tableWrap">
              <table>
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Operation</th>
                    <th>Targets</th>
                    <th>Interval</th>
                    <th>Policy</th>
                    <th>Next run</th>
                    <th>State</th>
                  </tr>
                </thead>
                <tbody>
                  {scheduleRows.map((schedule) => (
                    <tr key={schedule.id}>
                      <td>
                        <strong>{schedule.name}</strong>
                        <span className="mutedCell">{shortId(schedule.id)}</span>
                      </td>
                      <td>{schedule.command_type}</td>
                      <td>{schedule.clients.length + schedule.tags.length}</td>
                      <td>{formatInterval(schedule.interval_secs)}</td>
                      <td>
                        <strong>{formatCatchUpPolicy(schedule.catch_up_policy)}</strong>
                        <span className="mutedCell">
                          {schedule.catch_up_policy === "run_all_limited"
                            ? `limit ${schedule.catch_up_limit}`
                            : `retry ${formatInterval(schedule.retry_delay_secs)}`}
                        </span>
                      </td>
                      <td>{formatTime(schedule.next_run_at)}</td>
                      <td>
                        <span className={schedule.enabled ? "status ok" : "status neutral"}>
                          {schedule.enabled ? "enabled" : "disabled"}
                        </span>
                        {schedule.failure_count > 0 && (
                          <span className="mutedCell">
                            {schedule.failure_count}/{schedule.max_failures} failures
                          </span>
                        )}
                        {schedule.last_error && <span className="mutedCell">{schedule.last_error}</span>}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CrudPager>
      </section>

      <section className="fleetPanel scheduleComposer">
        <div className="sectionHeader">
          <div>
            <h2>Create schedule</h2>
            <span>{selectedTargetCount} selected targets</span>
          </div>
          <CalendarClock size={20} />
        </div>
        <form className="dispatchForm" onSubmit={submitSchedule}>
          <label>
            <span>Name</span>
            <input aria-label="Schedule name" onChange={(event) => setName(event.target.value)} value={name} />
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
                onChange={(event) => setIntervalSecs(Number(event.target.value))}
                type="number"
                value={intervalSecs}
              />
            </label>
            <label className="checkLine inlineCheck">
              <input checked={enabled} onChange={(event) => setEnabled(event.target.checked)} type="checkbox" />
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
                onChange={(event) => setCatchUpLimit(Number(event.target.value))}
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
                onChange={(event) => setRetryDelaySecs(Number(event.target.value))}
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
                onChange={(event) => setMaxFailures(Number(event.target.value))}
                type="number"
                value={maxFailures}
              />
            </label>
          </div>
          <div className="targetSelector">
            <strong>Targets</strong>
            <div className="chipList">
              {agents.map((agent) => (
                <label className="checkChip" key={agent.id}>
                  <input
                    checked={selectedClients.includes(agent.id)}
                    onChange={() => setSelectedClients(toggleValue(selectedClients, agent.id))}
                    type="checkbox"
                  />
                  <Server size={14} />
                  <span>{formatVpsName(agent, vpsNameDisplayMode)}</span>
                </label>
              ))}
            {tags.map((tag) => (
                <label className="checkChip" key={tag.name}>
                  <input
                    checked={selectedTags.includes(tag.name)}
                    onChange={() => setSelectedTags(toggleValue(selectedTags, tag.name))}
                    type="checkbox"
                  />
                  <Tag size={14} />
                  <span>{tag.name}</span>
                </label>
              ))}
            </div>
          </div>
          <button className="primaryAction" disabled={pending || !ready} type="submit">
            <Save size={17} />
            Save
          </button>
        </form>
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
