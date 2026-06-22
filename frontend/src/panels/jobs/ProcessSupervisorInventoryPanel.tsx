import { useMemo } from "react";
import { TerminalSquare } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import type { ProcessSupervisorInventoryRecord } from "../../types";
import { formatTime, shortId, statusClass } from "../../utils";

export function ProcessSupervisorInventoryPanel({
  clientLabel,
  inventory,
  loading,
  onRefresh,
}: {
  clientLabel: (clientId: string) => string;
  inventory: ProcessSupervisorInventoryRecord[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const columns = useMemo<ConsoleDataGridColumn<ProcessSupervisorInventoryRecord>[]>(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.name}</strong>
            <small>{clientLabel(row.client_id)}</small>
          </span>
        ),
        header: "Process",
        id: "process",
        searchValue: (row) => `${row.name} ${clientLabel(row.client_id)} ${row.client_id}`,
        sortValue: (row) => row.name,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <span className={`status ${statusClass(row.status)}`}>{row.status}</span>
            <small>{formatRestartEvidence(row)}</small>
          </span>
        ),
        header: "Status",
        id: "status",
        searchValue: (row) => `${row.status} ${formatRestartEvidence(row)}`,
        sortValue: (row) => row.status,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.pid ?? "-"}</strong>
            <small>{formatProcessRuntime(row)}</small>
          </span>
        ),
        header: "PID",
        id: "pid",
        searchValue: (row) => `${row.pid ?? ""} ${formatProcessRuntime(row)}`,
        sortValue: (row) => row.pid ?? 0,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.source_command_type}</strong>
            <small>{shortId(row.source_job_id)}</small>
          </span>
        ),
        header: "Source",
        id: "source",
        searchValue: (row) => `${row.source_command_type} ${row.source_job_id}`,
        sortValue: (row) => row.source_command_type,
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
    ],
    [clientLabel],
  );

  return (
    <div className="fleetPanel">
      <div className="sectionHeader">
        <div>
          <h2>Process supervisor inventory</h2>
          <span>{inventory.length} retained process states</span>
        </div>
        <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
          Refresh
        </button>
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
            <span>Source job</span>
            <strong>{row.source_job_id}</strong>
            <span>Runtime evidence</span>
            <strong>{formatProcessRuntime(row)}</strong>
            <span>Started</span>
            <strong>{formatUnixTime(row.started_unix)}</strong>
          </div>
        )}
        rows={inventory}
        searchPlaceholder="Search processes"
        selectable={false}
        storageKey="vpsman.jobs.processSupervisorInventory"
        title="Process records"
      />
    </div>
  );
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
    return `restarts ${attempts}; last exit ${row.last_exit_code}`;
  }
  return attempts > 0 ? `restarts ${attempts}` : "no restarts";
}

function formatProcessRuntime(row: ProcessSupervisorInventoryRecord): string {
  const limit = formatLimitEvidence(row.limit_effectiveness_status);
  const cgroup = formatCgroupEvidence(row);
  return cgroup ? `${limit}; ${cgroup}` : limit;
}

function formatLimitEvidence(status: string | null): string {
  if (status === "degraded_desired_only") {
    return "limits desired";
  }
  if (status === "enforced") {
    return "limits enforced";
  }
  if (status === "enforced_or_not_requested") {
    return "limits ok";
  }
  return "limits n/a";
}

function formatCgroupEvidence(row: ProcessSupervisorInventoryRecord): string | null {
  if (row.cgroup_status === "available") {
    const parts = [];
    if (row.cgroup_process_count !== null) {
      parts.push(`${row.cgroup_process_count} procs`);
    }
    if (row.cgroup_cpu_weight !== null) {
      parts.push(`cpu ${row.cgroup_cpu_weight}`);
    }
    if (row.cgroup_memory_current_bytes !== null) {
      parts.push(`${formatBytes(row.cgroup_memory_current_bytes)} mem`);
    }
    return parts.length > 0 ? parts.join(", ") : "cgroup ok";
  }
  if (row.cgroup_status === "missing") {
    return "cgroup missing";
  }
  return null;
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)}G`;
  }
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)}M`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)}K`;
  }
  return `${value}B`;
}
