import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { openConsoleSubpage } from "./support/consoleNavigation";

const captureEnabled = process.env.VPSMAN_MONITORING_REAL_DATA_CAPTURE === "1";
const outputDir = process.env.VPSMAN_MONITORING_REAL_DATA_OUTPUT ?? "";
const operatorUsername = process.env.VPSMAN_MONITORING_REAL_DATA_USERNAME ?? "";
const operatorPassword = process.env.VPSMAN_MONITORING_REAL_DATA_PASSWORD ?? "";
const visibleShareFragment =
  process.env.VPSMAN_MONITORING_VISIBLE_SHARE_FRAGMENT ?? "";
const hiddenShareFragment =
  process.env.VPSMAN_MONITORING_HIDDEN_SHARE_FRAGMENT ?? "";
const expectedClients = Number(
  process.env.VPSMAN_MONITORING_EXPECTED_CLIENTS ?? "8",
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
  expect(visibleShareFragment).toMatch(
    /^#\/share\/[0-9a-f-]{36}\/[0-9a-f]{64}$/,
  );
  expect(hiddenShareFragment).toMatch(
    /^#\/share\/[0-9a-f-]{36}\/[0-9a-f]{64}$/,
  );
  expect(expectedClients).toBe(8);
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
  await expect(
    cardNamed(privateGrid, "Unconfigured traffic").locator(
      '[data-fact-kind="billing"]',
    ),
  ).toHaveAttribute("title", /Billing is not configured/);

  const privateDensity = page.getByLabel("VPS cards density");
  await privateDensity.getByRole("button", { name: "Compact" }).click();
  await expect(privateGrid).toHaveAttribute("data-density", "compact");
  const privateUnconfigured = cardNamed(privateGrid, "Unconfigured traffic");
  await expect(privateUnconfigured.locator(".vpsMonitorTraffic")).toHaveClass(
    /unconfigured/,
  );
  await expect(
    privateUnconfigured.locator(".vpsMonitorTrafficTrack.missing"),
  ).toBeVisible();
  await expect(
    cardNamed(privateGrid, "RX quota · Annual").getByText(
      "Intermittent packet loss",
      { exact: true },
    ),
  ).toHaveCount(0);
  for (const name of [
    "Traffic quota exceeded",
    "RX quota · Annual",
    "Accumulated archive",
  ]) {
    await expect(
      cardNamed(privateGrid, name).locator(".vpsMonitorTraffic > small"),
    ).toHaveCount(0);
  }
  await expect(
    cardNamed(privateGrid, "Accumulated archive").locator(
      ".vpsMonitorTrafficTrack",
    ),
  ).toBeVisible();
  await expectEqualRowHeight(
    cardNamed(privateGrid, "Accumulated archive").locator(".vpsMonitorTraffic"),
    cardNamed(privateGrid, "TX quota · Unlimited").locator(
      ".vpsMonitorTraffic",
    ),
  );
  await expectHeadingSidePortSpeed(
    cardNamed(privateGrid, "Total quota · Monthly").locator(
      ".vpsMonitorTraffic",
    ),
  );
  await expectPingLayout(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    "Ping · Review healthy gateway",
    ["21.5 ms", "0% loss"],
  );
  await expectPingLayout(
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    "Ping",
    ["Unconfigured"],
  );
  await expectEqualRowHeight(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
  );
  await expectEqualRowHeight(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "RX quota · Annual").locator(".vpsMonitorPing"),
  );
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
  await expect(
    cardNamed(privateGrid, "RX quota · Annual").getByText(
      "Intermittent packet loss",
      { exact: true },
    ),
  ).toBeVisible();
  await expectEqualRowHeight(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
  );
  await expectHeadingSidePortSpeed(
    cardNamed(privateGrid, "Total quota · Monthly").locator(
      ".vpsMonitorTraffic",
    ),
  );
  await expectPingLayout(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    "Ping · Review healthy gateway",
    ["21.5 ms", "0% loss"],
  );
  await expectPingLayout(
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    "Ping",
    ["Unconfigured"],
  );
  await expectEqualRowHeight(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "RX quota · Annual").locator(".vpsMonitorPing"),
  );
  await expectComfortablePingEvidenceSlots(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "RX quota · Annual").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    "Intermittent packet loss",
  );
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
  await expect(
    page.getByRole("combobox", { name: "Filter shared VPSs by status" }),
  ).not.toHaveAttribute("title", /.+/);
  const visibleShareSecret = visibleShareFragment.split("/").at(-1) ?? "";
  expect(visibleShareSecret).not.toBe("");
  expect(
    await page
      .locator("[title]")
      .evaluateAll(
        (elements, secret) =>
          elements.some((element) =>
            (element.getAttribute("title") ?? "").includes(secret),
          ),
        visibleShareSecret,
      ),
  ).toBe(false);

  const sharedDensity = page.getByLabel("Shared view density");
  await sharedDensity.getByRole("button", { name: "Compact" }).click();
  await expect(sharedGrid).toHaveAttribute("data-density", "compact");
  const sharedUnconfigured = publicCardNamed(
    sharedGrid,
    "Unconfigured traffic",
  );
  await expect(
    sharedUnconfigured.locator(".publicMonitoringTraffic.unconfigured"),
  ).toBeVisible();
  await expect(
    sharedUnconfigured.locator(".vpsMonitorMetricTrack.missing"),
  ).toBeVisible();
  const sharedRxPingDiagnostic = publicCardNamed(
    sharedGrid,
    "RX quota · Annual",
  ).locator(".publicMonitoringPing > small:not(.vpsMonitorRowHeading)");
  await expect(sharedRxPingDiagnostic).toHaveCount(0);
  for (const name of [
    "Traffic quota exceeded",
    "RX quota · Annual",
    "Accumulated archive",
  ]) {
    await expect(
      publicCardNamed(sharedGrid, name).locator(
        ".publicMonitoringTraffic > small",
      ),
    ).toHaveCount(0);
  }
  await expect(
    publicCardNamed(sharedGrid, "Accumulated archive").locator(
      ".publicMonitoringTraffic .vpsMonitorMetricTrack",
    ),
  ).toBeVisible();
  await expectEqualRowHeight(
    publicCardNamed(sharedGrid, "Accumulated archive").locator(
      ".publicMonitoringTraffic",
    ),
    publicCardNamed(sharedGrid, "TX quota · Unlimited").locator(
      ".publicMonitoringTraffic",
    ),
  );
  await expectHeadingSidePortSpeed(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringTraffic",
    ),
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    "Ping · Review healthy gateway",
    ["21.5 ms", "0.0% loss"],
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    "Ping",
    ["Unconfigured"],
  );
  await expectEqualRowHeight(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
  );
  await expectEqualRowHeight(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "RX quota · Annual").locator(
      ".publicMonitoringPing",
    ),
  );
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
  await expect(sharedRxPingDiagnostic).toHaveText("Primary Ping degraded");
  await expect(sharedRxPingDiagnostic).toBeVisible();
  await expectHeadingSidePortSpeed(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringTraffic",
    ),
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    "Ping · Review healthy gateway",
    ["21.5 ms", "0.0% loss"],
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    "Ping",
    ["Unconfigured"],
  );
  await expectEqualRowHeight(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
  );
  await expectEqualRowHeight(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "RX quota · Annual").locator(
      ".publicMonitoringPing",
    ),
  );
  await expectComfortablePingEvidenceSlots(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "RX quota · Annual").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    "Primary Ping degraded",
  );
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
  await expect(
    detail.getByRole("region", { name: "Traffic volume chart" }),
  ).toBeVisible();
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
  await expect(
    hiddenSharedGrid.locator('[data-fact-kind="billing"]'),
  ).toHaveCount(0);
  await page
    .getByLabel("Shared view density")
    .getByRole("button", { name: "Compact" })
    .click();
  await expect(hiddenSharedGrid).toHaveAttribute("data-density", "compact");
  await capture(page, "shared-monitor-billing-hidden-compact.png");

  expect([...realApiRequests]).toEqual(
    expect.arrayContaining([
      "/api/v1/monitoring/cards",
      expect.stringMatching(
        /^\/api\/v1\/public\/monitoring-shares\/[0-9a-f-]+\/bootstrap$/,
      ),
      expect.stringMatching(
        /^\/api\/v1\/public\/monitoring-shares\/[0-9a-f-]+\/data$/,
      ),
    ]),
  );
  expect(serverErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

async function authenticate(page: Page) {
  const consoleShell = page.locator(".shell");
  const signIn = page.getByRole("heading", { exact: true, name: "Sign in" });
  await expect(consoleShell.or(signIn).first()).toBeVisible({
    timeout: 30_000,
  });
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
  const exceeded = cardNamed(grid, "Traffic quota exceeded");
  const rx = cardNamed(grid, "RX quota · Annual");
  const tx = cardNamed(grid, "TX quota · Unlimited");
  const noReset = cardNamed(grid, "Accumulated archive");
  const emptyRates = cardNamed(grid, "Rates intentionally empty");
  const unconfigured = cardNamed(grid, "Unconfigured traffic");
  const noPrimary = cardNamed(grid, "No primary Ping");

  for (const card of [
    total,
    exceeded,
    rx,
    tx,
    noReset,
    emptyRates,
    unconfigured,
    noPrimary,
  ]) {
    await expect(card).toBeVisible();
  }
  await expect(total.locator(".vpsMonitorTraffic")).toContainText("· Total ·");
  await expect(total.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews day 14",
  );
  await expectExceededTraffic(exceeded.locator(".vpsMonitorTraffic"));
  await expect(rx.locator(".vpsMonitorTraffic")).toContainText("· RX ·");
  await expect(rx.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews 06-15",
  );
  await expect(tx.locator(".vpsMonitorTraffic")).toContainText(
    "/ Unlimited · TX",
  );
  await expect(tx.locator(".unlimitedTrafficTrack")).toBeVisible();
  await expect(
    noReset.locator(".vpsMonitorTraffic .vpsMonitorRowHeading"),
  ).toHaveText("Traffic");
  await expect(noReset.locator(".vpsMonitorTraffic")).not.toContainText(
    "No reset",
  );
  await expect(emptyRates.locator(".vpsMonitorFlowFacts strong")).toHaveText([
    "-",
    "-",
  ]);
  await expect(
    unconfigured.locator('[data-fact-kind="billing"] strong'),
  ).toHaveText("-");
  await expect(unconfigured).not.toContainText("N/A");
  await expect(noPrimary.locator(".vpsMonitorPing")).toContainText(
    "Unconfigured",
  );
}

async function assertSharedFixtureSemantics(grid: ReturnType<Page["locator"]>) {
  const total = publicCardNamed(grid, "Total quota · Monthly");
  const exceeded = publicCardNamed(grid, "Traffic quota exceeded");
  const rx = publicCardNamed(grid, "RX quota · Annual");
  const tx = publicCardNamed(grid, "TX quota · Unlimited");
  const noReset = publicCardNamed(grid, "Accumulated archive");
  const emptyRates = publicCardNamed(grid, "Rates intentionally empty");
  const unconfigured = publicCardNamed(grid, "Unconfigured traffic");
  const noPrimary = publicCardNamed(grid, "No primary Ping");

  for (const card of [
    total,
    exceeded,
    rx,
    tx,
    noReset,
    emptyRates,
    unconfigured,
    noPrimary,
  ]) {
    await expect(card).toBeVisible();
  }
  await expect(total.locator(".publicMonitoringTraffic")).toContainText(
    "· Total ·",
  );
  await expect(total.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews day 14",
  );
  await expectExceededTraffic(exceeded.locator(".publicMonitoringTraffic"));
  await expect(rx.locator(".publicMonitoringTraffic")).toContainText("· RX ·");
  await expect(rx.locator('[data-fact-kind="billing"]')).toContainText(
    "Renews 06-15",
  );
  await expect(tx.locator(".publicMonitoringTraffic")).toContainText(
    "/ Unlimited · TX",
  );
  await expect(tx.locator(".unlimitedTrafficTrack")).toBeVisible();
  await expect(
    noReset.locator(".publicMonitoringTraffic .vpsMonitorRowHeading"),
  ).toHaveText("Traffic");
  await expect(noReset.locator(".publicMonitoringTraffic")).not.toContainText(
    "No reset",
  );
  await expect(emptyRates.locator(".vpsMonitorFlowFacts strong")).toHaveText([
    "-",
    "-",
  ]);
  await expect(
    unconfigured.locator('[data-fact-kind="billing"] strong'),
  ).toHaveText("-");
  await expect(unconfigured.locator(".publicMonitoringTraffic")).toHaveClass(
    /unconfigured/,
  );
  await expect(unconfigured).not.toContainText("N/A");
  await expect(noPrimary.locator(".publicMonitoringPing")).toContainText(
    "Unconfigured",
  );
}

async function capture(page: Page, filename: string) {
  await page.screenshot({
    animations: "disabled",
    fullPage: true,
    path: join(outputDir, filename),
  });
}

async function expectEqualRowHeight(
  configured: ReturnType<Page["locator"]>,
  unconfigured: ReturnType<Page["locator"]>,
) {
  await expect
    .poll(async () => {
      const configuredBox = await configured.boundingBox();
      const unconfiguredBox = await unconfigured.boundingBox();
      if (!configuredBox || !unconfiguredBox) return Number.POSITIVE_INFINITY;
      return Math.abs(configuredBox.height - unconfiguredBox.height);
    })
    .toBeLessThanOrEqual(1);
}

async function expectComfortablePingEvidenceSlots(
  healthy: ReturnType<Page["locator"]>,
  degraded: ReturnType<Page["locator"]>,
  unconfigured: ReturnType<Page["locator"]>,
  problem: string,
) {
  const healthySlot = healthy.locator(":scope > .vpsMonitorPingDetail");
  const degradedSlot = degraded.locator(":scope > .vpsMonitorPingDetail");
  const unconfiguredSlot = unconfigured.locator(
    ":scope > .vpsMonitorPingDetail",
  );
  await expect(healthySlot).toHaveAttribute("aria-hidden", "true");
  await expect(healthySlot).toHaveText("");
  await expect(unconfiguredSlot).toHaveAttribute("aria-hidden", "true");
  await expect(unconfiguredSlot).toHaveText("");
  await expect(degradedSlot).not.toHaveAttribute("aria-hidden", "true");
  await expect(degradedSlot).toHaveText(problem);
  await expect(degraded).toHaveAttribute("title", new RegExp(problem, "i"));
}

async function expectHeadingSidePortSpeed(row: ReturnType<Page["locator"]>) {
  await expect
    .poll(async () =>
      row.evaluate((element) => {
        const heading = element.querySelector<HTMLElement>(
          ".vpsMonitorRowHeading",
        );
        const speed = element.querySelector<HTMLElement>(
          ".publicMonitoringPortSpeed",
        );
        const value = element.querySelector<HTMLElement>(
          ".vpsMonitorTrafficQuota",
        );
        const track = element.querySelector<HTMLElement>(
          ".vpsMonitorTrafficTrack, :scope > .vpsMonitorMetricTrack",
        );
        if (!heading || !speed || !track || !value) return false;
        const headingBox = heading.getBoundingClientRect();
        const speedBox = speed.getBoundingClientRect();
        const trackBox = track.getBoundingClientRect();
        const valueBox = value.getBoundingClientRect();
        return (
          Math.abs(
            speedBox.top +
              speedBox.height / 2 -
              (headingBox.top + headingBox.height / 2),
          ) <= 2 &&
          trackBox.top >= headingBox.bottom - 1 &&
          valueBox.top >= trackBox.bottom - 1
        );
      }),
    )
    .toBe(true);
}

async function expectPingLayout(
  row: ReturnType<Page["locator"]>,
  headingText: string,
  evidence: string[],
) {
  const heading = row.locator(".vpsMonitorRowHeading");
  const chart = row.locator(
    ":scope > .vpsMonitorPingVisual, :scope > .vpsMonitorSparkline",
  );
  const evidenceValues = row.locator(".vpsMonitorPingEvidence > strong");
  await expect(heading).toHaveText(headingText);
  await expect(chart).toBeVisible();
  await expect(evidenceValues).toHaveText(evidence);
  const emptyChart = row.locator(".vpsMonitorSparkline.empty");
  if (await emptyChart.count()) {
    await expect(emptyChart).not.toHaveAttribute("title", /\S/);
    await expect(emptyChart).not.toHaveAttribute("role", /\S/);
  }
  await expect
    .poll(() =>
      row.evaluate((element) => {
        const headingElement = element.querySelector<HTMLElement>(
          ".vpsMonitorRowHeading",
        );
        const chartElement = element.querySelector<HTMLElement>(
          ":scope > .vpsMonitorPingVisual, :scope > .vpsMonitorSparkline",
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
        const headingBox = headingElement.getBoundingClientRect();
        const chartBox = chartElement.getBoundingClientRect();
        const evidenceBox = evidenceElement.getBoundingClientRect();
        const headingStyle = getComputedStyle(headingElement);
        return (
          headingStyle.overflow !== "hidden" &&
          headingStyle.textOverflow !== "ellipsis" &&
          evidenceItems.every((item) => {
            const style = getComputedStyle(item);
            return (
              style.overflow !== "hidden" && style.textOverflow !== "ellipsis"
            );
          }) &&
          chartBox.top >= headingBox.bottom - 1 &&
          evidenceBox.top >= chartBox.bottom - 1
        );
      }),
    )
    .toBe(true);
}

async function expectExceededTraffic(traffic: ReturnType<Page["locator"]>) {
  await expect(traffic).toHaveClass(/(?:exceeded|overQuota)/);
  await expect(traffic).toContainText(/120(?:\.0)?%/);
  const meter = traffic.getByRole("meter");
  await expect(meter).toHaveAttribute("aria-valuenow", "100");
  await expect(meter).toHaveAttribute(
    "aria-valuetext",
    /120(?:\.0)?(?: percent|%)/,
  );
  const fill = meter.locator(":scope > span");
  await expect(fill).toBeVisible();
  await expect
    .poll(() =>
      fill.evaluate((element) => getComputedStyle(element).backgroundColor),
    )
    .toBe("rgb(249, 171, 0)");
}
