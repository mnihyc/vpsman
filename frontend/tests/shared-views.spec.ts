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
  await expect(
    page.getByRole("heading", { name: "Create shared view" }),
  ).toBeVisible();
  const drawerLayout = await page
    .locator(".actionDrawer:visible")
    .evaluate((drawer) => {
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
  await page.getByRole("button", { name: "Review creation" }).click();

  const createConfirmation = page
    .locator(".confirmationPrompt")
    .filter({ hasText: "Confirm public monitoring view" });
  await expect(
    createConfirmation.getByText("Confirm public monitoring view", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    createConfirmation.getByText("3", { exact: true }),
  ).toBeVisible();
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
    .getByRole("row")
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
  await grid
    .getByLabel(`Expand Active shared views row ${activeShareId}`)
    .click();
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
  await expect(
    page.getByText("Customer status", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Regional customer view", { exact: true }),
  ).toBeVisible();
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

test("public monitoring reuses the Unicode country flag renderer", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { identityContext: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  await expect(card.locator(".countryFlagGlyph")).toHaveText("🇺🇸");
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
  await expect(card.getByText("Updated just now").first()).toHaveAttribute(
    "title",
    "Updated just now",
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
  await expect(publicTraffic.locator("div > strong")).toHaveAttribute(
    "title",
    /Traffic/,
  );
  await expect(publicTraffic.locator("div > span")).toHaveAttribute(
    "title",
    /\S/,
  );
  await expect(publicTraffic.locator(":scope > small")).toHaveAttribute(
    "title",
    /\S/,
  );
  const publicPing = card.locator(".publicMonitoringPing");
  await expect(publicPing.locator(":scope > span")).toHaveAttribute(
    "title",
    /\S/,
  );
  await expect(publicPing.locator(":scope > small")).toHaveAttribute(
    "title",
    /\S/,
  );
  await expect(card.locator(".publicMonitoringPing > small")).toContainText(
    "Reachable",
  );
  await expect(card.getByText("Ok", { exact: true })).toHaveCount(0);

  const cardGrid = page.getByLabel("Shared VPS cards");
  const columnCount = () =>
    cardGrid.evaluate(
      (node) =>
        getComputedStyle(node).gridTemplateColumns.split(/\s+/).filter(Boolean)
          .length,
    );
  const comfortableHeight = await card.evaluate(
    (node) => node.getBoundingClientRect().height,
  );
  const isMobile = (page.viewportSize()?.width ?? 1_000) < 500;
  expect(await columnCount()).toBe(isMobile ? 1 : 3);
  await page.getByRole("button", { name: "Compact", exact: true }).click();
  const compactHeight = await card.evaluate(
    (node) => node.getBoundingClientRect().height,
  );
  expect(await columnCount()).toBe(isMobile ? 1 : 5);
  expect(compactHeight).toBeLessThan(comfortableHeight);
  await page.getByRole("button", { name: "Comfortable", exact: true }).click();

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
  await expect(detail.getByText("Reachable", { exact: true })).toBeVisible();
  await expect(detail.getByText("Ok", { exact: true })).toHaveCount(0);
  await expect(
    detail.locator(".vpsMonitoringPingTarget strong").first(),
  ).toHaveAttribute("title", /\S/);

  const trafficChart = detail
    .getByRole("heading", { name: "Traffic volume" })
    .locator("..");
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
});

test("public monitoring presents warnings, disabled Ping, unlimited quotas, resources, and narrow detail without ambiguity", async ({
  page,
}) => {
  await installPublicMonitoringApiMock(page, { edgeCases: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
  });
  await expect(card).toBeVisible();
  await expect(
    card.getByText("Online · Warning", { exact: true }),
  ).toBeVisible();
  await expect(card.getByText(/Last sample 18\.5 ms/)).toBeVisible();
  await expect(card).toContainText("Primary Ping disabled");
  await expect(
    card.getByLabel("Current resources for Shared edge"),
  ).toBeVisible();
  await expect(card.getByText("TCP", { exact: true })).toBeVisible();
  await expect(card.getByText("Unlimited", { exact: false })).toBeVisible();
  await expect(card).not.toContainText("/ 0 B");
  await expect(
    card.getByLabel("Primary Ping Customer gateway: Disabled"),
  ).toBeVisible();

  await card.press("Enter");
  const detail = page.getByRole("region", {
    name: "Read-only history for Shared edge",
  });
  await expect(detail).toBeVisible();
  await expect(detail.locator(".publicMonitoringDetailHeader")).toContainText(
    "Online · Warning · Updated just now · Read-only history",
  );
  await expect(detail.getByText("Disabled", { exact: true })).toBeVisible();
  await expect(detail.getByText(/Last sample: 18\.5 ms/)).toBeVisible();
  await expect(detail.getByText("Unlimited", { exact: true })).toBeVisible();
  await expect(detail).not.toContainText("/ 0 B");
  const connectionsChart = detail
    .getByRole("heading", { name: "TCP / UDP connections" })
    .locator("..");
  await expect(
    connectionsChart.getByRole("button", { name: "Hide TCP series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    connectionsChart.getByRole("button", { name: "Hide UDP series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(connectionsChart.getByText("2/2 series")).toBeVisible();

  const range = detail.getByRole("group", { name: "History range" });
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

test("public monitoring grid and detail have complete screenshot coverage", async ({
  page,
}, testInfo) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installPublicMonitoringApiMock(page, { edgeCases: true });
  await page.goto(`/#/share/${publicShareId}/${publicShareSecret}`);

  const card = page.getByRole("link", {
    name: /Shared edge · Online shared monitoring card/,
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

  await capture("42d-public-monitoring-grid");
  await page
    .getByRole("group", { name: "Shared view density" })
    .getByRole("button", { name: "Compact" })
    .click();
  await expect(page.getByLabel("Shared VPS cards")).toHaveAttribute(
    "data-density",
    "compact",
  );
  await capture("42da-public-monitoring-grid-compact");
  await page
    .getByRole("group", { name: "Shared view density" })
    .getByRole("button", { name: "Comfortable" })
    .click();

  await card.click();
  await expect(
    page.getByRole("region", {
      name: "Read-only history for Shared edge",
    }),
  ).toBeVisible();
  await capture("42e-public-monitoring-detail");

  for (const state of ["expired", "revoked", "invalid"] as const) {
    const unavailableShareId = `${state}-share`;
    await page.route(
      new RegExp(
        `/api/v1/public/monitoring-shares/${unavailableShareId}/bootstrap$`,
      ),
      async (route) => {
        await route.fulfill({
          body: JSON.stringify({ error: "monitoring_share_not_found" }),
          contentType: "application/json",
          status: 404,
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
        });
        created.selector_expression = body.selector_expression ?? "*";
        created.target_client_ids = body.target_client_ids ?? [];
        created.target_count = created.target_client_ids.length;
        created.visibility = {
          detail_history: Boolean(body.visibility.detail_history),
          identity_context: Boolean(body.visibility.identity_context),
          network: Boolean(body.visibility.network),
          ping: Boolean(body.visibility.ping),
          resources: Boolean(body.visibility.resources),
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
        url.pathname ===
          `/api/v1/monitoring-shares/${activeShareId}/url` &&
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

async function installPublicMonitoringApiMock(
  page: Page,
  {
    detailAllowed = true,
    edgeCases = false,
    identityContext = false,
  }: {
    detailAllowed?: boolean;
    edgeCases?: boolean;
    identityContext?: boolean;
  } = {},
) {
  const now = Date.now();
  const observedAt = new Date(now - 10_000).toISOString();
  const minute = Math.floor(now / 60_000) * 60;
  const rangeStart = minute - 120;
  const rangeEnd = minute;
  const share: PublicMonitoringShareView = {
    expires_at: new Date(now + 24 * 60 * 60 * 1_000).toISOString(),
    id: publicShareId,
    name: "Customer network view",
    target_count: 1,
    visibility: {
      detail_history: detailAllowed,
      identity_context: identityContext,
      network: true,
      ping: true,
      resources: false,
      traffic: true,
    },
  };
  const card: PublicMonitoringCardView = {
    client_key: publicClientKey,
    display_name: "Shared edge",
    network: {
      observed_at: observedAt,
      rx_bps: 1_024_000,
      tx_bps: 512_000,
    },
    network_history: [
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
    ],
    primary_ping: {
      checked_at: observedAt,
      latency_avg_ms: 18.5,
      loss_ratio: 0,
      state: "ok",
      status: "ok",
      target_name: "Customer gateway",
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
        target_name: "Customer gateway",
      },
      {
        bucket_secs: 60,
        bucket_start: new Date(rangeEnd * 1_000).toISOString(),
        checked_at: observedAt,
        latency_avg_ms: 18.5,
        loss_ratio: 0,
        sample_count: 3,
        status: "ok",
        target_name: "Customer gateway",
      },
    ],
    status: "online",
    tags: identityContext ? ["country:US"] : undefined,
    traffic: {
      configured: true,
      cycle_end: new Date(now + 20 * 24 * 60 * 60 * 1_000).toISOString(),
      cycle_percent: 25,
      cycle_start: new Date(now - 10 * 24 * 60 * 60 * 1_000).toISOString(),
      observed_at: observedAt,
      port_speed: null,
      quota_rx_bytes: null,
      quota_total_bytes: 12_000,
      quota_tx_bytes: null,
      rx_bytes: 2_000,
      state: "ok",
      total_bytes: 3_000,
      tx_bytes: 1_000,
    },
  };
  if (edgeCases) {
    share.visibility.resources = true;
    const resources = {
      bucket_secs: 60,
      bucket_start: new Date(rangeEnd * 1_000).toISOString(),
      connections_observed_at: observedAt,
      cpu_cores: 4,
      cpu_usage_avg: 0.24,
      disk_available_bytes: 60_000_000_000,
      disk_total_bytes: 100_000_000_000,
      load_1: 0.8,
      load_5: 0.7,
      load_15: 0.6,
      memory_available_bytes: 6_000_000_000,
      memory_total_bytes: 8_000_000_000,
      observed_at: observedAt,
      sample_count: 1,
      tcp_sockets: 37,
      udp_sockets: 4,
    };
    card.resources = resources;
    card.resource_history = [resources];
    if (card.traffic) {
      card.traffic.cycle_percent = null;
      card.traffic.quota_total_bytes = -1;
    }
    if (card.primary_ping) {
      card.primary_ping.state = "disabled";
      card.primary_ping.status = "ok";
    }
  }
  const detail: PublicMonitoringDetailView = {
    client_key: publicClientKey,
    network: card.network_history,
    ping: card.primary_ping_history,
    ping_targets: card.primary_ping ? [card.primary_ping] : [],
    range: {
      end_unix: rangeEnd,
      points: 3,
      source: "minute",
      start_unix: rangeStart,
      step_secs: 60,
      window: "1d",
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
  if (edgeCases) {
    detail.resources = card.resource_history;
    detail.ping_targets = card.primary_ping ? [card.primary_ping] : [];
  }

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
        cards: [card],
        detail:
          url.searchParams.get("client_key") === publicClientKey
            ? detail
            : undefined,
        next_offset: null,
        offset: 0,
        share,
        total: 1,
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
    updated_at: createdAt,
    visibility: {
      detail_history: true,
      identity_context: false,
      network: true,
      ping: true,
      resources: true,
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
