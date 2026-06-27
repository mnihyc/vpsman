import { useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  Boxes,
  Clock3,
  DatabaseBackup,
  FileCog,
  FolderOpen,
  Gauge,
  History,
  Network,
  Server,
  TerminalSquare,
} from "lucide-react";
import { agentDisplayState } from "../agentDisplayState";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import type {
  AgentView,
  AuditLogRecord,
  BackupArtifactRecord,
  BackupRequestRecord,
  FleetAlertRecord,
  FleetSummary,
  JobHistoryRecord,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  RuntimeConfigApplyStateRecord,
  SourceStatusRecord,
  SourceTemplateAssignmentRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
  VpsRuleValueRecord,
} from "../types";
import {
  displayNameOrUnnamed,
  formatCompactTime,
  formatFullTime,
  shortId,
} from "../utils";

type VpsDetailTab =
  | "Summary"
  | "Remote access"
  | "Files"
  | "Processes"
  | "Config"
  | "Backups"
  | "Network"
  | "Activity";

type VpsDetailPanelProps = {
  agent: AgentView | null;
  agents: AgentView[];
  apiError: string | null;
  audits: AuditLogRecord[];
  backupArtifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  loading: boolean;
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  onOpenAudit: () => void;
  onOpenBackup: (agent: AgentView) => void;
  onOpenConfig: (agent: AgentView) => void;
  onOpenFiles: (agent: AgentView) => void;
  onOpenFleetAlerts: () => void;
  onOpenInstances: () => void;
  onOpenJob: (jobId: string) => void;
  onOpenJobs: () => void;
  onOpenNetwork: (agent: AgentView) => void;
  onOpenNetworkEvidence: (agent: AgentView) => void;
  onOpenProcesses: (agent: AgentView) => void;
  onOpenTerminal: (agent: AgentView) => void;
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  sourceStatus: SourceStatusRecord[];
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  summary: FleetSummary;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
};

const detailTabs: VpsDetailTab[] = [
  "Summary",
  "Remote access",
  "Files",
  "Processes",
  "Config",
  "Backups",
  "Network",
  "Activity",
];

export function VpsDetailPanel({
  agent,
  agents,
  apiError,
  audits,
  backupArtifacts,
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  loading,
  networkObservations,
  networkTrends,
  onOpenAudit,
  onOpenBackup,
  onOpenConfig,
  onOpenFiles,
  onOpenFleetAlerts,
  onOpenInstances,
  onOpenJob,
  onOpenJobs,
  onOpenNetwork,
  onOpenNetworkEvidence,
  onOpenProcesses,
  onOpenTerminal,
  runtimeConfigApplyStates,
  sourceStatus,
  sourceTemplateAssignments,
  telemetryNetworkRates,
  telemetryRollups,
  telemetryTunnels,
  vpsRuleValues,
}: VpsDetailPanelProps) {
  const [activeTab, setActiveTab] = useState<VpsDetailTab>("Summary");
  const related = useMemo(
    () =>
      agent
        ? buildVpsDetailContext({
            agent,
            audits,
            backupArtifacts,
            backups,
            fileTransfers,
            fleetAlerts,
            jobs,
            networkObservations,
            networkTrends,
            runtimeConfigApplyStates,
            sourceStatus,
            sourceTemplateAssignments,
            telemetryNetworkRates,
            telemetryRollups,
            telemetryTunnels,
            vpsRuleValues,
          })
        : null,
    [
      agent,
      audits,
      backupArtifacts,
      backups,
      fileTransfers,
      fleetAlerts,
      jobs,
      networkObservations,
      networkTrends,
      runtimeConfigApplyStates,
      sourceStatus,
      sourceTemplateAssignments,
      telemetryNetworkRates,
      telemetryRollups,
      telemetryTunnels,
      vpsRuleValues,
    ],
  );

  if (!agent || !related) {
    return (
      <section className="workspace singleColumn vpsDetailWorkspace">
        <div className="fleetPanel vpsDetailPanel">
          <div className="sectionHeader">
            <div>
              <h2>VPS detail</h2>
              <span>Select one VPS from Home, Fleet, Jobs, Backups, Network, or global search.</span>
            </div>
            <Server size={20} />
          </div>
          <div className="emptyState">
            <Server size={22} />
            <strong>{agents.length === 0 ? "No VPS inventory" : "No VPS selected"}</strong>
            <span>
              {apiError ??
                (loading
                  ? "Loading fleet inventory before opening the canonical detail page."
                  : "Open a VPS from an inventory row, monitor card, alert, job target, backup record, or network node.")}
            </span>
            <div className="emptyStateActions">
              <button className="secondaryAction compactAction" onClick={onOpenInstances} type="button">
                <Server size={14} />
                <span>Open Instances</span>
              </button>
            </div>
          </div>
        </div>
      </section>
    );
  }

  const activeAlertCount = related.alerts.filter((alert) => alert.operator_state !== "cleared").length;
  const latestJob = related.relatedJobs[0] ?? null;
  const displayState = agentDisplayState(agent);
  const activeJobCount = related.relatedJobs.filter((job) =>
    isActiveJobStatus(job.status),
  ).length;

  return (
    <section className="workspace singleColumn vpsDetailWorkspace" aria-label="Canonical VPS detail">
      <div className="fleetPanel vpsDetailPanel">
        <div className="sectionHeader vpsDetailHeader">
          <div>
            <h2>VPS detail</h2>
            <span>
              Canonical VPS page for {displayNameOrUnnamed(agent.display_name)}; workflows open in their owning pages.
            </span>
          </div>
          <div className="sectionActions">
            <button className="secondaryAction compactAction" onClick={onOpenInstances} type="button">
              <Server size={14} />
              <span>Instances</span>
            </button>
            <button className="secondaryAction compactAction" onClick={() => onOpenTerminal(agent)} type="button">
              <TerminalSquare size={14} />
              <span>Terminal</span>
            </button>
            <button className="secondaryAction compactAction" onClick={() => onOpenFiles(agent)} type="button">
              <FolderOpen size={14} />
              <span>Files</span>
            </button>
            <button className="secondaryAction compactAction" onClick={() => onOpenProcesses(agent)} type="button">
              <Activity size={14} />
              <span>Processes</span>
            </button>
          </div>
        </div>

        {apiError ? (
          <div className="vpsDetailNotice critical" role="status">
            <AlertTriangle size={16} />
            <span>{apiError}</span>
          </div>
        ) : null}

        <div
          className="vpsDetailResourceSummary"
          aria-label="VPS resource summary"
        >
          <div className="vpsDetailIdentity" aria-label="Selected VPS identity">
            <span className={`status ${statusToneClass(displayState.tone)}`}>
              {displayState.label}
            </span>
            <h3>{displayNameOrUnnamed(agent.display_name)}</h3>
            <span className="monoValue">{agent.id}</span>
            <small>{displayState.detail}</small>
            <div className="vpsDetailTags" aria-label="VPS tags">
              {agent.tags.length ? (
                agent.tags.map((tag) => <span key={tag}>{tag}</span>)
              ) : (
                <span>Untagged</span>
              )}
            </div>
          </div>
          <div className="vpsResourceFacts" aria-label="VPS resource facts">
            <VpsResourceFact
              icon={<Gauge size={16} />}
              label="State"
              value={displayState.label}
              detail={agent.status ? readableDetailToken(agent.status) : "Inventory state"}
              tone={displayState.tone === "ok" ? "ready" : "warning"}
            />
            <VpsResourceFact
              icon={<Clock3 size={16} />}
              label="Last contact"
              value={
                agent.last_seen_at ? (
                  <DetailTime value={agent.last_seen_at} />
                ) : (
                  "Not reported"
                )
              }
              detail={agent.last_seen_at ? "Gateway heartbeat" : "No gateway timestamp"}
              tone={agent.last_seen_at ? "ready" : "warning"}
            />
            <VpsResourceFact
              icon={<Network size={16} />}
              label="Last IP"
              value={agent.last_ip ?? agent.registration_ip ?? "Not reported"}
              detail={agent.last_ip ? "Latest source IP" : agent.registration_ip ? "Registration IP" : "No IP evidence"}
              mono
            />
            <VpsResourceFact
              icon={<Server size={16} />}
              label="Agent version"
              value={agentVersionLabel(agent)}
              detail={agent.arch ? `Architecture ${agent.arch}` : "Version not exposed by inventory"}
            />
            <VpsResourceFact
              icon={<AlertTriangle size={16} />}
              label="Alerts"
              value={`${activeAlertCount} active`}
              detail={`${related.alerts.length} loaded records`}
              tone={activeAlertCount > 0 ? "warning" : "ready"}
            />
            <VpsResourceFact
              icon={<History size={16} />}
              label="Active jobs"
              value={`${activeJobCount}`}
              detail={`${related.relatedJobs.length} related job records`}
              tone={activeJobCount > 0 ? "warning" : "neutral"}
            />
          </div>
        </div>

        <label className="detailTabSelect">
          <span>Detail section</span>
          <select
            aria-label="VPS detail section"
            onChange={(event) => setActiveTab(event.target.value as VpsDetailTab)}
            value={activeTab}
          >
            {detailTabs.map((tab) => (
              <option key={tab} value={tab}>
                {tab}
              </option>
            ))}
          </select>
        </label>

        <div className="detailTabs" role="tablist" aria-label="VPS detail tabs">
          {detailTabs.map((tab) => (
            <button
              aria-selected={activeTab === tab}
              className={activeTab === tab ? "selected" : ""}
              key={tab}
              onClick={() => setActiveTab(tab)}
              role="tab"
              type="button"
            >
              {tab}
            </button>
          ))}
        </div>

        <div className="vpsDetailTabPanel" role="tabpanel" aria-label={`${activeTab} tab`}>
          {activeTab === "Summary" && (
            <SummaryTab
              agent={agent}
              latestJob={latestJob}
              loading={loading}
              related={related}
              onOpenFleetAlerts={onOpenFleetAlerts}
              onOpenJob={onOpenJob}
            />
          )}
          {activeTab === "Remote access" && (
            <ActionTab
              icon={<TerminalSquare size={18} />}
              loading={loading}
              title="Remote access"
              description="Open browser terminal sessions from the Remote Operations surface. Session lifecycle, replay, input, resize, and close controls stay there."
              primary={{ label: "Open terminal", onClick: () => onOpenTerminal(agent) }}
              rows={[
                ["Agent status", agent.status],
                ["Privilege mode", privilegeLabel(agent)],
                ["Max timeout", `${agent.capabilities.max_job_timeout_secs}s`],
                ["Local workflow", "Remote Operations / Terminal"],
              ]}
            />
          )}
          {activeTab === "Files" && (
            <ActionTab
              icon={<FolderOpen size={18} />}
              loading={loading}
              title="Files"
              description="Browse, transfer, edit, and review file operations from Remote Operations / Files."
              primary={{ label: "Browse files", onClick: () => onOpenFiles(agent) }}
              rows={[
                ["Transfer sessions", String(related.fileTransfers.length)],
                ["Latest transfer", related.fileTransfers[0] ? `${related.fileTransfers[0].direction} ${related.fileTransfers[0].status}` : "No transfer record"],
                ["Latest path", related.fileTransfers[0]?.path ?? "No path recorded"],
              ]}
            />
          )}
          {activeTab === "Processes" && (
            <ActionTab
              icon={<Activity size={18} />}
              loading={loading}
              title="Processes"
              description="Inspect process inventory, logs, restarts, and reviewed stop/restart work from Remote Operations / Processes."
              primary={{ label: "Open processes", onClick: () => onOpenProcesses(agent) }}
              rows={[
                ["Process limits", agent.capabilities.can_apply_process_limits ? "Supported" : "Not reported"],
                ["Privilege mode", privilegeLabel(agent)],
                ["Workflow", "Remote Operations / Processes"],
              ]}
            />
          )}
          {activeTab === "Config" && (
            <ConfigTab
              agent={agent}
              loading={loading}
              related={related}
              onOpenConfig={() => onOpenConfig(agent)}
            />
          )}
          {activeTab === "Backups" && (
            <BackupsTab
              loading={loading}
              related={related}
              onOpenBackup={() => onOpenBackup(agent)}
              onOpenJob={onOpenJob}
            />
          )}
          {activeTab === "Network" && (
            <NetworkTab
              loading={loading}
              related={related}
              onOpenNetwork={() => onOpenNetwork(agent)}
              onOpenNetworkEvidence={() => onOpenNetworkEvidence(agent)}
            />
          )}
          {activeTab === "Activity" && (
            <ActivityTab
              loading={loading}
              related={related}
              onOpenAudit={onOpenAudit}
              onOpenFleetAlerts={onOpenFleetAlerts}
              onOpenJob={onOpenJob}
              onOpenJobs={onOpenJobs}
            />
          )}
        </div>
      </div>
    </section>
  );
}

function SummaryTab({
  agent,
  latestJob,
  loading,
  related,
  onOpenFleetAlerts,
  onOpenJob,
}: {
  agent: AgentView;
  latestJob: JobHistoryRecord | null;
  loading: boolean;
  related: VpsDetailContext;
  onOpenFleetAlerts: () => void;
  onOpenJob: (jobId: string) => void;
}) {
  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Health" icon={<Gauge size={18} />}>
        <VpsFact label="CPU load" value={related.rollup ? related.rollup.cpu_load_1_avg.toFixed(2) : "No resource rollup"} />
        <VpsFact label="Memory used" value={related.rollup ? percent(related.rollup.memory_total_bytes_max - related.rollup.memory_available_bytes_avg, related.rollup.memory_total_bytes_max) : "No resource rollup"} />
        <VpsFact label="Disk used" value={related.rollup ? percent(related.rollup.disk_total_bytes_max - related.rollup.disk_available_bytes_avg, related.rollup.disk_total_bytes_max) : "No resource rollup"} />
        <VpsFact
          label="Resource sample"
          value={
            related.rollup ? (
              <DetailTime value={related.rollup.latest_observed_at} />
            ) : (
              "No rollup sample"
            )
          }
        />
        {!related.rollup && (
          <DetailState
            loading={loading}
            title="Resource rollup unavailable"
            detail="Network, job, backup, and alert evidence may still exist because those workflows retain their own records."
          />
        )}
      </DetailBlock>
      <DetailBlock title="Warnings" icon={<AlertTriangle size={18} />}>
        {related.alerts.length === 0 ? (
          <DetailState loading={loading} title="No alert records" detail="Fleet alerts for this VPS are not present in the current page cache." />
        ) : (
          related.alerts.slice(0, 3).map((alert) => (
            <button className="vpsDetailRecord" key={alert.id} onClick={onOpenFleetAlerts} type="button">
              <strong>{alert.title}</strong>
              <span>
                {alertSeverityLabel(alert.severity)} · {operatorStateLabel(alert.operator_state)} ·{" "}
                <DetailTime value={alert.observed_at} />
              </span>
            </button>
          ))
        )}
      </DetailBlock>
      <DetailBlock title="Latest work" icon={<History size={18} />}>
        {latestJob ? (
          <button className="vpsDetailRecord" onClick={() => onOpenJob(latestJob.id)} type="button">
            <strong>{displayCommandType(latestJob.command_type)}</strong>
            <span>
              {jobStatusLabel(latestJob.status)} · {latestJob.target_count} targets ·{" "}
              <DetailTime value={latestJob.created_at} />
            </span>
          </button>
        ) : (
          <DetailState loading={loading} title="No related job evidence" detail={`No retained job target evidence is loaded for ${displayNameOrUnnamed(agent.display_name)}.`} />
        )}
        {related.backups[0] ? (
          <span className="vpsDetailRecord static">
            <strong>Backup {shortId(related.backups[0].id)}</strong>
            <span>
              {backupStatusLabel(related.backups[0].status)} ·{" "}
              <DetailTime value={related.backups[0].created_at} />
            </span>
          </span>
        ) : null}
      </DetailBlock>
    </div>
  );
}

function ActionTab({
  description,
  icon,
  loading,
  primary,
  rows,
  title,
}: {
  description: string;
  icon: JSX.Element;
  loading: boolean;
  primary: { label: string; onClick: () => void };
  rows: Array<[string, string]>;
  title: string;
}) {
  return (
    <div className="vpsDetailActionTab">
      <DetailBlock title={title} icon={icon}>
        <p>{description}</p>
        <button className="primaryAction compactAction" onClick={primary.onClick} type="button">
          <span>{primary.label}</span>
        </button>
        {rows.map(([label, value]) => (
          <VpsFact key={label} label={label} value={value} />
        ))}
        <DetailState loading={loading} title="Inline workflow intentionally absent" detail="This page links to the owning workflow instead of duplicating reviewed operations inline." />
      </DetailBlock>
    </div>
  );
}

function ConfigTab({
  agent,
  loading,
  related,
  onOpenConfig,
}: {
  agent: AgentView;
  loading: boolean;
  related: VpsDetailContext;
  onOpenConfig: () => void;
}) {
  const configPosture = buildConfigPosture(related);
  const sourceIssueRows = sourceRowsNeedingAttention(related.sourceStatus);
  const applyState = related.runtimeApplyState;

  return (
    <div className="vpsConfigDetailTab">
      <div className="vpsConfigPosture" aria-label="VPS config posture">
        {configPosture.map((item) => (
          <span className={`vpsConfigPostureItem ${item.tone}`} key={item.label}>
            <small>{item.label}</small>
            <strong>{item.value}</strong>
            <em>{item.detail}</em>
          </span>
        ))}
      </div>
      <div className="vpsConfigActions" aria-label="VPS config actions">
        <button className="primaryAction compactAction" onClick={onOpenConfig} type="button">
          <FileCog size={14} />
          <span>Open config</span>
        </button>
        <button
          className="secondaryAction compactAction"
          onClick={onOpenConfig}
          title="Open Config / Per-VPS with this VPS selected to compare the current redacted config before applying changes."
          type="button"
        >
          <Boxes size={14} />
          <span>Compare</span>
        </button>
        <button
          className="secondaryAction compactAction"
          onClick={onOpenConfig}
          title="Open Config / Per-VPS to review and apply a runtime config patch with privilege confirmation."
          type="button"
        >
          <Activity size={14} />
          <span>Apply</span>
        </button>
      </div>
      <div className="vpsDetailGrid">
        <DetailBlock title="Source readiness" icon={<Boxes size={18} />}>
          {related.sourceAssignments.length === 0 && related.sourceStatus.length === 0 ? (
            <DetailState loading={loading} title="No source-template evidence" detail="No assignment or readiness records are loaded for this VPS." />
          ) : (
            <>
              {sourceIssueRows.length > 0 ? (
                sourceIssueRows.slice(0, 3).map((record) => (
                  <span className="vpsDetailRecord static warning" key={`issue:${record.domain}:${record.module}:${record.template_id}`}>
                    <strong>{record.module || readableDetailToken(record.domain)}</strong>
                    <span>{sourceReadinessStatusLabel(record.status)} · {sourceReadinessReasonLabel(record)}</span>
                  </span>
                ))
              ) : (
                <DetailState loading={loading} title="Sources ready" detail="Loaded source assignments have no readiness blockers." />
              )}
              {related.sourceStatus.slice(0, 4).map((record) => (
                <span className="vpsDetailRecord static" key={`status:${record.domain}:${record.module}:${record.template_id}`}>
                  <strong>{record.domain} · {record.module}</strong>
                  <span>{sourceReadinessStatusLabel(record.status)} · {sourceReadinessReasonLabel(record)}</span>
                </span>
              ))}
            </>
          )}
        </DetailBlock>
        <DetailBlock title="Runtime sync" icon={<FileCog size={18} />}>
          <VpsFact
            label="Runtime tunnels"
            value={agent.capabilities.can_manage_runtime_tunnels ? "Supported" : "Not reported"}
          />
          <VpsFact label="Source assignments" value={String(related.sourceAssignments.length)} />
          <VpsFact label="Readiness records" value={String(related.sourceStatus.length)} />
          <VpsFact label="VPS rules" value={String(related.vpsRules.length)} />
          <VpsFact label="Last apply" value={runtimeApplyTimeLabel(applyState)} />
          <VpsFact label="Apply status" value={runtimeApplyStatusLabel(applyState)} />
        </DetailBlock>
        <DetailBlock title="Rules and raw details" icon={<FileCog size={18} />}>
          {related.vpsRules.length === 0 ? (
            <DetailState loading={loading} title="No VPS-specific rules" detail="No runtime config rules are scoped directly to this VPS." />
          ) : (
            related.vpsRules.slice(0, 4).map((rule) => (
              <span className={`vpsDetailRecord static ${rule.validation_errors.length ? "warning" : ""}`} key={rule.key}>
                <strong>{rule.key}</strong>
                <span>
                  {rule.parsed_display || rule.value_raw} · {rule.validation_errors.length ? rule.validation_errors.join("; ") : "valid"}
                </span>
              </span>
            ))
          )}
          <details className="vpsDetailDisclosure">
            <summary>Raw source state details</summary>
            <div>
              {related.sourceStatus.length === 0 ? (
                <span>No raw source readiness records loaded.</span>
              ) : (
                related.sourceStatus.map((record) => (
                  <span key={`raw:${record.domain}:${record.module}:${record.template_id}`}>
                    <strong>{record.domain}</strong>
                    <code>{record.status}</code>
                    <small>{record.status_reason}</small>
                  </span>
                ))
              )}
            </div>
          </details>
        </DetailBlock>
      </div>
    </div>
  );
}

function BackupsTab({
  loading,
  related,
  onOpenBackup,
  onOpenJob,
}: {
  loading: boolean;
  related: VpsDetailContext;
  onOpenBackup: () => void;
  onOpenJob: (jobId: string) => void;
}) {
  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Backup requests" icon={<DatabaseBackup size={18} />}>
        <button className="primaryAction compactAction" onClick={onOpenBackup} type="button">
          <span>Open backup workflow</span>
        </button>
        {related.backups.length === 0 ? (
          <DetailState loading={loading} title="No backup requests" detail="No current backup request record is loaded for this VPS." />
        ) : (
          related.backups.slice(0, 5).map((backup) => (
            <span className="vpsDetailRecord static" key={backup.id}>
              <strong>{shortId(backup.id)} · {backupStatusLabel(backup.status)}</strong>
              <span>
                {backup.paths.join(", ") || "No paths"} ·{" "}
                <DetailTime value={backup.created_at} />
              </span>
              {backup.source_job_id ? (
                <button className="secondaryAction compactAction" onClick={() => onOpenJob(backup.source_job_id as string)} type="button">
                  <span>Open source job</span>
                </button>
              ) : null}
            </span>
          ))
        )}
      </DetailBlock>
      <DetailBlock title="Artifacts" icon={<Boxes size={18} />}>
        {related.backupArtifacts.length === 0 ? (
          <DetailState loading={loading} title="No artifacts" detail="No retained backup artifact metadata is loaded for this VPS." />
        ) : (
          related.backupArtifacts.slice(0, 5).map((artifact) => (
            <span className="vpsDetailRecord static" key={artifact.id}>
              <strong>{shortId(artifact.id)} · {backupStatusLabel(artifact.status)}</strong>
              <span>{formatBytes(artifact.size_bytes)} · SHA-256 {artifact.sha256_hex.slice(0, 12)}</span>
            </span>
          ))
        )}
      </DetailBlock>
    </div>
  );
}

function NetworkTab({
  loading,
  related,
  onOpenNetwork,
  onOpenNetworkEvidence,
}: {
  loading: boolean;
  related: VpsDetailContext;
  onOpenNetwork: () => void;
  onOpenNetworkEvidence: () => void;
}) {
  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Network workflow" icon={<Network size={18} />}>
        <button className="primaryAction compactAction" onClick={onOpenNetwork} type="button">
          <span>Open network graph</span>
        </button>
        <button className="secondaryAction compactAction" onClick={onOpenNetworkEvidence} type="button">
          <span>Open network evidence</span>
        </button>
        <VpsFact label="Observed interfaces" value={String(related.networkRates.length)} />
        <VpsFact label="Tunnel records" value={String(related.tunnels.length)} />
        <VpsFact label="Trend records" value={String(related.networkTrends.length)} />
      </DetailBlock>
      <DetailBlock title="Latest observations" icon={<Activity size={18} />}>
        {related.networkObservations.length === 0 ? (
          <DetailState loading={loading} title="No network observations" detail="No retained network observation is loaded for this VPS." />
        ) : (
          related.networkObservations.slice(0, 6).map((observation) => (
            <span className="vpsDetailRecord static" key={observation.id}>
              <strong>{networkObservationLabel(observation.kind)} · {observation.healthy === false ? "Degraded" : "Observed"}</strong>
              <span>
                {observation.interface_name ?? "interface n/a"} ·{" "}
                <DetailTime value={observation.observed_at} />
              </span>
            </span>
          ))
        )}
      </DetailBlock>
    </div>
  );
}

function ActivityTab({
  loading,
  related,
  onOpenAudit,
  onOpenFleetAlerts,
  onOpenJob,
  onOpenJobs,
}: {
  loading: boolean;
  related: VpsDetailContext;
  onOpenAudit: () => void;
  onOpenFleetAlerts: () => void;
  onOpenJob: (jobId: string) => void;
  onOpenJobs: () => void;
}) {
  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Correlated events" icon={<History size={18} />}>
        {related.activity.length === 0 ? (
          <DetailState loading={loading} title="No correlated activity" detail="Loaded activity does not include target-scoped records for this VPS yet." />
        ) : (
          related.activity.slice(0, 8).map((event) => (
            <button
              className="vpsDetailRecord"
              key={`${event.kind}:${event.id}`}
              onClick={() => {
                if (event.kind === "job" && event.jobId) onOpenJob(event.jobId);
                else if (event.kind === "alert") onOpenFleetAlerts();
                else if (event.kind === "audit") onOpenAudit();
                else onOpenJobs();
              }}
              type="button"
            >
              <strong>{event.title}</strong>
              <span>{event.detail}</span>
            </button>
          ))
        )}
      </DetailBlock>
      <DetailBlock title="Evidence coverage" icon={<Boxes size={18} />}>
        <VpsFact label="Alerts" value={String(related.alerts.length)} />
        <VpsFact label="Audits" value={String(related.audits.length)} />
        <VpsFact label="Backups" value={String(related.backups.length)} />
        <VpsFact label="Transfers" value={String(related.fileTransfers.length)} />
        <VpsFact label="Network events" value={String(related.networkObservations.length)} />
        <DetailState loading={loading} title="Job target loading note" detail="Job history rows expose target records after opening a job, so direct job correlation is shown only when backup, transfer, output, or loaded target evidence carries this VPS ID." />
      </DetailBlock>
    </div>
  );
}

function DetailBlock({
  children,
  icon,
  title,
}: {
  children: ReactNode;
  icon: JSX.Element;
  title: string;
}) {
  return (
    <section className="vpsDetailBlock">
      <div className="vpsDetailBlockHeader">
        {icon}
        <h3>{title}</h3>
      </div>
      {children}
    </section>
  );
}

function VpsFact({
  icon,
  label,
  mono = false,
  value,
}: {
  icon?: JSX.Element;
  label: string;
  mono?: boolean;
  value: ReactNode;
}) {
  return (
    <span className="vpsFactRow">
      {icon}
      <span>{label}</span>
      <strong className={mono ? "monoValue" : undefined}>{value}</strong>
    </span>
  );
}

function VpsResourceFact({
  detail,
  icon,
  label,
  mono = false,
  tone = "neutral",
  value,
}: {
  detail: string;
  icon: JSX.Element;
  label: string;
  mono?: boolean;
  tone?: "neutral" | "ready" | "warning";
  value: ReactNode;
}) {
  return (
    <span className={`vpsResourceFact ${tone}`}>
      {icon}
      <span>{label}</span>
      <strong className={mono ? "monoValue" : undefined}>{value}</strong>
      <small>{detail}</small>
    </span>
  );
}

function DetailTime({ value }: { value: string }) {
  return (
    <time dateTime={value} title={formatFullTime(value)}>
      {formatCompactTime(value)}
    </time>
  );
}

function DetailState({
  detail,
  loading,
  title,
}: {
  detail: string;
  loading: boolean;
  title: string;
}) {
  return (
    <span className="vpsDetailState">
      <strong>{loading ? "Loading evidence" : title}</strong>
      <small>{loading ? "The backend request is still in progress for this page cache." : detail}</small>
    </span>
  );
}

type VpsDetailContext = ReturnType<typeof buildVpsDetailContext>;

function buildVpsDetailContext({
  agent,
  audits,
  backupArtifacts,
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  networkObservations,
  networkTrends,
  runtimeConfigApplyStates,
  sourceStatus,
  sourceTemplateAssignments,
  telemetryNetworkRates,
  telemetryRollups,
  telemetryTunnels,
  vpsRuleValues,
}: {
  agent: AgentView;
  audits: AuditLogRecord[];
  backupArtifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  sourceStatus: SourceStatusRecord[];
  sourceTemplateAssignments: SourceTemplateAssignmentRecord[];
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  vpsRuleValues: VpsRuleValueRecord[];
}) {
  const clientId = agent.id;
  const relatedBackups = backups
    .filter((backup) => backup.client_id === clientId)
    .sort(newestFirst((backup) => backup.created_at));
  const relatedTransfers = fileTransfers
    .filter((transfer) => transfer.client_id === clientId)
    .sort(newestFirst((transfer) => transfer.observed_at));
  const relatedAlerts = fleetAlerts
    .filter((alert) => alert.client_id === clientId || alert.target_id === clientId)
    .sort(newestFirst((alert) => alert.observed_at));
  const relatedAudits = audits
    .filter((audit) => audit.target.includes(clientId) || JSON.stringify(audit.metadata).includes(clientId))
    .sort(newestFirst((audit) => audit.created_at));
  const relatedNetworkObservations = networkObservations
    .filter((observation) => observation.client_id === clientId || observation.peer_client_id === clientId)
    .sort(newestFirst((observation) => observation.observed_at));
  const relatedNetworkTrends = networkTrends
    .filter((trend) => trend.client_id === clientId || trend.peer_client_id === clientId)
    .sort(newestFirst((trend) => trend.latest_observed_at));
  const relatedJobs = jobs
    .filter((job) =>
      relatedBackups.some((backup) => backup.source_job_id === job.id) ||
      relatedTransfers.some((transfer) => transfer.last_job_id === job.id) ||
      relatedNetworkObservations.some((observation) => observation.job_id === job.id) ||
      relatedAudits.some((audit) => audit.command_hash === job.payload_hash),
    )
    .sort(newestFirst((job) => job.created_at));
  const rollup =
    telemetryRollups
      .filter((record) => record.client_id === clientId)
      .sort(newestFirst((record) => record.latest_observed_at))[0] ?? null;
  const networkRates = telemetryNetworkRates
    .filter((rate) => rate.client_id === clientId)
    .sort((left, right) => left.interface.localeCompare(right.interface));
  const tunnels = telemetryTunnels
    .filter((tunnel) => tunnel.client_id === clientId || tunnel.peer_client_id === clientId)
    .sort(newestFirst((tunnel) => tunnel.observed_at));
  const sourceAssignments = sourceTemplateAssignments.filter((assignment) => assignment.client_id === clientId);
  const sourceStatusRows = sourceStatus.filter((row) => row.client_id === clientId);
  const vpsRules = vpsRuleValues.filter((rule) => rule.client_id === clientId);
  const relatedArtifacts = backupArtifacts
    .filter((artifact) => artifact.client_id === clientId)
    .sort(newestFirst((artifact) => artifact.created_at));
  const runtimeApplyState =
    runtimeConfigApplyStates
      .filter((state) => state.client_id === clientId)
      .sort(newestFirst((state) => runtimeApplyStateTime(state)))[0] ?? null;
  const activity: Array<{
    detail: string;
    id: string;
    jobId?: string;
    kind: "alert" | "audit" | "backup" | "job" | "network" | "transfer";
    title: string;
    when: string;
  }> = [
    ...relatedAlerts.map((alert) => ({
      detail: `${alertSeverityLabel(alert.severity)} · ${operatorStateLabel(alert.operator_state)} · ${formatCompactTime(alert.observed_at)}`,
      id: alert.id,
      kind: "alert" as const,
      title: alert.title,
      when: alert.observed_at,
    })),
    ...relatedBackups.map((backup) => ({
      detail: `${backupStatusLabel(backup.status)} · ${backup.paths.join(", ") || "no paths"} · ${formatCompactTime(backup.created_at)}`,
      id: backup.id,
      jobId: backup.source_job_id ?? undefined,
      kind: "backup" as const,
      title: `Backup ${shortId(backup.id)}`,
      when: backup.created_at,
    })),
    ...relatedTransfers.map((transfer) => ({
      detail: `${transferDirectionLabel(transfer.direction)} · ${readableDetailToken(transfer.status)} · ${transfer.path} · ${formatCompactTime(transfer.observed_at)}`,
      id: transfer.session_id,
      jobId: transfer.last_job_id,
      kind: "transfer" as const,
      title: `Transfer ${shortId(transfer.session_id)}`,
      when: transfer.observed_at,
    })),
    ...relatedNetworkObservations.map((observation) => ({
      detail: `${networkObservationLabel(observation.kind)} · ${observation.interface_name ?? "interface n/a"} · ${formatCompactTime(observation.observed_at)}`,
      id: observation.id,
      jobId: observation.job_id,
      kind: "network" as const,
      title: observation.healthy === false ? "Network degradation" : "Network observation",
      when: observation.observed_at,
    })),
    ...relatedJobs.map((job) => ({
      detail: `${displayCommandType(job.command_type)} · ${jobStatusLabel(job.status)} · ${job.target_count} targets · ${formatCompactTime(job.created_at)}`,
      id: job.id,
      jobId: job.id,
      kind: "job" as const,
      title: `Job ${shortId(job.id)}`,
      when: job.created_at,
    })),
    ...relatedAudits.map((audit) => ({
      detail: `${readableDetailToken(audit.action)} · ${audit.target} · ${formatCompactTime(audit.created_at)}`,
      id: audit.id,
      kind: "audit" as const,
      title: `Audit ${shortId(audit.id)}`,
      when: audit.created_at,
    })),
  ].sort((left, right) => Date.parse(right.when) - Date.parse(left.when));

  return {
    activity,
    alerts: relatedAlerts,
    audits: relatedAudits,
    backupArtifacts: relatedArtifacts,
    backups: relatedBackups,
    fileTransfers: relatedTransfers,
    networkObservations: relatedNetworkObservations,
    networkRates,
    networkTrends: relatedNetworkTrends,
    relatedJobs,
    rollup,
    runtimeApplyState,
    sourceAssignments,
    sourceStatus: sourceStatusRows,
    tunnels,
    vpsRules,
  };
}

function newestFirst<T>(dateFor: (record: T) => string) {
  return (left: T, right: T) =>
    Date.parse(dateFor(right)) - Date.parse(dateFor(left));
}

function privilegeLabel(agent: AgentView) {
  if (agent.capabilities.privilege_mode === "root") return "root capable";
  if (agent.capabilities.privilege_mode === "unprivileged") return "unprivileged";
  return agent.capabilities.can_attempt_privileged_ops ? "privilege available" : "unknown";
}

type ConfigPostureItem = {
  detail: string;
  label: string;
  tone: "critical" | "warning" | "ok" | "info" | "neutral";
  value: ReactNode;
};

function buildConfigPosture(related: VpsDetailContext): ConfigPostureItem[] {
  const sourceIssues = sourceRowsNeedingAttention(related.sourceStatus);
  const ruleErrors = related.vpsRules.flatMap((rule) => rule.validation_errors);
  const applyState = related.runtimeApplyState;
  const lastError =
    applyState?.pending_error ||
    ruleErrors[0] ||
    sourceIssues[0]?.status_reason ||
    null;
  return [
    {
      detail:
        related.sourceAssignments.length > 0
          ? sourceDomainSummary(related.sourceAssignments.map((assignment) => assignment.domain))
          : related.sourceStatus.length > 0
            ? sourceDomainSummary(related.sourceStatus.map((status) => status.domain))
            : "No assignment evidence loaded",
      label: "Desired source",
      tone: related.sourceAssignments.length > 0 || related.sourceStatus.length > 0 ? "info" : "neutral",
      value:
        related.sourceAssignments.length > 0
          ? `${related.sourceAssignments.length} selected`
          : related.sourceStatus.length > 0
            ? `${related.sourceStatus.length} reported`
            : "Not selected",
    },
    {
      detail:
        sourceIssues[0] !== undefined
          ? sourceReadinessReasonLabel(sourceIssues[0])
          : related.sourceStatus.length > 0
            ? "Loaded source readiness has no blockers"
            : "No readiness records loaded",
      label: "Render status",
      tone: sourceIssues.length > 0 ? "warning" : related.sourceStatus.length > 0 ? "ok" : "neutral",
      value: sourceIssues.length > 0 ? "Needs configuration" : related.sourceStatus.length > 0 ? "Ready" : "Unknown",
    },
    {
      detail: configDriftDetail(applyState, sourceIssues.length, ruleErrors.length),
      label: "Drift state",
      tone: configDriftTone(applyState, sourceIssues.length, ruleErrors.length),
      value: configDriftLabel(applyState, sourceIssues.length, ruleErrors.length),
    },
    {
      detail: runtimeApplyDetail(applyState),
      label: "Last apply",
      tone: applyState?.pending_status === "failed" ? "critical" : applyState?.applied_at ? "ok" : "neutral",
      value: runtimeApplyTimeLabel(applyState),
    },
    {
      detail: lastError ?? "No loaded config error",
      label: "Last error",
      tone: lastError ? "warning" : "ok",
      value: lastError ? "Needs review" : "None",
    },
  ];
}

function sourceRowsNeedingAttention(rows: SourceStatusRecord[]): SourceStatusRecord[] {
  return rows.filter((row) => !sourceReadinessIsOk(row.status));
}

function sourceReadinessIsOk(status: string): boolean {
  return ["ok", "ready", "ready_on_demand", "selected", "selected_workflow", "metadata_only"].includes(status);
}

function sourceReadinessStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    agent_offline: "Agent offline",
    degraded: "Degraded",
    metadata_only: "Metadata only",
    needs_promotion: "Needs promotion",
    ok: "Ready",
    ready: "Ready",
    ready_on_demand: "Ready on demand",
    selected: "Selected",
    selected_no_artifacts: "Selected; no artifacts",
    selected_no_limits: "Selected; no limits",
    selected_no_samples: "Selected; no samples",
    selected_no_store: "Selected; storage unavailable",
    selected_workflow: "Selected workflow",
    unknown_domain: "Unknown domain",
  };
  return labels[status] ?? readableDetailToken(status);
}

function sourceReadinessReasonLabel(record: SourceStatusRecord): string {
  if (record.status === "selected_no_store") {
    return "Backup object-store source selected; server storage is not configured.";
  }
  return sentenceCase(record.status_reason || sourceReadinessStatusLabel(record.status));
}

function sourceDomainSummary(domains: string[]): string {
  const unique = Array.from(new Set(domains)).filter(Boolean);
  if (unique.length === 0) {
    return "No source domains loaded";
  }
  return unique.slice(0, 3).map(readableDetailToken).join(", ") + (unique.length > 3 ? ` +${unique.length - 3}` : "");
}

function configDriftLabel(
  state: RuntimeConfigApplyStateRecord | null,
  sourceIssueCount: number,
  ruleErrorCount: number,
): string {
  if (state?.pending_status === "failed") return "Apply failed";
  if (state?.pending_status === "queued") return runtimeApplyQueuedIsStale(state) ? "Stale apply" : "Pending apply";
  if (ruleErrorCount > 0) return "Rule errors";
  if (sourceIssueCount > 0) return "Source attention";
  if (state?.applied_content_hash) return "No pending apply";
  return "Not compared";
}

function configDriftDetail(
  state: RuntimeConfigApplyStateRecord | null,
  sourceIssueCount: number,
  ruleErrorCount: number,
): string {
  if (state?.pending_status === "failed") return state.pending_error ?? "Runtime config apply failed";
  if (state?.pending_status === "queued") return state.pending_reason ?? "Runtime config apply is queued";
  if (ruleErrorCount > 0) return `${ruleErrorCount} VPS rule validation issue${ruleErrorCount === 1 ? "" : "s"}`;
  if (sourceIssueCount > 0) return `${sourceIssueCount} source readiness issue${sourceIssueCount === 1 ? "" : "s"}`;
  if (state?.applied_content_hash) return `Applied hash ${shortId(state.applied_content_hash)}`;
  return "Open Config / Per-VPS to compare current redacted config";
}

function configDriftTone(
  state: RuntimeConfigApplyStateRecord | null,
  sourceIssueCount: number,
  ruleErrorCount: number,
): ConfigPostureItem["tone"] {
  if (state?.pending_status === "failed") return "critical";
  if (state?.pending_status === "queued") return runtimeApplyQueuedIsStale(state) ? "warning" : "info";
  if (ruleErrorCount > 0 || sourceIssueCount > 0) return "warning";
  if (state?.applied_content_hash) return "ok";
  return "neutral";
}

function runtimeApplyTimeLabel(state: RuntimeConfigApplyStateRecord | null): ReactNode {
  if (state?.applied_at) {
    return <DetailTime value={state.applied_at} />;
  }
  if (state?.pending_updated_at) {
    return <DetailTime value={state.pending_updated_at} />;
  }
  return "Not applied";
}

function runtimeApplyStatusLabel(state: RuntimeConfigApplyStateRecord | null): string {
  if (!state) return "No apply-state evidence";
  if (state.pending_status === "failed") return "Failed apply";
  if (state.pending_status === "queued") return runtimeApplyQueuedIsStale(state) ? "Stale queued apply" : "Queued apply";
  if (state.applied_content_hash) return "Current";
  return "Unknown";
}

function runtimeApplyDetail(state: RuntimeConfigApplyStateRecord | null): string {
  if (!state) return "No server-applied runtime sync recorded";
  if (state.pending_status === "failed") return state.pending_error ?? "Runtime config apply failed";
  if (state.pending_status === "queued") return state.pending_reason ?? "Runtime config apply queued";
  if (state.applied_content_hash) {
    const version = state.applied_version ? `v${state.applied_version}; ` : "";
    const job = state.applied_job_id ? `; job ${shortId(state.applied_job_id)}` : "";
    return `${version}hash ${shortId(state.applied_content_hash)}${job}`;
  }
  return "No server-applied runtime sync recorded";
}

function runtimeApplyStateTime(state: RuntimeConfigApplyStateRecord): string {
  return state.pending_updated_at ?? state.applied_at ?? state.updated_at;
}

function runtimeApplyQueuedIsStale(state: RuntimeConfigApplyStateRecord): boolean {
  const updatedAt = Date.parse(runtimeApplyStateTime(state));
  return !Number.isFinite(updatedAt) || Date.now() - updatedAt > 24 * 60 * 60 * 1000;
}

function sentenceCase(value: string): string {
  const normalized = value.trim();
  if (!normalized) {
    return "Not reported";
  }
  return normalized.charAt(0).toUpperCase() + normalized.slice(1);
}

function statusToneClass(tone: string): string {
  return tone === "warning" ? "warn" : tone;
}

function agentVersionLabel(agent: AgentView): string {
  if (typeof agent.internal_build_number === "number") {
    return `Build ${agent.internal_build_number}`;
  }
  return "Not reported";
}

function isActiveJobStatus(status: string): boolean {
  return ["queued", "running", "dispatching"].includes(status);
}

function displayCommandType(value: string): string {
  switch (value) {
    case "shell_argv":
      return "Shell command";
    case "scheduled_shell_argv":
      return "Scheduled shell command";
    case "shell_pty":
      return "Terminal session";
    case "terminal_input":
      return "Terminal input";
    case "file_read":
      return "File read";
    case "file_write":
      return "File write";
    case "backup":
      return "Backup run";
    case "network_probe":
      return "Network probe";
    case "network_speed_test":
      return "Network speed test";
    case "network_status":
      return "Network status check";
    default:
      return readableDetailToken(value);
  }
}

function jobStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    canceled: "Canceled",
    completed: "Completed",
    dispatching: "Dispatching",
    failed: "Failed",
    queued: "Queued",
    running: "Running",
    timed_out: "Timed out",
  };
  return labels[status] ?? readableDetailToken(status);
}

function backupStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    accepted: "Accepted",
    active: "Available package",
    artifact_metadata_recorded: "Artifact metadata recorded",
    artifact_uploaded: "Artifact uploaded",
    completed: "Completed",
    creating: "Preparing package",
    deleted: "Deleted",
    delete_failed: "Delete failed",
    failed: "Failed",
    linked_metadata_only: "Linked metadata only",
    planned_metadata_only: "Planned metadata only",
    requested: "Requested",
    restored: "Restored",
    running: "Running",
    tombstoned: "Metadata retained",
  };
  return labels[status] ?? readableDetailToken(status);
}

function alertSeverityLabel(severity: string): string {
  const labels: Record<string, string> = {
    critical: "Critical",
    info: "Info",
    warning: "Warning",
  };
  return labels[severity] ?? readableDetailToken(severity);
}

function operatorStateLabel(state: string): string {
  const labels: Record<string, string> = {
    acknowledged: "Acknowledged",
    cleared: "Cleared",
    escalated: "Escalated",
    muted: "Muted",
    open: "Open",
  };
  return labels[state] ?? readableDetailToken(state);
}

function transferDirectionLabel(direction: string): string {
  const labels: Record<string, string> = {
    download: "Download",
    upload: "Upload",
  };
  return labels[direction] ?? readableDetailToken(direction);
}

function networkObservationLabel(kind: string): string {
  const labels: Record<string, string> = {
    latency_probe: "Latency probe",
    network_probe: "Network probe",
    network_speed_test: "Network speed test",
    network_status: "Network status",
    speed_test: "Speed test",
  };
  return labels[kind] ?? readableDetailToken(kind);
}

function readableDetailToken(value: string | null | undefined): string {
  const normalized = value?.trim();
  if (!normalized) {
    return "Not reported";
  }
  return normalized
    .split(/[_:\-.]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function percent(used: number, total: number) {
  if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) {
    return "n/a";
  }
  return `${Math.max(0, Math.min(100, Math.round((used / total) * 100)))}%`;
}

function formatBytes(value: number) {
  if (!Number.isFinite(value) || value < 0) return "n/a";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let next = value;
  let unitIndex = 0;
  while (next >= 1024 && unitIndex < units.length - 1) {
    next /= 1024;
    unitIndex += 1;
  }
  return `${next >= 10 || unitIndex === 0 ? next.toFixed(0) : next.toFixed(1)} ${units[unitIndex]}`;
}
