import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ClipboardCheck,
  ExternalLink,
  PackageCheck,
  RefreshCw,
  RotateCw,
  ShieldAlert,
} from "lucide-react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleActionMenu,
} from "../../components/ConsoleLayout";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { ConsoleDetailPanel } from "../../components/ConsoleDetailPanel";
import { createJobTargetCount, waitForBulkJobTargets } from "../../bulkJobProgress";
import { JOB_COMMAND_TYPE_BY_OPERATION_TYPE } from "../../generated/protocolContracts";
import {
  buildPrivilegeForJobOperation,
  type PrivilegeMaterial,
} from "../../privilege";
import {
  beginSubmission,
  createSubmissionGuard,
  finishSubmission,
} from "../../submissionGuard";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  HostPackageProvider,
  HostPackageUpdateApplyResult,
  HostPackageUpdatePlanRecord,
  HostPackageUpdateRecord,
  JobOperation,
  JobTargetRecord,
} from "../../types";
import { formatCompactTime, formatFullTime, shortHash } from "../../utils";

type OsUpdateRow = {
  agent: AgentView;
  plan: HostPackageUpdatePlanRecord;
};

type ActionFeedbackState = {
  clientId?: string;
  message: string;
  tone: "danger" | "progress" | "success" | "warning";
};

type ApplyReview = {
  row: OsUpdateRow;
};

type ApplyEvidence = {
  clientId: string;
  jobId: string;
  result: HostPackageUpdateApplyResult;
};

export function OsUpdatesPanel({
  agents,
  onCreateJob,
  onDownloadOutputStream,
  onLoadPlan,
  onLoadPlans,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  privilegeMaterial,
}: {
  agents: AgentView[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadOutputStream: (
    jobId: string,
    clientId: string,
    stream: "stdout" | "stderr" | "combined",
  ) => Promise<Blob>;
  onLoadPlan: (clientId: string) => Promise<HostPackageUpdatePlanRecord>;
  onLoadPlans: () => Promise<HostPackageUpdatePlanRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [plans, setPlans] = useState<HostPackageUpdatePlanRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [pendingClientId, setPendingClientId] = useState<string | null>(null);
  const pendingClientIdRef = useRef<string | null>(null);
  const submissionGuardRef = useRef(createSubmissionGuard());
  const [feedback, setFeedback] = useState<ActionFeedbackState | null>(null);
  const [selectedClientId, setSelectedClientId] = useState(readOsUpdateClientRoute);
  const [applyReview, setApplyReview] = useState<ApplyReview | null>(null);
  const [applyEvidence, setApplyEvidence] = useState<ApplyEvidence | null>(null);

  const reloadEvidence = useCallback(async () => {
    setLoading(true);
    try {
      setPlans(await onLoadPlans());
      setFeedback(null);
    } catch (error) {
      setFeedback({
        message:
          error instanceof Error
            ? error.message
            : "OS update evidence is unavailable",
        tone: "danger",
      });
    } finally {
      setLoading(false);
    }
  }, [onLoadPlans]);

  useEffect(() => {
    void reloadEvidence();
  }, [reloadEvidence]);

  useEffect(() => {
    const applyRoute = () => setSelectedClientId(readOsUpdateClientRoute());
    window.addEventListener("popstate", applyRoute);
    window.addEventListener("hashchange", applyRoute);
    return () => {
      window.removeEventListener("popstate", applyRoute);
      window.removeEventListener("hashchange", applyRoute);
    };
  }, []);

  const planByClientId = useMemo(
    () => new Map(plans.map((plan) => [plan.client_id, plan])),
    [plans],
  );
  const rows = useMemo<OsUpdateRow[]>(
    () =>
      agents.map((agent) => ({
        agent,
        plan:
          planByClientId.get(agent.id) ?? emptyPackagePlanRecord(agent.id),
      })),
    [agents, planByClientId],
  );
  const selectedRow = selectedClientId
    ? rows.find((row) => row.agent.id === selectedClientId) ?? null
    : null;

  function replacePlan(next: HostPackageUpdatePlanRecord) {
    setPlans((current) => {
      const withoutClient = current.filter(
        (plan) => plan.client_id !== next.client_id,
      );
      return [...withoutClient, next];
    });
  }

  function openPlan(row: OsUpdateRow) {
    setOsUpdateClientRoute(row.agent.id);
    setSelectedClientId(row.agent.id);
    setApplyEvidence(null);
    setFeedback(null);
  }

  function closePlan() {
    setOsUpdateClientRoute(null);
    setSelectedClientId(null);
    setApplyReview(null);
    setApplyEvidence(null);
  }

  async function runPlan(row: OsUpdateRow, refreshMetadata: boolean) {
    const operation = {
      type: "package_update_plan",
      expected_provider: row.plan.capability?.provider ?? null,
      refresh_metadata: refreshMetadata,
    } as const satisfies JobOperation;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = boundedAgentTimeout(
      row.agent,
      refreshMetadata ? 900 : 180,
    );
    let privilegeAssertion;
    if (refreshMetadata) {
      if (!privilegeMaterial) {
        throw new Error("Unlock privilege before refreshing package metadata.");
      }
      ({ privilegeAssertion } = await buildPrivilegeForJobOperation({
        clientIds: [row.agent.id],
        commandType: command,
        maxTimeoutSecs,
        operation,
        privilegeMaterial,
        selectorExpression: `id:${row.agent.id}`,
      }));
    }
    const job = await onCreateJob({
      argv: [],
      command,
      confirmed: false,
      destructive: false,
      force_unprivileged: false,
      job_id: crypto.randomUUID(),
      max_timeout_secs: maxTimeoutSecs,
      operation,
      privileged: refreshMetadata,
      privilege_assertion: privilegeAssertion,
      selector_expression: `id:${row.agent.id}`,
      target_client_ids: [row.agent.id],
    });
    const completion = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
      maxTimeoutSecs,
      onProgress: (progress) => {
        setFeedback({
          message: `${refreshMetadata ? "Refreshing metadata and checking" : "Checking cached metadata"} on ${agentLabel(row.agent)} · ${progress.terminal}/${progress.total} VPS reported`,
          tone: "progress",
        });
      },
      targetCount: createJobTargetCount(job),
      targets: [row.agent],
    });
    if (completion.progress.successful < completion.progress.total) {
      const reason = completion.progress.failureReasons?.[0]?.reason;
      throw new Error(
        `Package check failed${reason ? `: ${reason}` : "."}`,
      );
    }
    const next = await onLoadPlan(row.agent.id);
    replacePlan(next);
    return { jobId: job.job_id, plan: next };
  }

  async function checkPlan(row: OsUpdateRow, refreshMetadata: boolean) {
    if (pendingClientIdRef.current || row.agent.status !== "online") {
      return;
    }
    const refreshUnavailable = refreshMetadata
      ? metadataRefreshUnavailableReason(row)
      : null;
    if (refreshUnavailable) {
      setFeedback({
        clientId: row.agent.id,
        message: refreshUnavailable,
        tone: "warning",
      });
      return;
    }
    if (refreshMetadata && !privilegeMaterial) {
      setFeedback({
        clientId: row.agent.id,
        message: "Unlock privilege before refreshing package metadata.",
        tone: "warning",
      });
      onOpenPrivilegeUnlock();
      return;
    }
    const submissionKey = `package-update-plan:${row.agent.id}:${row.plan.capability?.provider ?? "unknown"}:${refreshMetadata}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let successful = false;
    pendingClientIdRef.current = row.agent.id;
    setPendingClientId(row.agent.id);
    setFeedback({
      clientId: row.agent.id,
      message: `${refreshMetadata ? "Refreshing package metadata" : "Checking cached package metadata"} on ${agentLabel(row.agent)}...`,
      tone: "progress",
    });
    try {
      const { plan } = await runPlan(row, refreshMetadata);
      successful = true;
      if (plan.capability?.status !== "supported") {
        setFeedback({
          clientId: row.agent.id,
          message:
            plan.capability?.reason ??
            `${agentLabel(row.agent)} did not report a supported native package provider.`,
          tone: "warning",
        });
      } else {
        setFeedback({
          clientId: row.agent.id,
          message: `${agentLabel(row.agent)} reported ${plan.packages.length} available update${plan.packages.length === 1 ? "" : "s"}${refreshMetadata ? " after refreshing repository metadata" : " from its current metadata cache"}.`,
          tone: "success",
        });
      }
    } catch (error) {
      setFeedback({
        clientId: row.agent.id,
        message:
          error instanceof Error ? error.message : "Package check failed",
        tone: "danger",
      });
    } finally {
      pendingClientIdRef.current = null;
      finishSubmission(
        submissionGuardRef.current,
        submissionKey,
        successful,
      );
      setPendingClientId(null);
    }
  }

  function reviewApply(row: OsUpdateRow) {
    const unavailable = applyUnavailableReason(row, privilegeMaterial);
    if (unavailable) {
      setFeedback({
        clientId: row.agent.id,
        message: unavailable,
        tone: "warning",
      });
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
      }
      return;
    }
    setFeedback(null);
    setApplyReview({ row });
  }

  async function applyPlan(review: ApplyReview) {
    const { row } = review;
    const provider = row.plan.capability?.provider;
    const planHash = row.plan.plan_hash;
    if (
      !provider ||
      !planHash ||
      !privilegeMaterial ||
      pendingClientIdRef.current
    ) {
      return;
    }
    const submissionKey = `package-update-apply:${row.agent.id}:${provider}:${planHash}`;
    if (!beginSubmission(submissionGuardRef.current, submissionKey)) return;
    let applyCompleted = false;
    pendingClientIdRef.current = row.agent.id;
    const operation = {
      type: "package_update_apply",
      provider,
      plan_hash: planHash,
    } as const satisfies JobOperation;
    const command = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const maxTimeoutSecs = boundedAgentTimeout(row.agent, 3600);
    setApplyReview(null);
    setApplyEvidence(null);
    setPendingClientId(row.agent.id);
    setFeedback({
      clientId: row.agent.id,
      message: `Applying ${row.plan.packages.length} reviewed update${row.plan.packages.length === 1 ? "" : "s"} on ${agentLabel(row.agent)}...`,
      tone: "progress",
    });
    try {
      const { privilegeAssertion } = await buildPrivilegeForJobOperation({
        clientIds: [row.agent.id],
        commandType: command,
        maxTimeoutSecs,
        operation,
        privilegeMaterial,
        selectorExpression: `id:${row.agent.id}`,
      });
      const job = await onCreateJob({
        argv: [],
        command,
        confirmed: true,
        destructive: true,
        force_unprivileged: false,
        job_id: crypto.randomUUID(),
        max_timeout_secs: maxTimeoutSecs,
        operation,
        privileged: true,
        privilege_assertion: privilegeAssertion,
        selector_expression: `id:${row.agent.id}`,
        target_client_ids: [row.agent.id],
      });
      const completion = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        maxTimeoutSecs,
        onProgress: (progress) => {
          setFeedback({
            clientId: row.agent.id,
            message: `Applying reviewed updates on ${agentLabel(row.agent)} · ${progress.terminal}/${progress.total} VPS reported`,
            tone: "progress",
          });
        },
        targetCount: createJobTargetCount(job),
        targets: [row.agent],
      });
      const result = await readApplyResult(
        await onDownloadOutputStream(job.job_id, row.agent.id, "stdout"),
      );
      applyCompleted = result.completed;
      setApplyEvidence({ clientId: row.agent.id, jobId: job.job_id, result });
      if (
        completion.progress.successful < completion.progress.total ||
        !result.completed
      ) {
        const reason = completion.progress.failureReasons?.[0]?.reason;
        throw new Error(
          reason ??
            `The native package manager left ${result.remaining_packages.length} update${result.remaining_packages.length === 1 ? "" : "s"} unapplied. Open job evidence for its diagnostic.`,
        );
      }
      let refreshedPlan: HostPackageUpdatePlanRecord | null = null;
      try {
        ({ plan: refreshedPlan } = await runPlan(row, false));
      } catch {
        // The accepted apply result remains authoritative; report the failed
        // posture refresh separately instead of hiding successful mutation.
      }
      if (refreshedPlan) {
        setFeedback(null);
      } else {
        setFeedback({
          clientId: row.agent.id,
          message: `${result.applied_package_count} package${result.applied_package_count === 1 ? "" : "s"} applied; ${result.remaining_packages.length} remaining on ${agentLabel(row.agent)}. Reload package posture before another update.${result.reboot_required_after ? " The OS reports that a reboot is required." : " No reboot was started."}`,
          tone: "warning",
        });
      }
    } catch (error) {
      setFeedback({
        clientId: row.agent.id,
        message:
          error instanceof Error ? error.message : "Package update failed",
        tone: "danger",
      });
    } finally {
      pendingClientIdRef.current = null;
      finishSubmission(
        submissionGuardRef.current,
        submissionKey,
        applyCompleted,
      );
      setPendingClientId(null);
    }
  }

  const supportedCount = rows.filter(
    (row) => row.plan.capability?.status === "supported",
  ).length;
  const uncheckedCount = rows.filter((row) => !row.plan.observed_at).length;
  const updateHostCount = rows.filter((row) => row.plan.packages.length > 0).length;
  const updateCount = rows.reduce(
    (count, row) => count + row.plan.packages.length,
    0,
  );
  const issueCount = rows.filter(
    (row) =>
      Boolean(row.plan.evidence_error) ||
      (row.plan.capability !== null &&
        row.plan.capability.status !== "supported") ||
      Boolean(
        row.plan.last_attempt && row.plan.last_attempt.status !== "completed",
      ),
  ).length;

  const rowActions = useMemo<ConsoleDataGridAction<OsUpdateRow>[]>(
    () => [
      {
        description: (selected) =>
          `Open the reviewed package candidate snapshot for ${agentLabel(selected[0]?.agent)}.`,
        disabled: (selected) => !selected[0]?.plan.observed_at,
        icon: <ClipboardCheck size={14} />,
        label: "Review plan",
        onSelect: (selected) => selected[0] && openPlan(selected[0]),
      },
      {
        description: (selected) =>
          `Read ${agentLabel(selected[0]?.agent)} using its current package metadata cache.`,
        disabled: (selected) =>
          Boolean(pendingClientId) || selected[0]?.agent.status !== "online",
        icon: <RotateCw size={14} />,
        label: "Check cached",
        onSelect: (selected) =>
          selected[0] && void checkPlan(selected[0], false),
      },
      {
        description: (selected) =>
          metadataRefreshUnavailableReason(selected[0]) ??
          `Refresh native repository metadata on ${agentLabel(selected[0]?.agent)}, then build a new candidate snapshot.`,
        disabled: (selected) =>
          Boolean(pendingClientId) ||
          selected[0]?.agent.status !== "online" ||
          Boolean(metadataRefreshUnavailableReason(selected[0])),
        icon: <RefreshCw size={14} />,
        label: "Refresh metadata",
        onSelect: (selected) =>
          selected[0] && void checkPlan(selected[0], true),
      },
    ],
    [pendingClientId, privilegeMaterial],
  );

  const columns = useMemo<ConsoleDataGridColumn<OsUpdateRow>[]>(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong title={agentLabel(row.agent)}>{agentLabel(row.agent)}</strong>
            <small title={row.agent.id}>{row.agent.id}</small>
          </span>
        ),
        header: "VPS",
        id: "vps",
        minSize: 160,
        searchValue: (row) => `${row.agent.display_name} ${row.agent.id}`,
        size: 220,
        sortValue: (row) => agentLabel(row.agent),
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{providerLabel(row.plan.capability?.provider ?? null)}</strong>
            <small title={distroLabel(row.plan)}>{distroLabel(row.plan)}</small>
          </span>
        ),
        header: "Platform",
        id: "platform",
        minSize: 130,
        searchValue: (row) =>
          `${row.plan.capability?.provider ?? ""} ${distroLabel(row.plan)}`,
        size: 168,
        sortValue: (row) => distroLabel(row.plan),
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{row.plan.packages.length}</strong>
            <small>
              {!row.plan.observed_at
                ? "Not checked"
                : row.plan.packages.length === 0
                  ? "Current"
                  : "Available"}
            </small>
          </span>
        ),
        header: "Updates",
        id: "updates",
        minSize: 92,
        searchValue: (row) => row.plan.packages.length,
        size: 104,
        sortValue: (row) => row.plan.packages.length,
      },
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong title={row.plan.observed_at ? formatFullTime(row.plan.observed_at) : undefined}>
              {row.plan.observed_at ? formatCompactTime(row.plan.observed_at) : "Never"}
            </strong>
            <small>
              {row.plan.observed_at
                ? row.plan.metadata_refreshed
                  ? "Metadata refreshed"
                  : "Cached metadata"
                : "Not checked"}
            </small>
          </span>
        ),
        header: "Plan evidence",
        id: "evidence",
        minSize: 140,
        searchValue: (row) =>
          `${row.plan.observed_at ?? "never"} ${row.plan.metadata_refreshed ? "refreshed" : "cached"}`,
        size: 170,
        sortValue: (row) => row.plan.observed_at ?? "",
      },
      {
        cell: (row) => {
          const state = packageRowState(row);
          return (
            <span className="historyPrimary">
              <span className={`status ${state.tone}`} title={state.reason}>
                {state.label}
              </span>
              <small title={state.reason}>{state.detail}</small>
            </span>
          );
        },
        header: "State",
        id: "state",
        minSize: 142,
        searchValue: (row) => {
          const state = packageRowState(row);
          return `${state.label} ${state.detail} ${state.reason}`;
        },
        size: 174,
        sortValue: (row) => packageRowState(row).label,
      },
      {
        align: "end",
        cell: (row) => (
          <span onClick={(event) => event.stopPropagation()}>
            <ConsoleActionMenu
              actions={rowActions.map((action) => ({
                disabled: action.disabled?.([row]),
                label: action.label,
                onSelect: () => action.onSelect([row]),
                title: action.description?.([row]),
              }))}
              label={`Actions for ${agentLabel(row.agent)}`}
            />
          </span>
        ),
        enableHiding: false,
        header: "Actions",
        id: "actions",
        minSize: 56,
        size: 64,
        stickyEnd: true,
      },
    ],
    [rowActions],
  );

  return (
    <div className="jobConsoleStack osUpdatesWorkspace">
      <section className="fleetPanel osUpdatesPanel">
        <div className="sectionHeader">
          <div>
            <h2>OS update posture</h2>
            <span>Native package support, reviewed candidates, and explicit application</span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction compactAction"
              disabled={loading || Boolean(pendingClientId)}
              onClick={() => void reloadEvidence()}
              title="Reload stored package-plan evidence without contacting agents"
              type="button"
            >
              <RefreshCw size={14} />
              <span>{loading ? "Loading" : "Reload evidence"}</span>
            </button>
            <ActionFeedback
              className="localActionFeedback"
              message={
                selectedRow && feedback?.clientId === selectedRow.agent.id
                  ? null
                  : feedback?.message ?? null
              }
              tone={feedback?.tone}
            />
          </div>
        </div>

        <div aria-label="OS update fleet summary" className="processSupervisorSummaryStrip">
          <span>
            <strong>{supportedCount} / {rows.length}</strong>
            <small>Supported</small>
          </span>
          <span className={updateHostCount > 0 ? "attention" : undefined}>
            <strong>{updateHostCount}</strong>
            <small>VPS with updates</small>
          </span>
          <span className={updateCount > 0 ? "attention" : undefined}>
            <strong>{updateCount}</strong>
            <small>Packages available</small>
          </span>
          <span className={uncheckedCount > 0 ? "attention" : undefined}>
            <strong>{uncheckedCount}</strong>
            <small>Not checked</small>
          </span>
          <span className={issueCount > 0 ? "attention" : undefined}>
            <strong>{issueCount}</strong>
            <small>Need attention</small>
          </span>
        </div>

        <ConsoleDataGrid
          columns={columns}
          defaultPageSize={25}
          empty={
            <div className="emptyState compactEmpty">
              <PackageCheck size={22} />
              <strong>{loading ? "Loading OS update posture" : "No VPSs available"}</strong>
              <span>Joined VPSs appear here after fleet state loads.</span>
            </div>
          }
          expandOnRowClick
          getRowId={(row) => row.agent.id}
          itemLabel="VPSs"
          mobileRowActionLimit={3}
          renderExpandedRow={(row) => (
            <div className="consoleInlineDetailGrid">
              <span>VPS</span>
              <strong title={row.agent.id}>{agentLabel(row.agent)}</strong>
              <span>Agent state</span>
              <strong>{readableState(row.agent.status)}</strong>
              <span>Package provider</span>
              <strong>{providerLabel(row.plan.capability?.provider ?? null)}</strong>
              <span>Distribution</span>
              <strong title={distroLabel(row.plan)}>{distroLabel(row.plan)}</strong>
              <span>Capability</span>
              <strong>{readableState(row.plan.capability?.status ?? "not_checked")}</strong>
              <span>Reason</span>
              <strong title={packageRowState(row).reason}>{packageRowState(row).reason}</strong>
              <span>Plan hash</span>
              <strong title={row.plan.plan_hash ?? undefined}>
                {row.plan.plan_hash ? shortHash(row.plan.plan_hash) : "No plan"}
              </strong>
            </div>
          )}
          rowActions={rowActions}
          rows={rows}
          searchPlaceholder="Search VPS, distro, provider, or state"
          selectable={false}
          singleExpandedRow
          storageKey="vpsman.automation.osUpdates"
          title="Fleet package posture"
        />
      </section>

      {selectedRow ? (
        <PackagePlanDetail
          applyEvidence={
            applyEvidence?.clientId === selectedRow.agent.id
              ? applyEvidence
              : null
          }
          onApply={() => reviewApply(selectedRow)}
          onCheckCached={() => void checkPlan(selectedRow, false)}
          onClose={closePlan}
          onOpenJobDetails={onOpenJobDetails}
          onRefreshMetadata={() => void checkPlan(selectedRow, true)}
          pending={pendingClientId === selectedRow.agent.id}
          privilegeMaterial={privilegeMaterial}
          row={selectedRow}
          feedback={
            feedback?.clientId === selectedRow.agent.id ? feedback : null
          }
        />
      ) : null}

      <ConfirmationPrompt
        confirmLabel="Apply all updates"
        detail="The agent rechecks the native cached candidate set immediately before mutation. Any provider, package, or version change rejects this request. The package manager still resolves its native dependency transaction, and the operation never starts a reboot."
        items={
          applyReview
            ? [
                { label: "VPS", value: agentLabel(applyReview.row.agent) },
                {
                  label: "Distribution",
                  value: distroLabel(applyReview.row.plan),
                },
                {
                  label: "Provider",
                  value: providerLabel(
                    applyReview.row.plan.capability?.provider ?? null,
                  ),
                },
                {
                  label: "Packages",
                  value: applyReview.row.plan.packages.length,
                },
                {
                  label: "Plan observed",
                  value: applyReview.row.plan.observed_at
                    ? formatFullTime(applyReview.row.plan.observed_at)
                    : "Unknown",
                },
                {
                  label: "Metadata",
                  value: applyReview.row.plan.metadata_refreshed
                    ? "Refreshed before plan"
                    : "Current host cache",
                },
                {
                  label: "Plan hash",
                  title: applyReview.row.plan.plan_hash ?? undefined,
                  value: applyReview.row.plan.plan_hash
                    ? shortHash(applyReview.row.plan.plan_hash)
                    : "Missing",
                },
                { label: "Automatic reboot", value: "Never" },
              ]
            : []
        }
        onCancel={() => {
          if (!pendingClientId) setApplyReview(null);
        }}
        onConfirm={() => applyReview && void applyPlan(applyReview)}
        open={Boolean(applyReview)}
        pending={Boolean(pendingClientId)}
        title="Confirm OS package update"
        tone="warning"
      />
    </div>
  );
}

function PackagePlanDetail({
  applyEvidence,
  feedback,
  onApply,
  onCheckCached,
  onClose,
  onOpenJobDetails,
  onRefreshMetadata,
  pending,
  privilegeMaterial,
  row,
}: {
  applyEvidence: ApplyEvidence | null;
  feedback: ActionFeedbackState | null;
  onApply: () => void;
  onCheckCached: () => void;
  onClose: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onRefreshMetadata: () => void;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  row: OsUpdateRow;
}) {
  const packageColumns = useMemo<ConsoleDataGridColumn<HostPackageUpdateRecord>[]>(
    () => [
      {
        cell: (item) => (
          <span className="historyPrimary">
            <strong title={item.name}>{item.name}</strong>
            <small title={item.architecture ?? undefined}>
              {item.architecture ?? "Native architecture"}
            </small>
          </span>
        ),
        header: "Package",
        id: "package",
        minSize: 170,
        searchValue: (item) => `${item.name} ${item.architecture ?? ""}`,
        size: 230,
        sortValue: (item) => item.name,
      },
      {
        cell: (item) => (
          <span title={item.current_version ?? undefined}>
            {item.current_version ?? "Not reported"}
          </span>
        ),
        header: "Installed",
        id: "installed",
        minSize: 140,
        searchValue: (item) => item.current_version,
        size: 200,
        sortValue: (item) => item.current_version ?? "",
      },
      {
        cell: (item) => (
          <strong title={item.candidate_version}>{item.candidate_version}</strong>
        ),
        header: "Candidate",
        id: "candidate",
        minSize: 140,
        searchValue: (item) => item.candidate_version,
        size: 200,
        sortValue: (item) => item.candidate_version,
      },
      {
        cell: (item) => (
          <span title={item.repository ?? undefined}>
            {item.repository ?? "Native repository"}
          </span>
        ),
        header: "Repository",
        id: "repository",
        minSize: 140,
        searchValue: (item) => item.repository,
        size: 220,
        sortValue: (item) => item.repository ?? "",
      },
    ],
    [],
  );
  const applyUnavailable = applyUnavailableReason(row, privilegeMaterial);
  const refreshUnavailable = metadataRefreshUnavailableReason(row);
  const state = packageRowState(row);
  return (
    <ConsoleDetailPanel
      actions={
        <>
          <button
            className="secondaryAction"
            disabled={pending || row.agent.status !== "online"}
            onClick={onCheckCached}
            title="Build a reviewed candidate snapshot from the host's current package metadata cache"
            type="button"
          >
            <RotateCw size={14} />
            Check cached
          </button>
          <button
            className="secondaryAction"
            disabled={
              pending ||
              row.agent.status !== "online" ||
              Boolean(refreshUnavailable)
            }
            onClick={onRefreshMetadata}
            title={
              refreshUnavailable ??
              "Refresh native repository metadata, then build a new candidate snapshot; privilege required"
            }
            type="button"
          >
            <RefreshCw size={14} />
            Refresh metadata
          </button>
          {row.plan.source_job_id ? (
            <button
              className="secondaryAction"
              onClick={() => onOpenJobDetails(row.plan.source_job_id!)}
              title={`Open package-plan job ${row.plan.source_job_id}`}
              type="button"
            >
              <ExternalLink size={14} />
              Plan evidence
            </button>
          ) : null}
          <button
            className="primaryAction"
            disabled={Boolean(applyUnavailable) || pending}
            onClick={onApply}
            title={applyUnavailable ?? "Review and apply this full native package candidate set"}
            type="button"
          >
            <PackageCheck size={14} />
            Apply all updates
          </button>
        </>
      }
      description={`${providerLabel(row.plan.capability?.provider ?? null)} · ${row.plan.packages.length} available update${row.plan.packages.length === 1 ? "" : "s"}`}
      onClose={onClose}
      title={`${agentLabel(row.agent)} package plan`}
    >
      <div className="consoleInlineDetailGrid osUpdatePlanSummary">
        <span>Capability</span>
        <strong className={`status ${state.tone}`} title={state.reason}>
          {state.label}
        </strong>
        <span>Distribution</span>
        <strong title={distroLabel(row.plan)}>{distroLabel(row.plan)}</strong>
        <span>Provider</span>
        <strong>{providerLabel(row.plan.capability?.provider ?? null)}</strong>
        <span>Observed</span>
        <strong title={row.plan.observed_at ? formatFullTime(row.plan.observed_at) : undefined}>
          {row.plan.observed_at ? formatCompactTime(row.plan.observed_at) : "Never"}
        </strong>
        <span>Metadata source</span>
        <strong>
          {row.plan.metadata_refreshed
            ? "Refreshed repository metadata"
            : row.plan.observed_at
              ? "Current host cache"
              : "Not checked"}
        </strong>
        <span>Plan hash</span>
        <strong title={row.plan.plan_hash ?? undefined}>
          {row.plan.plan_hash ? shortHash(row.plan.plan_hash) : "No plan"}
        </strong>
        <span>Reboot already required</span>
        <strong>
          {row.plan.reboot_required_before === null
            ? "Not reported by this distribution"
            : row.plan.reboot_required_before
              ? "Yes"
              : "No"}
        </strong>
        <span>Automatic reboot</span>
        <strong>Never</strong>
      </div>

      {row.plan.truncated ? (
        <ActionFeedback
          className="localActionFeedback"
          message="The package plan exceeded the review limit. Application is disabled; inspect the host package manager and reduce the plan before retrying."
          tone="warning"
        />
      ) : null}
      {row.plan.evidence_error ? (
        <ActionFeedback
          className="localActionFeedback"
          message={`Stored package evidence is unreadable (${readableState(row.plan.evidence_error)}). Run a new check before applying updates.`}
          tone="danger"
        />
      ) : null}
      {row.plan.capability?.status === "supported" &&
      row.plan.capability.reason ? (
        <ActionFeedback
          className="localActionFeedback"
          message={row.plan.capability.reason}
          tone="warning"
        />
      ) : null}
      {applyUnavailable && row.plan.packages.length > 0 ? (
        <ActionFeedback
          className="localActionFeedback"
          message={applyUnavailable}
          tone="warning"
        />
      ) : null}

      <ConsoleDataGrid
        columns={packageColumns}
        defaultPageSize={25}
        empty={
          <div className="emptyState compactEmpty">
            {row.plan.observed_at ? <PackageCheck size={22} /> : <ShieldAlert size={22} />}
            <strong>
              {row.plan.observed_at ? "No updates in this plan" : "No package plan"}
            </strong>
            <span>
              {row.plan.observed_at
                ? "The native package manager reported no available full-system updates."
                : "Check cached metadata or refresh repository metadata to create a reviewed candidate snapshot."}
            </span>
          </div>
        }
        getRowId={(item) =>
          `${item.name}:${item.architecture ?? "native"}:${item.candidate_version}`
        }
        itemLabel="packages"
        mobileFieldLayout="stacked"
        rows={row.plan.packages}
        searchPlaceholder="Search package, version, architecture, or repository"
        selectable={false}
        storageKey={`vpsman.automation.osUpdatePlan.${row.agent.id}`}
        title="Reviewed package candidates"
      />

      {applyEvidence ? (
        <div className="osUpdateApplyEvidence">
          {!feedback ? (
            <ActionFeedback
              className="localActionFeedback"
              message={`${applyEvidence.result.applied_package_count} package${applyEvidence.result.applied_package_count === 1 ? "" : "s"} applied; ${applyEvidence.result.remaining_packages.length} remaining on ${agentLabel(row.agent)}. Package posture refreshed.${applyEvidence.result.reboot_required_after ? " The OS reports that a reboot is required; no reboot was started." : " No reboot was started."}`}
              tone={applyEvidence.result.completed ? "success" : "warning"}
            />
          ) : null}
          <button
            className="secondaryAction compactAction"
            onClick={() => onOpenJobDetails(applyEvidence.jobId)}
            title={`Open update job ${applyEvidence.jobId}`}
            type="button"
          >
            <ExternalLink size={14} />
            Update evidence
          </button>
        </div>
      ) : null}
      <ActionFeedback
        className="localActionFeedback"
        message={feedback?.message ?? null}
        tone={feedback?.tone}
      />
    </ConsoleDetailPanel>
  );
}

function applyUnavailableReason(
  row: OsUpdateRow,
  privilegeMaterial: PrivilegeMaterial | null,
): string | null {
  if (row.agent.status !== "online") {
    return `${agentLabel(row.agent)} is ${readableState(row.agent.status)}; refresh when it is online.`;
  }
  if (row.plan.evidence_error) {
    return "Run a new package check because stored plan evidence is unreadable.";
  }
  if (row.plan.capability?.status !== "supported") {
    return (
      row.plan.capability?.reason ??
      "Run a package check and resolve native provider support before applying updates."
    );
  }
  if (!row.plan.capability.can_apply) {
    return row.plan.capability.reason ?? "This agent cannot apply package updates.";
  }
  if (!row.plan.plan_hash) {
    return "Run a package check before applying updates.";
  }
  if (row.plan.truncated) {
    return "This plan exceeds the review limit and cannot be applied from the console.";
  }
  if (row.plan.packages.length === 0) {
    return "This reviewed candidate snapshot contains no available updates.";
  }
  if (!privilegeMaterial) {
    return "Unlock privilege before applying OS updates.";
  }
  return null;
}

function metadataRefreshUnavailableReason(
  row: OsUpdateRow | undefined,
): string | null {
  if (!row?.plan.capability) return null;
  if (row.plan.capability.status !== "supported") {
    return (
      row.plan.capability.reason ??
      "Resolve native package-provider support before refreshing metadata."
    );
  }
  if (!row.plan.capability.can_refresh_metadata) {
    return (
      row.plan.capability.reason ??
      "This native package provider cannot refresh metadata as a separate safe action."
    );
  }
  return null;
}

function packageRowState(row: OsUpdateRow): {
  detail: string;
  label: string;
  reason: string;
  tone: "danger" | "neutral" | "ok" | "warn";
} {
  if (row.plan.evidence_error) {
    return {
      detail: "Evidence unreadable",
      label: "Error",
      reason: `Stored package evidence could not be read (${readableState(row.plan.evidence_error)}). Run a new check.`,
      tone: "danger",
    };
  }
  if (row.plan.last_attempt && row.plan.last_attempt.status !== "completed") {
    return {
      detail: "Last check failed",
      label: "Attention",
      reason:
        row.plan.last_attempt.message ??
        `The latest package check ended ${readableState(row.plan.last_attempt.status)}; stored successful evidence may still be shown.`,
      tone: "warn",
    };
  }
  if (!row.plan.observed_at) {
    return {
      detail: "Run first check",
      label: "Unchecked",
      reason: "No package provider or update plan has been observed for this VPS.",
      tone: "neutral",
    };
  }
  if (row.plan.capability?.status !== "supported") {
    return {
      detail: readableState(row.plan.capability?.status ?? "unsupported"),
      label: "Unsupported",
      reason:
        row.plan.capability?.reason ??
        "The agent did not confirm a supported native package provider.",
      tone: "warn",
    };
  }
  if (row.plan.truncated) {
    return {
      detail: "Review limit exceeded",
      label: "Blocked",
      reason: "The full native plan is too large for safe console review and application.",
      tone: "warn",
    };
  }
  if (row.plan.packages.length > 0) {
    return {
      detail: `${row.plan.packages.length} available`,
      label: "Updates",
      reason: `${row.plan.packages.length} package update candidate${row.plan.packages.length === 1 ? " is" : "s are"} available in the reviewed snapshot.`,
      tone: "warn",
    };
  }
  return {
    detail: "No updates",
    label: "Current",
    reason: "The latest native package candidate snapshot contains no available updates.",
    tone: "ok",
  };
}

function emptyPackagePlanRecord(clientId: string): HostPackageUpdatePlanRecord {
  return {
    capability: null,
    client_id: clientId,
    evidence_error: null,
    last_attempt: null,
    metadata_refresh_requested: false,
    metadata_refreshed: false,
    observed_at: null,
    packages: [],
    plan_hash: null,
    reboot_required_before: null,
    source_job_id: null,
    truncated: false,
  };
}

function agentLabel(agent: AgentView | undefined): string {
  if (!agent) return "selected VPS";
  return agent.display_name.trim() || agent.id;
}

function distroLabel(plan: HostPackageUpdatePlanRecord): string {
  const distro = plan.capability?.distro_id.trim();
  if (!distro) return "Not detected";
  const label = distroDisplayName(distro);
  return plan.capability?.distro_version
    ? `${label} ${plan.capability.distro_version}`
    : label;
}

function distroDisplayName(distro: string): string {
  switch (distro.toLowerCase()) {
    case "almalinux":
      return "AlmaLinux";
    case "arch":
      return "Arch Linux";
    case "centos":
      return "CentOS";
    case "debian":
      return "Debian";
    case "fedora":
      return "Fedora";
    case "ol":
      return "Oracle Linux";
    case "rhel":
      return "RHEL";
    case "rocky":
      return "Rocky Linux";
    case "ubuntu":
      return "Ubuntu";
    default:
      return distro;
  }
}

function providerLabel(provider: HostPackageProvider | null): string {
  switch (provider) {
    case "apt":
      return "APT";
    case "dnf":
      return "DNF";
    case "yum":
      return "YUM";
    case "pacman":
      return "Pacman";
    default:
      return "Not detected";
  }
}

function readableState(value: string): string {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function boundedAgentTimeout(agent: AgentView, requested: number): number {
  return Math.max(
    1,
    Math.min(requested, agent.capabilities.max_job_timeout_secs || requested),
  );
}

async function readApplyResult(blob: Blob): Promise<HostPackageUpdateApplyResult> {
  let parsed: HostPackageUpdateApplyResult;
  try {
    parsed = JSON.parse(await blob.text()) as HostPackageUpdateApplyResult;
  } catch {
    throw new Error("Agent returned invalid package-update evidence.");
  }
  if (
    parsed.type !== "package_update_apply" ||
    typeof parsed.accepted_plan_hash !== "string" ||
    !Array.isArray(parsed.remaining_packages)
  ) {
    throw new Error("Agent returned incomplete package-update evidence.");
  }
  return parsed;
}

function readOsUpdateClientRoute(): string | null {
  if (typeof window === "undefined") return null;
  return (
    new URLSearchParams(window.location.search).get("os_update_client")?.trim() ||
    null
  );
}

function setOsUpdateClientRoute(clientId: string | null) {
  if (typeof window === "undefined") return;
  const url = new URL(window.location.href);
  const current = url.searchParams.get("os_update_client")?.trim() || null;
  if (current === clientId) return;
  if (clientId) {
    url.searchParams.set("os_update_client", clientId);
  } else {
    url.searchParams.delete("os_update_client");
  }
  window.history.pushState(null, "", `${url.pathname}${url.search}${url.hash}`);
}
