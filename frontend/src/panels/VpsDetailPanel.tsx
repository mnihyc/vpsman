import { useEffect, useMemo, useState, type ReactNode } from "react";
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
  Play,
  Server,
  TerminalSquare,
} from "lucide-react";
import { agentDisplayState } from "../agentDisplayState";
import { ActionFeedback } from "../components/ActionFeedback";
import { handleTabListKeyDown, tabId } from "../components/AccessibleTabs";
import {
  formatLowerBoundCount,
  isActionableFleetAlertState,
} from "../constants";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import type {
  AgentView,
  AuditLogRecord,
  BackupArtifactRecord,
  BackupRequestRecord,
  ConfigurationSourceView,
  FleetAlertRecord,
  FleetAlertPolicyRecord,
  FleetSummary,
  JobHistoryRecord,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  PolicyAlertRecord,
  RuntimeConfigApplyStateRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
  VpsRuleValueRecord,
} from "../types";
import {
  dispatchFailureReason,
  displayNameOrUnnamed,
  formatCompactTime,
  formatFullTime,
  shortId,
  timestampMillis,
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

type VpsDetailRecordBounds = {
  audits: boolean;
  backupArtifacts: boolean;
  backups: boolean;
  fileTransfers: boolean;
  fleetAlerts: boolean;
  jobs: boolean;
};

type VpsDetailPanelProps = {
  agent: AgentView | null;
  agents: AgentView[];
  apiError: string | null;
  audits: AuditLogRecord[];
  backupArtifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  fleetAlertsTruncated: boolean;
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  jobs: JobHistoryRecord[];
  recordBounds: VpsDetailRecordBounds;
  loading: boolean;
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  onOpenAudit: () => void;
  onOpenAlertPolicies: (policyId?: string) => void;
  onOpenBackup: (agent: AgentView) => void;
  onOpenConfig: (agent: AgentView) => void;
  onOpenDispatch: (agent: AgentView) => void;
  onOpenFiles: (agent: AgentView) => void;
  onOpenFleetAlerts: () => void;
  onOpenFleetMetrics: (agent: AgentView) => void;
  onOpenInstances: () => void;
  onOpenJob: (jobId: string) => void;
  onOpenJobs: () => void;
  onLoadConfigurationSources: () => Promise<void>;
  onOpenNetwork: (agent: AgentView) => void;
  onOpenNetworkEvidence: (agent: AgentView) => void;
  onOpenProcesses: (agent: AgentView) => void;
  onOpenTerminal: (agent: AgentView) => void;
  policyAlerts: PolicyAlertRecord[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  runtimeConfigEvidenceState: "available" | "loading" | "unavailable";
  configurationSources: ConfigurationSourceView[];
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
  fleetAlertsTruncated,
  fleetAlertPolicies,
  jobs,
  recordBounds,
  loading,
  networkObservations,
  networkTrends,
  onOpenAudit,
  onOpenAlertPolicies,
  onOpenBackup,
  onOpenConfig,
  onOpenDispatch,
  onOpenFiles,
  onOpenFleetAlerts,
  onOpenFleetMetrics,
  onOpenInstances,
  onOpenJob,
  onOpenJobs,
  onLoadConfigurationSources,
  onOpenNetwork,
  onOpenNetworkEvidence,
  onOpenProcesses,
  onOpenTerminal,
  policyAlerts,
  runtimeConfigApplyStates,
  runtimeConfigEvidenceState,
  configurationSources,
  telemetryNetworkRates,
  telemetryRollups,
  telemetryTunnels,
  vpsRuleValues,
}: VpsDetailPanelProps) {
  const [activeTab, setActiveTab] = useState<VpsDetailTab>("Summary");
  const [sourceLoadError, setSourceLoadError] = useState<string | null>(null);
  useEffect(() => {
    if (!agent) {
      setSourceLoadError(null);
      return;
    }
    let active = true;
    setSourceLoadError(null);
    void onLoadConfigurationSources().catch((error) => {
      if (active) {
        setSourceLoadError(
          error instanceof Error
            ? `Configuration sources: ${error.message}`
            : "Configuration sources are unavailable",
        );
      }
    });
    return () => {
      active = false;
    };
  }, [agent, onLoadConfigurationSources]);
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
            fleetAlertPolicies,
            jobs,
            networkObservations,
            networkTrends,
            policyAlerts,
            runtimeConfigApplyStates:
              runtimeConfigEvidenceState === "available"
                ? runtimeConfigApplyStates
                : [],
            configurationSources,
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
      fleetAlertPolicies,
      jobs,
      networkObservations,
      networkTrends,
      policyAlerts,
      runtimeConfigApplyStates,
      runtimeConfigEvidenceState,
      configurationSources,
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
          <ActionFeedback
            className="localActionFeedback"
            message={apiError ?? sourceLoadError}
            tone="danger"
          />
          <div className="emptyState">
            <Server size={22} />
            <strong>{agents.length === 0 ? "No VPS inventory" : "No VPS selected"}</strong>
            <span>
              {loading
                ? "Loading fleet inventory before opening the canonical detail page."
                : "Open a VPS from an inventory row, monitor card, alert, job target, backup record, or network node."}
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

  const activeAlertCount = related.alerts.filter((alert) =>
    isActionableFleetAlertState(alert.operator_state),
  ).length;
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
            <button className="secondaryAction compactAction" onClick={() => onOpenDispatch(agent)} type="button">
              <Play size={14} />
              <span>Run command</span>
            </button>
            <button className="secondaryAction compactAction" onClick={() => onOpenBackup(agent)} type="button">
              <DatabaseBackup size={14} />
              <span>Back up</span>
            </button>
            <button className="secondaryAction compactAction" onClick={() => onOpenConfig(agent)} type="button">
              <FileCog size={14} />
              <span>Config</span>
            </button>
          </div>
        </div>

        <ActionFeedback
          className="localActionFeedback vpsDetailActionFeedback"
          message={apiError ?? sourceLoadError}
          tone="danger"
        />

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
              detail={agent.arch ? `Architecture ${agent.arch}` : "Architecture unavailable"}
            />
            <VpsResourceFact
              icon={<AlertTriangle size={16} />}
              label="Alerts"
              value={
                fleetAlertsTruncated && activeAlertCount === 0
                  ? "None in loaded page"
                  : `${formatLowerBoundCount(activeAlertCount, fleetAlertsTruncated)} active`
              }
              detail={`${related.alerts.length} loaded records${fleetAlertsTruncated ? "; the fleet alert page is capped" : ""}`}
              tone={
                activeAlertCount > 0 || fleetAlertsTruncated
                  ? "warning"
                  : "ready"
              }
            />
            <VpsResourceFact
              icon={<History size={16} />}
              label="Active jobs"
              value={
                recordBounds.jobs && activeJobCount === 0
                  ? "None in loaded history"
                  : `${formatLowerBoundCount(
                      activeJobCount,
                      recordBounds.jobs,
                    )} active${recordBounds.jobs ? " loaded" : ""}`
              }
              detail={`${formatLowerBoundCount(
                related.relatedJobs.length,
                recordBounds.jobs,
              )} related job records${recordBounds.jobs ? " in loaded history; more may exist" : ""}`}
              tone={
                activeJobCount > 0 || recordBounds.jobs
                  ? "warning"
                  : "neutral"
              }
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

        <div
          className="detailTabs"
          role="tablist"
          aria-label="VPS detail tabs"
          onKeyDown={handleTabListKeyDown}
        >
          {detailTabs.map((tab) => (
            <button
              aria-controls="vps-detail-tabpanel"
              aria-selected={activeTab === tab}
              className={activeTab === tab ? "selected" : ""}
              id={tabId("vps-detail", tab)}
              key={tab}
              onClick={() => setActiveTab(tab)}
              role="tab"
              tabIndex={activeTab === tab ? 0 : -1}
              type="button"
            >
              {tab}
            </button>
          ))}
        </div>

        <div
          aria-labelledby={tabId("vps-detail", activeTab)}
          className="vpsDetailTabPanel"
          id="vps-detail-tabpanel"
          role="tabpanel"
        >
          {activeTab === "Summary" && (
            <SummaryTab
              agent={agent}
              latestJob={latestJob}
              loading={loading}
              related={related}
              recordBounds={recordBounds}
              onOpenFleetAlerts={onOpenFleetAlerts}
              onOpenFleetMetrics={() => onOpenFleetMetrics(agent)}
              onOpenJob={onOpenJob}
            />
          )}
          {activeTab === "Remote access" && (
            <ActionTab
              icon={<TerminalSquare size={18} />}
              loading={loading}
              title="Remote access"
              description="Open browser terminal sessions from Remote. Session lifecycle, replay, input, resize, and close controls stay there."
              primary={{ label: "Open terminal", onClick: () => onOpenTerminal(agent) }}
              rows={[
                ["Agent status", agent.status],
                ["Privilege mode", privilegeLabel(agent)],
                ["Max timeout", `${agent.capabilities.max_job_timeout_secs}s`],
                ["Local workflow", "Remote / Terminal"],
              ]}
            />
          )}
          {activeTab === "Files" && (
            <ActionTab
              icon={<FolderOpen size={18} />}
              loading={loading}
              title="Files"
              description="Browse, transfer, edit, and review file operations from Remote / Files."
              primary={{ label: "Browse files", onClick: () => onOpenFiles(agent) }}
              rows={[
                [
                  "Transfer sessions",
                  `${formatLowerBoundCount(
                    related.fileTransfers.length,
                    recordBounds.fileTransfers,
                  )}${recordBounds.fileTransfers ? " loaded" : ""}`,
                ],
                [
                  "Latest transfer",
                  related.fileTransfers[0]
                    ? `${related.fileTransfers[0].direction} ${related.fileTransfers[0].status}`
                    : recordBounds.fileTransfers
                      ? "None in loaded history; more may exist"
                      : "No transfer record",
                ],
                ["Latest path", related.fileTransfers[0]?.path ?? "No path recorded"],
              ]}
            />
          )}
          {activeTab === "Processes" && (
            <ActionTab
              icon={<Activity size={18} />}
              loading={loading}
              title="Processes"
              description="Inspect process inventory, logs, restarts, and reviewed stop/restart work from Remote / Processes."
              primary={{ label: "Open processes", onClick: () => onOpenProcesses(agent) }}
              rows={[
                ["Process limits", agent.capabilities.can_apply_process_limits ? "Supported" : "Not reported"],
                ["Privilege mode", privilegeLabel(agent)],
                ["Workflow", "Remote / Processes"],
              ]}
            />
          )}
          {activeTab === "Config" && (
            <ConfigTab
              agent={agent}
              loading={loading}
              related={related}
              runtimeConfigEvidenceState={runtimeConfigEvidenceState}
              onOpenConfig={() => onOpenConfig(agent)}
            />
          )}
          {activeTab === "Backups" && (
            <BackupsTab
              loading={loading}
              related={related}
              recordBounds={recordBounds}
              onOpenBackup={() => onOpenBackup(agent)}
              onOpenJob={onOpenJob}
            />
          )}
          {activeTab === "Network" && (
            <NetworkTab
              loading={loading}
              related={related}
              onOpenAlertPolicies={onOpenAlertPolicies}
              onOpenConfig={() => onOpenConfig(agent)}
              onOpenFleetAlerts={onOpenFleetAlerts}
              onOpenNetwork={() => onOpenNetwork(agent)}
              onOpenNetworkEvidence={() => onOpenNetworkEvidence(agent)}
            />
          )}
          {activeTab === "Activity" && (
            <ActivityTab
              loading={loading}
              related={related}
              recordBounds={recordBounds}
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
  recordBounds,
  onOpenFleetAlerts,
  onOpenFleetMetrics,
  onOpenJob,
}: {
  agent: AgentView;
  latestJob: JobHistoryRecord | null;
  loading: boolean;
  related: VpsDetailContext;
  recordBounds: VpsDetailRecordBounds;
  onOpenFleetAlerts: () => void;
  onOpenFleetMetrics: () => void;
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
        <button className="secondaryAction compactAction" onClick={onOpenFleetMetrics} type="button">
          <Activity size={14} />
          View retained metrics
        </button>
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
              {jobStatusLabel(latestJob.status)} · {latestJob.target_count} target
              {latestJob.target_count === 1 ? "" : "s"} ·{" "}
              <DetailTime value={latestJob.created_at} />
            </span>
          </button>
        ) : (
          <DetailState
            loading={loading}
            title={
              recordBounds.jobs
                ? "No related job in loaded history"
                : "No related job evidence"
            }
            detail={
              recordBounds.jobs
                ? `No retained job target evidence for ${displayNameOrUnnamed(agent.display_name)} appears in the loaded job page; more history may exist.`
                : `No retained job target evidence is loaded for ${displayNameOrUnnamed(agent.display_name)}.`
            }
          />
        )}
        {related.backups[0] ? (
          <span className="vpsDetailRecord static">
            <strong>Backup {shortId(related.backups[0].id)}</strong>
            <span>
              {backupStatusLabel(related.backups[0].status)} ·{" "}
              <DetailTime value={related.backups[0].created_at} />
            </span>
          </span>
        ) : recordBounds.backups ? (
          <DetailState
            loading={loading}
            title="No backup in loaded history"
            detail="More backup history may exist outside the loaded page."
          />
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
  runtimeConfigEvidenceState,
  onOpenConfig,
}: {
  agent: AgentView;
  loading: boolean;
  related: VpsDetailContext;
  runtimeConfigEvidenceState: "available" | "loading" | "unavailable";
  onOpenConfig: () => void;
}) {
  const configPosture = buildConfigPosture(
    related,
    runtimeConfigEvidenceState,
  );
  const sourceIssueRows = sourceRowsNeedingAttention(
    related.configurationSources,
  );
  const sourceReadyRows = related.configurationSources.filter(
    sourceRowIsReady,
  );
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
          <span>Open per-VPS config</span>
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
          {related.configurationSources.length === 0 ? (
            <DetailState loading={loading} title="No configuration source evidence" detail="No effective preset, sync, or readiness records are loaded for this VPS." />
          ) : (
            <>
              {sourceIssueRows.length > 0 ? (
                sourceIssueRows.map((record) => (
                  <span className="vpsDetailRecord static warning" key={`issue:${record.behavior}:${record.effective_preset_id}`}>
                    <strong>{readableDetailToken(record.behavior)}</strong>
                    <span>{sourceReadinessStatusLabel(record.readiness.state)} · {sourceReadinessReasonLabel(record)}</span>
                  </span>
                ))
              ) : (
                <DetailState
                  loading={loading}
                  title={
                    sourceReadyRows.length ===
                    related.configurationSources.length
                      ? "Sources verified ready"
                      : "No source blockers"
                  }
                  detail={
                    sourceReadyRows.length ===
                    related.configurationSources.length
                      ? "All loaded effective sources are synced and verified ready."
                      : `${sourceReadyRows.length} of ${related.configurationSources.length} loaded sources are verified ready; offline or unverified sources remain neutral.`
                  }
                />
              )}
              {related.configurationSources.map((record) => (
                <span className="vpsDetailRecord static" key={`status:${record.behavior}:${record.effective_preset_id}`}>
                  <strong>{readableDetailToken(record.behavior)}</strong>
                  <span>{record.effective_preset_name} · {record.selection_origin === "explicit_override" ? "explicit override" : "inherited system default"}</span>
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
          <VpsFact label="Effective sources" value={String(related.configurationSources.length)} />
          <VpsFact label="Explicit overrides" value={String(related.configurationSources.filter((source) => source.selection_origin === "explicit_override").length)} />
          <VpsFact label="VPS rules" value={String(related.vpsRules.length)} />
          <VpsFact
            label="Last apply"
            value={
              runtimeConfigEvidenceState === "loading"
                ? "Checking evidence"
                : runtimeConfigEvidenceState === "unavailable"
                  ? "Evidence unavailable"
                  : runtimeApplyTimeLabel(applyState)
            }
          />
          <VpsFact
            label="Apply status"
            value={
              runtimeConfigEvidenceState === "loading"
                ? "Checking apply state"
                : runtimeConfigEvidenceState === "unavailable"
                  ? "Apply state unavailable"
                  : runtimeApplyStatusLabel(applyState)
            }
          />
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
              {related.configurationSources.length === 0 ? (
                <span>No raw configuration source records loaded.</span>
              ) : (
                related.configurationSources.map((record) => (
                  <span key={`raw:${record.behavior}:${record.effective_preset_id}`}>
                    <strong>{record.behavior}</strong>
                    <code>{record.runtime_sync.state}</code>
                    <small>{record.runtime_sync.reason} · {record.readiness.reason}</small>
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
  recordBounds,
  onOpenBackup,
  onOpenJob,
}: {
  loading: boolean;
  related: VpsDetailContext;
  recordBounds: VpsDetailRecordBounds;
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
          <DetailState
            loading={loading}
            title={
              recordBounds.backups
                ? "No backup request in loaded history"
                : "No backup requests"
            }
            detail={
              recordBounds.backups
                ? "No backup request for this VPS appears in the loaded page; more history may exist."
                : "No current backup request record is loaded for this VPS."
            }
          />
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
          <DetailState
            loading={loading}
            title={
              recordBounds.backupArtifacts
                ? "No artifact in loaded history"
                : "No artifacts"
            }
            detail={
              recordBounds.backupArtifacts
                ? "No backup artifact for this VPS appears in the loaded page; more history may exist."
                : "No retained backup artifact metadata is loaded for this VPS."
            }
          />
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
  onOpenAlertPolicies,
  onOpenConfig,
  onOpenFleetAlerts,
  onOpenNetwork,
  onOpenNetworkEvidence,
}: {
  loading: boolean;
  related: VpsDetailContext;
  onOpenAlertPolicies: (policyId?: string) => void;
  onOpenConfig: () => void;
  onOpenFleetAlerts: () => void;
  onOpenNetwork: () => void;
  onOpenNetworkEvidence: () => void;
}) {
  const latestRate = newestNetworkRate(related.networkRates);
  const trafficRules = related.vpsRules.filter((rule) =>
    rule.key.startsWith("traffic."),
  );
  const trafficPolicyAlerts = related.policyAlerts.filter((alert) =>
    `${alert.category} ${alert.title} ${alert.detail} ${JSON.stringify(alert.payload)}`
      .toLowerCase()
      .includes("traffic"),
  );
  const primaryPolicyId = trafficPolicyAlerts[0]?.policy_group_id;

  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Network workflow" icon={<Network size={18} />}>
        <button className="primaryAction compactAction" onClick={onOpenNetwork} type="button">
          <span>Open network graph</span>
        </button>
        <button
          className="secondaryAction compactAction"
          onClick={onOpenNetworkEvidence}
          title="Open fleet-wide retained network evidence; use the selected graph node for this VPS context."
          type="button"
        >
          <span>Fleet evidence</span>
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
      <DetailBlock title="Traffic & Rules" icon={<Gauge size={18} />}>
        <button className="primaryAction compactAction" onClick={onOpenConfig} type="button">
          <span>Edit VPS Rules</span>
        </button>
        <button
          className="secondaryAction compactAction"
          onClick={() => onOpenAlertPolicies(primaryPolicyId)}
          type="button"
        >
          <span>Open Alert Policy</span>
        </button>
        <button className="secondaryAction compactAction" onClick={onOpenFleetAlerts} type="button">
          <span>Open Fleet Alerts</span>
        </button>
        <VpsFact label="Selected traffic" value={trafficSelectorLabel(trafficRules)} />
        <VpsFact
          label="Latest avg RX"
          value={latestRate ? formatBytesPerSecond(latestRate.rx_bps_avg) : "No rate"}
        />
        <VpsFact
          label="Latest avg TX"
          value={latestRate ? formatBytesPerSecond(latestRate.tx_bps_avg) : "No rate"}
        />
        <VpsFact
          label="Cycle Total"
          value={
            latestRate
              ? formatBytes(latestRate.rx_bytes_delta + latestRate.tx_bytes_delta)
              : "No sample"
          }
        />
        {trafficRules.length === 0 ? (
          <DetailState loading={loading} title="No traffic rules" detail="No traffic-scoped VPS rules are loaded for this VPS." />
        ) : (
          trafficRules.slice(0, 6).map((rule) => (
            <span className={`vpsDetailRecord static ${rule.validation_errors.length ? "warning" : ""}`} key={rule.key}>
              <strong>{rule.key}</strong>
              <span>{rule.parsed_display || rule.value_raw || "unset"}</span>
            </span>
          ))
        )}
      </DetailBlock>
      <DetailBlock title="Matched policies" icon={<AlertTriangle size={18} />}>
        <strong>Recent policy alerts</strong>
        {trafficPolicyAlerts.length === 0 ? (
          <DetailState loading={loading} title="No policy alerts" detail="No traffic policy alert is loaded for this VPS." />
        ) : (
          trafficPolicyAlerts.slice(0, 4).map((alert) => (
            <span className="vpsDetailRecord static warning" key={alert.id}>
              <strong>{trafficPolicyLabel(alert, related.alertPolicies)}</strong>
              <span>{trafficPolicyRuleLabel(alert, related.alertPolicies)} · {alert.detail}</span>
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
  recordBounds,
  onOpenAudit,
  onOpenFleetAlerts,
  onOpenJob,
  onOpenJobs,
}: {
  loading: boolean;
  related: VpsDetailContext;
  recordBounds: VpsDetailRecordBounds;
  onOpenAudit: () => void;
  onOpenFleetAlerts: () => void;
  onOpenJob: (jobId: string) => void;
  onOpenJobs: () => void;
}) {
  return (
    <div className="vpsDetailGrid">
      <DetailBlock title="Correlated events" icon={<History size={18} />}>
        {related.activity.length === 0 ? (
          <DetailState
            loading={loading}
            title="No correlated activity"
            detail={
              recordBounds.audits ||
              recordBounds.jobs ||
              recordBounds.backups ||
              recordBounds.fileTransfers ||
              recordBounds.fleetAlerts
                ? "No target-scoped record for this VPS appears in the loaded history pages; more activity may exist."
                : "Loaded activity does not include target-scoped records for this VPS yet."
            }
          />
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
        <VpsFact
          label="Alerts"
          value={
            recordBounds.fleetAlerts && related.alerts.length === 0
              ? "None in loaded history"
              : `${formatLowerBoundCount(
                  related.alerts.length,
                  recordBounds.fleetAlerts,
                )}${recordBounds.fleetAlerts ? " loaded" : ""}`
          }
        />
        <VpsFact
          label="Jobs"
          value={`${formatLowerBoundCount(
            related.relatedJobs.length,
            recordBounds.jobs,
          )}${recordBounds.jobs ? " loaded" : ""}`}
        />
        <VpsFact
          label="Audits"
          value={`${formatLowerBoundCount(
            related.audits.length,
            recordBounds.audits,
          )}${recordBounds.audits ? " loaded" : ""}`}
        />
        <VpsFact
          label="Backups"
          value={`${formatLowerBoundCount(
            related.backups.length,
            recordBounds.backups,
          )}${recordBounds.backups ? " loaded" : ""}`}
        />
        <VpsFact
          label="Artifacts"
          value={`${formatLowerBoundCount(
            related.backupArtifacts.length,
            recordBounds.backupArtifacts,
          )}${recordBounds.backupArtifacts ? " loaded" : ""}`}
        />
        <VpsFact
          label="Transfers"
          value={`${formatLowerBoundCount(
            related.fileTransfers.length,
            recordBounds.fileTransfers,
          )}${recordBounds.fileTransfers ? " loaded" : ""}`}
        />
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
      <small>{loading ? "Refresh is still in progress for this detail view." : detail}</small>
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
  fleetAlertPolicies,
  jobs,
  networkObservations,
  networkTrends,
  policyAlerts,
  runtimeConfigApplyStates,
  configurationSources,
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
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  jobs: JobHistoryRecord[];
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  policyAlerts: PolicyAlertRecord[];
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
  configurationSources: ConfigurationSourceView[];
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
  const relatedPolicyAlerts = policyAlerts
    .filter((alert) => alert.client_id === clientId)
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
  const relatedConfigurationSources = configurationSources.filter(
    (source) => source.client_id === clientId,
  );
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
      detail: `${displayCommandType(job.command_type)} · ${jobStatusLabel(job.status)} · ${job.target_count} target${job.target_count === 1 ? "" : "s"} · ${formatCompactTime(job.created_at)}`,
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
    alertPolicies: fleetAlertPolicies,
    networkObservations: relatedNetworkObservations,
    networkRates,
    networkTrends: relatedNetworkTrends,
    policyAlerts: relatedPolicyAlerts,
    relatedJobs,
    rollup,
    runtimeApplyState,
    configurationSources: relatedConfigurationSources,
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

function buildConfigPosture(
  related: VpsDetailContext,
  runtimeConfigEvidenceState: "available" | "loading" | "unavailable",
): ConfigPostureItem[] {
  const sourceIssues = sourceRowsNeedingAttention(
    related.configurationSources,
  );
  const sourceReadyCount = related.configurationSources.filter(
    sourceRowIsReady,
  ).length;
  const allSourcesReady =
    related.configurationSources.length > 0 &&
    sourceReadyCount === related.configurationSources.length;
  const ruleErrors = related.vpsRules.flatMap((rule) => rule.validation_errors);
  const applyState = related.runtimeApplyState;
  const applyEvidenceDetail =
    runtimeConfigEvidenceState === "loading"
      ? "Runtime apply evidence is still loading"
      : "Runtime apply evidence is unavailable";
  const applyEvidenceKnown = runtimeConfigEvidenceState === "available";
  const lastError =
    (applyEvidenceKnown && applyState?.pending_status === "failed"
      ? dispatchFailureReason(
          applyState.pending_error,
          applyState.pending_status,
          "Runtime config apply",
        )
      : null) ||
    ruleErrors[0] ||
    (sourceIssues[0]
      ? sourceReadinessReasonLabel(sourceIssues[0])
      : null) ||
    null;
  return [
    {
      detail:
        related.configurationSources.length > 0
          ? sourceDomainSummary(
              related.configurationSources.map((source) => source.behavior),
            )
          : "No effective source evidence loaded",
      label: "Effective sources",
      tone:
        related.configurationSources.length > 0 ? "info" : "neutral",
      value:
        related.configurationSources.length > 0
          ? `${related.configurationSources.length} behaviors`
          : "Unknown",
    },
    {
      detail:
        sourceIssues[0] !== undefined
          ? sourceReadinessReasonLabel(sourceIssues[0])
          : allSourcesReady
            ? "All loaded effective sources are synced and verified ready"
            : related.configurationSources.length > 0
              ? `${sourceReadyCount} of ${related.configurationSources.length} loaded sources are verified ready; offline or unverified sources remain neutral`
              : "No readiness records loaded",
      label: "Source state",
      tone:
        sourceIssues.length > 0
          ? "warning"
          : allSourcesReady
            ? "ok"
            : "neutral",
      value: sourceIssues.length > 0
        ? "Needs attention"
        : allSourcesReady
          ? "Ready"
          : related.configurationSources.length > 0
            ? "Not verified"
            : "Unknown",
    },
    {
      detail: applyEvidenceKnown
        ? configDriftDetail(
            applyState,
            sourceIssues.length,
            ruleErrors.length,
          )
        : `${applyEvidenceDetail}; cached state is not used for drift claims.`,
      label: "Drift state",
      tone: applyEvidenceKnown
        ? configDriftTone(
            applyState,
            sourceIssues.length,
            ruleErrors.length,
          )
        : sourceIssues.length > 0 || ruleErrors.length > 0
          ? "warning"
          : "neutral",
      value: applyEvidenceKnown
        ? configDriftLabel(
            applyState,
            sourceIssues.length,
            ruleErrors.length,
          )
        : runtimeConfigEvidenceState === "loading"
          ? "Checking apply"
          : "Apply unknown",
    },
    {
      detail: applyEvidenceKnown
        ? runtimeApplyDetail(applyState)
        : `${applyEvidenceDetail}; cached state is not treated as current.`,
      label: "Last apply",
      tone: applyEvidenceKnown
        ? applyState?.pending_status === "failed"
          ? "critical"
          : applyState?.applied_at
            ? "ok"
            : "neutral"
        : "neutral",
      value: applyEvidenceKnown
        ? runtimeApplyTimeLabel(applyState)
        : runtimeConfigEvidenceState === "loading"
          ? "Checking evidence"
          : "Unavailable",
    },
    {
      detail:
        lastError ??
        (applyEvidenceKnown
          ? "No loaded config error"
          : `${applyEvidenceDetail}; a latest apply error cannot be confirmed.`),
      label: "Last error",
      tone: lastError ? "warning" : applyEvidenceKnown ? "ok" : "neutral",
      value: lastError
        ? "Needs review"
        : applyEvidenceKnown
          ? "None"
          : "Unknown",
    },
  ];
}

function sourceRowsNeedingAttention(
  rows: ConfigurationSourceView[],
): ConfigurationSourceView[] {
  return rows.filter(
    (row) =>
      ["failed", "stale"].includes(row.runtime_sync.state) ||
      ["degraded", "failed", "invalid"].includes(row.readiness.state),
  );
}

function sourceRowIsReady(row: ConfigurationSourceView): boolean {
  return (
    row.runtime_sync.state === "applied" &&
    row.readiness.state === "ready"
  );
}

function sourceReadinessStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    degraded: "Degraded",
    failed: "Failed",
    invalid: "Invalid",
    ready: "Ready",
    unavailable: "VPS offline",
    unverified: "Not verified",
  };
  return labels[status] ?? readableDetailToken(status);
}

function sourceReadinessReasonLabel(
  record: ConfigurationSourceView,
): string {
  if (["failed", "stale"].includes(record.runtime_sync.state)) {
    return sentenceCase(
      record.runtime_sync.reason ||
        `Runtime sync ${record.runtime_sync.state}`,
    );
  }
  return sentenceCase(
    record.readiness.reason ||
      sourceReadinessStatusLabel(record.readiness.state),
  );
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
  if (state?.pending_status === "failed") {
    return dispatchFailureReason(
      state.pending_error,
      state.pending_status,
      "Runtime config apply",
    );
  }
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
  if (state.pending_status === "failed") {
    return dispatchFailureReason(
      state.pending_error,
      state.pending_status,
      "Runtime config apply",
    );
  }
  if (state.pending_status === "queued") return state.pending_reason ?? "Runtime config apply queued";
  if (state.applied_content_hash) {
    const job = state.applied_job_id ? `; job ${shortId(state.applied_job_id)}` : "";
    return `Hash ${shortId(state.applied_content_hash)}${job}`;
  }
  return "No server-applied runtime sync recorded";
}

function runtimeApplyStateTime(state: RuntimeConfigApplyStateRecord): string {
  return state.pending_updated_at ?? state.applied_at ?? state.updated_at;
}

function runtimeApplyQueuedIsStale(state: RuntimeConfigApplyStateRecord): boolean {
  const updatedAt = timestampMillis(runtimeApplyStateTime(state));
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
      return "Argv command";
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
    artifact_metadata_recorded: "Package linked",
    artifact_uploaded: "Artifact uploaded",
    completed: "Completed",
    creating: "Preparing package",
    deleted: "Deleted",
    delete_failed: "Delete failed",
    missing: "Package unavailable",
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

function newestNetworkRate(rates: TelemetryNetworkRateRecord[]) {
  return (
    rates
      .slice()
      .sort((left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at))[0] ??
    null
  );
}

function formatBytesPerSecond(value: number) {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B/s";
  }
  if (value >= 1024 * 1024) {
    return `${(value / 1024 / 1024).toFixed(1)} MiB/s`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB/s`;
  }
  return `${Math.round(value)} B/s`;
}

function trafficSelectorLabel(rules: VpsRuleValueRecord[]) {
  const selectorRule = rules.find((rule) => rule.key === "traffic.selectors");
  return selectorRule?.value_raw || selectorRule?.parsed_display || "No selector";
}

function trafficPolicyLabel(
  alert: PolicyAlertRecord,
  policies: FleetAlertPolicyRecord[],
) {
  return (
    policies.find((policy) => policy.id === alert.policy_group_id)?.name ??
    shortId(alert.policy_group_id)
  );
}

function trafficPolicyRuleLabel(
  alert: PolicyAlertRecord,
  policies: FleetAlertPolicyRecord[],
) {
  const policy = policies.find((candidate) => candidate.id === alert.policy_group_id);
  const rule = policy?.rules.find((candidate) => candidate.id === alert.policy_rule_id);
  const payload = recordValue(alert.payload);
  const payloadRule = recordValue(payload?.rule);
  return (
    rule?.name ??
    stringValue(payloadRule?.name) ??
    stringValue(payload?.rule_name) ??
    alert.title
  );
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
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
