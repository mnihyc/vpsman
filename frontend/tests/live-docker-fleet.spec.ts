import path from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";
import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  activate,
  activateSystemMaintenanceSubpanel,
  openConsoleSubpage,
} from "./support/consoleNavigation";

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

test.setTimeout(extendedReview ? 1_200_000 : 300_000);

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
    page.getByRole("heading", { level: 1, name: "Home", exact: true }),
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
    page.getByRole("heading", { name: "Recent issues" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Needs attention" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home fleet scan")).toBeVisible();
  await expect(page.getByLabel("Home telemetry widgets")).toBeVisible();
  await expectCleanLayout(page);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "page-home-overview",
    "Home overview before operator action, focused on quick actions, fleet availability, running work, and failures.",
  );

  if (!extendedReview) {
    await maybeScreenshot(page, testInfo.project.name, "home");
  }
  await expectLiveSystemDashboardTelemetry(page, testInfo.project.name);
  if (isMobile) {
    writeScreenshotManifest(testInfo.project.name);
    await expectCleanLayout(page);
    await expect(page.locator(".workspaceRouteError")).toHaveCount(0);
    expect(recoveredWorkspaceModuleTimeouts(consoleErrors)).toBeLessThanOrEqual(
      1,
    );
    expect(actionableConsoleErrors(consoleErrors)).toEqual([]);
    return;
  }
  const sidebarBox = await page.locator(".sidebar").boundingBox();
  expect(sidebarBox?.x).toBe(0);
  expect(sidebarBox?.y).toBe(0);

  await openLiveConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet instances" }),
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
  const gridSearch = grid.getByLabel("VPS instance records search");
  await gridSearch.fill("provider:alpha");
  await expect(
    grid.getByText(`${providerAlphaCount} of ${expectedTotal} instances`),
  ).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-search-provider-alpha",
    "Fleet table after operator filters the live fleet to provider alpha.",
  );

  await gridSearch.fill("df-alpha-US-01");
  await expect(grid.getByText(`1 of ${expectedTotal} instances`)).toBeVisible();
  const firstRow = grid
    .locator(".gridBody [role=row]", { hasText: "df-alpha-US-01" })
    .first();
  const inlineDetail = await openFleetInlineDetail(grid, "df-alpha-US-01");
  await expect(inlineDetail).toContainText("Root uid 0");
  await activate(inlineDetail.getByRole("tab", { name: "Telemetry" }));
  await expectLiveFleetTelemetry(inlineDetail);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-inline-instance-detail-telemetry",
    "Fleet inline VPS detail opened from the row chevron with live telemetry visible.",
  );
  await activate(
    inlineDetail.getByLabel("Close VPS instance records row details"),
  );
  await expect(inlineDetail).toHaveCount(0);
  await openFleetDetailRoute(page, grid, "df-alpha-US-01");
  await expect(
    page.getByRole("heading", { level: 1, name: "Instance detail" }),
  ).toBeVisible({
    timeout: 30_000,
  });
  const canonicalDetail = page.getByRole("region", {
    name: "Canonical VPS detail",
  });
  await expect(
    canonicalDetail.getByRole("heading", { level: 2, name: "VPS detail" }),
  ).toBeVisible();
  await expect(page.getByLabel("Selected VPS identity")).toContainText(
    "df-alpha-US-01",
  );
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-instance-detail-route",
    "Canonical VPS detail page opened from the selected-row Actions menu.",
  );
  await openLiveConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet instances" }),
  ).toBeVisible();
  await gridSearch.fill("provider:alpha");
  await expect(
    grid.getByText(`${providerAlphaCount} of ${expectedTotal} instances`),
  ).toBeVisible();
  await selectVisibleGridRows(grid);
  await expect(
    grid.getByText(`${providerAlphaCount} selected`, { exact: true }),
  ).toBeVisible();
  const selectionPanel = page.locator(".fleetSelectionPanel");
  await expect(selectionPanel).toContainText(
    `${providerAlphaCount} selected VPSs`,
  );
  await expect(
    selectionPanel.getByRole("tab", { name: "Telemetry" }),
  ).toBeVisible();
  await activate(selectionPanel.getByRole("tab", { name: "Network" }));
  await expect(
    selectionPanel.getByRole("tabpanel", {
      name: "Network",
    }),
  ).toContainText("df-alpha-DE-10");
  await activate(selectionPanel.getByRole("tab", { name: "Capabilities" }));
  await expect(
    selectionPanel.getByRole("tabpanel", {
      name: "Capabilities",
    }),
  ).toContainText("Root uid 0");
  await forceClick(grid.getByRole("button", { name: "Actions", exact: true }));
  await expect(
    page.getByRole("menuitem", { name: "Copy client IDs" }),
  ).toBeVisible();
  const actionMenu = page.locator(".consoleMenu:visible").last();
  const actionMenuBox = await actionMenu.boundingBox();
  const viewport = page.viewportSize();
  expect(actionMenuBox).not.toBeNull();
  expect(viewport).not.toBeNull();
  expect(
    (actionMenuBox?.y ?? 0) + (actionMenuBox?.height ?? 0),
  ).toBeLessThanOrEqual((viewport?.height ?? 0) - 12);
  const deletionAction = page.getByRole("menuitem", {
    name: "Review VPS deletion",
  });
  await deletionAction.scrollIntoViewIfNeeded();
  await expect(deletionAction).toBeVisible();
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-action-menu-open",
    "Scrollable bulk action menu showing the bounded destructive action after selecting provider-filtered VPS rows.",
  );
  await page.keyboard.press("Escape");
  await gridSearch.fill("df-alpha-US-01");
  await expect(grid.getByText(`1 of ${expectedTotal} instances`)).toBeVisible();
  await expect(firstRow).toBeVisible();
  await firstRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Open detail" }),
  ).toBeVisible();
  const rowBox = await firstRow.boundingBox();
  const contextMenuBox = await page
    .locator(".consoleMenu:visible")
    .boundingBox();
  expect(rowBox).not.toBeNull();
  expect(contextMenuBox).not.toBeNull();
  expect(contextMenuBox?.x ?? 0).toBeGreaterThanOrEqual(rowBox?.x ?? 0);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-row-context-menu-open",
    "Right-click context menu opened on a fleet row with selected-row actions preserved.",
  );
  await page.keyboard.press("Escape");
  await gridSearch.fill("provider:alpha");
  await expect(
    grid.getByText(`${providerAlphaCount} of ${expectedTotal} instances`),
  ).toBeVisible();
  await clearVisibleGridRows(grid);
  await expect(grid.getByText("0 selected", { exact: true })).toBeVisible();
  await gridSearch.fill("");
  await expect(
    grid.getByText(`${expectedTotal} of ${expectedTotal} instances`),
  ).toBeVisible();
  await exerciseColumnControls(page, grid);
  await maybeExtendedScreenshot(
    page,
    testInfo.project.name,
    "fleet-column-controls-result",
    "Fleet table after operator resizes/reorders columns, hides Provider, and expands page size.",
  );
  if (!extendedReview) {
    await maybeScreenshot(page, testInfo.project.name, "fleet");
  }
  await expectCleanLayout(page);

  await openLiveConsoleSubpage(page, "Fleet", "Bulk groups");
  await expect(
    page.getByRole("heading", { level: 1, name: "Bulk groups" }),
  ).toBeVisible();
  await page
    .getByLabel("Bulk group", { exact: true })
    .fill("maintenance:2026-q2-patch");
  await page
    .getByRole("combobox", { name: "Bulk group selector expression" })
    .fill("provider:alpha && country:US");
  await page.keyboard.press("Escape");
  const bulkTagSelectorStatus = page
    .locator(".searchExpressionInput", {
      has: page.getByRole("combobox", {
        name: "Bulk group selector expression",
      }),
    })
    .locator(".searchExpressionMeta");
  await expect(bulkTagSelectorStatus).toHaveText(
    `${providerAlphaCountryUsCount}/${expectedTotal}`,
  );
  await expect(bulkTagSelectorStatus).toHaveAttribute(
    "title",
    new RegExp(
      `Local match ${providerAlphaCountryUsCount} VPSs.*${providerAlphaCountryUsCount} ready`,
    ),
  );
  const bulkTagAction = page.getByRole("button", {
    name: new RegExp(
      `Add maintenance:2026-q2-patch to ${providerAlphaCountryUsCount} VPSs`,
    ),
  });
  await expect(bulkTagAction).toBeEnabled();
  await activate(bulkTagAction);
  await expect(page.getByLabel("Bulk group target preview")).toContainText(
    "Server preview",
  );
  await expect(page.getByLabel("Bulk group preview evidence")).toBeVisible({
    timeout: 30_000,
  });
  const bulkTagPreviewChips = page.locator(".bulkTagPreview");
  await expect(bulkTagPreviewChips).toContainText("df-alpha-US-01", {
    timeout: 30_000,
  });
  await expect(bulkTagPreviewChips).toContainText("df-alpha-US-13");
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
  await openLiveConsoleSubpage(page, "System", "Preferences");
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
  if (!extendedReview) {
    await maybeScreenshot(page, testInfo.project.name, "preferences");
  }
  writeScreenshotManifest(testInfo.project.name);

  await expect(page.locator(".workspaceRouteError")).toHaveCount(0);
  expect(recoveredWorkspaceModuleTimeouts(consoleErrors)).toBeLessThanOrEqual(
    1,
  );
  expect(actionableConsoleErrors(consoleErrors)).toEqual([]);
});

async function login(page: Page) {
  let lastError: unknown;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goto("/", { waitUntil: "domcontentloaded" });
    const home = page.getByRole("heading", {
      level: 1,
      name: "Home",
      exact: true,
    });
    const access = page.getByRole("heading", {
      exact: true,
      name: "Sign in",
    });
    try {
      const state = await Promise.race([
        home
          .waitFor({ state: "visible", timeout: 20_000 })
          .then(() => "home" as const),
        access
          .waitFor({ state: "visible", timeout: 20_000 })
          .then(() => "access" as const),
      ]);
      if (state === "home") {
        return;
      }
      await page.getByLabel("Username").fill(username);
      await page.getByLabel("Password").fill(password);
      await page.getByRole("button", { name: "Sign in" }).click();
      await expect(home).toBeVisible({ timeout: 30_000 });
      return;
    } catch (error) {
      lastError = error;
      await page.reload({ waitUntil: "domcontentloaded" });
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

async function openLiveConsoleSubpage(
  page: Page,
  view: string,
  subpage: string,
  expectedHeaderTitle?: string,
) {
  try {
    await openConsoleSubpage(page, view, subpage, expectedHeaderTitle);
    return;
  } catch (error) {
    const lostSession =
      error instanceof Error &&
      error.message.includes("authenticated session was lost");
    const operatorAccessVisible = await page
      .getByRole("heading", { exact: true, name: "Sign in" })
      .isVisible()
      .catch(() => false);
    if (!lostSession && !operatorAccessVisible) {
      throw error;
    }
    await login(page);
    await openConsoleSubpage(page, view, subpage, expectedHeaderTitle);
  }
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
    .getByRole("button", { name: "Rate", exact: true })
    .click();
  await expect(
    networkSection.getByLabel("Network interval-average rate curve"),
  ).toBeVisible();
  await expect(networkSection).not.toContainText(
    /No network rate samples|unavailable/i,
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
  await openLiveConsoleSubpage(page, "System", "Overview");
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "System overview",
      exact: true,
    }),
  ).toBeVisible();

  const systemWorkspace = page.locator(".systemWorkspace");
  await expect(
    systemWorkspace.getByRole("heading", {
      name: "Control-plane overview",
      exact: true,
    }),
  ).toBeVisible();
  const serviceHealth = systemWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Service health", exact: true }),
  });
  await expect(serviceHealth).toContainText("Database");
  await expect(serviceHealth).toContainText("Control-plane queue");
  await expect(serviceHealth).toContainText("Worker");
  await expect(serviceHealth).toContainText("Gateway");
  await expect(serviceHealth).toContainText("What needs attention");
  await expect(serviceHealth).not.toContainText(/No durable metric samples/i);
  await expectCleanLayout(page);
  await maybeExtendedScreenshot(
    page,
    projectName,
    "page-system-dashboard",
    "System / Overview page with live control-plane posture, service health, and attention signals.",
  );

  await openLiveConsoleSubpage(page, "System", "Capacity");
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "System capacity",
      exact: true,
    }),
  ).toBeVisible();
  const capacityWorkspace = page.locator(".systemWorkspace");
  await expect(
    capacityWorkspace.getByRole("heading", {
      name: "Capacity telemetry",
      exact: true,
    }),
  ).toBeVisible();
  const subsystem = capacityWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", {
      name: "Subsystem capacity",
      exact: true,
    }),
  });
  await expect(subsystem).toContainText("Database");
  await expect(subsystem).toContainText("Dispatch");
  await expect(subsystem).toContainText("Gateway");
  await expect(subsystem).toContainText("Dispatch limit");
  await expect(subsystem).toContainText("Suite Config fields");

  const dispatch = capacityWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Dispatch capacity", exact: true }),
  });
  await expect(dispatch).toContainText("Dispatch queue");
  await expect(dispatch).toContainText("Active targets");
  await expect(dispatch).toContainText("Warning threshold");

  await capacityWorkspace.getByRole("tab", { name: /Database/ }).click();
  const database = capacityWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Database capacity", exact: true }),
  });
  await expect(database).toContainText("API DB pool");
  await expect(database).toContainText("Worker DB pool");
  await expect(database).toContainText("In use");

  await capacityWorkspace.getByRole("tab", { name: /Gateway/ }).click();
  const gateway = capacityWorkspace.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Gateway capacity", exact: true }),
  });
  await expect(gateway).toContainText("Queue depth");
  await expect(gateway).toContainText("Rejected connects");
  await expect(gateway).not.toContainText(/No durable metric samples/i);
  await expectCleanLayout(page);
  await maybeExtendedScreenshot(
    page,
    projectName,
    "page-system-capacity",
    "System / Capacity page with live database, dispatch, and gateway telemetry.",
  );
}

async function expectLiveFleetTelemetry(detail: Locator) {
  await expect(detail.locator(".metric", { hasText: "Traffic" })).toContainText(
    /\d+(?:\.\d+)?\s*(?:B|KB|MB|GB|TB|KiB|MiB|GiB|TiB)(?:\/s)?/i,
    {
      timeout: 30_000,
    },
  );
  await expect(detail.locator(".metric", { hasText: "Samples" })).toContainText(
    /\d+\s+(?:rollup|rate)\b/i,
    { timeout: 30_000 },
  );
  await activate(detail.getByRole("tab", { name: "Telemetry" }));
  const telemetryPanel = detail.getByRole("tabpanel");
  await expect(telemetryPanel.getByText("CPU load")).toBeVisible();
}

async function openFleetInlineDetail(grid: Locator, displayName: string) {
  const inlineDetail = grid.locator(".gridExpandedRow", {
    hasText: displayName,
  });
  let lastError: unknown;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    try {
      if ((await inlineDetail.count()) > 0) {
        await expect(inlineDetail).toBeVisible({ timeout: 1000 });
        return inlineDetail;
      }
      const row = grid
        .locator(".gridBody [role=row]", { hasText: displayName })
        .first();
      await expect(row).toBeVisible({ timeout: 3000 });
      await clickVisibleGridRowButton(
        grid,
        displayName,
        /^Expand VPS instance records row/i,
      );
      await expect(inlineDetail).toBeVisible({ timeout: 3000 });
      return inlineDetail;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

async function openFleetDetailRoute(
  page: Page,
  grid: Locator,
  displayName: string,
) {
  const instanceDetailCrumb = page.getByText(
    "vpsman / Fleet / Instance detail",
  );
  let lastError: unknown;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    try {
      const row = grid
        .locator(".gridBody [role=row]", { hasText: displayName })
        .first();
      await expect(row).toBeVisible({ timeout: 3000 });
      await row.getByRole("checkbox").first().check();
      await forceClick(
        grid
          .locator(".gridToolbarActions")
          .getByRole("button", { name: "Actions", exact: true }),
      );
      await forceClick(
        page.getByRole("menuitem", { name: "Open detail", exact: true }),
      );
      await expect(instanceDetailCrumb).toBeVisible({ timeout: 5000 });
      return;
    } catch (error) {
      lastError = error;
      await page.keyboard.press("Escape");
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

async function clickVisibleGridRowButton(
  grid: Locator,
  displayName: string,
  labelPattern: RegExp,
) {
  const result = await grid.evaluate(
    (gridElement, args) => {
      const pattern = new RegExp(args.labelPattern, "i");
      const rows = Array.from(
        gridElement.querySelectorAll<HTMLElement>(".gridBody [role=row]"),
      );
      const row = rows.find((candidate) =>
        candidate.textContent?.includes(args.displayName),
      );
      if (!row) {
        return "row-missing";
      }
      const button = Array.from(
        row.querySelectorAll<HTMLButtonElement>("button"),
      ).find(
        (candidate) =>
          pattern.test(candidate.getAttribute("aria-label") ?? "") ||
          pattern.test(candidate.textContent ?? ""),
      );
      if (!button) {
        return "button-missing";
      }
      if (button.disabled) {
        return "button-disabled";
      }
      button.click();
      return "clicked";
    },
    {
      displayName,
      labelPattern: labelPattern.source,
    },
  );
  expect(result).toBe("clicked");
}

async function exerciseColumnControls(page: Page, grid: Locator) {
  const vpsHeader = grid.locator(".gridHeaderCell", { hasText: "VPS" }).first();
  const locationHeader = grid
    .locator(".gridHeaderCell", { hasText: "Location" })
    .first();
  const resizeHandle = locationHeader.locator(".gridResizeHandle");
  await expect(resizeHandle).toBeVisible();
  const box = await resizeHandle.boundingBox();
  expect(box).not.toBeNull();
  if (box) {
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 38, box.y + box.height / 2, { steps: 5 });
    await page.mouse.up();
  }

  await expect(vpsHeader.locator(".gridDragHandle")).toBeVisible();
  await vpsHeader.locator(".gridDragHandle").focus();
  await page.keyboard.press("Space");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Space");
  await forceClick(grid.getByLabel("VPS instance records columns"));
  await forceClick(page.getByRole("menuitemcheckbox", { name: "Provider" }));
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("menuitemcheckbox", { name: "Provider" }),
  ).toHaveCount(0);
  await forceClick(grid.getByLabel("VPS instance records columns"));
  await forceClick(page.getByRole("menuitemcheckbox", { name: "Provider" }));
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("menuitemcheckbox", { name: "Provider" }),
  ).toHaveCount(0);
  await grid.getByLabel("VPS instance records search").focus();
  await grid.getByLabel("VPS instance records page size").selectOption("25");
  await expect(
    grid.getByText(`1 / ${Math.ceil(expectedTotal / 25)}`),
  ).toBeVisible();
}

async function clearVisibleGridRows(grid: Locator) {
  const checkbox = grid.getByLabel("Select all VPS instance records");
  const clearVisible = grid.getByRole("button", {
    name: "Clear visible VPS instance records",
  });
  if ((await clearVisible.count()) > 0) {
    await activate(clearVisible);
  }
  await expect(checkbox).not.toBeChecked({ timeout: 3000 });
}

async function selectVisibleGridRows(grid: Locator) {
  const checkbox = grid.getByLabel("Select all VPS instance records");
  const selectVisible = grid.getByRole("button", {
    name: "Select visible VPS instance records",
  });
  let lastError: unknown;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    try {
      await activate(selectVisible);
      await expect(checkbox).toBeChecked({ timeout: 3000 });
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw lastError;
}

async function exerciseExpressionWebhooks(page: Page, projectName: string) {
  await openLiveConsoleSubpage(page, "Observability", "Event webhooks");
  await expect(
    page.getByRole("heading", { level: 1, name: "Event webhooks" }),
  ).toBeVisible();
  const webhooks = page.locator(".observabilityWebhooksPanel");
  await expect(
    webhooks.getByText("Event webhook rules", { exact: true }).first(),
  ).toBeVisible();

  await webhooks.getByRole("button", { name: "Create rule" }).click();
  const detail = webhooks.locator(".consoleDetailPanel").filter({
    hasText: /Create webhook rule|Edit webhook rule/,
  });
  await expect(detail).toBeVisible();
  await detail.getByLabel("Webhook rule name").fill("docker-fleet-q2-capacity");
  await detail
    .getByLabel("Webhook target")
    .fill("http://127.0.0.1:9/vpsman/docker-fleet");
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
  const enableAfterCreation = detail.getByRole("checkbox", {
    name: "Enable after creation",
  });
  await expect(enableAfterCreation).not.toBeChecked();
  await enableAfterCreation.check();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rule-form-filled",
    "Webhook rule editor filled with the reviewed local smoke target, expression, template, and event kind.",
  );
  await detail.getByRole("button", { name: "Create rule" }).click();
  await expect(
    page.locator(".confirmationPrompt", {
      hasText: "Confirm webhook rule save",
    }),
  ).toBeVisible();
  await page
    .locator(".confirmationPrompt", {
      hasText: "Confirm webhook rule save",
    })
    .getByRole("button", { name: "Create rule" })
    .click();
  const savedFeedback = webhooks.locator(
    ".webhookEditorActionFeedback.actionFeedbackSuccess",
    { hasText: "saved docker-fleet-q2-capacity" },
  );
  await expect(savedFeedback).toBeVisible({ timeout: 30_000 });
  await savedFeedback.scrollIntoViewIfNeeded();
  await expect(webhooks).toContainText("docker-fleet-q2-capacity", {
    timeout: 30_000,
  });
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rule-saved",
    "Webhook editor footer showing saved status and the transition from Create rule to Update rule.",
  );

  await detail.getByRole("button", { name: "Test" }).click();
  const samplePreview = webhooks.locator(".webhookRuleSamplePreview");
  await expect(samplePreview).toBeVisible({ timeout: 90_000 });
  await expect(samplePreview).toContainText(`${roleEdgeCount} VPSs matched`, {
    timeout: 30_000,
  });
  await expect(samplePreview).toContainText(
    `docker-fleet-q2-capacity interval.30sec count=${roleEdgeCount}`,
    { timeout: 30_000 },
  );
  await expect(samplePreview).toContainText("df-alpha-US-01", {
    timeout: 30_000,
  });
  await expect(samplePreview).toContainText("dry run");
  await expect(samplePreview).not.toContainText("matched_dry_run");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rule-preview-result",
    "Webhook dry-run result showing matched live VPSs and rendered payload preview.",
  );

  await detail.getByLabel("Close detail panel").click();
  await webhooks.getByRole("button", { name: "Preview match" }).click();
  await expect(
    webhooks.getByRole("tab", { name: /^Deliveries\b/ }),
  ).toHaveAttribute("aria-selected", "true", { timeout: 30_000 });
  await expect(
    webhooks.locator(".deliveryPreviewSection", {
      hasText: "Event webhook delivery preview",
    }),
  ).toBeVisible({ timeout: 30_000 });
  await expect(
    webhooks.locator(".consoleDataGrid", {
      hasText: "docker-fleet-q2-capacity",
    }),
  ).toBeVisible({ timeout: 30_000 });
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-match-rules-result",
    "Webhook queue operation after matching saved rules against the preview event.",
  );

  await webhooks.getByRole("tab", { name: "Maintenance" }).click();
  await webhooks.getByLabel("Webhook rotation days").fill("7");
  await webhooks
    .getByLabel("Webhook rotation status")
    .selectOption("delivered");
  await webhooks.getByRole("button", { name: "Review rotation" }).click();
  await expect(
    webhooks.locator(".fleetPolicyActionFeedback.actionFeedbackSuccess", {
      hasText: "0 matched / 0 deleted",
    }),
  ).toBeVisible({ timeout: 30_000 });
  await maybeExtendedScreenshot(
    page,
    projectName,
    "webhook-rotation-preview-result",
    "Webhook retention maintenance after previewing rotation without deleting records.",
  );
  await expectCleanLayout(page);
}

async function exerciseAlertPolicyReview(page: Page, projectName: string) {
  await openLiveConsoleSubpage(page, "Observability", "Alerts");
  await expect(
    page.getByRole("heading", { level: 1, name: "Alerts" }),
  ).toBeVisible();
  const alerts = page.locator(".observabilityAlertsPanel");
  await expect(
    alerts.getByRole("heading", { name: "Alert policies" }),
  ).toBeVisible();
  const grid = alerts.getByLabel("Policy groups data grid");
  const row = grid
    .locator(".gridBody [role=row]", { hasText: "docker-edge-resource-alerts" })
    .first();
  await expect(row).toBeVisible();
  await row.getByLabel("Expand Policy groups row").click();
  const expandedPolicy = grid.locator(".gridExpandedRow", {
    hasText: "docker-edge-resource-alerts",
  });
  await expect(expandedPolicy).toBeVisible();
  await expect(expandedPolicy).toHaveCSS("opacity", "1");
  await expandedPolicy.scrollIntoViewIfNeeded();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "alert-policy-inline-detail",
    "Alert policy row opened with inline chevron detail on the seeded live policy.",
  );

  await row.getByLabel("Select Policy groups row").check({ force: true });
  await forceClick(grid.getByRole("button", { name: "Actions", exact: true }));
  await forceClick(page.getByRole("menuitem", { name: "Details" }));
  await expect(page.getByText("Alert policy details")).toBeVisible();
  const policyDetail = alerts.locator(".consoleDetailPanel", {
    hasText: "Alert policy details",
  });
  await policyDetail.scrollIntoViewIfNeeded();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "alert-policy-below-table-detail",
    "Alert policy detail action opened the same policy details below the table.",
  );

  await page.getByRole("button", { name: "Edit policy" }).click();
  await expect(page.getByText("Edit alert policy")).toBeVisible();
  await expect(page.getByLabel("Policy VPS selector expression")).toHaveValue(
    "tag:role:edge",
  );
  const conditionExpression = page.getByLabel("Rule condition expression");
  await expect(conditionExpression).toHaveValue("cpu.load_1 >= 0.5");
  await conditionExpression.fill("cpu.load_1 >= 0.55");
  await expect(conditionExpression).toHaveValue("cpu.load_1 >= 0.55");
  await maybeExtendedScreenshot(
    page,
    projectName,
    "alert-policy-edit-rule-expression",
    "Alert policy edit panel with selector expression and rule-row editor after operator changes the CPU condition.",
  );
  await page.getByLabel("Close detail panel").click();
}

async function exerciseAlertNotificationChannels(
  page: Page,
  projectName: string,
) {
  await openLiveConsoleSubpage(page, "Observability", "Alerts");
  await expect(
    page.getByRole("heading", { level: 1, name: "Alerts" }),
  ).toBeVisible();
  const notifications = page.locator(".observabilityAlertsPanel");
  await notifications.getByRole("tab", { name: "Destinations" }).click();
  await expect(
    notifications.getByRole("heading", { name: "Notification channels" }),
  ).toBeVisible();
  const grid = notifications.getByLabel(
    "Alert notification channels data grid",
  );
  const row = grid
    .locator(".gridBody [role=row]", {
      hasText: "docker-resource-webhook",
    })
    .first();
  await expect(row).toBeVisible();
  await row.getByLabel("Expand Alert notification channels row").click();
  const expandedChannel = grid.locator(".gridExpandedRow", {
    hasText: "docker-resource-webhook",
  });
  await expect(expandedChannel).toBeVisible();
  await expect(expandedChannel).toHaveCSS("opacity", "1");
  await expandedChannel.scrollIntoViewIfNeeded();
  await maybeExtendedScreenshot(
    page,
    projectName,
    "notification-channel-inline-detail",
    "Notification channel row opened with inline chevron detail for the seeded resource webhook channel.",
  );

  await row
    .getByLabel("Select Alert notification channels row")
    .check({ force: true });
  await forceClick(grid.getByRole("button", { name: "Actions", exact: true }));
  await forceClick(page.getByRole("menuitem", { name: "Details" }));
  await expect(page.getByText("Notification channel details")).toBeVisible();
  const channelDetail = notifications.locator(".consoleDetailPanel", {
    hasText: "Notification channel details",
  });
  await channelDetail.scrollIntoViewIfNeeded();
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

  await notifications.getByRole("button", { name: "Preview match" }).click();
  await expect(
    notifications.getByRole("tab", { name: /^Deliveries\b/ }),
  ).toHaveAttribute("aria-selected", "true", { timeout: 30_000 });
  await expect(
    notifications.locator(".deliveryPreviewSection", {
      hasText: "Notification delivery preview",
    }),
  ).toBeVisible({ timeout: 30_000 });
  await maybeExtendedScreenshot(
    page,
    projectName,
    "notification-delivery-preview-result",
    "Notification queue preview result after operator previews queued custom pager deliveries.",
  );
}

async function exerciseServerJobsCleanup(page: Page, projectName: string) {
  await openLiveConsoleSubpage(page, "System", "Maintenance");
  await activateSystemMaintenanceSubpanel(page, "Artifact cleanup");
  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await expect(cleanupPanel).toBeVisible();
  await cleanupPanel.getByLabel("Older than days").fill("");
  await cleanupPanel.getByText("Advanced expression").click();
  await cleanupPanel
    .getByRole("textbox", { name: "Expression" })
    .fill(cleanupExpression);
  await cleanupPanel.getByRole("button", { name: "Preview" }).click();
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    /^[\s\S]*[1-9][0-9]* artifacts?/,
    { timeout: 30_000 },
  );
  await expect(
    cleanupPanel.getByLabel("Artifact cleanup readiness"),
  ).toContainText("Ready for confirmation", { timeout: 30_000 });
  await expect(
    cleanupPanel.getByLabel("Representative cleanup objects"),
  ).toContainText("file-transfer-sources/", { timeout: 30_000 });
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeEnabled({ timeout: 30_000 });
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

  await activateSystemMaintenanceSubpanel(page, "Maintenance jobs");

  const serverJobsPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Maintenance jobs" }),
  });
  await expect(serverJobsPanel).toContainText("artifact cleanup", {
    timeout: 30_000,
  });
  await expect(serverJobsPanel).toContainText("queued", { timeout: 30_000 });
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

async function forceClick(locator: Locator) {
  await expect(locator).toBeVisible({ timeout: 5000 });
  await locator.focus();
  await locator.page().keyboard.press("Enter");
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
      marker: "Fleet groups",
      screenshot: "page-fleet-groups",
    },
    {
      view: "Fleet",
      subpage: "Assignments",
      marker: "Group assignments",
      screenshot: "page-fleet-group-assignments",
    },
    {
      view: "Fleet",
      subpage: "Bulk groups",
      marker: "Bulk groups",
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
      subpage: "VPS override patch",
      marker: "VPS override patch",
      screenshot: "page-config-bulk-apply",
    },
    {
      view: "Config",
      subpage: "Per-VPS",
      marker: "Per-VPS desired config",
      screenshot: "page-config-single-vps",
    },
    {
      view: "Config",
      subpage: "Sources",
      marker: "Configuration sources",
      screenshot: "page-config-sources",
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
      marker: "File browser",
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
      marker: "Host processes",
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
      marker: "Migration mappings",
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
    await openLiveConsoleSubpage(page, entry.view, entry.subpage);
    if (entry.view === "System" && entry.subpage === "Maintenance") {
      await activateSystemMaintenanceSubpanel(page, "Artifact cleanup");
    }
    await expectMainMarker(page, entry.marker);
    if (entry.view === "Network" && entry.subpage === "Graph") {
      const graphPanel = page.locator(".topologyGraphPanel");
      await expect(graphPanel).toContainText(
        "2 of 2 plan endpoints shown; 1 of 1 tunnel shown",
      );
      await expect(graphPanel.locator(".topologyGraphNode")).toHaveCount(2);
      await expect(graphPanel.locator(".topologyFreshnessBadge")).toContainText(
        /just now|ago/,
      );
      const layerSummary = graphPanel
        .locator(".topologyGraphLegend > div")
        .filter({ hasText: "Layers" });
      // Runtime evidence arrives asynchronously after the plan sync. Before the
      // first observation the tunnel is unknown; once both agents report that
      // the declared interface is absent, it correctly needs attention.
      await expect(layerSummary).toContainText(
        /0 healthy, (?:1 unknown, 0 attention|0 unknown, 1 attention)/,
      );
      await expect(layerSummary).not.toHaveClass(/\bready\b/);
      await expect(
        graphPanel.getByText("OSPF cost", { exact: true }),
      ).toHaveCount(0);
      await expect(graphPanel.getByText("Why OSPF cost changed")).toHaveCount(
        0,
      );
    }
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
      !isRecoverableWorkspaceModuleTimeout(entry) &&
      !entry.includes("ResizeObserver loop") &&
      !entry.includes("net::ERR_NETWORK_CHANGED") &&
      !entry.includes(
        "Failed to fetch dynamically imported module: http://localhost",
      ) &&
      !entry.includes(
        "Workspace route failed to render TypeError: Failed to fetch dynamically imported module: http://localhost",
      ) &&
      !entry.includes("status of 401") &&
      !entry.includes("status of 404"),
  );
}

function recoveredWorkspaceModuleTimeouts(errors: string[]): number {
  return errors.filter((entry) =>
    entry.startsWith("Error: Workspace module load timed out after"),
  ).length;
}

function isRecoverableWorkspaceModuleTimeout(entry: string): boolean {
  return (
    entry.startsWith("Error: Workspace module load timed out after") ||
    entry.startsWith(
      "Workspace route failed to render Error: Workspace module load timed out after",
    )
  );
}
