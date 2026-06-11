import { expect, test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";

const SCREENSHOT_DIR = join(
  process.env.VPSMAN_SCREENSHOT_DIR ?? join(process.cwd(), "..", "tmp"),
);

interface ScreenshotEntry {
  view: string;
  subpage?: string;
  heading: string;
  id: string;
}

const allViews: ScreenshotEntry[] = [
  { view: "Dashboard", heading: "Dashboard", id: "01-dashboard-overview" },
  { view: "Fleet", heading: "Fleet overview", id: "02-fleet-instances" },
  {
    view: "Fleet",
    subpage: "Alerts",
    heading: "Fleet alerts",
    id: "03-fleet-alerts",
  },
  {
    view: "Fleet",
    subpage: "Alert policies",
    heading: "Alert policies",
    id: "04-fleet-alert-policies",
  },
  {
    view: "Fleet",
    subpage: "Notifications",
    heading: "Notification channels",
    id: "05-fleet-notifications",
  },
  { view: "Config", heading: "Config overview", id: "06-config-overview" },
  {
    view: "Config",
    subpage: "Rules",
    heading: "Config rules",
    id: "07-config-rules",
  },
  {
    view: "Config",
    subpage: "Bulk apply",
    heading: "Bulk hot config",
    id: "08-config-bulk",
  },
  {
    view: "Config",
    subpage: "Single VPS",
    heading: "Single VPS config",
    id: "09-config-single",
  },
  {
    view: "Config",
    subpage: "Templates",
    heading: "Data-source presets",
    id: "10-config-templates",
  },
  {
    view: "Config",
    subpage: "Status",
    heading: "Active source status",
    id: "11-config-status",
  },
  { view: "Tags", heading: "Tags", id: "12-tags-registry" },
  {
    view: "Tags",
    subpage: "Assignments",
    heading: "Tag assignments",
    id: "13-tags-assignments",
  },
  { view: "Tags", subpage: "Bulk", heading: "Bulk tags", id: "14-tags-bulk" },
  { view: "Jobs", heading: "Job history", id: "15-jobs-history" },
  {
    view: "Jobs",
    subpage: "Dispatch",
    heading: "Command dispatch",
    id: "16-jobs-dispatch",
  },
  {
    view: "Jobs",
    subpage: "Files",
    heading: "VPS file browser",
    id: "17-jobs-files",
  },
  {
    view: "Jobs",
    subpage: "Multi files",
    heading: "Multi-file actions",
    id: "18-jobs-multi-files",
  },
  {
    view: "Jobs",
    subpage: "Updates",
    heading: "Agent updates",
    id: "19-jobs-updates",
  },
  {
    view: "Jobs",
    subpage: "Transfer history",
    heading: "File transfer history",
    id: "20-jobs-transfers",
  },
  {
    view: "Jobs",
    subpage: "Terminal sessions",
    heading: "Terminal sessions",
    id: "21-jobs-terminal",
  },
  {
    view: "Jobs",
    subpage: "Processes",
    heading: "Process supervisor",
    id: "22-jobs-processes",
  },
  {
    view: "Jobs",
    subpage: "Schedule runs",
    heading: "Schedule runs",
    id: "23-jobs-schedule-runs",
  },
  { view: "Schedules", heading: "Schedules", id: "24-schedules-registry" },
  { view: "Topology", heading: "Topology graph", id: "25-topology-graph" },
  {
    view: "Topology",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "26-topology-plans",
  },
  {
    view: "Topology",
    subpage: "Apply / rollback",
    heading: "Network apply",
    id: "27-topology-apply",
  },
  {
    view: "Topology",
    subpage: "Promotion",
    heading: "Tunnel promotion",
    id: "28-topology-promotion",
  },
  {
    view: "Topology",
    subpage: "Evidence",
    heading: "Topology evidence",
    id: "29-topology-evidence",
  },
  {
    view: "Topology",
    subpage: "OSPF",
    heading: "vpsman / Topology / OSPF",
    id: "30-topology-ospf",
  },
  { view: "Backups", heading: "Backup requests", id: "31-backups-requests" },
  {
    view: "Backups",
    subpage: "Policies",
    heading: "Backup policies",
    id: "32-backups-policies",
  },
  {
    view: "Backups",
    subpage: "Artifacts",
    heading: "Backup artifacts",
    id: "33-backups-artifacts",
  },
  {
    view: "Backups",
    subpage: "Restore",
    heading: "Restore operations",
    id: "34-backups-restore",
  },
  {
    view: "Backups",
    subpage: "Migration",
    heading: "Migration links",
    id: "35-backups-migration",
  },
  { view: "Audit", heading: "Audit log", id: "36-audit-events" },
  {
    view: "Audit",
    subpage: "Retention",
    heading: "History retention",
    id: "37-audit-retention",
  },
  { view: "Access", heading: "Access control", id: "38-access-overview" },
  {
    view: "Access",
    subpage: "Operators",
    heading: "Operators",
    id: "39-access-operators",
  },
  {
    view: "Access",
    subpage: "VPS keys",
    heading: "Gateway agent identities",
    id: "40-access-vps-keys",
  },
  {
    view: "Access",
    subpage: "Gateway",
    heading: "Gateway sessions",
    id: "41-access-gateway",
  },
  {
    view: "Access",
    subpage: "Privilege unlock",
    heading: "Privilege unlock",
    id: "42-access-privilege",
  },
  {
    view: "Preferences",
    heading: "Preferences",
    id: "43-preferences-operator",
  },
];

// Split into batches of 6 — each batch is a fresh page
const BATCH_SIZE = 6;
const batches: ScreenshotEntry[][] = [];
for (let i = 0; i < allViews.length; i += BATCH_SIZE) {
  batches.push(allViews.slice(i, i + BATCH_SIZE));
}

async function navigateAndScreenshot(
  page: import("@playwright/test").Page,
  entry: ScreenshotEntry,
  projectDir: string,
  projectName: string,
) {
  const label = entry.subpage ? `${entry.view} / ${entry.subpage}` : entry.view;

  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  await nav.getByRole("button", { name: entry.view, exact: true }).click();

  if (entry.subpage) {
    const subpageGroup = nav.getByLabel(`${entry.view} sections`);
    const subpageButton = subpageGroup.getByRole("button", {
      name: entry.subpage,
      exact: true,
    });
    if ((await subpageButton.count()) > 0) {
      await subpageButton.click();
    }
  }

  // Wait for heading or any main content
  try {
    await expect(
      page
        .locator(".consoleHeader")
        .getByRole("heading", { name: entry.heading, exact: true })
        .first(),
    ).toBeVisible({ timeout: 5_000 });
  } catch {
    try {
      await expect(
        page.getByText(entry.heading, { exact: true }).first(),
      ).toBeVisible({ timeout: 3_000 });
    } catch {
      await page.waitForTimeout(1_500);
    }
  }

  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(200);
  const horizontalOverflowPx = await page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  );
  expect(
    horizontalOverflowPx,
    `${label} page-level horizontal overflow`,
  ).toBeLessThanOrEqual(1);

  const filename = `${entry.id}-${projectName}.png`;
  const screenshotPath = join(projectDir, filename);
  await page.screenshot({ fullPage: true, path: screenshotPath });

  return {
    id: entry.id,
    view: entry.view,
    subpage: entry.subpage ?? null,
    heading: entry.heading,
    horizontalOverflowPx,
    screenshot: screenshotPath,
  };
}

const projectDir = join(SCREENSHOT_DIR, "desktop-chrome");

test.beforeAll(() => {
  mkdirSync(projectDir, { recursive: true });
});

// Install mock API before each test batch
test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

// Generate one test per batch
batches.forEach((batch, batchIndex) => {
  test(`screenshot batch ${batchIndex + 1} of ${batches.length} (${batch[0].id}–${batch[batch.length - 1].id})`, async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "structured screenshot capture uses the desktop navigation shell",
    );
    test.setTimeout(120_000);

    await page.goto("/");
    await expect(page.locator(".shell")).toBeVisible({ timeout: 15_000 });

    const results: Array<Record<string, unknown>> = [];

    for (const entry of batch) {
      try {
        const result = await navigateAndScreenshot(
          page,
          entry,
          projectDir,
          testInfo.project.name,
        );
        results.push(result);
        console.log(
          `[screenshot] OK  ${result.id} — ${entry.view}${entry.subpage ? ` / ${entry.subpage}` : ""}`,
        );
      } catch (error) {
        console.error(`[screenshot] ERR ${entry.id}: ${String(error)}`);
        // Try to capture error state
        try {
          const errPath = join(
            projectDir,
            `${entry.id}-${testInfo.project.name}-error.png`,
          );
          await page.screenshot({ fullPage: true, path: errPath });
          results.push({
            id: entry.id,
            view: entry.view,
            subpage: entry.subpage ?? null,
            heading: entry.heading,
            screenshot: errPath,
            error: String(error),
          });
        } catch {
          results.push({
            id: entry.id,
            view: entry.view,
            subpage: entry.subpage ?? null,
            heading: entry.heading,
            error: String(error),
          });
        }
      }
    }

    // Write per-batch manifest
    writeFileSync(
      join(projectDir, `manifest-batch-${batchIndex + 1}.json`),
      `${JSON.stringify({ generated_by: "structured-screenshots", batch: batchIndex + 1, total: results.length, views: results }, null, 2)}\n`,
    );
  });
});
