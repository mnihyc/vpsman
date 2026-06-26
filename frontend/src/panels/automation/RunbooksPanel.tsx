import { ClipboardList, ExternalLink, Play, RefreshCcw, Search } from "lucide-react";
import { useMemo, useState } from "react";
import type { JobDispatchPresetInput } from "../../jobDispatchPreset";
import { agentsMatchingExpression } from "../../searchExpression";
import type { AgentView, CommandTemplateRecord, JobHistoryRecord, JobOperation, JsonValue } from "../../types";
import { shortId } from "../../utils";
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
          matchingTargets,
          requiredParameters: requiredParametersForOperation(template.operation),
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
  const dispatchableCount = runbooks.filter((runbook) => runbook.capability.dispatchable).length;
  const customCount = runbooks.filter((runbook) => !runbook.template.built_in).length;
  const latestRun = jobs[0] ?? null;

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
            <button className="secondaryAction compactAction" onClick={onOpenSchedules} type="button">
              <ClipboardList size={16} />
              Schedules
            </button>
            <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
              <RefreshCcw size={16} />
              Refresh
            </button>
          </div>
        </div>

        <div className="runbookSummary" aria-label="Runbook catalog summary">
          <RunbookMetric label="Runbooks" value={String(runbooks.length)} detail="template-backed reviewed operations" />
          <RunbookMetric label="Ready" value={String(dispatchableCount)} detail="can open reviewed dispatch" />
          <RunbookMetric label="Custom" value={String(customCount)} detail="operator-defined templates" />
          <RunbookMetric
            label="Last run"
            value={latestRun ? shortId(latestRun.id) : "none"}
            detail={latestRun ? `${latestRun.command_type} ${latestRun.status}` : "no job evidence loaded"}
          />
        </div>

        <div className="runbookToolbar" aria-label="Runbook filters">
          <label className="runbookSearch">
            <Search size={16} />
            <input
              aria-label="Search runbooks"
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search name, scope, operation, or required review"
              type="search"
              value={query}
            />
          </label>
          <div className="segmented" role="group" aria-label="Runbook kind">
            {[
              ["all", "All"],
              ["built_in", "Built-in"],
              ["custom", "Custom"],
            ].map(([value, label]) => (
              <button
                aria-pressed={filter === value}
                className={filter === value ? "selected" : ""}
                key={value}
                onClick={() => setFilter(value as RunbookFilter)}
                type="button"
              >
                {label}
              </button>
            ))}
          </div>
          <button className="secondaryAction compactAction" onClick={onOpenJobsDispatch} type="button">
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
                  <span className={runbook.template.built_in ? "status info" : "status ok"}>
                    {runbook.template.built_in ? "built-in" : "custom"}
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
                    <dt>Review</dt>
                    <dd>{runbook.template.command_type}</dd>
                  </div>
                  <div>
                    <dt>Last evidence</dt>
                    <dd>{runbook.lastRun ? `${shortId(runbook.lastRun.id)} ${runbook.lastRun.status}` : "No matching run"}</dd>
                  </div>
                </dl>

                <div className="runbookRequirementList" aria-label={`Required review for ${runbook.template.name}`}>
                  {runbook.requiredParameters.map((item) => (
                    <span key={item}>{item}</span>
                  ))}
                </div>

                <div className="runbookCardFooter">
                  {runbook.capability.dispatchable ? (
                    <button className="primaryAction compactAction" onClick={() => reviewRunbook(runbook)} type="button">
                      <Play size={15} />
                      Review in Dispatch
                    </button>
                  ) : (
                    <button
                      className="secondaryAction compactAction"
                      onClick={runbook.capability.route === "terminal" ? onOpenRemoteTerminal : onOpenJobsDispatch}
                      type="button"
                    >
                      <ExternalLink size={15} />
                      {runbook.capability.route === "terminal" ? "Remote terminal" : "Open owner"}
                    </button>
                  )}
                  <span>{runbook.capability.dispatchable ? "Reviewed handoff" : runbook.capability.reason}</span>
                </div>
              </article>
            ))}
          </div>
        ) : (
          <div className="emptyState compactEmpty" role="status">
            <ClipboardList size={22} />
            <strong>No runbooks match</strong>
            <span>Adjust the filter or create command templates in Jobs / Dispatch.</span>
          </div>
        )}
      </section>
    </section>
  );
}

function RunbookMetric({ detail, label, value }: { detail: string; label: string; value: string }) {
  return (
    <div className="runbookMetric">
      <small>{label}</small>
      <strong>{value}</strong>
      <span>{detail}</span>
    </div>
  );
}

function latestRunForTemplate(template: CommandTemplateRecord, jobs: JobHistoryRecord[]) {
  return jobs.find((job) => job.command_type === template.command_type) ?? null;
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
      return { dispatchable: true, mode: "process_supervisor", supervisorAction: "start" };
    case "process_stop":
      return { dispatchable: true, mode: "process_supervisor", supervisorAction: "stop" };
    case "process_restart":
      return { dispatchable: true, mode: "process_supervisor", supervisorAction: "restart" };
    case "process_status":
      return { dispatchable: true, mode: "process_supervisor", supervisorAction: "status" };
    case "process_logs":
      return { dispatchable: true, mode: "process_supervisor", supervisorAction: "logs" };
    case "terminal_open":
    case "terminal_input":
    case "terminal_poll":
    case "terminal_resize":
    case "terminal_close":
      return { dispatchable: false, reason: "Terminal lifecycle belongs in Remote Operations.", route: "terminal" };
    case "file_transfer_start":
    case "file_transfer_chunk":
    case "file_transfer_commit":
    case "file_transfer_abort":
    case "file_transfer_download_start":
    case "file_transfer_download_chunk":
      return { dispatchable: false, reason: "Transfer sessions belong in Remote Operations / Transfers.", route: "transfers" };
    default:
      return { dispatchable: false, reason: "Open the owner workflow for this operation type.", route: "unsupported" };
  }
}

function requiredParametersForOperation(operation: JobOperation): string[] {
  switch (operation.type) {
    case "shell":
      return ["target scope", operation.pty ? "PTY shell review" : "argv review", "timeout"];
    case "shell_script":
      return ["target scope", "script review", "timeout"];
    case "file_pull":
      return ["target scope", `path ${operation.path}`, operation.follow_symlinks ? "follow symlinks" : "no symlink follow"];
    case "backup":
      return [
        "target scope",
        operation.include_config ? "include config" : "paths only",
        operation.paths.length ? `${operation.paths.length} paths` : "no paths",
      ];
    case "agent_update":
      return ["target scope", "artifact URL", "SHA-256"];
    case "agent_update_check":
      return ["target scope", "manifest URL", operation.activate ? "activation review" : "check only"];
    case "agent_update_activate":
      return ["target scope", "staged SHA-256", operation.restart_agent ? "restart agent" : "no restart"];
    case "agent_update_rollback":
      return ["target scope", operation.rollback_sha256_hex ? "rollback SHA-256" : "latest rollback"];
    case "user_sessions":
      return ["target scope", "session inventory"];
    case "process_list":
      return ["target scope", `${operation.limit} process limit`];
    case "process_start":
      return ["target scope", `process ${operation.name}`, "start review"];
    case "process_stop":
      return ["target scope", `process ${operation.name}`, "stop review"];
    case "process_restart":
      return ["target scope", `process ${operation.name}`, "restart review"];
    case "process_status":
      return ["target scope", operation.name ? `process ${operation.name}` : "all processes"];
    case "process_logs":
      return ["target scope", `process ${operation.name}`, `${operation.max_bytes} log bytes`];
    default:
      return ["owner workflow", operation.type];
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

function defaultMaxTimeout(defaults: JsonValue): number | undefined {
  if (!defaults || typeof defaults !== "object" || Array.isArray(defaults)) {
    return undefined;
  }
  const value = (defaults as Record<string, JsonValue>).max_timeout_secs;
  return typeof value === "number" ? value : undefined;
}

function processSupervisorPreset(operation: JobOperation): Partial<JobDispatchPresetInput> {
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
      return { supervisorLogBytes: operation.max_bytes, supervisorName: operation.name };
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
