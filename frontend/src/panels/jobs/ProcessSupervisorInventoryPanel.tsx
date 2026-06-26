import { useMemo } from "react";
import { Activity, FileText, RefreshCw, RotateCcw, Square, TerminalSquare } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import type { JobDispatchPresetInput } from "../../jobDispatchPreset";
import type { SupervisorAction } from "../jobDispatchModel";
import type { ProcessSupervisorInventoryRecord } from "../../types";
import { formatTime, shortId, statusClass } from "../../utils";

export function ProcessSupervisorInventoryPanel({
  clientLabel,
  inventory,
  loading,
  onOpenDispatchPreset,
  onOpenProcessMetrics,
  onRefresh,
}: {
  clientLabel: (clientId: string) => string;
  inventory: ProcessSupervisorInventoryRecord[];
  loading: boolean;
  onOpenDispatchPreset: (preset: JobDispatchPresetInput) => void;
  onOpenProcessMetrics: () => void;
  onRefresh: () => void;
}) {
  const runningCount = inventory.filter((row) => row.status === "running").length;
  const desiredOnlyLimitCount = inventory.filter((row) => row.limit_effectiveness_status === "degraded_desired_only").length;
  const restartedCount = inventory.filter((row) => (row.restart_attempts ?? 0) > 0).length;
  const logBackedCount = inventory.filter((row) => row.stdout_log || row.stderr_log).length;
  const retainedMemoryBytes = inventory.reduce((total, row) => total + (row.cgroup_memory_current_bytes ?? 0), 0);
  const columns = useMemo<ConsoleDataGridColumn<ProcessSupervisorInventoryRecord>[]>(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.name}</strong>
            <small>{clientLabel(row.client_id)} / {formatPid(row)}</small>
          </span>
        ),
        header: "Process",
        id: "process",
        searchValue: (row) => `${row.name} ${clientLabel(row.client_id)} ${row.client_id} ${formatPid(row)}`,
        sortValue: (row) => row.name,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <span className={`status ${statusClass(row.status)}`}>{row.status}</span>
            <small>{formatHealthEvidence(row)}</small>
          </span>
        ),
        header: "Health",
        id: "health",
        searchValue: (row) => `${row.status} ${formatHealthEvidence(row)} ${formatRestartEvidence(row)}`,
        sortValue: (row) => row.status,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatResourcePrimary(row)}</strong>
            <small>{formatResourceSecondary(row)}</small>
          </span>
        ),
        header: "Resources",
        id: "resources",
        searchValue: (row) => `${formatResourcePrimary(row)} ${formatResourceSecondary(row)} ${formatProcessRuntime(row)}`,
        sortValue: (row) => row.cgroup_memory_current_bytes ?? row.pid ?? 0,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatSourceCommand(row.source_command_type)}</strong>
            <small>Source job {shortId(row.source_job_id)}</small>
          </span>
        ),
        header: "Source",
        id: "source",
        searchValue: (row) => `${formatSourceCommand(row.source_command_type)} ${row.source_command_type} ${row.source_job_id}`,
        sortValue: (row) => row.source_command_type,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatLogsPrimary(row)}</strong>
            <small>{formatLogsSecondary(row)}</small>
          </span>
        ),
        header: "Logs",
        id: "logs",
        searchValue: (row) => `${formatLogsPrimary(row)} ${formatLogsSecondary(row)} ${row.stdout_log ?? ""} ${row.stderr_log ?? ""}`,
        sortValue: (row) => Number(Boolean(row.stdout_log)) + Number(Boolean(row.stderr_log)),
      },
      {
        cell: (row) => formatUnixTime(row.started_unix),
        header: "Started",
        id: "started",
        searchValue: (row) => formatUnixTime(row.started_unix),
        sortValue: (row) => row.started_unix ?? 0,
      },
      {
        cell: (row) => formatTime(row.observed_at),
        header: "Observed",
        id: "observed",
        searchValue: (row) => formatTime(row.observed_at),
        sortValue: (row) => row.observed_at,
      },
      {
        cell: (row) => (
          <span className="processRowActions" aria-label={`Process ${row.name} actions`}>
            <button
              aria-label={`Review logs for process ${row.name}`}
              className="processActionButton"
              onClick={(event) => {
                event.stopPropagation();
                onOpenDispatchPreset(supervisorPreset(row, "logs"));
              }}
              title="Prepare reviewed process logs dispatch for this VPS and process"
              type="button"
            >
              <FileText size={13} />
              <span>Logs</span>
            </button>
            <button
              aria-label={`Review restart for process ${row.name}`}
              className="processActionButton"
              onClick={(event) => {
                event.stopPropagation();
                onOpenDispatchPreset(supervisorPreset(row, "restart"));
              }}
              title="Prepare privileged restart review for this VPS and process"
              type="button"
            >
              <RotateCcw size={13} />
              <span>Restart</span>
            </button>
            <button
              aria-label={`Review stop for process ${row.name}`}
              className="processActionButton dangerAction"
              disabled={row.status !== "running"}
              onClick={(event) => {
                event.stopPropagation();
                onOpenDispatchPreset(supervisorPreset(row, "stop"));
              }}
              title={
                row.status === "running"
                  ? "Prepare privileged stop review for this VPS and process"
                  : "Stop is available when the process is running"
              }
              type="button"
            >
              <Square size={12} />
              <span>Stop</span>
            </button>
          </span>
        ),
        enableHiding: false,
        header: "Actions",
        id: "actions",
        minSize: 240,
        size: 260,
      },
    ],
    [clientLabel, onOpenDispatchPreset],
  );

  return (
    <div className="fleetPanel">
      <div className="sectionHeader">
        <div>
          <h2>Process supervisor inventory</h2>
          <span>
            {runningCount} running, {desiredOnlyLimitCount} desired-only limits, {restartedCount} restarted
          </span>
        </div>
        <div className="processHeaderActions">
          <button className="secondaryAction" onClick={onOpenProcessMetrics} type="button">
            <Activity size={14} />
            <span>Open process metrics</span>
          </button>
          <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
            <RefreshCw size={14} />
            <span>Refresh</span>
          </button>
        </div>
      </div>
      <div className="processSupervisorSummaryStrip" aria-label="Process supervisor health summary">
        <span>
          <strong>{runningCount} / {inventory.length}</strong>
          <small>Running processes</small>
        </span>
        <span className={desiredOnlyLimitCount > 0 ? "attention" : undefined}>
          <strong>{desiredOnlyLimitCount}</strong>
          <small>Desired-only limits</small>
        </span>
        <span className={restartedCount > 0 ? "attention" : undefined}>
          <strong>{restartedCount}</strong>
          <small>Processes restarted</small>
        </span>
        <span>
          <strong>{formatBytes(retainedMemoryBytes)}</strong>
          <small>Observed cgroup memory</small>
        </span>
        <span>
          <strong>{logBackedCount}</strong>
          <small>With log paths</small>
        </span>
      </div>
      <ConsoleDataGrid
        columns={columns}
        defaultPageSize={10}
        expandOnRowClick
        getRowId={(row) => `${row.client_id}:${row.name}`}
        itemLabel="processes"
        empty={
          <div className="emptyState">
            <TerminalSquare size={22} />
            <strong>No supervised processes</strong>
            <span>Process start, status, restart, log, and stop jobs populate this inventory.</span>
          </div>
        }
        renderExpandedRow={(row) => (
          <div className="consoleInlineDetailGrid">
            <span>Process</span>
            <strong>{row.name}</strong>
            <span>VPS</span>
            <strong>{clientLabel(row.client_id)}</strong>
            <span>PID</span>
            <strong>{row.pid ?? "Not reported"}</strong>
            <span>Status</span>
            <strong>{row.status}</strong>
            <span>Health summary</span>
            <strong>{formatHealthEvidence(row)}</strong>
            <span>Source command</span>
            <strong>{formatSourceCommand(row.source_command_type)}</strong>
            <span>Source job</span>
            <strong>{row.source_job_id}</strong>
            <span>Runtime evidence</span>
            <strong>{formatProcessRuntime(row)}</strong>
            <span>Resource snapshot</span>
            <strong>{formatResourceSecondary(row)}</strong>
            <span>Restart attempts</span>
            <strong>{row.restart_attempts ?? 0}</strong>
            <span>Last exit</span>
            <strong>{formatLastExitEvidence(row)}</strong>
            <span>Last restart</span>
            <strong>{formatUnixTime(row.last_restart_unix)}</strong>
            <span>Process exit code</span>
            <strong>{row.process_exit_code ?? "No exit reported"}</strong>
            <span>stdout log</span>
            <strong title={row.stdout_log ?? undefined}>{row.stdout_log ?? "Not reported"}</strong>
            <span>stderr log</span>
            <strong title={row.stderr_log ?? undefined}>{row.stderr_log ?? "Not reported"}</strong>
            <span>Supervisor config</span>
            <strong>Not reported by process supervisor API</strong>
            <span>Resource history</span>
            <strong>Long-term CPU, memory, restart history, and recent exits belong in Observability / Process Metrics when backend time series exist</strong>
            <span>Row actions</span>
            <strong>Logs, Restart, and Stop prepare reviewed Dispatch / Supervisor jobs with this VPS and process scope</strong>
            <span>Related jobs and alerts</span>
            <strong>Source job is linked by ID; alert links are not reported by process supervisor API</strong>
            <span>Started</span>
            <strong>{formatUnixTime(row.started_unix)}</strong>
            <span>Observed</span>
            <strong>{formatTime(row.observed_at)}</strong>
          </div>
        )}
        rows={inventory}
        searchPlaceholder="Search processes"
        selectable={false}
        storageKey="vpsman.jobs.processSupervisorInventory"
        title="Process health inventory"
      />
    </div>
  );
}

function supervisorPreset(
  row: ProcessSupervisorInventoryRecord,
  action: Extract<SupervisorAction, "logs" | "restart" | "stop">,
): JobDispatchPresetInput {
  return {
    maxTimeoutSecs: action === "logs" ? 30 : 60,
    mode: "process_supervisor",
    selectorExpression: `id:${row.client_id}`,
    supervisorAction: action,
    supervisorLogBytes: action === "logs" ? 65536 : undefined,
    supervisorName: row.name,
  };
}

function formatUnixTime(value: number | null): string {
  if (!value) {
    return "Not reported";
  }
  return new Date(value * 1000).toLocaleString();
}

function formatRestartEvidence(row: ProcessSupervisorInventoryRecord): string {
  const attempts = row.restart_attempts ?? 0;
  if (row.last_exit_code !== null) {
    return `${formatRestartCount(attempts)}; last exit code ${row.last_exit_code}`;
  }
  return attempts > 0 ? formatRestartCount(attempts) : "No restarts recorded";
}

function formatHealthEvidence(row: ProcessSupervisorInventoryRecord): string {
  const limit = formatLimitEvidence(row.limit_effectiveness_status);
  const restart = formatRestartEvidence(row);
  return `${limit}; ${restart}`;
}

function formatRestartCount(attempts: number): string {
  return attempts === 1 ? "Restarted 1 time" : `Restarted ${attempts} times`;
}

function formatProcessRuntime(row: ProcessSupervisorInventoryRecord): string {
  const limit = formatLimitEvidence(row.limit_effectiveness_status);
  const cgroup = formatCgroupEvidence(row);
  return cgroup ? `${limit}; ${cgroup}` : limit;
}

function formatLimitEvidence(status: string | null): string {
  if (status === "degraded_desired_only") {
    return "Limits desired only";
  }
  if (status === "enforced") {
    return "Limits enforced";
  }
  if (status === "enforced_or_not_requested") {
    return "Limits enforced or not requested";
  }
  return "Limit state not reported";
}

function formatCgroupEvidence(row: ProcessSupervisorInventoryRecord): string | null {
  if (row.cgroup_status === "available") {
    const parts = [];
    if (row.cgroup_process_count !== null) {
      parts.push(`${row.cgroup_process_count} processes`);
    }
    if (row.cgroup_cpu_weight !== null) {
      parts.push(`CPU weight ${row.cgroup_cpu_weight}`);
    }
    if (row.cgroup_memory_current_bytes !== null) {
      parts.push(`${formatBytes(row.cgroup_memory_current_bytes)} memory`);
    }
    if (row.cgroup_pids_current !== null) {
      parts.push(`${row.cgroup_pids_current} PIDs`);
    }
    return parts.length > 0 ? parts.join(", ") : "cgroup available";
  }
  if (row.cgroup_status === "missing") {
    return "Cgroup missing";
  }
  return null;
}

function formatPid(row: ProcessSupervisorInventoryRecord): string {
  return row.pid === null ? "PID not reported" : `PID ${row.pid}`;
}

function formatResourcePrimary(row: ProcessSupervisorInventoryRecord): string {
  if (row.cgroup_process_count !== null || row.cgroup_pids_current !== null) {
    const processes = row.cgroup_process_count !== null ? `${row.cgroup_process_count} processes` : "processes n/a";
    const pids = row.cgroup_pids_current !== null ? `${row.cgroup_pids_current} PIDs` : "PIDs n/a";
    return `${processes}, ${pids}`;
  }
  return formatPid(row);
}

function formatResourceSecondary(row: ProcessSupervisorInventoryRecord): string {
  const parts = [
    row.cgroup_cpu_weight !== null ? `CPU weight ${row.cgroup_cpu_weight}` : "CPU weight n/a",
    row.cgroup_memory_current_bytes !== null ? `${formatBytes(row.cgroup_memory_current_bytes)} memory` : "Memory n/a",
    row.cgroup_status ? `cgroup ${row.cgroup_status.replace(/_/g, " ")}` : "cgroup n/a",
  ];
  return parts.join("; ");
}

function formatSourceCommand(value: string): string {
  const labels: Record<string, string> = {
    process_logs: "Log snapshot",
    process_restart: "Restart command",
    process_start: "Start command",
    process_status: "Status snapshot",
    process_stop: "Stop command",
  };
  return labels[value] ?? value.replace(/_/g, " ");
}

function formatLogsPrimary(row: ProcessSupervisorInventoryRecord): string {
  if (row.stdout_log && row.stderr_log) {
    return "stdout + stderr logs";
  }
  if (row.stdout_log) {
    return "stdout log";
  }
  if (row.stderr_log) {
    return "stderr log";
  }
  return "No log paths";
}

function formatLogsSecondary(row: ProcessSupervisorInventoryRecord): string {
  if (row.stdout_log || row.stderr_log) {
    return "Request contents through Dispatch > Supervisor > Logs";
  }
  return "Log paths not reported";
}

function formatLastExitEvidence(row: ProcessSupervisorInventoryRecord): string {
  if (row.last_exit_code === null && row.last_exit_unix === null) {
    return "No exit reported";
  }
  const code = row.last_exit_code === null ? "code not reported" : `code ${row.last_exit_code}`;
  return `${code}; ${formatUnixTime(row.last_exit_unix)}`;
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
  }
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${value} B`;
}
