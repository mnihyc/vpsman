import { expect, test, type Page, type Route } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type {
  CreateMonitoringShareRequest,
  MonitoringShareView,
  PublicMonitoringCardView,
  PublicMonitoringDataView,
  PublicMonitoringDetailView,
  PublicMonitoringShareView,
} from "../src/types";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

const activeShareId = "11111111-1111-4111-8111-111111111111";
const createdShareId = "44444444-4444-4444-8444-444444444444";
const publicShareId = "55555555-5555-4555-8555-555555555555";
const publicShareSecret = "public-test-secret";
const publicClientKey = "shared-edge-key";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
  await installSharedViewApiMock(page);
});

test("shared views preserve frozen scope, recoverable URL, and bulk lifecycle", async ({
  page,
}) => {
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Observability", "Shared views");

  await expect(
    page.getByRole("heading", { level: 2, name: "Shared views" }),
  ).toBeVisible();
  await expect(
    page.getByText("Customer status", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("tab", { name: /^Active · 1/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );

  const activeGridBeforeCreate = page.getByLabel(
    "Active shared views data grid",
  );
  await expect
    .poll(() =>
      activeGridBeforeCreate.evaluate(
        (grid) => grid.scrollWidth <= grid.clientWidth + 1,
      ),
    )
    .toBe(true);
  await expect(
    activeGridBeforeCreate.getByRole("button", { name: "Refresh" }),
  ).toBeVisible();
  await activeGridBeforeCreate
    .getByRole("button", { name: "Create shared view" })
    .click();
  const createDrawer = page.getByRole("complementary", {
    name: "Create shared view",
  });
  await expect(createDrawer).toBeVisible();
  await expect(
    createDrawer.getByRole("heading", { name: "Create shared view" }),
  ).toBeVisible();
  const drawerLayout = await createDrawer.evaluate((drawer) => {
    const body = drawer.querySelector<HTMLElement>(".actionDrawerBody");
    return {
      bodyOverflowY: body ? getComputedStyle(body).overflowY : null,
      drawerMaxHeight: getComputedStyle(drawer).maxHeight,
      drawerOverflowY: getComputedStyle(drawer).overflowY,
      drawerPosition: getComputedStyle(drawer).position,
    };
  });
  expect(drawerLayout).toEqual({
    bodyOverflowY: "visible",
    drawerMaxHeight: "none",
    drawerOverflowY: "visible",
    drawerPosition: "static",
  });
  await page
    .getByLabel("Shared view display name")
    .fill("Regional customer view");
  await expect(page.getByLabel("Shared view target selector")).toHaveValue("*");
  const visibilityCheckbox = (label: string) =>
    createDrawer
      .locator(".consoleField")
      .filter({ hasText: label })
      .getByRole("checkbox");
  await visibilityCheckbox("System information").check();
  for (const label of ["Resources", "Network rate", "Traffic", "Ping"]) {
    await visibilityCheckbox(label).uncheck();
  }
  await expect(visibilityCheckbox("Detail history")).toBeEnabled();
  await expect(visibilityCheckbox("Detail history")).toBeChecked();
  await expect(
    createDrawer.getByRole("button", { name: "Review creation" }),
  ).toBeEnabled();
  for (const label of ["Resources", "Network rate", "Traffic", "Ping"]) {
    await visibilityCheckbox(label).check();
  }
  await visibilityCheckbox("System information").uncheck();
  await page.getByRole("button", { name: "Review creation" }).click();

  const createConfirmation = createDrawer
    .locator(".confirmationPrompt")
    .filter({ hasText: "Confirm public monitoring view" });
  await expect(createDrawer).toBeVisible();
  await expect(createDrawer.getByLabel("Shared view display name")).toHaveValue(
    "Regional customer view",
  );
  await expect(
    createConfirmation.getByText("Confirm public monitoring view", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    createConfirmation.getByText("3", { exact: true }),
  ).toBeVisible();
  const confirmationLayout = await createConfirmation.evaluate((prompt) => {
    const form = prompt.closest("form.consoleFormGrid");
    const actions = prompt.previousElementSibling;
    const formBounds = form?.getBoundingClientRect();
    const actionsBounds = actions?.getBoundingClientRect();
    const promptBounds = prompt.getBoundingClientRect();
    return {
      followsActions:
        actions?.classList.contains("consoleFormActions") === true,
      formContainsPrompt: form?.contains(prompt) === true,
      horizontalOffset: formBounds
        ? Math.abs(promptBounds.left - formBounds.left)
        : null,
      promptWidth: promptBounds.width,
      verticalGap: actionsBounds
        ? promptBounds.top - actionsBounds.bottom
        : null,
      widthDifference: formBounds
        ? formBounds.width - promptBounds.width
        : null,
    };
  });
  expect(confirmationLayout.formContainsPrompt).toBe(true);
  expect(confirmationLayout.followsActions).toBe(true);
  expect(confirmationLayout.horizontalOffset).not.toBeNull();
  expect(confirmationLayout.horizontalOffset!).toBeLessThanOrEqual(2);
  expect(confirmationLayout.widthDifference).not.toBeNull();
  expect(Math.abs(confirmationLayout.widthDifference!)).toBeLessThanOrEqual(4);
  expect(confirmationLayout.promptWidth).toBeGreaterThan(0);
  expect(confirmationLayout.verticalGap).not.toBeNull();
  expect(confirmationLayout.verticalGap!).toBeGreaterThanOrEqual(0);
  expect(confirmationLayout.verticalGap!).toBeLessThanOrEqual(20);
  await createConfirmation
    .getByRole("button", { name: "Create shared view", exact: true })
    .click();

  const sharedViewUrl = page.getByRole("region", {
    name: "Shared view public URL",
  });
  await expect(sharedViewUrl).toBeVisible();
  await expect(sharedViewUrl.locator("pre")).toContainText(
    `#/share/${createdShareId}/public-url-secret`,
  );
  await expect(
    page
      .getByLabel("Active shared views data grid")
      .getByText("Regional customer view", { exact: true }),
  ).toBeVisible();
  const createdRow = page
    .getByLabel("Active shared views data grid")
    .locator(".gridBody [role=row], .gridMobileCard")
    .filter({ hasText: "Regional customer view" })
    .first();
  await expect(createdRow).toContainText(
    "Frozen targets match the latest server check",
  );

  await openConsoleSubpage(page, "Fleet", "Monitor");
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goBack();
    if (page.url().includes("#/observability/shared-views")) break;
  }
  await waitForConsoleShell(page);
  await expect(sharedViewUrl).toBeVisible();

  await page.reload();
  await waitForConsoleShell(page);
  await expect(sharedViewUrl).toHaveCount(0);
  await expect(
    page
      .getByLabel("Active shared views data grid")
      .getByText("Regional customer view", { exact: true }),
  ).toBeVisible();

  const grid = page.getByLabel("Active shared views data grid");
  const activeRow = grid
    .locator(".gridBody [role=row], .gridMobileCard")
    .filter({ hasText: "Customer status" })
    .first();
  await activeRow.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Copy URL", exact: true }).click();
  await expect(sharedViewUrl.locator("pre")).toContainText(
    `#/share/${activeShareId}/customer-status-secret`,
  );

  await activeRow.click({ button: "right" });
  await page
    .getByRole("menuitem", { name: "Update targets", exact: true })
    .click();
  const targetUpdatePrompt = page.getByRole("region", {
    name: "Confirm shared-view target update",
  });
  await expect(targetUpdatePrompt).toContainText("agent-fra-02");
  await targetUpdatePrompt
    .getByRole("button", { name: "Update targets", exact: true })
    .click();
  await activeRow.click();
  await expect(activeRow).toHaveAttribute("aria-expanded", "true");
  const frozenTargetCount = grid
    .locator(".gridExpandedRow .consoleInlineDetailGrid > span")
    .filter({ hasText: "Frozen VPS count" });
  await expect(frozenTargetCount.locator(":scope > span")).toHaveText("2");

  await grid
    .getByLabel(`Select Active shared views row ${activeShareId}`)
    .check();
  await grid
    .getByLabel(`Select Active shared views row ${createdShareId}`)
    .check();
  await grid.getByRole("button", { name: "Actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Extend", exact: true }).click();
  await expect(
    page.getByText("5 total frozen target references"),
  ).toBeVisible();
  await page.getByRole("button", { name: "Extend views" }).click();
  await expect(page.getByText("Extended 2 shared views.")).toBeVisible();

  await grid.getByRole("button", { name: "Actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Revoke", exact: true }).click();
  await page.getByRole("button", { name: "Revoke now" }).click();
  await expect(page.getByText("Revoked 2 shared views.")).toBeVisible();
  await page.getByRole("tab", { name: /^Revoked · 3/ }).click();
  const revokedGrid = page.getByLabel("Revoked shared views data grid");
  await expect(
    revokedGrid.getByText("Customer status", { exact: true }),
  ).toBeVisible();
  await expect(
    revokedGrid.getByText("Regional customer view", { exact: true }),
  ).toBeVisible();
});

test("shared views expose unavailable target-refresh evidence without making frozen targets actionable", async ({
  page,
}) => {
  const unavailableShareId = "66666666-6666-4666-8666-666666666666";
  const unavailableShare: MonitoringShareView = {
    ...shareFixture({
      createdAt: new Date(Date.now() - 60 * 60 * 1_000).toISOString(),
      expiresAt: new Date(Date.now() + 24 * 60 * 60 * 1_000).toISOString(),
      id: unavailableShareId,
      name: "Rule-scoped customer status",
      status: "active",
      targetClientIds: ["agent-sfo-01"],
      targetUpdateAvailable: false,
      targetUpdateEvidenceAvailable: false,
    }),
    selector_expression: "vps.rules:traffic.quota.total",
  };
  await page.route(
    /\/api\/v1\/monitoring-shares(?:\/[^?]*)?(?:\?.*)?$/,
    async (route) => {
      const request = route.request();
      if (
        request.method() === "GET" &&
        new URL(request.url()).pathname === "/api/v1/monitoring-shares"
      ) {
        await json(route, [unavailableShare]);
        return;
      }
      await route.fallback();
    },
  );

  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Observability", "Shared views");

  const grid = page.getByLabel("Active shared views data grid");
  const unavailableRow = grid
    .locator(".gridBody [role=row], .gridMobileCard")
    .filter({ hasText: "Rule-scoped customer status" })
    .first();
  await expect(unavailableRow).toContainText("Target refresh unavailable");
  await grid
    .getByLabel(`Select Active shared views row ${unavailableShareId}`)
    .check();
  await grid.getByRole("button", { name: "Actions", exact: true }).click();
  const updateTargets = page.getByRole("menuitem", {
    name: "Update targets",
    exact: true,
  });
  await expect(updateTargets).toHaveAttribute("aria-disabled", "true");
  await expect(updateTargets).toHaveAttribute(
    "title",
    /Target refresh evidence is unavailable.*frozen targets remain unchanged/i,
  );
  await page.keyboard.press("Escape");

  await unavailableRow.click();
  await expect(unavailableRow).toHaveAttribute("aria-expanded", "true");
  const targetRefreshEvidence = grid
    .locator(".gridExpandedRow .consoleInlineDetailGrid > span")
    .filter({ hasText: "Target refresh" });
  await expect(targetRefreshEvidence).toContainText(
    "Target refresh unavailable",
  );
  await expect(targetRefreshEvidence.locator(":scope > span")).toHaveAttribute(
    "title",
    /Target refresh evidence is unavailable.*frozen targets remain unchanged.*required access/i,
  );
});

test("grid share scope is limited to the shortcut visit", async ({ page }) => {
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");
  await page.getByLabel("Search VPS cards").fill("edge-sfo-01");
  await expect(page.getByText("1 matched", { exact: true })).toBeVisible();
  await page
    .locator(".fleetMonitorWorkspace")
    .getByRole("button", { name: "Shared views", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { level: 2, name: "Shared views" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Create shared view" }).click();
  await expect(page.getByLabel("Shared view target selector")).toHaveValue(
    "id:agent-sfo-01",
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();

  await openConsoleSubpage(page, "Fleet", "Monitor");
  await page.getByLabel("Search VPS cards").fill("core-fra-02");
  await expect(page.getByText("1 matched", { exact: true })).toBeVisible();
  await page
    .locator(".fleetMonitorWorkspace")
    .getByRole("button", { name: "Shared views", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { level: 2, name: "Shared views" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Fleet", "Monitor");
  await openConsoleSubpage(page, "Observability", "Shared views");
  await page.getByRole("button", { name: "Create shared view" }).click();
  await expect(page.getByLabel("Shared view target selector")).toHaveValue("*");
});

test("malformed public share hashes return to a valid console route", async ({
  page,
}) => {
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    window.location.hash = "#/share/%/%";
  });
  await expect(page).toHaveURL(/#\/home\/overview$/);
  await waitForConsoleShell(page);
});

test("unauthenticated public shares call only public monitoring APIs", async ({
  page,
}) => {
  await page.context().clearCookies();
  const apiRequests: Array<{
    authorization: string | undefined;
    method: string;
    path: string;
    shareToken: string | undefined;
  }> = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (!url.pathname.startsWith("/api/v1/")) return;
    const headers = request.headers();
    apiRequests.push({
      authorization: headers.authorization,
      method: request.method(),
      path: url.pathname,
      shareToken: headers["x-vpsman-share-token"],
    });
  });

  await installPublicMonitoringApiMock(page);
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);
  await expect(
    page.getByRole("heading", { level: 1, name: "Customer network view" }),
  ).toBeVisible();
  await expect
    .poll(() => apiRequests.some((request) => request.path.endsWith("/data")), {
      message: "public monitoring data request",
    })
    .toBe(true);

  expect(apiRequests.length).toBeGreaterThanOrEqual(2);
  for (const request of apiRequests) {
    expect(request).toMatchObject({
      authorization: undefined,
      method: "GET",
      shareToken: publicShareSecret,
    });
    expect(request.path).toMatch(
      new RegExp(
        `^/api/v1/public/monitoring-shares/${publicShareId}/(?:bootstrap|data)$`,
      ),
    );
  }
  expect(
    apiRequests.filter(
      ({ path }) =>
        path === "/api/v1/monitoring-shares" ||
        path.startsWith("/api/v1/monitoring-shares/"),
    ),
  ).toEqual([]);
  await expect(
    page.getByRole("combobox", { name: "Filter shared VPSs by status" }),
  ).not.toHaveAttribute("title", /\S/);
  const generatedTitles = await page
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? ""),
    );
  expect(
    generatedTitles.some((title) => title.includes(publicShareSecret)),
  ).toBe(false);
});

test("public cards remain static when detail history is not shared", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { detailAllowed: false });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("article", {
    name: /Shared edge · Online shared monitoring card/,
  });
  await expect(card).toBeVisible();
  await expect(card.getByText("128.0 KB/s", { exact: true })).toBeVisible();
  await expect(card.getByText("64.0 KB/s", { exact: true })).toBeVisible();
  await expect(card).not.toContainText("Mbps");
  await expect(card).not.toHaveAttribute("tabindex");
  await expect(
    page.getByRole("link", {
      name: /Shared edge · Online shared monitoring card/,
    }),
  ).toHaveCount(0);
  await card.click();
  await expect(page).toHaveURL(
    new RegExp(`#/share/${publicShareId}/${publicShareSecret}$`),
  );
});

test("compact public cards preserve long Ping targets and evidence at minimum width", async ({
  page,
}) => {
  const pingTargetName =
    "Primary customer gateway with a deliberately long operator label";
  await page.setViewportSize({ width: 320, height: 900 });
  await installPublicMonitoringApiMock(page, {
    pingLatencyMs: 123_456_789.1,
    pingLossRatio: null,
    pingTargetName,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", { name: /Shared edge/ });
  const ping = card.locator(".publicMonitoringPing");
  const heading = ping.locator(".vpsMonitorRowHeading");
  const evidence = ping.locator(".vpsMonitorPingEvidence > strong");
  await expect(heading).toHaveText(`Ping · ${pingTargetName}`);
  await expect(evidence).toHaveText(["123456789.1 ms", "loss unavailable"]);
  await expect
    .poll(() =>
      ping.evaluate((element) => {
        const headingElement = element.querySelector<HTMLElement>(
          ".vpsMonitorRowHeading",
        );
        const chartElement = element.querySelector<HTMLElement>(
          ":scope > .vpsMonitorSparkline",
        );
        const evidenceElement = element.querySelector<HTMLElement>(
          ".vpsMonitorPingEvidence",
        );
        const evidenceItems = Array.from(
          element.querySelectorAll<HTMLElement>(
            ".vpsMonitorPingEvidence > strong",
          ),
        );
        if (!headingElement || !chartElement || !evidenceElement) return false;
        const headingStyle = getComputedStyle(headingElement);
        const headingBox = headingElement.getBoundingClientRect();
        const chartBox = chartElement.getBoundingClientRect();
        const evidenceBox = evidenceElement.getBoundingClientRect();
        const lastEvidenceBox = evidenceItems.at(-1)?.getBoundingClientRect();
        return (
          headingStyle.overflow !== "hidden" &&
          headingStyle.textOverflow !== "ellipsis" &&
          headingElement.scrollWidth <= headingElement.clientWidth + 1 &&
          headingElement.scrollHeight <= headingElement.clientHeight + 1 &&
          evidenceItems.every((item) => {
            const style = getComputedStyle(item);
            return (
              style.overflow !== "hidden" &&
              style.textOverflow !== "ellipsis" &&
              item.scrollWidth <= item.clientWidth + 1 &&
              item.scrollHeight <= item.clientHeight + 1
            );
          }) &&
          getComputedStyle(evidenceElement).justifyContent === "flex-end" &&
          Math.abs((lastEvidenceBox?.right ?? 0) - evidenceBox.right) <= 1 &&
          chartBox.top >= headingBox.bottom - 1 &&
          evidenceBox.top >= chartBox.top - 1 &&
          evidenceBox.top - chartBox.bottom >= -5 &&
          evidenceBox.top - chartBox.bottom <= -1 &&
          element.getBoundingClientRect().right <= window.innerWidth + 1
        );
      }),
    )
    .toBe(true);
});

test("comfortable shared cards align configured Ping and collapse unconfigured evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the focused geometry assertion is viewport-independent and runs once on desktop",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await installPublicMonitoringApiMock(page, {
    cardCount: 3,
    pingStateCoverage: true,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);
  await page
    .getByLabel("Shared view density")
    .getByRole("button", { name: "Comfortable" })
    .click();

  const healthyCard = page.getByRole("link", {
    name: /Shared edge .* shared monitoring card/,
  });
  const healthy = healthyCard.locator(".publicMonitoringPing");
  const degraded = page
    .getByRole("link", { name: /Frankfurt build .* shared monitoring card/ })
    .locator(".publicMonitoringPing");
  const unconfigured = page
    .getByRole("link", { name: /Tokyo relay .* shared monitoring card/ })
    .locator(".publicMonitoringPing");

  await expect(healthy.locator(".vpsMonitorRowHeading")).toHaveText(
    "Ping · Customer gateway",
  );
  await expect(healthy.locator(":scope > .vpsMonitorPingDetail")).toHaveCount(
    0,
  );
  await expect(degraded.locator(":scope > .vpsMonitorPingDetail")).toHaveText(
    "Primary Ping degraded",
  );
  await expect(degraded).toHaveAttribute(
    "title",
    /Fixture degraded gateway.*24\.5 ms.*20\.0% loss.*Primary Ping degraded/i,
  );
  await expect(
    unconfigured.locator(
      ".publicMonitoringPingHeading > .vpsMonitorPingEvidence > strong",
    ),
  ).toHaveText("Unconfigured");
  await expect(
    unconfigured.locator(":scope > .vpsMonitorPingDetail"),
  ).toHaveCount(0);
  await expectPublicPingShorter(healthy, degraded);
  await expectPublicPingShorter(unconfigured, healthy);
  await expect
    .poll(() =>
      healthyCard
        .locator(".vpsMonitorTrafficEvidenceRow")
        .evaluate((element) => {
          const observed = element.querySelector<HTMLElement>(":scope > small");
          const quota = element.querySelector<HTMLElement>(
            ":scope > .vpsMonitorTrafficQuota",
          );
          if (!observed || !quota) return false;
          const observedBox = observed.getBoundingClientRect();
          const quotaBox = quota.getBoundingClientRect();
          return (
            observed.scrollWidth <= observed.clientWidth + 1 &&
            quota.scrollWidth <= quota.clientWidth + 1 &&
            Math.abs(
              observedBox.top +
                observedBox.height / 2 -
                (quotaBox.top + quotaBox.height / 2),
            ) <= 2 &&
            element.scrollWidth <= element.clientWidth + 1
          );
        }),
    )
    .toBe(true);

  await page
    .getByLabel("Shared view density")
    .getByRole("button", { name: "Compact" })
    .click();
  await expect(
    page.getByLabel("Shared VPS cards").locator(".vpsMonitorPingDetail"),
  ).toHaveCount(0);
});

test("public cards keep an intentionally empty network-rate selection neutral", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { networkRateExpected: false });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  const trafficQuota = card.locator(
    ".publicMonitoringTraffic .vpsMonitorTrafficQuota",
  );
  await expect(trafficQuota).toContainText("· Total · 25.0%");
  await expect(trafficQuota).toHaveCSS("font-weight", "700");
  await expect(trafficQuota.locator(".vpsMonitorTrafficDirection")).toHaveCSS(
    "font-weight",
    "400",
  );
  await expect(card).toBeVisible();
  await expect(card.getByText("Online", { exact: true })).toBeVisible();
  await expect(card.getByText("Online · Warning", { exact: true })).toHaveCount(
    0,
  );
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();
  await expect(card.getByText("Network rates not selected")).toBeVisible();
  await expect(card.getByText("Needs attention")).toHaveCount(0);
  await expect(
    card.getByLabel("Current network rate for Shared edge").getByText("-"),
  ).toHaveCount(2);
  const realtimeSpeed = page
    .getByText("Realtime speed", { exact: true })
    .locator("..");
  await expect(realtimeSpeed).toContainText("↓ -");
  await expect(realtimeSpeed).toContainText("↑ -");

  await card.click();
  await expect(
    page
      .getByRole("region", { name: "Network RX / TX chart" })
      .locator(".dashboardWidgetHeader > small"),
  ).toHaveText("↓ - · ↑ -");
});

test("public monitoring reuses the Unicode country flag renderer", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { identityContext: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  await expect(card.locator(".countryFlagGlyph")).toHaveText("🇺🇸");
  await expect(card.locator(".vpsMonitorCompactProductContext")).toHaveCount(0);
  await expect(card.locator(".vpsMonitorCardName")).toHaveAttribute(
    "title",
    "Shared edge · Northwind · Storage-Box 4 · Provider Transit · US · Virginia",
  );
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();
  await expect(card.locator(".countryFlagGlyph")).toHaveCount(0);
  await expect(card.locator(".vpsMonitorCardMain > small")).toHaveText(
    "Updated just now",
  );
  await expect(card.getByLabel("Shared identity context")).toHaveText(
    "Northwind · Storage-Box 4 · Transit · US",
  );
  await expect(card.getByLabel("Shared identity context")).toHaveAttribute(
    "title",
    "Provider Northwind, Transit · Product Storage-Box 4 · US · Virginia",
  );
  await expect(card.getByLabel("Shared identity context")).not.toContainText(
    "Virginia",
  );
  await card.click();
  await expect(
    page
      .getByRole("region", { name: "Read-only history for Shared edge" })
      .locator(".publicMonitoringDetailHeader p"),
  ).toContainText("US · Virginia");
});

test("public monitoring keeps grid and detail history state without exposing hidden resource evidence", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page);
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  await expect(
    page.getByRole("heading", { level: 1, name: "Customer network view" }),
  ).toBeVisible();
  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  await expect(card).toBeVisible();
  await expect(card.getByText("Updated just now").first()).not.toHaveAttribute(
    "title",
    /\S/,
  );
  await expect(
    card.getByLabel("Current resources for Shared edge"),
  ).toHaveCount(0);
  await expect(page.getByText(/resource telemetry unavailable/i)).toHaveCount(
    0,
  );
  await expect(page.getByText("Visible telemetry unavailable")).toHaveCount(0);
  await expect(card.getByText("Traffic", { exact: true })).toBeVisible();
  const publicTraffic = card.locator(".publicMonitoringTraffic");
  await expect(publicTraffic.locator(".vpsMonitorRowHeading")).toContainText(
    /Traffic · Resets/,
  );
  await expect(
    publicTraffic.locator(".vpsMonitorRowHeading"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(
    publicTraffic.locator(".vpsMonitorTrafficQuota"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(publicTraffic.locator(":scope > small")).toHaveCount(0);
  const publicPing = card.locator(".publicMonitoringPing");
  await expect(publicPing.locator(".vpsMonitorRowHeading")).toContainText(
    "Ping · Customer gateway",
  );
  await expect(
    publicPing.locator(":scope > .vpsMonitorPingEvidence"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(
    publicPing.locator(":scope > small:not(.vpsMonitorRowHeading)"),
  ).toHaveCount(0);
  await expect(card.getByText("Ok", { exact: true })).toHaveCount(0);
  await expect(card.getByText("Billing", { exact: true })).toHaveCount(0);
  await expect(card.getByText("Uptime", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Filter shared VPSs by tag")).toHaveCount(0);
  await expect(page.getByLabel("Filter shared VPSs by provider")).toHaveCount(
    0,
  );

  const cardGrid = page.getByLabel("Shared VPS cards");
  const columnCount = () =>
    cardGrid.evaluate(
      (node) =>
        getComputedStyle(node).gridTemplateColumns.split(/\s+/).filter(Boolean)
          .length,
    );
  await expect(cardGrid).toHaveAttribute("data-density", "compact");
  const compactHeight = await card.evaluate(
    (node) => node.getBoundingClientRect().height,
  );
  const isMobile = (page.viewportSize()?.width ?? 1_000) < 500;
  expect(await columnCount()).toBe(isMobile ? 1 : 5);
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();
  await expect(
    publicTraffic.locator(".vpsMonitorTrafficEvidenceRow > small"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(
    publicPing.locator(":scope > small:not(.vpsMonitorRowHeading)"),
  ).toHaveCount(0);
  const comfortableHeight = await card.evaluate(
    (node) => node.getBoundingClientRect().height,
  );
  expect(await columnCount()).toBe(isMobile ? 1 : 3);
  expect(compactHeight).toBeLessThan(comfortableHeight);
  await page.getByRole("button", { name: "Compact", exact: true }).click();
  await expect(cardGrid).toHaveAttribute("data-density", "compact");

  await page.getByLabel("Search shared VPSs").fill("edge");
  if (isMobile) {
    await expect(page.getByLabel("Shared VPS cards")).toHaveCSS(
      "grid-template-columns",
      /\d+(?:\.\d+)?px/,
    );
    const fitsViewport = await card.evaluate(
      (node) => node.getBoundingClientRect().right <= window.innerWidth + 1,
    );
    expect(fitsViewport).toBe(true);
  }

  await card.click();
  await expect(page).toHaveURL(
    new RegExp(
      `#/share/${publicShareId}/${publicShareSecret}/vps/${publicClientKey}$`,
    ),
  );
  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  await expect(detail).toBeVisible();
  await expect(detail.locator(".publicMonitoringDetailHeader")).toContainText(
    "Online · Updated just now · Read-only history",
  );
  await expect(detail.getByLabel("Current shared VPS evidence")).toContainText(
    "Customer gateway",
  );
  await expect(detail.getByText("System", { exact: true })).toHaveCount(0);
  await expect(detail.getByText("Billing", { exact: true })).toHaveCount(0);
  await detail
    .getByRole("button", {
      name: /Ping Targets · latency · loss/,
    })
    .click();
  await expect(detail.getByText("Reachable", { exact: true })).toBeVisible();
  await expect(detail.getByText("Ok", { exact: true })).toHaveCount(0);
  const pingTarget = detail.getByRole("button", {
    name: "Hide Customer gateway Ping history",
  });
  await expect(pingTarget).toHaveAttribute("aria-pressed", "true");
  await pingTarget.click();
  await expect(
    detail.getByRole("button", {
      name: "Show Customer gateway Ping history",
    }),
  ).toHaveAttribute("aria-pressed", "false");
  await detail.getByRole("button", { name: "Select all" }).click();
  await detail
    .getByRole("button", {
      name: /Resources .*network · traffic/i,
    })
    .click();

  const trafficChart = detail.getByRole("region", {
    name: "Traffic volume chart",
  });
  const totalSeries = trafficChart.getByRole("button", {
    name: "Show Total volume series",
  });
  await expect(totalSeries).toHaveAttribute("aria-pressed", "false");
  await expect(
    trafficChart.getByRole("button", { name: "Hide RX volume series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    trafficChart.getByRole("button", { name: "Hide TX volume series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(trafficChart.getByText("2/3 series")).toBeVisible();
  await totalSeries.click();
  await expect(
    trafficChart.getByRole("button", { name: "Hide Total volume series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await detail
    .getByRole("button", {
      name: /Ping Targets · latency · loss/,
    })
    .click();
  await detail
    .getByRole("button", { name: "Hide Customer gateway Ping history" })
    .click();

  await page.goBack();
  await expect(page).toHaveURL(
    new RegExp(`#/share/${publicShareId}/${publicShareSecret}$`),
  );
  await expect(page.getByLabel("Search shared VPSs")).toHaveValue("edge");
  await expect(card).toBeVisible();

  await page.goForward();
  await expect(page).toHaveURL(
    new RegExp(
      `#/share/${publicShareId}/${publicShareSecret}/vps/${publicClientKey}$`,
    ),
  );
  await expect(detail).toBeVisible();
  await expect(detail.locator(".publicMonitoringDetailHeader")).toContainText(
    "Online · Updated just now · Read-only history",
  );
  await expect(
    detail.getByRole("button", {
      name: /Ping Targets · latency · loss/,
    }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    detail.getByRole("button", {
      name: /Show Customer gateway Ping history/,
    }),
  ).toHaveAttribute("aria-pressed", "false");
});

test("public monitoring reports mixed retained source resolutions without exposing hidden domains", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { tieredHistory: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);
  await page
    .getByRole("link", { name: /Shared edge · Online shared monitoring card/ })
    .click();

  const header = page
    .getByRole("region", { name: "Read-only history for Shared edge" })
    .locator(".publicMonitoringDetailHeader");
  await expect(header).toContainText("retained tiered history");
  await expect(header).toContainText(
    "network/Ping 5m; traffic 1m coarsest source resolutions",
  );
  await expect(header).not.toContainText("resources");
});

test("public traffic-only old custom history uses only its visible source resolution", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { trafficOnlyOldCustom: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);
  await page
    .getByRole("link", { name: /Shared edge · Online shared monitoring card/ })
    .click();

  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  const header = detail.locator(".publicMonitoringDetailHeader");
  await expect(header).toContainText(
    "retained tiered history · 1m coarsest source resolution",
  );
  await expect(header).not.toContainText("30m");
  await expect(detail.locator(".observabilitySparseNotice")).toHaveCount(0);
  await expect(
    detail.getByRole("heading", { name: "Traffic volume" }),
  ).toBeVisible();
});

test("public monitoring presents warnings, disabled Ping, unlimited quotas, resources, and narrow detail without ambiguity", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { edgeCases: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online · Warning shared monitoring card/,
  });
  await expect(card).toBeVisible();
  await expect(
    card.getByText("Online · Warning", { exact: true }),
  ).toBeVisible();
  await expect(card.getByText(/Last sample 18\.5 ms/)).toBeVisible();
  const pingDiagnostic = card.locator(
    ".publicMonitoringPing > small:not(.vpsMonitorRowHeading)",
  );
  await expect(pingDiagnostic).toHaveCount(0);
  await expect(card.locator(".publicMonitoringWarnings")).toHaveCount(0);
  await expect(
    card.getByLabel("Current resources for Shared edge"),
  ).toBeVisible();
  await expect(card.locator(".vpsMonitorMetric > strong small")).toHaveText([
    "(4-core)",
    "(4.0 GB)",
    "(50 GB)",
  ]);
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();
  await expect(pingDiagnostic).toHaveText("Primary Ping disabled");
  await expect(pingDiagnostic).toBeVisible();
  await expect(card.locator(".publicMonitoringWarnings")).toHaveCount(1);
  const resourceMetrics = card.locator(".vpsMonitorMetric");
  for (const index of [0, 1, 2]) {
    await expect(
      resourceMetrics.nth(index).locator(":scope > small"),
    ).toHaveCount(0);
  }
  await expect(resourceMetrics.nth(3).locator(":scope > small")).toContainText(
    "5m",
  );
  await expect(card).not.toContainText("maximum reported capacity");
  await expect(card).not.toContainText("reported cores");
  await expect(card.getByText("TCP", { exact: true })).toBeVisible();
  await expect(card.getByText("Unlimited", { exact: false })).toBeVisible();
  const cardBilling = card.locator('[data-fact-kind="billing"]');
  await expect(cardBilling).toContainText("35.00 ¥/m");
  await expect(cardBilling).toContainText("Renews day 15");
  await expect(cardBilling.locator("em")).toHaveCount(0);
  await expect(
    card.locator(".publicMonitoringTraffic").getByRole("meter"),
  ).toHaveCount(0);
  await expect(
    card
      .locator(".publicMonitoringTraffic")
      .getByLabel("Traffic quota is unlimited"),
  ).toBeVisible();
  await expect(
    card.locator(".publicMonitoringTraffic .unlimitedTrafficTrack > span"),
  ).toHaveCSS("background-image", /repeating-linear-gradient/);
  const cardUptime = card.locator('[data-fact-kind="uptime"]');
  await expect(cardUptime).toContainText("8d");
  await expect(card).not.toContainText("/ 0 B");
  await expect(card).not.toContainText(/No (recent|continuous) history/);
  await expect(
    card.getByLabel("Primary Ping Customer gateway: Disabled"),
  ).toBeVisible();

  await card.press("Enter");
  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  await expect(detail).toBeVisible();
  await expect(detail.getByText("Debian GNU/Linux 13")).toBeVisible();
  await expect(detail.getByText("AMD EPYC 7B13")).toBeVisible();
  await expect(detail.getByText("6.12.38-amd64")).toBeVisible();
  await expect(detail.getByText("KVM", { exact: true })).toBeVisible();
  const detailBilling = detail
    .getByLabel("Current shared VPS evidence")
    .locator('[data-fact-kind="billing"]');
  await expect(detailBilling).toContainText("Billing · Renews day 15");
  await expect(detailBilling).toContainText("35.00 ¥/m");
  await expect(detail.getByText("Swap", { exact: true })).toBeVisible();
  await expect(detail.getByText("None", { exact: true })).toBeVisible();
  await expect(
    detail.getByText("RAM", { exact: true }).locator(".."),
  ).toHaveAttribute(
    "title",
    "Average used-memory percentage and maximum reported RAM capacity",
  );
  await expect(
    detail.getByText("Reported filesystems", { exact: true }).locator(".."),
  ).toHaveAttribute(
    "title",
    "Average used-disk percentage and maximum aggregate filesystem capacity",
  );
  await expect(detail.locator(".publicMonitoringDetailHeader")).toContainText(
    "Online · Warning · Updated just now · Read-only history",
  );
  await expect(
    detail
      .getByLabel("Current shared VPS evidence")
      .locator(":scope > span")
      .filter({ hasText: "Primary Ping" }),
  ).toContainText("Customer gateway · Disabled");
  await detail
    .getByRole("button", {
      name: /Ping Targets · latency · loss/,
    })
    .click();
  await expect(detail.getByText("Disabled", { exact: true })).toBeVisible();
  await expect(
    detail.getByRole("button", {
      name: "Hide Customer gateway Ping history",
    }),
  ).toContainText("Last sample: 18.5 ms");
  await detail
    .getByRole("button", { name: "Hide Regional transit Ping history" })
    .click();
  const restoreRegionalSeries = detail.getByRole("button", {
    name: "Show Regional transit series",
  });
  await expect(restoreRegionalSeries).toHaveAttribute("aria-pressed", "false");
  await restoreRegionalSeries.click();
  await expect(
    detail.getByRole("button", {
      name: "Hide Regional transit Ping history",
    }),
  ).toHaveAttribute("aria-pressed", "true");
  await detail.getByRole("button", { name: "Select none" }).click();
  await expect(
    detail.getByText("Select at least one Ping target to display its history"),
  ).toBeVisible();
  await detail.getByRole("button", { name: "Select all" }).click();
  await expect(
    detail.getByRole("group", {
      name: /Ping latency shared monitoring chart/,
    }),
  ).toBeVisible();
  await detail
    .getByRole("button", {
      name: /Resources .*network · traffic/i,
    })
    .click();
  await expect(
    detail.getByText("Unlimited", { exact: true }).first(),
  ).toBeVisible();
  await expect(
    detail
      .locator(".publicMonitoringTrafficCycle")
      .getByLabel("Traffic quota is unlimited"),
  ).toBeVisible();
  await expect(detail).not.toContainText("/ 0 B");
  await expect(detail).not.toContainText(
    "Traffic is accounted for, but no quota is configured.",
  );
  const connectionsChart = detail.getByRole("region", {
    name: "TCP / UDP connections chart",
  });
  await expect(
    connectionsChart.getByRole("button", { name: "Hide TCP series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    connectionsChart.getByRole("button", { name: "Hide UDP series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(connectionsChart.getByText("2/2 series")).toBeVisible();
  const memoryChart = detail.getByRole("region", {
    name: "Memory / swap used chart",
  });
  await expect(
    memoryChart.locator(".dashboardWidgetHeader > small"),
  ).toHaveText("Max · RAM 8.0 GB · Swap 2.0 GB");
  await expect(
    memoryChart.getByRole("button", { name: "Hide Swap used series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    memoryChart.locator("table.srOnly tbody tr").last().locator("td").nth(1),
  ).toHaveText("-");
  const diskChart = detail.getByRole("region", {
    name: "Aggregate reported disk used chart",
  });
  await expect(diskChart.locator(".dashboardWidgetHeader > small")).toHaveText(
    "100 GB maximum",
  );

  const range = detail.getByRole("group", { name: "History range" });
  await expect(detail.locator(".publicMonitoringHistoryControls")).toHaveCSS(
    "border-top-width",
    "2px",
  );
  await expect(detail.locator(".publicMonitoringHistoryControls")).toHaveCSS(
    "margin-top",
    "24px",
  );
  await expect(detail.locator(".publicMonitoringHistoryControls")).toHaveCSS(
    "padding-top",
    "20px",
  );
  await expect(range.getByRole("button")).toHaveText([
    "15m",
    "1h",
    "8h",
    "1d",
    "7d",
    "30d",
    "90d",
    "180d",
    "1y",
    "All",
    "Custom",
  ]);
  await expect(
    range.getByRole("button", {
      name: "Realtime, last 15 minutes",
      exact: true,
    }),
  ).toHaveText("15m");

  await page.setViewportSize({ height: 844, width: 320 });
  await expect
    .poll(() =>
      page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth + 1,
      ),
    )
    .toBe(true);
});

test("public monitoring does not present an unconfigured traffic cycle as authoritative", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { trafficConfigured: false });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online(?: · Warning)? shared monitoring card/,
  });
  const traffic = card.locator(".publicMonitoringTraffic.unconfigured");
  await expect(traffic.getByText("Traffic", { exact: true })).toBeVisible();
  await expect(
    traffic.getByText("Unconfigured", { exact: true }),
  ).toBeVisible();
  await expect(traffic.locator(".vpsMonitorMetricTrack.missing")).toBeVisible();
  await expect(traffic).toHaveCSS("background-color", /rgb/);
  await expect(card.getByText("Online", { exact: true })).toBeVisible();
  await expect(card.getByText("Online · Warning", { exact: true })).toHaveCount(
    0,
  );
  await expect(card.locator(".publicMonitoringWarnings")).toHaveCount(0);
  await card.click();

  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  const currentEvidence = detail.getByLabel("Current shared VPS evidence");
  await expect(currentEvidence).not.toContainText("Traffic");
  await expect(detail).not.toContainText("Traffic cycle");
  await expect(
    detail.getByRole("region", { name: /Traffic .* chart/ }),
  ).toHaveCount(0);
  await expect(detail).not.toContainText("2.9 KB");
  await expect(detail).not.toContainText("Quota");
});

test("public monitoring labels retained traffic after accounting is unconfigured", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, {
    retainTrafficHistory: true,
    trafficConfigured: false,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);
  await page
    .getByRole("link", {
      name: /Shared edge · Online shared monitoring card/,
    })
    .click();

  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  const history = detail.getByRole("region", {
    name: "Prior traffic accounting history chart",
  });
  await expect(history).toContainText("Current accounting unconfigured");
  await expect(detail).toContainText(
    "Retained volume predates the current unconfigured accounting state",
  );
  await expect(
    detail.getByLabel("Current shared VPS evidence"),
  ).not.toContainText("Traffic");
  await expect(detail).not.toContainText("Traffic cycle");
});

test("public monitoring identifies the most-used finite directional traffic quota", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { mixedTrafficQuotas: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online · Warning shared monitoring card/,
  });
  await expect(
    card.locator(".publicMonitoringTraffic .vpsMonitorTrafficQuota"),
  ).toHaveText("1.0 KB / 800 B · TX · 125%");
  await expect(
    card.locator(".publicMonitoringTraffic .vpsMonitorTrafficQuota"),
  ).not.toContainText(/observed/i);
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();
  await expect(
    card.locator(".publicMonitoringTraffic .vpsMonitorTrafficEvidenceRow"),
  ).toContainText("RX 2.0 KB · TX 1.0 KB");
  await card.click();

  const cycle = page.locator(".publicMonitoringTrafficCycle");
  await expect(cycle.getByText("TX 800 B", { exact: true })).toBeVisible();
  await expect(
    cycle.getByText("Observed RX", { exact: true }).locator(".."),
  ).toContainText("2.0 KB");
  await expect(
    cycle.getByText("Observed TX", { exact: true }).locator(".."),
  ).toContainText("1.0 KB");
  await expect(
    cycle.getByText("Counted total", { exact: true }).locator(".."),
  ).toContainText("1.0 KB");
  await expect(cycle).not.toContainText("Unlimited");
});

test("public monitoring uses warning color above ninety percent without changing status", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { nearQuota: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  const traffic = card.locator(".publicMonitoringTraffic");
  const track = traffic.locator(".vpsMonitorMetricTrack");
  await expect(traffic).toContainText("95.0%");
  await expect(traffic.locator(".exceptionEvidence")).toHaveCount(0);
  await expect(track).toHaveClass(/warning/);
  await expect(track.locator(":scope > span")).toHaveCSS(
    "background-color",
    "rgb(249, 171, 0)",
  );
});

test("public monitoring uses counted directional traffic for an unlimited summary", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { directionalUnlimited: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  const traffic = card.locator(".publicMonitoringTraffic");
  await expect(traffic.locator(".vpsMonitorTrafficQuota")).toHaveText(
    "1.0 KB / Unlimited · TX",
  );
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();
  await expect(traffic.locator(".vpsMonitorTrafficEvidenceRow")).toContainText(
    "RX 2.0 KB · TX 1.0 KB",
  );

  await card.click();
  const cycle = page.locator(".publicMonitoringTrafficCycle");
  await expect(
    cycle.getByText("Observed RX", { exact: true }).locator(".."),
  ).toContainText("2.0 KB");
  await expect(
    cycle.getByText("Observed TX", { exact: true }).locator(".."),
  ).toContainText("1.0 KB");
  await expect(
    cycle.getByText("Counted total", { exact: true }).locator(".."),
  ).toContainText("1.0 KB");
});

test("public monitoring presents no-reset accumulation without a synthetic cycle", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { noResetTraffic: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  const traffic = card.locator(".publicMonitoringTraffic");
  await expect(traffic.locator(".vpsMonitorRowHeading")).toHaveText("Traffic");
  await expect(traffic.locator(".vpsMonitorTrafficQuota")).toHaveText(
    "3.0 KB / 12 KB · Total · 25.0%",
  );

  await card.click();
  const cycle = page.locator(".publicMonitoringTrafficCycle");
  await expect(cycle.getByText("Traffic", { exact: true })).toBeVisible();
  await expect(cycle).toContainText("Accumulated total");
  await expect(cycle).not.toContainText("No reset");
  await expect(cycle).not.toContainText("Current accounting cycle");
});

test("public monitoring shows a canonical MM-DD renewal anchor", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, {
    annualBilling: true,
    supplementalVisibility: true,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  const billing = card.locator('[data-fact-kind="billing"]');
  await expect(billing).toContainText("120.00 USD/y");
  await expect(billing).toContainText("Renews 06-15");
});

test("public monitoring keeps enabled billing and system facts visible when evidence is missing", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, {
    supplementalVisibility: true,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  await expect(card).toBeVisible();
  await expect(card.locator('[data-fact-kind="billing"]')).toContainText(
    "Billing-",
  );
  await expect(card.locator('[data-fact-kind="uptime"]')).toContainText(
    "Uptime-",
  );
  await card.click();

  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  await expect(detail).toBeVisible();
  const currentEvidence = detail.getByLabel("Current shared VPS evidence");
  await expect(
    currentEvidence.locator('[data-fact-kind="billing"]'),
  ).toContainText("Billing-");
  await expect(
    currentEvidence.locator('[data-fact-kind="uptime"]'),
  ).toContainText("Uptime-");
  const information = detail.getByLabel("Shared VPS system information");
  await expect(
    information.getByRole("heading", { name: "Hardware", exact: true }),
  ).toHaveCount(0);
  await expect(
    information.getByRole("heading", { name: "System", exact: true }),
  ).toHaveCount(0);
  await expect(
    information.getByRole("heading", { name: "Storage", exact: true }),
  ).toHaveCount(0);
});

test("public monitoring mirrors fleet tag, provider, and sort controls when identity is shared", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, {
    cardCount: 8,
    edgeCases: true,
    identityContext: true,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const grid = page.getByLabel("Shared VPS cards");
  await page.getByLabel("Search shared VPSs").fill("Storage-Box 4");
  await expect(grid.getByRole("link")).toHaveCount(1);
  await page.getByLabel("Search shared VPSs").fill("");
  await page
    .getByLabel("Filter shared VPSs by provider")
    .selectOption("Hetzner");
  await expect(grid.getByRole("link")).toHaveCount(2);
  await page.getByLabel("Filter shared VPSs by provider").selectOption("all");
  await page.getByLabel("Filter shared VPSs by tag").selectOption("country:JP");
  await expect(grid.getByRole("link")).toHaveCount(1);
  await expect(grid.getByRole("link")).toHaveAccessibleName(
    /Tokyo relay · Online shared monitoring card/,
  );
  await page.getByLabel("Filter shared VPSs by tag").selectOption("all");
  await page.getByLabel("Shared VPS sort").selectOption("cpu");
  await expect(grid.getByRole("link").first()).toHaveAccessibleName(
    /Toronto transit · Online shared monitoring card/,
  );
});

test("public monitoring grid and detail have complete screenshot coverage", async ({
  page,
}, testInfo) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installPublicMonitoringApiMock(page, {
    cardCount: 8,
    edgeCases: true,
    identityContext: true,
  });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online · Warning shared monitoring card/,
  });
  await expect(card).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth + 1,
      ),
    )
    .toBe(true);

  const screenshotDir = join(
    process.env.VPSMAN_SCREENSHOT_DIR ??
      join(process.cwd(), "..", "output", "playwright", "public-monitoring"),
    testInfo.project.name,
  );
  mkdirSync(screenshotDir, { recursive: true });
  const entries: Array<Record<string, string>> = [];
  const capture = async (id: string) => {
    await expect
      .poll(() =>
        page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth + 1,
        ),
      )
      .toBe(true);
    const screenshot = join(
      screenshotDir,
      `${id}-${testInfo.project.name}.png`,
    );
    const fullScreenshot = join(
      screenshotDir,
      `${id}-${testInfo.project.name}-full.png`,
    );
    await page.screenshot({ path: screenshot });
    await page.screenshot({ fullPage: true, path: fullScreenshot });
    entries.push({ fullScreenshot, id, screenshot });
  };

  await expect(page.getByLabel("Shared VPS cards")).toHaveAttribute(
    "data-density",
    "compact",
  );
  await capture("42d-public-monitoring-grid-compact");
  await page
    .getByRole("group", { name: "Shared view density" })
    .getByRole("button", { name: "Comfortable" })
    .click();
  await expect(page.getByLabel("Shared VPS cards")).toHaveAttribute(
    "data-density",
    "comfortable",
  );
  await capture("42da-public-monitoring-grid-comfortable");
  await page
    .getByRole("group", { name: "Shared view density" })
    .getByRole("button", { name: "Compact" })
    .click();

  await card.click();
  await expect(
    page.getByRole("group", {
      name: /CPU utilization shared monitoring chart/,
    }),
  ).toBeVisible();
  await capture("42e-public-monitoring-detail-resources");
  await page
    .getByRole("button", {
      name: /Ping Targets · latency · loss/,
    })
    .click();
  await expect(
    page.getByRole("group", {
      name: /Ping latency shared monitoring chart/,
    }),
  ).toBeVisible();
  await capture("42ea-public-monitoring-detail-ping");
  await page
    .getByRole("button", { name: "Hide Customer gateway Ping history" })
    .click();
  await expect(
    page.getByRole("button", {
      name: /Show Customer gateway Ping history/,
    }),
  ).toHaveAttribute("aria-pressed", "false");
  await capture("42eb-public-monitoring-detail-ping-target-hidden");

  for (const state of ["expired", "revoked", "invalid"] as const) {
    const unavailableShareId = `${state}-share`;
    const isInactive = state !== "invalid";
    await page.route(
      new RegExp(
        `/api/v1/public/monitoring-shares/${unavailableShareId}/bootstrap$`,
      ),
      async (route) => {
        await route.fulfill({
          body: JSON.stringify({
            error: isInactive
              ? "monitoring_share_unavailable"
              : "monitoring_share_not_found",
          }),
          contentType: "application/json",
          status: isInactive ? 410 : 404,
        });
      },
    );
    await page.goto(`/#/share/${unavailableShareId}/${state}-secret`);
    await expect(
      page.getByRole("alert").getByText("Shared view unavailable"),
    ).toBeVisible();
    await expect(page.getByRole("alert")).toContainText(
      "This shared view link is invalid, expired, or revoked.",
    );
    await capture(`42f-public-monitoring-${state}`);
  }
  writeFileSync(
    join(screenshotDir, "manifest-public.json"),
    `${JSON.stringify({ generated_by: "public-monitoring-screenshots", views: entries }, null, 2)}\n`,
  );
});

async function installSharedViewApiMock(page: Page) {
  const now = Date.now();
  let shares: MonitoringShareView[] = [
    shareFixture({
      createdAt: new Date(now - 2 * 60 * 60 * 1_000).toISOString(),
      expiresAt: new Date(now + 24 * 60 * 60 * 1_000).toISOString(),
      id: activeShareId,
      lastVisitedAt: new Date(now - 30 * 60 * 1_000).toISOString(),
      name: "Customer status",
      status: "active",
      targetClientIds: ["agent-sfo-01"],
      targetUpdateAvailable: true,
      visitorCount: 2,
    }),
    shareFixture({
      createdAt: new Date(now - 3 * 24 * 60 * 60 * 1_000).toISOString(),
      expiresAt: new Date(now - 2 * 24 * 60 * 60 * 1_000).toISOString(),
      id: "22222222-2222-4222-8222-222222222222",
      name: "Expired handoff",
      status: "expired",
    }),
    {
      ...shareFixture({
        createdAt: new Date(now - 4 * 24 * 60 * 60 * 1_000).toISOString(),
        expiresAt: new Date(now + 24 * 60 * 60 * 1_000).toISOString(),
        id: "33333333-3333-4333-8333-333333333333",
        name: "Revoked wall",
        status: "revoked",
      }),
      revoked_at: new Date(now - 24 * 60 * 60 * 1_000).toISOString(),
    },
  ];

  await page.route(
    /\/api\/v1\/monitoring-shares(?:\/[^?]*)?(?:\?.*)?$/,
    async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const method = request.method();
      if (url.pathname === "/api/v1/monitoring-shares" && method === "GET") {
        const limit = Number(url.searchParams.get("limit") ?? "100");
        const offset = Number(url.searchParams.get("offset") ?? "0");
        await json(route, shares.slice(offset, offset + limit));
        return;
      }
      if (url.pathname === "/api/v1/monitoring-shares" && method === "POST") {
        const body = request.postDataJSON() as CreateMonitoringShareRequest;
        const created = shareFixture({
          createdAt: new Date().toISOString(),
          expiresAt: new Date(
            Date.now() + body.expires_in_secs * 1_000,
          ).toISOString(),
          id: createdShareId,
          name: body.name,
          status: "active",
          targetUpdateEvidenceAvailable: false,
        });
        created.selector_expression = body.selector_expression ?? "*";
        created.target_client_ids = body.target_client_ids ?? [];
        created.target_count = created.target_client_ids.length;
        created.visibility = {
          billing: Boolean(body.visibility.billing),
          detail_history: Boolean(body.visibility.detail_history),
          identity_context: Boolean(body.visibility.identity_context),
          network: Boolean(body.visibility.network),
          ping: Boolean(body.visibility.ping),
          resources: Boolean(body.visibility.resources),
          system_information: Boolean(body.visibility.system_information),
          traffic: Boolean(body.visibility.traffic),
        };
        shares = [created, ...shares];
        await json(route, {
          fragment_path: `#/share/${createdShareId}/public-url-secret`,
          share: created,
        });
        return;
      }
      if (
        url.pathname === `/api/v1/monitoring-shares/${activeShareId}/url` &&
        method === "GET"
      ) {
        await json(route, {
          fragment_path: `#/share/${activeShareId}/customer-status-secret`,
        });
        return;
      }
      if (
        url.pathname === "/api/v1/monitoring-shares/update-targets" &&
        method === "POST"
      ) {
        const body = request.postDataJSON() as {
          confirmed?: boolean;
          preview_hash?: string;
          share_ids: string[];
        };
        const selected = new Set(body.share_ids);
        const changes = shares
          .filter((share) => selected.has(share.id))
          .map((share) => ({
            added_client_ids:
              share.id === activeShareId ? ["agent-fra-02"] : [],
            removed_client_ids: [],
            selector_expression: share.selector_expression,
            share_id: share.id,
            share_name: share.name,
            unchanged_count: share.target_client_ids.length,
          }));
        const previewHash = "shared-view-target-preview";
        if (!body.confirmed) {
          await json(route, {
            applied: false,
            changes,
            preview_hash: previewHash,
          });
          return;
        }
        if (body.preview_hash !== previewHash) {
          await route.fulfill({
            body: JSON.stringify({ error: "monitoring_share_preview_stale" }),
            contentType: "application/json",
            status: 409,
          });
          return;
        }
        shares = shares.map((share) =>
          selected.has(share.id) && share.id === activeShareId
            ? {
                ...share,
                target_client_ids: ["agent-sfo-01", "agent-fra-02"],
                target_count: 2,
                target_update_available: false,
                target_update_evidence_available: true,
                updated_at: new Date().toISOString(),
              }
            : share,
        );
        await json(route, {
          applied: true,
          changes,
          preview_hash: previewHash,
        });
        return;
      }
      if (
        url.pathname === "/api/v1/monitoring-shares/extend" &&
        method === "POST"
      ) {
        const body = request.postDataJSON() as {
          extend_by_secs: number;
          share_ids: string[];
        };
        const selected = new Set(body.share_ids);
        shares = shares.map((share) =>
          selected.has(share.id)
            ? {
                ...share,
                expires_at: new Date(
                  timestamp(share.expires_at) + body.extend_by_secs * 1_000,
                ).toISOString(),
                updated_at: new Date().toISOString(),
              }
            : share,
        );
        await json(route, {
          shares: shares.filter((share) => selected.has(share.id)),
        });
        return;
      }
      if (
        url.pathname === "/api/v1/monitoring-shares/revoke" &&
        method === "POST"
      ) {
        const body = request.postDataJSON() as { share_ids: string[] };
        const selected = new Set(body.share_ids);
        const revokedAt = new Date().toISOString();
        shares = shares.map((share) =>
          selected.has(share.id)
            ? {
                ...share,
                revoked_at: revokedAt,
                status: "revoked",
                updated_at: revokedAt,
              }
            : share,
        );
        await json(route, {
          shares: shares.filter((share) => selected.has(share.id)),
        });
        return;
      }
      await route.fallback();
    },
  );
}

async function expectPublicPingShorter(
  shorter: ReturnType<Page["locator"]>,
  taller: ReturnType<Page["locator"]>,
) {
  await expect
    .poll(async () => {
      const shorterBox = await shorter.boundingBox();
      const tallerBox = await taller.boundingBox();
      if (!shorterBox || !tallerBox) return Number.NEGATIVE_INFINITY;
      return tallerBox.height - shorterBox.height;
    })
    .toBeGreaterThanOrEqual(8);
}

async function installPublicMonitoringApiMock(
  page: Page,
  {
    annualBilling = false,
    cardCount = 1,
    detailAllowed = true,
    directionalUnlimited = false,
    edgeCases = false,
    identityContext = false,
    mixedTrafficQuotas = false,
    nearQuota = false,
    networkRateExpected = true,
    noResetTraffic = false,
    pingLatencyMs = 18.5,
    pingLossRatio = 0,
    pingStateCoverage = false,
    pingTargetName = "Customer gateway",
    retainTrafficHistory = false,
    supplementalVisibility = edgeCases,
    tieredHistory = false,
    trafficOnlyOldCustom = false,
    trafficConfigured = true,
  }: {
    annualBilling?: boolean;
    cardCount?: number;
    detailAllowed?: boolean;
    directionalUnlimited?: boolean;
    edgeCases?: boolean;
    identityContext?: boolean;
    mixedTrafficQuotas?: boolean;
    nearQuota?: boolean;
    networkRateExpected?: boolean;
    noResetTraffic?: boolean;
    pingLatencyMs?: number | null;
    pingLossRatio?: number | null;
    pingStateCoverage?: boolean;
    pingTargetName?: string;
    retainTrafficHistory?: boolean;
    supplementalVisibility?: boolean;
    tieredHistory?: boolean;
    trafficOnlyOldCustom?: boolean;
    trafficConfigured?: boolean;
  } = {},
) {
  const now = Date.now();
  const observedAt = new Date(now - 10_000).toISOString();
  const minute = Math.floor(now / 60_000) * 60;
  const rangeStart = minute - 14 * 60;
  const rangeEnd = minute;
  const share: PublicMonitoringShareView = {
    expires_at: new Date(now + 24 * 60 * 60 * 1_000).toISOString(),
    id: publicShareId,
    name: "Customer network view",
    target_count: cardCount,
    visibility: {
      billing: supplementalVisibility,
      detail_history: detailAllowed,
      identity_context: identityContext,
      network: true,
      ping: true,
      resources: false,
      system_information: supplementalVisibility,
      traffic: true,
    },
  };
  const card: PublicMonitoringCardView = {
    client_key: publicClientKey,
    display_name: "Shared edge",
    product_name: identityContext ? "Storage-Box 4" : undefined,
    network: {
      observed_at: networkRateExpected ? observedAt : null,
      rate_expected: networkRateExpected,
      rx_bps: networkRateExpected ? 1_024_000 : null,
      tx_bps: networkRateExpected ? 512_000 : null,
    },
    network_history: networkRateExpected
      ? [
          {
            bucket_secs: 60,
            bucket_start: new Date(rangeStart * 1_000).toISOString(),
            rx_bps: 900_000,
            tx_bps: 450_000,
          },
          {
            bucket_secs: 60,
            bucket_start: new Date(rangeEnd * 1_000).toISOString(),
            rx_bps: 1_024_000,
            tx_bps: 512_000,
          },
        ]
      : [],
    primary_ping: {
      checked_at: observedAt,
      latency_avg_ms: pingLatencyMs,
      loss_ratio: pingLossRatio,
      state: "ok",
      status: "ok",
      target_name: pingTargetName,
    },
    primary_ping_history: [
      {
        bucket_secs: 60,
        bucket_start: new Date(rangeStart * 1_000).toISOString(),
        checked_at: new Date(rangeStart * 1_000).toISOString(),
        latency_avg_ms: 19,
        loss_ratio: 0,
        sample_count: 3,
        status: "ok",
        target_name: pingTargetName,
      },
      {
        bucket_secs: 60,
        bucket_start: new Date(rangeEnd * 1_000).toISOString(),
        checked_at: observedAt,
        latency_avg_ms: pingLatencyMs,
        loss_ratio: pingLossRatio,
        sample_count: 3,
        status: "ok",
        target_name: pingTargetName,
      },
    ],
    status: "online",
    tags: identityContext
      ? [
          "country:US",
          "provider:Northwind",
          "provider:Transit",
          "region:Virginia",
        ]
      : undefined,
    traffic: {
      configured: true,
      reset_day: 14,
      cycle_end: new Date(now + 20 * 24 * 60 * 60 * 1_000).toISOString(),
      cycle_percent: 25,
      cycle_start: new Date(now - 10 * 24 * 60 * 60 * 1_000).toISOString(),
      diagnostic_rx_bytes: 2_000,
      diagnostic_total_bytes: 3_000,
      diagnostic_tx_bytes: 1_000,
      observed_at: observedAt,
      quota_total_bytes: 12_000,
      rx_bytes: 2_000,
      state: "ok",
      total_bytes: 3_000,
      tx_bytes: 1_000,
    },
  };
  if (card.traffic && !trafficConfigured) {
    card.traffic = {
      configured: false,
      state: "unconfigured",
    };
  }
  if (edgeCases) {
    share.visibility.resources = true;
    card.billing = {
      cycle: "15",
      disabled: false,
      display: "35.00 ¥/m",
      period_code: "m",
    };
    card.system_information = {
      architecture: "x86_64",
      cpu_model: "AMD EPYC 7B13",
      kernel_release: "6.12.38-amd64",
      os_name: "Debian GNU/Linux 13",
      reported_at: observedAt,
      uptime_observed_at: observedAt,
      uptime_secs: 8 * 24 * 60 * 60 + 3 * 60 * 60,
      virtualization: "kvm",
    };
    const resources = {
      bucket_secs: 60,
      bucket_start: new Date(rangeEnd * 1_000).toISOString(),
      connections_observed_at: observedAt,
      cpu_cores: 4,
      cpu_usage_avg: 0.24,
      disk_available_bytes: 60_000_000_000,
      disk_total_bytes: 100_000_000_000,
      disk_used_ratio_avg: 0.4,
      load_1: 0.8,
      load_5: 0.7,
      load_15: 0.6,
      memory_available_bytes: 6_000_000_000,
      memory_total_bytes: 8_000_000_000,
      memory_used_ratio_avg: 0.25,
      observed_at: observedAt,
      sample_count: 1,
      swap_available_bytes: 1_500_000_000,
      swap_sample_count: 1,
      swap_total_bytes: 2_000_000_000,
      swap_used_ratio_avg: 0.25,
      tcp_sockets: 37,
      udp_sockets: 4,
    };
    card.resources = resources;
    card.resource_history = [resources];
    if (card.traffic) {
      delete card.traffic.cycle_percent;
      card.traffic.quota_total_bytes = -1;
    }
    if (card.primary_ping) {
      card.primary_ping.state = "disabled";
      card.primary_ping.status = "ok";
    }
  }
  if (mixedTrafficQuotas && card.traffic) {
    card.traffic.cycle_percent = 125;
    card.traffic.quota_rx_bytes = 4_000;
    card.traffic.quota_total_bytes = -1;
    card.traffic.quota_tx_bytes = 800;
    card.traffic.rx_bytes = 0;
    card.traffic.total_bytes = 1_000;
    card.traffic.tx_bytes = 1_000;
  }
  if (nearQuota && card.traffic) {
    card.traffic.cycle_percent = 95;
    card.traffic.quota_total_bytes = 12_000;
    card.traffic.rx_bytes = 7_600;
    card.traffic.total_bytes = 11_400;
    card.traffic.tx_bytes = 3_800;
  }
  if (directionalUnlimited && card.traffic) {
    delete card.traffic.cycle_percent;
    card.traffic.quota_total_bytes = undefined;
    card.traffic.quota_tx_bytes = -1;
    card.traffic.rx_bytes = 0;
    card.traffic.total_bytes = 1_000;
    card.traffic.tx_bytes = 1_000;
  }
  if (annualBilling) {
    share.visibility.billing = true;
    card.billing = {
      cycle: "06-15",
      disabled: false,
      display: "120.00 USD/y",
      period_code: "y",
    };
  }
  if (noResetTraffic && card.traffic) {
    card.traffic.reset_day = -1;
    delete card.traffic.cycle_start;
    delete card.traffic.cycle_end;
  }
  if (trafficOnlyOldCustom) {
    share.visibility.network = false;
    share.visibility.ping = false;
    card.network = undefined;
    card.network_history = undefined;
    card.primary_ping = undefined;
    card.primary_ping_history = undefined;
  }
  const detail: PublicMonitoringDetailView = {
    client_key: publicClientKey,
    network: card.network_history,
    ping: card.primary_ping_history,
    ping_targets: card.primary_ping ? [card.primary_ping] : [],
    range: {
      end_unix: rangeEnd,
      effective_points: tieredHistory ? 4 : 2,
      effective_resolution_secs: tieredHistory ? 300 : 60,
      points: 2,
      requested_step_secs: 60,
      resolutions: {
        network: tieredHistory ? 300 : 60,
        ping: tieredHistory ? 300 : 60,
        resources: tieredHistory ? 300 : 60,
        traffic: 60,
      },
      source: tieredHistory ? "retained" : "raw",
      start_unix: rangeStart,
      step_secs: tieredHistory ? 300 : 60,
      window: "15m",
    },
    traffic: [
      {
        bucket_secs: 60,
        bucket_start: new Date(rangeStart * 1_000).toISOString(),
        reset_count: 0,
        rx_bytes: 1_000,
        sample_count: 1,
        total_bytes: 1_500,
        tx_bytes: 500,
      },
      {
        bucket_secs: 60,
        bucket_start: new Date(rangeEnd * 1_000).toISOString(),
        reset_count: 0,
        rx_bytes: 2_000,
        sample_count: 1,
        total_bytes: 3_000,
        tx_bytes: 1_000,
      },
    ],
  };
  if (trafficOnlyOldCustom) {
    detail.network = undefined;
    detail.ping = undefined;
    detail.ping_targets = undefined;
    detail.resources = undefined;
    const halfHour = 30 * 60;
    const oldEnd =
      Math.floor((Math.floor(now / 1_000) - 20 * 24 * 60 * 60) / halfHour) *
      halfHour;
    const oldStart = oldEnd - 60 * 60;
    detail.range = {
      ...detail.range,
      effective_points: 3,
      effective_resolution_secs: halfHour,
      end_unix: oldEnd,
      resolutions: {
        network: halfHour,
        ping: halfHour,
        resources: halfHour,
        traffic: 60,
      },
      source: "retained",
      start_unix: oldStart,
      step_secs: halfHour,
      window: "custom",
    };
    detail.traffic = [oldStart, oldStart + halfHour, oldEnd].map(
      (bucket, index) => ({
        bucket_secs: halfHour,
        bucket_start: new Date(bucket * 1_000).toISOString(),
        reset_count: 0,
        rx_bytes: 1_000 + index * 100,
        sample_count: 30,
        total_bytes: 1_500 + index * 150,
        tx_bytes: 500 + index * 50,
      }),
    );
  }
  if (edgeCases) {
    const visualBuckets = Array.from(
      { length: 15 },
      (_, index) => rangeEnd - (14 - index) * 60,
    );
    const resourceBase = card.resources!;
    card.resource_history = visualBuckets.map((bucket, index) => ({
      ...resourceBase,
      bucket_start: new Date(bucket * 1_000).toISOString(),
      cpu_usage_avg: 0.2 + Math.sin(index / 3) * 0.08,
      load_1: 0.7 + Math.sin(index / 4) * 0.18,
      load_5: 0.65 + Math.sin(index / 5) * 0.12,
      load_15: 0.6 + Math.sin(index / 6) * 0.08,
      memory_available_bytes: 6_000_000_000 - Math.sin(index / 4) * 300_000_000,
      memory_used_ratio_avg:
        1 -
        (6_000_000_000 - Math.sin(index / 4) * 300_000_000) /
          resourceBase.memory_total_bytes,
      swap_available_bytes:
        index === visualBuckets.length - 1
          ? 0
          : resourceBase.swap_available_bytes,
      swap_sample_count: index === visualBuckets.length - 1 ? 0 : 1,
      swap_total_bytes:
        index === visualBuckets.length - 1 ? 0 : resourceBase.swap_total_bytes,
      swap_used_ratio_avg:
        index === visualBuckets.length - 1
          ? undefined
          : resourceBase.swap_used_ratio_avg,
      tcp_sockets: 37 + Math.round(Math.sin(index / 3) * 5),
      udp_sockets: 4 + Math.round(Math.cos(index / 4)),
    }));
    card.network_history = visualBuckets.map((bucket, index) => ({
      bucket_secs: 60,
      bucket_start: new Date(bucket * 1_000).toISOString(),
      rx_bps: 900_000 + Math.sin(index / 2) * 180_000,
      tx_bps: 450_000 + Math.cos(index / 3) * 90_000,
    }));
    const primaryHistory = visualBuckets.map((bucket, index) => ({
      bucket_secs: 60,
      bucket_start: new Date(bucket * 1_000).toISOString(),
      checked_at: new Date(bucket * 1_000).toISOString(),
      latency_avg_ms: (pingLatencyMs ?? 18.5) + Math.sin(index / 3) * 2.5,
      loss_ratio: index % 8 === 0 ? 0.03 : 0,
      sample_count: 3,
      status: "ok",
      target_name: pingTargetName,
    }));
    card.primary_ping_history = primaryHistory;
    const secondaryTargets: NonNullable<
      PublicMonitoringDetailView["ping_targets"]
    > = [
      {
        checked_at: observedAt,
        latency_avg_ms: 42,
        loss_ratio: 0,
        state: "ok",
        status: "ok",
        target_name: "Regional transit",
      },
      {
        checked_at: observedAt,
        latency_avg_ms: 86,
        loss_ratio: 0.04,
        state: "degraded",
        status: "degraded",
        target_name: "Backup resolver",
      },
    ];
    detail.range = {
      ...detail.range,
      points: visualBuckets.length,
      start_unix: visualBuckets[0],
    };
    detail.resources = card.resource_history;
    detail.network = card.network_history;
    detail.traffic = visualBuckets.map((bucket, index) => ({
      bucket_secs: 60,
      bucket_start: new Date(bucket * 1_000).toISOString(),
      reset_count: 0,
      rx_bytes: 2_000 + index * 80,
      sample_count: 1,
      total_bytes: 3_000 + index * 120,
      tx_bytes: 1_000 + index * 40,
    }));
    detail.ping_targets = [
      ...(card.primary_ping ? [card.primary_ping] : []),
      ...secondaryTargets,
    ];
    detail.ping = [
      ...primaryHistory,
      ...secondaryTargets.flatMap((target, targetIndex) =>
        visualBuckets.map((bucket, index) => ({
          bucket_secs: 60,
          bucket_start: new Date(bucket * 1_000).toISOString(),
          checked_at: new Date(bucket * 1_000).toISOString(),
          latency_avg_ms:
            (target.latency_avg_ms ?? 0) + Math.sin(index / 4) * 4,
          loss_ratio:
            targetIndex === 1 && index % 6 === 0
              ? 0.08
              : (target.loss_ratio ?? 0),
          sample_count: 3,
          status: target.status ?? target.state,
          target_name: target.target_name,
        })),
      ),
    ];
    card.resources = {
      ...card.resources!,
      disk_available_bytes: 30_000_000_000,
      disk_total_bytes: 50_000_000_000,
      memory_available_bytes: 3_000_000_000,
      memory_total_bytes: 4_000_000_000,
      swap_available_bytes: 0,
      swap_sample_count: 0,
      swap_total_bytes: 0,
      swap_used_ratio_avg: undefined,
    };
  }
  if (!trafficConfigured && !retainTrafficHistory) {
    detail.traffic = undefined;
  }
  const cardProfiles = [
    ["Shared edge", "US", "Northwind", "Virginia"],
    ["Frankfurt build", "DE", "Hetzner", "Frankfurt"],
    ["Tokyo relay", "JP", "Vultr", "Tokyo"],
    ["New York API", "US", "Linode", "New York"],
    ["Sydney backup", "AU", "OVH", "Sydney"],
    ["London cache", "GB", "Hetzner", "London"],
    ["Mumbai worker", "IN", "Vultr", "Mumbai"],
    ["Toronto transit", "CA", "Linode", "Toronto"],
  ] as const;
  const cards = Array.from({ length: cardCount }, (_, index) => {
    if (index === 0) return card;
    const [displayName, country, provider, region] =
      cardProfiles[index % cardProfiles.length];
    const status = index === 6 ? "offline" : "online";
    const resource = card.resources
      ? {
          ...card.resources,
          cpu_usage_avg: Math.min(0.95, 0.18 + index * 0.09),
          load_1: card.resources.load_1 + index * 0.16,
          memory_available_bytes:
            card.resources.memory_available_bytes - index * 240_000_000,
          memory_used_ratio_avg:
            1 -
            (card.resources.memory_available_bytes - index * 240_000_000) /
              card.resources.memory_total_bytes,
          tcp_sockets: (card.resources.tcp_sockets ?? 30) + index * 7,
          udp_sockets: (card.resources.udp_sockets ?? 4) + index,
        }
      : undefined;
    const primaryPing =
      pingStateCoverage && index === 2
        ? undefined
        : card.primary_ping
          ? {
              ...card.primary_ping,
              latency_avg_ms:
                (card.primary_ping.latency_avg_ms ?? 18) + index * 6,
              loss_ratio:
                pingStateCoverage && index === 1
                  ? 0.2
                  : card.primary_ping.loss_ratio,
              state:
                (pingStateCoverage && index === 1) ||
                (!pingStateCoverage && index === 4)
                  ? "degraded"
                  : "ok",
              status:
                (pingStateCoverage && index === 1) ||
                (!pingStateCoverage && index === 4)
                  ? "degraded"
                  : "ok",
              target_name: pingStateCoverage
                ? index === 1
                  ? "Fixture degraded gateway"
                  : "Fixture healthy gateway"
                : card.primary_ping.target_name,
            }
          : undefined;
    return {
      ...card,
      client_key: `${publicClientKey}-${index + 1}`,
      display_name: displayName,
      product_name: identityContext ? `${provider} plan` : undefined,
      network: card.network
        ? {
            ...card.network,
            rx_bps: (card.network.rx_bps ?? 0) + index * 180_000,
            tx_bps: (card.network.tx_bps ?? 0) + index * 70_000,
          }
        : undefined,
      primary_ping: primaryPing,
      primary_ping_history:
        pingStateCoverage && index === 2 ? [] : card.primary_ping_history,
      resources: resource,
      status,
      tags: identityContext
        ? [`country:${country}`, `provider:${provider}`, `region:${region}`]
        : undefined,
      traffic: card.traffic?.configured
        ? {
            ...card.traffic,
            cycle_percent: 20 + index * 8,
            quota_total_bytes: 1_000_000,
            total_bytes: (card.traffic.total_bytes ?? 0) + index * 50_000,
          }
        : card.traffic,
    };
  });

  await page.route(
    /\/api\/v1\/public\/monitoring-shares\/[^/?]+\/(?:bootstrap|data)(?:\?.*)?$/,
    async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (request.headers()["x-vpsman-share-token"] !== publicShareSecret) {
        await route.fulfill({
          body: JSON.stringify({ error: "monitoring_share_not_found" }),
          contentType: "application/json",
          status: 404,
        });
        return;
      }
      if (url.pathname.endsWith("/bootstrap")) {
        await json(route, {
          share,
          visitor_id: "66666666-6666-4666-8666-666666666666",
        });
        return;
      }
      const response: PublicMonitoringDataView = {
        cards,
        detail:
          url.searchParams.get("client_key") === publicClientKey
            ? detail
            : undefined,
        next_offset: null,
        offset: 0,
        share,
        total: cards.length,
      };
      await json(route, response);
    },
  );
}

function shareFixture({
  createdAt,
  expiresAt,
  id,
  lastVisitedAt = null,
  name,
  status,
  targetClientIds = ["v-1"],
  targetUpdateAvailable = false,
  targetUpdateEvidenceAvailable = true,
  visitorCount = 0,
}: {
  createdAt: string;
  expiresAt: string;
  id: string;
  lastVisitedAt?: string | null;
  name: string;
  status: MonitoringShareView["status"];
  targetClientIds?: string[];
  targetUpdateAvailable?: boolean;
  targetUpdateEvidenceAvailable?: boolean;
  visitorCount?: number;
}): MonitoringShareView {
  return {
    created_at: createdAt,
    expires_at: expiresAt,
    id,
    created_by: "operator",
    first_visited_at: null,
    last_visited_at: lastVisitedAt,
    name,
    revoked_at: null,
    selector_expression: "*",
    status,
    target_client_ids: targetClientIds,
    target_count: targetClientIds.length,
    target_update_available: targetUpdateAvailable,
    target_update_evidence_available: targetUpdateEvidenceAvailable,
    updated_at: createdAt,
    visibility: {
      billing: false,
      detail_history: true,
      identity_context: false,
      network: true,
      ping: true,
      resources: true,
      system_information: false,
      traffic: true,
    },
    visitor_count: visitorCount,
  };
}

async function json(route: Route, body: unknown) {
  await route.fulfill({
    body: JSON.stringify(body),
    contentType: "application/json",
    status: 200,
  });
}

function timestamp(value: string): number {
  const numeric = Number(value);
  return Number.isFinite(numeric) && /^\d+$/.test(value)
    ? numeric * 1_000
    : new Date(value).getTime();
}
