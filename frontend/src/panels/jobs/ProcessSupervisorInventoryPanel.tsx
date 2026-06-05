import { TerminalSquare } from "lucide-react";
import { CrudPager } from "../../components/CrudPager";
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
      <CrudPager
        fields={[
          { label: "Process", value: (row) => row.name },
          { label: "VPS", value: (row) => clientLabel(row.client_id) },
          { label: "Status", value: (row) => `${row.status} ${formatRestartEvidence(row)} ${formatProcessRuntime(row)}` },
          { label: "PID", value: (row) => row.pid },
          { label: "Source", value: (row) => `${row.source_command_type} ${row.source_job_id}` },
        ]}
        itemLabel="processes"
        items={inventory}
        pageSize={10}
        title="Process records"
        empty={
          <div className="emptyState">
            <TerminalSquare size={22} />
            <strong>No supervised processes</strong>
            <span>Process start, status, restart, log, and stop jobs populate this inventory.</span>
          </div>
        }
      >
        {(rows) => (
          <div className="table historyTable">
            <div className="historyRow heading supervisorInventoryGrid">
              <span>Process</span>
              <span>Status</span>
              <span>PID</span>
              <span>Source</span>
              <span>Started</span>
              <span>Observed</span>
            </div>
            {rows.map((row) => (
              <div className="historyRow supervisorInventoryGrid" key={`${row.client_id}:${row.name}`}>
                <span className="historyPrimary">
                  <strong>{row.name}</strong>
                  <small>{clientLabel(row.client_id)}</small>
                </span>
                <span className="historyPrimary">
                  <span className={`status ${statusClass(row.status)}`}>{row.status}</span>
                  <small>{formatRestartEvidence(row)}</small>
                </span>
                <span className="historyPrimary">
                  <strong>{row.pid ?? "-"}</strong>
                  <small>{formatProcessRuntime(row)}</small>
                </span>
                <span className="historyPrimary">
                  <strong>{row.source_command_type}</strong>
                  <small>{shortId(row.source_job_id)}</small>
                </span>
                <span>{formatUnixTime(row.started_unix)}</span>
                <span>{formatTime(row.observed_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
    </div>
  );
}

function formatUnixTime(value: number | null): string {
  if (!value) {
    return "-";
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
