import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

test.skip(
  process.env.VPSMAN_FRONTEND_PRODUCTION_TEST !== "1",
  "the Home bootstrap request contract is verified against a production bundle",
);

test.beforeEach(({}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the bootstrap request contract is viewport independent",
  );
});

async function trackedGetPaths(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
    };
    return (trackedWindow.__vpsmanFetchRequests ?? [])
      .filter((request) => request.method === "GET")
      .map((request) => new URL(request.url, window.location.href).pathname);
  });
}

async function clearTrackedRequests(page: Page): Promise<void> {
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      }
    ).__vpsmanFetchRequests = [];
  });
}

test("initial Home hydrates from one request without changing explicit loaders", async ({
  page,
}) => {
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);

  await expect(
    page.getByRole("heading", { name: "Fleet command home", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Home fleet posture")).toContainText(
    "0/3 visible live",
  );
  await expect
    .poll(() => trackedGetPaths(page))
    .toEqual(["/api/v1/home/snapshot"]);
  await page.waitForTimeout(300);
  expect(await trackedGetPaths(page)).toEqual(["/api/v1/home/snapshot"]);

  await clearTrackedRequests(page);
  await page.locator('button[title="Refresh dashboard telemetry"]').click();
  await expect
    .poll(() => trackedGetPaths(page))
    .toEqual(["/api/v1/dashboard/overview"]);

  await clearTrackedRequests(page);
  await openConsoleSubpage(page, "Jobs", "History");
  const jobsLoaderPaths = [
    "/api/v1/jobs",
    "/api/v1/job-approvals",
    "/api/v1/job-rollouts",
    "/api/v1/agent-update-releases",
    "/api/v1/process-supervisor/inventory",
    "/api/v1/file-transfers",
    "/api/v1/file-transfer-sources",
    "/api/v1/terminal-sessions",
    "/api/v1/server-jobs",
    "/api/v1/command-templates",
  ];
  await expect
    .poll(async () => {
      const paths = await trackedGetPaths(page);
      return jobsLoaderPaths.map(
        (expected) => paths.filter((path) => path === expected).length,
      );
    })
    .toEqual(jobsLoaderPaths.map(() => 1));
});

test("one failed Home source stays local without launching legacy fallbacks", async ({
  page,
}) => {
  await installConsoleApiMock(page, {
    homeSnapshotSourceFailure: "monitoring_cards",
    storedAuthSession: true,
  });
  await page.goto("/");
  await waitForConsoleShell(page);

  await expect(
    page.getByRole("heading", { name: "Fleet command home", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Home fleet posture")).toContainText(
    "0/3 visible live",
  );
  await expect(
    page.getByRole("alert").filter({
      hasText: "Monitoring cards: home_snapshot_monitoring_cards_unavailable",
    }),
  ).toBeVisible();
  await expect
    .poll(() => trackedGetPaths(page))
    .toEqual(["/api/v1/home/snapshot"]);
});
