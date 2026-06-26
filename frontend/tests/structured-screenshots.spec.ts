import { expect, test } from "@playwright/test";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";
import type { ActiveView } from "../src/types";

const SCREENSHOT_DIR = join(
  process.env.VPSMAN_SCREENSHOT_DIR ?? join(process.cwd(), "..", "tmp"),
);

interface ScreenshotEntry {
  view: ActiveView;
  subpage?: string;
  tab?: string;
  expandVpsRow?: string;
  detailTab?: string;
  prepare?: "alert-policy-editor" | "tunnel-plan-create" | "tunnel-plan-promotion" | "vps-rules-preview" | "webhook-rule-editor";
  requiredText?: string[];
  heading: string;
  id: string;
}

const releaseTopLevel: ActiveView[] = [
  "Home",
  "Fleet",
  "Remote Operations",
  "Jobs",
  "Automation",
  "Network",
  "Backups",
  "Config",
  "Observability",
  "Audit",
  "Access",
  "System",
];

const legacyTopLevel = ["Dashboard", "Tags", "Schedules", "Topology"];

test.describe.configure({ mode: "serial" });

const allViews: ScreenshotEntry[] = [
  {
    view: "Home",
    subpage: "Overview",
    heading: "Home",
    id: "01-home-overview",
    requiredText: ["Fleet command home", "Needs attention", "Recent activity"],
  },
  {
    view: "Fleet",
    subpage: "Instances",
    heading: "Fleet instances",
    id: "02-fleet-instances",
  },
  {
    view: "Fleet",
    subpage: "Instances",
    expandVpsRow: "edge-sfo-01",
    detailTab: "Config",
    heading: "Instance detail",
    id: "02b-fleet-instance-config-detail",
    requiredText: [
      "Config ownership",
      "Open per-VPS config",
      "Source templates",
      "VPS rules",
    ],
  },
  {
    view: "Fleet",
    subpage: "Monitor",
    heading: "Fleet monitor",
    id: "03-fleet-monitor",
    requiredText: ["VPS cards"],
  },
  {
    view: "Fleet",
    subpage: "Groups",
    heading: "Fleet groups",
    id: "04-fleet-groups",
    requiredText: ["Tags"],
  },
  {
    view: "Fleet",
    subpage: "Assignments",
    heading: "Group assignments",
    id: "05-fleet-group-assignments",
    requiredText: ["Tag assignments"],
  },
  {
    view: "Fleet",
    subpage: "Bulk groups",
    heading: "Bulk groups",
    id: "06-fleet-bulk-groups",
    requiredText: ["Bulk mutation", "Target preview"],
  },
  {
    view: "Fleet",
    subpage: "Alerts",
    heading: "Fleet alerts",
    id: "07-fleet-alerts",
  },
  {
    view: "Fleet",
    subpage: "Instance detail",
    heading: "Instance detail",
    id: "08-fleet-instance-detail",
  },
  {
    view: "Remote Operations",
    subpage: "Terminal",
    heading: "Terminal",
    id: "09-remote-operations-terminal",
    requiredText: ["Terminal sessions"],
  },
  {
    view: "Remote Operations",
    subpage: "Files",
    heading: "Files",
    id: "10-remote-operations-files",
    requiredText: ["File browser"],
  },
  {
    view: "Remote Operations",
    subpage: "Transfers",
    heading: "Transfers",
    id: "11-remote-operations-transfers",
    requiredText: ["File transfer sessions"],
  },
  {
    view: "Remote Operations",
    subpage: "Processes",
    heading: "Processes",
    id: "12-remote-operations-processes",
    requiredText: ["Process supervisor"],
  },
  {
    view: "Remote Operations",
    subpage: "Bulk files",
    heading: "Bulk files",
    id: "13-remote-operations-bulk-files",
  },
  {
    view: "Jobs",
    subpage: "History",
    heading: "Job history",
    id: "14-jobs-history",
  },
  {
    view: "Jobs",
    subpage: "Dispatch",
    heading: "Command dispatch",
    id: "15-jobs-dispatch",
  },
  {
    view: "Jobs",
    subpage: "Approvals",
    heading: "Approvals",
    id: "16-jobs-approvals",
  },
  {
    view: "Jobs",
    subpage: "Scheduled runs",
    heading: "Scheduled runs",
    id: "17-jobs-scheduled-runs",
  },
  {
    view: "Jobs",
    subpage: "Artifacts",
    heading: "Job artifacts",
    id: "18-jobs-artifacts",
  },
  {
    view: "Automation",
    subpage: "Schedules",
    heading: "Schedules",
    id: "19-automation-schedules",
  },
  {
    view: "Automation",
    subpage: "Runbooks",
    heading: "Runbooks",
    id: "20-automation-runbooks",
  },
  {
    view: "Automation",
    subpage: "Source templates",
    heading: "Source templates",
    id: "21-automation-source-templates",
  },
  {
    view: "Automation",
    subpage: "Agent updates",
    heading: "Agent updates",
    id: "22-automation-agent-updates",
  },
  {
    view: "Network",
    subpage: "Overview",
    heading: "Network overview",
    id: "23-network-overview",
  },
  {
    view: "Network",
    subpage: "Graph",
    heading: "Network graph",
    id: "24-network-graph",
    requiredText: ["Topology graph"],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25-network-tunnel-plans",
    requiredText: ["Create tunnel plan", "Promotion workflow", "Generated config"],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25b-network-tunnel-plans-create",
    prepare: "tunnel-plan-create",
    requiredText: ["Create tunnel plan", "Plan identity", "Endpoints"],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25c-network-tunnel-plans-promotion",
    prepare: "tunnel-plan-promotion",
    requiredText: ["Tunnel promotion", "Promotion diff workflow"],
  },
  {
    view: "Network",
    subpage: "Tests",
    heading: "Network tests",
    id: "26-network-tests",
  },
  {
    view: "Network",
    subpage: "OSPF",
    heading: "Network OSPF",
    id: "27-network-ospf",
  },
  {
    view: "Network",
    subpage: "Evidence",
    heading: "Network evidence",
    id: "28-network-evidence",
  },
  { view: "Backups", subpage: "Overview", heading: "Backup overview", id: "29-backups-overview" },
  {
    view: "Backups",
    subpage: "Requests",
    heading: "Backup requests",
    id: "30-backups-requests",
  },
  {
    view: "Backups",
    subpage: "Policies",
    heading: "Backup policies",
    id: "31-backups-policies",
  },
  {
    view: "Backups",
    subpage: "Artifacts",
    heading: "Backup artifacts",
    id: "32-backups-artifacts",
  },
  {
    view: "Backups",
    subpage: "Restore",
    heading: "Restore",
    id: "33-backups-restore",
  },
  {
    view: "Backups",
    subpage: "Migration",
    heading: "Migration",
    id: "34-backups-migration",
  },
  {
    view: "Config",
    subpage: "Overview",
    heading: "Runtime config overview",
    id: "35-config-overview",
  },
  {
    view: "Config",
    subpage: "Per-VPS",
    heading: "Per-VPS config",
    id: "36-config-per-vps",
  },
  {
    view: "Config",
    subpage: "Bulk patch",
    heading: "Bulk patch",
    id: "37-config-bulk-patch",
  },
  {
    view: "Config",
    subpage: "Templates",
    heading: "Templates",
    id: "38-config-templates",
  },
  {
    view: "Config",
    subpage: "Rules",
    heading: "VPS Rules",
    id: "39-config-rules",
    prepare: "vps-rules-preview",
    requiredText: [
      "Bulk rule editor",
      "Target VPS selector",
      "Set values",
      "Unset values",
      "Dry-run changed rows",
    ],
  },
  {
    view: "Observability",
    subpage: "Fleet metrics",
    heading: "Fleet metrics",
    id: "40-observability-fleet-metrics",
    requiredText: ["CPU load by VPS", "Top VPS", "Fleet grouping"],
  },
  {
    view: "Observability",
    subpage: "Network metrics",
    heading: "Network metrics",
    id: "41-observability-network-metrics",
    requiredText: ["Latency, loss, and speed", "Tunnel grouping", "Endpoint comparison", "Alert overlays"],
  },
  {
    view: "Observability",
    subpage: "Process metrics",
    heading: "Process metrics",
    id: "42-observability-process-metrics",
    requiredText: ["Long-term process history is not exposed by the backend yet."],
  },
  {
    view: "Observability",
    subpage: "Alerts",
    heading: "Alerts",
    id: "43-observability-alerts",
    requiredText: ["Alert policies", "Notification channels", "Notification deliveries"],
  },
  {
    view: "Observability",
    subpage: "Alerts",
    heading: "Alerts",
    id: "43b-observability-alerts-policy-editor",
    prepare: "alert-policy-editor",
    requiredText: [
      "Create alert policy",
      "VPS selector expression",
      "Rule rows",
      "Condition expression",
      "Window",
      "Severity",
    ],
  },
  {
    view: "Observability",
    subpage: "Webhooks",
    heading: "Webhooks",
    id: "43c-observability-webhooks",
    requiredText: ["Webhook rules", "Webhook deliveries", "Webhook delivery maintenance"],
  },
  {
    view: "Observability",
    subpage: "Webhooks",
    heading: "Webhooks",
    id: "43d-observability-webhooks-rule-editor",
    prepare: "webhook-rule-editor",
    requiredText: ["Create webhook rule", "Webhook rules are saved expression records"],
  },
  {
    view: "Observability",
    subpage: "Dashboards",
    heading: "Dashboards",
    id: "44-observability-dashboards",
    requiredText: ["Saved dashboards", "Widget layout", "Share and export"],
  },
  { view: "Audit", subpage: "Events", heading: "Audit events", id: "45-audit-events" },
  {
    view: "Audit",
    subpage: "Job evidence",
    heading: "Job audit evidence",
    id: "46-audit-job-evidence",
    requiredText: ["Job evidence ledger", "Selected job proof", "Audit context"],
  },
  {
    view: "Audit",
    subpage: "Sessions",
    heading: "Session evidence",
    id: "47-audit-sessions",
    requiredText: ["Terminal session evidence", "Operator session evidence", "Transcript references"],
  },
  {
    view: "Audit",
    subpage: "Retention & export",
    heading: "History retention",
    id: "48-audit-retention-export",
    requiredText: ["Export scope", "Cleanup review", "Evidence retention only"],
  },
  { view: "Access", subpage: "Overview", heading: "Access overview", id: "49-access-overview" },
  {
    view: "Access",
    subpage: "Operators",
    heading: "Operators",
    id: "50-access-operators",
  },
  {
    view: "Access",
    subpage: "VPS identities",
    heading: "VPS identities",
    id: "51-access-vps-identities",
  },
  {
    view: "Access",
    subpage: "Gateway sessions",
    heading: "Gateway sessions",
    id: "52-access-gateway-sessions",
  },
  {
    view: "Access",
    subpage: "Privilege vault",
    heading: "Privilege vault",
    id: "53-access-privilege-vault",
  },
  {
    view: "System",
    subpage: "Overview",
    heading: "System overview",
    id: "54-system-overview",
  },
  {
    view: "System",
    subpage: "Capacity",
    heading: "System capacity",
    id: "55-system-capacity",
    requiredText: ["Capacity telemetry", "Artifact storage", "Retention pressure", "Worker lag"],
  },
  {
    view: "System",
    subpage: "Suite config",
    heading: "Suite config",
    id: "56-system-suite-config",
    requiredText: ["System scope", "Runtime config scope", "Save contract"],
  },
  {
    view: "System",
    subpage: "Maintenance",
    heading: "System maintenance",
    id: "57-system-maintenance",
    requiredText: ["Dry-run gate", "Maintenance jobs", "Queue cleanup"],
  },
  {
    view: "System",
    subpage: "Preferences",
    heading: "System preferences",
    id: "58-system-preferences",
  },
];

test("structured screenshot manifest uses release IA top-level routes", () => {
  for (const entry of allViews) {
    expect(releaseTopLevel).toContain(entry.view);
    expect(legacyTopLevel).not.toContain(entry.view);
  }
});

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
  const label = entry.subpage
    ? `${entry.view} / ${entry.subpage}${entry.tab ? ` / ${entry.tab}` : ""}`
    : entry.view;

  await expectNoLegacyTopLevelSidebarEntries(page);
  await openConsoleSubpage(page, entry.view, entry.subpage ?? "Overview");
  await expectNoLegacyTopLevelSidebarEntries(page);

  if (entry.tab) {
    const tab = page.getByRole("tab", { name: entry.tab, exact: true });
    await expect(tab).toBeVisible({ timeout: 5_000 });
    await tab.click();
  }

  if (entry.expandVpsRow) {
    const grid = page.getByLabel("VPS instance records data grid");
    const row = grid
      .locator(".gridBody [role=row]", { hasText: entry.expandVpsRow })
      .first();
    await expect(row).toBeVisible({ timeout: 5_000 });
    await row.click();
    await expect(
      page.locator(".consoleHeader").getByText("vpsman / Fleet / Instance detail"),
    ).toBeVisible({ timeout: 5_000 });

    if (entry.detailTab) {
      const detailTab = page.getByRole("tab", {
        name: entry.detailTab,
        exact: true,
      });
      await expect(detailTab).toBeVisible({ timeout: 5_000 });
      await detailTab.click();
    }
  }

  // Wait for heading or any main content
  const activeSection = entry.expandVpsRow ? "Instance detail" : entry.subpage ?? "Overview";
  await expect(
    page.locator(".consoleHeader").getByText(`vpsman / ${entry.view} / ${activeSection}`),
  ).toBeVisible({ timeout: 5_000 });
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

  if (entry.prepare === "alert-policy-editor") {
    await page.getByRole("button", { name: "Create policy" }).click();
    await expect(
      page.locator(".consoleDetailPanel", { hasText: "Create alert policy" }),
    ).toBeVisible({ timeout: 5_000 });
    await expect(page.getByRole("button", { name: "Close detail panel" })).toBeVisible();
  }

  if (entry.prepare === "tunnel-plan-create") {
    await page.getByRole("button", { name: "Create tunnel plan" }).click();
    await expect(page.getByRole("heading", { name: "Create tunnel plan" })).toBeVisible({
      timeout: 5_000,
    });
    await expect(page.getByRole("button", { name: "Close create tunnel plan workflow" })).toBeVisible();
  }

  if (entry.prepare === "tunnel-plan-promotion") {
    await page.getByRole("button", { name: "Promotion workflow" }).click();
    await expect(page.getByLabel("Tunnel plan promotion workflow")).toBeVisible({
      timeout: 5_000,
    });
    await expect(page.getByRole("button", { name: "Close tunnel promotion workflow" })).toBeVisible();
  }

  if (entry.prepare === "webhook-rule-editor") {
    await page.getByRole("button", { name: "Create rule" }).click();
    await expect(
      page.locator(".consoleDetailPanel", { hasText: "Create webhook rule" }),
    ).toBeVisible({ timeout: 5_000 });
    await expect(page.getByRole("button", { name: "Close detail panel" })).toBeVisible();
  }

  if (entry.prepare === "vps-rules-preview") {
    await page.getByLabel("VPS rule set values").fill(
      "traffic.reset_day=14\ntraffic.quota.total=3TB\ntraffic.selectors=eth0+tx,ens3",
    );
    await page.getByRole("button", { name: "Dry-run set values" }).click();
    await expect(page.getByText("Dry-run changed rows")).toBeVisible({
      timeout: 5_000,
    });
    const prompt = page.locator(".confirmationPrompt", {
      hasText: "Confirm VPS rule write",
    });
    await expect(prompt).toBeVisible({ timeout: 5_000 });
    await prompt.getByRole("button", { name: "Cancel" }).click();
  }

  for (const text of entry.requiredText ?? []) {
    await expectVisibleText(page, text);
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
    tab: entry.tab ?? null,
    expandVpsRow: entry.expandVpsRow ?? null,
    detailTab: entry.detailTab ?? null,
    heading: entry.heading,
    horizontalOverflowPx,
    screenshot: screenshotPath,
  };
}

async function expectNoLegacyTopLevelSidebarEntries(
  page: import("@playwright/test").Page,
) {
  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  for (const label of legacyTopLevel) {
    await expect(
      nav.locator(".navItem").filter({ hasText: new RegExp(`^${label}$`) }),
      `Legacy top-level sidebar entry ${label}`,
    ).toHaveCount(0);
  }
}

async function expectVisibleText(
  page: import("@playwright/test").Page,
  text: string,
) {
  await expect
    .poll(
      async () => {
        const matches = page.getByText(text);
        const count = await matches.count();
        for (let index = 0; index < count; index += 1) {
          if (await matches.nth(index).isVisible()) {
            return true;
          }
        }
        return false;
      },
      { message: `visible text "${text}"`, timeout: 5_000 },
    )
    .toBe(true);
}

// Install mock API before each test batch
test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

// Generate one test per batch
batches.forEach((batch, batchIndex) => {
  test(`screenshot batch ${batchIndex + 1} of ${batches.length} (${batch[0].id}–${batch[batch.length - 1].id})`, async ({
    page,
  }, testInfo) => {
    test.setTimeout(120_000);
    const projectDir = join(SCREENSHOT_DIR, testInfo.project.name);
    if (batchIndex === 0) {
      rmSync(projectDir, { recursive: true, force: true });
    }
    mkdirSync(projectDir, { recursive: true });

    await page.goto("/");
    await expect(page.locator(".shell")).toBeVisible({ timeout: 15_000 });

    const results: Array<Record<string, unknown>> = [];
    const errors: string[] = [];

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
        errors.push(`${entry.id}: ${String(error)}`);
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
    expect(errors).toEqual([]);
  });
});
