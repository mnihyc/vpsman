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
  await openConsoleSubpage(page, "System", "Preferences");

  const fleetSearch = page.getByRole("combobox", { name: "Search fleet" });
  await fleetSearch.fill("v");
  const fleetSuggestions = page.getByRole("listbox", {
    name: "Search fleet suggestions",
  });
  await fleetSuggestions.getByRole("option", { name: /VPS rules…/ }).click();
  await expect(
    fleetSuggestions.getByText("Product name", { exact: true }),
  ).toBeVisible({ timeout: 30_000 });
  await capture(page, "private-vps-rules-dropdown.png");
  await fleetSearch.fill("");
  await fleetSearch.press("Escape");

  await page
    .getByLabel("Fleet table location", { exact: true })
    .selectOption("country_region");
  await capture(page, "fleet-location-personal-preference.png");
  const savePreferences = page.getByRole("button", {
    name: "Save preferences",
    exact: true,
  });
  if (await savePreferences.isEnabled()) await savePreferences.click();
  await expect(
    page.locator(".preferencesPanel").getByText("Saved", { exact: true }),
  ).toBeVisible();

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
  await expectInlineTrafficEvidence(
    cardNamed(privateGrid, "Accumulated archive").locator(".vpsMonitorTraffic"),
  );
  await expectHeadingSidePortSpeed(
    cardNamed(privateGrid, "Total quota · Monthly").locator(
      ".vpsMonitorTraffic",
    ),
  );
  await expectPingLayout(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    "Ping · Review healthy gateway",
    ["20.5 ms", "0% loss"],
  );
  await expectPingLayout(
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    "Ping",
    ["Unconfigured"],
  );
  await expectShorterRow(
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
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
  await expectShorterRow(
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
  );
  await expectHeadingSidePortSpeed(
    cardNamed(privateGrid, "Total quota · Monthly").locator(
      ".vpsMonitorTraffic",
    ),
  );
  await expectComfortableTrafficEvidence(
    cardNamed(privateGrid, "Total quota · Monthly").locator(
      ".vpsMonitorTraffic",
    ),
    /↓ .+ · ↑ /,
  );
  await expectComfortableTrafficEvidence(
    cardNamed(privateGrid, "Accumulated archive").locator(".vpsMonitorTraffic"),
    /↓ .+ · ↑ /,
  );
  await expectPingLayout(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    "Ping · Review healthy gateway",
    ["20.5 ms", "0% loss"],
  );
  await expectPingLayout(
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    "Ping",
    ["Unconfigured"],
  );
  await expectShorterRow(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "RX quota · Annual").locator(".vpsMonitorPing"),
  );
  await expectComfortablePingDiagnostics(
    cardNamed(privateGrid, "Total quota · Monthly").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "RX quota · Annual").locator(".vpsMonitorPing"),
    cardNamed(privateGrid, "No primary Ping").locator(".vpsMonitorPing"),
    "Intermittent packet loss",
  );
  const privateComfortableIdentity = cardNamed(
    privateGrid,
    "Total quota · Monthly",
  ).locator(".vpsMonitorCardMain > small");
  await expect(privateComfortableIdentity).toHaveText(
    "reviewcloud · LN.V2.HKGv3 / SG",
  );
  await expect(privateComfortableIdentity).not.toContainText("sin");
  await expect(privateComfortableIdentity).toHaveAttribute(
    "title",
    "reviewcloud · LN.V2.HKGv3 · SG · sin",
  );
  await expect(
    cardNamed(privateGrid, "Total quota · Monthly").locator(
      ".countryFlagGlyph",
    ),
  ).toHaveText("🇸🇬");
  await capture(page, "private-monitor-comfortable.png");

  await cardNamed(privateGrid, "Total quota · Monthly").click();
  const canonicalIdentity = page.getByLabel("Selected VPS identity");
  await expect(canonicalIdentity).toBeVisible({ timeout: 30_000 });
  await expect(canonicalIdentity.getByLabel("Provider")).toHaveText(
    "Providerreviewcloud · LN.V2.HKGv3",
  );
  const canonicalLocation = canonicalIdentity.getByLabel("VPS location");
  await expect(canonicalLocation).toContainText("SG");
  await expect(canonicalLocation).toContainText("sin");
  await expect(canonicalLocation.locator(".countryFlag")).toBeVisible();
  await expect(canonicalIdentity.getByLabel("VPS tags")).not.toContainText(
    "provider:reviewcloud",
  );
  await expect(canonicalIdentity.getByLabel("VPS tags")).not.toContainText(
    /country:|region:/,
  );
  await capture(page, "private-vps-detail.png");

  await openConsoleSubpage(page, "Fleet", "Instances");
  const instanceGrid = page.getByLabel("VPS instance records data grid");
  const monthlyRow = instanceGrid
    .locator(".gridBody [role=row]", { hasText: "Total quota · Monthly" })
    .first();
  const instanceHeaders = await instanceGrid
    .locator('[role="columnheader"]')
    .allTextContents();
  const locationIndex = instanceHeaders.findIndex((header) =>
    header.includes("Location"),
  );
  expect(locationIndex).toBeGreaterThanOrEqual(0);
  const locationCell = monthlyRow.getByRole("gridcell").nth(locationIndex);
  await expect(monthlyRow.locator(".instance strong")).toHaveAttribute(
    "title",
    "Total quota · Monthly (thly)",
  );
  await expect(locationCell.locator(".countryBadge > span").last()).toHaveText(
    "SG",
  );
  await expect(locationCell.locator(".fleetLocationValue > small")).toHaveText(
    "sin",
  );
  await expect(locationCell.locator(".fleetLocationValue")).toHaveAttribute(
    "title",
    "SG · sin",
  );
  const locationLines = await locationCell
    .locator(".countryBadge, .fleetLocationValue > small")
    .evaluateAll((elements) =>
      elements.map((element) => element.getBoundingClientRect().top),
    );
  expect(locationLines[1]).toBeGreaterThan(locationLines[0]);
  await monthlyRow.getByLabel("Expand VPS instance records row").click();
  const inlineDetail = instanceGrid
    .locator(".gridExpandedRow", { hasText: "review-total-monthly" })
    .first();
  const inlineProvider = inlineDetail
    .locator(".timeline")
    .filter({ has: page.getByText("Provider", { exact: true }) });
  await expect(inlineProvider).toContainText("reviewcloud · LN.V2.HKGv3");
  const inlineLocation = inlineDetail
    .locator(".timeline")
    .filter({ has: page.getByText("Location", { exact: true }) });
  await expect(inlineLocation).toContainText("SG");
  await expect(inlineLocation).toContainText("sin");
  await expect(inlineLocation.locator(".countryFlag")).toBeVisible();
  await expect(inlineDetail).not.toContainText("Contact evidence");
  const countryTag = inlineDetail.getByText("country:SG", { exact: true });
  const regionTag = inlineDetail.getByText("region:sin", { exact: true });
  await expect(countryTag).toBeVisible();
  await expect(regionTag).toBeVisible();
  const [countryTagBox, regionTagBox] = await Promise.all([
    countryTag.boundingBox(),
    regionTag.boundingBox(),
  ]);
  expect(countryTagBox).not.toBeNull();
  expect(regionTagBox).not.toBeNull();
  expect(Math.abs(regionTagBox!.y - countryTagBox!.y)).toBeLessThanOrEqual(1);
  expect(regionTagBox!.x).toBeGreaterThan(countryTagBox!.x);
  await capture(page, "private-vps-inline-detail.png");

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
  await expectInlineTrafficEvidence(
    publicCardNamed(sharedGrid, "Accumulated archive").locator(
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
    ["20.5 ms", "0.0% loss"],
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    "Ping",
    ["Unconfigured"],
  );
  await expectShorterRow(
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
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
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".vpsMonitorCardMain > small",
    ),
  ).toHaveText("Updated just now");
  await expect(
    publicCardNamed(sharedGrid, "Total quota · Monthly").getByLabel(
      "Shared identity context",
    ),
  ).toHaveText("reviewcloud · LN.V2.HKGv3 · SG");
  await expect(
    publicCardNamed(sharedGrid, "Total quota · Monthly").getByLabel(
      "Shared identity context",
    ),
  ).toHaveAttribute(
    "title",
    "Provider reviewcloud · Product LN.V2.HKGv3 · SG · sin",
  );
  await expect(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".countryFlagGlyph",
    ),
  ).toHaveText("🇸🇬");
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
  await expectComfortableTrafficEvidence(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringTraffic",
    ),
    /RX .+ · TX /,
  );
  await expectComfortableTrafficEvidence(
    publicCardNamed(sharedGrid, "Accumulated archive").locator(
      ".publicMonitoringTraffic",
    ),
    /RX .+ · TX /,
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    "Ping · Review healthy gateway",
    ["20.5 ms", "0.0% loss"],
  );
  await expectPingLayout(
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    "Ping",
    ["Unconfigured"],
  );
  await expectShorterRow(
    publicCardNamed(sharedGrid, "No primary Ping").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
  );
  await expectShorterRow(
    publicCardNamed(sharedGrid, "Total quota · Monthly").locator(
      ".publicMonitoringPing",
    ),
    publicCardNamed(sharedGrid, "RX quota · Annual").locator(
      ".publicMonitoringPing",
    ),
  );
  await expectComfortablePingDiagnostics(
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
  await expect(detail.locator(".publicMonitoringDetailHeader p")).toContainText(
    "reviewcloud · LN.V2.HKGv3 · SG · sin",
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

  await expect(total.locator(".vpsMonitorCompactProductContext")).toHaveCount(
    0,
  );
  await expect(total.locator(".vpsMonitorCardName")).toHaveAttribute(
    "title",
    "Total quota · Monthly · reviewcloud · LN.V2.HKGv3 · SG · sin",
  );
  await expect(rx.locator(".vpsMonitorCardName")).toHaveAttribute(
    "title",
    "RX quota · Annual · reviewcloud · Storage-Box 4 · DE · fra",
  );

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

  await expect(total.locator(".vpsMonitorCompactProductContext")).toHaveCount(
    0,
  );
  await expect(total.locator(".vpsMonitorCardName")).toHaveAttribute(
    "title",
    "Total quota · Monthly · reviewcloud · LN.V2.HKGv3 · SG · sin",
  );
  await expect(rx.locator(".vpsMonitorCardName")).toHaveAttribute(
    "title",
    "RX quota · Annual · reviewcloud · Storage-Box 4 · DE · fra",
  );

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

async function expectShorterRow(
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

async function expectInlineTrafficEvidence(row: ReturnType<Page["locator"]>) {
  const heading = row.locator(
    ".vpsMonitorTrafficHeading, .publicMonitoringTrafficHeading",
  );
  const value = heading.locator(":scope > .vpsMonitorTrafficQuota");
  await expect(value).toBeVisible();
  await expect(row.locator(":scope > .vpsMonitorTrafficQuota")).toHaveCount(0);
  await expect
    .poll(() =>
      row.evaluate((element) => {
        const headingElement = element.querySelector<HTMLElement>(
          ".vpsMonitorTrafficHeading, .publicMonitoringTrafficHeading",
        );
        const valueElement = headingElement?.querySelector<HTMLElement>(
          ":scope > .vpsMonitorTrafficQuota",
        );
        const trackElement = element.querySelector<HTMLElement>(
          ":scope > .vpsMonitorTrafficTrack, :scope > .vpsMonitorMetricTrack",
        );
        if (!headingElement || !valueElement || !trackElement) return false;
        const headingBox = headingElement.getBoundingClientRect();
        const valueBox = valueElement.getBoundingClientRect();
        const trackBox = trackElement.getBoundingClientRect();
        return (
          valueBox.right <= headingBox.right + 1 &&
          valueBox.top >= headingBox.top - 1 &&
          valueBox.bottom <= headingBox.bottom + 1 &&
          trackBox.top >= headingBox.bottom - 1
        );
      }),
    )
    .toBe(true);
}

async function expectComfortablePingDiagnostics(
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
  await expect(healthySlot).toHaveCount(0);
  await expect(unconfiguredSlot).toHaveCount(0);
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

async function expectComfortableTrafficEvidence(
  row: ReturnType<Page["locator"]>,
  observedPattern: RegExp,
) {
  const evidence = row.locator(":scope > .vpsMonitorTrafficEvidenceRow");
  const observed = evidence.locator(":scope > small");
  const quota = evidence.locator(":scope > .vpsMonitorTrafficQuota");
  await expect(evidence).toBeVisible();
  await expect(observed).toHaveText(observedPattern);
  await expect(observed).not.toContainText(/reset/i);
  await expect(quota).toContainText(" / ");
  await expect
    .poll(() =>
      evidence.evaluate((element) => {
        const observedElement =
          element.querySelector<HTMLElement>(":scope > small");
        const quotaElement = element.querySelector<HTMLElement>(
          ":scope > .vpsMonitorTrafficQuota",
        );
        if (!observedElement || !quotaElement) return false;
        const rowBox = element.getBoundingClientRect();
        const observedBox = observedElement.getBoundingClientRect();
        const quotaBox = quotaElement.getBoundingClientRect();
        return (
          observedBox.left >= rowBox.left - 1 &&
          quotaBox.right <= rowBox.right + 1 &&
          observedBox.right <= quotaBox.left + 1 &&
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
  const inlineEvidence =
    evidence.length === 1 && evidence[0] === "Unconfigured";
  await expect(heading).toHaveText(headingText);
  if (inlineEvidence) {
    await expect(chart).toHaveCount(0);
  } else {
    await expect(chart).toBeVisible();
  }
  await expect(evidenceValues).toHaveText(evidence);
  const emptyChart = row.locator(".vpsMonitorSparkline.empty");
  if (await emptyChart.count()) {
    await expect(emptyChart).not.toHaveAttribute("title", /\S/);
    await expect(emptyChart).not.toHaveAttribute("role", /\S/);
  }
  if (inlineEvidence) {
    await expect(row.locator(":scope > .vpsMonitorPingEvidence")).toHaveCount(
      0,
    );
  }
  await expect
    .poll(() =>
      row.evaluate((element, evidenceIsInline) => {
        const headingElement = element.querySelector<HTMLElement>(
          ".vpsMonitorRowHeading",
        );
        const headingContainer = element.querySelector<HTMLElement>(
          ".vpsMonitorPingHeading, .publicMonitoringPingHeading",
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
        if (!headingElement || !headingContainer || !evidenceElement)
          return false;
        const headingBox = headingElement.getBoundingClientRect();
        const evidenceBox = evidenceElement.getBoundingClientRect();
        const headingStyle = getComputedStyle(headingElement);
        const evidenceFits =
          headingStyle.overflow !== "hidden" &&
          headingStyle.textOverflow !== "ellipsis" &&
          evidenceItems.every((item) => {
            const style = getComputedStyle(item);
            return (
              style.overflow !== "hidden" &&
              style.textOverflow !== "ellipsis" &&
              style.textAlign === "right"
            );
          });
        if (!evidenceFits) return false;
        const headingContainerBox = headingContainer.getBoundingClientRect();
        if (evidenceIsInline) {
          const rowBox = element.getBoundingClientRect();
          return (
            chartElement === null &&
            evidenceElement.parentElement === headingContainer &&
            evidenceBox.right <= headingContainerBox.right + 1 &&
            evidenceBox.top >= headingContainerBox.top - 1 &&
            evidenceBox.bottom <= headingContainerBox.bottom + 1 &&
            Math.abs(rowBox.bottom - headingContainerBox.bottom) <= 1
          );
        }
        if (!chartElement) return false;
        const chartBox = chartElement.getBoundingClientRect();
        const chartEvidenceGap = evidenceBox.top - chartBox.bottom;
        return (
          getComputedStyle(evidenceElement).justifyContent === "flex-end" &&
          Math.abs(
            (evidenceItems.at(-1)?.getBoundingClientRect().right ?? 0) -
              evidenceBox.right,
          ) <= 1 &&
          chartBox.top >= headingBox.bottom - 1 &&
          evidenceBox.top >= chartBox.top - 1 &&
          chartEvidenceGap >= -5 &&
          chartEvidenceGap <= -1
        );
      }, inlineEvidence),
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
