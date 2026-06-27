import { useCallback, useMemo, useState } from "react";
import { FileText, RefreshCw, RotateCcw, Square, TerminalSquare } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE,
  JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
} from "../../generated/protocolContracts";
import type { JobDispatchPresetInput } from "../../jobDispatchPreset";
import {
  buildPrivilegeForJobOperation,
  type PrivilegeMaterial,
} from "../../privilege";
import type { SupervisorAction } from "../jobDispatchModel";
import type {
  CreateJobRequest,
  CreateJobResponse,
  JobOperation,
  ProcessSupervisorInventoryRecord,
} from "../../types";
import { formatCompactTime, formatFullTime, statusClass } from "../../utils";

export function ProcessSupervisorInventoryPanel({
  clientLabel,
  inventory,
  loading,
  onCreateJob,
  onOpenDispatchPreset,
  onOpenPrivilegeUnlock,
  onRefresh,
  privilegeMaterial,
}: {
  clientLabel: (clientId: string) => string;
  inventory: ProcessSupervisorInventoryRecord[];
  loading: boolean;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onOpenDispatchPreset: (preset: JobDispatchPresetInput) => void;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [stopProcess, setStopProcess] =
    useState<ProcessSupervisorInventoryRecord | null>(null);
  const runningCount = inventory.filter((row) => row.status === "running").length;
  const desiredOnlyLimitCount = inventory.filter((row) => row.limit_effectiveness_status === "degraded_desired_only").length;
  const restartedCount = inventory.filter((row) => (row.restart_attempts ?? 0) > 0).length;
  const logBackedCount = inventory.filter((row) => row.stdout_log || row.stderr_log).length;
  const chronologyWarningCount = inventory.filter((row) => processTimingEvidence(row).tone === "warn").length;
  const retainedMemoryBytes = inventory.reduce((total, row) => total + (row.cgroup_memory_current_bytes ?? 0), 0);
  const executeProcessAction = useCallback(
    async (
      row: ProcessSupervisorInventoryRecord,
      action: Extract<SupervisorAction, "restart" | "stop">,
    ) => {
      if (!privilegeMaterial) {
        setActionError("Unlock privilege before restarting or stopping a supervised process.");
        onOpenPrivilegeUnlock();
        return;
      }
      const operation = processOperation(row, action);
      const commandType = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
      const selectorExpression = `id:${row.client_id}`;
      const maxTimeoutSecs = 60;
      setActionError(null);
      setActionPending(true);
      try {
        const { privilegeAssertion } = await buildPrivilegeForJobOperation({
          clientIds: [row.client_id],
          commandType,
          operation,
          privilegeMaterial,
          selectorExpression,
          maxTimeoutSecs,
        });
        await onCreateJob({
          job_id: crypto.randomUUID(),
          selector_expression: selectorExpression,
          target_client_ids: [row.client_id],
          destructive: Boolean(
            JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE[operation.type],
          ),
          confirmed: true,
          command: commandType,
          argv: [],
          operation,
          max_timeout_secs: maxTimeoutSecs,
          privileged: true,
          privilege_assertion: privilegeAssertion,
        });
        setStopProcess(null);
        onRefresh();
      } catch (error) {
        setActionError(
          error instanceof Error
            ? error.message
            : `Could not ${action} ${row.name}`,
        );
      } finally {
        setActionPending(false);
      }
    },
    [onCreateJob, onOpenPrivilegeUnlock, onRefresh, privilegeMaterial],
  );
  const renderProcessActions = useCallback(
    (row: ProcessSupervisorInventoryRecord) => (
      <span className="processRowActions" aria-label={`Process ${row.name} actions`}>
        <button
          aria-label={`Open logs for process ${row.name}`}
          className="processActionButton"
          onClick={(event) => {
            event.stopPropagation();
            onOpenDispatchPreset(supervisorPreset(row, "logs"));
          }}
          title="Open Dispatch with this VPS and process preselected for a log read"
          type="button"
        >
          <FileText size={13} />
          <span>Logs</span>
        </button>
        <button
          aria-label={`Restart process ${row.name}`}
          className="processActionButton"
          disabled={actionPending}
          onClick={(event) => {
            event.stopPropagation();
            void executeProcessAction(row, "restart");
          }}
          title={
            privilegeMaterial
              ? "Restart immediately using the unlocked process privilege"
              : "Unlock privilege before restarting this process"
          }
          type="button"
        >
          <RotateCcw size={13} />
          <span>Restart</span>
        </button>
        <button
          aria-label={`Stop process ${row.name}`}
          className="processActionButton dangerAction"
          disabled={row.status !== "running" || actionPending}
          onClick={(event) => {
            event.stopPropagation();
            setActionError(null);
            setStopProcess(row);
          }}
          title={
            row.status === "running"
              ? "Review one confirmation before stopping this process"
              : "Stop is available when the process is running"
          }
          type="button"
        >
          <Square size={12} />
          <span>Stop</span>
        </button>
      </span>
    ),
    [actionPending, executeProcessAction, onOpenDispatchPreset, privilegeMaterial],
  );
  const columns = useMemo<ConsoleDataGridColumn<ProcessSupervisorInventoryRecord>[]>(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong title={row.name}>{row.name}</strong>
            <small>{formatPid(row)}</small>
          </span>
        ),
        header: "Process",
        id: "process",
        minSize: 100,
        searchValue: (row) => `${row.name} ${clientLabel(row.client_id)} ${row.client_id} ${formatPid(row)}`,
        size: 110,
        sortValue: (row) => row.name,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong title={clientLabel(row.client_id)}>{clientLabel(row.client_id)}</strong>
            <small>{row.client_id}</small>
          </span>
        ),
        header: "VPS",
        id: "vps",
        minSize: 112,
        searchValue: (row) => `${clientLabel(row.client_id)} ${row.client_id}`,
        size: 120,
        sortValue: (row) => clientLabel(row.client_id),
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            {(() => {
              const state = processStateEvidence(row);
              return (
                <>
                  <span className={`status ${state.tone}`}>{state.label}</span>
                  <small title={state.detail}>{state.detail}</small>
                </>
              );
            })()}
          </span>
        ),
        header: "State",
        id: "state",
        minSize: 140,
        searchValue: (row) => `${processStateEvidence(row).label} ${processStateEvidence(row).detail} ${formatHealthEvidence(row)} ${formatRestartEvidence(row)}`,
        size: 148,
        sortValue: (row) => processStateEvidence(row).label,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatCpuPrimary(row)}</strong>
            <small>{formatCpuSecondary(row)}</small>
          </span>
        ),
        header: "CPU",
        id: "cpu",
        minSize: 72,
        searchValue: (row) => `${formatCpuPrimary(row)} ${formatCpuSecondary(row)} ${formatLimitEvidence(row.limit_effectiveness_status)}`,
        size: 84,
        sortValue: (row) => row.cgroup_cpu_weight ?? -1,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{formatMemoryPrimary(row)}</strong>
            <small>{formatProcessCardinality(row)}</small>
          </span>
        ),
        header: "Memory",
        id: "memory",
        minSize: 100,
        searchValue: (row) => `${formatMemoryPrimary(row)} ${formatProcessCardinality(row)} ${formatCgroupEvidence(row) ?? ""}`,
        size: 108,
        sortValue: (row) => row.cgroup_memory_current_bytes ?? -1,
      },
      {
        cell: (row) => {
          const timing = processTimingEvidence(row);
          return (
            <span className="historyPrimary">
              <strong>{timing.uptimeLabel}</strong>
              <small title={timing.detail}>{timing.detail}</small>
            </span>
          );
        },
        header: "Uptime",
        id: "uptime",
        minSize: 128,
        searchValue: (row) => `${processTimingEvidence(row).uptimeLabel} ${processTimingEvidence(row).detail}`,
        size: 138,
        sortValue: (row) => processTimingEvidence(row).uptimeMs ?? -1,
      },
      {
        cell: (row) => {
          const restart = processRestartEvidence(row);
          return (
            <span className="historyPrimary">
              <strong>{restart.primary}</strong>
              <small title={restart.detail}>{restart.detail}</small>
            </span>
          );
        },
        header: "Restarts",
        id: "restarts",
        minSize: 114,
        searchValue: (row) => `${processRestartEvidence(row).primary} ${processRestartEvidence(row).detail}`,
        size: 122,
        sortValue: (row) => row.restart_attempts ?? 0,
      },
      {
        cell: (row) => {
          const exit = processLastExitEvidence(row);
          return (
            <span className="historyPrimary">
              <strong>{exit.primary}</strong>
              <small title={exit.detail}>{exit.detail}</small>
            </span>
          );
        },
        header: "Last exit",
        id: "last_exit",
        minSize: 106,
        searchValue: (row) => `${processLastExitEvidence(row).primary} ${processLastExitEvidence(row).detail}`,
        size: 114,
        sortValue: (row) => row.last_exit_unix ?? -1,
      },
      {
        cell: (row) => renderProcessActions(row),
        enableHiding: false,
        header: "Actions",
        id: "actions",
        minSize: 172,
        size: 182,
      },
    ],
    [clientLabel, renderProcessActions],
  );

  return (
    <div className="fleetPanel">
      <div className="sectionHeader">
        <div>
          <h2>Process supervisor inventory</h2>
          <span>
            {countPhrase(runningCount, "running process", "running processes")}, {countPhrase(desiredOnlyLimitCount, "desired-only limit")}, {countPhrase(restartedCount, "process restarted", "processes restarted")}
          </span>
        </div>
        <div className="processHeaderActions">
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
          <small>{restartedCount === 1 ? "Process restarted" : "Processes restarted"}</small>
        </span>
        <span>
          <strong>{formatBytes(retainedMemoryBytes)}</strong>
          <small>Cgroup memory</small>
        </span>
        <span className={chronologyWarningCount > 0 ? "attention" : undefined}>
          <strong>{countPhrase(chronologyWarningCount, "warning")}</strong>
          <small>Chronology</small>
        </span>
        <span>
          <strong>{logBackedCount}</strong>
          <small>With log paths</small>
        </span>
      </div>
      {actionError && !stopProcess && (
        <div className="operationNote formSectionNote processActionNotice" role="status">
          <strong>Process action blocked</strong>
          <span>{actionError}</span>
        </div>
      )}
      <ConfirmationPrompt
        confirmLabel="Stop process"
        detail="Stop this supervised process on the selected VPS."
        error={actionError && stopProcess ? actionError : undefined}
        items={
          stopProcess
            ? [
                { label: "Process", value: stopProcess.name },
                { label: "VPS", value: clientLabel(stopProcess.client_id) },
                { label: "State", value: processStateEvidence(stopProcess).label },
                { label: "Effect", value: "Submit one privileged process_stop job" },
              ]
            : []
        }
        onCancel={() => {
          if (!actionPending) {
            setStopProcess(null);
            setActionError(null);
          }
        }}
        onConfirm={() => {
          if (stopProcess) {
            void executeProcessAction(stopProcess, "stop");
          }
        }}
        open={Boolean(stopProcess)}
        pending={actionPending}
        title="Confirm process stop"
        tone="danger"
      />
      <div className="processMobileList" aria-label="Process supervisor mobile cards">
        {inventory.length === 0 ? (
          <div className="emptyState compactEmpty">
            <TerminalSquare size={22} />
            <strong>No supervised processes</strong>
            <span>Process start, status, restart, log, and stop jobs populate this inventory.</span>
          </div>
        ) : (
          inventory.map((row) => {
            const state = processStateEvidence(row);
            const timing = processTimingEvidence(row);
            const restart = processRestartEvidence(row);
            const exit = processLastExitEvidence(row);
            return (
              <article className="processMobileCard" key={`${row.client_id}:${row.name}`}>
                <div className="processMobileCardHeader">
                  <span>
                    <strong title={row.name}>{row.name}</strong>
                    <small>{clientLabel(row.client_id)} / {formatPid(row)}</small>
                  </span>
                  <span className={`status ${state.tone}`}>{state.label}</span>
                </div>
                <dl className="processMobileMetricGrid">
                  <div>
                    <dt>CPU</dt>
                    <dd>{formatCpuPrimary(row)}</dd>
                  </div>
                  <div>
                    <dt>Memory</dt>
                    <dd>{formatMemoryPrimary(row)}</dd>
                  </div>
                  <div>
                    <dt>Uptime</dt>
                    <dd title={timing.detail}>{timing.uptimeLabel}</dd>
                  </div>
                  <div>
                    <dt>Restarts</dt>
                    <dd title={restart.detail}>{restart.primary}</dd>
                  </div>
                  <div>
                    <dt>Last exit</dt>
                    <dd title={exit.detail}>{exit.primary}</dd>
                  </div>
                </dl>
                <p className="processMobileDetail" title={state.detail}>{state.detail}</p>
                {renderProcessActions(row)}
              </article>
            );
          })
        )}
      </div>
      <div className="processInventoryGridShell">
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
              <strong className="processEvidenceValue" title={row.name}>{row.name}</strong>
              <span>VPS</span>
              <strong>{clientLabel(row.client_id)}</strong>
              <span>PID</span>
              <strong>{row.pid ?? "Not reported"}</strong>
              <span>State</span>
              <strong>{processStateEvidence(row).label}</strong>
              <span>State detail</span>
              <strong>{processStateEvidence(row).detail}</strong>
              <span>Source command</span>
              <strong>{formatSourceCommand(row.source_command_type)}</strong>
              <span>Raw source job ID</span>
              <strong className="processEvidenceValue" title={row.source_job_id}>{row.source_job_id}</strong>
              <span>Uptime</span>
              <strong>{processTimingEvidence(row).uptimeLabel}</strong>
              <span>Timeline detail</span>
              <strong>{processTimingEvidence(row).detail}</strong>
              <span>Resource snapshot</span>
              <strong>{formatResourceSecondary(row)}</strong>
              <span>Restart attempts</span>
              <strong>{processRestartEvidence(row).primary}</strong>
              <span>Last exit</span>
              <strong>{processLastExitEvidence(row).primary}; {processLastExitEvidence(row).detail}</strong>
              <span>Last restart</span>
              <strong>{processRestartEvidence(row).detail}</strong>
              <span>Process exit code</span>
              <strong>{row.process_exit_code ?? "No exit reported"}</strong>
              <span>stdout log</span>
              <strong className="processEvidenceValue" title={row.stdout_log ?? undefined}>{row.stdout_log ?? "Not reported"}</strong>
              <span>stderr log</span>
              <strong className="processEvidenceValue" title={row.stderr_log ?? undefined}>{row.stderr_log ?? "Not reported"}</strong>
              <span>Supervisor config</span>
              <strong>Not reported by process supervisor API</strong>
              <span>Resource history</span>
              <strong>Not available yet; backend process time series for CPU, memory, restart history, and recent exits are not exposed.</strong>
              <span>Row actions</span>
              <strong>Logs open Dispatch for retained output. Restart submits directly after privilege unlock. Stop uses one confirmation on this page.</strong>
              <span>Related jobs and alerts</span>
              <strong>Source job is linked by raw ID in this detail view; alert links are not reported by the process supervisor API.</strong>
              <span>Started</span>
              <strong>{formatUnixTime(row.started_unix)}</strong>
              <span>Observed</span>
              <strong>{formatObservedTime(row.observed_at)}</strong>
            </div>
          )}
          rows={inventory}
          searchPlaceholder="Search processes"
          selectable={false}
          storageKey="vpsman.jobs.processSupervisorInventory"
          title="Process health inventory"
        />
      </div>
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

function processOperation(
  row: ProcessSupervisorInventoryRecord,
  action: Extract<SupervisorAction, "restart" | "stop">,
): Extract<JobOperation, { type: "process_restart" | "process_stop" }> {
  if (action === "restart") {
    return { name: row.name, type: "process_restart" };
  }
  return { name: row.name, type: "process_stop" };
}

function formatUnixTime(value: number | null): string {
  if (!value) {
    return "Not reported";
  }
  return formatFullTime(new Date(value * 1000).toISOString());
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

function processStateEvidence(row: ProcessSupervisorInventoryRecord): {
  detail: string;
  label: string;
  tone: string;
} {
  const timing = processTimingEvidence(row);
  if (timing.tone === "warn") {
    return {
      detail: `Backend state ${displayToken(row.status)}; ${timing.issueLabel ?? timing.detail}`,
      label: "Timeline inconsistent",
      tone: "warn",
    };
  }
  return {
    detail: formatHealthEvidence(row),
    label: displayToken(row.status),
    tone: statusClass(row.status),
  };
}

function processTimingEvidence(row: ProcessSupervisorInventoryRecord): {
  detail: string;
  issueLabel?: string;
  observedLabel: string;
  tone: "neutral" | "warn";
  uptimeLabel: string;
  uptimeMs: number | null;
} {
  const observedMs = parseIsoMs(row.observed_at);
  const startedMs = unixMs(row.started_unix);
  if (observedMs === null) {
    return {
      detail: "Observed timestamp is invalid; uptime unknown",
      issueLabel: "observed timestamp invalid",
      observedLabel: "Observed unknown",
      tone: "warn",
      uptimeLabel: "Unknown",
      uptimeMs: null,
    };
  }
  const observedLabel = `Observed ${formatCompactTime(row.observed_at)}`;
  if (startedMs === null) {
    return {
      detail: `${observedLabel}; started time not reported`,
      observedLabel,
      tone: "neutral",
      uptimeLabel: "Unknown",
      uptimeMs: null,
    };
  }
  if (startedMs > observedMs) {
    return {
      detail: `${observedLabel}; started after observed`,
      issueLabel: "started after observed",
      observedLabel,
      tone: "warn",
      uptimeLabel: "Unknown",
      uptimeMs: null,
    };
  }
  const uptimeMs = observedMs - startedMs;
  return {
    detail: `${observedLabel}; started ${formatCompactTime(new Date(startedMs).toISOString())}`,
    observedLabel,
    tone: "neutral",
    uptimeLabel: formatDurationMs(uptimeMs),
    uptimeMs,
  };
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

function formatCpuPrimary(row: ProcessSupervisorInventoryRecord): string {
  return row.cgroup_cpu_weight !== null ? String(row.cgroup_cpu_weight) : "Not reported";
}

function formatCpuSecondary(row: ProcessSupervisorInventoryRecord): string {
  return row.cgroup_cpu_weight !== null
    ? `CPU weight; ${formatLimitEvidence(row.limit_effectiveness_status)}`
    : formatLimitEvidence(row.limit_effectiveness_status);
}

function formatMemoryPrimary(row: ProcessSupervisorInventoryRecord): string {
  return row.cgroup_memory_current_bytes !== null ? formatBytes(row.cgroup_memory_current_bytes) : "Not reported";
}

function formatProcessCardinality(row: ProcessSupervisorInventoryRecord): string {
  if (row.cgroup_process_count !== null || row.cgroup_pids_current !== null) {
    const processes = row.cgroup_process_count !== null ? countPhrase(row.cgroup_process_count, "process", "processes") : "processes n/a";
    const pids = row.cgroup_pids_current !== null ? `${row.cgroup_pids_current} ${row.cgroup_pids_current === 1 ? "PID" : "PIDs"}` : "PIDs n/a";
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

function processRestartEvidence(row: ProcessSupervisorInventoryRecord): {
  detail: string;
  primary: string;
} {
  const attempts = row.restart_attempts ?? 0;
  const lastRestartMs = unixMs(row.last_restart_unix);
  const observedMs = parseIsoMs(row.observed_at);
  if (lastRestartMs === null) {
    return {
      detail: "Last restart not reported",
      primary: countPhrase(attempts, "restart"),
    };
  }
  if (observedMs !== null && lastRestartMs > observedMs) {
    return {
      detail: "Time unknown; after observed",
      primary: countPhrase(attempts, "restart"),
    };
  }
  const restartIso = new Date(lastRestartMs).toISOString();
  return {
    detail: `Last restart ${formatCompactTime(restartIso)}`,
    primary: countPhrase(attempts, "restart"),
  };
}

function processLastExitEvidence(row: ProcessSupervisorInventoryRecord): {
  detail: string;
  primary: string;
} {
  if (row.last_exit_code === null && row.last_exit_unix === null) {
    return {
      detail: "No exit timestamp reported",
      primary: "No exit",
    };
  }
  const code = row.last_exit_code === null ? "Code not reported" : `Code ${row.last_exit_code}`;
  const lastExitMs = unixMs(row.last_exit_unix);
  const observedMs = parseIsoMs(row.observed_at);
  if (lastExitMs === null) {
    return {
      detail: "Exit time not reported",
      primary: code,
    };
  }
  if (observedMs !== null && lastExitMs > observedMs) {
    return {
      detail: "Time unknown; after observed",
      primary: code,
    };
  }
  const exitIso = new Date(lastExitMs).toISOString();
  return {
    detail: `Exited ${formatCompactTime(exitIso)}`,
    primary: code,
  };
}

function formatObservedTime(value: string): string {
  const ms = parseIsoMs(value);
  if (ms === null) {
    return "Invalid timestamp";
  }
  return formatFullTime(value);
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

function parseIsoMs(value: string): number | null {
  const ms = new Date(value).getTime();
  return Number.isFinite(ms) ? ms : null;
}

function unixMs(value: number | null): number | null {
  if (value === null || !Number.isFinite(value)) {
    return null;
  }
  return value * 1000;
}

function formatDurationMs(value: number): string {
  const totalMinutes = Math.max(0, Math.floor(value / 60_000));
  if (totalMinutes < 1) {
    return "<1m";
  }
  const days = Math.floor(totalMinutes / 1_440);
  const hours = Math.floor((totalMinutes % 1_440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) {
    return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  }
  if (hours > 0) {
    return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  }
  return `${minutes}m`;
}

function countPhrase(count: number, singular: string, plural = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : plural}`;
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}
