import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { openConsoleSubpage } from "./support/consoleNavigation";

const captureEnabled =
  process.env.VPSMAN_MONITORING_REAL_DATA_CAPTURE === "1";
const outputDir = process.env.VPSMAN_MONITORING_REAL_DATA_OUTPUT ?? "";
const operatorUsername =
  process.env.VPSMAN_MONITORING_REAL_DATA_USERNAME ?? "";
const operatorPassword =
  process.env.VPSMAN_MONITORING_REAL_DATA_PASSWORD ?? "";
const visibleShareFragment =
  process.env.VPSMAN_MONITORING_VISIBLE_SHARE_FRAGMENT ?? "";
const hiddenShareFragment =
  process.env.VPSMAN_MONITORING_HIDDEN_SHARE_FRAGMENT ?? "";
const expectedClients = Number(
  process.env.VPSMAN_MONITORING_EXPECTED_CLIENTS ?? "7",
);

test.skip(
  !captureEnabled,
  "real PostgreSQL review capture is enabled by scripts/review-monitoring-real-data.sh",
);

test("captures private and shared monitoring from the isolated real stack", async ({
  page,
}) => {
  test.setTimeout(180_000);
  expect(outputDir).not.toBe("");
  expect(operatorUsername).not.toBe("");
  expect(operatorPassword).not.toBe("");
  expect(visibleShareFragment).toMatch(/^#\/share\/[0-9a-f-]{36}\/[0-9a-f]{64}$/);
  expect(hiddenShareFragment).toMatch(/^#\/share\/[0-9a-f-]{36}\/[0-9a-f]{64}$/);
  expect(expectedClients).toBe(7);
  mkdirSync(outputDir, { recursive: true });

  const pageErrors: string[] = [];
  const serverErrors: string[] = [];
  const realApiRequests = new Set<string>();
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("request", (request) => {
    const pathname = new URL(request.url()).pathname;
    if (pathname.startsWith("/api/v1/")) realApiRequests.add(pathname);
  });
  page.on("response", (response) => {
    const pathname = new URL(response.url()).pathname;
    if (pathname.startsWith("/api/v1/") && response.status() >= 500) {
      serverErrors.push(`${response.status()} ${pathname}`);
    }
  });
  await page.setViewportSize({ width: 1440, height: 1300 });
  await page.emulateMedia({ reducedMotion: "reduce" });

  await page.goto("/", { waitUntil: "domcontentloaded" });
  await authenticate(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  const privateGrid = page.getByLabel("VPS monitor cards");
  await expect(privateGrid).toBeVisible();
  await expect(page.locator(".fleetMonitorMatchCount")).toContainText(
    `${expectedClients} matched`,
    { timeout: 30_000 },
  );
  await expect(privateGrid.locator(".vpsMonitorCard")).toHaveCount(
    expectedClients,
  );
  await assertPrivateFixtureSemantics(privateGrid);

  const privateDensity = page.getByLabel("VPS cards density");
  await privateDensity.getByRole("button", { name: "Compact" }).click();
  await expect(privateGrid).toHaveAttribute("data-density", "compact");
  await capture(page, "private-monitor-compact.png");

  await page.setViewportSize({ width: 1440, height: 1600 });
  await privateDensity.getByRole("button", { name: "Comfortable" }).click();
  await expect(privateGrid).toHaveAttribute("data-density", "comfortable");
  await expect(
    cardNamed(privateGrid, "Rates intentionally empty").locator(
      ".vpsMonitorFlowFacts strong",
    ),
  ).toHaveText(["-", "-"]);
  await expect(
    cardNamed(privateGrid, "Rates intentionally empty").getByText(
      "Telemetry partial",
    ),
  ).toHaveCount(0);
  await expect(
    cardNamed(privateGrid, "Rates intentionally empty").getByText(
      "Needs attention",
    ),
  ).toHaveCount(0);
  await capture(page, "private-monitor-comfortable.png");

  await page.setViewportSize({ width: 1440, height: 1200 });
  await page.goto(`/${visibleShareFragment}`, {
    waitUntil: "domcontentloaded",
  });
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Monitoring review · Billing visible",
    }),
  ).toBeVisible({ timeout: 30_000 });
  const sharedGrid = page.getByLabel("Shared VPS cards");
  await expect(sharedGrid.locator(".publicMonitoringCard")).toHaveCount(
    expectedClients,
    { timeout: 30_000 },
  );
  await assertSharedFixtureSemantics(sharedGrid);

  const sharedDensity = page.getByLabel("Shared view density");
  await sharedDensity.getByRole("button", { name: "Compact" }).click();
  await expect(sharedGrid).toHaveAttribute("data-density", "compact");
  await capture(page, "shared-monitor-billing-visible-compact.png");

  await sharedDensity.getByRole("button", { name: "Comfortable" }).click();
  await expect(sharedGrid).toHaveAttribute("data-density", "comfortable");
  await expect(
    publicCardNamed(sharedGrid, "Rates intentionally empty").locator(
      ".vpsMonitorFlowFacts strong",
    ),
  ).toHaveText(["-", "-"]);
  await expect(
    publicCardNamed(sharedGrid, "Rates intentionally empty").getByText(
      "Needs attention",
    ),
  ).toHaveCount(0);
  await capture(page, "shared-monitor-billing-visible-comfortable.png");

  const detailCard = page.getByRole("link", {
    name: /Total quota · Monthly .* shared monitoring card/,
  });
  await expect(detailCard).toBeVisible();
  await detailCard.click();
  const detail = page.getByRole("region", {
    name: "Read-only history for Total quota · Monthly",
  });
  await expect(detail).toBeVisible({ timeout: 30_000 });
  await expect(detail.getByLabel("Current shared VPS evidence")).toContainText(
    "29.90 ¥/m",
  );
  await expect(detail.getByRole("region", { name: "Traffic volume chart" })).toBeVisible();
  await capture(page, "shared-monitor-billing-visible-detail.png");

  await page.goto(`/${hiddenShareFragment}`, {
    waitUntil: "domcontentloaded",
  });
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Monitoring review · Billing hidden",
    }),
  ).toBeVisible({ timeout: 30_000 });
  const hiddenSharedGrid = page.getByLabel("Shared VPS cards");
  await expect(hiddenSharedGrid.locator(".publicMonitoringCard")).toHaveCount(
    expectedClients,
    { timeout: 30_000 },
  );
  await expect(hiddenSharedGrid.locator('[data-fact-kind="billing"]')).toHaveCount(0);
  await page
    .getByLabel("Shared view density")
    .getByRole("button", { name: "Compact" })
    .click();
  await expect(hiddenSharedGrid).toHaveAttribute("data-density", "compact");
  await capture(page, "shared-monitor-billing-hidden-compact.png");

  expect([...realApiRequests]).toEqual(
    expect.arrayContaining([
      "/api/v1/monitoring/cards",
      expect.stringMatching(/^\/api\/v1\/public\/monitoring-shares\/[0-9a-f-]+\/bootstrap$/),
      expect.stringMatching(/^\/api\/v1\/public\/monitoring-shares\/[0-9a-f-]+\/data$/),
    ]),
  );
  expect(serverErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

async function authenticate(page: Page) {
  const consoleShell = page.locator(".shell");
  const signIn = page.getByRole("heading", { exact: true, name: "Sign in" });
  await expect(consoleShell.or(signIn).first()).toBeVisible({ timeout: 30_000 });
  if (await consoleShell.isVisible()) return;

  await page.getByLabel("Username").fill(operatorUsername);
  await page.getByLabel("Password").fill(operatorPassword);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(consoleShell).toBeVisible({ timeout: 30_000 });
}

function cardNamed(root: ReturnType<Page["locator"]>, name: string) {
  return root.locator(".vpsMonitorCard", { hasText: name }).first();
}

function publicCardNamed(root: ReturnType<Page["locator"]>, name: string) {
  return root.locator(".publicMonitoringCard", { hasText: name }).first();
}

async function assertPrivateFixtureSemantics(
  grid: ReturnType<Page["locator"]>,
) {
  const total = cardNamed(grid, "Total quota · Monthly");
  const rx = cardNamed(grid, "RX quota · Annual");
  const tx = cardNamed(grid, "TX quota · Unlimited");
  const noReset = cardNamed(grid, "Accumulated archive");
  const emptyRates = cardNamed(grid, "Rates intentionally empty");
  const unconfigured = cardNamed(grid, "Unconfigured traffic");
  const noPrimary = cardNamed(grid, "No primary Ping");

  for (const card of [total, rx, tx, noReset, emptyRates, unconfigured, noPrimary]) {
    await expect(card).toBeVisible();
  }
  await expect(total.locator(".vpsMonitorTraffic")).toContainText("· Total ·");
  await expect(total.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews day 14",
  );
  await expect(rx.locator(".vpsMonitorTraffic")).toContainText("· RX ·");
  await expect(rx.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews 06-15",
  );
  await expect(tx.locator(".vpsMonitorTraffic")).toContainText(
    "/ Unlimited · TX",
  );
  await expect(tx.locator(".unlimitedTrafficTrack")).toBeVisible();
  await expect(noReset.locator(".vpsMonitorTraffic")).toContainText("No reset");
  await expect(
    emptyRates.locator(".vpsMonitorFlowFacts strong"),
  ).toHaveText(["-", "-"]);
  await expect(unconfigured.locator('[data-fact-kind="billing"] strong')).toHaveText("-");
  await expect(unconfigured).not.toContainText("N/A");
  await expect(noPrimary.locator(".vpsMonitorPing")).toContainText("-");
}

async function assertSharedFixtureSemantics(
  grid: ReturnType<Page["locator"]>,
) {
  const total = publicCardNamed(grid, "Total quota · Monthly");
  const rx = publicCardNamed(grid, "RX quota · Annual");
  const tx = publicCardNamed(grid, "TX quota · Unlimited");
  const noReset = publicCardNamed(grid, "Accumulated archive");
  const emptyRates = publicCardNamed(grid, "Rates intentionally empty");
  const unconfigured = publicCardNamed(grid, "Unconfigured traffic");
  const noPrimary = publicCardNamed(grid, "No primary Ping");

  for (const card of [total, rx, tx, noReset, emptyRates, unconfigured, noPrimary]) {
    await expect(card).toBeVisible();
  }
  await expect(total.locator(".publicMonitoringTraffic")).toContainText(
    "· Total ·",
  );
  await expect(total.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews day 14",
  );
  await expect(rx.locator(".publicMonitoringTraffic")).toContainText("· RX ·");
  await expect(rx.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews 06-15",
  );
  await expect(tx.locator(".publicMonitoringTraffic")).toContainText(
    "/ Unlimited · TX",
  );
  await expect(tx.locator(".unlimitedTrafficTrack")).toBeVisible();
  await expect(noReset.locator(".publicMonitoringTraffic")).toContainText(
    "No reset",
  );
  await expect(
    emptyRates.locator(".vpsMonitorFlowFacts strong"),
  ).toHaveText(["-", "-"]);
  await expect(unconfigured.locator('[data-fact-kind="billing"] strong')).toHaveText("-");
  await expect(unconfigured.locator(".publicMonitoringTraffic")).toHaveClass(
    /unconfigured/,
  );
  await expect(unconfigured).not.toContainText("N/A");
  await expect(noPrimary.locator(".publicMonitoringPing")).toContainText("-");
}

async function capture(page: Page, filename: string) {
  await page.screenshot({
    animations: "disabled",
    fullPage: true,
    path: join(outputDir, filename),
  });
}
