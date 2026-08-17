import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useByteCountFormatter } from "../../panelDisplay";
import { Database, FolderTree, HardDrive, RefreshCw } from "lucide-react";
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
  HostBlockDeviceRecord,
  HostMountRecord,
  HostStorageInventoryRecord,
  HostStorageProvider,
  JobOperation,
  JobTargetRecord,
} from "../../types";
import { formatCompactTime, formatFullTime } from "../../utils";
import { pushHistoryEntry, replaceHistoryEntry } from "../../historyEntryState";

const STORAGE_INVENTORY_LIMIT = 2048;

type StorageView = "devices" | "mounts";

export function HostStoragePanel({
  agents,
  clientLabel,
  onCreateJob,
  onLoadInventory,
  onLoadTargets,
}: {
  agents: AgentView[];
  clientLabel: (clientId: string) => string;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadInventory: (
    clientId: string,
    limit?: number,
  ) => Promise<HostStorageInventoryRecord>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
}) {
  const formatBytes = useByteCountFormatter();
  const [route, setRoute] = useState(readStorageRoute);
  const activeView = route.view;
  const [inventory, setInventory] = useState<HostStorageInventoryRecord | null>(
    null,
  );
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const submissionGuardRef = useRef(createSubmissionGuard());
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const selectedAgent = route.clientId
    ? (agents.find((agent) => agent.id === route.clientId) ?? null)
    : null;

  const loadInventory = useCallback(
    async (clientId: string, signal?: { cancelled: boolean }) => {
      setLoading(true);
      setError(null);
      try {
        const next = await onLoadInventory(clientId, STORAGE_INVENTORY_LIMIT);
        if (!signal?.cancelled) {
          setInventory(next);
        }
      } catch (nextError) {
        if (!signal?.cancelled) {
          setInventory(null);
          setError(
            nextError instanceof Error
              ? nextError.message
              : "Storage inventory is unavailable",
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
    const applyRoute = () => setRoute(readStorageRoute());
    window.addEventListener("popstate", applyRoute);
    window.addEventListener("hashchange", applyRoute);
    return () => {
      window.removeEventListener("popstate", applyRoute);
      window.removeEventListener("hashchange", applyRoute);
    };
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    setStatus(null);
    if (!route.clientId) {
      setInventory(null);
      setError(null);
      setLoading(false);
      return () => {
        signal.cancelled = true;
      };
    }
    void loadInventory(route.clientId, signal);
    return () => {
      signal.cancelled = true;
    };
  }, [loadInventory, route.clientId]);

  function updateRoute(
    clientId: string | null,
    includePseudoMounts = route.includePseudoMounts,
    view = route.view,
  ) {
    setStorageRoute(clientId, includePseudoMounts, view, "push");
    setRoute({ clientId, includePseudoMounts, view });
  }

  async function refreshInventory() {
    if (!selectedAgent) return;
    const submissionKey = `storage-inventory:${selectedAgent.id}:${route.includePseudoMounts}:${STORAGE_INVENTORY_LIMIT}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    const operation = {
      type: "storage_inventory",
      include_pseudo_mounts: route.includePseudoMounts,
      limit: STORAGE_INVENTORY_LIMIT,
    } as const satisfies JobOperation;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = 60;
    setRefreshing(true);
    setError(null);
    setStatus(`Refreshing storage on ${clientLabel(selectedAgent.id)}...`);
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
            `Refreshing storage on ${clientLabel(selectedAgent.id)} · ${progress.terminal}/${progress.total} VPS reported`,
          );
        },
        targetCount: createJobTargetCount(job),
        targets: [selectedAgent],
      });
      if (result.progress.completed < result.progress.total) {
        const reason = result.progress.failureReasons?.[0]?.reason;
        throw new Error(
          `Storage refresh did not complete${reason ? `: ${reason}` : "."}`,
        );
      }
      const next = await onLoadInventory(
        selectedAgent.id,
        STORAGE_INVENTORY_LIMIT,
      );
      setInventory(next);
      if (!next.capability || next.capability.status !== "supported") {
        setStatus(null);
        setError(
          next.capability?.reason ??
            "The agent did not confirm a supported storage provider.",
        );
      } else {
        setStatus(
          `Storage refreshed from ${clientLabel(selectedAgent.id)} using ${providerLabel(next.capability.provider)}.`,
        );
        successful = true;
      }
    } catch (nextError) {
      try {
        setInventory(
          await onLoadInventory(selectedAgent.id, STORAGE_INVENTORY_LIMIT),
        );
      } catch {
        // Preserve the refresh diagnostic below.
      }
      setError(
        nextError instanceof Error
          ? nextError.message
          : "Storage refresh failed",
      );
      setStatus(null);
    } finally {
      finishSubmission(submissionGuardRef.current, submissionKey, successful);
      setRefreshing(false);
    }
  }

  const devices = inventory?.devices ?? [];
  const mounts = inventory?.mounts ?? [];
  const capability = inventory?.capability ?? null;
  const rawDiskBytes = devices
    .filter((device) => device.device_type === "disk" && !device.parent_path)
    .reduce((total, device) => total + device.size_bytes, 0);
  const readOnlyMounts = mounts.filter((mount) => mount.read_only).length;
  const measuredFilesystems = devices.filter(
    (device) => device.filesystem_used_percent !== null,
  ).length;
  const highUsageFilesystems = devices.filter(
    (device) => (device.filesystem_used_percent ?? -1) >= 85,
  ).length;
  const lastAttemptFailed = Boolean(
    inventory?.last_attempt && inventory.last_attempt.status !== "completed",
  );
  const filterChanged = Boolean(
    inventory?.source_job_id &&
    inventory.include_pseudo_mounts !== route.includePseudoMounts,
  );

  const deviceColumns = useMemo<ConsoleDataGridColumn<HostBlockDeviceRecord>[]>(
    () => [
      {
        cell: (device) => (
          <span className="historyPrimary">
            <strong title={device.path}>{device.name}</strong>
            <small title={device.path}>{device.path}</small>
          </span>
        ),
        header: "Device",
        id: "device",
        minSize: 160,
        searchValue: (device) =>
          `${device.name} ${device.path} ${device.model ?? ""} ${device.serial ?? ""}`,
        size: 230,
        sortValue: (device) => device.path,
      },
      {
        cell: (device) => readableToken(device.device_type),
        header: "Type",
        id: "type",
        minSize: 78,
        searchValue: (device) => device.device_type,
        size: 92,
        sortValue: (device) => device.device_type,
      },
      {
        align: "end",
        cell: (device) => formatBytes(device.size_bytes),
        header: "Capacity",
        id: "capacity",
        minSize: 96,
        searchValue: (device) => device.size_bytes,
        size: 112,
        sortValue: (device) => device.size_bytes,
      },
      {
        cell: (device) => (
          <span className="historyPrimary">
            <strong title={device.filesystem_type ?? undefined}>
              {device.filesystem_type ?? "Not reported"}
            </strong>
            <small title={device.label ?? device.uuid ?? undefined}>
              {device.label ?? device.uuid ?? "No label"}
            </small>
          </span>
        ),
        header: "Filesystem",
        id: "filesystem",
        minSize: 128,
        searchValue: (device) =>
          `${device.filesystem_type ?? ""} ${device.label ?? ""} ${device.uuid ?? ""}`,
        size: 170,
        sortValue: (device) => device.filesystem_type ?? "",
      },
      {
        cell: (device) => <FilesystemUsage device={device} />,
        header: "Used",
        id: "usage",
        minSize: 120,
        resizeMinSize: 96,
        searchValue: (device) => device.filesystem_used_percent,
        size: 138,
        sortValue: (device) => device.filesystem_used_percent ?? -1,
      },
      {
        cell: (device) => {
          const value = device.mount_points.join(", ");
          return (
            <span title={value || undefined}>{value || "Not mounted"}</span>
          );
        },
        header: "Mounted at",
        id: "mounts",
        minSize: 150,
        searchValue: (device) => device.mount_points.join(" "),
        size: 220,
        sortValue: (device) => device.mount_points[0] ?? "",
      },
      {
        cell: (device) => (
          <span
            className={`status ${device.read_only ? "warning" : "neutral"}`}
            title={
              device.read_only
                ? "Kernel reports this block device as read-only"
                : "Kernel reports this block device as writable"
            }
          >
            {device.read_only ? "Read-only" : "Writable"}
          </span>
        ),
        header: "State",
        id: "state",
        minSize: 96,
        searchValue: (device) =>
          `${device.read_only ? "read only" : "writable"} ${device.removable ? "removable" : "fixed"}`,
        size: 110,
        sortValue: (device) => (device.read_only ? 1 : 0),
      },
    ],
    [formatBytes],
  );

  const mountColumns = useMemo<ConsoleDataGridColumn<HostMountRecord>[]>(
    () => [
      {
        cell: (mount) => (
          <span className="historyPrimary">
            <strong title={mount.target}>{mount.target}</strong>
            <small title={mount.root}>Root {mount.root}</small>
          </span>
        ),
        header: "Mount point",
        id: "target",
        minSize: 180,
        searchValue: (mount) => `${mount.target} ${mount.root}`,
        size: 260,
        sortValue: (mount) => mount.target,
      },
      {
        cell: (mount) => <span title={mount.source}>{mount.source}</span>,
        header: "Source",
        id: "source",
        minSize: 150,
        searchValue: (mount) => mount.source,
        size: 220,
        sortValue: (mount) => mount.source,
      },
      {
        cell: (mount) => readableToken(mount.filesystem_type),
        header: "Filesystem",
        id: "filesystem",
        minSize: 110,
        searchValue: (mount) => mount.filesystem_type,
        size: 132,
        sortValue: (mount) => mount.filesystem_type,
      },
      {
        cell: (mount) => {
          const value = mount.options.join(", ");
          return <span title={value}>{compactText(value, 64)}</span>;
        },
        header: "Options",
        id: "options",
        minSize: 180,
        searchValue: (mount) => mount.options.join(" "),
        size: 270,
        sortValue: (mount) => mount.options.join(","),
      },
      {
        cell: (mount) => (
          <span
            className={`status ${mount.read_only ? "warning" : "neutral"}`}
            title={`${mount.read_only ? "Read-only" : "Read-write"}${mount.pseudo ? "; kernel/system filesystem" : ""}`}
          >
            {mount.read_only ? "Read-only" : "Read-write"}
          </span>
        ),
        header: "State",
        id: "state",
        minSize: 100,
        searchValue: (mount) =>
          `${mount.read_only ? "read only" : "read write"} ${mount.pseudo ? "system" : "user"}`,
        size: 116,
        sortValue: (mount) => (mount.read_only ? 1 : 0),
      },
    ],
    [],
  );

  const refreshUnavailable = !selectedAgent
    ? "Choose one VPS before refreshing storage"
    : selectedAgent.status !== "online"
      ? `${clientLabel(selectedAgent.id)} is ${selectedAgent.status}; the last successful inventory remains visible`
      : null;

  return (
    <div className="fleetPanel hostStoragePanel">
      <div className="sectionHeader">
        <div>
          <h2>Host storage</h2>
          <span>
            Read-only block devices, mounted filesystems, and reported usage
          </span>
        </div>
        <div className="headerActionStack">
          <div className="processHeaderActions storageHeaderActions">
            <label
              className="processTargetPicker"
              title={
                agents.length === 0
                  ? "No VPS is available in the current fleet scope."
                  : refreshing
                    ? "The VPS cannot change while storage inventory is refreshing."
                    : "Choose the VPS whose storage should be inspected."
              }
            >
              <span>VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Storage inventory VPS"
                disabled={agents.length === 0 || refreshing}
                onChange={(value) => updateRoute(value || null)}
                placeholder="Choose one VPS"
                value={route.clientId ?? ""}
              />
            </label>
            <label
              className="storageSystemMountToggle"
              title="Include kernel and system pseudo filesystems such as proc, sysfs, and cgroup in the next snapshot"
            >
              <input
                checked={route.includePseudoMounts}
                data-tooltip-disabled-reason={
                  refreshing
                    ? "System-mount scope cannot change while storage inventory is refreshing."
                    : undefined
                }
                disabled={refreshing}
                onChange={(event) =>
                  updateRoute(route.clientId, event.currentTarget.checked)
                }
                type="checkbox"
              />
              <span>System mounts</span>
            </label>
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                refreshing
                  ? "A storage inventory refresh is already running."
                  : (refreshUnavailable ?? undefined)
              }
              disabled={Boolean(refreshUnavailable) || refreshing}
              onClick={() => void refreshInventory()}
              title={
                refreshUnavailable ??
                "Read block devices with lsblk and mounts from /proc/self/mountinfo"
              }
              type="button"
            >
              <RefreshCw size={14} />
              <span>{refreshing ? "Refreshing" : "Refresh inventory"}</span>
            </button>
          </div>
          <ActionFeedback
            className="localActionFeedback"
            message={error ?? status}
            tone={
              error
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

      {route.clientId && !selectedAgent ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`VPS ${route.clientId} is not in the current fleet scope.`}
          tone="warning"
        />
      ) : null}
      {capability?.status !== "supported" && capability ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`${readableToken(capability.status)}: ${capability.reason ?? "the agent did not confirm a supported lsblk machine format"}`}
          tone="warning"
        />
      ) : capability?.reason ? (
        <ActionFeedback
          className="localActionFeedback"
          message={capability.reason}
          tone="info"
        />
      ) : null}
      {lastAttemptFailed && !error ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`Latest refresh ${inventory?.last_attempt?.status}${inventory?.last_attempt?.message ? `: ${inventory.last_attempt.message}` : "; showing the last successful inventory."}`}
          tone="warning"
        />
      ) : null}
      {filterChanged ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`This snapshot ${inventory?.include_pseudo_mounts ? "includes" : "hides"} system mounts. Refresh inventory to apply the changed setting.`}
          tone="info"
        />
      ) : null}

      <div
        aria-label="Storage inventory summary"
        className="processSupervisorSummaryStrip storageSummaryStrip"
      >
        <span
          className={
            capability?.status !== "supported" ? "attention" : undefined
          }
          title={
            capability
              ? `${providerLabel(capability.provider)} capability is ${readableToken(capability.status)}.`
              : "The storage inventory provider has not been checked."
          }
        >
          <strong title={capability?.provider_version ?? undefined}>
            {providerLabel(capability?.provider ?? null)}
          </strong>
          <small>Provider</small>
        </span>
        <span
          title={`${formatBytes(rawDiskBytes)} raw capacity is reported across top-level disk devices.`}
        >
          <strong>{formatBytes(rawDiskBytes)}</strong>
          <small>Raw disks</small>
        </span>
        <span
          title={`${mounts.length} mount records are included in the current snapshot view.`}
        >
          <strong>{mounts.length}</strong>
          <small>Mounts shown</small>
        </span>
        <span
          className={readOnlyMounts > 0 ? "attention" : undefined}
          title={`${readOnlyMounts} displayed mounts are read-only.`}
        >
          <strong>{readOnlyMounts}</strong>
          <small>Read-only mounts</small>
        </span>
        <span
          className={highUsageFilesystems > 0 ? "attention" : undefined}
          title={`${highUsageFilesystems} filesystems report at least 85% usage.`}
        >
          <strong>{highUsageFilesystems}</strong>
          <small>At least 85% used</small>
        </span>
        <span
          className={
            capability && !capability.can_report_filesystem_usage
              ? "attention"
              : undefined
          }
          title={
            capability?.can_report_filesystem_usage
              ? `${measuredFilesystems} filesystems include usage measurements.`
              : "The detected provider does not report filesystem usage."
          }
        >
          <strong>
            {capability?.can_report_filesystem_usage
              ? `${measuredFilesystems} measured`
              : "Not reported"}
          </strong>
          <small>Usage coverage</small>
        </span>
        <span
          title={
            inventory?.observed_at
              ? `Storage inventory observed ${formatFullTime(inventory.observed_at)}.`
              : "No storage inventory observation time has been reported."
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

      <div className="storageViewBar">
        <div>
          <strong>Inventory view</strong>
          <span>
            {activeView === "devices"
              ? "Block-device hierarchy and filesystem usage"
              : "Kernel mount table and effective mount options"}
          </span>
        </div>
        <div
          aria-label="Storage inventory view"
          className="segmented"
          role="group"
        >
          <button
            aria-pressed={activeView === "devices"}
            className={activeView === "devices" ? "selected" : ""}
            onClick={() =>
              updateRoute(route.clientId, route.includePseudoMounts, "devices")
            }
            title="Show block devices and filesystems"
            type="button"
          >
            <HardDrive size={14} />
            <span>Devices</span>
          </button>
          <button
            aria-pressed={activeView === "mounts"}
            className={activeView === "mounts" ? "selected" : ""}
            onClick={() =>
              updateRoute(route.clientId, route.includePseudoMounts, "mounts")
            }
            title="Show the selected host mount table"
            type="button"
          >
            <FolderTree size={14} />
            <span>Mounts</span>
          </button>
        </div>
      </div>

      {activeView === "devices" ? (
        <ConsoleDataGrid
          columns={deviceColumns}
          defaultColumnVisibility={{ state: false }}
          defaultPageSize={25}
          empty={
            <StorageEmptyState
              capabilityStatus={capability?.status ?? null}
              loading={loading}
              selected={Boolean(route.clientId)}
              view="devices"
            />
          }
          expandOnRowClick
          getRowId={(device) => device.path}
          itemLabel="block devices"
          pageResetKey={`${inventory?.client_id ?? "none"}:${inventory?.include_pseudo_mounts ?? "none"}`}
          renderExpandedRow={(device) => (
            <DeviceDetails
              device={device}
              observedAt={inventory?.observed_at}
            />
          )}
          rows={devices}
          searchPlaceholder="Search device, path, filesystem, model, or serial"
          selectable={false}
          singleExpandedRow
          storageKey="vpsman.remote.hostStorage.devices"
          title="Block devices"
        />
      ) : (
        <ConsoleDataGrid
          columns={mountColumns}
          defaultPageSize={25}
          empty={
            <StorageEmptyState
              capabilityStatus={capability?.status ?? null}
              loading={loading}
              selected={Boolean(route.clientId)}
              view="mounts"
            />
          }
          expandOnRowClick
          getRowId={(mount) => `${mount.mount_id}:${mount.target}`}
          itemLabel="mounts"
          pageResetKey={`${inventory?.client_id ?? "none"}:${inventory?.include_pseudo_mounts ?? "none"}`}
          renderExpandedRow={(mount) => (
            <MountDetails mount={mount} observedAt={inventory?.observed_at} />
          )}
          rows={mounts}
          searchPlaceholder="Search mount point, source, filesystem, or option"
          selectable={false}
          singleExpandedRow
          storageKey="vpsman.remote.hostStorage.mounts"
          title="Mounted filesystems"
        />
      )}
    </div>
  );
}

function FilesystemUsage({ device }: { device: HostBlockDeviceRecord }) {
  const formatBytes = useByteCountFormatter();
  const percent = device.filesystem_used_percent;
  if (percent === null) {
    return <span className="mutedValue">Not reported</span>;
  }
  const tone = percent >= 90 ? "danger" : percent >= 85 ? "warning" : "normal";
  const detail = `${percent}% used${device.filesystem_available_bytes !== null ? `; ${formatBytes(device.filesystem_available_bytes)} available` : ""}`;
  return (
    <span className={`storageUsageCell ${tone}`} title={detail}>
      <span
        aria-label={detail}
        aria-valuemax={100}
        aria-valuemin={0}
        aria-valuenow={percent}
        className="storageUsageTrack"
        role="progressbar"
      >
        <i style={{ width: `${percent}%` }} />
      </span>
      <strong>{percent}%</strong>
    </span>
  );
}

function DeviceDetails({
  device,
  observedAt,
}: {
  device: HostBlockDeviceRecord;
  observedAt: string | null | undefined;
}) {
  const formatBytes = useByteCountFormatter();
  return (
    <div className="consoleInlineDetailGrid">
      <span>Path</span>
      <strong title={device.path}>{device.path}</strong>
      <span>Kernel name</span>
      <strong>{device.kernel_name ?? "Not reported"}</strong>
      <span>Parent</span>
      <strong title={device.parent_path ?? undefined}>
        {device.parent_path ?? "Top-level device"}
      </strong>
      <span>Type</span>
      <strong>{readableToken(device.device_type)}</strong>
      <span>Capacity</span>
      <strong>{formatBytes(device.size_bytes)}</strong>
      <span>Filesystem</span>
      <strong>
        {[device.filesystem_type, device.filesystem_version]
          .filter(Boolean)
          .join(" ") || "Not reported"}
      </strong>
      <span>Label</span>
      <strong title={device.label ?? undefined}>
        {device.label ?? "Not set"}
      </strong>
      <span>UUID</span>
      <strong title={device.uuid ?? undefined}>
        {device.uuid ?? "Not reported"}
      </strong>
      <span>Mounted at</span>
      <strong title={device.mount_points.join(", ") || undefined}>
        {device.mount_points.join(", ") || "Not mounted"}
      </strong>
      <span>Filesystem usage</span>
      <strong>
        {device.filesystem_used_percent === null
          ? "Not reported by this lsblk provider"
          : `${device.filesystem_used_percent}% used${device.filesystem_available_bytes !== null ? `; ${formatBytes(device.filesystem_available_bytes)} available` : ""}`}
      </strong>
      <span>Device state</span>
      <strong>
        {device.read_only ? "Read-only" : "Writable"} ·{" "}
        {device.removable ? "Removable" : "Fixed"}
      </strong>
      <span>Model</span>
      <strong title={device.model ?? undefined}>
        {device.model ?? "Not reported"}
      </strong>
      <span>Serial</span>
      <strong title={device.serial ?? undefined}>
        {device.serial ?? "Not reported"}
      </strong>
      <span>Transport</span>
      <strong>{device.transport ?? "Not reported"}</strong>
      <span>Major:minor</span>
      <strong>{device.major_minor ?? "Not reported"}</strong>
      <span>Observed</span>
      <strong>{observedAt ? formatFullTime(observedAt) : "Unknown"}</strong>
    </div>
  );
}

function MountDetails({
  mount,
  observedAt,
}: {
  mount: HostMountRecord;
  observedAt: string | null | undefined;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>Mount point</span>
      <strong title={mount.target}>{mount.target}</strong>
      <span>Source</span>
      <strong title={mount.source}>{mount.source}</strong>
      <span>Filesystem</span>
      <strong>{mount.filesystem_type}</strong>
      <span>Filesystem root</span>
      <strong title={mount.root}>{mount.root}</strong>
      <span>Mount ID</span>
      <strong>{mount.mount_id}</strong>
      <span>Parent mount ID</span>
      <strong>{mount.parent_id}</strong>
      <span>Major:minor</span>
      <strong>{mount.major_minor}</strong>
      <span>Access</span>
      <strong>{mount.read_only ? "Read-only" : "Read-write"}</strong>
      <span>System mount</span>
      <strong>{mount.pseudo ? "Yes" : "No"}</strong>
      <span>Options</span>
      <strong title={mount.options.join(", ")}>
        {mount.options.join(", ")}
      </strong>
      <span>Observed</span>
      <strong>{observedAt ? formatFullTime(observedAt) : "Unknown"}</strong>
    </div>
  );
}

function StorageEmptyState({
  capabilityStatus,
  loading,
  selected,
  view,
}: {
  capabilityStatus: string | null;
  loading: boolean;
  selected: boolean;
  view: StorageView;
}) {
  const noun = view === "devices" ? "block devices" : "mounts";
  return (
    <div className="emptyState compactEmpty">
      {view === "devices" ? <Database size={22} /> : <FolderTree size={22} />}
      <strong>
        {!selected
          ? "Choose a VPS"
          : loading
            ? `Loading ${noun}`
            : capabilityStatus && capabilityStatus !== "supported"
              ? "Storage inventory unsupported"
              : `No ${noun} reported`}
      </strong>
      <span>
        {!selected
          ? "Storage evidence is scoped to one VPS so device and mount identities stay unambiguous."
          : "Refresh inventory to read the selected host without changing disks, filesystems, or mounts."}
      </span>
    </div>
  );
}

function readStorageRoute(): {
  clientId: string | null;
  includePseudoMounts: boolean;
  view: StorageView;
} {
  if (typeof window === "undefined") {
    return { clientId: null, includePseudoMounts: false, view: "devices" };
  }
  const params = new URLSearchParams(window.location.search);
  return {
    clientId: params.get("storage_client")?.trim() || null,
    includePseudoMounts: params.get("storage_system") === "1",
    view: params.get("storage_view") === "mounts" ? "mounts" : "devices",
  };
}

function setStorageRoute(
  clientId: string | null,
  includePseudoMounts: boolean,
  view: StorageView,
  historyMode: "push" | "replace",
) {
  if (typeof window === "undefined") {
    return;
  }
  const url = new URL(window.location.href);
  if (clientId) {
    url.searchParams.set("storage_client", clientId);
  } else {
    url.searchParams.delete("storage_client");
  }
  if (includePseudoMounts) {
    url.searchParams.set("storage_system", "1");
  } else {
    url.searchParams.delete("storage_system");
  }
  if (view === "mounts") {
    url.searchParams.set("storage_view", "mounts");
  } else {
    url.searchParams.delete("storage_view");
  }
  const next = `${url.pathname}${url.search}${url.hash}`;
  if (
    `${window.location.pathname}${window.location.search}${window.location.hash}` ===
    next
  ) {
    return;
  }
  if (historyMode === "replace") {
    replaceHistoryEntry(next);
  } else {
    pushHistoryEntry(next);
  }
}

function providerLabel(provider: HostStorageProvider | null): string {
  switch (provider) {
    case "lsblk_json":
      return "lsblk JSON";
    case "lsblk_pairs":
      return "lsblk pairs";
    default:
      return "Not checked";
  }
}

function readableToken(value: string): string {
  if (!value) {
    return "Unknown";
  }
  return value
    .replace(/_/g, " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function compactText(value: string, limit: number): string {
  if (value.length <= limit) {
    return value;
  }
  return `${value.slice(0, Math.max(1, limit - 3))}...`;
}
