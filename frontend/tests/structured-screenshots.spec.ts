import { expect, test } from "@playwright/test";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  unlockPrivilegeFromTop,
  waitForConsoleShell,
} from "./support/consoleNavigation";
import { viewLabel } from "../src/constants";
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
    | "fleet-delete-success"
    | "fleet-metrics-advanced"
    | "fleet-metrics-chart"
    | "network-latency-chart"
    | "network-throughput-chart"
    | "port-forward-confirmation"
    | "port-forward-create"
    | "port-forward-details"
    | "source-template-adapter-detail"
    | "tunnel-plan-assessment"
    | "tunnel-plan-create"
    | "tunnel-plan-delete-review"
    | "tunnel-plan-disable-review"
    | "tunnel-plan-ospf"
    | "vps-rules-preview"
    | "webhook-rule-editor";
  desktopRequiredText?: string[];
  mobileRequiredText?: string[];
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
      "Privilege locked",
      "Unlock privilege",
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
      "Unlock to browse this VPS.",
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
      "Advanced: source artifacts",
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
    subpage: "Source templates",
    heading: "Source templates",
    id: "21b-automation-tunnel-adapter-detail",
    prepare: "source-template-adapter-detail",
    requiredText: [
      "shared:tunnel-lifecycle-v1",
      "Bound from tunnel plans",
      "never ambient VPS configuration",
      "Open tunnel plans",
    ],
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
    requiredText: ["Create plan", "Latest evidence", "stale"],
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
    requiredText: ["Create plan", "sfo-fra-gre", "external-openvpn"],
    desktopRequiredText: [
      "Runtime owner",
      "Agent iproute2",
      "External observed",
      "Tunnel only",
    ],
  },
  {
    view: "Network",
    subpage: "Port forwards",
    heading: "Port forwarding",
    id: "25g-network-port-forwarding",
    requiredText: [
      "Create rule",
      "Public web ingress",
      "IPv6 service range",
      "Retired DNS relay",
    ],
  },
  {
    view: "Network",
    subpage: "Port forwards",
    heading: "Port-forward rule details",
    id: "25h-network-port-forwarding-details",
    prepare: "port-forward-details",
    requiredText: [
      "Control desired",
      "Agent desired",
      "Observed table",
      "Listener scope",
      "NAT matches",
    ],
  },
  {
    view: "Network",
    subpage: "Port forwards",
    heading: "Create port-forward rule",
    id: "25i-network-port-forwarding-create",
    prepare: "port-forward-create",
    requiredText: [
      "Create port-forward rule",
      "Incoming ports",
      "Target ports",
      "Target IP or hostname",
      "Return path",
    ],
  },
  {
    view: "Network",
    subpage: "Port forwards",
    heading: "Confirm port-forward rule",
    id: "25j-network-port-forwarding-confirmation",
    prepare: "port-forward-confirmation",
    requiredText: [
      "Confirm rule creation",
      "Listener scope",
      "Claimed ports",
      "Target",
      "Return",
    ],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25f-network-tunnel-connectivity-assessment",
    prepare: "tunnel-plan-assessment",
    requiredText: [
      "Operator connectivity assessment",
      "Display-only annotation",
      "Left outer path",
      "Right outer path",
      "Peer probe failed; not proof of disconnect",
    ],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25b-network-tunnel-plans-create",
    prepare: "tunnel-plan-create",
    requiredText: [
      "Create tunnel plan",
      "Plan and endpoints",
      "Runtime ownership",
      "Endpoint addresses",
      "OSPF cost control",
    ],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25c-network-tunnel-plans-ospf",
    prepare: "tunnel-plan-ospf",
    requiredText: [
      "Enable OSPF adapter workflow",
      "Left routing adapter",
      "Control mode",
      "live preview",
    ],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25d-network-tunnel-plan-disable-review",
    prepare: "tunnel-plan-disable-review",
    requiredText: [
      "Confirm tunnel plan disable",
      "OSPF control stops; existing external daemon costs are not reverted",
      "Stop control; keep current external values",
    ],
  },
  {
    view: "Network",
    subpage: "Tunnel plans",
    heading: "Tunnel plans",
    id: "25e-network-tunnel-plan-delete-review",
    prepare: "tunnel-plan-delete-review",
    requiredText: [
      "Confirm tunnel plan deletion",
      "Permanently retire this disabled declaration",
      "Left Removed; right Removed",
      "audit history remains",
    ],
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
      "Reachability observations",
      "Top VPS",
      "Fleet grouping",
    ],
  },
  {
    view: "Observability",
    subpage: "Fleet metrics",
    heading: "Fleet metrics",
    id: "40b-observability-fleet-metrics-advanced",
    prepare: "fleet-metrics-advanced",
    requiredText: [
      "Advanced filters",
      "Scope value",
      "Points",
      "Start",
      "End",
      "Reset filters",
    ],
  },
  {
    view: "Observability",
    subpage: "Fleet metrics",
    heading: "Fleet metrics",
    id: "40c-observability-fleet-metrics-chart",
    prepare: "fleet-metrics-chart",
    requiredText: [
      "CPU load trend",
      "Metric definition:",
      "Linux 1-minute load",
      "Data coverage:",
      "Top VPS",
    ],
  },
  {
    view: "Observability",
    subpage: "Network metrics",
    heading: "Network metrics",
    id: "41-observability-network-metrics",
    requiredText: [
      "Stale network evidence",
      "Latency, loss, and throughput",
      "Observations",
      "OSPF review",
      "Time filter: retained evidence",
      "Tunnel grouping",
      "agent-fra-02 -> agent-sfo-01",
      "Endpoint comparison",
      "only declared plans",
      "Network review signals",
    ],
  },
  {
    view: "Observability",
    subpage: "Network metrics",
    heading: "Network metrics",
    id: "41b-observability-network-latency-chart",
    prepare: "network-latency-chart",
    requiredText: [
      "Latency",
      "Metric definition:",
      "mean RTT",
      "Data coverage:",
      "sfo-fra-gre",
    ],
  },
  {
    view: "Observability",
    subpage: "Network metrics",
    heading: "Network metrics",
    id: "41c-observability-network-throughput-chart",
    prepare: "network-throughput-chart",
    requiredText: [
      "Throughput",
      "average TCP throughput",
      "Average throughput 10.1 Mbps",
      "Data coverage:",
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
      "Copy / Export",
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
      "Authentication signals",
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
      "Access responsibilities",
      "Session scopes",
      "VPS identities",
      "Gateway sessions",
      "Privilege unlock",
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
      "recommended rather than enforced",
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
      "Dispatch capacity",
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
  {
    view: "Fleet",
    subpage: "Instances",
    heading: "Fleet instances",
    id: "59-fleet-delete-tunnel-cleanup",
    prepare: "fleet-delete-success",
    requiredText: [
      "VPS deleted; tunnel cleanup queued for 1 surviving peer.",
    ],
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
    ? `${viewLabel(entry.view)} / ${entry.subpage}${entry.tab ? ` / ${entry.tab}` : ""}`
    : viewLabel(entry.view);

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
      const explicitOpen = row
        .getByRole("button", { name: /Open .*detail/ })
        .first();
      await expect(explicitOpen).toBeVisible({ timeout: 5_000 });
      await explicitOpen.click();
    } else {
      const card = grid
        .locator(".gridMobileCard", { hasText: entry.expandVpsRow })
        .first();
      await expect(card).toBeVisible({ timeout: 5_000 });
      const explicitOpen = card.getByRole("button", { name: /Open VPS/ });
      if ((await explicitOpen.count()) > 0) {
        await explicitOpen.click();
      } else {
        await card.getByRole("button", { name: "Open", exact: true }).click();
      }
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
      .getByText(`vpsman / ${viewLabel(entry.view)} / ${activeSection}`),
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
    await expectSectionBelowToolbar(
      page.locator(".consoleDetailPanel", { hasText: "Create alert policy" }),
    );
  }

  if (entry.prepare === "fleet-delete-success") {
    await unlockPrivilegeFromTop(page);
    await openConsoleSubpage(page, "Fleet", "Instances");
    const grid = page.getByLabel("VPS instance records data grid");
    await grid
      .getByLabel("Select VPS instance records row agent-nyc-03")
      .check();
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await page
      .getByRole("menuitem", { name: "Review VPS deletion" })
      .click();
    const prompt = page.locator(".fleetInstancesPanel > .confirmationPrompt");
    await expect(prompt).toBeVisible({ timeout: 5_000 });
    await prompt.getByRole("button", { name: "Delete VPS" }).click();
    await expect(
      page.getByText(
        "VPS deleted; tunnel cleanup queued for 1 surviving peer.",
      ),
    ).toBeVisible({ timeout: 5_000 });
  }
  if (entry.prepare === "fleet-metrics-advanced") {
    const filters = page.locator(".fleetMetricsAdvancedFilters");
    await filters.locator("summary").click();
    await filters.getByLabel("Fleet metrics scope kind").selectOption("provider");
    await filters.getByLabel("Fleet metrics scope value").selectOption({ index: 1 });
  }

  if (entry.prepare === "fleet-metrics-chart") {
    const filters = page.locator(".fleetMetricsAdvancedFilters");
    if (!(await filters.getByRole("button", { name: "Reset filters" }).isVisible())) {
      await filters.locator("summary").click();
    }
    const reset = filters.getByRole("button", { name: "Reset filters" });
    if (await reset.isEnabled()) {
      await reset.click();
    }
    if ((await filters.getAttribute("open")) !== null) {
      await filters.locator("summary").click();
    }
    const chartSection = page.locator(".observabilityMetricsPanel .observabilityChartSection");
    await scrollSectionBelowToolbar(chartSection);
    await expect(chartSection.locator(".timeSeriesChartShell")).toBeVisible();
  }

  if (
    entry.prepare === "network-latency-chart" ||
    entry.prepare === "network-throughput-chart"
  ) {
    if (entry.prepare === "network-throughput-chart") {
      await page
        .getByLabel("Network metric selector")
        .getByRole("button", { name: "Throughput" })
        .click();
    }
    const chartSection = page.locator(
      ".observabilityNetworkMetricsPanel .observabilityChartSection",
    );
    await scrollSectionBelowToolbar(chartSection);
    await expect(chartSection.locator(".timeSeriesChartShell")).toBeVisible();
  }

  if (
    entry.prepare === "port-forward-details" ||
    entry.prepare === "port-forward-create" ||
    entry.prepare === "port-forward-confirmation"
  ) {
    await closePortForwardWorkflow(page);
    if (entry.prepare === "port-forward-details") {
      await page
        .getByRole("row", { name: /Public web ingress/ })
        .click();
      await expect(
        page.getByRole("region", { name: "Details for Public web ingress" }),
      ).toBeVisible({ timeout: 5_000 });
    } else {
      await page.getByRole("button", { name: "Create rule" }).click();
      const editor = page.locator(".portForwardEditor");
      await expect(editor).toBeVisible({ timeout: 5_000 });
      if (entry.prepare === "port-forward-confirmation") {
        await editor.getByLabel("Name", { exact: true }).fill("Audit web ingress");
        await editor.getByLabel("Incoming ports").fill("8443");
        await editor.getByLabel("Target ports").fill("443");
        await editor.getByLabel("Target IP or hostname").fill("10.20.0.25");
        await editor.getByRole("button", { name: "Create rule" }).click();
        await expect(page.getByLabel("Confirm rule creation")).toBeVisible({
          timeout: 5_000,
        });
      }
    }
  }

  if (entry.prepare === "source-template-adapter-detail") {
    const registry = page.getByLabel("Template registry data grid");
    const adapterRow = registry
      .locator(".gridRow, .gridMobileCard")
      .filter({ hasText: "shared:tunnel-lifecycle-v1" })
      .first();
    await adapterRow.click();
    const adapterRecord = adapterRow.locator(
      "xpath=ancestor::div[contains(concat(' ', normalize-space(@class), ' '), ' gridRecord ')][1]",
    );
    const lifecycleAction = adapterRecord
      .getByRole("button", { name: /^(Test\/update|Edit\/test)$/ })
      .first();
    await expect(lifecycleAction).toBeVisible({ timeout: 5_000 });
    await lifecycleAction.click();
    await expect(
      page.getByLabel("shared:tunnel-lifecycle-v1", { exact: true }),
    ).toBeVisible({ timeout: 5_000 });
  }

  if (entry.prepare === "tunnel-plan-create") {
    await closeTunnelPlanWorkflow(page);
    await page.getByRole("button", { name: "Create plan" }).click();
    await expect(
      page.getByRole("heading", { name: "Create tunnel plan" }),
    ).toBeVisible({
      timeout: 5_000,
    });
    await expect(
      page.getByRole("button", { name: "Close tunnel plan editor" }),
    ).toBeVisible();
  }

  if (entry.prepare === "tunnel-plan-assessment") {
    await closeTunnelPlanWorkflow(page);
    const row = page
      .getByRole("table", { name: "Tunnel plans" })
      .locator("tbody > tr")
      .filter({ hasText: "sfo-fra-gre" })
      .first();
    await row.click();
    await expect(page.locator(".tunnelConnectionAssessment")).toBeVisible({
      timeout: 5_000,
    });
  }

  if (entry.prepare === "tunnel-plan-ospf") {
    await closeTunnelPlanWorkflow(page);
    await page.getByRole("button", { name: "Create plan" }).click();
    await page.getByText("Enable OSPF adapter workflow").click();
    await expect(
      page.getByLabel("OSPF control mode"),
    ).toBeVisible();
  }

  if (entry.prepare === "tunnel-plan-disable-review") {
    await closeTunnelPlanWorkflow(page);
    await page
      .getByRole("button", { name: "Disable sfo-fra-gre" })
      .click();
    await expect(
      page.getByLabel("Confirm tunnel plan disable"),
    ).toBeVisible({ timeout: 5_000 });
  }

  if (entry.prepare === "tunnel-plan-delete-review") {
    await closeTunnelPlanWorkflow(page);
    await page
      .getByRole("button", { name: "Disable sfo-fra-gre" })
      .click();
    await page
      .getByLabel("Confirm tunnel plan disable")
      .getByRole("button", { name: "Disable plans" })
      .click();
    await expect(
      page.getByRole("button", { name: "Enable sfo-fra-gre" }),
    ).toBeVisible({ timeout: 5_000 });
    await page
      .getByRole("button", { name: "Delete sfo-fra-gre" })
      .click();
    await expect(
      page.getByLabel("Confirm tunnel plan deletion"),
    ).toBeVisible({ timeout: 5_000 });
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
    await editor.getByLabel("Webhook rule name").fill("edge-status-webhook");
    await editor
      .getByLabel("Webhook expression")
      .fill("interval.30sec && tag:edge");
    await editor
      .getByLabel("Webhook target")
      .fill("https://hooks.example.net/vpsman");
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
    await page
      .getByLabel("VPS rules preview final action")
      .getByRole("button", { name: /Apply \d+ change/ })
      .click();
    const prompt = page.locator(".confirmationPrompt", {
      hasText: "Confirm VPS rule write",
    });
    await expect(prompt).toBeVisible({ timeout: 5_000 });
    await prompt.getByRole("button", { name: "Cancel" }).click();
  }

  const viewportRequiredText = projectName.startsWith("mobile")
    ? entry.mobileRequiredText
    : entry.desktopRequiredText;
  for (const text of [
    ...(entry.requiredText ?? []),
    ...(viewportRequiredText ?? []),
  ]) {
    await expectVisibleText(page, text);
  }

  await page.waitForTimeout(200);
  const portForwardWorkflow = entry.prepare?.startsWith("port-forward-") ?? false;
  const preserveWorkflowFocus =
    (!projectName.startsWith("mobile") || portForwardWorkflow) &&
    Boolean(
      (entry.prepare && entry.prepare !== "fleet-metrics-advanced") ||
        entry.expandVpsRow ||
        entry.tab,
    );
  if (!preserveWorkflowFocus) {
    await page.evaluate(() => {
      window.scrollTo(0, 0);
      document.querySelector<HTMLElement>(".content")?.scrollTo(0, 0);
    });
  }
  await page.waitForTimeout(50);
  const verticalScrollOffsets = await page.evaluate(() => ({
    body: document.body.scrollTop,
    content: document.querySelector<HTMLElement>(".content")?.scrollTop ?? 0,
    document: document.documentElement.scrollTop,
    window: window.scrollY,
  }));
  if (!preserveWorkflowFocus) {
    expect(
      Math.max(...Object.values(verticalScrollOffsets)),
      `${label} screenshot scroll origin`,
    ).toBeLessThanOrEqual(1);
  }
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
  await page.screenshot({ fullPage: !portForwardWorkflow, path: screenshotPath });

  return {
    id: entry.id,
    view: viewLabel(entry.view),
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
  const prompt = page.locator(".confirmationPrompt:visible");
  if ((await prompt.count()) > 0) {
    const cancel = prompt.first().getByRole("button", { name: "Cancel" });
    if (await cancel.isVisible().catch(() => false)) {
      await cancel.click();
    }
  }
  const openDetails = page.getByRole("button", {
    name: /^Close details for /,
  });
  while ((await openDetails.count()) > 0) {
    const button = openDetails.first();
    if (!(await button.isVisible().catch(() => false))) break;
    await button.click();
  }
  for (const label of [
    "Close tunnel plan editor",
  ]) {
    const button = page.getByRole("button", { name: label });
    if (await button.isVisible().catch(() => false)) {
      await button.click();
    }
  }
}

async function closePortForwardWorkflow(page: import("@playwright/test").Page) {
  const prompt = page.getByLabel("Confirm rule creation");
  if (await prompt.isVisible().catch(() => false)) {
    await prompt.getByRole("button", { name: "Cancel" }).click();
  }
  const closeEditor = page.getByRole("button", {
    name: "Close port-forward editor",
  });
  if (await closeEditor.isVisible().catch(() => false)) {
    await closeEditor.click();
  }
  const closeDetails = page.getByRole("button", {
    name: "Close port-forward details",
  });
  if (await closeDetails.isVisible().catch(() => false)) {
    await closeDetails.click();
  }
}

async function scrollSectionBelowToolbar(
  section: import("@playwright/test").Locator,
) {
  await section.evaluate((element) => {
    if (window.innerWidth <= 600) {
      window.scrollTo({ top: 0, behavior: "auto" });
      document
        .querySelector<HTMLElement>(".content")
        ?.scrollTo({ top: 0, behavior: "auto" });
      return;
    }
    element.scrollIntoView({ block: "start", behavior: "auto" });
    const content = document.querySelector<HTMLElement>(".content");
    if (content && content.scrollHeight > content.clientHeight) {
      content.scrollBy({ top: -96, behavior: "auto" });
    } else {
      window.scrollBy({ top: -96, behavior: "auto" });
    }
  });
}

async function expectSectionBelowToolbar(
  section: import("@playwright/test").Locator,
) {
  await section.page().waitForTimeout(300);
  const gap = await section.evaluate((element) => {
    const topbar = document.querySelector<HTMLElement>(".topbar");
    const visibleTop = topbar?.getBoundingClientRect().bottom ?? 0;
    return Math.round(element.getBoundingClientRect().top - visibleTop);
  });
  expect(gap, "on-demand panel gap below sticky toolbar").toBeGreaterThanOrEqual(8);
  expect(gap, "on-demand panel gap below sticky toolbar").toBeLessThanOrEqual(24);
}

// Install mock API before each test batch
test.beforeEach(async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
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
    await waitForConsoleShell(page, 15_000);

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
          `[screenshot] OK  ${result.id} — ${viewLabel(entry.view)}${entry.subpage ? ` / ${entry.subpage}` : ""}`,
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
              view: viewLabel(entry.view),
            subpage: entry.subpage ?? null,
            heading: entry.heading,
            screenshot: errPath,
            error: String(error),
          });
        } catch {
          results.push({
            id: entry.id,
            view: viewLabel(entry.view),
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
