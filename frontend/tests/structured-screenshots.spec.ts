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
  prepare?:
    | "alert-policy-editor"
    | "config-bulk-patch-preview"
    | "config-per-vps-loaded"
    | "tunnel-plan-create"
    | "tunnel-plan-promotion"
    | "vps-rules-preview"
    | "webhook-rule-editor";
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
    requiredText: [
      "Fleet command home",
      "Running work",
      "Recent failures",
      "Needs attention",
      "Recent activity",
    ],
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
      "Desired source",
      "Render status",
      "Drift state",
      "Open config",
      "Compare",
      "Apply",
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
    requiredText: ["Group registry", "Create group"],
  },
  {
    view: "Fleet",
    subpage: "Assignments",
    heading: "Group assignments",
    id: "05-fleet-group-assignments",
    requiredText: ["VPS group assignments"],
  },
  {
    view: "Fleet",
    subpage: "Bulk groups",
    heading: "Bulk groups",
    id: "06-fleet-bulk-groups",
    requiredText: [
      "Bulk tag mutation",
      "Server resolution runs before confirmation",
    ],
  },
  {
    view: "Fleet",
    subpage: "Alerts",
    heading: "Fleet alerts",
    id: "07-fleet-alerts",
    requiredText: [
      "Tunnel adapter degraded",
      "Traffic policy",
      "Acknowledge",
      "Open",
    ],
  },
  {
    view: "Fleet",
    subpage: "Instance detail",
    heading: "Instance detail",
    id: "08-fleet-instance-detail",
    requiredText: [
      "State",
      "Last contact",
      "Agent version",
      "Active jobs",
      "Scheduled shell command",
    ],
  },
  {
    view: "Remote Operations",
    subpage: "Terminal",
    heading: "Terminal",
    id: "09-remote-operations-terminal",
    requiredText: [
      "Terminal sessions",
      "Open terminal",
      "Focus terminal",
      "Advanced session controls",
    ],
  },
  {
    view: "Remote Operations",
    subpage: "Files",
    heading: "Files",
    id: "10-remote-operations-files",
    requiredText: [
      "File browser",
      "Select a VPS and file to begin.",
      "Download folder as archive",
      "Advanced file options",
    ],
  },
  {
    view: "Remote Operations",
    subpage: "Transfers",
    heading: "Transfers",
    id: "11-remote-operations-transfers",
    requiredText: [
      "File transfer sessions",
      "Upload file",
      "Ready downloads",
      "Transfer sessions",
      "Advanced: reusable upload sources",
    ],
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
    requiredText: ["Check update", "Start update", "Registered artifact"],
  },
  {
    view: "Network",
    subpage: "Overview",
    heading: "Network overview",
    id: "23-network-overview",
    requiredText: ["Create tunnel", "Latest evidence", "stale"],
  },
  {
    view: "Network",
    subpage: "Graph",
    heading: "Network graph",
    id: "24-network-graph",
    requiredText: ["Topology graph", "Last topology evidence", "stale"],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25-network-tunnel-plans",
    requiredText: [
      "Create tunnel plan",
      "Desired state",
      "Runtime state",
      "100 Mbps target",
      "Promotion workflow",
      "Generated config",
    ],
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
  {
    view: "Backups",
    subpage: "Overview",
    heading: "Backup overview",
    id: "29-backups-overview",
  },
  {
    view: "Backups",
    subpage: "Requests",
    heading: "Backup requests",
    id: "30-backups-requests",
    requiredText: [
      "Backup request history",
      "artifact-backed",
      "Open artifact",
    ],
  },
  {
    view: "Backups",
    subpage: "Policies",
    heading: "Backup policies",
    id: "31-backups-policies",
    requiredText: [
      "Scheduled backup policies",
      "No scheduled backups",
      "Create policy",
    ],
  },
  {
    view: "Backups",
    subpage: "Artifacts",
    heading: "Backup artifacts",
    id: "32-backups-artifacts",
    requiredText: [
      "Artifact inventory records",
      "Available package",
      "Transfer package",
    ],
  },
  {
    view: "Backups",
    subpage: "Restore",
    heading: "Restore",
    id: "33-backups-restore",
    requiredText: [
      "Restore source records",
      "Available package",
      "Draft restore",
    ],
  },
  {
    view: "Backups",
    subpage: "Migration",
    heading: "Migration",
    id: "34-backups-migration",
    requiredText: [
      "Source VPS/artifact",
      "Replacement VPS",
      "Migration mapping records",
    ],
  },
  {
    view: "Config",
    subpage: "Overview",
    heading: "Runtime config overview",
    id: "35-config-overview",
    requiredText: [
      "Affected VPS current state",
      "Stale apply",
      "Deleted or unavailable VPS",
      "3/3 rules valid",
    ],
  },
  {
    view: "Config",
    subpage: "Per-VPS",
    heading: "Per-VPS config",
    id: "36-config-per-vps",
    requiredText: ["Select one VPS", "Read current config"],
  },
  {
    view: "Config",
    subpage: "Per-VPS",
    heading: "Per-VPS config",
    id: "36b-config-per-vps-loaded",
    prepare: "config-per-vps-loaded",
    requiredText: ["Current base", "Desired patch", "Apply patch"],
  },
  {
    view: "Config",
    subpage: "Bulk patch",
    heading: "Bulk patch",
    id: "37-config-bulk-patch",
    requiredText: ["Incremental patch", "Targets", "Preview changes"],
  },
  {
    view: "Config",
    subpage: "Bulk patch",
    heading: "Bulk patch",
    id: "37b-config-bulk-patch-preview",
    prepare: "config-bulk-patch-preview",
    requiredText: ["1 VPS resolved", "edge-sfo-01", "Apply patch"],
  },
  {
    view: "Config",
    subpage: "Template coverage",
    heading: "Template coverage",
    id: "38-config-templates",
    requiredText: [
      "Desired source",
      "Server storage missing",
      "Fix source",
      "Runtime selected only",
    ],
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
      "Common rule cards",
      "Total quota",
      "Interfaces / selectors",
      "Advanced raw key/value",
      "Preview changes",
    ],
  },
  {
    view: "Observability",
    subpage: "Fleet metrics",
    heading: "Fleet metrics",
    id: "40-observability-fleet-metrics",
    requiredText: [
      "CPU load by VPS",
      "Selected: 24h",
      "Data available:",
      "Sparse data:",
      "Active alerts",
      "Warning observations",
      "Top VPS",
      "Fleet grouping",
    ],
  },
  {
    view: "Observability",
    subpage: "Network metrics",
    heading: "Network metrics",
    id: "41-observability-network-metrics",
    requiredText: [
      "Stale network evidence",
      "Latency, loss, and speed",
      "Chart samples",
      "Time filter: retained evidence",
      "Tunnel grouping",
      "agent-fra-02 -> agent-sfo-01",
      "Endpoint comparison",
      "Saved plan match",
      "Alert overlays",
    ],
  },
  {
    view: "Observability",
    subpage: "Alerts",
    heading: "Alerts",
    id: "43-observability-alerts",
    requiredText: ["Alert policies", "Destinations", "Deliveries"],
  },
  {
    view: "Observability",
    subpage: "Alerts",
    heading: "Alerts",
    id: "43b-observability-alerts-policy-editor",
    prepare: "alert-policy-editor",
    requiredText: [
      "Create alert policy",
      "Enable after creation",
      "Preview matches",
      "VPS selector expression",
      "Rule rows",
      "Condition expression",
      "Window",
      "Severity",
    ],
  },
  {
    view: "Observability",
    subpage: "Event webhooks",
    heading: "Event webhooks",
    id: "43c-observability-webhooks",
    requiredText: [
      "Event webhook rules",
      "Send test",
      "Retry failed",
      "Deliveries",
      "Maintenance",
    ],
  },
  {
    view: "Observability",
    subpage: "Event webhooks",
    heading: "Event webhooks",
    id: "43d-observability-webhooks-rule-editor",
    prepare: "webhook-rule-editor",
    requiredText: [
      "Create webhook rule",
      "Enable after creation",
      "Signing secret",
      "Sample payload",
      "VPSs matched",
      "Rendered message",
      "Create rule",
    ],
  },
  {
    view: "Observability",
    subpage: "Dashboards",
    heading: "Dashboards",
    id: "44-observability-dashboards",
    requiredText: [
      "Dashboard presets",
      "Source counts",
      "Range coverage",
      "Widget layout",
      "Share / Export",
    ],
  },
  {
    view: "Audit",
    subpage: "Events",
    heading: "Audit events",
    id: "45-audit-events",
    requiredText: [
      "Visible events",
      "Coverage warning",
      "Related job/session",
      "Latest visible",
    ],
  },
  {
    view: "Audit",
    subpage: "Job evidence",
    heading: "Job audit evidence",
    id: "46-audit-job-evidence",
    requiredText: [
      "Job evidence ledger",
      "Selected job proof",
      "Audit event missing",
    ],
  },
  {
    view: "Audit",
    subpage: "Sessions",
    heading: "Session evidence",
    id: "47-audit-sessions",
    requiredText: [
      "Terminal session evidence",
      "Operator session evidence",
      "Transcript references",
      "Started",
      "Last activity",
      "Expiry",
      "Demo/test auth signals",
    ],
  },
  {
    view: "Audit",
    subpage: "Retention & export",
    heading: "History retention",
    id: "48-audit-retention-export",
    requiredText: [
      "Policy domains",
      "Audit logs",
      "Retention days",
      "Export scope",
      "Evidence retention only",
    ],
  },
  {
    view: "Access",
    subpage: "Overview",
    heading: "Access overview",
    id: "49-access-overview",
    requiredText: [
      "Actions required",
      "Policy recommends MFA",
      "Operators and active sessions",
      "VPS identities",
      "Gateway sessions",
      "Privilege state",
    ],
  },
  {
    view: "Access",
    subpage: "Operators",
    heading: "Operators",
    id: "50-access-operators",
    requiredText: [
      "Operator access policy",
      "MFA policy",
      "recommended instead of enforced",
      "Operator accounts",
      "Policy recommends MFA",
      "Revoke sessions",
    ],
  },
  {
    view: "Access",
    subpage: "VPS identities",
    heading: "VPS identities",
    id: "51-access-vps-identities",
    requiredText: [
      "VPS identities",
      "Register VPS",
      "Current key",
      "Client key revocations",
      "Host rebuild",
    ],
  },
  {
    view: "Access",
    subpage: "Gateway sessions",
    heading: "Gateway sessions",
    id: "52-access-gateway-sessions",
    requiredText: [
      "No active gateway sessions",
      "gateway endpoint and server key",
      "Gateway settings",
    ],
  },
  {
    view: "Access",
    subpage: "Privilege vault",
    heading: "Privilege vault",
    id: "53-access-privilege-vault",
    requiredText: [
      "Privilege workflow",
      "Privilege vault",
      "Unlock scope",
      "Unlocked until",
      "Keep encrypted in this browser",
      "QR/secret",
      "Complete setup",
    ],
  },
  {
    view: "System",
    subpage: "Overview",
    heading: "System overview",
    id: "54-system-overview",
    requiredText: [
      "Service health",
      "Control-plane queue",
      "What needs attention",
      "Diagnostics",
      "Selected chart - Dispatch queue",
    ],
  },
  {
    view: "System",
    subpage: "Capacity",
    heading: "System capacity",
    id: "55-system-capacity",
    requiredText: [
      "Capacity telemetry",
      "Subsystem capacity",
      "Queue growth",
      "Suite Config fields",
      "Unavailable capacity telemetry",
    ],
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
    requiredText: [
      "Preview gate",
      "Artifact types",
      "Delete artifacts",
      "Maintenance jobs",
    ],
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
    if ((await row.count()) > 0) {
      await expect(row).toBeVisible({ timeout: 5_000 });
      await row.click();
    } else {
      const card = grid
        .locator(".gridMobileCard", { hasText: entry.expandVpsRow })
        .first();
      await expect(card).toBeVisible({ timeout: 5_000 });
      await card.getByRole("button", { name: "Open", exact: true }).click();
    }
    await expect(
      page
        .locator(".consoleHeader")
        .getByText("vpsman / Fleet / Instance detail"),
    ).toBeVisible({ timeout: 5_000 });

    if (entry.detailTab) {
      const detailTab = page.getByRole("tab", {
        name: entry.detailTab,
        exact: true,
      });
      const hasDetailTab = (await detailTab.count()) > 0;
      if (hasDetailTab && (await detailTab.first().isVisible())) {
        await detailTab.click();
      } else {
        const detailSection = page.getByLabel("VPS detail section");
        await expect(detailSection).toBeVisible({ timeout: 5_000 });
        await detailSection.selectOption(entry.detailTab);
      }
    }
  }

  // Wait for heading or any main content
  const activeSection = entry.expandVpsRow
    ? "Instance detail"
    : (entry.subpage ?? "Overview");
  await expect(
    page
      .locator(".consoleHeader")
      .getByText(`vpsman / ${entry.view} / ${activeSection}`),
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
    await expect(
      page.getByRole("button", { name: "Close detail panel" }),
    ).toBeVisible();
  }

  if (entry.prepare === "tunnel-plan-create") {
    await closeTunnelPlanWorkflow(page);
    await page.getByRole("button", { name: "Create tunnel plan" }).click();
    await expect(
      page.getByRole("heading", { name: "Create tunnel plan" }),
    ).toBeVisible({
      timeout: 5_000,
    });
    await expect(
      page.getByRole("button", { name: "Close create tunnel plan workflow" }),
    ).toBeVisible();
  }

  if (entry.prepare === "tunnel-plan-promotion") {
    await closeTunnelPlanWorkflow(page);
    await page.getByRole("button", { name: "Promotion workflow" }).click();
    await expect(page.getByLabel("Tunnel plan promotion workflow")).toBeVisible(
      {
        timeout: 5_000,
      },
    );
    await expect(
      page.getByRole("button", { name: "Close tunnel promotion workflow" }),
    ).toBeVisible();
  }

  if (entry.prepare === "webhook-rule-editor") {
    await page.getByRole("button", { name: "Create rule" }).click();
    const editor = page.locator(".consoleDetailPanel", {
      hasText: "Create webhook rule",
    });
    await expect(editor).toBeVisible({ timeout: 5_000 });
    await expect(
      page.getByRole("button", { name: "Close detail panel" }),
    ).toBeVisible();
    await editor.getByRole("button", { name: "Test" }).click();
    await expect(editor).toContainText("Rendered message");
  }

  if (entry.prepare === "config-per-vps-loaded") {
    const targetPicker = page.getByRole("combobox", {
      name: "VPS config target",
    });
    await targetPicker.fill("fra");
    await expect(
      page.getByRole("option", { name: /core-fra-02.*agent-fra-02/ }),
    ).toBeVisible({ timeout: 5_000 });
    await page.keyboard.press("Enter");
    await page.getByRole("button", { name: "Read current config" }).click();
    await expect(
      page.getByLabel("VPS redacted runtime config TOML"),
    ).toHaveValue(/client_id = "agent-fra-02"/, { timeout: 5_000 });
    await page
      .getByLabel("One-VPS runtime config override TOML")
      .fill("[telemetry]\ninterval_secs = 60\n");
    await expect(
      page.getByLabel("One-VPS config override guard"),
    ).toContainText("telemetry", { timeout: 5_000 });
  }

  if (entry.prepare === "config-bulk-patch-preview") {
    const selector = page.getByRole("searchbox", {
      name: "Bulk patch target expression",
    });
    await selector.fill("id:agent-sfo-01");
    await expect(
      page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ }),
    ).toBeVisible({ timeout: 5_000 });
    await page.keyboard.press("Enter");
    await page.getByRole("button", { name: "Preview changes" }).click();
    await expect(page.getByText("1 VPS resolved")).toBeVisible({
      timeout: 5_000,
    });
    await expect(page.getByLabel("Bulk patch change summary")).toContainText(
      "edge-sfo-01",
      { timeout: 5_000 },
    );
  }

  if (entry.prepare === "vps-rules-preview") {
    await page.getByLabel("Reset day").fill("14");
    await page.getByLabel("Total quota").fill("4TB");
    await page.getByLabel("Interfaces / selectors").fill("ens3, eth0+tx");
    await page.getByRole("button", { name: "Preview changes" }).click();
    await expect(page.getByLabel("Preview changes data grid")).toBeVisible({
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

async function closeTunnelPlanWorkflow(page: import("@playwright/test").Page) {
  for (const label of [
    "Close create tunnel plan workflow",
    "Close tunnel promotion workflow",
    "Close generated config workflow",
  ]) {
    const button = page.getByRole("button", { name: label });
    if (await button.isVisible().catch(() => false)) {
      await button.click();
    }
  }
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
