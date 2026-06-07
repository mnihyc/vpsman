import path from "node:path";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { activate, openConsoleSubpage } from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_DOCKER_FLEET_UI_SMOKE, "enabled by scripts/smoke-docker-24-agent-fleet.sh");

const expectedTotal = Number(process.env.VPSMAN_DOCKER_FLEET_EXPECTED_TOTAL ?? "24");
const username = process.env.VPSMAN_DOCKER_FLEET_USERNAME ?? "docker-fleet-admin";
const password = process.env.VPSMAN_DOCKER_FLEET_PASSWORD ?? "docker-fleet-password";
const screenshotDir = process.env.VPSMAN_DOCKER_FLEET_SCREENSHOT_DIR;

test.setTimeout(180_000);

test("validates the live Docker fleet console with 20+ VPS agents", async ({ page }, testInfo) => {
  const isMobile = testInfo.project.name.includes("mobile");
  const consoleErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors.push(message.text());
    }
  });

  await login(page);
  await expect(page.getByRole("heading", { name: "Dashboard", exact: true })).toBeVisible();
  await expect(page.locator(".quickStats .metric", { hasText: "Connected" }).getByText(String(expectedTotal))).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByRole("heading", { name: "Operational Health" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Resource Usage" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Network", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Grouped Statistics" })).toBeVisible();
  await expectLiveDashboardTelemetry(page);
  await expectCleanLayout(page);

  await page.getByLabel("Dashboard group by").selectOption("providers");
  await expect(page.getByText(/All VPS; grouped by Providers/)).toBeVisible();
  await expect(page.locator(".dashboardClusterCard", { hasText: "provider:alpha" })).toBeVisible();
  await page.getByLabel("Dashboard scope kind").selectOption("country");
  await page.getByLabel("Dashboard scope value").selectOption("US");
  await expect(page.getByText(/country:US; grouped by Providers/)).toBeVisible({ timeout: 15_000 });
  await page.getByLabel("Dashboard group by").selectOption("date");
  await expect(page.getByText(/country:US; grouped by Date buckets/)).toBeVisible({ timeout: 15_000 });
  const dashboardPreferences = await page.evaluate(() =>
    JSON.parse(window.localStorage.getItem("vpsman.dashboardPreferences") ?? "{}"),
  );
  expect(dashboardPreferences).toMatchObject({
    groupBy: "date",
    scopeKind: "country",
    scopeValue: "US",
  });
  await maybeScreenshot(page, testInfo.project.name, "dashboard");
  if (isMobile) {
    await expectCleanLayout(page);
    expect(actionableConsoleErrors(consoleErrors)).toEqual([]);
    return;
  }
  const sidebarBox = await page.locator(".sidebar").boundingBox();
  expect(sidebarBox?.x).toBe(0);
  expect(sidebarBox?.y).toBe(0);

  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(page.getByRole("heading", { name: "Fleet overview" })).toBeVisible();
  const grid = page.getByLabel("VPS instance records data grid");
  await expect(grid.getByText(`${expectedTotal} of ${expectedTotal} instances`)).toBeVisible({ timeout: 20_000 });
  await grid.getByLabel("VPS instance records search").fill("provider:alpha");
  await expect(grid.getByText(`8 of ${expectedTotal} instances`)).toBeVisible();
  await grid.getByLabel("VPS instance records search").fill("");

  const firstRow = grid.locator(".gridBody [role=row]", { hasText: "df-alpha-US-01" }).first();
  const secondRow = grid.locator(".gridBody [role=row]").nth(1);
  await firstRow.click();
  await expect(page.locator(".inspector").getByRole("heading", { name: /df-alpha-US-01/ })).toBeVisible();
  await expectLiveFleetTelemetry(page);
  await firstRow.getByLabel("Expand VPS instance records row").click();
  await expect(grid.locator(".gridExpandedRow").first()).toContainText("root; uid 0");
  await firstRow.getByLabel("Select VPS instance records row").check();
  await secondRow.getByLabel("Select VPS instance records row").check();
  await expect(grid.getByText("2 selected")).toBeVisible();
  await grid.getByRole("button", { name: "Selection" }).click();
  await expect(page.getByRole("menuitem", { name: "Copy client IDs" })).toBeVisible();
  await page.keyboard.press("Escape");
  await firstRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Inspect selected" })).toBeVisible();
  await page.keyboard.press("Escape");
  await exerciseColumnControls(page, grid);
  await maybeScreenshot(page, testInfo.project.name, "fleet");
  await expectCleanLayout(page);

  await openConsoleSubpage(page, "Tags", "Targeting");
  await expect(page.getByRole("heading", { name: "Targeting" })).toBeVisible();
  await checkControl(page.getByLabel("provider:alpha", { exact: true }));
  await checkControl(page.getByLabel("country:US", { exact: true }));
  await page.getByRole("group", { name: "Bulk tag match mode" }).getByRole("button", { name: "All", exact: true }).click();
  await page.getByRole("button", { name: "Preview targets" }).click();
  await expect(page.getByText("2 targets")).toBeVisible();
  await expectCleanLayout(page);

  await verifyDesktopSubpages(page);
  await openConsoleSubpage(page, "Preferences", "Operator");
  await page.getByLabel("Default expansion").selectOption("active");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(page.locator(".consoleStatusBadge", { hasText: /^Saved$/ })).toBeVisible();
  await maybeScreenshot(page, testInfo.project.name, "preferences");

  expect(actionableConsoleErrors(consoleErrors)).toEqual([]);
});

async function login(page: Page) {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Operator access" })).toBeVisible({ timeout: 20_000 });
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Submit login" }).click();
  await expect(page.getByRole("heading", { name: "Dashboard", exact: true })).toBeVisible({ timeout: 30_000 });
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
  const resourceUsage = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Resource Usage" }),
  });
  await expect(resourceUsage).toContainText(`${expectedTotal} VPS plotted`);
  await expect(resourceUsage.getByLabel("Resource usage curve")).toBeVisible();
  await expect(resourceUsage).not.toContainText(/No resource telemetry|No data|No rollup|unavailable/i);
  await resourceUsage.getByRole("button", { name: "Memory", exact: true }).click();
  await expect(resourceUsage).toContainText("Memory used");
  await resourceUsage.getByRole("button", { name: "Disk", exact: true }).click();
  await expect(resourceUsage).toContainText("Disk free");

  const networkSection = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Network", exact: true }),
  });
  await networkSection.getByRole("button", { name: "Speed", exact: true }).click();
  await expect(networkSection.getByLabel("Network speed curve")).toBeVisible();
  await expect(networkSection).not.toContainText(/No network speed samples|unavailable/i);
  expect(await networkSection.locator(".dashboardClientRow").count()).toBeGreaterThan(0);
  await networkSection.getByRole("button", { name: "Traffic", exact: true }).click();
  await expect(networkSection.getByLabel("Network traffic curve")).toBeVisible();
  await expect(networkSection).not.toContainText(/No network traffic samples|unavailable/i);
  expect(await networkSection.locator(".dashboardClientRow").count()).toBeGreaterThan(0);
}

async function expectLiveFleetTelemetry(page: Page) {
  const inspector = page.locator(".inspector");
  await expect(inspector.locator(".metric", { hasText: "Traffic" })).not.toContainText(/No rate samples|No counters|No rollup|No data|unavailable/i);
  await expect(inspector.locator(".metric", { hasText: "Samples" })).not.toContainText(/No rollup|No data|unavailable/i);
  await page.getByRole("tab", { name: "Telemetry" }).click();
  await expect(inspector.getByText(/CPU load/)).toBeVisible();
  await expect(inspector).not.toContainText(/No rollup|No rate samples|No counters|No data|unavailable/i);
}

async function checkControl(locator: Locator) {
  await locator.evaluate((element) => {
    const input = element as HTMLInputElement;
    if (!input.checked) {
      input.click();
    }
  });
}

async function exerciseColumnControls(page: Page, grid: Locator) {
  const nameHeader = grid.locator(".gridHeaderCell", { hasText: "Name" }).first();
  const providerHeader = grid.locator(".gridHeaderCell", { hasText: "Provider" }).first();
  const tagsHeader = grid.locator(".gridHeaderCell", { hasText: "Tags" }).first();
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

  await nameHeader.locator(".gridDragHandle").dragTo(providerHeader.locator(".gridDragHandle"));
  await grid.getByLabel("VPS instance records columns").click();
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  await expect(grid.getByRole("columnheader", { name: /Provider/ })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await grid.getByLabel("VPS instance records page size").selectOption("25");
  await expect(grid.getByText(`1 / 1`)).toBeVisible();
}

async function verifyDesktopSubpages(page: Page) {
  const subpages = [
    ["Fleet", "Alerts", "Fleet alerts"],
    ["Fleet", "Alert policies", "Alert policies"],
    ["Fleet", "Notifications", "Notification channels"],
    ["Tags", "Tag registry", "Tags"],
    ["Tags", "Data-source presets", "Data-source presets"],
    ["Tags", "Source status", "Active source status"],
    ["Jobs", "History", "Job history"],
    ["Jobs", "Dispatch", "Dispatch command"],
    ["Jobs", "Updates", "Agent update releases"],
    ["Jobs", "Transfer history", "File transfer sessions"],
    ["Jobs", "Terminal sessions", "Terminal sessions"],
    ["Jobs", "Processes", "Process supervisor inventory"],
    ["Jobs", "Approvals", "No privileged approvals"],
    ["Schedules", "Schedule registry", "Schedules"],
    ["Topology", "Graph", "Topology graph"],
    ["Topology", "Tunnel plans", "Tunnel plans"],
    ["Topology", "Apply / rollback", "Network apply"],
    ["Topology", "Promotion", "Tunnel promotion"],
    ["Topology", "Evidence", "Topology evidence"],
    ["Topology", "OSPF", "vpsman / Topology / OSPF"],
    ["Backups", "Requests", "Backup requests"],
    ["Backups", "Policies", "Backup policies"],
    ["Backups", "Artifacts", "Backup artifacts"],
    ["Backups", "Restore", "Restore operations"],
    ["Backups", "Migration", "Migration links"],
    ["Audit", "Events", "Audit log"],
    ["Audit", "Retention", "History retention"],
    ["Access", "Overview", "Operator session"],
    ["Access", "Operators", "Operators"],
    ["Access", "VPS keys", "Enrollment tokens"],
    ["Access", "Gateway", "Gateway sessions"],
    ["Access", "Proof vault", "Proof rotation history"],
  ] as const;

  for (const [view, subpage, heading] of subpages) {
    await openConsoleSubpage(page, view, subpage);
    await expectMainMarker(page, heading);
    await expectCleanLayout(page);
  }
}

async function expectMainMarker(page: Page, text: string) {
  const main = page.locator("main");
  const heading = main.getByRole("heading", { name: text, exact: true }).first();
  try {
    await expect(heading).toBeVisible({ timeout: 2_500 });
    return;
  } catch {
    await expect(main.getByText(text, { exact: true }).first()).toBeVisible({ timeout: 7_500 });
  }
}

async function maybeScreenshot(page: Page, projectName: string, name: string) {
  if (!screenshotDir) {
    return;
  }
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({
    fullPage: true,
    path: path.join(screenshotDir, `${projectName}-${name}.png`),
  });
}

function actionableConsoleErrors(errors: string[]): string[] {
  return errors.filter(
    (entry) =>
      !entry.includes("ResizeObserver loop") &&
      !entry.includes("status of 401") &&
      !entry.includes("status of 404"),
  );
}
