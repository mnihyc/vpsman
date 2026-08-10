import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

function source(relativePath: string) {
  return readFileSync(join(process.cwd(), "src", relativePath), "utf8");
}

test("keeps action feedback in dedicated local containers", () => {
  const accessPanel = source("panels/AccessPanel.tsx");
  expect(accessPanel).not.toMatch(
    /<span>\{revokeError\s*\?\?\s*"Block the current VPS gateway key"\}<\/span>/,
  );
  expect(accessPanel).not.toMatch(
    /identityError\s*\?\?\s*\(\s*identityMode\s*===\s*"rotate"/,
  );
  expect(accessPanel).toContain("accessRevokeActionFeedback");
  expect(accessPanel).toContain("identityActionFeedback");

  const systemPanel = source("panels/SystemPanel.tsx");
  expect(systemPanel).not.toMatch(
    /reviewPending\s*\?\s*"Preparing review"\s*:\s*`\$\{sessions\.length\} recent sessions`/,
  );
  expect(systemPanel).not.toContain(
    '{configError && <div className="panelError">{configError}</div>}',
  );
  expect(systemPanel).not.toContain(
    '{configMessage && <div className="panelSuccess">{configMessage}</div>}',
  );
  expect(systemPanel).toContain("systemSessionActionFeedback");
  expect(systemPanel).toContain("systemConfigActionFeedback");
  expect(systemPanel).not.toMatch(
    /<div className="panelError">\{error\}<\/div>/,
  );

  const jobsPanel = source("panels/JobsPanel.tsx");
  expect(jobsPanel).not.toMatch(
    /approvalActionError\s*&&\s*\(\s*<div className="panelError"/,
  );
  expect(jobsPanel).not.toContain(
    '{error ?? "No job records match the current search."}',
  );
  expect(jobsPanel).not.toMatch(/targetError\s*\?\?\s*\(\s*targetsLoading/);
  expect(jobsPanel).not.toMatch(
    /targetError\s*\?\?\s*"This job has no resolved per-client records\."/,
  );
  expect(jobsPanel).not.toMatch(
    /outputError\s*\?\?\s*downloadError\s*\?\?\s*\(\s*outputsLoading/,
  );
  expect(jobsPanel).not.toMatch(
    /outputError\s*\?\?\s*"This job has no retained stdout, stderr, or status output\."/,
  );
  expect(jobsPanel).not.toMatch(
    /comparisonError\s*\?\?\s*\(\s*comparisonLoading/,
  );
  expect(jobsPanel).toContain("approvalActionFeedback");
  expect(jobsPanel).toContain("jobDetailActionFeedback");
  expect(jobsPanel).not.toContain('className="vpsDetailNotice warning"');
  expect(jobsPanel).toContain("jobHistoryFreshnessFeedback");

  const fleetWorkspace = source("panels/FleetWorkspace.tsx");
  expect(fleetWorkspace).not.toMatch(/apiError\s*\?\?\s*\(\s*scopeActive\s*\?/);
  expect(fleetWorkspace).not.toContain(
    '{apiError ? "API unavailable" : "Live control-plane inventory"}',
  );
  expect(fleetWorkspace).not.toMatch(
    /const status\s*=\s*error\s*\?\?[\s\S]{0,260}bulkOutcomeSummary\(progress\)/,
  );
  expect(fleetWorkspace).toContain("<span>Live control-plane inventory</span>");
  expect(fleetWorkspace).toContain("networkInterfacesActionFeedback");
  expect(fleetWorkspace).not.toContain('className="notice infoNotice"');
  expect(fleetWorkspace).not.toContain('className="notice warningNotice"');
  expect(fleetWorkspace).toContain("policyFocusNotice");
  expect(fleetWorkspace).toContain("policyDryRunValidationFeedback");

  const vpsDetailPanel = source("panels/VpsDetailPanel.tsx");
  expect(vpsDetailPanel).not.toMatch(/apiError\s*\?\?\s*\(\s*loading\s*\?/);
  expect(vpsDetailPanel).not.toContain('className="vpsDetailNotice critical"');
  expect(vpsDetailPanel).toContain("ActionFeedback");
  expect(vpsDetailPanel).toContain("vpsDetailActionFeedback");

  const processSupervisorPanel = source(
    "panels/jobs/ProcessSupervisorInventoryPanel.tsx",
  );
  expect(processSupervisorPanel).not.toContain("processActionNotice");
  expect(processSupervisorPanel).toContain("processSupervisorActionFeedback");
  expect(processSupervisorPanel).toMatch(
    /message=\{\s*!stopProcess\s*&&\s*!restartProcess\s*\?\s*\(?actionError\s*\?\?\s*actionStatus\)?\s*:\s*null\s*\}/,
  );

  const hostServicesPanel = source("panels/jobs/HostServicesPanel.tsx");
  expect(hostServicesPanel).toContain("hostServiceActionFeedback");

  const fleetGroupsPanel = source("panels/FleetGroupsPanel.tsx");
  expect(fleetGroupsPanel).not.toMatch(
    /:\s*previewStatus\s*\?\?\s*bulkMutationPrimaryLabel/,
  );
  expect(fleetGroupsPanel).not.toMatch(
    /<span>\{previewStatus\s*\?\?\s*\(preview\s*\?/,
  );
  expect(fleetGroupsPanel).not.toContain("bulkPreviewFailure");
  expect(fleetGroupsPanel).toContain("bulkTagPreviewActionFeedback");

  const terminalSessionsPanel = source("panels/jobs/TerminalSessionsPanel.tsx");
  expect(terminalSessionsPanel).not.toMatch(
    /const terminalSummary\s*=\s*replayError\s*\?\?/,
  );
  expect(terminalSessionsPanel).not.toMatch(
    /\{launchStatus\s*\?\?\s*\(\s*privilegeReady/,
  );
  expect(terminalSessionsPanel).toContain("terminalLaunchActionFeedback");
  expect(terminalSessionsPanel).toContain("terminalReplayActionFeedback");

  const fileTransferSessionsPanel = source(
    "panels/jobs/FileTransferSessionsPanel.tsx",
  );
  expect(fileTransferSessionsPanel).not.toMatch(
    /const handoffSummary\s*=\s*handoffError\s*\?\?\s*handoffProgress\s*\?\?/,
  );
  expect(fileTransferSessionsPanel).not.toMatch(
    /<small>\{sourceError\s*\?\?\s*`\$\{sources\.length\} source artifacts`\}<\/small>/,
  );
  expect(fileTransferSessionsPanel).toContain("sourceArtifactActionFeedback");
  expect(fileTransferSessionsPanel).toContain("transferHandoffActionFeedback");

  const configurationSourcesPanel = source(
    "panels/ConfigurationSourcesPanel.tsx",
  );
  expect(configurationSourcesPanel).not.toContain(
    '{actionError ?? "No effective configuration"}',
  );
  expect(configurationSourcesPanel).toContain("localActionFeedback");
  expect(configurationSourcesPanel).toContain(
    "The selected targets did not match any VPS",
  );

  const auditLogPanel = source("panels/AuditLogPanel.tsx");
  expect(auditLogPanel).not.toMatch(/error\s*\?\?\s*\(\s*hasAuditFilters/);

  const jobEvidencePanel = source("panels/audit/JobEvidencePanel.tsx");
  expect(jobEvidencePanel).not.toContain('className="errorBanner"');
  expect(jobEvidencePanel).toContain("jobEvidencePageActionFeedback");
  expect(jobEvidencePanel).toContain("jobEvidenceDetailActionFeedback");

  const homeTelemetryPanel = source("panels/HomeTelemetryPanel.tsx");
  expect(homeTelemetryPanel).not.toContain(
    '{error && <div className="panelError">{error}</div>}',
  );
  expect(homeTelemetryPanel).toContain("homeTelemetryActionFeedback");

  const fleetMetricsPanel = source(
    "panels/observability/FleetMetricsPanel.tsx",
  );
  expect(fleetMetricsPanel).not.toContain(
    "panelError observabilityMetricsError",
  );
  expect(fleetMetricsPanel).toContain("fleetMetricsActionFeedback");

  const alertsPanel = source("panels/observability/AlertsPanel.tsx");
  expect(alertsPanel).not.toContain("panelError observabilityMetricsError");
  expect(alertsPanel).toContain("alertsActionFeedback");

  const webhooksPanel = source("panels/observability/WebhooksPanel.tsx");
  expect(webhooksPanel).not.toContain("panelError observabilityMetricsError");
  expect(webhooksPanel).toContain("webhooksActionFeedback");

  const dashboardsPanel = source(
    "panels/observability/ObservabilityDashboardsPanel.tsx",
  );
  expect(dashboardsPanel).not.toContain("panelError observabilityMetricsError");
  expect(dashboardsPanel).toContain("dashboardPageActionFeedback");

  const preferencesPanel = source("panels/PreferencesPanel.tsx");
  expect(preferencesPanel).not.toContain('<p className="preferencesError"');
  expect(preferencesPanel).not.toContain('<p className="preferencesNotice">');
  expect(preferencesPanel).toContain("preferencesActionFeedback");
  expect(preferencesPanel).toContain("preferencesSelectionActionFeedback");

  const topologyPanel = source("panels/TopologyPanel.tsx");
  expect(topologyPanel).not.toMatch(
    /const status\s*=\s*actionError\s*\?\?\s*error\s*\?\?/,
  );
  expect(topologyPanel).not.toMatch(
    /status\s*===\s*"Loading"\s*\?\s*"Loading tunnel plans"/,
  );
  expect(topologyPanel).not.toMatch(
    /toolbarActions=\{\s*automationBulkStatus\s*\?/,
  );
  expect(topologyPanel).toContain("topologyPlanActionFeedback");

  const topologyOspfControls = source(
    "panels/topology/TopologyOspfUpdateControls.tsx",
  );
  expect(topologyOspfControls).toContain("topologyOspfActionFeedback");

  const backupHistoryTables = source("panels/backups/BackupHistoryTables.tsx");
  expect(backupHistoryTables).not.toMatch(
    /if\s*\(error\)\s*\{[\s\S]{0,180}<div className="emptyState"/,
  );
  expect(backupHistoryTables).not.toMatch(/\berror:\s*string\s*\|\s*null/);

  const backupsPanel = source("panels/BackupsPanel.tsx");
  expect(backupsPanel).not.toMatch(
    /<BackupHistoryTables[\s\S]{0,360}\berror=\{error\}/,
  );

  const jobDispatchPanel = source("panels/JobDispatchPanel.tsx");
  expect(jobDispatchPanel).not.toMatch(
    /const status\s*=\s*visibleDispatchProgress/,
  );
  expect(jobDispatchPanel).toMatch(
    /\{!dispatchPromptOpen\s*&&[\s\S]{0,80}visibleDispatchProgress\s*&&/,
  );

  const topologyNetworkTests = source(
    "panels/topology/TopologyNetworkTestControls.tsx",
  );
  expect(topologyNetworkTests).not.toMatch(
    /const status\s*=\s*visibleJobProgress/,
  );
  expect(topologyNetworkTests).toMatch(
    /\{networkSnapshot\s*===\s*null\s*&&\s*visibleJobProgress\s*&&/,
  );
});
