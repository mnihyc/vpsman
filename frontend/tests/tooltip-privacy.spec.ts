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

test("keeps command-bearing tooltip sources on static exclusion contracts", () => {
  const grid = source("components/ConsoleDataGrid.tsx");
  const gridTooltipHelper = grid.slice(
    grid.indexOf("function columnTooltip"),
    grid.indexOf("function SortableHeaderCell"),
  );
  expect(gridTooltipHelper).not.toContain("searchValue");
  expect(gridTooltipHelper).not.toContain("sortValue");

  const confirmation = source("components/ConfirmationPrompt.tsx");
  const confirmationTooltipHelper = confirmation.slice(
    confirmation.indexOf("function confirmationItemTitle"),
  );
  const sensitiveGuard = confirmationTooltipHelper.indexOf("if (sensitive)");
  const scalarFormatting = confirmationTooltipHelper.indexOf("String(value)");
  expect(sensitiveGuard).toBeGreaterThanOrEqual(0);
  expect(scalarFormatting).toBeGreaterThan(sensitiveGuard);
  expect(confirmationTooltipHelper).toContain(
    "its exact value is excluded from tooltips",
  );

  const access = source("panels/AccessPanel.tsx");
  expect(access).not.toContain("title={foregroundStartCommand}");
  expect(access).toContain(
    'title="Foreground agent start command. Exact command content is excluded from tooltips."',
  );

  const dispatch = source("panels/JobDispatchPanel.tsx");
  expect(dispatch).not.toMatch(/label:\s*"Command argv",\s*title:\s*command/);
  expect(dispatch).toMatch(
    /label:\s*"Command argv",\s*sensitive:\s*true,\s*value:\s*command/,
  );
});

test("keeps Suite Config endpoint values visible but out of tooltips", async ({
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
  await expect(endpointInput).toHaveAttribute("data-tooltip-sensitive", "true");
  await expect(endpointInput).toHaveAttribute("data-value-tooltip-skip", "true");
  await expect(endpointInput).toHaveAttribute(
    "title",
    "API URL endpoint field. Exact URL content is excluded from tooltips.",
  );
  await expect(
    endpointField.getByRole("button", { name: "Reset current" }),
  ).toHaveAttribute("title", /exact URL content is excluded from tooltips/i);
  await expect(
    endpointField.getByRole("button", { name: "Use default" }),
  ).toHaveAttribute("title", /exact URL content is excluded from tooltips/i);

  await expect
    .poll(async () =>
      endpointField.locator("[title]").evaluateAll((elements) =>
        elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
      ),
    )
    .not.toContain(credentialUrl);
  const endpointTitles = await endpointField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(endpointTitles).not.toContain("credential-bearing-url-sentinel");
  expect(endpointTitles).not.toContain("http://api:8080");

  const ordinaryField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Gateway ID"),
  });
  const ordinaryInput = ordinaryField.getByLabel("Gateway ID");
  await expect(ordinaryInput).toHaveAttribute(
    "title",
    "Gateway ID: compose-gateway.",
  );

  const websocketCredentialUrl =
    "wss://tooltip-user:websocket-url-sentinel@example.invalid/events";
  await ordinaryInput.fill(websocketCredentialUrl);
  await expect(ordinaryInput).toHaveValue(websocketCredentialUrl);
  await expect(ordinaryInput).toHaveAttribute(
    "data-tooltip-sensitive",
    "true",
  );
  await expect(ordinaryInput).toHaveAttribute("data-value-tooltip-skip", "true");
  const ordinaryTitles = await ordinaryField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(ordinaryTitles).not.toContain("websocket-url-sentinel");
  expect(ordinaryTitles).not.toContain("compose-gateway");

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
  await expect(gatewayControlInput).toHaveAttribute(
    "data-tooltip-sensitive",
    "true",
  );
  const gatewayControlTitles = await gatewayControlField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(gatewayControlTitles).not.toContain(
    "unix:/var/lib/vpsman/gateway-control.sock",
  );
});

test("keeps retained external errors visible without promoting them into titles", async ({
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
  const protectedResult = visibleError.locator("..");
  await expect(protectedResult).toHaveAttribute("data-tooltip-sensitive", "true");
  await expect(protectedResult).toHaveAttribute(
    "data-value-tooltip-skip",
    "true",
  );
  await expect
    .poll(async () =>
      page.locator("[title]").evaluateAll(
        (elements, sentinel) =>
          elements.some((element) =>
            (element.getAttribute("title") ?? "").includes(sentinel),
          ),
        externalErrorSentinel,
      ),
    )
    .toBe(false);
});

test("keeps raw error and log tooltip sites on static exclusion boundaries", () => {
  const actionFeedback = source("components/ActionFeedback.tsx");
  expect(actionFeedback).not.toContain("title={`${tone} feedback: ${message}`}");
  expect(actionFeedback).toContain('data-tooltip-sensitive="true"');
  expect(actionFeedback).toContain('title={toneTitle[tone]}');

  const supervisor = source(
    "panels/jobs/ProcessSupervisorInventoryPanel.tsx",
  );
  expect(supervisor).not.toMatch(/title=\{row\.(?:stdout|stderr)_log/);
  expect(supervisor).toContain(
    'data-tooltip-sensitive={row.stdout_log ? "true" : undefined}',
  );
  expect(supervisor).toContain(
    'data-value-tooltip-skip={row.stderr_log ? "true" : undefined}',
  );

  const hostServices = source("panels/jobs/HostServicesPanel.tsx");
  expect(hostServices).not.toContain("title={service.state_reason");
  expect(hostServices).toContain(
    "Exact provider diagnostic content is excluded from tooltips.",
  );

  const execution = source("components/ExecutionResultPanel.tsx");
  expect(execution).not.toContain("<span title={group.reason}>");
  expect(execution).toMatch(
    /data-tooltip-sensitive="true"[\s\S]{0,100}data-value-tooltip-skip="true"[\s\S]{0,180}\{group\.reason\}/,
  );

  const portForwarding = source("panels/topology/PortForwardingPanel.tsx");
  expect(portForwarding).not.toContain("title={rule.configuration_error}");
  expect(portForwarding).not.toContain(
    "title: corruptDelete.configuration_error",
  );
  expect(portForwarding).not.toContain("title={mappingPreview.error}");
  expect(portForwarding).toMatch(
    /label: "Configuration error",\s*sensitive: true,\s*value: corruptDelete\.configuration_error/,
  );

  const topology = source("panels/TopologyPanel.tsx");
  expect(topology).not.toContain("title={plan.configuration_error}");
  expect(topology).toMatch(
    /data-tooltip-sensitive="true"[\s\S]{0,100}data-value-tooltip-skip="true"[\s\S]{0,600}\{plan\.configuration_error\}/,
  );

  const pingTargets = source("panels/observability/PingTargetsPanel.tsx");
  expect(pingTargets).not.toContain("<span title={evidence.title}>");
  expect(pingTargets).toContain("tooltipSensitive: true");
  expect(pingTargets).toContain('tooltipSensitive: state === "failed"');

  const configurationSources = source(
    "panels/ConfigurationSourcesPanel.tsx",
  );
  expect(configurationSources).not.toContain(
    "title={source.runtime_sync.reason}",
  );

  const multiFile = source("panels/jobs/MultiFileActionsPanel.tsx");
  expect(multiFile).not.toContain(
    "title={group.detail || group.reason || group.label}",
  );
  expect(multiFile).toContain(
    "Exact per-target detail is excluded from tooltips.",
  );

  const monitoringDetail = source("panels/VpsMonitoringDetailPanel.tsx");
  expect(monitoringDetail).not.toContain("title={target.reason || undefined}");
  expect(monitoringDetail).toContain(
    "Exact probe diagnostic content is excluded from tooltips.",
  );

  const topologyPanel = source("panels/TopologyPanel.tsx");
  expect(topologyPanel).not.toMatch(/title:[^\n]*reachability_reason/);
  expect(topologyPanel).not.toContain("readableTelemetryToken(reason)");

  const topologyGraph = source("panels/topology/TopologyGraphPanel.tsx");
  expect(topologyGraph).not.toMatch(
    /endpointReachabilityDetail\([\s\S]{0,250}reachability_reason/,
  );

  const backupHistory = source("panels/backups/BackupHistoryTables.tsx");
  expect(backupHistory).not.toContain("title: policy.last_error");
  expect(backupHistory).toContain(
    'data-tooltip-sensitive={policy.last_error ? "true" : undefined}',
  );

  const jobs = source("panels/JobsPanel.tsx");
  expect(jobs).not.toContain(
    "<small title={schedule?.cadence_error ?? undefined}>",
  );
  expect(jobs).toMatch(
    /data-value-tooltip-skip=\{\s*schedule\?\.cadence_error \? "true" : undefined\s*\}/,
  );
});
