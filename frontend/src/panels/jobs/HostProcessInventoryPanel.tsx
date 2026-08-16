import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, RefreshCw } from "lucide-react";
import { ActionFeedback } from "../../components/ActionFeedback";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  createJobTargetCount,
  waitForBulkJobTargets,
} from "../../bulkJobProgress";
import { JOB_COMMAND_TYPE_BY_OPERATION_TYPE } from "../../generated/protocolContracts";
import {
  beginSubmission,
  createSubmissionGuard,
  finishSubmission,
} from "../../submissionGuard";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  HostProcessInventoryRecord,
  HostProcessRecord,
  JobTargetRecord,
} from "../../types";
import { formatCompactTime, formatFullTime } from "../../utils";
import { useByteCountFormatter } from "../../panelDisplay";

const HOST_PROCESS_LIMIT = 512;

export function HostProcessInventoryPanel({
  agents,
  clientLabel,
  onCreateJob,
  onLoadInventory,
  onLoadTargets,
  onSelectedClientIdChange,
  selectedClientId,
}: {
  agents: AgentView[];
  clientLabel: (clientId: string) => string;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadInventory: (
    clientId: string,
    limit?: number,
  ) => Promise<HostProcessInventoryRecord>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onSelectedClientIdChange: (clientId: string | null) => void;
  selectedClientId: string | null;
}) {
  const formatBytes = useByteCountFormatter();
  const formatKib = useCallback(
    (value: number) => formatBytes(Math.max(0, value) * 1024),
    [formatBytes],
  );
  const [inventory, setInventory] = useState<HostProcessInventoryRecord | null>(
    null,
  );
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const submissionGuardRef = useRef(createSubmissionGuard());
  const [status, setStatus] = useState<string | null>(null);
  const selectedAgent = selectedClientId
    ? (agents.find((agent) => agent.id === selectedClientId) ?? null)
    : null;

  const loadInventory = useCallback(
    async (clientId: string, signal?: { cancelled: boolean }) => {
      setLoading(true);
      setLoadError(null);
      try {
        const next = await onLoadInventory(clientId, HOST_PROCESS_LIMIT);
        if (!signal?.cancelled) {
          setInventory(next);
        }
      } catch (error) {
        if (!signal?.cancelled) {
          setInventory(null);
          setLoadError(
            error instanceof Error
              ? error.message
              : "Host process snapshot is unavailable",
          );
        }
      } finally {
        if (!signal?.cancelled) {
          setLoading(false);
        }
      }
    },
    [onLoadInventory],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    setStatus(null);
    if (!selectedClientId) {
      setInventory(null);
      setLoadError(null);
      setLoading(false);
      return () => {
        signal.cancelled = true;
      };
    }
    void loadInventory(selectedClientId, signal);
    return () => {
      signal.cancelled = true;
    };
  }, [loadInventory, selectedClientId]);

  async function refreshSnapshot() {
    if (!selectedAgent) return;
    const submissionKey = `process-list:${selectedAgent.id}:${HOST_PROCESS_LIMIT}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    const operation = {
      type: "process_list",
      limit: HOST_PROCESS_LIMIT,
    } as const;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = 45;
    setLoadError(null);
    setStatus(
      `Refreshing host processes on ${clientLabel(selectedAgent.id)}...`,
    );
    setRefreshing(true);
    try {
      const job = await onCreateJob({
        argv: [],
        command,
        confirmed: false,
        destructive: false,
        force_unprivileged: false,
        job_id: crypto.randomUUID(),
        max_timeout_secs: maxTimeoutSecs,
        operation,
        privileged: false,
        selector_expression: `id:${selectedAgent.id}`,
        target_client_ids: [selectedAgent.id],
      });
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        maxTimeoutSecs,
        onProgress: (progress) => {
          setStatus(
            `Refreshing host processes on ${clientLabel(selectedAgent.id)} · ${progress.terminal}/${progress.total} VPS reported`,
          );
        },
        targetCount: createJobTargetCount(job),
        targets: [selectedAgent],
      });
      const next = await onLoadInventory(selectedAgent.id, HOST_PROCESS_LIMIT);
      setInventory(next);
      if (result.progress.completed < result.progress.total) {
        const reason = result.progress.failureReasons?.[0]?.reason;
        setLoadError(
          `Host process refresh did not complete${reason ? `: ${reason}` : "."}`,
        );
        setStatus(null);
        return;
      }
      setStatus(
        `Host process snapshot refreshed from ${clientLabel(selectedAgent.id)}.`,
      );
      successful = true;
    } catch (error) {
      try {
        setInventory(
          await onLoadInventory(selectedAgent.id, HOST_PROCESS_LIMIT),
        );
      } catch {
        // The action error below is the useful operator-facing diagnostic.
      }
      setLoadError(
        error instanceof Error ? error.message : "Host process refresh failed",
      );
      setStatus(null);
    } finally {
      finishSubmission(submissionGuardRef.current, submissionKey, successful);
      setRefreshing(false);
    }
  }

  const rows = inventory?.processes ?? [];
  const totalRssKib = rows.reduce((total, row) => total + row.rss_kib, 0);
  const rootCount = rows.filter((row) => row.uid === 0).length;
  const lastAttemptFailed = Boolean(
    inventory?.last_attempt && inventory.last_attempt.status !== "completed",
  );
  const columns = useMemo<ConsoleDataGridColumn<HostProcessRecord>[]>(
    () => [
      {
        cell: (row) => {
          const command = row.command.trim() || row.name;
          const compact = compactCommand(row.command, row.name);
          return (
            <span className="historyPrimary">
              <strong title={row.name}>{row.name}</strong>
              <small title={compact === command ? undefined : command}>
                {compact}
              </small>
            </span>
          );
        },
        header: "Process",
        id: "process",
        minSize: 180,
        searchValue: (row) => `${row.name} ${row.command}`,
        size: 280,
        sortValue: (row) => row.name,
      },
      {
        align: "end",
        cell: (row) => row.pid,
        header: "PID",
        id: "pid",
        minSize: 72,
        searchValue: (row) => row.pid,
        size: 78,
        sortValue: (row) => row.pid,
      },
      {
        align: "end",
        cell: (row) => formatKib(row.rss_kib),
        header: "RSS",
        id: "rss",
        minSize: 88,
        searchValue: (row) => row.rss_kib,
        size: 98,
        sortValue: (row) => row.rss_kib,
      },
      {
        cell: (row) => (
          <span
            className="status neutral"
            title={processStateDetail(row.state)}
          >
            {processStateLabel(row.state)}
          </span>
        ),
        header: "State",
        id: "state",
        minSize: 86,
        searchValue: (row) => `${row.state} ${processStateLabel(row.state)}`,
        size: 94,
        sortValue: (row) => row.state,
      },
      {
        align: "end",
        cell: (row) => row.uid,
        header: "UID",
        id: "uid",
        minSize: 72,
        searchValue: (row) => row.uid,
        size: 78,
        sortValue: (row) => row.uid,
      },
      {
        align: "end",
        cell: (row) => row.ppid,
        header: "PPID",
        id: "ppid",
        minSize: 72,
        searchValue: (row) => row.ppid,
        size: 78,
        sortValue: (row) => row.ppid,
      },
    ],
    [formatKib],
  );

  const refreshUnavailable = !selectedAgent
    ? "Choose one VPS before refreshing host processes"
    : selectedAgent.status !== "online"
      ? `${clientLabel(selectedAgent.id)} is ${selectedAgent.status}; the last accepted snapshot remains available`
      : null;

  return (
    <div className="fleetPanel hostProcessPanel">
      <div className="sectionHeader">
        <div>
          <h2>Host processes</h2>
          <span>
            Read-only Linux process inventory from one explicitly selected VPS
          </span>
        </div>
        <div className="headerActionStack">
          <div className="processHeaderActions">
            <label
              className="processTargetPicker"
              title={
                agents.length === 0
                  ? "No VPS is available in the current fleet scope."
                  : refreshing
                    ? "The VPS cannot change while a host-process snapshot refresh is running."
                    : "Choose the VPS whose host processes should be inspected."
              }
            >
              <span>VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Host process VPS"
                disabled={agents.length === 0 || refreshing}
                onChange={(value) => onSelectedClientIdChange(value || null)}
                placeholder="Choose one VPS"
                value={selectedClientId ?? ""}
              />
            </label>
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                refreshing
                  ? "A host-process snapshot refresh is already running."
                  : (refreshUnavailable ?? undefined)
              }
              disabled={Boolean(refreshUnavailable) || refreshing}
              onClick={() => void refreshSnapshot()}
              title={
                refreshUnavailable ?? "Capture a new bounded /proc snapshot"
              }
              type="button"
            >
              <RefreshCw size={14} />
              <span>{refreshing ? "Refreshing" : "Refresh snapshot"}</span>
            </button>
          </div>
          <ActionFeedback
            className="localActionFeedback"
            message={loadError ?? status}
            tone={
              loadError
                ? inventory?.source_job_id
                  ? "warning"
                  : "danger"
                : refreshing || loading
                  ? "progress"
                  : "success"
            }
          />
        </div>
      </div>

      {selectedClientId && !selectedAgent ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`VPS ${selectedClientId} is not in the current fleet scope.`}
          tone="warning"
        />
      ) : null}
      {inventory?.last_attempt && lastAttemptFailed && !loadError ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`Latest refresh ${inventory.last_attempt.status}${inventory.last_attempt.message ? `: ${inventory.last_attempt.message}` : "; showing the last successful snapshot."}`}
          tone="warning"
        />
      ) : null}

      <div
        aria-label="Host process snapshot summary"
        className="processSupervisorSummaryStrip"
      >
        <span
          title={`${rows.length} processes are included in the retained bounded snapshot.`}
        >
          <strong>{rows.length}</strong>
          <small>Processes shown</small>
        </span>
        <span
          title={`${formatKib(totalRssKib)} total resident memory is reported across the displayed processes.`}
        >
          <strong>{formatKib(totalRssKib)}</strong>
          <small>Reported RSS</small>
        </span>
        <span
          title={`${rootCount} displayed processes report Linux user ID 0.`}
        >
          <strong>{rootCount}</strong>
          <small>UID 0</small>
        </span>
        <span
          className={inventory?.truncated ? "attention" : undefined}
          title={
            !inventory
              ? "No host-process snapshot has been retained."
              : inventory.truncated
                ? "The agent bounded this process snapshot; additional processes may exist."
                : "The retained process snapshot was not truncated."
          }
        >
          <strong>
            {!inventory
              ? "No snapshot"
              : inventory.truncated
                ? "Bounded"
                : "Complete"}
          </strong>
          <small>Snapshot</small>
        </span>
        <span
          title={
            inventory?.source
              ? `Snapshot source: ${inventory.source}.`
              : "No process snapshot source has been reported."
          }
        >
          <strong title={inventory?.source ?? undefined}>
            {inventory?.source ?? "No source"}
          </strong>
          <small>Agent source</small>
        </span>
        <span
          title={
            inventory?.observed_at
              ? `Snapshot observed ${formatFullTime(inventory.observed_at)}.`
              : "No process snapshot observation time has been reported."
          }
        >
          <strong
            title={
              inventory?.observed_at
                ? formatFullTime(inventory.observed_at)
                : undefined
            }
          >
            {inventory?.observed_at
              ? formatCompactTime(inventory.observed_at)
              : "Never"}
          </strong>
          <small>Observed</small>
        </span>
      </div>

      <div className="hostProcessGridShell">
        <ConsoleDataGrid
          columns={columns}
          defaultColumnVisibility={{ ppid: false, uid: false }}
          defaultPageSize={25}
          empty={
            <div className="emptyState compactEmpty">
              <Activity size={22} />
              <strong>
                {selectedClientId
                  ? loading
                    ? "Loading host processes"
                    : "No host process snapshot"
                  : "Choose a VPS"}
              </strong>
              <span>
                {selectedClientId
                  ? "Refresh snapshot to capture the current bounded process list."
                  : "Host process inventory is scoped to one VPS to keep PID context unambiguous."}
              </span>
            </div>
          }
          expandOnRowClick
          getRowId={(row) => String(row.pid)}
          itemLabel="host processes"
          renderExpandedRow={(row) => (
            <div className="consoleInlineDetailGrid">
              <span>Name</span>
              <strong title={row.name}>{row.name}</strong>
              <span>Command</span>
              <strong className="processEvidenceValue">
                {row.command || row.name}
              </strong>
              <span>PID</span>
              <strong>{row.pid}</strong>
              <span>Parent PID</span>
              <strong>{row.ppid}</strong>
              <span>User ID</span>
              <strong>{row.uid}</strong>
              <span>State</span>
              <strong title={processStateDetail(row.state)}>
                {processStateLabel(row.state)} ({row.state})
              </strong>
              <span>Resident memory</span>
              <strong>{formatKib(row.rss_kib)}</strong>
              <span>Snapshot</span>
              <strong>
                {inventory?.observed_at
                  ? formatFullTime(inventory.observed_at)
                  : "Unknown"}
              </strong>
            </div>
          )}
          rows={rows}
          searchPlaceholder="Search name, command, PID, or UID"
          selectable={false}
          storageKey="vpsman.remote.hostProcesses"
          title="Host process inventory"
        />
      </div>
    </div>
  );
}

function compactCommand(command: string, name: string): string {
  const value = command.trim() || name;
  return value.length > 84 ? `${value.slice(0, 81)}...` : value;
}

function processStateLabel(state: string): string {
  switch (state.trim().slice(0, 1).toUpperCase()) {
    case "R":
      return "Running";
    case "S":
      return "Sleeping";
    case "D":
      return "I/O wait";
    case "T":
      return "Stopped";
    case "Z":
      return "Zombie";
    case "I":
      return "Idle";
    default:
      return state || "Unknown";
  }
}

function processStateDetail(state: string): string {
  return `Linux process state ${state || "unknown"}: ${processStateLabel(state)}`;
}
