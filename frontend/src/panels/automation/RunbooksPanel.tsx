import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import {
  ChevronDown,
  ClipboardList,
  ExternalLink,
  Play,
  RefreshCcw,
  Search,
} from "lucide-react";
import { useMemo, useState } from "react";
import type { JobDispatchPresetInput } from "../../jobDispatchPreset";
import { agentsMatchingExpression } from "../../searchExpression";
import type {
  AgentView,
  CommandTemplateRecord,
  JobHistoryRecord,
  JobOperation,
  JsonValue,
} from "../../types";
import { formatCompactTime, formatTime } from "../../utils";
import type { DispatchMode, SupervisorAction } from "../jobDispatchModel";

type RunbookFilter = "all" | "built_in" | "custom";

type RunbookCapability =
  | {
      dispatchable: true;
      mode: DispatchMode;
      supervisorAction?: SupervisorAction;
    }
  | {
      dispatchable: false;
      reason: string;
      route: "terminal" | "files" | "transfers" | "network" | "unsupported";
    };

type RunbooksPanelProps = {
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  jobs: JobHistoryRecord[];
  loading: boolean;
  onOpenDispatchPreset: (preset: JobDispatchPresetInput) => void;
  onOpenJobsDispatch: () => void;
  onOpenRemoteTerminal: () => void;
  onOpenSchedules: () => void;
  onRefresh: () => void;
};

export function RunbooksPanel({
  agents,
  commandTemplates,
  jobs,
  loading,
  onOpenDispatchPreset,
  onOpenJobsDispatch,
  onOpenRemoteTerminal,
  onOpenSchedules,
  onRefresh,
}: RunbooksPanelProps) {
  const [filter, setFilter] = useState<RunbookFilter>("all");
  const [query, setQuery] = useState("");
  const runbooks = useMemo(
    () =>
      commandTemplates.map((template) => {
        const selectorExpression = selectorForTemplateScope(template);
        const matchingTargets = selectorExpression.trim()
          ? agentsMatchingExpression(agents, selectorExpression).length
          : agents.length;
        const lastRun = latestRunForTemplate(template, jobs);
        return {
          capability: capabilityForOperation(template.operation),
          lastEvidence: lastRunEvidenceForTemplate(
            template,
            lastRun,
            jobs.length,
          ),
          matchingTargets,
          operationKind: operationTypeLabel(template.command_type),
          requiredParameters: requiredParametersForOperation(
            template.operation,
          ),
          lastRun,
          operationSummary: operationSummary(template.operation),
          selectorExpression,
          template,
        };
      }),
    [agents, commandTemplates, jobs],
  );
  const visibleRunbooks = runbooks.filter((runbook) => {
    if (filter === "built_in" && !runbook.template.built_in) {
      return false;
    }
    if (filter === "custom" && runbook.template.built_in) {
      return false;
    }
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) {
      return true;
    }
    return [
      runbook.template.name,
      runbook.template.command_type,
      runbook.template.display_group ?? "",
      runbook.template.scope_kind,
      runbook.template.scope_value ?? "",
      runbook.operationSummary,
      runbook.requiredParameters.join(" "),
    ]
      .join(" ")
      .toLocaleLowerCase()
      .includes(needle);
  });
  const dispatchableCount = runbooks.filter(
    (runbook) => runbook.capability.dispatchable,
  ).length;
  const customCount = runbooks.filter(
    (runbook) => !runbook.template.built_in,
  ).length;
  const latestRun = latestLoadedJob(jobs);

  function reviewRunbook(runbook: (typeof runbooks)[number]) {
    if (!runbook.capability.dispatchable) {
      return;
    }
    onOpenDispatchPreset({
      commandTemplateId: runbook.template.id,
      maxTimeoutSecs: defaultMaxTimeout(runbook.template.defaults),
      mode: runbook.capability.mode,
      selectorExpression: runbook.selectorExpression,
      supervisorAction: runbook.capability.supervisorAction,
      ...processSupervisorPreset(runbook.template.operation),
    });
  }

  function openRunbookManagement(runbook: (typeof runbooks)[number]) {
    if (runbook.capability.dispatchable) {
      reviewRunbook(runbook);
      return;
    }
    onOpenJobsDispatch();
  }

  return (
    <section className="workspace singleColumn runbooksWorkspace">
      <section className="fleetPanel runbooksPanel">
        <div className="sectionHeader">
          <div>
            <h2>Runbooks</h2>
            <span>
              {loading
                ? "Refreshing reviewed operation catalog"
                : `${runbooks.length} reusable operations`}
            </span>
          </div>
          <div className="inlineActions">
            <button
              className="secondaryAction compactAction"
              onClick={onOpenSchedules}
              title="Open scheduled runbook and command executions."
              type="button"
            >
              <ClipboardList size={16} />
              Schedules
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={loading}
              onClick={onRefresh}
              title="Refresh the reviewed operation catalog and latest job evidence."
              type="button"
            >
              <RefreshCcw size={16} />
              Refresh
            </button>
          </div>
        </div>

        <div className="runbookSummary" aria-label="Runbook catalog summary">
          <RunbookMetric
            label="Runbooks"
            value={String(runbooks.length)}
            detail="template-backed reviewed operations"
          />
          <RunbookMetric
            label="Ready"
            value={String(dispatchableCount)}
            detail="can open dispatch prefilled"
          />
          <RunbookMetric
            label="Custom"
            value={String(customCount)}
            detail="operator-defined templates"
          />
          <RunbookMetric
            label="Latest job"
            value={latestRun ? jobResultLabel(latestRun.status) : "none"}
            detail={
              latestRun
                ? `${operationTypeLabel(latestRun.command_type)} · ${formatCompactTime(
                    latestRun.completed_at ?? latestRun.created_at,
                  )}`
                : "no job evidence loaded"
            }
          />
        </div>

        <div className="runbookToolbar" aria-label="Runbook filters">
          <label
            className="runbookSearch"
            title="Search runbooks by name, scope, operation, display group, or required review input."
          >
            <Search size={16} />
            <input
              aria-label="Search runbooks"
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search runbooks"
              type="search"
              value={query}
            />
          </label>
          <div className="segmented" role="group" aria-label="Runbook kind">
            {[
              ["all", "All", "Show built-in and custom runbooks."],
              ["built_in", "Built-in", "Show shipped runbooks."],
              ["custom", "Custom", "Show operator-defined runbooks."],
            ].map(([value, label, title]) => (
              <button
                aria-pressed={filter === value}
                className={filter === value ? "selected" : ""}
                key={value}
                onClick={() => setFilter(value as RunbookFilter)}
                title={title}
                type="button"
              >
                {label}
              </button>
            ))}
          </div>
          <button
            className="secondaryAction compactAction"
            onClick={onOpenJobsDispatch}
            title="Open Jobs / Dispatch to create, edit, or run command templates."
            type="button"
          >
            <ExternalLink size={15} />
            Jobs / Dispatch
          </button>
        </div>

        {visibleRunbooks.length > 0 ? (
          <div className="runbookCatalogGrid" aria-label="Runbook catalog">
            {visibleRunbooks.map((runbook) => (
              <article className="runbookCard" key={runbook.template.id}>
                <div className="runbookCardHeader">
                  <div>
                    <h3>{runbook.template.name}</h3>
                    <span>{runbook.operationSummary}</span>
                  </div>
                  <span
                    className={
                      runbook.template.built_in ? "status info" : "status ok"
                    }
                  >
                    {runbook.template.built_in ? "Built-in" : "Custom"}
                  </span>
                </div>

                <dl className="runbookMetaGrid">
                  <div>
                    <dt>Scope</dt>
                    <dd>{scopeLabel(runbook.template)}</dd>
                  </div>
                  <div>
                    <dt>Targets</dt>
                    <dd>{runbook.matchingTargets} matched</dd>
                  </div>
                  <div>
                    <dt>Operation</dt>
                    <dd>{runbook.operationKind}</dd>
                  </div>
                  <div>
                    <dt>Last result</dt>
                    <dd>
                      <span
                        className={`runbookLastResult ${runbook.lastEvidence.tone}`}
                        title={runbook.lastEvidence.title}
                      >
                        <strong>{runbook.lastEvidence.label}</strong>
                        <small>{runbook.lastEvidence.detail}</small>
                      </span>
                    </dd>
                  </div>
                </dl>

                <details className="runbookRequirementsDisclosure">
                  <summary>
                    Review inputs
                    <span>{runbook.requiredParameters.length}</span>
                  </summary>
                  <div
                    className="runbookRequirementList"
                    aria-label={`Required review for ${runbook.template.name}`}
                  >
                    {runbook.requiredParameters.map((item) => (
                      <span key={item}>{item}</span>
                    ))}
                  </div>
                </details>

                <div className="runbookCardFooter">
                  <div className="runbookPrimaryActions">
                    {runbook.capability.dispatchable ? (
                      <button
                        className="primaryAction compactAction"
                        onClick={() => reviewRunbook(runbook)}
                        title={`Open Dispatch with ${runbook.template.name} prefilled.`}
                        type="button"
                      >
                        <Play size={15} />
                        Run
                      </button>
                    ) : (
                      <button
                        className="secondaryAction compactAction"
                        onClick={
                          runbook.capability.route === "terminal"
                            ? onOpenRemoteTerminal
                            : onOpenJobsDispatch
                        }
                        title={
                          runbook.capability.route === "terminal"
                            ? "Open Remote Operations / Terminal for terminal lifecycle actions."
                            : "Open the owner workflow for this operation type."
                        }
                        type="button"
                      >
                        <ExternalLink size={15} />
                        {runbook.capability.route === "terminal"
                          ? "Remote terminal"
                          : "Open owner"}
                      </button>
                    )}
                    {!runbook.template.built_in && (
                      <RunbookManageMenu
                        onDelete={() => openRunbookManagement(runbook)}
                        onDuplicate={() => openRunbookManagement(runbook)}
                        onEdit={() => openRunbookManagement(runbook)}
                        templateName={runbook.template.name}
                      />
                    )}
                  </div>
                  <span>
                    {runbook.capability.dispatchable
                      ? "Opens Dispatch with scope and template prefilled."
                      : runbook.capability.reason}
                  </span>
                </div>
              </article>
            ))}
          </div>
        ) : (
          <div className="emptyState compactEmpty" role="status">
            <ClipboardList size={22} />
            <strong>No runbooks match</strong>
            <span>
              Adjust the filter or create command templates in Jobs / Dispatch.
            </span>
          </div>
        )}
      </section>
    </section>
  );
}

function RunbookManageMenu({
  onDelete,
  onDuplicate,
  onEdit,
  templateName,
}: {
  onDelete: () => void;
  onDuplicate: () => void;
  onEdit: () => void;
  templateName: string;
}) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label={`Manage ${templateName}`}
          className="secondaryAction compactAction"
          title={`Manage custom runbook ${templateName}.`}
          type="button"
        >
          Manage
          <ChevronDown size={14} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="consoleMenu"
          collisionPadding={12}
          sideOffset={6}
        >
          <DropdownMenu.Item className="consoleMenuItem" onSelect={onEdit}>
            Edit in Dispatch
          </DropdownMenu.Item>
          <DropdownMenu.Item className="consoleMenuItem" onSelect={onDuplicate}>
            Duplicate in Dispatch
          </DropdownMenu.Item>
          <DropdownMenu.Separator className="consoleMenuSeparator" />
          <DropdownMenu.Item
            className="consoleMenuItem danger"
            onSelect={onDelete}
          >
            Delete in Dispatch
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function RunbookMetric({
  detail,
  label,
  value,
}: {
  detail: string;
  label: string;
  value: string;
}) {
  return (
    <div className="runbookMetric" title={`${label}: ${value}. ${detail}`}>
      <small>{label}</small>
      <strong>{value}</strong>
      <span>{detail}</span>
    </div>
  );
}

function latestRunForTemplate(
  template: CommandTemplateRecord,
  jobs: JobHistoryRecord[],
) {
  return (
    jobs
      .filter((job) => job.command_type === template.command_type)
      .sort((left, right) => jobTimeMs(right) - jobTimeMs(left))[0] ?? null
  );
}

function selectorForTemplateScope(template: CommandTemplateRecord): string {
  const value = template.scope_value?.trim() ?? "";
  if (template.scope_kind === "global" || !value) {
    return "";
  }
  if (template.scope_kind === "provider") {
    return value.startsWith("provider:") ? value : `provider:${value}`;
  }
  if (template.scope_kind === "tag") {
    return value.startsWith("tag:") ? value : `tag:${value}`;
  }
  if (template.scope_kind === "client") {
    return value.startsWith("id:") ? value : `id:${value}`;
  }
  return value;
}

function scopeLabel(template: CommandTemplateRecord): string {
  if (template.scope_kind === "global") {
    return "All scoped VPS";
  }
  return `${template.scope_kind}:${template.scope_value ?? "-"}`;
}

function capabilityForOperation(operation: JobOperation): RunbookCapability {
  switch (operation.type) {
    case "shell":
      return { dispatchable: true, mode: "shell" };
    case "shell_script":
      return { dispatchable: true, mode: "shell_script" };
    case "file_pull":
      return { dispatchable: true, mode: "file_pull" };
    case "backup":
      return { dispatchable: true, mode: "backup" };
    case "agent_update":
      return { dispatchable: true, mode: "agent_update" };
    case "agent_update_check":
      return { dispatchable: true, mode: "agent_update_check" };
    case "agent_update_activate":
      return { dispatchable: true, mode: "agent_update_activate" };
    case "agent_update_rollback":
      return { dispatchable: true, mode: "agent_update_rollback" };
    case "user_sessions":
      return { dispatchable: true, mode: "user_sessions" };
    case "process_list":
      return { dispatchable: true, mode: "process_list" };
    case "process_start":
      return {
        dispatchable: true,
        mode: "process_supervisor",
        supervisorAction: "start",
      };
    case "process_stop":
      return {
        dispatchable: true,
        mode: "process_supervisor",
        supervisorAction: "stop",
      };
    case "process_restart":
      return {
        dispatchable: true,
        mode: "process_supervisor",
        supervisorAction: "restart",
      };
    case "process_status":
      return {
        dispatchable: true,
        mode: "process_supervisor",
        supervisorAction: "status",
      };
    case "process_logs":
      return {
        dispatchable: true,
        mode: "process_supervisor",
        supervisorAction: "logs",
      };
    case "terminal_open":
    case "terminal_input":
    case "terminal_poll":
    case "terminal_resize":
    case "terminal_close":
      return {
        dispatchable: false,
        reason: "Terminal lifecycle belongs in Remote Operations.",
        route: "terminal",
      };
    case "file_transfer_start":
    case "file_transfer_chunk":
    case "file_transfer_commit":
    case "file_transfer_abort":
    case "file_transfer_download_start":
    case "file_transfer_download_chunk":
      return {
        dispatchable: false,
        reason: "Transfer sessions belong in Remote Operations / Transfers.",
        route: "transfers",
      };
    default:
      return {
        dispatchable: false,
        reason: "Open the owner workflow for this operation type.",
        route: "unsupported",
      };
  }
}

function requiredParametersForOperation(operation: JobOperation): string[] {
  switch (operation.type) {
    case "shell":
      return [
        "Target scope",
        operation.pty ? "PTY session" : "Command arguments",
        "Timeout",
      ];
    case "shell_script":
      return ["Target scope", "Script body", "Timeout"];
    case "file_pull":
      return [
        "Target scope",
        `Path ${operation.path}`,
        operation.follow_symlinks
          ? "Follow symlinks"
          : "Do not follow symlinks",
      ];
    case "backup":
      return [
        "Target scope",
        operation.include_config ? "Include config" : "Paths only",
        operation.paths.length ? `${operation.paths.length} paths` : "No paths",
      ];
    case "agent_update":
      return ["Target scope", "Artifact URL", "SHA-256"];
    case "agent_update_check":
      return [
        "Target scope",
        "Manifest URL",
        operation.activate ? "Activation review" : "Check only",
      ];
    case "agent_update_activate":
      return [
        "Target scope",
        "Staged SHA-256",
        operation.restart_agent ? "Restart agent" : "No restart",
      ];
    case "agent_update_rollback":
      return [
        "Target scope",
        operation.rollback_sha256_hex ? "Rollback SHA-256" : "Latest rollback",
      ];
    case "user_sessions":
      return ["Target scope", "Session inventory"];
    case "process_list":
      return ["Target scope", `${operation.limit} process limit`];
    case "process_start":
      return ["Target scope", `Process ${operation.name}`, "Start"];
    case "process_stop":
      return ["Target scope", `Process ${operation.name}`, "Stop"];
    case "process_restart":
      return ["Target scope", `Process ${operation.name}`, "Restart"];
    case "process_status":
      return [
        "Target scope",
        operation.name ? `Process ${operation.name}` : "All processes",
      ];
    case "process_logs":
      return [
        "Target scope",
        `Process ${operation.name}`,
        `${operation.max_bytes} log bytes`,
      ];
    default:
      return ["Owner workflow", operationTypeLabel(operation.type)];
  }
}

function operationSummary(operation: JobOperation): string {
  switch (operation.type) {
    case "shell":
      return operation.argv.join(" ") || "shell argv";
    case "shell_script":
      return "shell script";
    case "file_pull":
      return `download ${operation.path}`;
    case "backup":
      return `backup ${operation.include_config ? "config and paths" : "paths"}`;
    case "agent_update":
      return "manual agent update";
    case "agent_update_check":
      return "check agent update";
    case "agent_update_activate":
      return "activate staged agent";
    case "agent_update_rollback":
      return "rollback agent update";
    case "user_sessions":
      return "list user sessions";
    case "process_list":
      return "list processes";
    case "process_start":
      return `start ${operation.name}`;
    case "process_stop":
      return `stop ${operation.name}`;
    case "process_restart":
      return `restart ${operation.name}`;
    case "process_status":
      return operation.name ? `status ${operation.name}` : "process status";
    case "process_logs":
      return `logs ${operation.name}`;
    case "terminal_open":
      return "terminal open";
    default:
      return operation.type;
  }
}

type RunbookLastEvidence = {
  detail: string;
  label: string;
  title?: string;
  tone: "ok" | "warn" | "neutral";
};

function lastRunEvidenceForTemplate(
  template: CommandTemplateRecord,
  lastRun: JobHistoryRecord | null,
  loadedJobCount: number,
): RunbookLastEvidence {
  if (lastRun) {
    const timestamp = lastRun.completed_at ?? lastRun.created_at;
    return {
      detail: `${formatCompactTime(timestamp)} · ${lastRun.target_count} ${lastRun.target_count === 1 ? "VPS" : "VPSs"}`,
      label: jobResultLabel(lastRun.status),
      title: `Job ${lastRun.id} · ${formatTime(timestamp)}`,
      tone: jobResultTone(lastRun.status),
    };
  }
  if (loadedJobCount > 0) {
    return {
      detail: `No ${operationTypeLabel(template.command_type).toLowerCase()} execution in loaded history`,
      label: "No loaded run",
      tone: "neutral",
    };
  }
  return {
    detail: "Refresh or run this runbook to create evidence",
    label: "No job evidence",
    tone: "neutral",
  };
}

function latestLoadedJob(jobs: JobHistoryRecord[]): JobHistoryRecord | null {
  return (
    [...jobs].sort((left, right) => jobTimeMs(right) - jobTimeMs(left))[0] ??
    null
  );
}

function jobTimeMs(job: JobHistoryRecord): number {
  const value = Date.parse(job.completed_at ?? job.created_at);
  return Number.isFinite(value) ? value : 0;
}

function operationTypeLabel(commandType: string): string {
  switch (commandType) {
    case "shell_argv":
      return "Shell command";
    case "scheduled_shell_argv":
      return "Scheduled shell command";
    case "shell_pty":
      return "Interactive shell";
    case "shell_script":
      return "Shell script";
    case "file_pull":
      return "File download";
    case "backup":
      return "Backup";
    case "agent_update":
      return "Agent update";
    case "agent_update_check":
      return "Agent update check";
    case "agent_update_activate":
      return "Agent update activation";
    case "agent_update_rollback":
      return "Agent update rollback";
    case "user_sessions":
      return "User session inventory";
    case "process_list":
      return "Process inventory";
    case "process_start":
      return "Start process";
    case "process_stop":
      return "Stop process";
    case "process_restart":
      return "Restart process";
    case "process_status":
      return "Process status";
    case "process_logs":
      return "Process logs";
    default:
      return commandType.replace(/_/g, " ");
  }
}

function jobResultLabel(status: string): string {
  switch (status) {
    case "completed":
      return "Succeeded";
    case "failed":
      return "Failed";
    case "running":
      return "Running";
    case "queued":
      return "Queued";
    case "canceled":
    case "cancelled":
      return "Canceled";
    default:
      return status.replace(/_/g, " ");
  }
}

function jobResultTone(status: string): RunbookLastEvidence["tone"] {
  if (status === "completed") {
    return "ok";
  }
  if (status === "failed" || status === "canceled" || status === "cancelled") {
    return "warn";
  }
  return "neutral";
}

function defaultMaxTimeout(defaults: JsonValue): number | undefined {
  if (!defaults || typeof defaults !== "object" || Array.isArray(defaults)) {
    return undefined;
  }
  const value = (defaults as Record<string, JsonValue>).max_timeout_secs;
  return typeof value === "number" ? value : undefined;
}

function processSupervisorPreset(
  operation: JobOperation,
): Partial<JobDispatchPresetInput> {
  switch (operation.type) {
    case "process_start":
      return {
        supervisorArgv: formatArgvForInput(operation.argv),
        supervisorCwd: operation.cwd ?? "",
        supervisorEnv: formatEnvironment(operation.env),
        supervisorName: operation.name,
      };
    case "process_stop":
    case "process_restart":
      return { supervisorName: operation.name };
    case "process_status":
      return { supervisorName: operation.name ?? "" };
    case "process_logs":
      return {
        supervisorLogBytes: operation.max_bytes,
        supervisorName: operation.name,
      };
    default:
      return {};
  }
}

function formatEnvironment(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

function formatArgvForInput(argv: string[]): string {
  return argv.map(shellQuoteArg).join(" ");
}

function shellQuoteArg(value: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, `'\\''`)}'`;
}
