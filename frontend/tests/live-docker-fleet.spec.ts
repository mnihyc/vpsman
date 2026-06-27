import path from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.skip(
  !process.env.VPSMAN_DOCKER_FLEET_UI_SMOKE,
  "enabled by scripts/smoke-docker-24-agent-fleet.sh",
);

const expectedTotal = Number(
  process.env.VPSMAN_DOCKER_FLEET_EXPECTED_TOTAL ?? "24",
);
const providerAlphaCount = Number(
  process.env.VPSMAN_DOCKER_FLEET_PROVIDER_ALPHA_COUNT ??
    String(Math.ceil(expectedTotal / 3)),
);
const countryUsCount = Number(
  process.env.VPSMAN_DOCKER_FLEET_COUNTRY_US_COUNT ??
    String(Math.ceil(expectedTotal / 4)),
);
const providerAlphaCountryUsCount = Number(
  process.env.VPSMAN_DOCKER_FLEET_PROVIDER_ALPHA_COUNTRY_US_COUNT ??
    String(Math.ceil(expectedTotal / 12)),
);
const roleEdgeCount = Number(
  process.env.VPSMAN_DOCKER_FLEET_ROLE_EDGE_COUNT ?? String(countryUsCount),
);
const username =
  process.env.VPSMAN_DOCKER_FLEET_USERNAME ?? "docker-fleet-admin";
const password =
  process.env.VPSMAN_DOCKER_FLEET_PASSWORD ?? "docker-fleet-password";
const screenshotDir = process.env.VPSMAN_DOCKER_FLEET_SCREENSHOT_DIR;
const extendedReview = process.env.VPSMAN_DOCKER_FLEET_EXTENDED_REVIEW === "1";
const cleanupExpression =
  process.env.VPSMAN_DOCKER_FLEET_CLEANUP_EXPRESSION ??
  'artifact.domain = "file_transfer_source"';

type ScreenshotManifestEntry = {
  description: string | null;
  name: string;
  project: string;
  screenshot: string;
};

const screenshotManifest: ScreenshotManifestEntry[] = [];

test.setTimeout(extendedReview ? 600_000 : 300_000);

test("validates the live Docker fleet console with 20+ VPS agents", async ({
  page,
}, testInfo) => {
  const isMobile = testInfo.project.name.includes("mobile");
  const consoleErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors.push(message.text());
    }
  });

  await login(page);
  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fleet command home" }),
  ).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByLabel("Home quick actions")).toBeVisible();
  await expect(page.getByLabel("Home posture strip")).toContainText(
    `${expectedTotal}/${expectedTotal}`,
  );
  await expect(
    page.getByRole("heading", { name: "Running work" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Recent failures" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Needs attention" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home telemetry widgets")).toHaveCount(0);
  await expectCleanLayout(page);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "page-home-overview",
    "Home overview before operator action, focused on quick actions, fleet availability, running work, and failures.",
  );

  await maybeScreenshot(page, testInfo.project.name, "home");
  await expectLiveSystemDashboardTelemetry(page, testInfo.project.name);
  if (isMobile) {
    writeScreenshotManifest(testInfo.project.name);
    await expectCleanLayout(page);
    expect(actionableConsoleErrors(consoleErrors)).toEqual([]);
    return;
  }
  const sidebarBox = await page.locator(".sidebar").boundingBox();
  expect(sidebarBox?.x).toBe(0);
  expect(sidebarBox?.y).toBe(0);

  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { name: "Fleet instances" }),
  ).toBeVisible();
  const grid = page.getByLabel("VPS instance records data grid");
  await expect(
    grid.getByText(`${expectedTotal} of ${expectedTotal} instances`),
  ).toBeVisible({ timeout: 20_000 });
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "page-fleet-instances",
    "Fleet / Instances page with the live inventory table before filtering.",
  );
  await grid.getByLabel("VPS instance records search").fill("provider:alpha");
  await expect(
    grid.getByText(`${providerAlphaCount} of ${expectedTotal} instances`),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-search-provider-alpha",
    "Fleet table after operator filters the live fleet to provider alpha.",
  );
  await grid.getByLabel("VPS instance records search").fill("");

  const firstRow = grid
    .locator(".gridBody [role=row]", { hasText: "df-alpha-US-01" })
    .first();
  const secondRow = grid
    .locator(".gridBody [role=row]", { hasText: "df-alpha-DE-10" })
    .first();
  await selectGridRow(firstRow);
  await selectGridRow(secondRow);
  await expect(grid.getByText("2 selected", { exact: true })).toBeVisible();
  await clickGridRowControl(firstRow, "Expand VPS instance records row");
  const firstDetail = grid
    .locator(".gridExpandedRow", { hasText: "df-alpha-US-01" })
    .first();
  await expect(
    firstDetail.getByRole("heading", { name: /df-alpha-US-01/ }),
  ).toBeVisible();
  await expect(firstDetail).toContainText("Root uid 0");
  await expectLiveFleetTelemetry(firstDetail);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-expanded-telemetry-detail",
    "Expanded VPS row showing live telemetry detail for a real agent.",
  );
  await grid.getByRole("button", { name: "Action" }).click();
  await expect(
    page.getByRole("menuitem", { name: "Copy client IDs" }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-action-menu-open",
    "Bulk action menu opened after selecting two VPS rows.",
  );
  await page.keyboard.press("Escape");
  await firstRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Inspect selected" }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-row-context-menu-open",
    "Right-click context menu opened on a fleet row with selected-row actions preserved.",
  );
  await page.keyboard.press("Escape");
  await exerciseColumnControls(page, grid);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-column-controls-result",
    "Fleet table after operator resizes/reorders columns, hides Provider, and expands page size.",
  );
  await maybeScreenshot(page, testInfo.project.name, "fleet");
  await expectCleanLayout(page);

  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await expect(page.getByRole("heading", { name: "Bulk tags" })).toBeVisible();
  await page
    .getByLabel("Bulk tag", { exact: true })
    .fill("maintenance:2026-q2-patch");
  await page
    .getByRole("searchbox", { name: "Bulk tag selector expression" })
    .fill("provider:alpha && country:US");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Preview targets" }).click();
  await expect(
    page.getByText(`${providerAlphaCountryUsCount}/${expectedTotal}`),
  ).toBeVisible();
  await expect(page.locator(".bulkTagPreview")).toContainText("df-alpha-US-01");
  await expect(page.locator(".bulkTagPreview")).toContainText("df-alpha-US-13");
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "bulk-tag-preview-result",
    "Bulk tag workflow after previewing provider alpha US targets before mutation.",
  );
  await expectCleanLayout(page);

  await exerciseAlertPolicyReview(page, testInfo.project.name);
  await exerciseAlertNotificationChannels(page, testInfo.project.name);
  await exerciseExpressionWebhooks(page, testInfo.project.name);
  await exerciseServerJobsCleanup(page, testInfo.project.name);

  if (extendedReview) {
    await verifyDesktopSubpages(page, testInfo.project.name);
    expectExtendedScreenshotNames(testInfo.project.name, [
      "extended-page-system-dashboard",
      "extended-page-system-config",
      "extended-page-system-preferences",
    ]);
  }
  await openConsoleSubpage(page, "System", "Preferences");
  const preferencesPanel = page.locator(".preferencesPanel");
  await expect(
    preferencesPanel.locator(".consoleStatusBadge", { hasText: /^Saved$/ }),
  ).toBeVisible();
  const nameDisplay = page.getByLabel("Name display");
  await expect(nameDisplay).toBeVisible();
  const bulkCompare = page.getByLabel("Bulk output comparison default");
  await expect(bulkCompare).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "page-preferences-operator",
    "System / Preferences page with saved display and workflow defaults.",
  );
  await maybeScreenshot(page, testInfo.project.name, "preferences");
  writeScreenshotManifest(testInfo.project.name);

  expect(actionableConsoleErrors(consoleErrors)).toEqual([]);
});

async function login(page: Page) {
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Operator access" }),
  ).toBeVisible({ timeout: 20_000 });
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Submit login" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible({ timeout: 30_000 });
}

async function expectCleanLayout(page: Page) {
  await expect(page.getByText(/HTTP 404|Http 404|404 fixture/i)).toHaveCount(0);
  const layout = await page.evaluate(() => {
    const root = document.documentElement;
    const main = document.querySelector("main");
    const mainRect = main?.getBoundingClientRect();
    const visibleText = main?.textContent?.replace(/\s+/g, " ").trim() ?? "";
    return {
      overflow: root.scrollWidth - root.clientWidth,
      mainHeight: mainRect?.height ?? 0,
      visibleTextLength: visibleText.length,
    };
  });
  expect(layout.overflow).toBeLessThanOrEqual(1);
  expect(layout.mainHeight).toBeGreaterThan(300);
  expect(layout.visibleTextLength).toBeGreaterThan(200);
}

async function expectLiveDashboardTelemetry(page: Page) {
  const operationalHealth = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Operational Health" }),
  });
  await expect(operationalHealth).toContainText(
    `${expectedTotal}/${expectedTotal} online`,
  );
  await expect(operationalHealth).not.toContainText("DB pool");
  await expect(operationalHealth).not.toContainText("Dispatch queue");
  await expect(operationalHealth).not.toContainText("Gateway events");
  await expect(operationalHealth).not.toContainText(
    /No data|Gateway metrics unavailable/i,
  );

  const resourceUsage = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Resource Usage" }),
  });
  await expect(resourceUsage).toContainText(`${expectedTotal} VPS plotted`);
  await expect(resourceUsage.getByLabel("Resource usage curve")).toBeVisible();
  await expect(resourceUsage).not.toContainText(
    /No resource telemetry|No data|No rollup|unavailable/i,
  );
  await resourceUsage
    .getByRole("button", { name: "Memory", exact: true })
    .click();
  await expect(resourceUsage).toContainText("Memory used");
  await resourceUsage
    .getByRole("button", { name: "Disk", exact: true })
    .click();
  await expect(resourceUsage).toContainText("Disk free");

  const networkSection = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Network", exact: true }),
  });
  await networkSection
    .getByRole("button", { name: "Speed", exact: true })
    .click();
  await expect(networkSection.getByLabel("Network speed curve")).toBeVisible();
  await expect(networkSection).not.toContainText(
    /No network speed samples|unavailable/i,
  );
  expect(
    await networkSection.locator(".dashboardClientRow").count(),
  ).toBeGreaterThan(0);
  await networkSection
    .getByRole("button", { name: "Traffic", exact: true })
    .click();
  await expect(
    networkSection.getByLabel("Network traffic curve"),
  ).toBeVisible();
  await expect(networkSection).not.toContainText(
    /No network traffic samples|unavailable/i,
  );
  expect(
    await networkSection.locator(".dashboardClientRow").count(),
  ).toBeGreaterThan(0);
}

async function expectLiveSystemDashboardTelemetry(
  page: Page,
  projectName: string,
) {
  await openConsoleSubpage(page, "System", "Overview");
  await expect(
    page.getByRole("heading", { name: "System overview", exact: true }),
  ).toBeVisible();

  const systemWorkspace = page.locator(".systemWorkspace");
  await expect(
    systemWorkspace.getByRole("heading", { name: "Capacity", exact: true }),
  ).toBeVisible();
  const capacity = systemWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Capacity", exact: true }),
  });
  await expect(capacity).toContainText("API DB pool");
  await expect(capacity).toContainText("Worker DB pool");
  await expect(capacity).toContainText("Dispatcher in-flight");

  const lifecycle = systemWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Dispatch Lifecycle" }),
  });
  await expect(lifecycle).toContainText("Dispatch queue");
  await expect(lifecycle).toContainText("Active targets");

  const deadlines = systemWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Deadlines" }),
  });
  await expect(deadlines).toContainText("Deadline timeouts");
  await expect(deadlines).toContainText("Control timed out");

  const cancellations = systemWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Cancellations" }),
  });
  await expect(cancellations).toContainText("Cancel acks");
  await expect(cancellations).toContainText("Awaiting ack");

  const gatewayEvents = systemWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Gateway Events" }),
  });
  await expect(gatewayEvents).toContainText("Event retries");
  await expect(gatewayEvents).not.toContainText(/unavailable/i);
  await expectCleanLayout(page);
  await maybeExtendedScreenshot(
    page,
    projectName,
    "page-system-dashboard",
    "System / Overview page with live control-plane capacity, dispatch, deadline, cancellation, and gateway event metrics.",
  );
}

async function expectLiveFleetTelemetry(detail: Locator) {
  await expect(
    detail.locator(".metric", { hasText: "Traffic" }),
  ).not.toContainText(
    /No rate samples|No counters|No rollup|No data|unavailable/i,
  );
  await expect(
    detail.locator(".metric", { hasText: "Samples" }),
  ).not.toContainText(/No rollup|No data|unavailable/i);
  await detail.getByRole("tab", { name: "Telemetry" }).click();
  await expect(
    detail.getByRole("tabpanel").getByText("CPU load"),
  ).toBeVisible();
  await expect(detail).not.toContainText(
    /No rollup|No rate samples|No counters|No data|unavailable/i,
  );
}

async function exerciseColumnControls(page: Page, grid: Locator) {
  const nameHeader = grid
    .locator(".gridHeaderCell", { hasText: "Name" })
    .first();
  const providerHeader = grid
    .locator(".gridHeaderCell", { hasText: "Provider" })
    .first();
  const tagsHeader = grid
    .locator(".gridHeaderCell", { hasText: "Tags" })
    .first();
  const resizeHandle = tagsHeader.locator(".gridResizeHandle");
  await expect(resizeHandle).toBeVisible();
  const box = await resizeHandle.boundingBox();
  expect(box).not.toBeNull();
  if (box) {
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 38, box.y + box.height / 2, { steps: 5 });
    await page.mouse.up();
  }

  await expect(providerHeader.locator(".gridDragHandle")).toBeVisible();
  await nameHeader.locator(".gridDragHandle").focus();
  await page.keyboard.press("Space");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Space");
  await grid.getByLabel("VPS instance records columns").click();
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await grid.getByLabel("VPS instance records page size").selectOption("25");
  await expect(
    grid.getByText(`1 / ${Math.ceil(expectedTotal / 25)}`),
  ).toBeVisible();
}

async function selectGridRow(row: Locator) {
  const checkbox = await retryGridRowControl(
    row,
    "Select VPS instance records row",
    (control) => control.check({ timeout: 10_000 }),
  );
  await expect(checkbox).toBeChecked({ timeout: 5000 });
}

async function clickGridRowControl(row: Locator, label: string) {
  await retryGridRowControl(row, label, (control) =>
    control.click({ timeout: 10_000 }),
  );
}

async function retryGridRowControl<T>(
  row: Locator,
  label: string,
  action: (control: Locator) => Promise<T>,
): Promise<Locator> {
  let lastError: unknown;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    const control = row.getByLabel(label).first();
    try {
      await action(control);
      return control;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw lastError;
}

async function exerciseExpressionWebhooks(page: Page, projectName: string) {
  await openConsoleSubpage(page, "Fleet", "Notifications");
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toBeVisible();
  const notifications = page.locator(".consoleCrudPanel").filter({
    has: page.getByRole("tablist", { name: "Notification registries" }),
  });
  await notifications.getByRole("tab", { name: "Webhooks" }).click();
  await expect(
    notifications.getByText("Webhook rules", { exact: true }).first(),
  ).toBeVisible();

  await notifications.getByRole("button", { name: "Create rule" }).click();
  const detail = notifications.locator(".consoleDetailPanel").filter({
    hasText: /Create webhook rule|Edit webhook rule/,
  });
  await expect(detail).toBeVisible();
  await detail.getByLabel("Webhook rule name").fill("docker-fleet-q2-capacity");
  await detail
    .getByLabel("Webhook target")
    .fill("https://hooks.example/vpsman/docker-fleet");
  await detail.getByLabel("Webhook cooldown seconds").fill("60");
  await fillSearchExpression(
    detail.getByLabel("Webhook expression"),
    'interval.30sec && vps.tag = "role:edge"',
  );
  await fillWebhookTemplate(
    detail,
    "{rule.name} {event.kind} count={matched_vps.length} [for v in matched_vps]{v.display_name} [endfor]",
  );
  await detail.getByLabel("Webhook event kind").fill("interval.30sec");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rule-form-filled",
    "Webhook rule editor filled with production-style target, expression, template, and event kind.",
  );
  await detail.getByRole("button", { name: "Create rule" }).click();
  await expect(
    notifications.locator(".fleetPolicyStatus", {
      hasText: "saved docker-fleet-q2-capacity",
    }),
  ).toBeVisible();
  await expect(notifications).toContainText("docker-fleet-q2-capacity");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rule-saved",
    "Webhook rule creation result showing saved status and the new rule in context.",
  );

  await detail.getByRole("button", { name: "Review rule" }).click();
  await expect(
    notifications.locator(".deliveryPreviewSection", {
      hasText: "Webhook delivery preview",
    }),
  ).toBeVisible();
  await expect(notifications).toContainText(
    `${roleEdgeCount} VPSs matched webhook dry run`,
  );
  await expect(notifications).toContainText(
    `docker-fleet-q2-capacity interval.30sec count=${roleEdgeCount}`,
  );
  await expect(notifications).toContainText("df-alpha-US-01");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rule-preview-result",
    "Webhook dry-run result showing matched live VPSs and rendered payload preview.",
  );

  await notifications.getByRole("tab", { name: "Webhooks" }).click();
  await notifications.getByRole("button", { name: "Review matches" }).click();
  await expect(
    notifications.locator(".deliveryPreviewSection", {
      hasText: "Webhook delivery preview",
    }),
  ).toBeVisible();
  await expect(
    notifications.locator(".consoleDataGrid", {
      hasText: "docker-fleet-q2-capacity",
    }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-match-rules-result",
    "Webhook queue operation after matching saved rules against the preview event.",
  );

  await notifications.getByRole("tab", { name: "Maintenance" }).click();
  await notifications.getByLabel("Webhook rotation days").fill("7");
  await notifications
    .getByLabel("Webhook rotation status")
    .selectOption("delivered");
  await notifications.getByRole("button", { name: "Review rotation" }).click();
  await expect(
    notifications.locator(".fleetPolicyStatus", {
      hasText: "0 matched / 0 deleted",
    }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rotation-preview-result",
    "Webhook retention maintenance after previewing rotation without deleting records.",
  );
  await expectCleanLayout(page);
}

async function exerciseAlertPolicyReview(page: Page, projectName: string) {
  await openConsoleSubpage(page, "Fleet", "Alert policies");
  await expect(
    page.getByRole("heading", { name: "Alert policies" }),
  ).toBeVisible();
  const grid = page.getByLabel("Alert policy rules data grid");
  const row = grid
    .locator(".gridBody [role=row]", { hasText: "docker-edge-resource-alerts" })
    .first();
  await expect(row).toBeVisible();
  await row.getByLabel("Expand Alert policy rules row").click();
  await expect(
    grid.locator(".gridExpandedRow", {
      hasText: "docker-edge-resource-alerts",
    }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "alert-policy-inline-detail",
    "Alert policy row opened with inline chevron detail on the seeded live policy.",
  );

  await row.getByLabel("Select Alert policy rules row").check();
  await grid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Details" }).click();
  await expect(page.getByText("Alert policy details")).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "alert-policy-below-table-detail",
    "Alert policy detail action opened the same policy details below the table.",
  );

  await page.getByRole("button", { name: "Edit policy" }).click();
  await expect(page.getByText("Edit alert policy")).toBeVisible();
  await page.getByLabel("Disk warning ratio").fill("0.22");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "alert-policy-edit-matrix",
    "Alert policy edit panel with compact Memory/Disk/CPU threshold matrix after operator changes disk warning.",
  );
  await page.getByLabel("Close detail panel").click();
}

async function exerciseAlertNotificationChannels(
  page: Page,
  projectName: string,
) {
  await openConsoleSubpage(page, "Fleet", "Notifications");
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toBeVisible();
  const notifications = page.locator(".consoleCrudPanel").filter({
    has: page.getByRole("tablist", { name: "Notification registries" }),
  });
  const grid = notifications.getByLabel(
    "Alert notification channels data grid",
  );
  const row = grid
    .locator(".gridBody [role=row]", {
      hasText: "docker-resource-audit",
    })
    .first();
  await expect(row).toBeVisible();
  await row.getByLabel("Expand Alert notification channels row").click();
  await expect(
    grid.locator(".gridExpandedRow", {
      hasText: "docker-resource-audit",
    }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "notification-channel-inline-detail",
    "Notification channel row opened with inline chevron detail for the seeded resource audit channel.",
  );

  await row.getByLabel("Select Alert notification channels row").check();
  await grid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Details" }).click();
  await expect(page.getByText("Notification channel details")).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "notification-channel-below-table-detail",
    "Notification channel detail action opened routing filters and delivery target below the table.",
  );

  await page.getByRole("button", { name: "Edit channel" }).click();
  await expect(page.getByText("Edit notification channel")).toBeVisible();
  await page.getByLabel("Alert categories").fill("resource, agent_status");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "notification-channel-edit-token-preview",
    "Notification channel editor showing category token preview after operator edits category filters.",
  );
  await page.getByLabel("Close detail panel").click();

  await notifications
    .getByRole("button", { name: "Review queued deliveries" })
    .click();
  await expect(
    notifications.locator(".deliveryPreviewSection", {
      hasText: "Notification delivery preview",
    }),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "notification-delivery-preview-result",
    "Notification queue preview result after operator previews queued custom pager deliveries.",
  );
}

async function exerciseServerJobsCleanup(page: Page, projectName: string) {
  await openConsoleSubpage(page, "System", "Maintenance");
  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await expect(cleanupPanel).toBeVisible();
  await cleanupPanel.getByLabel("Older than days").fill("");
  await cleanupPanel.getByText("Advanced expression").click();
  await cleanupPanel.getByLabel("Expression").fill(cleanupExpression);
  await cleanupPanel.getByRole("button", { name: "Preview" }).click();
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    /^[\s\S]*[1-9][0-9]* artifacts/,
  );
  await expect(
    cleanupPanel.getByLabel("Artifact cleanup readiness"),
  ).toContainText("Ready for confirmation");
  await expect(
    cleanupPanel.getByLabel("Representative cleanup objects"),
  ).toContainText("file-transfer-sources/");
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeEnabled();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "system-maintenance-artifact-cleanup-preview",
    "System maintenance page after previewing a cleanup expression against a real uploaded source artifact with age, retention, and representative object evidence.",
  );

  await cleanupPanel.getByRole("button", { name: "Delete artifacts" }).click();
  const prompt = cleanupPanel.locator(".confirmationPrompt", {
    hasText: "Confirm artifact deletion",
  });
  await expect(prompt).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "system-maintenance-artifact-cleanup-confirm",
    "System maintenance page showing the destructive cleanup confirmation prompt with matched artifact count and preview evidence.",
  );
  await prompt
    .getByLabel("Type DELETE to confirm artifact deletion")
    .fill("DELETE");
  await prompt.getByRole("button", { name: "Delete artifacts" }).click();

  const serverJobsPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Maintenance jobs" }),
  });
  await expect(serverJobsPanel).toContainText("artifact cleanup", {
    timeout: 15_000,
  });
  await expect(serverJobsPanel).toContainText("queued");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "system-maintenance-artifact-cleanup-queued",
    "System maintenance page after queueing reviewed artifact cleanup from the browser.",
  );
  await expectCleanLayout(page);
}

async function fillWebhookTemplate(manager: Locator, value: string) {
  const editor = manager.locator(".webhookCodeMirror .cm-content").first();
  await expect(editor).toBeVisible();
  await editor.click();
  await editor.page().keyboard.press("Control+A");
  await editor.page().keyboard.press("Backspace");
  await editor.page().keyboard.insertText(value);
}

async function fillSearchExpression(editor: Locator, value: string) {
  await editor.click();
  await editor.page().keyboard.press("Control+A");
  await editor.page().keyboard.press("Backspace");
  await editor.page().keyboard.insertText(value);
  await editor.page().keyboard.press("Escape");
}

async function verifyDesktopSubpages(page: Page, projectName: string) {
  const subpages = [
    {
      view: "Fleet",
      subpage: "Alerts",
      marker: "Fleet alerts",
      screenshot: "page-fleet-alerts",
    },
    {
      view: "Fleet",
      subpage: "Groups",
      marker: "Tags",
      screenshot: "page-fleet-groups",
    },
    {
      view: "Fleet",
      subpage: "Assignments",
      marker: "Tag assignments",
      screenshot: "page-fleet-group-assignments",
    },
    {
      view: "Fleet",
      subpage: "Bulk groups",
      marker: "Bulk tags",
      screenshot: "page-fleet-bulk-groups",
    },
    {
      view: "Config",
      subpage: "Overview",
      marker: "Runtime config overview",
      screenshot: "page-config-overview",
    },
    {
      view: "Config",
      subpage: "Bulk patch",
      marker: "Bulk patch",
      screenshot: "page-config-bulk-apply",
    },
    {
      view: "Config",
      subpage: "Per-VPS",
      marker: "Per-VPS config",
      screenshot: "page-config-single-vps",
    },
    {
      view: "Config",
      subpage: "Template coverage",
      marker: "Template coverage",
      screenshot: "page-config-templates",
    },
    {
      view: "Jobs",
      subpage: "History",
      marker: "Job history",
      screenshot: "page-jobs-history",
    },
    {
      view: "Jobs",
      subpage: "Dispatch",
      marker: "Dispatch command",
      screenshot: "page-jobs-dispatch",
    },
    {
      view: "Remote Operations",
      subpage: "Files",
      marker: "VPS file browser",
      screenshot: "page-remote-operations-files",
    },
    {
      view: "Remote Operations",
      subpage: "Bulk files",
      marker: "Bulk files",
      screenshot: "page-remote-operations-bulk-files",
    },
    {
      view: "Automation",
      subpage: "Agent updates",
      marker: "Agent update registry",
      screenshot: "page-automation-agent-updates",
    },
    {
      view: "Remote Operations",
      subpage: "Transfers",
      marker: "File transfer sessions",
      screenshot: "page-remote-operations-transfers",
    },
    {
      view: "Remote Operations",
      subpage: "Terminal",
      marker: "Terminal sessions",
      screenshot: "page-remote-operations-terminal",
    },
    {
      view: "Remote Operations",
      subpage: "Processes",
      marker: "Process supervisor inventory",
      screenshot: "page-remote-operations-processes",
    },
    {
      view: "System",
      subpage: "Maintenance",
      marker: "Artifact cleanup",
      screenshot: "page-system-maintenance",
    },
    {
      view: "Jobs",
      subpage: "Scheduled runs",
      marker: "Scheduled runs",
      screenshot: "page-jobs-scheduled-runs",
    },
    {
      view: "Automation",
      subpage: "Schedules",
      marker: "Schedules",
      screenshot: "page-automation-schedules",
    },
    {
      view: "Network",
      subpage: "Graph",
      marker: "Topology graph",
      screenshot: "page-network-graph",
    },
    {
      view: "Network",
      subpage: "Tunnel plans",
      marker: "Tunnel plans",
      screenshot: "page-network-tunnel-plans",
    },
    {
      view: "Network",
      subpage: "Tests",
      marker: "Network tests",
      screenshot: "page-network-tests",
    },
    {
      view: "Network",
      subpage: "Evidence",
      marker: "Network evidence",
      screenshot: "page-network-evidence",
    },
    {
      view: "Network",
      subpage: "OSPF",
      marker: "vpsman / Network / OSPF",
      screenshot: "page-network-ospf",
    },
    {
      view: "Backups",
      subpage: "Overview",
      marker: "Backup overview",
      screenshot: "page-backups-overview",
    },
    {
      view: "Backups",
      subpage: "Requests",
      marker: "Backup requests",
      screenshot: "page-backups-requests",
    },
    {
      view: "Backups",
      subpage: "Policies",
      marker: "Backup policies",
      screenshot: "page-backups-policies",
    },
    {
      view: "Backups",
      subpage: "Artifacts",
      marker: "Backup artifacts",
      screenshot: "page-backups-artifacts",
    },
    {
      view: "Backups",
      subpage: "Restore",
      marker: "Restore operations",
      screenshot: "page-backups-restore",
    },
    {
      view: "Backups",
      subpage: "Migration",
      marker: "Migration links",
      screenshot: "page-backups-migration",
    },
    {
      view: "Observability",
      subpage: "Fleet metrics",
      marker: "Fleet metrics",
      screenshot: "page-observability-fleet-metrics",
    },
    {
      view: "Observability",
      subpage: "Network metrics",
      marker: "Network metrics",
      screenshot: "page-observability-network-metrics",
    },
    {
      view: "Observability",
      subpage: "Alerts",
      marker: "Alert policies",
      screenshot: "page-observability-alerts",
    },
    {
      view: "Observability",
      subpage: "Event webhooks",
      marker: "Event webhook rules",
      screenshot: "page-observability-webhooks",
    },
    {
      view: "Observability",
      subpage: "Dashboards",
      marker: "Dashboard presets",
      screenshot: "page-observability-dashboards",
    },
    {
      view: "Audit",
      subpage: "Events",
      marker: "Audit log",
      screenshot: "page-audit-events",
    },
    {
      view: "Audit",
      subpage: "Job evidence",
      marker: "Job audit evidence",
      screenshot: "page-audit-job-evidence",
    },
    {
      view: "Audit",
      subpage: "Retention & export",
      marker: "History retention",
      screenshot: "page-audit-retention",
    },
    {
      view: "Access",
      subpage: "Overview",
      marker: "Access overview",
      screenshot: "page-access-overview",
    },
    {
      view: "Access",
      subpage: "VPS identities",
      marker: "VPS identities",
      screenshot: "page-access-vps-identities",
    },
    {
      view: "Access",
      subpage: "Gateway sessions",
      marker: "Gateway sessions",
      screenshot: "page-access-gateway",
    },
    {
      view: "Access",
      subpage: "Privilege vault",
      marker: "Privilege vault",
      screenshot: "page-access-privilege-vault",
    },
    {
      view: "Access",
      subpage: "Operators",
      marker: "Operators",
      screenshot: "page-access-operators",
    },
    {
      view: "Audit",
      subpage: "Sessions",
      marker: "Session evidence",
      screenshot: "page-audit-sessions",
    },
    {
      view: "System",
      subpage: "Suite config",
      marker: "Suite config",
      screenshot: "page-system-config",
    },
    {
      view: "System",
      subpage: "Preferences",
      marker: "System preferences",
      screenshot: "page-system-preferences",
    },
  ] as const;

  for (const entry of subpages) {
    await openConsoleSubpage(page, entry.view, entry.subpage);
    await expectMainMarker(page, entry.marker);
    await expectCleanLayout(page);
    await maybeExtendedScreenshot(
      page,
      projectName,
      entry.screenshot,
      `${entry.view} / ${entry.subpage} page after live navigation and fixture-backed data load.`,
    );
  }
}

async function expectMainMarker(page: Page, text: string) {
  const main = page.locator("main");
  const heading = main
    .getByRole("heading", { name: text, exact: true })
    .first();
  try {
    await expect(heading).toBeVisible({ timeout: 2_500 });
    return;
  } catch {
    await expect(main.getByText(text, { exact: true }).first()).toBeVisible({
      timeout: 7_500,
    });
  }
}

async function maybeScreenshot(page: Page, projectName: string, name: string) {
  if (!screenshotDir) {
    return;
  }
  mkdirSync(screenshotDir, { recursive: true });
  await page.evaluate(() => window.scrollTo(0, 0));
  const screenshotPath = path.join(screenshotDir, `${projectName}-${name}.png`);
  await page.screenshot({
    fullPage: true,
    path: screenshotPath,
  });
  screenshotManifest.push({
    description: null,
    name,
    project: projectName,
    screenshot: screenshotPath,
  });
}

async function maybeExtendedScreenshot(
  page: Page,
  projectName: string,
  name: string,
  description: string,
) {
  if (!extendedReview) {
    return;
  }
  await maybeScreenshot(page, projectName, `extended-${name}`);
  const entry = [...screenshotManifest]
    .reverse()
    .find(
      (candidate) =>
        candidate.project === projectName &&
        candidate.name === `extended-${name}`,
    );
  if (entry) {
    entry.description = description;
  }
}

function writeScreenshotManifest(projectName: string) {
  if (!screenshotDir) {
    return;
  }
  mkdirSync(screenshotDir, { recursive: true });
  writeFileSync(
    path.join(screenshotDir, `${projectName}-screenshot-manifest.json`),
    `${JSON.stringify(
      {
        extended_review: extendedReview,
        generated_by: "live-docker-fleet",
        project: projectName,
        screenshots: screenshotManifest.filter(
          (entry) => entry.project === projectName,
        ),
      },
      null,
      2,
    )}\n`,
  );
}

function expectExtendedScreenshotNames(projectName: string, names: string[]) {
  if (!extendedReview || !screenshotDir) {
    return;
  }
  const captured = new Set(
    screenshotManifest
      .filter((entry) => entry.project === projectName)
      .map((entry) => entry.name),
  );
  for (const name of names) {
    expect(captured).toContain(name);
  }
}

function actionableConsoleErrors(errors: string[]): string[] {
  return errors.filter(
    (entry) =>
      !entry.includes("ResizeObserver loop") &&
      !entry.includes("net::ERR_NETWORK_CHANGED") &&
      !entry.includes("status of 401") &&
      !entry.includes("status of 404"),
  );
}
