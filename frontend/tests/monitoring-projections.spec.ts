import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { waitForConsoleShell } from "./support/consoleNavigation";

async function selectSection(page: Page, name: "Resources" | "Ping") {
  const detail = page.getByLabel("Canonical VPS detail");
  const selector = detail
    .locator(".detailTabSelect:visible")
    .getByLabel("VPS detail section");
  const tab = detail
    .locator(".detailTabs:visible")
    .getByRole("tab", { name, exact: true });
  await expect(selector.or(tab)).toBeVisible();
  if ((await selector.count()) === 1) {
    await selector.selectOption(name);
  } else {
    await tab.click();
  }
}

async function monitoringQueries(page: Page) {
  return page.evaluate(() => {
    const requests =
      (
        window as typeof window & {
          __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
        }
      ).__vpsmanFetchRequests ?? [];
    return requests
      .filter((request) => request.method === "GET")
      .map((request) => new URL(request.url, window.location.href))
      .filter(
        (url) => url.pathname === "/api/v1/clients/agent-sfo-01/monitoring",
      )
      .map((url) => ({
        points: url.searchParams.get("points"),
        projection: url.searchParams.get("projection"),
        window: url.searchParams.get("window"),
      }));
  });
}

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/#/fleet/instance-detail/agent-sfo-01");
  await waitForConsoleShell(page);
});

test("VPS Resources and Ping request only their visible domains and preserve range and refresh", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await selectSection(page, "Resources");
  const monitoring = page.getByRole("region", {
    name: "Monitoring history for edge-sfo-01",
  });
  await expect(monitoring).toBeVisible();
  await expect
    .poll(() => monitoringQueries(page))
    .toEqual([{ points: "720", projection: "resources", window: "15m" }]);
  for (const name of [
    /CPU utilization monitoring history\. Latest values: CPU used 29.0%/,
    /Network RX \/ TX monitoring history\. Latest values: RX rate/,
    /Traffic volume monitoring history\. Latest values: RX volume/,
  ]) {
    await expect(monitoring.getByRole("figure", { name })).toBeVisible();
  }
  const interfaces = monitoring.getByLabel("Current interface telemetry");
  await expect(interfaces.locator(".vpsMonitoringPingTarget")).toHaveCount(3);
  for (const name of ["eth0", "wg0", "tunab"]) {
    await expect(interfaces).toContainText(name);
  }
  await expect(
    monitoring.locator(".vpsMonitoringTrafficSummary"),
  ).toContainText("Observed RX");
  await expect(
    monitoring.getByLabel("Current Ping target evidence"),
  ).toHaveCount(0);
  await monitoring
    .getByRole("button", { name: "Last hour", exact: true })
    .click();
  await expect
    .poll(async () => (await monitoringQueries(page)).at(-1))
    .toEqual({
      points: "720",
      projection: "resources",
      window: "1h",
    });

  await selectSection(page, "Ping");
  const targets = monitoring.getByLabel("Current Ping target evidence");
  await expect(targets).toContainText("Singapore gateway");
  await expect(targets).toContainText("Cloudflare DNS");
  await expect(targets.locator(".vpsMonitoringPingTarget")).toHaveCount(2);
  await expect(
    monitoring.getByRole("button", { name: "Last hour", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(interfaces).toHaveCount(0);
  await expect(monitoring.locator(".vpsMonitoringTrafficCycle")).toHaveCount(0);
  await expect
    .poll(async () => (await monitoringQueries(page)).at(-1))
    .toEqual({
      points: "720",
      projection: "ping",
      window: "1h",
    });
  const beforeRefresh = (await monitoringQueries(page)).length;
  await monitoring
    .getByRole("button", { name: "Refresh", exact: true })
    .click();
  await expect
    .poll(async () => (await monitoringQueries(page)).length)
    .toBe(beforeRefresh + 1);
  await expect(targets).toContainText("Singapore gateway");
  expect((await monitoringQueries(page)).at(-1)).toEqual({
    points: "720",
    projection: "ping",
    window: "1h",
  });

  await selectSection(page, "Resources");
  await expect(interfaces).toContainText("tunab");
  await expect(
    monitoring.locator(".vpsMonitoringTrafficSummary"),
  ).toContainText("Observed RX");
  await expect
    .poll(async () => (await monitoringQueries(page)).at(-1))
    .toEqual({
      points: "720",
      projection: "resources",
      window: "1h",
    });
  expect(errors).toEqual([]);
});

test("a late Resources response cannot replace the selected Ping projection", async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixtureFetch = window.fetch;
    const trackedWindow = window as typeof window & {
      __releaseResourceProjection?: () => void;
      __resourceProjectionReleased?: boolean;
    };
    window.fetch = async (input, init) => {
      const response = await fixtureFetch(input, init);
      const url = new URL(
        input instanceof Request ? input.url : String(input),
        location.href,
      );
      if (
        url.pathname.endsWith("/monitoring") &&
        url.searchParams.get("projection") === "resources"
      ) {
        await new Promise<void>((resolve) => {
          trackedWindow.__releaseResourceProjection = resolve;
        });
        trackedWindow.__resourceProjectionReleased = true;
      }
      return response;
    };
  });
  await selectSection(page, "Resources");
  await expect
    .poll(() => monitoringQueries(page))
    .toEqual([{ points: "720", projection: "resources", window: "15m" }]);
  await selectSection(page, "Ping");
  const targets = page.getByLabel("Current Ping target evidence");
  await expect(targets).toContainText("Singapore gateway");
  await page.evaluate(() => {
    (
      window as typeof window & { __releaseResourceProjection?: () => void }
    ).__releaseResourceProjection?.();
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as typeof window & { __resourceProjectionReleased?: boolean })
            .__resourceProjectionReleased,
      ),
    )
    .toBe(true);
  await expect(targets).toContainText("Singapore gateway");
  await expect(page.getByLabel("Current interface telemetry")).toHaveCount(0);
});
