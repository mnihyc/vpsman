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
  SourceStatusRecord,
  SourceTemplateAssignmentRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
  VpsRuleValueRecord,
} from "../types";
import { displayNameOrUnnamed, formatTime, shortId } from "../utils";
import {
  VpsMonitorCard,
  type VpsMonitorCardSignal,
} from "./FleetMonitorPanel";

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
  sourceStatus,
  sourceTemplateAssignments,
  summary,
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
  const latestJob = related.relatedJobs[0] ?? jobs[0] ?? null;
  const signal = buildDetailCardSignal({
    activeAlertCount,
    backups: related.backups,
    fileTransfers: related.fileTransfers,
    relatedJobs: related.relatedJobs,
  });

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

        <div className="vpsDetailTopGrid">
          <div className="vpsDetailMonitorSlot" aria-label="Selected VPS health card">
            <VpsMonitorCard
              agent={agent}
              density="comfortable"
              onOpenBackup={onOpenBackup}
              onOpenFiles={onOpenFiles}
              onOpenNetwork={onOpenNetwork}
              onOpenProcesses={onOpenProcesses}
              onOpenTerminal={onOpenTerminal}
              onOpenVpsDetail={() => undefined}
              rates={related.networkRates}
              rollup={related.rollup}
              signals={signal}
              tunnels={related.tunnels}
            />
          </div>
          <div className="vpsDetailFacts" aria-label="VPS identity and status facts">
            <VpsFact icon={<Server size={16} />} label="Client ID" value={agent.id} mono />
            <VpsFact icon={<Gauge size={16} />} label="Status" value={agent.status} />
            <VpsFact icon={<Clock3 size={16} />} label="Last seen" value={agent.last_seen_at ? formatTime(agent.last_seen_at) : "Not reported"} />
            <VpsFact icon={<Boxes size={16} />} label="Tags" value={agent.tags.length ? agent.tags.join(", ") : "Untagged"} />
            <VpsFact icon={<Network size={16} />} label="Last IP" value={agent.last_ip ?? "Not reported"} mono />
            <VpsFact icon={<Gauge size={16} />} label="Privilege" value={privilegeLabel(agent)} />
          </div>
        </div>

        <div className="vpsDetailPosture" aria-label="VPS detail posture">
          <VpsPostureMetric label="Fleet status" value={`${summary.online}/${summary.total}`} detail="online / visible VPSs" />
          <VpsPostureMetric label="Alerts" value={activeAlertCount} detail="active alert records" tone={activeAlertCount > 0 ? "warning" : "ready"} />
          <VpsPostureMetric label="Backups" value={related.backups.length} detail="current request records" tone={backupTone(related.backups)} />
          <VpsPostureMetric label="Network" value={related.networkObservations.length + related.tunnels.length} detail="observations plus tunnels" />
          <VpsPostureMetric label="Config" value={related.sourceAssignments.length + related.vpsRules.length} detail="source assignments and VPS rules" />
        </div>

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
        <VpsFact label="CPU load" value={related.rollup ? related.rollup.cpu_load_1_avg.toFixed(2) : "No telemetry"} />
        <VpsFact label="Memory used" value={related.rollup ? percent(related.rollup.memory_total_bytes_max - related.rollup.memory_available_bytes_avg, related.rollup.memory_total_bytes_max) : "No telemetry"} />
        <VpsFact label="Disk used" value={related.rollup ? percent(related.rollup.disk_total_bytes_max - related.rollup.disk_available_bytes_avg, related.rollup.disk_total_bytes_max) : "No telemetry"} />
        <VpsFact label="Telemetry sample" value={related.rollup ? formatTime(related.rollup.latest_observed_at) : "No sample"} />
      </DetailBlock>
      <DetailBlock title="Warnings" icon={<AlertTriangle size={18} />}>
        {related.alerts.length === 0 ? (
          <DetailState loading={loading} title="No alert records" detail="Fleet alerts for this VPS are not present in the current page cache." />
        ) : (
          related.alerts.slice(0, 3).map((alert) => (
            <button className="vpsDetailRecord" key={alert.id} onClick={onOpenFleetAlerts} type="button">
              <strong>{alert.title}</strong>
              <span>{alert.severity} · {alert.operator_state} · {formatTime(alert.observed_at)}</span>
            </button>
          ))
        )}
      </DetailBlock>
      <DetailBlock title="Latest work" icon={<History size={18} />}>
        {latestJob ? (
          <button className="vpsDetailRecord" onClick={() => onOpenJob(latestJob.id)} type="button">
            <strong>{latestJob.command_type}</strong>
            <span>{latestJob.status} · {latestJob.target_count} targets · {formatTime(latestJob.created_at)}</span>
          </button>
        ) : (
          <DetailState loading={loading} title="No related job evidence" detail={`No retained job target evidence is loaded for ${displayNameOrUnnamed(agent.display_name)}.`} />
        )}
        {related.backups[0] ? (
          <span className="vpsDetailRecord static">
            <strong>Backup {shortId(related.backups[0].id)}</strong>
            <span>{related.backups[0].status} · {formatTime(related.backups[0].created_at)}</span>
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
  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Config ownership" icon={<FileCog size={18} />}>
        <button className="primaryAction compactAction" onClick={onOpenConfig} type="button">
          <span>Open per-VPS config</span>
        </button>
        <VpsFact label="Runtime tunnels" value={agent.capabilities.can_manage_runtime_tunnels ? "Supported" : "Not reported"} />
        <VpsFact label="Source assignments" value={String(related.sourceAssignments.length)} />
        <VpsFact label="Readiness records" value={String(related.sourceStatus.length)} />
        <VpsFact label="VPS rules" value={String(related.vpsRules.length)} />
      </DetailBlock>
      <DetailBlock title="Source templates" icon={<Boxes size={18} />}>
        {related.sourceAssignments.length === 0 && related.sourceStatus.length === 0 ? (
          <DetailState loading={loading} title="No source-template evidence" detail="No assignment or readiness records are loaded for this VPS." />
        ) : (
          <>
            {related.sourceAssignments.slice(0, 3).map((record) => (
              <span className="vpsDetailRecord static" key={`assignment:${record.domain}:${record.template_id}`}>
                <strong>{record.template_name || record.template_id}</strong>
                <span>{record.domain} · assigned · {record.template_scope}</span>
              </span>
            ))}
            {related.sourceStatus.slice(0, 3).map((record) => (
              <span className="vpsDetailRecord static" key={`status:${record.domain}:${record.module}:${record.template_id}`}>
                <strong>{record.domain} · {record.module}</strong>
                <span>{record.status} · {record.status_reason}</span>
              </span>
            ))}
          </>
        )}
      </DetailBlock>
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
              <strong>{shortId(backup.id)} · {backup.status}</strong>
              <span>{backup.paths.join(", ") || "No paths"} · {formatTime(backup.created_at)}</span>
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
              <strong>{shortId(artifact.id)} · {artifact.status}</strong>
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
              <strong>{observation.kind} · {observation.healthy === false ? "degraded" : "observed"}</strong>
              <span>{observation.interface_name ?? "interface n/a"} · {formatTime(observation.observed_at)}</span>
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
  value: string;
}) {
  return (
    <span className="vpsFactRow">
      {icon}
      <span>{label}</span>
      <strong className={mono ? "monoValue" : undefined}>{value}</strong>
    </span>
  );
}

function VpsPostureMetric({
  detail,
  label,
  tone = "neutral",
  value,
}: {
  detail: string;
  label: string;
  tone?: "neutral" | "ready" | "warning";
  value: number | string;
}) {
  return (
    <span className={`vpsPostureMetric ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </span>
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
  const activity: Array<{
    detail: string;
    id: string;
    jobId?: string;
    kind: "alert" | "audit" | "backup" | "job" | "network" | "transfer";
    title: string;
    when: string;
  }> = [
    ...relatedAlerts.map((alert) => ({
      detail: `${alert.severity} · ${alert.operator_state} · ${formatTime(alert.observed_at)}`,
      id: alert.id,
      kind: "alert" as const,
      title: alert.title,
      when: alert.observed_at,
    })),
    ...relatedBackups.map((backup) => ({
      detail: `${backup.status} · ${backup.paths.join(", ") || "no paths"} · ${formatTime(backup.created_at)}`,
      id: backup.id,
      jobId: backup.source_job_id ?? undefined,
      kind: "backup" as const,
      title: `Backup ${shortId(backup.id)}`,
      when: backup.created_at,
    })),
    ...relatedTransfers.map((transfer) => ({
      detail: `${transfer.direction} · ${transfer.status} · ${transfer.path} · ${formatTime(transfer.observed_at)}`,
      id: transfer.session_id,
      jobId: transfer.last_job_id,
      kind: "transfer" as const,
      title: `Transfer ${shortId(transfer.session_id)}`,
      when: transfer.observed_at,
    })),
    ...relatedNetworkObservations.map((observation) => ({
      detail: `${observation.kind} · ${observation.interface_name ?? "interface n/a"} · ${formatTime(observation.observed_at)}`,
      id: observation.id,
      jobId: observation.job_id,
      kind: "network" as const,
      title: observation.healthy === false ? "Network degradation" : "Network observation",
      when: observation.observed_at,
    })),
    ...relatedJobs.map((job) => ({
      detail: `${job.command_type} · ${job.status} · ${job.target_count} targets · ${formatTime(job.created_at)}`,
      id: job.id,
      jobId: job.id,
      kind: "job" as const,
      title: `Job ${shortId(job.id)}`,
      when: job.created_at,
    })),
    ...relatedAudits.map((audit) => ({
      detail: `${audit.action} · ${audit.target} · ${formatTime(audit.created_at)}`,
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

function buildDetailCardSignal({
  activeAlertCount,
  backups,
  fileTransfers,
  relatedJobs,
}: {
  activeAlertCount: number;
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  relatedJobs: JobHistoryRecord[];
}): VpsMonitorCardSignal {
  const latestBackup = backups[0] ?? null;
  const latestTransfer = fileTransfers[0] ?? null;
  const failedJobs = relatedJobs.filter((job) => String(job.status).includes("fail")).length;
  const runningJobs = relatedJobs.filter((job) => ["queued", "running", "dispatching"].includes(String(job.status))).length;

  return {
    alertText: activeAlertCount > 0 ? `${activeAlertCount} active` : "none",
    alertTone: activeAlertCount > 0 ? "warning" : "ok",
    backupText: latestBackup ? String(latestBackup.status) : "no record",
    backupTone: latestBackup && String(latestBackup.status).includes("fail") ? "critical" : latestBackup ? "ok" : "neutral",
    jobText: runningJobs > 0 ? `${runningJobs} running` : failedJobs > 0 ? `${failedJobs} failed` : relatedJobs.length > 0 ? `${relatedJobs.length} recent` : "none",
    jobTone: failedJobs > 0 ? "critical" : runningJobs > 0 ? "info" : relatedJobs.length > 0 ? "ok" : "neutral",
    statusText: activeAlertCount > 0 ? "Review active alerts" : "No active alert records",
    transferText: latestTransfer ? String(latestTransfer.status) : "none",
    transferTone: latestTransfer && String(latestTransfer.status).includes("fail") ? "critical" : latestTransfer ? "info" : "neutral",
  };
}

function privilegeLabel(agent: AgentView) {
  if (agent.capabilities.privilege_mode === "root") return "root capable";
  if (agent.capabilities.privilege_mode === "unprivileged") return "unprivileged";
  return agent.capabilities.can_attempt_privileged_ops ? "privilege available" : "unknown";
}

function backupTone(backups: BackupRequestRecord[]): "neutral" | "ready" | "warning" {
  if (backups.length === 0) return "warning";
  return backups.some((backup) => String(backup.status).includes("fail"))
    ? "warning"
    : "ready";
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
