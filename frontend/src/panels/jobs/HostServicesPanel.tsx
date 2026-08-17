import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FileText,
  Power,
  RefreshCw,
  RotateCcw,
  ServerCog,
  Square,
} from "lucide-react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ConsoleDetailPanel } from "../../components/ConsoleDetailPanel";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  createJobTargetCount,
  waitForBulkJobTargets,
} from "../../bulkJobProgress";
import {
  JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE,
  JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
} from "../../generated/protocolContracts";
import {
  buildPrivilegeForJobOperation,
  type PrivilegeMaterial,
} from "../../privilege";
import { scrollIntoViewWithMotion } from "../../motion";
import {
  beginSubmission,
  createSubmissionGuard,
  finishSubmission,
} from "../../submissionGuard";
import { pushHistoryEntry } from "../../historyEntryState";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  HostServiceAction,
  HostServiceInventoryRecord,
  HostServiceLogSnapshot,
  HostServiceProvider,
  HostServiceRecord,
  JobOperation,
  JobTargetRecord,
} from "../../types";
import { formatCompactTime, formatFullTime } from "../../utils";

const SERVICE_INVENTORY_LIMIT = 1024;

type PendingServiceAction = {
  action: HostServiceAction;
  service: HostServiceRecord;
};

export function HostServicesPanel({
  agents,
  clientLabel,
  onCreateJob,
  onDownloadOutputStream,
  onLoadInventory,
  onLoadTargets,
  onOpenPrivilegeUnlock,
  privilegeMaterial,
}: {
  agents: AgentView[];
  clientLabel: (clientId: string) => string;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadOutputStream: (
    jobId: string,
    clientId: string,
    stream: "stdout" | "stderr" | "combined",
  ) => Promise<Blob>;
  onLoadInventory: (
    clientId: string,
    limit?: number,
  ) => Promise<HostServiceInventoryRecord>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenPrivilegeUnlock: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [selectedClientId, setSelectedClientId] = useState(
    readServiceClientRoute,
  );
  const [inventory, setInventory] = useState<HostServiceInventoryRecord | null>(
    null,
  );
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState(false);
  const submissionGuardRef = useRef(createSubmissionGuard());
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [warning, setWarning] = useState(false);
  const [pendingAction, setPendingAction] =
    useState<PendingServiceAction | null>(null);
  const [logs, setLogs] = useState<HostServiceLogSnapshot | null>(null);
  const actionFeedbackRef = useRef<HTMLDivElement | null>(null);
  const logsPanelRef = useRef<HTMLDivElement | null>(null);
  const selectedAgent = selectedClientId
    ? (agents.find((agent) => agent.id === selectedClientId) ?? null)
    : null;

  const loadInventory = useCallback(
    async (clientId: string, signal?: { cancelled: boolean }) => {
      setLoading(true);
      setError(null);
      try {
        const next = await onLoadInventory(clientId, SERVICE_INVENTORY_LIMIT);
        if (!signal?.cancelled) {
          setInventory(next);
        }
      } catch (nextError) {
        if (!signal?.cancelled) {
          setInventory(null);
          setError(
            nextError instanceof Error
              ? nextError.message
              : "Service inventory is unavailable",
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
    const applyRoute = () => setSelectedClientId(readServiceClientRoute());
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
    setWarning(false);
    setLogs(null);
    if (!selectedClientId) {
      setInventory(null);
      setError(null);
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

  useEffect(() => {
    if (!logs) {
      return;
    }
    window.requestAnimationFrame(() => {
      logsPanelRef.current?.scrollIntoView({
        behavior: "smooth",
        block: "start",
      });
      logsPanelRef.current?.focus({ preventScroll: true });
    });
  }, [logs]);

  useEffect(() => {
    if ((!error && !status) || pendingAction) {
      return;
    }
    const feedback = actionFeedbackRef.current;
    if (feedback) {
      scrollIntoViewWithMotion(feedback, { block: "nearest" });
    }
  }, [error, pendingAction, status]);

  function selectClient(clientId: string | null) {
    setServiceClientRoute(clientId);
    setSelectedClientId(clientId);
  }

  async function dispatchInventory(
    agent: AgentView,
    progressLabel = "Refreshing service inventory",
  ): Promise<HostServiceInventoryRecord> {
    const operation = {
      type: "service_inventory",
      expected_provider: inventory?.capability?.provider ?? null,
      limit: SERVICE_INVENTORY_LIMIT,
    } as const satisfies JobOperation;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = 60;
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
      selector_expression: `id:${agent.id}`,
      target_client_ids: [agent.id],
    });
    const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
      maxTimeoutSecs,
      onProgress: (progress) => {
        setStatus(
          `${progressLabel} on ${clientLabel(agent.id)} · ${progress.terminal}/${progress.total} VPS reported`,
        );
      },
      targetCount: createJobTargetCount(job),
      targets: [agent],
    });
    if (result.progress.completed < result.progress.total) {
      const reason = result.progress.failureReasons?.[0]?.reason;
      throw new Error(
        `${progressLabel} did not complete${reason ? `: ${reason}` : "."}`,
      );
    }
    return onLoadInventory(agent.id, SERVICE_INVENTORY_LIMIT);
  }

  async function refreshInventory() {
    if (!selectedAgent) return;
    const submissionKey = `service-inventory:${selectedAgent.id}:${SERVICE_INVENTORY_LIMIT}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    setPending(true);
    setError(null);
    setWarning(false);
    setStatus(
      `Refreshing service inventory on ${clientLabel(selectedAgent.id)}...`,
    );
    try {
      const next = await dispatchInventory(selectedAgent);
      setInventory(next);
      successful = true;
      const capability = next.capability;
      if (!capability || capability.status !== "supported") {
        setWarning(true);
        setStatus(
          capability?.reason ??
            "The agent did not confirm a supported service provider.",
        );
      } else {
        setStatus(
          `Service inventory refreshed from ${clientLabel(selectedAgent.id)} using ${providerLabel(capability.provider)}.`,
        );
      }
    } catch (nextError) {
      try {
        setInventory(
          await onLoadInventory(selectedAgent.id, SERVICE_INVENTORY_LIMIT),
        );
      } catch {
        // Preserve the action diagnostic below.
      }
      setError(
        nextError instanceof Error
          ? nextError.message
          : "Service inventory refresh failed",
      );
      setStatus(null);
    } finally {
      finishSubmission(submissionGuardRef.current, submissionKey, successful);
      setPending(false);
    }
  }

  function reviewAction(service: HostServiceRecord, action: HostServiceAction) {
    if (!privilegeMaterial) {
      setError("Unlock privilege before changing a host service.");
      setStatus(null);
      onOpenPrivilegeUnlock();
      return;
    }
    setError(null);
    setPendingAction({ action, service });
  }

  async function executeAction(review: PendingServiceAction) {
    const agent = selectedAgent;
    const provider = inventory?.capability?.provider;
    if (!agent || !provider || !privilegeMaterial) return;
    const submissionKey = `service-action:${agent.id}:${provider}:${review.service.name}:${review.action}:${review.service.active_state}:${review.service.enabled_state}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let actionCompleted = false;
    const operation = {
      type: "service_action",
      provider,
      service: review.service.name,
      action: review.action,
      expected_active_state: review.service.active_state,
      expected_enabled_state: review.service.enabled_state,
    } as const satisfies JobOperation;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = 120;
    setPendingAction(null);
    setPending(true);
    setError(null);
    setWarning(false);
    setStatus(
      `${actionPresentParticiple(review.action)} ${review.service.name} on ${clientLabel(agent.id)}...`,
    );
    try {
      const { privilegeAssertion } = await buildPrivilegeForJobOperation({
        clientIds: [agent.id],
        commandType: command,
        maxTimeoutSecs,
        operation,
        privilegeMaterial,
        selectorExpression: `id:${agent.id}`,
      });
      const job = await onCreateJob({
        argv: [],
        command,
        confirmed: true,
        destructive: Boolean(
          JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE[operation.type],
        ),
        force_unprivileged: false,
        job_id: crypto.randomUUID(),
        max_timeout_secs: maxTimeoutSecs,
        operation,
        privileged: true,
        privilege_assertion: privilegeAssertion,
        selector_expression: `id:${agent.id}`,
        target_client_ids: [agent.id],
      });
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        maxTimeoutSecs,
        onProgress: (progress) => {
          setStatus(
            `${actionPresentParticiple(review.action)} ${review.service.name} · ${progress.terminal}/${progress.total} VPS reported`,
          );
        },
        targetCount: createJobTargetCount(job),
        targets: [agent],
      });
      if (result.progress.completed < result.progress.total) {
        const reason = result.progress.failureReasons?.[0]?.reason;
        throw new Error(
          `${actionLabel(review.action)} failed${reason ? `: ${reason}` : "."}`,
        );
      }
      actionCompleted = true;
      setStatus(
        `${actionLabel(review.action)} accepted; refreshing ${review.service.name} state...`,
      );
      const next = await dispatchInventory(agent, "Refreshing service state");
      setInventory(next);
      setStatus(
        `${actionPastTense(review.action)} ${review.service.name} on ${clientLabel(agent.id)}.`,
      );
    } catch (nextError) {
      const message =
        nextError instanceof Error
          ? nextError.message
          : `${actionLabel(review.action)} failed`;
      if (actionCompleted) {
        setWarning(true);
        setError(null);
        setStatus(
          `${actionPastTense(review.action)} ${review.service.name}, but state refresh failed: ${message}`,
        );
      } else {
        setError(message);
        setStatus(null);
      }
    } finally {
      finishSubmission(
        submissionGuardRef.current,
        submissionKey,
        actionCompleted,
      );
      setPending(false);
    }
  }

  async function loadLogs(service: HostServiceRecord) {
    const agent = selectedAgent;
    const provider = inventory?.capability?.provider;
    if (!agent || !provider) return;
    const submissionKey = `service-logs:${agent.id}:${provider}:${service.name}:500`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    const operation = {
      type: "service_logs",
      provider,
      service: service.name,
      max_lines: 500,
    } as const satisfies JobOperation;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = 45;
    setPending(true);
    setError(null);
    setWarning(false);
    setStatus(
      `Loading logs for ${service.name} on ${clientLabel(agent.id)}...`,
    );
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
        selector_expression: `id:${agent.id}`,
        target_client_ids: [agent.id],
      });
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        maxTimeoutSecs,
        targetCount: createJobTargetCount(job),
        targets: [agent],
      });
      if (result.progress.completed < result.progress.total) {
        const reason = result.progress.failureReasons?.[0]?.reason;
        throw new Error(`Service logs failed${reason ? `: ${reason}` : "."}`);
      }
      const blob = await onDownloadOutputStream(job.job_id, agent.id, "stdout");
      const snapshot = JSON.parse(await blob.text()) as HostServiceLogSnapshot;
      if (
        snapshot.type !== "service_logs" ||
        snapshot.service !== service.name ||
        !Array.isArray(snapshot.lines)
      ) {
        throw new Error("Agent returned an invalid service log snapshot");
      }
      setLogs(snapshot);
      successful = true;
      setStatus(
        `Loaded ${snapshot.lines.length} log line${snapshot.lines.length === 1 ? "" : "s"} for ${service.name}.`,
      );
    } catch (nextError) {
      setError(
        nextError instanceof Error ? nextError.message : "Service logs failed",
      );
      setStatus(null);
    } finally {
      finishSubmission(submissionGuardRef.current, submissionKey, successful);
      setPending(false);
    }
  }

  const capability = inventory?.capability ?? null;
  const services = inventory?.services ?? [];
  const activeCount = services.filter(
    (service) => service.active_state === "active",
  ).length;
  const failedCount = services.filter(
    (service) => service.active_state === "failed",
  ).length;
  const enabledCount = services.filter((service) =>
    service.enabled_state.startsWith("enabled"),
  ).length;
  const columns = useMemo<ConsoleDataGridColumn<HostServiceRecord>[]>(
    () => [
      {
        cell: (service) => (
          <span className="historyPrimary">
            <strong title={service.name}>{service.name}</strong>
            <small title={service.description || undefined}>
              {service.description || "No provider description"}
            </small>
          </span>
        ),
        header: "Service",
        id: "service",
        minSize: 190,
        searchValue: (service) => `${service.name} ${service.description}`,
        size: 280,
        sortValue: (service) => service.name,
      },
      {
        cell: (service) => (
          <span
            className="historyPrimary"
            title={
              service.state_reason
                ? `Provider diagnostic: ${service.state_reason}`
                : undefined
            }
          >
            <span className={`status ${serviceTone(service.active_state)}`}>
              {readableState(service.active_state)}
            </span>
            <small>{readableState(service.sub_state)}</small>
          </span>
        ),
        header: "Runtime",
        id: "runtime",
        minSize: 118,
        searchValue: (service) =>
          `${service.active_state} ${service.sub_state} ${service.state_reason ?? ""}`,
        size: 132,
        sortValue: (service) => service.active_state,
      },
      {
        cell: (service) => (
          <span className="historyPrimary">
            <strong>{readableState(service.enabled_state)}</strong>
            <small>{readableState(service.load_state)}</small>
          </span>
        ),
        header: "At boot",
        id: "enabled",
        minSize: 112,
        searchValue: (service) =>
          `${service.enabled_state} ${service.load_state}`,
        size: 126,
        sortValue: (service) => service.enabled_state,
      },
    ],
    [],
  );
  const rowActions = useMemo<ConsoleDataGridAction<HostServiceRecord>[]>(
    () => [
      {
        description: (rows) =>
          `Read journal logs for ${rows[0]?.name ?? "service"}.`,
        disabled: () => pending || !capability?.can_read_logs,
        icon: <FileText size={14} />,
        label: "Logs",
        onSelect: (rows) => rows[0] && void loadLogs(rows[0]),
      },
      {
        description: (rows) => `Start ${rows[0]?.name ?? "service"}.`,
        disabled: () => pending || !capability?.can_start_stop_restart,
        hidden: (rows) => rows[0]?.active_state === "active",
        icon: <Power size={14} />,
        label: "Start",
        onSelect: (rows) => rows[0] && reviewAction(rows[0], "start"),
      },
      {
        description: (rows) => `Restart ${rows[0]?.name ?? "service"}.`,
        disabled: () => pending || !capability?.can_start_stop_restart,
        icon: <RotateCcw size={14} />,
        label: "Restart",
        onSelect: (rows) => rows[0] && reviewAction(rows[0], "restart"),
      },
      {
        description: (rows) => `Stop ${rows[0]?.name ?? "service"}.`,
        disabled: () => pending || !capability?.can_start_stop_restart,
        hidden: (rows) => rows[0]?.active_state !== "active",
        icon: <Square size={14} />,
        label: "Stop",
        onSelect: (rows) => rows[0] && reviewAction(rows[0], "stop"),
        tone: "danger",
      },
      {
        description: (rows) => `Enable ${rows[0]?.name ?? "service"} at boot.`,
        disabled: () => pending || !capability?.can_enable_disable,
        hidden: (rows) => rows[0]?.enabled_state !== "disabled",
        label: "Enable at boot",
        onSelect: (rows) => rows[0] && reviewAction(rows[0], "enable"),
      },
      {
        description: (rows) => `Disable ${rows[0]?.name ?? "service"} at boot.`,
        disabled: () => pending || !capability?.can_enable_disable,
        hidden: (rows) => !rows[0]?.enabled_state.startsWith("enabled"),
        label: "Disable at boot",
        onSelect: (rows) => rows[0] && reviewAction(rows[0], "disable"),
        tone: "danger",
      },
    ],
    [capability, pending, privilegeMaterial],
  );
  const refreshUnavailable = !selectedAgent
    ? "Choose one VPS before refreshing services"
    : selectedAgent.status !== "online"
      ? `${clientLabel(selectedAgent.id)} is ${selectedAgent.status}; the last successful inventory remains visible`
      : null;

  return (
    <div className="jobConsoleStack">
      <div className="fleetPanel hostServicesPanel">
        <div className="sectionHeader">
          <div>
            <h2>Host services</h2>
            <span>
              Active init provider, service state, boot policy, actions, and
              logs
            </span>
          </div>
          <div className="headerActionStack">
            <div className="processHeaderActions">
              <label
                className="processTargetPicker"
                title={
                  agents.length === 0
                    ? "No VPS is available in the current fleet scope."
                    : pending
                      ? "The VPS cannot change while a service inventory operation is running."
                      : "Choose the VPS whose services should be inspected."
                }
              >
                <span>VPS</span>
                <VpsCombobox
                  agents={agents}
                  ariaLabel="Service inventory VPS"
                  disabled={agents.length === 0 || pending}
                  onChange={(value) => selectClient(value || null)}
                  placeholder="Choose one VPS"
                  value={selectedClientId ?? ""}
                />
              </label>
              <button
                className="secondaryAction compactAction"
                data-tooltip-disabled-reason={
                  pending
                    ? "A service inventory operation is already running."
                    : (refreshUnavailable ?? undefined)
                }
                disabled={Boolean(refreshUnavailable) || pending}
                onClick={() => void refreshInventory()}
                title={
                  refreshUnavailable ??
                  "Detect the active init provider and refresh services"
                }
                type="button"
              >
                <RefreshCw size={14} />
                <span>{pending ? "Working" : "Refresh inventory"}</span>
              </button>
            </div>
          </div>
        </div>

        {capability && capability.status !== "supported" ? (
          <ActionFeedback
            className="localActionFeedback"
            message={`${readableState(capability.status)}: ${capability.reason ?? "the agent did not confirm a supported init provider"}`}
            tone="warning"
          />
        ) : null}
        {inventory?.last_attempt &&
        inventory.last_attempt.status !== "completed" &&
        !error ? (
          <ActionFeedback
            className="localActionFeedback"
            message={`Latest refresh ${inventory.last_attempt.status}${inventory.last_attempt.message ? `: ${inventory.last_attempt.message}` : "; showing the last successful inventory."}`}
            tone="warning"
          />
        ) : null}

        <div
          aria-label="Service capability summary"
          className="processSupervisorSummaryStrip"
        >
          <span
            className={
              capability?.status !== "supported" ? "attention" : undefined
            }
            title={
              capability
                ? `${providerLabel(capability.provider)} provider capability is ${readableState(capability.status)}.`
                : "The service provider has not been detected."
            }
          >
            <strong>{providerLabel(capability?.provider ?? null)}</strong>
            <small>Provider</small>
          </span>
          <span
            title={`${activeCount} of ${services.length} retained services are active.`}
          >
            <strong>
              {activeCount} / {services.length}
            </strong>
            <small>Active</small>
          </span>
          <span
            className={failedCount > 0 ? "attention" : undefined}
            title={`${failedCount} retained services report a failed runtime state.`}
          >
            <strong>{failedCount}</strong>
            <small>Failed</small>
          </span>
          <span
            title={`${enabledCount} retained services are enabled at boot.`}
          >
            <strong>{enabledCount}</strong>
            <small>Enabled at boot</small>
          </span>
          <span
            className={
              capability && !capability.can_read_logs ? "attention" : undefined
            }
            title={
              capability?.can_read_logs
                ? "The detected provider supports service log reads."
                : "Service log reads are not supported by the detected provider."
            }
          >
            <strong>
              {capability?.can_read_logs ? "Available" : "Unsupported"}
            </strong>
            <small>Service logs</small>
          </span>
          <span
            title={
              inventory?.observed_at
                ? `Service inventory observed ${formatFullTime(inventory.observed_at)}.`
                : "No service inventory observation time has been reported."
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

        <ActionFeedback
          className="localActionFeedback hostServiceActionFeedback"
          message={!pendingAction ? (error ?? status) : null}
          ref={actionFeedbackRef}
          tone={
            error
              ? inventory?.source_job_id
                ? "warning"
                : "danger"
              : pending || loading
                ? "progress"
                : warning
                  ? "warning"
                  : "success"
          }
        />

        <ConsoleDataGrid
          columns={columns}
          defaultPageSize={25}
          empty={
            <div className="emptyState compactEmpty">
              <ServerCog size={22} />
              <strong>
                {selectedClientId
                  ? loading
                    ? "Loading service inventory"
                    : capability?.status === "supported"
                      ? "No services reported"
                      : "Service provider not checked"
                  : "Choose a VPS"}
              </strong>
              <span>
                {selectedClientId
                  ? "Refresh inventory to detect the active provider and read its services."
                  : "Service state is scoped to one host so actions cannot target the wrong VPS."}
              </span>
            </div>
          }
          expandOnRowClick
          getRowId={(service) => service.name}
          itemLabel="services"
          pageResetKey={inventory?.client_id ?? null}
          renderExpandedRow={(service) => (
            <div className="consoleInlineDetailGrid">
              <span>Service</span>
              <strong title={service.name}>{service.name}</strong>
              <span>Description</span>
              <strong title={service.description || undefined}>
                {service.description || "Not reported by provider"}
              </strong>
              <span>Provider</span>
              <strong>{providerLabel(capability?.provider ?? null)}</strong>
              <span>Load state</span>
              <strong>{readableState(service.load_state)}</strong>
              <span>Active state</span>
              <strong>{readableState(service.active_state)}</strong>
              <span>Sub-state</span>
              <strong>{readableState(service.sub_state)}</strong>
              <span>Boot state</span>
              <strong>{readableState(service.enabled_state)}</strong>
              <span>Provider evidence</span>
              <strong>
                {service.state_reason ?? "No additional diagnostic"}
              </strong>
              <span>Observed</span>
              <strong>
                {inventory?.observed_at
                  ? formatFullTime(inventory.observed_at)
                  : "Unknown"}
              </strong>
            </div>
          )}
          rowActions={rowActions}
          rows={services}
          searchPlaceholder="Search service, description, or state"
          showMobileRowActions={false}
          singleExpandedRow
          storageKey="vpsman.remote.hostServices"
          title="Host service inventory"
        />

        <ConfirmationPrompt
          confirmLabel={
            pendingAction ? actionLabel(pendingAction.action) : "Confirm"
          }
          detail="The agent rechecks the provider and both observed states immediately before mutation. A changed snapshot is rejected without applying the action."
          error={pendingAction ? (error ?? undefined) : undefined}
          items={
            pendingAction && selectedAgent && capability?.provider
              ? [
                  { label: "VPS", value: clientLabel(selectedAgent.id) },
                  {
                    label: "Provider",
                    value: providerLabel(capability.provider),
                  },
                  { label: "Service", value: pendingAction.service.name },
                  {
                    label: "Observed runtime",
                    value: readableState(pendingAction.service.active_state),
                  },
                  {
                    label: "Observed boot state",
                    value: readableState(pendingAction.service.enabled_state),
                  },
                  { label: "Action", value: actionLabel(pendingAction.action) },
                ]
              : []
          }
          onCancel={() => {
            if (!pending) {
              setPendingAction(null);
              setError(null);
            }
          }}
          onConfirm={() => pendingAction && void executeAction(pendingAction)}
          open={Boolean(pendingAction)}
          pending={pending}
          title="Confirm service action"
          tone={
            pendingAction && ["stop", "disable"].includes(pendingAction.action)
              ? "danger"
              : "normal"
          }
        />
      </div>

      {logs ? (
        <div ref={logsPanelRef} tabIndex={-1}>
          <ConsoleDetailPanel
            description={`${logs.lines.length} journal line${logs.lines.length === 1 ? "" : "s"}${logs.truncated ? "; bounded output" : ""}`}
            onClose={() => setLogs(null)}
            title={`${logs.service} logs`}
          >
            <pre className="serviceLogOutput">
              {logs.lines.length > 0
                ? logs.lines.join("\n")
                : "No journal entries returned."}
            </pre>
          </ConsoleDetailPanel>
        </div>
      ) : null}
    </div>
  );
}

function readServiceClientRoute(): string | null {
  if (typeof window === "undefined") {
    return null;
  }
  return (
    new URLSearchParams(window.location.search).get("service_client")?.trim() ||
    null
  );
}

function setServiceClientRoute(clientId: string | null) {
  if (typeof window === "undefined") {
    return;
  }
  const url = new URL(window.location.href);
  if (clientId) {
    url.searchParams.set("service_client", clientId);
  } else {
    url.searchParams.delete("service_client");
  }
  pushHistoryEntry(`${url.pathname}${url.search}${url.hash}`);
}

function providerLabel(provider: HostServiceProvider | null): string {
  switch (provider) {
    case "systemd":
      return "systemd";
    case "openrc":
      return "OpenRC";
    case "sysv":
      return "SysV init";
    default:
      return "Not detected";
  }
}

function readableState(value: string): string {
  if (!value) {
    return "Unknown";
  }
  return value
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function serviceTone(state: string): string {
  if (state === "active") return "ok";
  if (state === "failed") return "danger";
  if (state === "inactive") return "neutral";
  return "warn";
}

function actionLabel(action: HostServiceAction): string {
  switch (action) {
    case "start":
      return "Start service";
    case "stop":
      return "Stop service";
    case "restart":
      return "Restart service";
    case "enable":
      return "Enable at boot";
    case "disable":
      return "Disable at boot";
  }
}

function actionPresentParticiple(action: HostServiceAction): string {
  switch (action) {
    case "start":
      return "Starting";
    case "stop":
      return "Stopping";
    case "restart":
      return "Restarting";
    case "enable":
      return "Enabling";
    case "disable":
      return "Disabling";
  }
}

function actionPastTense(action: HostServiceAction): string {
  switch (action) {
    case "start":
      return "Started";
    case "stop":
      return "Stopped";
    case "restart":
      return "Restarted";
    case "enable":
      return "Enabled at boot";
    case "disable":
      return "Disabled at boot";
  }
}
