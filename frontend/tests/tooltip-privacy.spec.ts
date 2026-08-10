import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { AuditLogRecord, BackupPolicyRecord } from "../src/types";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

function source(relativePath: string) {
  return readFileSync(join(process.cwd(), "src", relativePath), "utf8");
}

test("does not promote hidden data-grid search metadata into titles", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared data-grid tooltip contract is covered on desktop",
  );
  const hiddenSearchSentinel = "grid-hidden-search-secret-8c31";
  const audit: AuditLogRecord = {
    action: "job.dispatch_requested",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: null,
    created_at: "2026-08-08T08:31:00Z",
    id: "audit-tooltip-privacy-0001",
    metadata: {
      command_type: "shell_argv",
      component: "job-submission-controller",
      hidden_search_only: hiddenSearchSentinel,
      origin_kind: "operator_request",
      result: "requested",
      target_count: 1,
    },
    target: "api:/api/v1/jobs",
  };
  await installConsoleApiMock(page, { auditLogsOverride: [audit] });
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Events");

  const grid = page.getByLabel("Audit records data grid");
  await expect(grid.locator(".gridBody [role=row]")).toHaveCount(1);
  await expect
    .poll(async () => grid.locator("[title]").count())
    .toBeGreaterThan(0);
  const titles = await grid
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? ""),
    );
  expect(titles.join("\n")).not.toContain(hiddenSearchSentinel);
});

test("keeps the tooltip foundation supplemental and preserves authored evidence", () => {
  const decorator = source("useValueTooltips.ts");
  expect(decorator).toContain("function isVisiblyShortened");
  expect(decorator).toContain('style.textOverflow === "ellipsis"');
  expect(decorator).not.toContain("\"input:not([type='hidden'])\"");
  expect(decorator).not.toMatch(
    /Activate |Current value:|(?:excluded|omitted) from (?:the )?tooltips?|\bfield\.|\bcolumn\./i,
  );

  const grid = source("components/ConsoleDataGrid.tsx");
  const gridTooltipHelper = grid.slice(
    grid.indexOf("function columnTooltip"),
    grid.indexOf("function SortableHeaderCell"),
  );
  expect(gridTooltipHelper).not.toContain("searchValue");
  expect(gridTooltipHelper).not.toContain("sortValue");

  const confirmation = source("components/ConfirmationPrompt.tsx");
  expect(confirmation).not.toContain("confirmationItemTitle");
  expect(confirmation).toContain(
    'data-tooltip-sensitive={item.sensitive ? "true" : undefined}',
  );
  expect(confirmation).toContain(
    'data-value-tooltip-skip={item.sensitive ? "true" : undefined}',
  );

  for (const relativePath of [
    "components/Metric.tsx",
    "panels/HomeTelemetryPanel.tsx",
    "panels/PreferencesPanel.tsx",
    "panels/ReleaseStatusPanel.tsx",
    "panels/TargetImpactPreview.tsx",
    "panels/automation/RunbooksPanel.tsx",
    "panels/observability/FleetMetricsPanel.tsx",
    "panels/observability/ObservabilityDashboardsPanel.tsx",
  ]) {
    expect(source(relativePath)).not.toMatch(
      /title=\{`\$\{(?:label|definition\.label)\}: \$\{(?:value|definition\.value)\}/,
    );
  }

  const access = source("panels/AccessPanel.tsx");
  expect(access).not.toContain("title={foregroundStartCommand}");
  expect(access).not.toContain("title={installCommand}");
  expect(access).toContain(
    'title="Paste-ready install command containing the one-time private key; copy it only into a trusted shell."',
  );
  expect(access).toContain(
    'title="Foreground command for a staged unprivileged installation."',
  );

  const dispatch = source("panels/JobDispatchPanel.tsx");
  expect(dispatch).toMatch(/label:\s*"Command argv",\s*value:\s*command/);
  expect(dispatch).not.toMatch(/label:\s*"Command argv",\s*sensitive:\s*true/);
  expect(dispatch).toContain(
    '`${environmentNames.join(", ")} (values hidden)`',
  );
});

test("keeps editable Suite Config values out of input titles", async ({
  page,
}) => {
  const credentialUrl =
    "https://tooltip-user:credential-bearing-url-sentinel@example.invalid/control";
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Suite config");

  const sections = page.getByLabel("Suite config sections");
  await sections.getByRole("button", { name: /Gateway/ }).click();
  const endpointField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("API URL"),
  });
  const endpointInput = endpointField.getByLabel("API URL");
  await endpointInput.fill(credentialUrl);

  await expect(endpointInput).toHaveValue(credentialUrl);
  await expect(endpointInput).not.toHaveAttribute(
    "title",
    /credential-bearing-url-sentinel/,
  );
  await expect(
    endpointField.getByRole("button", { name: "Reset current" }),
  ).toHaveAttribute("title", "Reset API URL to the loaded value.");
  await expect(
    endpointField.getByRole("button", { name: "Use default" }),
  ).toHaveAttribute(
    "title",
    "Use the inherited default for API URL; removes the explicit value.",
  );

  const endpointTitles = await endpointField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(endpointTitles).not.toContain("credential-bearing-url-sentinel");
  expect(endpointTitles).toContain("http://api:8080");

  const ordinaryField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Gateway ID"),
  });
  const ordinaryInput = ordinaryField.getByLabel("Gateway ID");
  await expect(ordinaryInput).not.toHaveAttribute("title", /compose-gateway/);

  const websocketCredentialUrl =
    "wss://tooltip-user:websocket-url-sentinel@example.invalid/events";
  await ordinaryInput.fill(websocketCredentialUrl);
  await expect(ordinaryInput).toHaveValue(websocketCredentialUrl);
  await expect(ordinaryInput).not.toHaveAttribute(
    "title",
    /websocket-url-sentinel/,
  );
  const ordinaryTitles = await ordinaryField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(ordinaryTitles).not.toContain("websocket-url-sentinel");
  expect(ordinaryTitles).toContain("compose-gateway");

  await sections.getByRole("button", { name: /API/ }).click();
  const gatewayControlField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Gateway control URL"),
  });
  const gatewayControlInput = gatewayControlField.getByLabel(
    "Gateway control URL",
  );
  await expect(gatewayControlInput).toHaveValue(
    "unix:/var/lib/vpsman/gateway-control.sock",
  );
  await expect(gatewayControlInput).not.toHaveAttribute(
    "title",
    /unix:\/var\/lib\/vpsman\/gateway-control\.sock/,
  );
  const gatewayControlTitles = await gatewayControlField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(gatewayControlTitles).toContain(
    "unix:/var/lib/vpsman/gateway-control.sock",
  );
});

test("reveals a retained external error only when its visible value is shortened", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the retained-error tooltip boundary is covered in the desktop data grid",
  );
  const externalErrorSentinel =
    "external-backup-error-sentinel-8dd9: upstream returned opaque detail";
  const policy: BackupPolicyRecord = {
    cadence_error: null,
    catch_up_limit: 1,
    catch_up_policy: "skip_missed",
    created_at: "2026-08-08T08:00:00Z",
    cron_expr: "0 2 * * *",
    enabled: true,
    failure_count: 1,
    follow_symlinks: false,
    include_config: true,
    keep_last: 7,
    last_error: externalErrorSentinel,
    last_run_at: "2026-08-08T08:15:00Z",
    max_failures: 3,
    missing_path_policy: "skip",
    name: "retained failure evidence",
    next_run_at: "2026-08-09T02:00:00Z",
    next_runs: ["2026-08-09T02:00:00Z"],
    paths: ["/srv/backup"],
    retention_days: 30,
    retry_delay_secs: 300,
    rotation_generation: null,
    schedule_id: "tooltip-privacy-backup-policy",
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    timezone: "UTC",
    updated_at: "2026-08-08T08:15:00Z",
  };

  await installConsoleApiMock(page, { backupPoliciesOverride: [policy] });
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Policies");

  const grid = page.getByLabel("Backup policy records data grid");
  const visibleError = grid.getByText(externalErrorSentinel, { exact: true });
  await expect(visibleError).toBeVisible();
  const result = visibleError.locator("..");
  await expect(result).not.toHaveAttribute("data-tooltip-sensitive", "true");
  await expect(result).not.toHaveAttribute("data-value-tooltip-skip", "true");
  await expect
    .poll(() =>
      visibleError.evaluate(
        (element) => element.scrollWidth > element.clientWidth + 1,
      ),
    )
    .toBe(true);
  await expect(visibleError).toHaveAttribute("title", externalErrorSentinel);
  await expect
    .poll(async () =>
      page
        .locator("[title]")
        .evaluateAll(
          (elements, sentinel) =>
            elements.some((element) =>
              (element.getAttribute("title") ?? "").includes(sentinel),
            ),
          externalErrorSentinel,
        ),
    )
    .toBe(true);
});

test("exposes API-authorized operational diagnostics in semantic titles", () => {
  const actionFeedback = source("components/ActionFeedback.tsx");
  expect(actionFeedback).not.toContain("title=");
  expect(actionFeedback).not.toContain("data-tooltip-sensitive");
  expect(actionFeedback).not.toContain("data-value-tooltip-skip");

  const supervisor = source("panels/jobs/ProcessSupervisorInventoryPanel.tsx");
  expect(supervisor).not.toContain("Retained stdout log content");
  expect(supervisor).not.toContain("Retained stderr log content");

  const hostServices = source("panels/jobs/HostServicesPanel.tsx");
  expect(hostServices).toContain(
    "Provider diagnostic: ${service.state_reason}",
  );
  expect(hostServices).toMatch(
    /title=\{[\s\S]{0,120}service\.state_reason[\s\S]{0,120}Provider diagnostic/,
  );

  const execution = source("components/ExecutionResultPanel.tsx");
  expect(execution).toContain("<span>{group.reason}</span>");

  const portForwarding = source("panels/topology/PortForwardingPanel.tsx");
  expect(portForwarding).toContain("{rule.configuration_error}");
  expect(portForwarding).toContain("title={mappingPreview.error}");
  expect(portForwarding).toMatch(
    /label: "Configuration error",\s*value: corruptDelete\.configuration_error/,
  );

  const topology = source("panels/TopologyPanel.tsx");
  expect(topology).toContain("{plan.configuration_error}");
  expect(topology).toContain(
    "Runtime configuration error: ${runtimeConfig.error}",
  );

  const pingTargets = source("panels/observability/PingTargetsPanel.tsx");
  expect(pingTargets).toContain("<span title={evidence.title}>");
  expect(pingTargets).not.toContain("tooltipSensitive");

  const configurationSources = source("panels/ConfigurationSourcesPanel.tsx");
  expect(configurationSources).toContain("${source.runtime_sync.reason}");
  expect(configurationSources).not.toContain('data-tooltip-sensitive="true"');

  const multiFile = source("panels/jobs/MultiFileActionsPanel.tsx");
  expect(multiFile).toContain("{group.reason || group.label}");
  expect(multiFile).toContain("{group.preview}");

  const monitoringDetail = source("panels/VpsMonitoringDetailPanel.tsx");
  expect(monitoringDetail).toContain("diagnostic: ${target.reason}");

  const topologyPanel = source("panels/TopologyPanel.tsx");
  expect(topologyPanel).toContain(
    "Left probe diagnostic: ${edge.left_reachability_reason}",
  );
  expect(topologyPanel).toContain(
    "Right probe diagnostic: ${edge.right_reachability_reason}",
  );

  const topologyGraph = source("panels/topology/TopologyGraphPanel.tsx");
  expect(topologyGraph).toContain("edge.left_reachability_reason");
  expect(topologyGraph).toContain("diagnostic: ${reason}");

  for (const diagnosticSurface of [
    hostServices,
    topology,
    pingTargets,
    configurationSources,
    monitoringDetail,
    topologyGraph,
  ]) {
    expect(diagnosticSurface).not.toMatch(
      /(?:excluded|omitted) from (?:the )?tooltips?/i,
    );
  }

  const backupHistory = source("panels/backups/BackupHistoryTables.tsx");
  expect(backupHistory).toContain("detail: policy.last_error");
  expect(backupHistory).not.toContain("title: policy.last_error");

  const jobs = source("panels/JobsPanel.tsx");
  expect(jobs).toContain("? schedule.cadence_error");
  expect(jobs).toContain('{group.preview || "No preview"}');

  const schedules = source("panels/SchedulesPanel.tsx");
  expect(schedules).toContain(
    "Operation evidence:\\n${JSON.stringify(operation, null, 2)}",
  );
  expect(schedules).toContain(
    '"Command and arguments submitted by each scheduled run."',
  );
  expect(schedules).not.toContain("title={commandText || undefined}");

  const runbooks = source("panels/automation/RunbooksPanel.tsx");
  expect(runbooks).toContain(
    "Operation evidence:\\n${JSON.stringify(runbook.template.operation, null, 2)}",
  );

  const rollouts = source("panels/automation/RolloutsPanel.tsx");
  expect(rollouts).toContain('className="truncateValue"');
  expect(rollouts).toContain('{target.message ?? "-"}');

  const operationControls = source("panels/jobs/JobOperationControls.tsx");
  expect(operationControls).toContain(
    'title="Command and arguments used to start the managed process."',
  );
  expect(operationControls).toContain(
    'title="HTTPS URL of version.json used to select the architecture-specific agent artifact."',
  );
  expect(operationControls).not.toContain(
    "title={supervisorArgv || undefined}",
  );
  expect(operationControls).not.toContain(
    "title={updateCheckVersionUrl || undefined}",
  );

  for (const evidenceSurface of [
    supervisor,
    hostServices,
    execution,
    multiFile,
    backupHistory,
    jobs,
    schedules,
    runbooks,
    rollouts,
    operationControls,
  ]) {
    expect(evidenceSurface).not.toMatch(
      /(?:excluded|omitted) from (?:the )?tooltips?/i,
    );
  }
});
