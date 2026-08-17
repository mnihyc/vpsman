import { expect, test, type Locator, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { normalizeSubpage, viewLabel, viewSubpages } from "../src/constants";
import { OPERATOR_MONITOR_DENSITY_STORAGE_KEY } from "../src/monitorCardDensity";
import {
  backupId,
  installConsoleApiMock,
} from "./support/consoleLayoutFixtures";
import {
  activate,
  activateSystemMaintenanceSubpanel,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
  waitForConsoleShell,
} from "./support/consoleNavigation";
import type { ActiveView } from "../src/types";

const releaseTopLevel = [
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
const releaseAccessibilityRoutes: Array<{ view: ActiveView; subpage: string }> =
  [
    { view: "Jobs", subpage: "Scheduled runs" },
    { view: "Config", subpage: "VPS override patch" },
    { view: "Config", subpage: "Per-VPS" },
    { view: "Config", subpage: "Rules" },
    { view: "Observability", subpage: "Alerts" },
    { view: "Observability", subpage: "Event webhooks" },
    { view: "System", subpage: "Suite config" },
  ];
const customMockTests = new Set([
  "audit latest visible event uses newest timestamp instead of row order",
  "audit identifies control-plane events without inventing an unknown actor",
  "audit event route loads one exact ID outside the list page",
  "failed audit event lookup stays scoped to its detail page",
  "observability dashboards use safe labels when summary counts are missing",
  "home shows a useful empty state when no VPS agents are loaded",
  "fleet monitor keeps unsettled evidence neutral while cards load",
  "fleet monitor keeps an intentionally empty rate selection out of partial telemetry",
  "fleet monitor sorts warning ties by name and client ID",
  "fleet monitor cards remain readable for 0 generated VPS fixtures",
  "fleet monitor cards remain readable for 1 generated VPS fixtures",
  "fleet monitor cards remain readable for 8 generated VPS fixtures",
  "fleet monitor cards remain readable for 20 generated VPS fixtures",
  "fleet monitor cards remain readable for 100 generated VPS fixtures",
  "fleet monitor cards remain readable for 1000 generated VPS fixtures",
  "startup WebSocket core preserves the in-flight HTTP telemetry snapshot",
  "fleet telemetry refresh keeps successful domains current when one domain fails",
  "fleet metrics freshness uses exact sample time instead of the coarse chart bucket",
  "system maintenance presents an empty cleanup preview as a neutral no-op",
  "config surfaces unavailable runtime apply evidence without trusting health claims",
  "malformed notification channels remain visible and fail closed",
  "VPS monitoring reports per-domain retained resolutions",
]);

test.beforeEach(async ({ page }, testInfo) => {
  if (customMockTests.has(testInfo.title)) {
    return;
  }
  await installConsoleApiMock(page, {
    alertEvidenceSaturated: testInfo.tags.includes("@alert-evidence-saturated"),
    alertStateCoverage: testInfo.tags.includes("@alert-state-coverage"),
    dashboardCountsTruncated: testInfo.tags.includes(
      "@dashboard-counts-truncated",
    ),
    recordPagesSaturated: testInfo.tags.includes("@record-pages-saturated"),
  });
});

async function gotoConsoleHome(page: Page) {
  await page.goto("/");
  await waitForConsoleShell(page);
}

async function selectEvidenceGridRecord(grid: Locator, label: string) {
  const mobileCardAction = grid
    .locator(".gridMobileCard", { hasText: label })
    .getByRole("button", { name: "Select proof" })
    .first();
  if ((await mobileCardAction.count()) > 0) {
    await mobileCardAction.click();
    return;
  }
  await grid.getByText(label).first().click();
}

async function invokeGridRowAction(
  page: Page,
  grid: Locator,
  row: Locator,
  action: string,
) {
  await row.getByRole("checkbox").first().check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await activate(page.getByRole("menuitem", { name: action, exact: true }));
}

async function openMobilePageSelector(page: Page): Promise<Locator | null> {
  await waitForConsoleShell(page);
  const menu = page.locator(".mobilePageMenu");
  if (!(await menu.isVisible())) {
    return null;
  }
  const selector = page.locator(".mobilePageSelector");
  if (!(await selector.isVisible())) {
    await menu.getByLabel("Open mobile page navigation").click();
  }
  await expect(selector).toBeVisible();
  return selector;
}

async function expectReachableByTab(
  page: Page,
  locator: Locator,
  label: string,
  maxTabs = 120,
  resetFocus = true,
) {
  await expect(
    locator,
    `${label} is visible before keyboard traversal`,
  ).toBeVisible();
  if (resetFocus) {
    await page.evaluate(() => {
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur();
      }
      document.body.tabIndex = -1;
      document.body.focus({ preventScroll: true });
    });
  }
  for (let index = 0; index < maxTabs; index += 1) {
    const reached = await locator.evaluate((element) => {
      const active = document.activeElement;
      return active === element || element.contains(active);
    });
    if (reached) {
      return;
    }
    await page.keyboard.press("Tab");
  }

  const activeLabel = await page.evaluate(() => {
    const active = document.activeElement as HTMLElement | null;
    if (!active) return "no active element";
    return [
      active.tagName.toLowerCase(),
      active.getAttribute("aria-label"),
      active.getAttribute("name"),
      active.textContent?.trim().slice(0, 80),
    ]
      .filter(Boolean)
      .join(" / ");
  });
  throw new Error(
    `Keyboard traversal did not reach ${label}; active element was ${activeLabel}`,
  );
}

async function visibleDisabledControlsWithoutReason(page: Page) {
  return page.evaluate(() => {
    function isVisible(element: Element) {
      const style = window.getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return (
        rect.width > 0 &&
        rect.height > 0 &&
        style.display !== "none" &&
        style.visibility !== "hidden" &&
        Number(style.opacity) > 0
      );
    }

    function describedText(element: Element) {
      const describedBy = element.getAttribute("aria-describedby") ?? "";
      return describedBy
        .split(/\s+/)
        .map((id) => document.getElementById(id)?.textContent?.trim() ?? "")
        .filter(Boolean)
        .join(" ");
    }

    return Array.from(
      document.querySelectorAll<HTMLElement>(
        'button:disabled, input:disabled, select:disabled, textarea:disabled, [aria-disabled="true"]',
      ),
    )
      .filter(isVisible)
      .map((element) => {
        const reason = [
          element.getAttribute("title") ?? "",
          element.getAttribute("data-tooltip-disabled-reason") ?? "",
          describedText(element),
        ]
          .map((value) => value.trim())
          .find((value) => value.length >= 12);
        if (reason) return null;
        const name =
          element.getAttribute("aria-label") ??
          element.textContent?.replace(/\s+/g, " ").trim() ??
          element.tagName.toLowerCase();
        return `${name || element.tagName.toLowerCase()} (${element.className || "no class"})`;
      })
      .filter((value): value is string => Boolean(value));
  });
}

async function tooltipContractViolations(page: Page) {
  return page.evaluate(() => {
    function isVisible(element: HTMLElement) {
      const style = window.getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return (
        rect.width > 0 &&
        rect.height > 0 &&
        style.display !== "none" &&
        style.visibility !== "hidden" &&
        Number(style.opacity) > 0
      );
    }

    const generic =
      /^(?:Activate\b|Current value:)|(?:excluded|omitted) from (?:the )?tooltips?|\b(?:review )?field(?: group)?\.$|\bcolumn\.$/i;
    return Array.from(document.querySelectorAll<HTMLElement>("[title]"))
      .filter(isVisible)
      .map((element) => {
        const title = element.getAttribute("title")?.trim() ?? "";
        if (!generic.test(title)) return null;
        const label =
          element.getAttribute("aria-label") ??
          element.textContent?.replace(/\s+/g, " ").trim().slice(0, 80) ??
          element.tagName.toLowerCase();
        return `${label || element.tagName.toLowerCase()}: ${title}`;
      })
      .filter((value): value is string => Boolean(value));
  });
}

async function contrastFailures(page: Page) {
  return page.evaluate(() => {
    type Rgba = { a: number; b: number; g: number; r: number };
    const samples = [
      {
        label: "body text",
        selector: "body, .consoleHeader h1, .consoleDataGrid",
        min: 4.5,
      },
      {
        label: "labels",
        selector:
          "label span, .compactForm strong, .consoleInlineDetailGrid strong",
        min: 4.5,
      },
      { label: "badges", selector: ".consoleStatusBadge", min: 4.5 },
      {
        label: "disabled controls",
        selector: "button:disabled, [role='button'][aria-disabled='true']",
        min: 4.5,
      },
      {
        label: "help text",
        selector:
          ".formHint, .compactForm small, .consoleField small, .configOverrideEditor > span",
        min: 4.5,
      },
    ];

    function parseColor(value: string): Rgba | null {
      const match = value.match(/^rgba?\(([^)]+)\)$/);
      if (!match) return null;
      const parts = match[1].split(",").map((part) => part.trim());
      if (parts.length < 3) return null;
      const [r, g, b] = parts.slice(0, 3).map(Number);
      const alpha = parts[3] === undefined ? 1 : Number(parts[3]);
      if (![r, g, b, alpha].every(Number.isFinite)) return null;
      return { r, g, b, a: alpha };
    }

    function blend(foreground: Rgba, background: Rgba): Rgba {
      const alpha = foreground.a + background.a * (1 - foreground.a);
      if (alpha === 0) return { r: 255, g: 255, b: 255, a: 1 };
      return {
        r:
          (foreground.r * foreground.a +
            background.r * background.a * (1 - foreground.a)) /
          alpha,
        g:
          (foreground.g * foreground.a +
            background.g * background.a * (1 - foreground.a)) /
          alpha,
        b:
          (foreground.b * foreground.a +
            background.b * background.a * (1 - foreground.a)) /
          alpha,
        a: alpha,
      };
    }

    function visible(element: Element) {
      const style = window.getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return (
        rect.width > 0 &&
        rect.height > 0 &&
        style.display !== "none" &&
        style.visibility !== "hidden" &&
        Number(style.opacity) > 0 &&
        (element.textContent?.trim().length ?? 0) > 0
      );
    }

    function effectiveBackground(element: Element) {
      let current: Element | null = element;
      const chain: Element[] = [];
      while (current) {
        chain.unshift(current);
        current = current.parentElement;
      }
      let color: Rgba = { r: 255, g: 255, b: 255, a: 1 };
      for (const item of chain) {
        const parsed = parseColor(
          window.getComputedStyle(item).backgroundColor,
        );
        if (parsed && parsed.a > 0) {
          color = blend(parsed, color);
        }
      }
      return color;
    }

    function channel(value: number) {
      const normalized = value / 255;
      return normalized <= 0.03928
        ? normalized / 12.92
        : Math.pow((normalized + 0.055) / 1.055, 2.4);
    }

    function luminance(color: Rgba) {
      return (
        0.2126 * channel(color.r) +
        0.7152 * channel(color.g) +
        0.0722 * channel(color.b)
      );
    }

    function contrast(foreground: Rgba, background: Rgba) {
      const lighter = Math.max(luminance(foreground), luminance(background));
      const darker = Math.min(luminance(foreground), luminance(background));
      return (lighter + 0.05) / (darker + 0.05);
    }

    const failures: string[] = [];
    for (const sample of samples) {
      const elements = Array.from(document.querySelectorAll(sample.selector))
        .filter(visible)
        .slice(0, 12);
      for (const element of elements) {
        const style = window.getComputedStyle(element);
        const foreground = parseColor(style.color);
        if (!foreground) continue;
        const background = effectiveBackground(element);
        const effectiveForeground =
          foreground.a < 1 ? blend(foreground, background) : foreground;
        const ratio = contrast(effectiveForeground, background);
        if (ratio < sample.min) {
          failures.push(
            `${sample.label}: ${element.tagName.toLowerCase()} "${element.textContent
              ?.replace(/\s+/g, " ")
              .trim()
              .slice(0, 60)}" contrast ${ratio.toFixed(2)} < ${sample.min}`,
          );
        }
      }
    }
    return failures;
  });
}

test("release IA exposes the intended top-level product areas", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    for (const label of releaseTopLevel) {
      await expect(mobilePageSelector).toContainText(
        `${viewLabel(label as ActiveView)} /`,
      );
    }
    for (const label of legacyTopLevel) {
      await expect(mobilePageSelector).not.toContainText(`${label} /`);
    }
  } else {
    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    for (const label of releaseTopLevel) {
      await expect(
        nav.getByRole("button", {
          name: viewLabel(label as ActiveView),
          exact: true,
        }),
      ).toBeVisible();
    }
    for (const label of legacyTopLevel) {
      await expect(
        nav.getByRole("button", { name: label, exact: true }),
      ).toHaveCount(0);
    }
  }
});

test("keyboard navigation reaches release shell controls and page primary action", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    await expectReachableByTab(
      page,
      mobilePageSelector,
      "mobile page selector",
      80,
    );
  } else {
    await expectReachableByTab(
      page,
      page
        .getByRole("navigation", { name: "Primary console navigation" })
        .getByRole("button", { name: "Home", exact: true })
        .first(),
      "sidebar Home navigation",
      20,
    );
  }

  await expectReachableByTab(
    page,
    page.getByRole("button", { name: /All VPS resources/ }),
    "fleet scope selector",
    80,
  );
  await expectReachableByTab(
    page,
    page.getByRole("combobox", { name: "Search fleet" }),
    "global fleet search",
    80,
  );
  await expectReachableByTab(
    page,
    page.getByRole("button", { name: "Open privilege unlock" }),
    "privilege unlock control",
    100,
  );

  await openConsoleSubpage(page, "System", "Preferences");
  await page.getByLabel("Name display").selectOption("name");
  await page.getByLabel("Home telemetry curve exclusions").focus();
  await expectReachableByTab(
    page,
    page.getByRole("button", { name: "Save preferences" }),
    "page primary action",
    8,
    false,
  );
});

test("fleet scope selector edits scope and clear is explicit", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "mobile shell compression is tracked separately in newest issues",
  );

  await gotoConsoleHome(page);

  const scopeEditor = page.getByRole("button", { name: /Edit fleet scope/ });
  const fleetSearch = page.getByRole("combobox", { name: "Search fleet" });
  const clearScope = page.getByRole("button", { name: "Clear fleet scope" });

  await expect(scopeEditor).toBeVisible();
  await expect(clearScope).toBeDisabled();
  await expect(page.getByLabel("Fleet status summary")).toContainText(
    "Entire fleet",
  );
  await activate(scopeEditor);
  await expect(fleetSearch).toBeFocused();

  await fleetSearch.fill("sfo");
  await expect(scopeEditor).toContainText("Filtered resources");
  await expect(page.getByLabel("Fleet status summary")).toContainText(
    "Current scope",
  );
  await expect(clearScope).toBeEnabled();
  await activate(clearScope);
  await expect(fleetSearch).toHaveText("");
  await expect(scopeEditor).toContainText("All VPS resources");
  await expect(page.getByLabel("Fleet status summary")).toContainText(
    "Entire fleet",
  );
});

test("invalid hash changes replace stale content with the canonical home route", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Instance detail");
  await expect(
    page.getByRole("heading", { name: "Instance detail", exact: true }),
  ).toBeVisible();

  await page.evaluate(() => {
    window.location.hash = "#/remote-work/terminal";
  });

  await expect(page).toHaveURL(/#\/home\/overview$/);
  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Instance detail", exact: true }),
  ).toHaveCount(0);
});

test("invalid subpage hashes replace the URL with the rendered canonical route", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  await page.evaluate(() => {
    window.location.hash = "#/fleet/not-a-page";
  });

  await expect(page).toHaveURL(/#\/fleet\/instances$/);
  await expect(
    page.getByRole("heading", {
      name: "Fleet instances",
      exact: true,
      level: 1,
    }),
  ).toBeVisible();
});

test("visible navigation labels use readable canonical routes", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  await page.evaluate(() => {
    window.location.hash = "#/fleet/assignments";
  });
  await expect(page).toHaveURL(/#\/fleet\/assignments$/);
  await expect(
    page.getByRole("heading", {
      name: "Group assignments",
      exact: true,
      level: 1,
    }),
  ).toBeVisible();

  await page.evaluate(() => {
    window.location.hash = "#/remote/terminal";
  });
  await expect(page).toHaveURL(/#\/remote\/terminal$/);
  await expect(
    page.getByRole("heading", {
      name: "Terminal",
      exact: true,
      level: 1,
    }),
  ).toBeVisible();
});

test("home applies fleet scope to target-bound records and labels fleet-wide evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop scope editing provides the clearest coverage for mixed scoped and fleet-wide evidence",
  );

  await gotoConsoleHome(page);
  await page.getByRole("combobox", { name: "Search fleet" }).fill("sfo");

  const posture = page.getByLabel("Home posture strip");
  const alertMetric = posture.locator(".homePostureMetric").filter({
    hasText: "Open alerts",
  });
  await expect(alertMetric.locator("strong")).toHaveText("1");
  await expect(alertMetric.locator("small")).toHaveText(
    "0 critical, 1 warning, 0 info",
  );
  await expect(
    posture
      .locator(".homePostureMetric")
      .filter({ hasText: "Backups" })
      .locator("strong"),
  ).toHaveText("1");
  await expect(
    posture
      .locator(".homePostureMetric")
      .filter({ hasText: "Transfers" })
      .locator("strong"),
  ).toHaveText("3");
  await expect(page.getByLabel("Home fleet posture")).toContainText(
    "0 critical / 1 warning / 0 info",
  );
  await expect(
    page.getByRole("button", { name: /Tunnel adapter status failed/ }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Running work and fleet jobs" }),
  ).toBeVisible();
  await expect(page.getByText("Fleet audit").first()).toBeVisible();
});

test(
  "daily checks exclude muted and acknowledged alerts without hiding their records",
  { tag: "@alert-state-coverage" },
  async ({ page }) => {
    await gotoConsoleHome(page);

    const posture = page.getByLabel("Home posture strip");
    const alertMetric = posture
      .locator(".homePostureMetric")
      .filter({ hasText: "Open alerts" });
    await expect(alertMetric.locator("strong")).toHaveText("2");
    await expect(alertMetric).toContainText("1 critical, 1 warning, 0 info");
    await expect(
      page.getByText("Open daily alert", { exact: true }).first(),
    ).toBeVisible();
    await expect(
      page.getByText("Escalated daily alert", { exact: true }).first(),
    ).toBeVisible();
    await expect(
      page.getByText("Muted daily alert", { exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByText("Acknowledged daily alert", { exact: true }),
    ).toHaveCount(0);

    const shellSummary = page.getByLabel("Fleet status summary");
    await expect(
      shellSummary
        .locator(".metric")
        .filter({ hasText: "Alerts" })
        .locator("strong"),
    ).toHaveText("2");

    await openConsoleSubpage(page, "Fleet", "Alerts");
    for (const title of [
      "Open daily alert",
      "Escalated daily alert",
      "Muted daily alert",
      "Acknowledged daily alert",
    ]) {
      await expect(
        page.getByText(title, { exact: true }).first(),
      ).toBeVisible();
    }
  },
);

test(
  "bounded overview counts stay visibly lower-bound on mounted daily-check pages",
  { tag: "@dashboard-counts-truncated" },
  async ({ page }, testInfo) => {
    test.setTimeout(60_000);
    await gotoConsoleHome(page);

    const posture = page.getByLabel("Home posture strip");
    await expect(
      posture
        .locator(".homePostureMetric")
        .filter({ hasText: "Open alerts" })
        .locator("strong"),
    ).toHaveText("3");
    await expect(
      posture.locator(".homePostureMetric").filter({ hasText: "Open alerts" }),
    ).not.toContainText("in loaded page");
    await expect(
      posture
        .locator(".homePostureMetric")
        .filter({ hasText: "Running jobs" })
        .locator("strong"),
    ).toHaveText("3");
    await expect(
      posture
        .locator(".homePostureMetric")
        .filter({ hasText: "Backups" })
        .locator("strong"),
    ).toHaveText("1");

    if (!testInfo.project.name.includes("mobile")) {
      await page.getByRole("combobox", { name: "Search fleet" }).fill("sfo");
      await expect(
        posture
          .locator(".homePostureMetric")
          .filter({ hasText: "Fleet jobs" })
          .locator("strong"),
      ).toHaveText("3");
    }

    await openConsoleSubpage(page, "Observability", "Fleet metrics");
    const definitions = page.getByLabel(
      "Fleet metrics availability definitions",
    );
    await expect(
      definitions.locator("div").filter({ hasText: "Active alerts" }).first(),
    ).toContainText("≥3");
    await expect(definitions).toContainText("Alerts in shown groups");
    await expect(
      page.getByLabel("Fleet metrics group breakdown"),
    ).toContainText("≥2 alerts in the loaded operations page");

    await openConsoleSubpage(page, "Observability", "Dashboards");
    const dashboardPanel = page.locator(".observabilityDashboardsPanel");
    await expect(
      dashboardPanel
        .getByLabel("Fleet operations dashboard widgets")
        .locator(".metricCard")
        .filter({ hasText: "Completed backups" })
        .locator("strong"),
    ).toHaveText("≥1");

    if (testInfo.project.name.includes("mobile")) {
      await dashboardPanel
        .getByLabel("Dashboard preset", { exact: true })
        .selectOption({ label: "Group posture" });
    } else {
      await dashboardPanel
        .getByLabel("Dashboard preset registry")
        .getByRole("button", { name: /Group posture/ })
        .click();
    }
    const groupDashboard = dashboardPanel.getByLabel(
      "Group posture dashboard widgets",
    );
    await expect(groupDashboard).toContainText("Alerts");
    await expect(groupDashboard).toContainText("≥2");
    await expect(groupDashboard).toContainText(
      "alert/job counts use loaded operations page",
    );
  },
);

test(
  "scoped daily counts disclose saturated source pages",
  { tag: "@record-pages-saturated" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "desktop scope editing provides direct coverage of client-side filtering after global page caps",
    );
    await gotoConsoleHome(page);

    const shellAlertMetric = page
      .getByLabel("Fleet status summary")
      .locator(".metric")
      .filter({ hasText: "Alerts" });
    const posture = page.getByLabel("Home posture strip");
    const alertMetric = posture
      .locator(".homePostureMetric")
      .filter({ hasText: "Open alerts" });
    const backupMetric = posture
      .locator(".homePostureMetric")
      .filter({ hasText: "Backups" });
    const transferMetric = posture
      .locator(".homePostureMetric")
      .filter({ hasText: "Transfers" });

    await page
      .getByRole("combobox", { name: "Search fleet" })
      .fill("does-not-match");
    await expect(shellAlertMetric.locator("strong")).toHaveText("0 loaded");
    await expect(shellAlertMetric).toHaveClass(/\byellow\b/);
    await expect(shellAlertMetric).toHaveAttribute(
      "title",
      /loaded alert page; additional matching alerts may exist/,
    );
    await expect(alertMetric.locator("strong")).toHaveText("0");
    await expect(alertMetric).toHaveClass(/\binfo\b/);
    await expect(backupMetric.locator("strong")).toHaveText("0");
    await expect(backupMetric).toHaveClass(/\binfo\b/);
    await expect(transferMetric.locator("strong")).toHaveText("0");
    await expect(transferMetric).toHaveClass(/\binfo\b/);

    await page.getByRole("combobox", { name: "Search fleet" }).fill("sfo");
    await expect(shellAlertMetric.locator("strong")).toHaveText("≥1");
    await expect(shellAlertMetric).toHaveAttribute(
      "title",
      /loaded alert page; additional matching alerts may exist/,
    );

    await expect(alertMetric.locator("strong")).toHaveText("≥1");
    await expect(alertMetric).toContainText("in loaded page");

    await expect(backupMetric.locator("strong")).toHaveText("≥1");
    await expect(backupMetric).toContainText("≥1 artifact in the loaded page");

    await expect(transferMetric.locator("strong")).toHaveText("≥3");
    await expect(transferMetric).toContainText("in loaded history");

    await openConsoleSubpage(page, "Fleet", "Monitor");
    await page
      .getByLabel("VPS cards density")
      .getByRole("button", { name: "Comfortable", exact: true })
      .click();
    const card = page
      .getByLabel("VPS monitor cards")
      .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" });
    const signals = card.getByLabel("Operational signals for edge-sfo-01");
    await expect(
      signals.locator(".vpsMonitorSignal").filter({ hasText: "Alerts" }),
    ).toContainText("≥1 warning");
    await expect(
      signals.locator(".vpsMonitorSignal").filter({ hasText: "Backup" }),
    ).toContainText("≥1 recorded");
    await expect(
      signals.locator(".vpsMonitorSignal").filter({ hasText: "Transfer" }),
    ).toContainText("≥1 failed");
    await expect(card).not.toContainText("counts use capped loaded pages");

    await card.click();
    const alertFact = page
      .getByLabel("VPS resource facts")
      .locator(".vpsResourceFact")
      .filter({ hasText: "Alerts" });
    await expect(alertFact.locator("strong")).toHaveText("≥1 active");
    await expect(alertFact).toContainText("fleet alert page is capped");

    await openConsoleSubpage(page, "Fleet", "Monitor");
    await page.getByRole("combobox", { name: "Search fleet" }).fill("nyc");
    const nycCard = page
      .getByLabel("VPS monitor cards")
      .locator(".vpsMonitorCard", { hasText: "backup-nyc-03" });
    await expect(
      nycCard
        .getByLabel("Operational signals for backup-nyc-03")
        .locator(".vpsMonitorSignal")
        .filter({ hasText: "Backup" }),
    ).toContainText("None in loaded page");

    await openConsoleSubpage(page, "Fleet", "Alerts");
    await expect(
      page.getByLabel("Fleet alerts").locator(".fleetAlertHeader small"),
    ).toContainText("Loaded page:");
  },
);

test(
  "bounded alert subcounts identify the loaded page",
  { tag: "@alert-evidence-saturated" },
  async ({ page }) => {
    await gotoConsoleHome(page);
    await openConsoleSubpage(page, "Observability", "Alerts");

    const summary = page.getByLabel("Alert routing summary");
    const policyMetric = summary
      .locator(".metricCard")
      .filter({ hasText: "Policy alerts" });
    await expect(policyMetric.locator("strong")).toHaveText("200+");
    await expect(policyMetric).toContainText(
      "200 warning or critical policy-issued alerts in the loaded page",
    );

    const deliveryMetric = summary
      .locator(".metricCard")
      .filter({ hasText: "Delivery history" });
    await expect(deliveryMetric.locator("strong")).toHaveText("200+");
    await expect(deliveryMetric).toContainText(
      "200 failed retained notification deliveries in the loaded page",
    );
  },
);

test("release IA reaches every configured page and subpage", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await gotoConsoleHome(page);

  expect([...Object.keys(viewSubpages)].sort()).toEqual(
    [...releaseTopLevel].sort(),
  );

  for (const view of releaseTopLevel as ActiveView[]) {
    for (const subpage of viewSubpages[view]) {
      await openConsoleSubpage(page, view, subpage.label);

      const header = page.locator(".consoleHeader");
      await expect(
        header.getByText(`vpsman / ${viewLabel(view)} / ${subpage.label}`),
      ).toBeVisible();
      await expect(header.getByLabel("Page operational context")).toContainText(
        subpage.label,
      );
      const ownsFleetSummary =
        view === "Fleet" &&
        (subpage.id === "instance_detail" || subpage.id === "monitor");
      if (ownsFleetSummary) {
        await expect(header.getByLabel("Fleet status summary")).toHaveCount(0);
        if (subpage.id === "monitor") {
          await expect(
            page.getByLabel("VPS cards fleet summary"),
          ).toBeVisible();
        }
      } else {
        await expect(header.getByLabel("Fleet status summary")).toBeVisible();
      }
      await expect(
        page.getByText(/Http 404 \(404\)|HTTP 404 \(404\)/),
      ).toHaveCount(0);
      await expect(page.getByText(/Loading .* workspace/)).toHaveCount(0);
      await page.evaluate(() => new Promise(requestAnimationFrame));
      const tooltipViolations = await tooltipContractViolations(page);
      expect(tooltipViolations, `${view} / ${subpage.label}`).toEqual([]);
    }
  }
});

test("release pages use operational page headers", async ({ page }) => {
  await gotoConsoleHome(page);

  const defaultRoutes = [
    { view: "Home", subpage: "Overview", title: "Home", section: "Overview" },
    {
      view: "Fleet",
      subpage: "Instances",
      title: "Fleet instances",
      section: "Instances",
    },
    {
      view: "Remote Operations",
      subpage: "Terminal",
      title: "Terminal",
      section: "Terminal",
    },
    {
      view: "Jobs",
      subpage: "History",
      title: "Job history",
      section: "History",
    },
    {
      view: "Automation",
      subpage: "Schedules",
      title: "Schedules",
      section: "Schedules",
    },
    {
      view: "Network",
      subpage: "Overview",
      title: "Network overview",
      section: "Overview",
    },
    {
      view: "Backups",
      subpage: "Overview",
      title: "Backup overview",
      section: "Overview",
    },
    {
      view: "Config",
      subpage: "Overview",
      title: "Config",
      section: "Overview",
    },
    {
      view: "Observability",
      subpage: "Fleet metrics",
      title: "Fleet metrics",
      section: "Fleet metrics",
    },
    {
      view: "Audit",
      subpage: "Events",
      title: "Audit events",
      section: "Events",
    },
    {
      view: "Access",
      subpage: "Overview",
      title: "Access overview",
      section: "Overview",
    },
    {
      view: "System",
      subpage: "Overview",
      title: "System overview",
      section: "Overview",
    },
  ];

  for (const route of defaultRoutes) {
    await openConsoleSubpage(page, route.view, route.subpage);
    const header = page.locator(".consoleHeader");
    await expect(
      header.getByRole("heading", { name: route.title }),
    ).toBeVisible();
    await expect(
      header.getByText(
        `vpsman / ${viewLabel(route.view as ActiveView)} / ${route.section}`,
      ),
    ).toBeVisible();

    const context = header.getByLabel("Page operational context");
    await expect(context).toContainText("Scope");
    await expect(context).toContainText("Resources");
    await expect(context).toContainText("Section");
    await expect(context).toContainText(route.section);
    await expect(header.getByLabel("Fleet status summary")).toBeVisible();
    if (route.view === "Home") {
      await expect(header.locator(".quickStats")).toBeVisible();
      await expect(header.locator(".fleetStatusStrip")).toHaveCount(0);
    } else {
      await expect(header.locator(".quickStats")).toHaveCount(0);
      await expect(header.locator(".fleetStatusStrip")).toContainText("VPS");
    }
  }

  await openConsoleSubpage(page, "Fleet", "Monitor");
  const fleetMonitorHeader = page.locator(".consoleHeader");
  await expect(fleetMonitorHeader.locator(".quickStats")).toHaveCount(0);
  await expect(fleetMonitorHeader.locator(".fleetStatusStrip")).toHaveCount(0);
  await expect(page.getByLabel("VPS cards fleet summary")).toBeVisible();
});

test("remote operations owns terminal, files, transfers, processes, services, storage, and bulk files", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);

  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await expect(
    page.getByRole("heading", { name: "Terminal", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Terminal sessions" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(page.getByRole("heading", { name: "Files" })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "File browser" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Transfers");
  await expect(page.getByRole("heading", { name: "Transfers" })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "File transfer sessions" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  await expect(
    page.getByRole("heading", { level: 1, name: "Processes", exact: true }),
  ).toBeVisible();
  const processScope = page.getByRole("group", { name: "Process scope" });
  await expect(
    processScope.getByRole("button", { name: "Host" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    page.getByRole("heading", { name: "Host processes", exact: true }),
  ).toBeVisible();
  await activate(processScope.getByRole("button", { name: "Managed" }));
  await expect(
    page.getByRole("heading", {
      name: "Process supervisor inventory",
      exact: true,
    }),
  ).toBeVisible();
  const processGrid = page.getByLabel("Process health inventory data grid");
  if (testInfo.project.name.includes("mobile")) {
    const processCard = processGrid.locator(".gridMobileCard", {
      hasText: "ospf-worker",
    });
    await expect(processCard).toContainText("Timestamp inconsistent");
    await expect(processCard.locator(".gridMobileActions")).toHaveCount(0);
    await expect(processGrid).not.toContainText("1 processes");
  } else {
    const processRow = processGrid
      .getByRole("row")
      .filter({ hasText: "ospf-worker" })
      .first();
    await expect(processRow).toContainText("Timestamp inconsistent");
    await expect(processRow).toContainText("2 processes, 2 PIDs");
    await expect(processRow).not.toContainText("1 processes");
  }

  await openConsoleSubpage(page, "Remote Operations", "Services");
  await expect(
    page.getByRole("heading", { level: 1, name: "Services", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Host services", exact: true }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Storage");
  await expect(
    page.getByRole("heading", { level: 1, name: "Storage", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Host storage", exact: true }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Bulk files");
  await expect(page.getByRole("heading", { name: "Bulk files" })).toBeVisible();
});

test("jobs history links to operational owners without embedding their workflows", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Jobs", "History");

  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Terminal sessions" }),
  ).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "File browser" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("heading", { name: "File transfer sessions" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Host processes", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "Process supervisor inventory",
      exact: true,
    }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Host services" }),
  ).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Host storage" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("heading", { name: "Artifact cleanup" }),
  ).toHaveCount(0);

  const jobsGrid = page.getByLabel("Job records data grid");
  await expect(page.locator(".jobHistoryFreshnessFeedback")).toContainText(
    "Showing historical jobs from",
  );
  await expect(jobsGrid).toContainText("Scheduled shell command");
  await expect(jobsGrid).toContainText("2 targets");
  await expect(jobsGrid).toContainText("5s");
  await expect(jobsGrid).toContainText("Worker automation");
  await expect(
    jobsGrid.getByRole("button", { name: "Actions", exact: true }),
  ).toBeDisabled();

  if (testInfo.project.name.includes("mobile")) {
    const card = jobsGrid
      .locator(".gridMobileCard", { hasText: "Scheduled shell command" })
      .first();
    await expect(card).toContainText("completed");
    await expect(card).toContainText("Duration");
    await expect(card).toContainText("Age");
    await expect(card.locator(".gridMobileActions")).toHaveCount(0);
    await card.click();
  } else {
    for (const header of [
      "Operation",
      "Targets",
      "Result",
      "Duration",
      "Started by",
      "Age",
    ]) {
      await expect(
        jobsGrid.getByRole("columnheader", { name: new RegExp(header) }),
      ).toBeVisible();
    }
    await expect(
      jobsGrid.getByRole("columnheader", { name: /Payload/ }),
    ).toHaveCount(0);
    await jobsGrid
      .getByLabel(/Expand Job records row/)
      .first()
      .click();
  }
  await expect(jobsGrid.locator(".gridExpandedRow")).toContainText(
    "Payload hash",
  );

  if (testInfo.project.name.includes("mobile")) {
    const scheduledJobCard = jobsGrid
      .locator(".gridMobileCard", { hasText: "Scheduled shell command" })
      .first();
    await invokeGridRowAction(
      page,
      jobsGrid,
      scheduledJobCard,
      "Open target detail",
    );
  } else {
    const scheduledJobRow = jobsGrid
      .locator(".gridBody [role=row]", { hasText: "Scheduled shell command" })
      .first();
    await invokeGridRowAction(
      page,
      jobsGrid,
      scheduledJobRow,
      "Open target detail",
    );
  }
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Jobs", "History");
  const relatedLinks = page.getByLabel("Related Remote pages");
  await expect(relatedLinks).toContainText("Related workflow owners");
  for (const link of [
    { button: "Terminal", heading: "Terminal" },
    { button: "Files", heading: "Files" },
    { button: "Transfers", heading: "Transfers" },
    { button: "Processes", heading: "Processes" },
    { button: "Services", heading: "Services" },
    { button: "Storage", heading: "Storage" },
    { button: "Bulk files", heading: "Bulk files" },
  ]) {
    await openConsoleSubpage(page, "Jobs", "History");
    await relatedLinks
      .getByRole("button", { name: link.button, exact: true })
      .click();
    await expect(
      page.getByRole("heading", { level: 1, name: link.heading, exact: true }),
    ).toBeVisible();
  }
});

test("job detail opens from release evidence pages", async ({
  page,
}, testInfo) => {
  test.slow();
  await gotoConsoleHome(page);

  await homeActivityPanel(page)
    .getByRole("button", { name: /Scheduled shell command job completed/ })
    .click();
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Remote Operations", "Transfers");
  if (testInfo.project.name.includes("mobile")) {
    const transferGrid = page.getByLabel("Transfer sessions data grid");
    await invokeGridRowAction(
      page,
      transferGrid,
      transferGrid
        .locator(".gridMobileCard", { hasText: "Download from VPS" })
        .first(),
      "Job",
    );
  } else {
    const transferGrid = page.getByLabel("Transfer sessions data grid");
    const transferRow = transferGrid
      .locator(".gridBody [role=row]", { hasText: "Download from VPS" })
      .first();
    await invokeGridRowAction(page, transferGrid, transferRow, "Job");
  }
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Network", "Evidence");
  await page
    .getByLabel("Network evidence actions")
    .getByRole("button", { name: /Load output|Reload output/ })
    .click();
  await activate(
    page.getByRole("button", { name: "Open job details" }).first(),
  );
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Backups", "Artifacts");
  await activate(page.getByRole("button", { name: "Open source job details" }));
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Automation", "Agent updates");
  await activate(page.getByRole("button", { name: "Latest job" }));
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Audit", "Job evidence");
  const evidencePanel = page.locator(".auditJobEvidencePanel");
  await evidencePanel
    .getByLabel("Job evidence ledger data grid")
    .getByText("network speed test")
    .first()
    .click();
  await activate(
    evidencePanel.getByRole("button", { name: "Open in Jobs / History" }),
  );
  await expectJobHistoryDetailOpen(page);
});

test("terminal open and resume stay in Remote Operations without Jobs", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal action composer details are covered through the desktop release workflow",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const launcher = page.getByLabel("New terminal composer");
  await expect(
    launcher.getByRole("heading", { name: "New terminal" }),
  ).toBeVisible();
  const terminalTarget = launcher.getByLabel("New terminal target");
  await expect(terminalTarget).toBeVisible();
  await expect(terminalTarget).toHaveValue("");
  await expect(
    launcher.getByRole("button", { name: "Unlock privilege" }),
  ).toBeVisible();
  await expect(page.locator(".terminalCommandComposer")).toBeHidden();
  await expect(
    page.getByText("Session inventory and controls", { exact: true }),
  ).toBeVisible();
  await expect(launcher.getByLabel("New terminal columns")).toBeHidden();
  await launcher.getByText("Advanced terminal options").click();
  await expect(launcher.getByLabel("New terminal columns")).toBeVisible();

  await chooseVpsBySearch(
    launcher,
    "New terminal target",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await expect(terminalTarget).toHaveValue(/edge-sfo-01/);
  await page.getByRole("button", { name: "Open terminal" }).click();
  await expect(launcher).toContainText("terminal open job submitted");
  const terminalOpenRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(terminalOpenRequest)).not.toContain(
    "local-super-password",
  );
  expect(terminalOpenRequest).toMatchObject({
    selector_expression: "id:agent-sfo-01",
    command: "terminal_open",
    operation: {
      argv: ["/bin/sh", "-l"],
      type: "terminal_open",
    },
    privileged: true,
  });
  expect(
    (
      terminalOpenRequest as {
        privilege_assertion?: { assertion_hex?: string };
      }
    ).privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);

  const terminalGrid = page.getByLabel(
    "Session inventory and controls data grid",
  );
  const activeTerminalRow = terminalGrid
    .locator(".gridBody [role=row]", { hasText: "61616161" })
    .first();
  await invokeGridRowAction(page, terminalGrid, activeTerminalRow, "Attach");
  const focusedTerminal = page.getByLabel("Focused terminal workspace");
  await expect(focusedTerminal).toBeVisible();
  await expect(focusedTerminal).toContainText("61616161");
  await expect(
    focusedTerminal.getByLabel("Focused terminal emulator"),
  ).toBeVisible();
  await expect(page.locator(".terminalCommandComposer")).toBeHidden();
  await focusedTerminal
    .getByRole("button", { name: "Exit focused terminal view" })
    .click();
  await expect(focusedTerminal).toBeHidden();

  const terminalPanel = page.locator(".terminalSessionsPanel");
  await expect(terminalPanel).toContainText("Following");
  await terminalPanel
    .locator(".terminalActiveHeader")
    .getByRole("button", { name: "Replay" })
    .click();
  await expect(
    terminalPanel.getByLabel("Durable terminal replay status"),
  ).toContainText("Durable replay 61616161");
  await expect(
    terminalPanel.getByRole("button", { name: "Copy transcript" }),
  ).toBeEnabled();
  await expect(
    terminalPanel.getByRole("button", { name: "Download transcript" }),
  ).toBeEnabled();
  await activate(terminalPanel.getByRole("button", { name: "Evidence" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Session evidence" }),
  ).toBeVisible();
});

test("jobs dispatch keeps terminal creation in remote operations", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const jobsComposer = page.locator(".commandComposer", {
    has: page.getByRole("heading", { name: "Dispatch command" }),
  });
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
  await expect(jobsComposer.getByLabel("Dispatch mode boundary")).toContainText(
    "Advanced dispatch",
  );
  await expect(jobsComposer.getByLabel("Dispatch mode boundary")).toContainText(
    "Remote / Terminal",
  );
  await expect(
    jobsComposer.getByLabel("Dispatch operation groups").getByRole("button", {
      exact: true,
      name: "Terminal",
    }),
  ).toHaveCount(0);
  if (testInfo.project.name.includes("mobile")) {
    await expect(
      jobsComposer.getByLabel("Dispatch operation", { exact: true }),
    ).toBeVisible();
  } else {
    await expect(
      jobsComposer.getByLabel("Command operations").getByRole("button", {
        exact: true,
        name: "Argv",
      }),
    ).toBeVisible();
  }
  await expect(
    jobsComposer.getByLabel("Dispatch operation", { exact: true }),
  ).toHaveValue("shell");

  await jobsComposer.getByRole("button", { name: "Remote terminal" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Terminal" }),
  ).toBeVisible();

  await expect(
    page.getByRole("heading", { name: "New terminal" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Unlock privilege" }),
  ).toBeVisible();
  await openConsoleSubpage(page, "Jobs", "Dispatch");
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
});

test("backup dispatch keeps paths full-width and collection options compact", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Jobs", "Dispatch");
  const composer = page.locator(".commandComposer", {
    has: page.getByRole("heading", { name: "Dispatch command" }),
  });
  if (testInfo.project.name.includes("mobile")) {
    await composer
      .getByLabel("Dispatch operation", { exact: true })
      .selectOption("backup");
  } else {
    await composer
      .getByLabel("Dispatch operation groups")
      .getByRole("button", { name: "Backup", exact: true })
      .click();
  }

  const operation = composer.locator(".backupOperation");
  const options = operation.getByLabel("Backup collection options");
  await expect(operation.getByLabel("Backup selected paths")).toBeVisible();
  const layout = await operation.evaluate((element) => {
    const paths = element.querySelector("textarea");
    const optionStrip =
      element.querySelector<HTMLElement>(".backupOptionStrip");
    return {
      operationWidth: element.getBoundingClientRect().width,
      optionHeight: optionStrip?.getBoundingClientRect().height ?? 0,
      optionOverflow: optionStrip
        ? optionStrip.scrollWidth - optionStrip.clientWidth
        : Number.POSITIVE_INFINITY,
      pathsWidth: paths?.getBoundingClientRect().width ?? 0,
    };
  });
  expect(layout.pathsWidth / layout.operationWidth).toBeGreaterThan(0.9);
  expect(layout.optionOverflow).toBeLessThanOrEqual(1);
  expect(layout.optionHeight).toBeLessThanOrEqual(90);
  await expect(
    options.getByRole("checkbox", { name: "Skip missing roots" }),
  ).not.toBeChecked();
});

test("file browser reads a selected VPS path from Remote Operations without Jobs", async ({
  page,
}, testInfo) => {
  test.slow();
  test.skip(
    testInfo.project.name.includes("mobile"),
    "file browser path and editor behavior is a dense desktop operations workflow",
  );
  await gotoConsoleHome(page);
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.fileBrowser.state"),
  );
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(
    page.getByRole("heading", { name: "Files", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "File browser", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Unlock to browse this VPS.")).toBeVisible();
  await expect(page.locator(".codeMirrorShell")).toHaveCount(0);
  await expect(
    page.getByText("Download folder as archive").first(),
  ).toBeVisible();

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(
    page.getByRole("heading", { name: "File browser", exact: true }),
  ).toBeVisible();
  const targetPicker = page.getByRole("combobox", {
    name: "File browser target VPS",
  });
  await expect(targetPicker).toHaveValue("edge-sfo-01 (fo01)");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByRole("button", { name: /etc dir/ })).toBeVisible();
  await expect(page.getByLabel("Remote path")).toHaveValue("/");
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "Path",
  );
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "/",
  );
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "6 entries",
  );
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "Complete",
  );

  await page.getByRole("button", { name: /etc dir/ }).dblclick();
  await expect(page.getByRole("button", { name: /app\.conf/ })).toBeVisible();
  await page.getByRole("button", { name: /app\.conf/ }).dblclick();
  await expect(page.locator(".codeMirrorShell")).toContainText("listen=443");

  const deleteRequestsBeforeReview = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          fileBrowserJobs: Array<{ operation?: { type?: string } }>;
        };
      }
    ).__vpsmanTestRequests.fileBrowserJobs;
    return requests.filter(
      (request) => request.operation?.type === "file_delete",
    ).length;
  });
  const listRequestsBeforeReview = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          fileBrowserJobs: Array<{ operation?: { type?: string } }>;
        };
      }
    ).__vpsmanTestRequests.fileBrowserJobs;
    return requests.filter(
      (request) => request.operation?.type === "file_list_dir",
    ).length;
  });
  const deleteButton = page.getByRole("button", {
    name: "Review delete selected",
  });
  await expect(deleteButton).toBeEnabled();
  await activate(deleteButton);
  const deletePrompt = page.locator(".confirmationPrompt").last();
  await expect(deletePrompt).toContainText("Delete path");
  await expect(deletePrompt).toContainText("/etc/app.conf");
  await expect(deletePrompt).toContainText("Privilege");
  await expect(
    page.evaluate(() => {
      const requests = (
        window as unknown as {
          __vpsmanTestRequests: {
            fileBrowserJobs: Array<{ operation?: { type?: string } }>;
          };
        }
      ).__vpsmanTestRequests.fileBrowserJobs;
      return requests.filter(
        (request) => request.operation?.type === "file_delete",
      ).length;
    }),
  ).resolves.toBe(deleteRequestsBeforeReview);
  await activate(
    deletePrompt.getByRole("button", { name: "Delete path", exact: true }),
  );
  await expect
    .poll(
      () =>
        page.evaluate(() => {
          const requests = (
            window as unknown as {
              __vpsmanTestRequests: {
                fileBrowserJobs: Array<{ operation?: { type?: string } }>;
              };
            }
          ).__vpsmanTestRequests.fileBrowserJobs;
          return requests.filter(
            (request) => request.operation?.type === "file_delete",
          ).length;
        }),
      { timeout: 10_000 },
    )
    .toBe(deleteRequestsBeforeReview + 1);
  await expect(
    page.getByText("Delete /etc/app.conf completed", { exact: true }),
  ).toBeVisible();
  const listRequestsAfterDelete = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          fileBrowserJobs: Array<{ operation?: { type?: string } }>;
        };
      }
    ).__vpsmanTestRequests.fileBrowserJobs;
    return requests.filter(
      (request) => request.operation?.type === "file_list_dir",
    ).length;
  });
  expect(listRequestsAfterDelete).toBe(listRequestsBeforeReview + 1);
  await expect(page.getByRole("button", { name: /app\.conf/ })).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Refresh", exact: true }),
  ).toBeEnabled();

  await page.getByLabel("Remote path").fill("/empty");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByText("No entries under /empty").first()).toBeVisible();
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "0 entries",
  );

  await page.getByLabel("Remote path").fill("/large");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  const largeEntry = page
    .getByRole("tree")
    .getByRole("button", { name: /log-249\.txt/ });
  await largeEntry.scrollIntoViewIfNeeded();
  await expect(largeEntry).toBeVisible();
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "250 of 320 entries",
  );
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "320 scanned, capped at 320",
  );

  await page.getByLabel("Remote path").fill("/root/blocked");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(
    page.getByText(/Permission denied loading \/root\/blocked/).first(),
  ).toBeVisible();
  await expect(page.getByLabel("File browser directory state")).toContainText(
    "permission denied",
  );

  await page.getByLabel("Remote path").fill("/var/log/routing");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  const routingEntry = page
    .getByRole("tree")
    .getByRole("button", { name: /routing\.log 1\.0 MB/ });
  await routingEntry.scrollIntoViewIfNeeded();
  await routingEntry.click();
  const downloadEvent = page.waitForEvent("download");
  await activate(
    page
      .getByLabel("Selected file actions")
      .getByRole("button", { name: "Download file" }),
  );
  await downloadEvent;
  await expect(page.getByLabel("File transfer output")).toContainText(
    "1 related transfer sessions, 1 ready to download",
  );
  await activate(page.getByRole("button", { name: "Open transfers" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Transfers" }),
  ).toBeVisible();
  await expect(page.getByLabel("Focused transfer path")).toContainText(
    "/var/log/routing/routing.log",
  );
  await expect(page.getByLabel("Focused transfer path")).toContainText(
    "2 matching sessions",
  );
  await expect(page.getByLabel("Focused transfer path")).toContainText(
    "2 ready to download",
  );
  await expect(page.getByLabel("Transfer sessions data grid")).toContainText(
    "5 of 5 transfers",
  );
  await expect(page.getByLabel("Transfer sessions data grid")).toContainText(
    "/var/log/routing/routing.log",
  );
  await expect(page.getByLabel("Transfer sessions data grid")).toContainText(
    "edge-sfo-01",
  );

  const fileBrowserRequests = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          fileBrowserJobs: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests.fileBrowserJobs;
    return requests.map((request) => ({
      operationType: (request.operation as { type?: string } | undefined)?.type,
      selector: request.selector_expression,
    }));
  });
  expect(fileBrowserRequests).toContainEqual({
    operationType: "file_list_dir",
    selector: "id:agent-sfo-01",
  });
  expect(fileBrowserRequests).toContainEqual({
    operationType: "file_read_text",
    selector: "id:agent-sfo-01",
  });
  const deleteRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          fileBrowserJobs: Array<{
            confirmed?: boolean;
            destructive?: boolean;
            operation?: { path?: string; type?: string };
            privileged?: boolean;
            selector_expression?: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.fileBrowserJobs;
    return requests.find(
      (request) => request.operation?.type === "file_delete",
    );
  });
  expect(deleteRequest).toMatchObject({
    confirmed: true,
    destructive: true,
    operation: {
      path: "/etc/app.conf",
      type: "file_delete",
    },
    privileged: true,
    selector_expression: "id:agent-sfo-01",
  });
});

test("home exposes quick actions, availability, running work, failures, attention, and activity", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  await expect(
    page.getByRole("heading", { name: "Fleet command home" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home posture strip")).toContainText("Live VPS");
  const quickActions = page.getByLabel("Home quick actions");
  await expect(
    quickActions.getByLabel("Home quick action target"),
  ).toBeVisible();
  for (const action of [
    "Open terminal",
    "Browse files",
    "Dispatch command",
    "Run backup",
    "View network",
  ]) {
    await expect(
      quickActions.getByRole("button", { name: action }),
    ).toBeEnabled();
  }

  await expect(page.getByLabel("Home fleet scan")).toBeVisible();
  await expect(page.getByLabel("Home telemetry widgets")).toBeVisible();
  await expect(page.locator("body")).not.toContainText(
    "artifact_metadata_recorded",
  );

  const runningPanel = homePanel(page, "Running work");
  await expect(runningPanel).toBeVisible();
  await expect(
    runningPanel.getByRole("button", { name: /3 fleet jobs running/ }),
  ).toBeVisible();
  await expect(runningPanel).toContainText("Fleet summary");

  const failurePanel = homePanel(page, "Recent issues");
  await expect(failurePanel).toBeVisible();
  await expect(
    failurePanel.getByRole("button", {
      name: /Tunnel adapter status failed/,
    }),
  ).toBeVisible();
  await expect(
    failurePanel.getByRole("button", { name: /Transfer .* aborted/ }),
  ).toBeVisible();

  await expect(
    page.getByRole("heading", { name: "Needs attention" }),
  ).toBeVisible();
  const attentionPanel = homeAttentionPanel(page);
  await expect(
    attentionPanel.getByRole("button", {
      name: /Tunnel adapter status failed/,
    }),
  ).toBeVisible();
  await expect(
    attentionPanel.getByRole("button", { name: /backup-nyc-03 needs review/ }),
  ).toBeVisible();
  await expect(
    attentionPanel.getByRole("button", {
      name: /Gateway event drops need review/,
    }),
  ).toBeVisible();
  const attentionTime = attentionPanel.locator(".homeActionMeta").first();
  await expect(attentionTime).toContainText(/ago|in|just now/);
  await expect(attentionTime).toHaveAttribute("title", /2026.*(GMT|UTC)/);
  const attentionDetail = attentionPanel
    .locator(".homeActionText small")
    .first();
  await expect(attentionDetail).not.toHaveAttribute("title", /\S/);
  await expect(
    page.getByRole("heading", { name: "Recent activity" }),
  ).toBeVisible();
  await expect(
    homeActivityPanel(page).getByRole("button", { name: /privilege unlock/i }),
  ).toBeVisible();
  const activityTime = homeActivityPanel(page).locator("time").first();
  await expect(activityTime).toContainText(/ago|in|just now/);
  await expect(activityTime).toHaveAttribute("title", /2026.*(GMT|UTC)/);
});

test("home quick actions route to release pages with selected VPS scope", async ({
  page,
}) => {
  await clickHomeQuickAction(page, "Open terminal");
  await expect(
    page.getByRole("heading", { name: "Terminal", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Terminal sessions" }),
  ).toBeVisible();

  await clickHomeQuickAction(page, "Browse files");
  await expect(
    page.getByRole("heading", { name: "Files", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "File browser" }),
  ).toBeVisible();

  await clickHomeQuickAction(page, "Dispatch command");
  await expect(
    page.getByRole("heading", { name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "Bulk target selector expression" }),
  ).toHaveValue("id:agent-sfo-01");

  await clickHomeQuickAction(page, "Run backup");
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup requests" }),
  ).toBeVisible();

  await clickHomeQuickAction(page, "View network");
  await expect(
    page.getByRole("heading", { name: "Network graph" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Topology graph" }),
  ).toBeVisible();
});

test("home attention queue links to release evidence pages", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  await homeAttentionPanel(page)
    .getByRole("button", { name: /Tunnel adapter status failed/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network evidence" }),
  ).toBeVisible();

  await gotoConsoleHome(page);
  await homeAttentionPanel(page)
    .getByRole("button", { name: /Transfer .*error\.log/ })
    .click();
  await expect(
    page.getByRole("heading", { name: "Transfers", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "File transfer sessions" }),
  ).toBeVisible();

  await gotoConsoleHome(page);
  await homeAttentionPanel(page)
    .getByRole("button", { name: /backup-nyc-03 needs review/ })
    .click();
  await expect(
    page.getByRole("heading", { name: "Instance detail" }),
  ).toBeVisible();

  await gotoConsoleHome(page);
  await homeAttentionPanel(page)
    .getByRole("button", { name: /Gateway event drops need review/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "System capacity" }),
  ).toBeVisible();
});

test("home shows a useful empty state when no VPS agents are loaded", async ({
  page,
}) => {
  await installConsoleApiMock(page, { agentListOverride: [] });
  await gotoConsoleHome(page);

  await expect(
    page.getByRole("heading", { name: "Fleet command home" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home quick action target")).toBeVisible();
  await expect(
    page
      .getByLabel("Home quick actions")
      .getByRole("button", { name: "Open terminal" }),
  ).toBeDisabled();
  await expect(page.getByLabel("Home posture strip")).toContainText("0/0");
  await expect(page.getByLabel("Home empty scope notice")).toBeVisible();
  await activate(
    page
      .getByLabel("Home empty scope notice")
      .getByRole("button", { name: "Register VPS" }),
  );
  await expect(
    page.getByText("vpsman / Access / VPS identities"),
  ).toBeVisible();
  await expect(
    page
      .locator(".identityWorkflowPanel")
      .getByRole("heading", { name: "Register VPS" }),
  ).toBeVisible();
});

test("home overview text fits desktop tablet and mobile widths", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "viewport sweep explicitly covers mobile width from the desktop project",
  );
  for (const viewport of [
    { height: 900, label: "desktop", width: 1440 },
    { height: 900, label: "tablet", width: 900 },
    { height: 844, label: "mobile", width: 390 },
  ]) {
    await page.setViewportSize({
      height: viewport.height,
      width: viewport.width,
    });
    await gotoConsoleHome(page);
    await expect(
      page.getByRole("heading", { name: "Fleet command home" }),
    ).toBeVisible();
    await expect(homePanel(page, "Running work")).toBeVisible();
    await expect(homePanel(page, "Recent issues")).toBeVisible();
    await expectHomeOverviewToFit(page, viewport.label);
  }
});

test("home refresh defaults to 15 seconds without replacing valid saved intervals", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  const refreshInterval = page.getByLabel("Home refresh interval");
  await expect(refreshInterval).toHaveValue("15");
  await expect(refreshInterval.locator("option")).toHaveText([
    "5s",
    "15s",
    "30s",
    "1m",
  ]);

  for (const [stored, expected] of [
    [999, "15"],
    [5, "5"],
    [30, "30"],
    [60, "60"],
  ] as const) {
    await page.evaluate((refreshIntervalSecs) => {
      window.localStorage.setItem(
        "vpsman.dashboardPreferences",
        JSON.stringify({ refreshIntervalSecs }),
      );
    }, stored);
    await page.reload();
    await waitForConsoleShell(page);
    await expect(page.getByLabel("Home refresh interval")).toHaveValue(
      expected,
    );
  }
});

test("fleet monitor keeps unsettled evidence neutral while cards load", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the loading lifecycle is density-independent and is covered once on desktop",
  );
  await installConsoleApiMock(page, {
    agentListOverride: makeMonitorAgentFixtures(2)
      .slice(1)
      .map((agent) => ({
        ...agent,
        last_seen_at: new Date().toISOString(),
      })),
    monitoringCardsDelayMs: 1_500,
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  await expect(page.getByText("Loading monitoring evidence…")).toBeVisible();
  const monitor = page.getByLabel("VPS monitor cards");
  const onlineCard = monitor
    .locator(".vpsMonitorCard", { hasText: "fleet-002-de" })
    .first();
  await expect(onlineCard).toHaveClass(/\bonline\b/);
  await expect(onlineCard).not.toHaveClass(/\bwarning\b/);
  await expect(onlineCard.locator(".vpsMonitorTraffic")).not.toHaveClass(
    /\bunconfigured\b/,
  );
  await expect(onlineCard.getByText("Online", { exact: true })).toBeVisible();
  await expect(
    page
      .getByLabel("VPS cards fleet summary")
      .getByRole("button", { name: /Warning/ }),
  ).toContainText("0");

  await expect(page.getByText("1 matched")).toBeVisible();
});

test("fleet monitor keeps an intentionally empty rate selection out of partial telemetry", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the telemetry-state contract is viewport independent",
  );
  await installConsoleApiMock(page, {
    monitoringNetworkRateExpectedOverride: false,
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");
  await expect(page.getByText(/matched$/)).toBeVisible();

  const card = page
    .getByLabel("VPS monitor cards")
    .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" })
    .first();
  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Comfortable" })
    .click();
  await expect(card.locator(".vpsMonitorFlowFacts strong")).toHaveText([
    "-",
    "-",
  ]);
  await expect(card.locator(".telemetryEvidence")).not.toHaveClass(/partial/);
  await expect(card.locator(".telemetryEvidence")).toHaveAttribute(
    "title",
    /no live-rate interfaces are selected/i,
  );
});

test("fleet monitor presents no-reset traffic as accumulated evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the no-reset card and detail contract is density-independent",
  );
  await installConsoleApiMock(page, {
    trafficAccountingOverride: {
      cycle_end: null,
      cycle_start: null,
      reset_day: -1,
    },
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  const card = page
    .getByLabel("VPS monitor cards")
    .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" })
    .first();
  await expect(
    card.locator(".vpsMonitorTraffic .vpsMonitorRowHeading"),
  ).toHaveText("Traffic");
  await card.click();
  await page.getByRole("tab", { name: "Resources", exact: true }).click();
  const traffic = page.locator(".vpsMonitoringTrafficCycle");
  await expect(traffic.getByText("Traffic", { exact: true })).toBeVisible();
  await expect(traffic).toContainText("Accumulated total");
  await expect(traffic).not.toContainText("No reset");
  await expect(traffic).not.toContainText("Current accounting cycle");
});

test("comfortable fleet cards align configured Ping and collapse unconfigured evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the focused geometry assertion is viewport-independent and runs once on desktop",
  );
  await installConsoleApiMock(page, {
    agentListOverride: makeMonitorAgentFixtures(3).map((agent) => ({
      ...agent,
      last_seen_at: new Date().toISOString(),
      status: "online" as const,
    })),
    monitoringPingStateCoverage: true,
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  const monitor = page.getByLabel("VPS monitor cards");
  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Comfortable" })
    .click();
  await expect(monitor).toHaveAttribute("data-density", "comfortable");
  const healthy = monitor
    .locator(".vpsMonitorCard", { hasText: "fleet-001-us" })
    .locator(".vpsMonitorPing");
  const degraded = monitor
    .locator(".vpsMonitorCard", { hasText: "fleet-002-de" })
    .locator(".vpsMonitorPing");
  const unconfigured = monitor
    .locator(".vpsMonitorCard", { hasText: "fleet-003-sg" })
    .locator(".vpsMonitorPing");

  await expect(healthy.locator(".vpsMonitorRowHeading")).toHaveText(
    "Ping · Fixture healthy gateway",
  );
  await expect(healthy.locator(".vpsMonitorPingEvidence > strong")).toHaveText([
    "18.5 ms",
    "0% loss",
  ]);
  await expect(healthy.locator(":scope > .vpsMonitorPingDetail")).toHaveCount(
    0,
  );
  await expect(degraded.locator(":scope > .vpsMonitorPingDetail")).toHaveText(
    "Intermittent packet loss",
  );
  await expect(degraded).toHaveAttribute(
    "title",
    /Fixture degraded gateway.*68(?:\.0)? ms.*20% loss.*Intermittent packet loss/i,
  );
  await expect(
    unconfigured.locator(
      ".vpsMonitorPingHeading > .vpsMonitorPingEvidence > strong",
    ),
  ).toHaveText("Unconfigured");
  await expect(
    unconfigured.locator(":scope > .vpsMonitorPingDetail"),
  ).toHaveCount(0);
  await expectShorterElement(healthy, degraded);
  await expectShorterElement(unconfigured, healthy);

  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Compact" })
    .click();
  await expect(monitor.locator(".vpsMonitorPingDetail")).toHaveCount(0);
});

test("fleet monitor cards are density-distinct and open canonical detail", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop card interaction covers density and detail routing without doubling the mobile suite",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  await expect(
    page.getByRole("heading", { name: "Fleet monitor" }),
  ).toBeVisible();
  const monitor = page.getByLabel("VPS monitor cards");
  await expect(monitor).toBeVisible();
  await expect(monitor).toHaveAttribute("data-density", "compact");
  await expect
    .poll(() =>
      page.evaluate(
        (storageKey) => window.localStorage.getItem(storageKey),
        OPERATOR_MONITOR_DENSITY_STORAGE_KEY,
      ),
    )
    .toBe("compact");
  const edgeCard = monitor
    .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" })
    .first();
  await expect(edgeCard).toHaveAttribute("role", "link");
  const billingFact = edgeCard.locator(
    '.vpsMonitorAuxFacts > [data-fact-kind="billing"]',
  );
  await expect(billingFact.locator(".vpsMonitorAuxFactHeading")).toHaveText(
    "Billing · Renews day 14",
  );
  await expect(billingFact.locator(":scope > strong")).toHaveText("29.90 ¥/m");
  await expect(billingFact.locator(":scope > em")).toHaveCount(0);
  await expect(
    monitor
      .locator(".vpsMonitorCard", { hasText: "core-fra-02" })
      .locator('[data-fact-kind="billing"]'),
  ).toContainText("Billing-");
  const unconfiguredTraffic = monitor
    .locator(".vpsMonitorCard", { hasText: "core-fra-02" })
    .locator(".vpsMonitorTraffic");
  await expect(unconfiguredTraffic).toHaveClass(/\bunconfigured\b/);
  await expect(
    unconfiguredTraffic.locator(".vpsMonitorTrafficTrack.missing"),
  ).toBeVisible();
  await expect(
    edgeCard.locator('.vpsMonitorAuxFacts > [data-fact-kind="uptime"] strong'),
  ).toHaveText("8d 3h");
  await expect(
    edgeCard.locator('.vpsMonitorAuxFacts > [data-fact-kind="uptime"]'),
  ).toHaveAttribute("title", /^Up since .+2026/);
  await expect(
    edgeCard.locator('.vpsMonitorAuxFacts > [data-fact-kind="uptime"]'),
  ).toHaveCSS("flex-basis", "68px");
  await expect(
    edgeCard
      .locator('.vpsMonitorAuxFacts > [data-fact-kind="connection"]')
      .first(),
  ).toHaveCSS("flex-basis", "42px");
  const snapshot = page.getByLabel("VPS cards current totals");
  await expect(snapshot.locator("strong[title], em[title]")).toHaveCount(0);
  await expect(edgeCard.locator(".vpsMonitorTraffic")).toHaveAttribute(
    "title",
    /traffic/i,
  );
  await expect(
    edgeCard.locator(".vpsMonitorTraffic .vpsMonitorTrafficQuota"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(
    edgeCard.locator(".vpsMonitorTraffic .vpsMonitorTrafficQuota"),
  ).toContainText("· Total · 80.3%");
  await expect(edgeCard.locator(".vpsMonitorTraffic > small")).toHaveCount(0);
  const trafficHeading = edgeCard.locator(
    ".vpsMonitorTraffic .vpsMonitorRowHeading",
  );
  await expect(trafficHeading.locator("strong")).toHaveText("Traffic");
  await expect(trafficHeading).toContainText(/Resets|Cycle reset/);
  await expect(
    edgeCard.locator(".vpsMonitorTraffic .publicMonitoringPortSpeed"),
  ).toContainText("1.5 Gbps");
  await expect(edgeCard.locator(".vpsMonitorPing")).toHaveAttribute(
    "title",
    /Ping/,
  );
  await expect(
    edgeCard.locator(".vpsMonitorPing .vpsMonitorPingEvidence > strong"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(
    edgeCard.locator(".vpsMonitorPing .vpsMonitorRowHeading strong"),
  ).toHaveText("Ping");
  await expect(
    edgeCard.locator(".vpsMonitorPingHeading > .vpsMonitorRowHeading"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(edgeCard.getByText("No contact").first()).toBeVisible();
  await expect(
    edgeCard.locator(".vpsMonitorCardMain > small"),
  ).not.toBeVisible();
  await expect(edgeCard.locator(".comfortableSummary")).toHaveCount(0);
  await expect(edgeCard.locator("button, a, summary, details")).toHaveCount(0);
  const resourceMetrics = edgeCard.locator(".vpsMonitorMetric");
  await expect(resourceMetrics.locator(":scope > strong small")).toHaveText([
    "(4-core)",
    "(8.0 GB)",
    "(100 GB)",
  ]);
  for (const index of [0, 1, 2]) {
    await expect(
      resourceMetrics.nth(index).locator(":scope > small"),
    ).toHaveCount(0);
  }
  const compactFirstRow = await monitorFirstRowCount(monitor);
  const compactCardWidth = await monitor
    .locator(".vpsMonitorCard")
    .first()
    .evaluate((card) => card.getBoundingClientRect().width);
  const compactCardHeight = await edgeCard.evaluate(
    (card) => card.getBoundingClientRect().height,
  );

  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Comfortable" })
    .click();
  await expect(monitor).toHaveAttribute("data-density", "comfortable");
  const comfortableFirstRow = await monitorFirstRowCount(monitor);
  const comfortableCardWidth = await monitor
    .locator(".vpsMonitorCard")
    .first()
    .evaluate((card) => card.getBoundingClientRect().width);
  const comfortableCardHeight = await edgeCard.evaluate(
    (card) => card.getBoundingClientRect().height,
  );
  expect(comfortableFirstRow).toBeGreaterThanOrEqual(2);
  expect(compactFirstRow).toBeGreaterThanOrEqual(comfortableFirstRow);
  expect(compactCardWidth).toBeLessThan(comfortableCardWidth);
  expect(compactCardHeight).toBeLessThan(comfortableCardHeight);
  expect(comfortableCardHeight).toBeLessThan(460);
  await expect(edgeCard.locator(".vpsMonitorCardMain > small")).toBeVisible();
  await expect(edgeCard.locator(".vpsMonitorCardMain > small")).toContainText(
    "alpha",
  );
  await expect(edgeCard.locator(".vpsMonitorAuxFacts")).toContainText(
    "29.90 ¥/m",
  );
  await expect(edgeCard.locator(".vpsMonitorAuxFacts")).toContainText(
    "Renews day 14",
  );
  await expect(
    edgeCard.locator('.vpsMonitorAuxFacts > [data-fact-kind="uptime"] strong'),
  ).toHaveText("8d 3h");
  await expect(edgeCard).toContainText("1m load");
  await expect(edgeCard).not.toContainText(/No (recent|continuous) history/);
  await expect(edgeCard.locator(".comfortableSummary")).toHaveCount(1);
  await expect(
    edgeCard.locator(".vpsMonitorTrafficEvidenceRow > small"),
  ).toContainText(/↓ .+ · ↑ /);
  await expect(
    edgeCard.locator(".vpsMonitorTrafficEvidenceRow > small"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(edgeCard.locator("button, a, summary, details")).toHaveCount(0);
  for (const index of [0, 1, 2]) {
    await expect(
      resourceMetrics.nth(index).locator(":scope > small"),
    ).toHaveCount(0);
  }

  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Compact" })
    .click();
  await expect(monitor).toHaveAttribute("data-density", "compact");

  await page.getByLabel("VPS cards sort").selectOption("traffic");
  await expect(monitor).toHaveAttribute("data-sort", "traffic");
  await edgeCard.locator(".vpsMonitorTraffic").click();
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\//);
  await expect(
    page.getByLabel("Canonical VPS detail").getByLabel("Selected VPS identity"),
  ).toContainText("edge-sfo-01");

  await page.goBack();
  await expect(monitor).toHaveAttribute("data-density", "compact");
  await expect(monitor).toHaveAttribute("data-sort", "traffic");
  await edgeCard.focus();
  await expect(edgeCard).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\//);
  await page.goBack();
  await expect(monitor).toHaveAttribute("data-density", "compact");
  await page.reload();
  await expect(monitor).toHaveAttribute("data-density", "compact");
  await expect(monitor).toHaveAttribute("data-sort", "warning");
});

test("fleet monitor sorts warning ties by name and client ID", async ({
  page,
}) => {
  const names = ["Zulu offline", "Alpha", "Alpha", "Beta"];
  const ids = ["sort-offline", "sort-b", "sort-a", "sort-beta"];
  await installConsoleApiMock(page, {
    agentListOverride: makeMonitorAgentFixtures(4).map((agent, index) => ({
      ...agent,
      display_name: names[index],
      id: ids[index],
      last_seen_at: new Date().toISOString(),
      status: index === 0 ? ("offline" as const) : ("online" as const),
    })),
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  const sort = page.getByLabel("VPS cards sort");
  await expect(sort.locator("option").nth(0)).toHaveText("Warnings first");
  await expect(sort.locator("option").nth(1)).toHaveText("Name");

  const monitor = page.getByLabel("VPS monitor cards");
  const cardNames = monitor.locator(".vpsMonitorCardNameText");
  await expect(cardNames).toHaveText([
    "Zulu offline",
    "Alpha",
    "Alpha",
    "Beta",
  ]);
  await expect(
    monitor.locator(".vpsMonitorCardMain > small").nth(1),
  ).toContainText("gamma");
  await expect(
    monitor.locator(".vpsMonitorCardMain > small").nth(2),
  ).toContainText("beta");

  await sort.selectOption("name");
  await expect(cardNames).toHaveText([
    "Alpha",
    "Alpha",
    "Beta",
    "Zulu offline",
  ]);
  await expect(monitor).toHaveAttribute("data-sort", "name");
});

for (const fixtureCount of [0, 1, 8, 20, 100, 1_000]) {
  test(`fleet monitor cards remain readable for ${fixtureCount} generated VPS fixtures`, async ({
    page,
  }, testInfo) => {
    if (fixtureCount >= 1_000) test.setTimeout(60_000);
    test.skip(
      testInfo.project.name.includes("mobile"),
      "desktop fixture-count sweep covers high-card rendering without doubling suite time",
    );
    await installConsoleApiMock(page, {
      agentListOverride: makeMonitorAgentFixtures(fixtureCount),
    });
    await page.setViewportSize({ height: 900, width: 1280 });
    await gotoConsoleHome(page);
    await openConsoleSubpage(page, "Fleet", "Monitor");

    const monitor = page.getByLabel("VPS monitor cards");
    if (fixtureCount === 0) {
      await expect(page.getByText("No VPS cards to show")).toBeVisible();
      await expect(monitor).toHaveCount(0);
      return;
    }
    await expect(monitor).toBeVisible();
    await expect
      .poll(
        async () => {
          const rendered = await monitor.locator(".vpsMonitorCard").count();
          if (rendered < fixtureCount) {
            await page
              .locator(".fleetMonitorProgress")
              .scrollIntoViewIfNeeded()
              .catch(() => undefined);
          }
          return rendered;
        },
        {
          message: `progressively render all ${fixtureCount} matched VPS cards`,
          timeout: fixtureCount >= 1_000 ? 40_000 : 5_000,
        },
      )
      .toBe(fixtureCount);
    await expect(monitor).toHaveAttribute("data-density", "compact");
    await expectMonitorCardsToFit(page, `${fixtureCount} generated VPS`);
  });
}

test("fleet monitor density remains responsive on a narrow viewport", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the desktop project performs the explicit viewport transition",
  );
  await installConsoleApiMock(page, {
    agentListOverride: makeMonitorAgentFixtures(20),
  });
  await page.setViewportSize({ height: 844, width: 390 });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  const monitor = page.getByLabel("VPS monitor cards");
  await expect(monitor).toHaveAttribute("data-density", "compact");
  expect(await monitorFirstRowCount(monitor)).toBe(1);
  expect(await monitorFirstRowCount(monitor)).toBe(1);
  await expectMonitorCardsToFit(page, "narrow compact VPS");
});

test("fleet monitor densities remain distinct at a common laptop width", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the desktop project performs the explicit viewport transition",
  );
  await installConsoleApiMock(page, {
    agentListOverride: makeMonitorAgentFixtures(20),
  });
  await page.setViewportSize({ height: 900, width: 1024 });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");

  const monitor = page.getByLabel("VPS monitor cards");
  await expect(monitor).toHaveAttribute("data-density", "compact");
  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Comfortable" })
    .click();
  await expect(monitor).toHaveAttribute("data-density", "comfortable");
  const comfortableFirstRow = await monitorFirstRowCount(monitor);
  expect(comfortableFirstRow).toBeGreaterThanOrEqual(2);
  await expectMonitorCardsToFit(page, "laptop comfortable VPS");

  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Compact" })
    .click();
  const compactFirstRow = await monitorFirstRowCount(monitor);
  expect(compactFirstRow).toBeGreaterThan(comfortableFirstRow);
  await expectMonitorCardsToFit(page, "laptop compact VPS");

  const content = page.locator(".content");
  const lowerCard = monitor.locator(".vpsMonitorCard").nth(16);
  await lowerCard.scrollIntoViewIfNeeded();
  await expect
    .poll(() => content.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(100);
  const savedScrollTop = await content.evaluate((element) => element.scrollTop);
  await page.waitForTimeout(50);
  await lowerCard.click();
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\//);
  await page.goBack();
  await expect(monitor).toHaveAttribute("data-density", "compact");
  // Ready-state unconfigured Traffic and Ping rows intentionally collapse their
  // empty evidence lines; preserve the same scroll neighborhood across remount.
  await expect
    .poll(() =>
      content.evaluate((element, priorScrollTop) => {
        const restoredScrollTop = Math.min(
          priorScrollTop,
          element.scrollHeight - element.clientHeight,
        );
        return Math.abs(element.scrollTop - restoredScrollTop);
      }, savedScrollTop),
    )
    .toBeLessThanOrEqual(40);
});

test("startup WebSocket core preserves the in-flight HTTP telemetry snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the startup snapshot ordering contract is viewport independent",
  );
  await installConsoleApiMock(page, { holdInitialFleetSnapshots: true });
  await page.goto("/");
  await waitForConsoleShell(page);

  const fleetSnapshotModes = () =>
    page.evaluate(
      () =>
        (
          window as typeof window & {
            __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
          }
        ).__vpsmanFetchRequests
          ?.filter(
            (request) =>
              request.method === "GET" &&
              new URL(request.url, window.location.href).pathname ===
                "/api/v1/fleet/snapshot",
          )
          .map(
            (request) =>
              new URL(request.url, window.location.href).searchParams.get(
                "mode",
              ) ?? "",
          ) ?? [],
    );
  await expect.poll(fleetSnapshotModes).toContain("full");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanTestWebSockets: EventTarget[];
            }
          ).__vpsmanTestWebSockets.length,
      ),
    )
    .toBeGreaterThan(0);

  await page.evaluate(async () => {
    const [summaryResponse, agentsResponse] = await Promise.all([
      window.fetch("/api/v1/fleet/summary"),
      window.fetch("/api/v1/agents"),
    ]);
    const summary = (await summaryResponse.json()) as Record<string, unknown>;
    const agents = (await agentsResponse.json()) as Array<
      Record<string, unknown>
    >;
    agents[0] = { ...agents[0], display_name: "WS startup core" };
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: EventTarget[];
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "fleet_snapshot",
          summary: { ...summary, running_jobs: 91 },
          agents,
        }),
      }),
    );
  });
  await expect(page.getByText(/WS startup core/).first()).toBeVisible();

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanReleaseFleetSnapshots?: () => void;
      }
    ).__vpsmanReleaseFleetSnapshots?.();
  });
  await openConsoleSubpage(page, "Fleet", "Monitor");
  const startupCard = page
    .getByLabel("VPS monitor cards")
    .locator(".vpsMonitorCard", { hasText: "WS startup core" });
  await expect(startupCard).toBeVisible();
  await expect(
    startupCard.locator('[data-fact-kind="uptime"] strong'),
  ).toHaveText("8d 3h");
  expect((await fleetSnapshotModes()).every((mode) => mode === "full")).toBe(
    true,
  );
});

test("connected fleet consumes one aggregate telemetry invalidation without live polling", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the polling contract is viewport independent",
  );
  await page.clock.install({ time: new Date("2026-06-02T10:02:00Z") });
  await gotoConsoleHome(page);
  const clockTime = await page.evaluate(() => Date.now());
  await page.clock.pauseAt(clockTime + 1_000);
  await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
    };
    trackedWindow.__vpsmanFetchRequests = [];
  });
  const fleetSnapshotModes = () =>
    page.evaluate(() => {
      const trackedWindow = window as typeof window & {
        __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      };
      return (trackedWindow.__vpsmanFetchRequests ?? [])
        .filter((request) => request.method === "GET")
        .map((request) => new URL(request.url, window.location.href))
        .filter((url) => url.pathname === "/api/v1/fleet/snapshot")
        .map((url) => url.searchParams.get("mode") ?? "");
    });

  await page.clock.runFor(15_500);
  expect(await fleetSnapshotModes()).toEqual([]);

  await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanTestWebSockets: Array<EventTarget>;
    };
    trackedWindow.__vpsmanTestWebSockets.at(-1)?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ type: "fleet_telemetry_invalidated" }),
      }),
    );
  });

  await expect.poll(fleetSnapshotModes).toEqual(["live"]);
  await page.clock.runFor(15_000);
  expect(await fleetSnapshotModes()).toEqual(["live"]);

  const urls = await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
    };
    return (trackedWindow.__vpsmanFetchRequests ?? []).map(
      (request) => request.url,
    );
  });
  for (const replacedPath of [
    "/api/v1/fleet/summary",
    "/api/v1/agents",
    "/api/v1/telemetry/rollups",
    "/api/v1/telemetry/network-rates",
    "/api/v1/telemetry/tunnels",
  ]) {
    expect(urls.some((url) => url.includes(replacedPath))).toBe(false);
  }
  for (const operationalPath of [
    "/api/v1/fleet-alert-policies",
    "/api/v1/fleet-alert-notification-channels",
    "/api/v1/webhook-rules",
    "/api/v1/webhook-deliveries",
    "/api/v1/vps-rules",
  ]) {
    expect(urls.some((url) => url.includes(operationalPath))).toBe(false);
  }

  await page.clock.runFor(30_000);
  await expect.poll(fleetSnapshotModes).toEqual(["live", "full"]);
});

test("reconnecting fleet performs one live recovery refresh", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the recovery contract is viewport independent",
  );
  await page.clock.install({ time: new Date("2026-06-02T10:02:00Z") });
  await gotoConsoleHome(page);
  const clockTime = await page.evaluate(() => Date.now());
  await page.clock.pauseAt(clockTime + 1_000);
  await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      __vpsmanTestWebSockets: Array<{ close: () => void }>;
    };
    trackedWindow.__vpsmanFetchRequests = [];
    trackedWindow.__vpsmanTestWebSockets.at(-1)?.close();
  });
  await page.clock.runFor(1_100);
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as typeof window & {
            __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
          }
        ).__vpsmanFetchRequests;
        return (requests ?? []).filter(
          (request) =>
            request.method === "GET" &&
            request.url.includes("/api/v1/fleet/snapshot") &&
            request.url.includes("mode=live"),
        ).length;
      }),
    )
    .toBe(1);
  await page.clock.runFor(14_000);
  const liveRefreshCount = await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
    };
    return (trackedWindow.__vpsmanFetchRequests ?? []).filter(
      (request) =>
        request.method === "GET" &&
        request.url.includes("/api/v1/fleet/snapshot") &&
        request.url.includes("mode=live"),
    ).length;
  });
  expect(liveRefreshCount).toBe(1);
});

test("disconnected fleet falls back to one live snapshot per 15 seconds", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the fallback contract is viewport independent",
  );
  await page.clock.install({ time: new Date("2026-06-02T10:02:00Z") });
  await gotoConsoleHome(page);
  const clockTime = await page.evaluate(() => Date.now());
  await page.clock.pauseAt(clockTime + 1_000);
  await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      __vpsmanTestWebSockets: Array<{ close: () => void }>;
    };
    trackedWindow.__vpsmanFetchRequests = [];
    Object.defineProperty(window, "WebSocket", {
      configurable: true,
      value: class StalledWebSocket extends EventTarget {
        static CONNECTING = 0;
        static OPEN = 1;
        static CLOSING = 2;
        static CLOSED = 3;
        readyState = StalledWebSocket.CONNECTING;
        close() {
          this.readyState = StalledWebSocket.CLOSED;
        }
        send() {}
      },
    });
    trackedWindow.__vpsmanTestWebSockets.at(-1)?.close();
  });

  await page.clock.runFor(15_100);
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as typeof window & {
            __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
          }
        ).__vpsmanFetchRequests;
        return (requests ?? []).filter(
          (request) =>
            request.method === "GET" &&
            request.url.includes("/api/v1/fleet/snapshot") &&
            request.url.includes("mode=live"),
        ).length;
      }),
    )
    .toBe(1);
});

test("Ping targets stays idle and keeps rows interactive during manual refresh", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the Ping target request lifecycle is viewport independent",
  );
  await gotoConsoleHome(page);

  let listGets = 0;
  let detailGets = 0;
  let listGate: Promise<void> | null = null;
  let releaseList: (() => void) | null = null;
  let target = {
    assigned_count: 1,
    created_at: "2026-06-02T10:00:00Z",
    enabled: true,
    generation: 3,
    host: "1.1.1.1",
    id: "41000000-0000-4000-8000-000000000001",
    name: "Cloud resolver",
    port: null,
    primary_count: 1,
    probe_kind: "icmp",
    runtime_sync: { reason: "agent acknowledged", state: "applied" },
    selector_expression: "provider:edge",
    target_client_ids: ["agent-sfo-01"],
    target_update_available: false,
    target_update_evidence_available: true,
    updated_at: "2026-06-02T10:00:00Z",
  };
  await page.route("**/api/v1/ping-targets*", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === "/api/v1/ping-targets") {
      listGets += 1;
      if (listGate) await listGate;
      await route.fulfill({ json: [target] });
      return;
    }
    if (/^\/api\/v1\/ping-targets\/[^/]+$/.test(pathname)) {
      detailGets += 1;
      await route.fulfill({ json: { assignments: [], target } });
      return;
    }
    await route.fallback();
  });

  await openConsoleSubpage(page, "Observability", "Ping targets");
  const grid = page.getByLabel("Ping targets data grid");
  const row = grid.locator(".gridBody [role=row]", {
    hasText: "Cloud resolver",
  });
  await expect(row).toBeVisible();
  await expect.poll(() => listGets).toBeGreaterThanOrEqual(1);
  const initialListGets = listGets;
  expect(initialListGets).toBeLessThanOrEqual(2);
  expect(detailGets).toBe(0);

  await page.evaluate(() => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    for (let index = 0; index < 100; index += 1) {
      socket?.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({ type: "fleet_telemetry_invalidated" }),
        }),
      );
    }
  });
  await grid.getByLabel("Ping targets search").fill("Cloud");
  await page.waitForTimeout(100);
  expect({ detailGets, listGets }).toEqual({
    detailGets: 0,
    listGets: initialListGets,
  });

  listGate = new Promise<void>((resolve) => {
    releaseList = resolve;
  });
  target = {
    ...target,
    assigned_count: 2,
    updated_at: "2026-06-02T10:05:00Z",
  };
  const refresh = grid.getByRole("button", { name: "Refresh", exact: true });
  await refresh.click();
  await expect.poll(() => listGets).toBe(initialListGets + 1);
  await expect(row).toBeVisible();
  await expect(row.getByRole("gridcell").nth(5)).toHaveText("1");
  await grid.getByLabel("Ping targets search").fill("resolver");
  await expect(grid.getByLabel("Ping targets search")).toHaveValue("resolver");
  expect(detailGets).toBe(0);

  releaseList?.();
  listGate = null;
  await expect(refresh).toBeEnabled();
  await expect(row.getByRole("gridcell").nth(5)).toHaveText("2");
  expect({ detailGets, listGets }).toEqual({
    detailGets: 0,
    listGets: initialListGets + 1,
  });
});
test("job detail invalidations keep visible data and coalesce an in-flight wave", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the job detail refresh contract is viewport independent",
  );
  const selectedJobId = "55555555-aaaa-4bbb-8ccc-dddddddddddd";
  await page.goto(`/#/jobs/history/${selectedJobId}`);
  await waitForConsoleShell(page);
  const detail = page.getByRole("region", { name: "Job target details" });
  await expect(
    detail.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();
  await expect(
    detail.getByText("edge-sfo-01 (fo01)", { exact: true }).first(),
  ).toBeVisible();

  await page.evaluate((jobId) => {
    type DetailKind = "comparison" | "outputs" | "targets";
    type DetailStressState = {
      activeTargets: number;
      counts: Record<DetailKind, number>;
      hold: boolean;
      maxActiveTargets: number;
      release: () => void;
      waiters: Array<() => void>;
    };
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      __vpsmanJobDetailStress?: DetailStressState;
    };
    const originalFetch = window.fetch.bind(window);
    const state: DetailStressState = {
      activeTargets: 0,
      counts: { comparison: 0, outputs: 0, targets: 0 },
      hold: true,
      maxActiveTargets: 0,
      release: () => {
        state.hold = false;
        for (const resolve of state.waiters.splice(0)) {
          resolve();
        }
      },
      waiters: [],
    };
    trackedWindow.__vpsmanJobDetailStress = state;
    trackedWindow.__vpsmanFetchRequests = [];
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const prefix = `/api/v1/jobs/${jobId}`;
      const kind: DetailKind | null =
        pathname === `${prefix}/targets`
          ? "targets"
          : pathname === `${prefix}/outputs`
            ? "outputs"
            : pathname === `${prefix}/output-comparison`
              ? "comparison"
              : null;
      if (!kind) {
        return originalFetch(input, init);
      }
      state.counts[kind] += 1;
      if (kind === "targets") {
        state.activeTargets += 1;
        state.maxActiveTargets = Math.max(
          state.maxActiveTargets,
          state.activeTargets,
        );
      }
      try {
        if (state.hold) {
          await new Promise<void>((resolve) => state.waiters.push(resolve));
        }
        return await originalFetch(input, init);
      } finally {
        if (kind === "targets") {
          state.activeTargets -= 1;
        }
      }
    };
  }, selectedJobId);

  await page.evaluate((jobId) => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "job_details_invalidated",
          job_ids: [
            jobId,
            ...Array.from(
              { length: 200 },
              (_, index) => `unselected-job-${index}`,
            ),
          ],
        }),
      }),
    );
  }, selectedJobId);

  const detailRequestCounts = () =>
    page.evaluate(() => {
      const state = (
        window as typeof window & {
          __vpsmanJobDetailStress?: {
            counts: Record<string, number>;
            maxActiveTargets: number;
          };
        }
      ).__vpsmanJobDetailStress;
      return {
        comparison: state?.counts.comparison ?? 0,
        maxActiveTargets: state?.maxActiveTargets ?? 0,
        outputs: state?.counts.outputs ?? 0,
        targets: state?.counts.targets ?? 0,
      };
    });
  await expect.poll(detailRequestCounts).toEqual({
    comparison: 1,
    maxActiveTargets: 1,
    outputs: 1,
    targets: 1,
  });
  await expect(
    detail.getByText("edge-sfo-01 (fo01)", { exact: true }).first(),
  ).toBeVisible();
  await expect(detail.getByText("Loading target records")).toHaveCount(0);
  await expect(detail.getByText("Loading output records")).toHaveCount(0);

  await page.evaluate((jobId) => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    for (let sequence = 0; sequence < 100; sequence += 1) {
      socket?.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({
            type: "job_details_invalidated",
            job_ids: [jobId, `concurrent-job-${sequence}`],
          }),
        }),
      );
    }
  }, selectedJobId);
  expect(await detailRequestCounts()).toEqual({
    comparison: 1,
    maxActiveTargets: 1,
    outputs: 1,
    targets: 1,
  });

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanJobDetailStress?: { release: () => void };
      }
    ).__vpsmanJobDetailStress?.release();
  });
  await expect.poll(detailRequestCounts).toEqual({
    comparison: 2,
    maxActiveTargets: 1,
    outputs: 2,
    targets: 2,
  });
  await expect(
    detail.getByText("edge-sfo-01 (fo01)", { exact: true }).first(),
  ).toBeVisible();

  await page.evaluate(() => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "job_details_invalidated",
          job_ids: ["unselected-job"],
        }),
      }),
    );
  });
  await page.waitForTimeout(50);
  expect(await detailRequestCounts()).toEqual({
    comparison: 2,
    maxActiveTargets: 1,
    outputs: 2,
    targets: 2,
  });
  const jobListRefreshes = await page.evaluate(() => {
    const requests = (
      window as typeof window & {
        __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      }
    ).__vpsmanFetchRequests;
    return (requests ?? []).filter(
      (request) =>
        request.method === "GET" &&
        new URL(request.url, window.location.href).pathname === "/api/v1/jobs",
    ).length;
  });
  expect(jobListRefreshes).toBe(0);
});

test("job terminal events update loaded history rows without replacing the page", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the paged history transport contract is viewport independent",
  );
  await gotoConsoleHome(page);
  await page.evaluate(() => {
    type HistoryJob = {
      actor_id: string | null;
      command_type: string;
      completed_at: string | null;
      created_at: string;
      id: string;
      max_timeout_secs: number;
      payload_hash: string;
      privileged: boolean;
      source_schedule_id: string | null;
      status: string;
      target_count: number;
    };
    type HistoryStressState = {
      itemGetIds: string[];
      jobs: HistoryJob[];
      listGets: number;
    };
    const trackedWindow = window as typeof window & {
      __vpsmanJobHistoryStress?: HistoryStressState;
    };
    const originalFetch = window.fetch.bind(window);
    const jobs = Array.from(
      { length: 15 },
      (_, index): HistoryJob => ({
        actor_id: null,
        command_type: `history_probe_${index + 1}`,
        completed_at: null,
        created_at: new Date(
          Date.parse("2026-06-02T10:00:00Z") - index * 1_000,
        ).toISOString(),
        id: `10000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
        max_timeout_secs: 60,
        payload_hash: String(index).padStart(64, "0"),
        privileged: false,
        source_schedule_id: null,
        status: "running",
        target_count: 1,
      }),
    );
    const state: HistoryStressState = {
      itemGetIds: [],
      jobs,
      listGets: 0,
    };
    trackedWindow.__vpsmanJobHistoryStress = state;
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (
        init?.method ?? (input instanceof Request ? input.method : "GET")
      ).toUpperCase();
      if (method === "GET" && pathname === "/api/v1/jobs") {
        state.listGets += 1;
        return new Response(JSON.stringify(state.jobs), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }
      const jobMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)$/);
      if (method === "GET" && jobMatch) {
        const jobId = decodeURIComponent(jobMatch[1]);
        state.itemGetIds.push(jobId);
        const job = state.jobs.find((candidate) => candidate.id === jobId);
        return new Response(JSON.stringify(job ?? { error: "not_found" }), {
          headers: { "Content-Type": "application/json" },
          status: job ? 200 : 404,
        });
      }
      return originalFetch(input, init);
    };
  });

  await openConsoleSubpage(page, "Jobs", "History");
  const jobsGrid = page.getByLabel("Job records data grid");
  await expect(jobsGrid).toContainText("15 of 15 jobs");
  await jobsGrid.getByLabel("Job records page size").selectOption("10");
  await jobsGrid.getByLabel("Job records next page").click();
  await expect(jobsGrid.locator(".gridPageLabel")).toHaveText("2 / 2");

  const loadedJobId = "10000000-0000-4000-8000-000000000014";
  const loadedRow = jobsGrid
    .locator(".gridBody [role=row]")
    .filter({ hasText: "history probe 14" });
  await expect(loadedRow.locator(".status")).toHaveAttribute(
    "title",
    "running",
  );
  await page.evaluate((jobId) => {
    const state = (
      window as typeof window & {
        __vpsmanJobHistoryStress?: {
          itemGetIds: string[];
          jobs: Array<{ id: string; status: string }>;
          listGets: number;
        };
      }
    ).__vpsmanJobHistoryStress;
    if (!state) return;
    state.itemGetIds = [];
    state.listGets = 0;
    const job = state.jobs.find((candidate) => candidate.id === jobId);
    if (job) job.status = "completed";
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "job_finished",
          job_id: jobId,
          status: "completed",
        }),
      }),
    );
  }, loadedJobId);

  await expect(loadedRow.locator(".status")).toHaveAttribute(
    "title",
    "completed",
  );
  await expect(jobsGrid.locator(".gridPageLabel")).toHaveText("2 / 2");
  await expect
    .poll(() =>
      page.evaluate(() => {
        const state = (
          window as typeof window & {
            __vpsmanJobHistoryStress?: {
              itemGetIds: string[];
              listGets: number;
            };
          }
        ).__vpsmanJobHistoryStress;
        return {
          itemGetIds: state?.itemGetIds ?? [],
          listGets: state?.listGets ?? -1,
        };
      }),
    )
    .toEqual({ itemGetIds: [loadedJobId], listGets: 0 });

  const unknownJobId = "20000000-0000-4000-8000-000000000001";
  await page.evaluate((jobId) => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "job_rejected",
          job_id: jobId,
          status: "rejected",
        }),
      }),
    );
  }, unknownJobId);
  await page.waitForTimeout(100);
  expect(
    await page.evaluate((jobId) => {
      const state = (
        window as typeof window & {
          __vpsmanJobHistoryStress?: {
            itemGetIds: string[];
            listGets: number;
          };
        }
      ).__vpsmanJobHistoryStress;
      return {
        itemGetIds: state?.itemGetIds ?? [],
        listGets: state?.listGets ?? -1,
        unknownRequested: state?.itemGetIds.includes(jobId) ?? false,
      };
    }, unknownJobId),
  ).toEqual({
    itemGetIds: [loadedJobId],
    listGets: 0,
    unknownRequested: false,
  });
  await expect(jobsGrid.getByTitle(unknownJobId)).toHaveCount(0);
  await expect(jobsGrid.locator(".gridPageLabel")).toHaveText("2 / 2");

  const historySearch = jobsGrid.getByLabel("Job records search");
  await historySearch.fill("history probe 14");
  await expect(jobsGrid.locator(".gridPageLabel")).toHaveText("1 / 1");
  await expect(loadedRow).toBeVisible();
  await historySearch.fill("");
  await expect(jobsGrid.locator(".gridPageLabel")).toHaveText("1 / 2");

  const manualJobId = "30000000-0000-4000-8000-000000000001";
  await page.evaluate((jobId) => {
    const state = (
      window as typeof window & {
        __vpsmanJobHistoryStress?: {
          jobs: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanJobHistoryStress;
    state?.jobs.unshift({
      actor_id: null,
      command_type: "manual_refresh_head",
      completed_at: null,
      created_at: "2026-06-02T10:01:00Z",
      id: jobId,
      max_timeout_secs: 60,
      payload_hash: "f".repeat(64),
      privileged: false,
      source_schedule_id: null,
      status: "running",
      target_count: 1,
    });
  }, manualJobId);
  const historyPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", {
      level: 2,
      name: "Job history",
      exact: true,
    }),
  });
  await historyPanel.getByRole("button", { name: "Refresh" }).click();
  await expect(jobsGrid).toContainText("16 of 16 jobs");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobHistoryStress?: { listGets: number };
            }
          ).__vpsmanJobHistoryStress?.listGets ?? -1,
      ),
    )
    .toBe(1);
  await expect(jobsGrid.getByTitle(manualJobId)).toBeVisible();
});

test("fleet telemetry refresh keeps successful domains current when one domain fails", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the partial-refresh contract is viewport independent",
  );
  await installConsoleApiMock(page, {
    telemetryFailurePath: "tunnels",
    telemetryNetworkRateScales: [1, 2, 4, 8, 16, 32, 64],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");
  await expect(
    page.locator(".fleetMonitorWorkspace").getByRole("alert"),
  ).toContainText("Some live fleet sources are unavailable: tunnel telemetry");
  await openConsoleSubpage(page, "Fleet", "Instances");
  const grid = page.getByLabel("VPS instance records data grid");
  const row = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await activate(row.getByLabel("Expand VPS instance records row"));
  const detail = grid
    .locator(".gridExpandedRow", { hasText: "edge-sfo-01" })
    .first();
  await activate(detail.getByRole("tab", { name: "Telemetry" }));
  const networkValue = detail
    .locator(".timeline")
    .filter({ hasText: /^Network rate/ });
  await expect(networkValue).toContainText("RX");
  const initialValue = await networkValue.textContent();
  expect(initialValue).toContain("TX");
  await expect
    .poll(() => networkValue.textContent(), { timeout: 20_000 })
    .not.toBe(initialValue);
});

test("fleet groups expose registry assignments and reviewed bulk mutation evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk group mutation review is covered through the desktop operator workflow",
  );
  await gotoConsoleHome(page);

  await openConsoleSubpage(page, "Fleet", "Groups");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet groups" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Fleet groups" }),
  ).toBeVisible();
  const groupRegistryGrid = page.getByLabel("Group registry data grid");
  await activate(
    groupRegistryGrid.getByRole("button", {
      name: "Create group",
      exact: true,
    }),
  );
  const createGroupDrawer = page.getByLabel("Create group", { exact: true });
  await expect(createGroupDrawer.getByLabel("Group name")).toHaveAttribute(
    "placeholder",
    "role:edge or maintenance",
  );
  await createGroupDrawer.getByLabel("Group name").fill("role:a,role:b");
  await expect(
    createGroupDrawer.getByText("Use one group name; commas are not accepted."),
  ).toBeVisible();
  await expect(
    createGroupDrawer.getByRole("button", {
      name: "Create group",
      exact: true,
    }),
  ).toBeDisabled();
  await activate(
    createGroupDrawer.getByRole("button", { name: "Close Create group" }),
  );
  await expect(page.getByText("Group registry")).toBeVisible();
  await expect(page.getByLabel("Group registry search")).toBeVisible();
  await expect(page.getByLabel("Fleet group counts")).toContainText(
    "provider groups",
  );
  await expect(
    page
      .getByLabel("Fleet group counts")
      .locator("span")
      .filter({ hasText: "provider groups" }),
  ).toContainText("1");
  await expect(page.getByLabel("Fleet group counts")).toContainText(
    "country groups",
  );
  await expect(
    page
      .getByLabel("Fleet group counts")
      .locator("span")
      .filter({ hasText: "country groups" }),
  ).toContainText("2");
  await expect(page.getByLabel("Fleet group counts")).toContainText(
    "operator groups",
  );
  await expect(
    page
      .getByLabel("Fleet group counts")
      .locator("span")
      .filter({ hasText: "operator groups" }),
  ).toContainText("4");
  await expect(
    page
      .getByLabel("Fleet group counts")
      .locator("span")
      .filter({ hasText: "group assignments" }),
  ).toContainText("9");
  await expect(page.getByLabel("Fleet group counts")).toContainText(
    "reachable/review/offline",
  );
  await expect(page.getByLabel("Fleet group counts")).not.toContainText(
    "live/review/offline",
  );
  await expect(page.getByText("Manage display order")).toBeVisible();
  await expect(groupRegistryGrid).toContainText("Operator group");
  const summaryTop = await page.getByLabel("Fleet group counts").boundingBox();
  const registryTop = await groupRegistryGrid.boundingBox();
  expect(summaryTop?.y ?? 0).toBeLessThan(registryTop?.y ?? 0);

  const operatorGroupRow = groupRegistryGrid
    .getByRole("row")
    .filter({ hasText: "edge" })
    .first();
  await operatorGroupRow.click({ button: "right" });
  await activate(page.getByRole("menuitem", { name: "Delete", exact: true }));
  const deletePrompt = page.getByLabel("Confirm group delete");
  await expect(deletePrompt).toBeVisible();
  await expect(deletePrompt).toContainText("Group");
  await expect(deletePrompt).toContainText("Assignments");
  await activate(page.getByRole("button", { name: "Close confirmation" }));

  await openConsoleSubpage(page, "Fleet", "Assignments");
  await expect(
    page.getByRole("heading", { level: 1, name: "Group assignments" }),
  ).toBeVisible();
  await expect(page.getByText("VPS group assignments")).toBeVisible();
  const assignmentsGrid = page.getByLabel("VPS group assignments data grid");
  await expect(assignmentsGrid).toContainText("Reachability");
  const sfoAssignmentRow = assignmentsGrid
    .getByRole("row")
    .filter({ hasText: "edge-sfo-01" })
    .first();
  await expect(sfoAssignmentRow).toContainText("Contact unknown");
  await expect(
    sfoAssignmentRow.getByText("online", { exact: true }),
  ).toHaveCount(0);
  await sfoAssignmentRow.click();
  await expect(assignmentsGrid).toContainText("Groups");
  await expect(
    assignmentsGrid.getByRole("button", { name: /^Remove / }),
  ).toHaveCount(0);
  await sfoAssignmentRow.click({ button: "right" });
  await activate(
    page.getByRole("menuitem", { name: "Edit groups", exact: true }),
  );
  const assignmentDrawer = page.getByLabel(/^Edit groups · edge-sfo-01/);
  await expect(assignmentDrawer).toBeVisible();
  await expect(
    assignmentDrawer.getByRole("button", { name: "Remove provider:alpha" }),
  ).toBeVisible();
  await expect(
    assignmentDrawer.getByRole("button", { name: "Remove country:US" }),
  ).toBeVisible();
  await expect(
    assignmentDrawer.getByRole("button", {
      name: "Remove provider:alpha from edge-sfo-01",
    }),
  ).toBeVisible();
  await expect(
    assignmentDrawer.getByRole("button", {
      name: "Remove country:US from edge-sfo-01",
    }),
  ).toBeVisible();
  await expect(assignmentDrawer).toContainText("Used by 1 schedule");
  await expect(
    assignmentDrawer.getByLabel("Group to add to edge-sfo-01"),
  ).not.toHaveAttribute("title", /\S/);
  await expect(
    assignmentDrawer.getByText("Add group", { exact: true }),
  ).toHaveAttribute("title", /Suggestions: edge \(2 VPSs\)/);
  await expect(
    assignmentDrawer.getByRole("button", {
      name: "Remove role:edge from edge-sfo-01",
    }),
  ).toBeVisible();

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Fleet", "Assignments");
  await expect(
    page.getByRole("heading", { level: 1, name: "Group assignments" }),
  ).toBeVisible();
  const unlockedAssignmentRow = page
    .getByLabel("VPS group assignments data grid")
    .getByRole("row")
    .filter({ hasText: "edge-sfo-01" })
    .first();
  await unlockedAssignmentRow.click({ button: "right" });
  await activate(
    page.getByRole("menuitem", { name: "Edit groups", exact: true }),
  );
  const unlockedAssignmentDrawer = page.getByLabel(
    /^Edit groups · edge-sfo-01/,
  );
  const roleEdgeChip = unlockedAssignmentDrawer
    .locator(".tagRemoveChip")
    .filter({ hasText: "role:edge" });
  await expect(roleEdgeChip).toHaveCSS("border-radius", "999px");
  await expect(roleEdgeChip).toHaveCSS("border-top-style", "solid");
  await expect(roleEdgeChip.getByRole("button")).toHaveCount(1);
  await roleEdgeChip.getByText("role:edge", { exact: true }).click();
  const requestsBeforeRemove = await page.evaluate(() => {
    const requestLog = (
      window as unknown as {
        __vpsmanTestRequests: {
          bulkTagMutations: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requestLog.bulkTagMutations;
  });
  expect(requestsBeforeRemove).toHaveLength(0);
  await activate(
    unlockedAssignmentDrawer.getByRole("button", {
      name: "Remove role:edge from edge-sfo-01",
    }),
  );
  const undoNotice = unlockedAssignmentDrawer
    .getByRole("status")
    .filter({ hasText: /Removed\s+role:edge\s+from\s+edge-sfo-01/ });
  await expect(undoNotice).toBeVisible();
  await activate(undoNotice.getByRole("button", { name: "Undo" }));
  await expect(undoNotice).toBeHidden();
  await expect(
    unlockedAssignmentDrawer.locator(".localActionFeedback"),
  ).toContainText(/Group role:edge: \d+ changed, \d+ skipped/);
  await activate(
    unlockedAssignmentDrawer.getByRole("button", {
      name: /^Close Edit groups · edge-sfo-01/,
    }),
  );
  const fraAssignmentRow = page
    .getByLabel("VPS group assignments data grid")
    .getByRole("row")
    .filter({ hasText: "core-fra-02" })
    .first();
  await fraAssignmentRow.click({ button: "right" });
  await activate(
    page.getByRole("menuitem", { name: "Edit groups", exact: true }),
  );
  const fraAssignmentDrawer = page.getByLabel(/^Edit groups · core-fra-02/);
  await expect(fraAssignmentDrawer).toBeVisible();
  await expect(fraAssignmentDrawer.locator(".localActionFeedback")).toHaveCount(
    0,
  );

  const undoRequests = await page.evaluate(() => {
    const requestLog = (
      window as unknown as {
        __vpsmanTestRequests: {
          bulkTagMutations: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requestLog.bulkTagMutations;
  });
  expect(undoRequests.at(-4)).toMatchObject({
    action: "remove",
    confirmed: false,
    privilege_assertion: null,
    tag: "role:edge",
    target_client_ids: ["agent-sfo-01"],
  });
  expect(undoRequests.at(-3)).toMatchObject({
    action: "remove",
    confirmed: true,
    preview_hash: "7".repeat(64),
    tag: "role:edge",
    target_client_ids: ["agent-sfo-01"],
  });
  expect(undoRequests.at(-2)).toMatchObject({
    action: "add",
    confirmed: false,
    privilege_assertion: null,
    tag: "role:edge",
    target_client_ids: ["agent-sfo-01"],
  });
  expect(undoRequests.at(-1)).toMatchObject({
    action: "add",
    confirmed: true,
    preview_hash: "7".repeat(64),
    tag: "role:edge",
    target_client_ids: ["agent-sfo-01"],
  });

  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await expect(
    page.getByRole("heading", { level: 1, name: "Bulk groups" }),
  ).toBeVisible();
  await page.getByLabel("Bulk group", { exact: true }).fill("maintenance:test");
  await expect(page.getByLabel("Bulk group target preview")).toHaveCount(0);
  await page
    .getByRole("combobox", { name: "Bulk group selector expression" })
    .fill("id:agent-sfo-01");
  const selectorStatus = page
    .locator(".searchExpressionInput", {
      has: page.getByRole("combobox", {
        name: "Bulk group selector expression",
      }),
    })
    .locator(".searchExpressionMeta");
  await expect(selectorStatus).toHaveAttribute(
    "title",
    "Local match 1 VPS · 0 ready · 1 needs review · review targets excluded",
  );
  await expect(selectorStatus).toHaveText("1/3");
  await expect(selectorStatus).toHaveAttribute(
    "aria-label",
    "Local match 1 VPS · 0 ready · 1 needs review · review targets excluded",
  );
  await expect(
    page.getByRole("button", {
      name: "Include review targets to apply maintenance:test",
    }),
  ).toBeDisabled();
  await page.getByLabel("Include targets needing review").check();
  await expect(selectorStatus).toHaveText("1/3");
  await expect(selectorStatus).toHaveAttribute("title", /1 included/);
  await expect(
    page.getByRole("button", { name: "Add maintenance:test to 1 VPS" }),
  ).toBeEnabled();
  await activate(
    page.getByRole("button", { name: "Add maintenance:test to 1 VPS" }),
  );

  const evidence = page.getByLabel("Bulk group preview evidence");
  await expect(evidence).toBeVisible();
  await expect(evidence).toContainText("selected");
  await expect(evidence).toContainText("changed");
  await expect(evidence).toContainText("no-change");
  await expect(evidence).toContainText("schedule impacts");
  await expect(evidence).toContainText("preview hash");
  await expect(evidence).toContainText("7".repeat(64));
  await expect(page.locator(".bulkTagPreview")).toContainText("edge-sfo-01");

  const confirmation = page.getByLabel("Confirm tag mutation");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Preview hash");
  await expect(confirmation).toContainText("Membership after apply");
  await expect(confirmation).toContainText("1 VPS will gain the group");
  await expect(confirmation).toContainText("Excluded / no-change");
  await activate(
    confirmation.getByRole("button", { name: "Apply tag mutation" }),
  );

  const requests = await page.evaluate(() => {
    const requestLog = (
      window as unknown as {
        __vpsmanTestRequests: {
          bulkTagMutations: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requestLog.bulkTagMutations;
  });
  expect(requests.at(-2)).toMatchObject({
    confirmed: false,
    tag: "maintenance:test",
    target_client_ids: ["agent-sfo-01"],
  });
  expect(requests.at(-1)).toMatchObject({
    confirmed: true,
    preview_hash: "7".repeat(64),
    tag: "maintenance:test",
    target_client_ids: ["agent-sfo-01"],
  });
});

test("fleet instance row actions expose release VPS workflows", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "fleet grid action menu is covered through desktop data-grid behavior",
  );

  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Instances");
  const actionOrderGrid = page.getByLabel("VPS instance records data grid");
  await actionOrderGrid
    .getByLabel("Select VPS instance records row agent-sfo-01")
    .check();
  await actionOrderGrid.getByRole("button", { name: /^Actions$/ }).click();
  const actionLabels = await page
    .locator(".consoleMenu:visible")
    .getByRole("menuitem")
    .allTextContents();
  expect(actionLabels.slice(-3).map((label) => label.trim())).toEqual([
    "Stop agent",
    "Restart agent",
    "Review VPS deletion",
  ]);
  await page.keyboard.press("Escape");

  for (const action of [
    { label: "Open detail", heading: "Instance detail" },
    { label: "Open terminal", heading: "Terminal" },
    { label: "Open files", heading: "Files" },
    { label: "Open processes", heading: "Processes" },
    { label: "Open backups", heading: "Backup requests" },
    { label: "Open network", heading: "Network graph" },
  ]) {
    await gotoConsoleHome(page);
    await openConsoleSubpage(page, "Fleet", "Instances");

    const grid = page.getByLabel("VPS instance records data grid");
    const edgeRow = grid
      .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
      .first();
    await edgeRow.getByLabel("Select VPS instance records row").check();
    await grid.getByRole("button", { name: /^Actions$/ }).click();
    await page.getByRole("menuitem", { name: action.label }).click();
    await expect(
      page.getByRole("heading", {
        level: 1,
        name: action.heading,
        exact: true,
      }),
    ).toBeVisible();
    if (action.label === "Open detail") {
      await expect(page).toHaveURL(/#\/fleet\/instance-detail\/agent-sfo-01$/);
      await expect(
        page
          .getByLabel("Canonical VPS detail")
          .getByLabel("Selected VPS identity"),
      ).toContainText("edge-sfo-01");
    } else if (action.label === "Open terminal") {
      await expect(page.getByLabel("New terminal target")).toHaveValue(
        /edge-sfo-01/,
      );
    } else if (action.label === "Open files") {
      await expect(
        page.getByRole("combobox", { name: "File browser target VPS" }),
      ).toHaveValue(/edge-sfo-01/);
    } else if (action.label === "Open processes") {
      await expect(
        page
          .getByRole("group", { name: "Process scope" })
          .getByRole("button", { name: "Host" }),
      ).toHaveAttribute("aria-pressed", "true");
      await expect(
        page.getByRole("combobox", { name: "Host process VPS" }),
      ).toHaveValue(/edge-sfo-01/);
    } else if (action.label === "Open backups") {
      const workflow = page.getByRole("complementary", { name: "Run backup" });
      const target = workflow.getByRole("combobox", { name: "Backup client" });
      await expect(workflow).toBeVisible();
      await expect(target).toHaveValue(/edge-sfo-01/);
      await expect(target).toBeFocused();
    } else if (action.label === "Open network") {
      await expect(page.locator(".topologyNodeInspector")).toContainText(
        "edge-sfo-01",
      );
    }
  }
});

test("resource-specific VPS detail routes survive hard refresh", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the desktop fleet grid is the canonical resource-detail entry point",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  const edgeRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await edgeRow.getByLabel("Select VPS instance records row").check();
  await grid.getByRole("button", { name: /^Actions$/ }).click();
  await page.getByRole("menuitem", { name: "Open detail" }).click();
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\/agent-sfo-01$/);

  await page.reload();
  await waitForConsoleShell(page);
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\/agent-sfo-01$/);
  await expect(
    page.getByLabel("Canonical VPS detail").getByLabel("Selected VPS identity"),
  ).toContainText("edge-sfo-01");
});

test("resource-specific VPS detail routes keep navigation context", async ({
  page,
}) => {
  await page.goto("/#/fleet/instance-detail/agent-sfo-01");
  await waitForConsoleShell(page);

  const pageSelector = page.locator(".mobilePageSelector");
  await expect(pageSelector).toHaveValue("Fleet::instance_detail");
  await expect(pageSelector.locator("option:checked")).toHaveText(
    "Fleet / Instance detail",
  );

  const fleetSections = page.getByLabel("Fleet sections");
  if (await fleetSections.isVisible()) {
    await expect(
      fleetSections.getByRole("button", { name: "Instance detail" }),
    ).toHaveAttribute("aria-current", "page");
  }
});

test("VPS detail sections stay scoped to their resource history entry", async ({
  page,
}) => {
  await page.goto("/#/fleet/instance-detail/agent-sfo-01");
  await waitForConsoleShell(page);

  const detail = page.getByLabel("Canonical VPS detail");
  const detailSection = detail
    .locator(".detailTabSelect:visible")
    .getByLabel("VPS detail section");
  const resourcesTab = detail.locator(".detailTabs:visible").getByRole("tab", {
    name: "Resources",
    exact: true,
  });
  await expect(detailSection.or(resourcesTab)).toBeVisible();
  const usesSectionSelector = (await detailSection.count()) === 1;
  if (usesSectionSelector) {
    await detailSection.selectOption("Resources");
    await expect(detailSection).toHaveValue("Resources");
  } else {
    await resourcesTab.click();
    await expect(resourcesTab).toHaveAttribute("aria-selected", "true");
  }

  await page.evaluate(() => {
    window.location.hash = "#/fleet/instance-detail/agent-fra-02";
  });
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\/agent-fra-02$/);
  await expect(detail.getByLabel("Selected VPS identity")).toContainText(
    "core-fra-02",
  );
  if (usesSectionSelector) {
    await expect(detailSection).toHaveValue("Summary");
  } else {
    await expect(
      detail.getByRole("tab", { name: "Summary", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
  }

  await page.goBack();
  await expect(page).toHaveURL(/#\/fleet\/instance-detail\/agent-sfo-01$/);
  await expect(detail.getByLabel("Selected VPS identity")).toContainText(
    "edge-sfo-01",
  );
  if (usesSectionSelector) {
    await expect(detailSection).toHaveValue("Resources");
  } else {
    await expect(
      detail.getByRole("tab", { name: "Resources", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
  }
});

test("VPS monitoring reports per-domain retained resolutions", async ({
  page,
}) => {
  await installConsoleApiMock(page, {
    monitoringRangeOverride: {
      effective_points: 4,
      effective_resolution_secs: 300,
      resolutions: {
        network: 300,
        ping: 300,
        resources: 300,
        traffic: 60,
      },
      source: "retained",
      step_secs: 300,
    },
  });
  await page.goto("/#/fleet/instance-detail/agent-sfo-01");
  await waitForConsoleShell(page);

  const detail = page.getByLabel("Canonical VPS detail");
  const sectionSelector = detail
    .locator(".detailTabSelect:visible")
    .getByLabel("VPS detail section");
  const resourcesTab = detail
    .locator(".detailTabs:visible")
    .getByRole("tab", { name: "Resources", exact: true });
  await expect(sectionSelector.or(resourcesTab)).toBeVisible();
  if ((await sectionSelector.count()) === 1) {
    await sectionSelector.selectOption("Resources");
  } else {
    await resourcesTab.click();
  }

  const monitoring = detail.getByRole("region", {
    name: "Monitoring history for edge-sfo-01",
  });
  await expect(monitoring).toContainText("retained tiered history");
  await expect(monitoring).toContainText(
    "resources/network/Ping 5m; traffic 1m coarsest source resolutions",
  );
});

test("VPS detail workflow actions preserve the exact resource target", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense detail actions are covered through the desktop resource workflow",
  );
  const openDetail = async () => {
    await page.goto("/#/fleet/instance-detail/agent-sfo-01");
    await waitForConsoleShell(page);
    await expect(
      page
        .getByLabel("Canonical VPS detail")
        .getByLabel("Selected VPS identity"),
    ).toContainText("edge-sfo-01");
    return page.getByLabel("Canonical VPS detail");
  };

  let detail = await openDetail();
  await detail
    .getByRole("button", { name: "Run command", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "Bulk target selector expression" }),
  ).toHaveValue("id:agent-sfo-01");

  detail = await openDetail();
  await detail.getByRole("button", { name: "Files", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "File browser target VPS" }),
  ).toHaveValue(/edge-sfo-01/);

  detail = await openDetail();
  await detail.getByRole("button", { name: "Processes", exact: true }).click();
  await expect(
    page
      .getByRole("group", { name: "Process scope" })
      .getByRole("button", { name: "Host" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    page.getByRole("combobox", { name: "Host process VPS" }),
  ).toHaveValue(/edge-sfo-01/);

  detail = await openDetail();
  await detail.getByRole("button", { name: "Back up", exact: true }).click();
  const backupWorkflow = page.getByRole("complementary", {
    name: "Run backup",
  });
  const backupTarget = backupWorkflow.getByRole("combobox", {
    name: "Backup client",
  });
  await expect(backupTarget).toHaveValue(/edge-sfo-01/);
  await expect(backupTarget).toBeFocused();

  detail = await openDetail();
  await detail.getByRole("button", { name: "Config", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "VPS config target" }),
  ).toHaveValue(/edge-sfo-01/);
});

test("network graph handoffs never substitute a different VPS", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the desktop fleet grid exposes the direct network handoff",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  const backupRow = grid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await backupRow.getByLabel("Select VPS instance records row").check();
  await grid.getByRole("button", { name: /^Actions$/ }).click();
  await page.getByRole("menuitem", { name: "Open network" }).click();

  await expect(page.getByText("VPS not in the managed graph")).toBeVisible();
  await expect(page.getByText("No different VPS was selected.")).toBeVisible();
  await expect(page.locator(".topologyNodeInspector")).toHaveCount(0);
});

test("fleet instance detail is the canonical VPS route from release workflows", async ({
  page,
}, testInfo) => {
  testInfo.setTimeout(60_000);
  test.skip(
    testInfo.project.name.includes("mobile"),
    "cross-page dense grid and graph entry points are covered in desktop workflow tests",
  );

  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");
  await page
    .getByLabel("VPS monitor cards")
    .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" })
    .first()
    .click();
  await expectCanonicalVpsDetail(page, "edge-sfo-01");
  await expect(page.getByLabel("Canonical VPS detail")).toContainText(
    "Scheduled shell command",
  );

  await openConsoleSubpage(page, "Fleet", "Instances");
  const grid = page.getByLabel("VPS instance records data grid");
  const edgeRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await edgeRow.getByLabel("Select VPS instance records row").check();
  await grid.getByRole("button", { name: /^Actions$/ }).click();
  await page.getByRole("menuitem", { name: "Open detail" }).click();
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Fleet", "Alerts");
  const alertRow = page
    .getByLabel("Fleet alerts data grid")
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();
  await alertRow.click({ button: "right" });
  await activate(page.getByRole("menuitem", { name: "Open VPS", exact: true }));
  await expectCanonicalVpsDetail(page, "core-fra-02");

  await openConsoleSubpage(page, "Network", "Graph");
  await expect(
    page.getByRole("heading", { level: 1, name: "Network graph" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: /Select edge-sfo-01/ })
    .first()
    .click();
  await activate(
    page
      .locator(".topologyNodeInspector")
      .getByRole("button", { name: "Open VPS" }),
  );
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Jobs", "History");
  const jobsGrid = page.getByLabel("Job records data grid");
  const firstJobRow = jobsGrid.locator(".gridBody [role=row]").first();
  await invokeGridRowAction(page, jobsGrid, firstJobRow, "Open target detail");
  const targetGrid = page.getByLabel("Target result records data grid");
  const targetRow = targetGrid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await invokeGridRowAction(page, targetGrid, targetRow, "Open VPS");
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Backups", "Requests");
  await activate(page.getByRole("button", { name: "Run backup", exact: true }));
  const backupWorkflow = page.getByRole("complementary", {
    name: "Run backup",
  });
  await chooseVpsBySearch(
    backupWorkflow,
    "Backup client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(
    backupWorkflow
      .locator(".backupContextActions")
      .getByRole("button", { name: "Open VPS detail" }),
  );
  await expectCanonicalVpsDetail(page, "edge-sfo-01");
});

test("fleet instance config detail separates source readiness, drift, apply state, and actions", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "mobile config-detail shape is covered by structured screenshots",
  );

  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Instances");
  const grid = page.getByLabel("VPS instance records data grid");
  const edgeRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await edgeRow.getByLabel("Select VPS instance records row").check();
  await grid.getByRole("button", { name: /^Actions$/ }).click();
  await page.getByRole("menuitem", { name: "Open detail" }).click();

  const detail = page.getByLabel("Canonical VPS detail");
  await activate(detail.getByRole("tab", { name: "Config" }));
  const configTab = detail.getByRole("tabpanel", { name: "Config" });
  const posture = configTab.getByLabel("VPS config posture");
  await expect(posture).toContainText("Effective sources");
  await expect(posture).toContainText("Source state");
  await expect(posture).toContainText("Drift state");
  await expect(posture).toContainText("Last apply");
  await expect(posture).toContainText("Last error");
  await expect(posture).toContainText(
    "All loaded effective sources are synced and verified ready",
  );
  await expect(posture).toContainText("Ready");
  await expect(posture).not.toContainText("Fleet status");

  const actions = configTab.getByLabel("VPS config actions");
  await expect(
    actions.getByRole("button", { name: "Open per-VPS config" }),
  ).toBeVisible();
  await expect(actions.getByRole("button", { name: "Compare" })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Apply" })).toBeVisible();
  await expect(configTab.getByText("Raw source state details")).toBeVisible();

  await configTab.getByText("Raw source state details").click();
  await expect(
    configTab.getByText("The effective configuration is applied.").first(),
  ).toBeVisible();

  await actions.getByRole("button", { name: "Compare" }).click();
  await expect(
    page.getByRole("heading", { name: "Per-VPS desired config" }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "VPS config target" }),
  ).toHaveValue(/edge-sfo-01/);
});

test("fleet instances table keeps dense grid controls and routes card view separately", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop grid controls are covered separately from mobile navigation",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  await expect(
    page.locator(".topbarActions > .savedViewControls").first(),
  ).toBeVisible();
  await expect(grid.getByLabel("VPS instance records search")).toBeVisible();
  await expect(grid.getByLabel("Fleet instance view mode")).toContainText(
    "Table",
  );
  await expect(grid.getByRole("button", { name: "Cards" })).toBeVisible();

  await grid.getByLabel("VPS instance records search").fill("edge-sfo-01");
  await expect(
    grid.locator(".gridBody [role=row]", { hasText: "edge-sfo-01" }),
  ).toBeVisible();
  await expect(
    grid.locator(".gridBody [role=row]", { hasText: "core-fra-02" }),
  ).toHaveCount(0);
  await grid.getByLabel("VPS instance records search").fill("");

  await grid.getByRole("button", { name: "VPS", exact: true }).click();
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toHaveCount(0);
  await grid
    .getByRole("button", { name: "VPS instance records columns" })
    .click();
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  await page.keyboard.press("Escape");
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toBeVisible();

  const edgeRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await edgeRow.getByLabel("Select VPS instance records row").check();
  await expect(grid).toContainText("1 selected");
  await expect(grid.locator(".gridExpandedRow")).toHaveCount(0);
  await expect(
    grid.getByRole("heading", { name: /Terminal sessions|Files|Processes/ }),
  ).toHaveCount(0);

  await grid.getByRole("button", { name: "Cards" }).click();
  await expect(
    page.getByRole("heading", { name: "Fleet monitor" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Fleet", "Instances");
  const restoredGrid = page.getByLabel("VPS instance records data grid");
  await expect(
    restoredGrid.getByRole("columnheader", { name: /Provider/ }),
  ).toBeVisible();
});

test("command palette indexes release pages and fixture entities", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await waitForConsoleShell(page);

  const commandPaletteButton = page.getByRole("button", {
    name: "Open command palette",
  });
  await commandPaletteButton.click();
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  const search = page.getByLabel("Command palette search");
  await page.keyboard.press("Control+K");
  await expect(search).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(palette).toBeHidden();
  await expect(commandPaletteButton).toBeFocused();
  await commandPaletteButton.click();
  await expect(palette).toBeVisible();
  await expect(search).toHaveAttribute("role", "combobox");
  await expect(search).toHaveAttribute(
    "aria-controls",
    "command-palette-results",
  );
  await expect(page.locator(".sidebar")).toHaveAttribute("aria-hidden", "true");
  const initialOptions = palette.getByRole("option");
  await expect(initialOptions.first()).toHaveAttribute("aria-selected", "true");
  await search.press("End");
  await expect(initialOptions.last()).toHaveAttribute("aria-selected", "true");
  await search.press("Home");
  await expect(initialOptions.first()).toHaveAttribute("aria-selected", "true");
  await search.press("Tab");
  await expect(search).toBeFocused();
  await search.fill("Remote Terminal");
  await expect(
    palette
      .locator('[data-command-group="Page"]')
      .filter({ hasText: "Remote / Terminal" }),
  ).toBeVisible();
  await palette
    .getByRole("option", { name: /Page: Remote \/ Terminal/ })
    .click();
  await expect(page.getByRole("heading", { name: "Terminal" })).toBeVisible();

  await expectCommandPaletteGroup(page, "VPS", "edge-sfo");
  await expectCommandPaletteGroup(page, "Job", "network_speed_test");
  await expectCommandPaletteGroup(page, "Terminal", "61616161");
  await expectCommandPaletteGroup(page, "Transfer", "routing.log");
  await expectCommandPaletteGroup(page, "Backup", "fixture backup");
  await expectCommandPaletteGroup(page, "Audit", "privilege.unlock");
  await expectCommandPaletteGroup(page, "Schedule", "edge-health-hourly");
});

test("command palette entity selections use release route helpers", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  await selectCommandPaletteResult(page, "VPS", "edge-sfo");
  await expect(
    page.getByRole("heading", { name: "Instance detail" }),
  ).toBeVisible();

  await selectCommandPaletteResult(page, "Job", "network_speed_test");
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();

  await selectCommandPaletteResult(page, "Terminal", "61616161");
  await expect(
    page.getByRole("heading", { name: "Terminal", exact: true }),
  ).toBeVisible();
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await expect(
    page.getByRole("heading", { name: "Terminal sessions" }),
  ).toBeVisible();

  await selectCommandPaletteResult(page, "Audit", "privilege.unlock");
  await expect(
    page.getByRole("heading", { name: "Audit events" }),
  ).toBeVisible();
});

test("jobs approvals and scheduled runs stay separate", async ({
  page,
}, testInfo) => {
  testInfo.setTimeout(60_000);
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Jobs", "Approvals");

  await expect(
    page.getByRole("heading", { level: 1, name: "Approvals" }),
  ).toBeVisible();
  await expect(page.getByText("1 pending · 1 total request")).toBeVisible();
  await expect(page.getByText("Job approval queue")).toBeVisible();
  await expect(page.getByText("noc-operator")).toBeVisible();
  await expect(page.getByText("destructive")).toBeVisible();
  const approvalGrid = page.getByLabel("Job approval queue data grid");
  if (testInfo.project.name.includes("mobile")) {
    await invokeGridRowAction(
      page,
      approvalGrid,
      approvalGrid.locator(".gridMobileCard", { hasText: "noc-operator" }),
      "Review",
    );
  } else {
    await approvalGrid
      .locator(".gridBody [role=row]", { hasText: "noc-operator" })
      .first()
      .click({ button: "right" });
    await activate(page.getByRole("menuitem", { name: "Review", exact: true }));
  }
  const reviewPrompt = page.getByRole("region", {
    name: "Review job approval",
  });
  await expect(reviewPrompt).toBeVisible();
  await expect(reviewPrompt).toContainText("Argv command");
  await expect(reviewPrompt).toContainText("noc-operator (operator)");
  await expect(reviewPrompt).toContainText("Request reason");
  await expect(reviewPrompt).toContainText("destructive");
  await expect(reviewPrompt).not.toContainText("destructive · destructive");
  await activate(reviewPrompt.getByRole("button", { name: "Reject" }));
  await expect(
    reviewPrompt.getByRole("button", { name: "Reject request" }),
  ).toBeDisabled();
  await reviewPrompt
    .getByLabel("Rejection reason")
    .fill("maintenance window closed");
  await activate(reviewPrompt.getByRole("button", { name: "Reject request" }));
  await expect(reviewPrompt).toHaveCount(0);
  const approvalDecisions = await page.evaluate(() => {
    const requestLog = (
      window as unknown as {
        __vpsmanTestRequests: {
          jobApprovalDecisions: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requestLog.jobApprovalDecisions;
  });
  expect(approvalDecisions.at(-1)).toMatchObject({
    decision: "reject",
    body: {
      confirmed: true,
      reason: "maintenance window closed",
    },
  });
  await expect(page.getByText("schedule-created run")).toHaveCount(0);
  await expect(page.getByText("worker-created due runs")).toHaveCount(0);

  await openConsoleSubpage(page, "Jobs", "Scheduled runs");
  await expect(
    page.getByRole("heading", { level: 1, name: "Scheduled runs" }),
  ).toBeVisible();
  await expect(page.getByText("1 schedule-created run")).toBeVisible();
  const scheduledRunsGrid = page.getByLabel("Schedule run records data grid");
  await expect(scheduledRunsGrid).toContainText("edge-health-hourly");
  await expect(scheduledRunsGrid).toContainText("Hourly at minute 0");
  await expect(scheduledRunsGrid).toContainText("Scheduled shell command");
  await expect(
    scheduledRunsGrid.getByRole("columnheader", { name: "Due" }),
  ).toHaveCount(0);
  await expect(scheduledRunsGrid).not.toContainText("Not reported");
  await expect(scheduledRunsGrid).toContainText("completed");
  await expect(
    scheduledRunsGrid.getByRole("button", { name: "Run again" }),
  ).toHaveCount(0);
  await expect(page.getByText("Schedule link not exposed")).toHaveCount(0);
  await expect(page.getByText("due not exposed")).toHaveCount(0);
  await expect(page.getByText("Retry/worker health not exposed")).toHaveCount(
    0,
  );
  await expect(page.getByRole("button", { name: "Retry" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Review create" })).toHaveCount(
    0,
  );

  await activate(page.getByRole("button", { name: "Open schedule registry" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Schedules" }),
  ).toBeVisible();
  const schedulesGrid = page.getByLabel("Schedule records data grid");
  await expect(page.getByLabel("Schedule execution policy")).toContainText(
    "Enabled schedules with a valid cadence automatically dispatch future jobs",
  );
  await expect(schedulesGrid).toContainText("edge-health-hourly");
  await expect(schedulesGrid).toContainText("Hourly at minute 0");
  await expect(schedulesGrid).toContainText("Argv command");
  await expect(schedulesGrid).toContainText("Automatic runs authorized");
  await expect(schedulesGrid).toContainText("Overdue");
  await expect(schedulesGrid).toContainText(/Overdue by \d+[wdh]/);
  await expect(schedulesGrid).toContainText(/schedule calculation stale/i);
  await expect(schedulesGrid.getByText(/next 1 runs|next 5 runs/)).toHaveCount(
    0,
  );
  if (testInfo.project.name.includes("mobile")) {
    await expect(
      page.getByRole("button", { name: "Unlock privilege" }),
    ).toBeVisible();
  }
  await activate(
    page
      .locator(".sectionHeader .inlineActions")
      .getByRole("button", { name: "Scheduled runs" }),
  );
  await expect(
    page.getByRole("heading", { level: 1, name: "Scheduled runs" }),
  ).toBeVisible();
});

test("generic data grids keep mobile actions in the shared header", async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes("mobile"),
    "mobile card rendering is a mobile-only data-grid contract",
  );

  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Jobs", "Approvals");

  const grid = page.getByLabel("Job approval queue data grid");
  const cards = grid.locator(".gridMobileCard");
  await expect(cards.first()).toBeVisible();
  await expect(grid.locator(".gridHeaderGroup")).toBeHidden();

  const pendingCard = cards.filter({ hasText: "Argv command" }).first();
  await expect(pendingCard).toContainText("pending");
  await expect(pendingCard).toContainText("Scope");
  await expect(pendingCard.locator(".gridMobileActions")).toHaveCount(0);
  await pendingCard.click();
  await expect(grid.locator(".gridExpandedRow")).toBeVisible();
  await pendingCard.getByRole("checkbox").check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");

  const horizontalOverflowPx = await page.evaluate(() =>
    Math.max(
      0,
      document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  );
  expect(horizontalOverflowPx).toBeLessThanOrEqual(1);
});

test("advanced release labels provide inline expert help", async ({ page }) => {
  await gotoConsoleHome(page);

  await openConsoleSubpage(page, "Config", "VPS override patch");
  await expect(
    page.getByLabel("Advanced · VPS override patch help"),
  ).toHaveAttribute("title", /Use -field\.path or -\[section\.path\]/);
  await expect(page.getByLabel("Targets help")).toHaveAttribute(
    "title",
    /Selector expressions freeze/,
  );
  await expect(
    page.getByLabel("Max timeout seconds help").first(),
  ).toHaveAttribute("title", /Per-target command timeout/);
  await expect(
    page.getByText(/Saved generators render incremental TOML/),
  ).toBeVisible();

  await openConsoleSubpage(page, "Config", "Per-VPS");
  const target = page.getByRole("combobox", { name: "VPS config target" });
  await target.fill("edge-sfo");
  const targetOption = page
    .locator(".vpsComboboxMenu")
    .getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ });
  await expect(targetOption).toBeVisible();
  await targetOption.click();
  await expect(page.getByText("Desired runtime hierarchy")).toBeVisible();
  await expect(page.getByLabel("Saved desired runtime TOML")).toContainText(
    "this is not the VPS override editor",
  );
  await page.getByRole("tab", { name: "Advanced" }).click();
  await expect(
    page.getByText("Complete VPS override replacement TOML"),
  ).toBeVisible();
  await expect(page.getByLabel("VPS replacement override TOML")).toBeVisible();

  await openConsoleSubpage(page, "Config", "Rules");
  await expect(page.getByLabel("Bulk rule editor help")).toHaveAttribute(
    "title",
    /accounting,? and alert policies/,
  );
  await expect(
    page.locator('h4[title*="Fleet selector used for the dry-run"]'),
  ).toBeVisible();
  await expect(
    page.locator('h4[title*="Key=value lines become typed"]'),
  ).toBeVisible();
  await expect(
    page.locator('h4[title*="Explicit rule keys removed"]'),
  ).toBeVisible();
});

test("visible disabled release controls explain their disabled reason", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  for (const route of releaseAccessibilityRoutes) {
    await openConsoleSubpage(page, route.view, route.subpage);
    const missingReasons = await visibleDisabledControlsWithoutReason(page);
    expect(missingReasons, `${route.view} / ${route.subpage}`).toEqual([]);
  }
});

test("release console text colors preserve WCAG AA contrast", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  for (const route of releaseAccessibilityRoutes) {
    await openConsoleSubpage(page, route.view, route.subpage);
    const failures = await contrastFailures(page);
    expect(failures, `${route.view} / ${route.subpage}`).toEqual([]);
  }
});

test("automation runbooks promotes command templates into reviewed catalog", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Automation", "Runbooks");

  await expect(
    page.getByRole("heading", { level: 1, name: "Runbooks" }),
  ).toBeVisible();
  await expect(page.getByLabel("Runbook catalog summary")).toContainText(
    "Runbooks",
  );
  await expect(page.getByLabel("Runbook catalog summary")).toContainText(
    "Ready",
  );

  const catalog = page.getByLabel("Runbook catalog", { exact: true });
  await expect(catalog).toContainText("Default shell command");
  await expect(catalog).toContainText("edge-health-check");
  await expect(catalog).toContainText("Argv command");
  await expect(catalog).toContainText("Latest same operation");
  await expect(catalog).toContainText("not attributed to this runbook");
  await expect(catalog).not.toContainText("No loaded run");
  await expect(catalog).not.toContainText("shell_argv");
  await expect(catalog).not.toContainText("No matching run");
  await expect(page.getByLabel("Runbook filters")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Dispatch command" }),
  ).toHaveCount(0);

  const edgeRunbook = catalog.locator(".runbookCard", {
    hasText: "edge-health-check",
  });
  await expect(
    edgeRunbook.getByText("uptime", { exact: true }),
  ).toHaveAttribute(
    "title",
    /Operation evidence:[\s\S]*"argv": \[[\s\S]*"uptime"/,
  );
  await activate(edgeRunbook.getByText("Review inputs"));
  await expect(
    edgeRunbook.getByLabel("Required review for edge-health-check"),
  ).toContainText("Target scope");
  await expect(
    edgeRunbook.getByLabel("Required review for edge-health-check"),
  ).toContainText("Command arguments");
  await edgeRunbook
    .getByRole("button", { name: "Manage edge-health-check" })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Edit in Dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Duplicate in Dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Delete in Dispatch" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await activate(edgeRunbook.getByRole("button", { name: "Run" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();

  const composer = page.locator(".commandComposer", {
    has: page.getByRole("heading", { name: "Dispatch command" }),
  });
  await expect(composer.getByLabel("Template selector")).toHaveValue(
    "46464646-5656-4789-8abc-defdefdefdef",
  );
  await expect(
    composer.getByLabel("Bulk target selector expression"),
  ).toHaveValue("tag:provider:alpha");
});

test("jobs artifacts is read-only inventory linked to source workflows", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Jobs", "Artifacts");

  await expect(
    page.getByRole("heading", { level: 1, name: "Job artifacts" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Job artifacts" }),
  ).toBeVisible();

  await page.reload();
  await waitForConsoleShell(page);

  const summary = page.getByLabel("Job artifact inventory summary");
  await expect(summary).toContainText("Artifact types");
  await expect(summary).toContainText("Records");
  await expect(summary).toContainText("Stored bytes");
  await expect(summary).toContainText("Cleanup boundary");
  await expect(summary).toContainText("System / Maintenance");

  const grid = page.getByLabel("Job artifact inventory data grid");
  await expect(grid).toContainText("Backup artifact");
  await expect(grid).toContainText("Transfer package");
  await expect(grid).toContainText("Agent update bundle");
  await expect(grid).toContainText("Ready");
  await expect(grid).toContainText("Recorded with SHA-256 evidence");
  await expect(grid.getByLabel("Artifact type filter")).toBeVisible();
  await grid.getByLabel("Artifact type filter").selectOption("Backup artifact");
  await expect(grid).toContainText("1 of 1 artifact");
  await expect(grid).toContainText("Backup artifact");
  await grid.getByLabel("Artifact type filter").selectOption("all");
  await expect(grid).toContainText("Backups / Artifacts");
  await expect(grid).toContainText("Remote / Transfers");
  await expect(grid).toContainText("Automation / Agent updates");
  await expect(grid).not.toContainText("file_transfer_source");
  await expect(grid).not.toContainText("agent_update");

  const firstArtifactRecord = testInfo.project.name.includes("mobile")
    ? grid.locator(".gridMobileCard").first()
    : grid.locator(".gridBody [role=row]").first();
  if (testInfo.project.name.includes("mobile")) {
    await expect(firstArtifactRecord.locator(".gridMobileActions")).toHaveCount(
      0,
    );
    await firstArtifactRecord.click();
  } else {
    await grid
      .getByRole("button", { name: /Expand Job artifact inventory row/ })
      .first()
      .click();
  }
  await expect(grid).toContainText("Object key / URL");
  await expect(grid).toContainText("SHA-256");
  await expect(grid).toContainText("Raw status:");
  await expect(grid.locator(".gridExpandedContent button")).toHaveCount(0);

  await firstArtifactRecord.getByRole("checkbox").check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Copy object keys" }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Copy SHA-256" }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Copy download paths" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");

  await expect(page.getByRole("button", { name: "Queue cleanup" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("button", { name: "Preview cleanup" }),
  ).toHaveCount(0);

  await page
    .getByLabel("Artifact source workflow links")
    .getByRole("button", { name: "Backups / Artifacts" })
    .first()
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup artifacts" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Jobs", "Artifacts");
  const sourceLinks = page.getByLabel("Artifact source workflow links");
  await sourceLinks.getByRole("button", { name: "Remote / Transfers" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Transfers" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Jobs", "Artifacts");
  await page
    .getByLabel("Artifact source workflow links")
    .getByRole("button", { name: "Automation / Agent updates" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Agent updates" }),
  ).toBeVisible();
});

test("automation owns agent update rollout, health, and rollback posture", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Automation", "Agent updates");

  await expect(
    page.getByRole("heading", { level: 1, name: "Agent updates" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Agent update registry" }),
  ).toBeVisible();

  const posture = page.getByLabel("Agent update rollout posture");
  await expect(posture).toContainText("Available version");
  await expect(posture).toContainText("Current fleet versions");
  await expect(posture).toContainText("Registered artifact");
  await expect(posture).toContainText("Targets");
  await expect(posture).toContainText("Registry policy");
  await expect(posture).toContainText("Advisory metadata");
  await expect(posture).toContainText(
    "This registry is optional audit metadata",
  );
  await expect(posture).toContainText("Health checks");
  await expect(posture).toContainText("Rollback");
  await expect(posture).toContainText("Version telemetry unavailable");
  const shortcuts = page.getByLabel("Agent update dispatch shortcuts");
  await expect(
    shortcuts.getByRole("button", { name: "Start update" }),
  ).toBeEnabled();
  await expect(
    shortcuts.getByRole("button", { name: "Rollback" }),
  ).toBeDisabled();
  await expect(
    page.getByText("Latest release has no rollback artifact."),
  ).toBeVisible();
  await expect(posture).toContainText("agent update");

  await posture.getByRole("button", { name: "Update jobs" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();

  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    await expect(mobilePageSelector).not.toContainText(
      "Jobs / Update registry",
    );
  } else {
    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await activate(
      nav.getByRole("button", { name: "Jobs", exact: true }).first(),
    );
    await expect(
      nav
        .getByLabel("Jobs sections")
        .getByRole("button", { name: "Update registry", exact: true }),
    ).toHaveCount(0);
  }
});

test("config overview focuses on drift risk and routes to config workflows", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Config", "Overview");

  await expect(
    page.getByRole("heading", { name: "Runtime config overview" }),
  ).toBeVisible();
  const health = page.getByLabel("Config health posture");
  await expect(health).toContainText("Config health");
  await expect(health).toContainText("Action required");
  await expect(health).toContainText("current resources");
  await expect(health).toContainText("need attention");
  await expect(health).toContainText("historical records");
  await expect(health).toContainText("verified source checks");
  await expect(health).toContainText("4/4 rules valid");

  const currentState = page.getByLabel("Current config state by VPS");
  await expect(currentState).toContainText("Affected VPS current state");
  await expect(currentState).toContainText("Stale apply");
  await expect(currentState).not.toContainText("Deleted or unavailable VPS");
  await expect(currentState).toContainText("Unknown");
  await expect(currentState).toContainText("No apply evidence");
  await expect(currentState).not.toContainText("1970");
  await expect(currentState).not.toContainText("Queued apply");
  const historicalState = page.getByLabel("Historical config apply state");
  await expect(historicalState).toContainText("Historical apply-state records");
  await expect(historicalState).toContainText("Deleted or unavailable VPS");
  await expect(historicalState).toContainText(
    "do not affect current retry or health counts",
  );

  const drift = page.getByLabel("Config drift summary");
  await expect(drift).toContainText("Runtime apply state");
  await expect(drift).toContainText(
    "current fleet only, historical records separated",
  );
  await expect(drift).toContainText("Source readiness drift");
  await expect(drift).toContainText("Rule validation");
  await expect(drift).toContainText("4/4 rules valid");
  await expect(drift).not.toContainText("rows are not ok");

  const sourceSummary = page.getByLabel("Configuration source summary");
  await expect(sourceSummary).toContainText("Configuration sources");
  await expect(sourceSummary).toContainText("effective presets");
  await expect(sourceSummary).toContainText("explicit overrides");
  await expect(sourceSummary).toContainText("VPS without source evidence");

  await expect(page.getByLabel("Recent config changes")).toContainText(
    "Apply state",
  );
  const recentConfigCells = page
    .getByLabel("Recent config changes")
    .locator(".configRecentGrid:not(.heading) > span");
  const recentConfigTooltipState = await recentConfigCells.evaluateAll(
    (cells) =>
      cells.map((cell, index) => ({
        cellIndex: index % 5,
        generated: cell.getAttribute("data-value-tooltip") === "true",
        text: cell.textContent?.replace(/\s+/g, " ").trim() ?? "",
        title: cell.getAttribute("title"),
      })),
  );
  expect(
    recentConfigTooltipState.filter(
      ({ cellIndex, generated, title }) =>
        cellIndex !== 3 && Boolean(title) && !generated,
    ),
    "only a shortened cell or the full detail diagnostic may expose a recent-config tooltip",
  ).toEqual([]);
  expect(
    recentConfigTooltipState.filter(
      ({ generated, text, title }) =>
        Boolean(title) && !generated && title === text,
    ),
    "authored recent-config tooltips must add diagnostic detail instead of echoing visible values",
  ).toEqual([]);
  expect(
    recentConfigTooltipState.some(
      ({ cellIndex, generated, text, title }) =>
        cellIndex === 3 && Boolean(title) && !generated && title !== text,
    ),
    "full recent-config diagnostics remain available where the visible detail is shortened",
  ).toBe(true);
  await expect(page.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(page.getByLabel("VPS config target")).toHaveCount(0);
  await expect(page.getByLabel("Patch generators data grid")).toHaveCount(0);
  await expect(
    page.getByRole("button", { exact: true, name: "Apply override patch" }),
  ).toHaveCount(0);

  const links = page.getByLabel("Config overview workflow links");
  for (const label of ["Per-VPS", "VPS override patch", "Sources", "Rules"]) {
    await expect(
      links.getByRole("button", { name: new RegExp(label) }),
    ).toBeVisible();
  }

  await activate(currentState.getByRole("button", { name: "Retry" }).first());
  await expect(
    page.getByRole("heading", { name: "VPS override patch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "Bulk patch target expression" }),
  ).toHaveValue("id:agent-fra-02");

  await openConsoleSubpage(page, "Config", "Overview");
  await links.getByRole("button", { name: /Per-VPS/ }).click();
  await expect(
    page.getByRole("heading", { name: "Per-VPS desired config" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Config", "Overview");
  await page
    .getByLabel("Config overview workflow links")
    .getByRole("button", { name: /VPS override patch/ })
    .click();
  await expect(
    page.getByRole("heading", { name: "VPS override patch" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Config", "Overview");
  await page
    .getByLabel("Config overview workflow links")
    .getByRole("button", { name: /Sources/ })
    .click();
  await expect(
    page.getByRole("heading", { name: "Configuration sources" }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Effective configuration data grid"),
  ).toBeVisible();

  await openConsoleSubpage(page, "Config", "Overview");
  await page
    .getByLabel("Config overview workflow links")
    .getByRole("button", { name: /Rules/ })
    .click();
  await expect(page.getByRole("heading", { name: "VPS Rules" })).toBeVisible();
});

test("config surfaces unavailable runtime apply evidence without trusting health claims", async ({
  page,
}) => {
  await installConsoleApiMock(page, { runtimeConfigApplyFailure: true });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Config", "Overview");

  const health = page.getByLabel("Config health posture");
  await expect(health).toContainText("Evidence incomplete");
  await expect(health).toContainText(
    "Health, drift, and zero-value claims remain unknown",
  );
  await expect(health.getByText("Healthy", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Current config state by VPS")).toContainText(
    "Evidence unavailable",
  );
  await expect(page.getByLabel("Historical config apply state")).toHaveCount(0);
  const runtimeRisk = page
    .getByLabel("Config drift summary")
    .locator(".configRiskRow", { hasText: "Runtime apply state" });
  await expect(runtimeRisk).toContainText(
    "Current runtime apply evidence is unavailable.",
  );
  await expect(runtimeRisk).toContainText("Unknown");

  await page.goto("/#/fleet/instance-detail/agent-sfo-01");
  await waitForConsoleShell(page);
  const detail = page.getByLabel("Canonical VPS detail");
  const detailSection = detail.getByLabel("VPS detail section");
  if (await detailSection.isVisible()) {
    await detailSection.selectOption("Config");
  } else {
    await detail.getByRole("tab", { name: "Config", exact: true }).click();
  }
  await expect(detail.getByLabel("VPS config posture")).toContainText(
    "Apply unknown",
  );
  await expect(detail.getByLabel("VPS config posture")).toContainText(
    "cached state is not treated as current",
  );
  await expect(
    detail.getByText("Apply state unavailable", { exact: true }),
  ).toBeVisible();
  await expect(detail.getByText("Current", { exact: true })).toHaveCount(0);
});

test("config sources owns effective configuration and reusable presets", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Config", "Sources");

  await expect(
    page.getByRole("heading", { name: "Configuration sources" }),
  ).toBeVisible();
  const panel = page.locator(".configurationSourcesPanel");
  await expect(
    panel.getByRole("tab", { name: "Effective configuration" }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    panel.getByLabel("Effective configuration data grid"),
  ).toContainText("Inherited system default");
  await expect(
    panel.getByLabel("Effective configuration data grid"),
  ).toContainText("Explicit override");
  const effectiveTab = panel.getByRole("tab", {
    name: "Effective configuration",
  });
  await effectiveTab.focus();
  await effectiveTab.press("ArrowRight");
  await expect(
    panel.getByRole("tab", { name: "Configuration presets" }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(panel.getByRole("tabpanel")).toHaveAttribute(
    "aria-labelledby",
    /configuration-sources-tab-presets/,
  );
  const presets = panel.getByLabel("Configuration presets data grid");
  await expect(presets).toContainText("System default");
  await expect(presets).toContainText("System option");
  await expect(presets).toContainText("Custom");
  await expect(
    presets.getByRole("button", { name: "New preset" }),
  ).toBeVisible();
  await expect(page.getByText("Bulk mode", { exact: true })).toHaveCount(0);
});

test("config rules show affected alert policy context and route to alerts", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Config", "Rules");

  await expect(page.getByRole("heading", { name: "VPS Rules" })).toBeVisible();
  await expect(page.getByLabel("VPS rule values data grid")).toBeVisible();
  const alertContext = page.getByLabel("Affected alert policy context");
  await expect(alertContext).toContainText("Affected alert policies");
  await expect(alertContext).toContainText("edge-resource-policy");
  await expect(alertContext).toContainText("80% total quota");
  await expect(alertContext).toContainText("traffic.quota.total");
  await activate(
    alertContext.getByRole("button", { name: "Open Observability alerts" }),
  );
  await expect(page.getByText("vpsman / Observability / Alerts")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "Alerts" }),
  ).toBeVisible();
});

test("observability alerts and webhooks are explicit separate pages", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "alert and webhook registries are dense desktop operator workflows",
  );
  await gotoConsoleHome(page);

  await openConsoleSubpage(page, "Fleet", "Alerts");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet alerts" }),
  ).toBeVisible();
  await expect(page.getByLabel("Fleet alerts", { exact: true })).toContainText(
    "Tunnel adapter status failed",
  );
  await expect(page.getByLabel("Fleet alerts", { exact: true })).toContainText(
    "Tunnel adapter degraded",
  );
  await expect(page.getByLabel("Fleet alerts", { exact: true })).toContainText(
    "Traffic policy",
  );
  const criticalAlertRow = page
    .getByLabel("Fleet alerts data grid")
    .getByRole("row")
    .filter({ hasText: "Tunnel adapter status failed" })
    .first();
  await criticalAlertRow.getByRole("checkbox").check();
  await page
    .getByLabel("Fleet alerts data grid")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Acknowledge open" }),
  ).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Open VPS" })).toBeVisible();
  await page.keyboard.press("Escape");
  const alertObservedTime = page
    .getByLabel("Fleet alerts", { exact: true })
    .locator("time")
    .first();
  await expect(alertObservedTime).toContainText(/ago|in|just now/);
  await expect(alertObservedTime).toHaveAttribute("title", /2026.*(GMT|UTC)/);
  await expect(page.getByRole("button", { name: "Create policy" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Webhook rules" }),
  ).toHaveCount(0);
  await activate(
    page.getByLabel(
      "Expand Fleet alerts row fleet-alert-network-agent-fra-02-tun0",
    ),
  );
  const alertDetail = page.locator(".fleetAlertDetail").first();
  await expect(alertDetail).toContainText("Alert status");
  await expect(alertDetail).toContainText("Network");
  await expect(alertDetail.getByRole("button")).toHaveCount(0);
  await activate(page.getByLabel("Close Fleet alerts row details"));
  await page
    .getByLabel("Fleet alerts data grid")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await activate(
    page.getByRole("menuitem", { name: "Acknowledge open", exact: true }),
  );
  const triageConfirmation = page.getByLabel("Confirm fleet alert triage");
  await expect(triageConfirmation).toBeVisible();
  await expect(triageConfirmation).toContainText(
    "Tunnel adapter status failed",
  );
  await activate(
    triageConfirmation.getByRole("button", { name: "Acknowledge" }),
  );
  const triageRequests = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          fleetAlertStates: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.fleetAlertStates;
  });
  expect(triageRequests.at(-1)).toMatchObject({
    action: "acknowledge",
    alert_id: "fleet-alert-network-agent-fra-02-tun0",
    confirmed: true,
  });

  await criticalAlertRow.click({ button: "right" });
  await activate(page.getByRole("menuitem", { name: "Open VPS" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Instance detail" }),
  ).toBeVisible();
  await expect(page.locator("body")).toContainText("core-fra-02");

  await openConsoleSubpage(page, "Fleet", "Alerts");
  const refreshedCriticalAlertRow = page
    .getByLabel("Fleet alerts data grid")
    .getByRole("row")
    .filter({ hasText: "Tunnel adapter status failed" })
    .first();
  await refreshedCriticalAlertRow.click({ button: "right" });
  await activate(page.getByRole("menuitem", { name: "Policies" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Alerts" }),
  ).toBeVisible();

  await expect(page.getByText("vpsman / Observability / Alerts")).toBeVisible();
  const alertSummary = page.getByLabel("Alert routing summary");
  await expect(alertSummary).toContainText("Active fleet alerts");
  await expect(alertSummary).toContainText("Policy alerts");
  await expect(alertSummary).toContainText("Destinations");
  await expect(alertSummary).toContainText("Delivery history");
  await expect(
    alertSummary.getByRole("button", { name: "Open failed deliveries" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Alert policies" }),
  ).toBeVisible();
  const alertTabs = page.getByRole("tablist", {
    name: "Alert configuration sections",
  });
  await expect(alertTabs).toContainText("Destinations");
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Create policy" }),
  ).toBeVisible();
  await expect(page.getByText("edge-resource-policy")).toBeVisible();

  const policiesTab = alertTabs.getByRole("tab", { name: /Policies/ });
  const destinationsTab = alertTabs.getByRole("tab", {
    name: /Destinations/,
  });
  await expect(policiesTab).toHaveAttribute("tabindex", "0");
  await expect(destinationsTab).toHaveAttribute("tabindex", "-1");
  await policiesTab.focus();
  await policiesTab.press("ArrowRight");
  await expect(destinationsTab).toBeFocused();
  await expect(destinationsTab).toHaveAttribute("aria-selected", "true");
  await expect(destinationsTab).toHaveAttribute(
    "aria-controls",
    "observability-alert-destinations",
  );
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toBeVisible();
  await expect(
    page
      .getByLabel("Alert notification channels data grid")
      .getByText("edge-webhook-channel"),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Preview match" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Deliver queued" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Open delivery" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Queue dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Webhook rules" }),
  ).toHaveCount(0);

  await activate(
    page
      .getByLabel("Alert action links")
      .getByRole("button", { name: "Open triage" }),
  );
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet alerts" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Observability", "Alerts");
  await activate(page.getByRole("tab", { name: /Deliveries/ }));
  await expect(
    page.getByRole("heading", { name: "Notification deliveries" }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Notification delivery history data grid"),
  ).toBeVisible();

  await openConsoleSubpage(page, "Observability", "Event webhooks");
  await expect(
    page.getByText("vpsman / Observability / Event webhooks"),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "Event webhooks" }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Event webhooks are independent from alert notification destinations.",
    ),
  ).toBeVisible();
  await expect(page.getByLabel("Webhook routing summary")).toContainText(
    "Event webhook rules",
  );
  await expect(
    page
      .getByLabel("Webhook routing summary")
      .getByRole("button", { name: "Open failed deliveries" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Event webhook rules" }),
  ).toBeVisible();
  await expect(
    page
      .getByLabel("Webhook rules data grid")
      .getByText("edge-interval-webhook"),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Create rule" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Send test" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Retry failed" }).first(),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Queue dispatch" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Event webhook deliveries" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Event webhook maintenance" }),
  ).toHaveCount(0);

  await activate(page.getByRole("tab", { name: /Deliveries/ }));
  await expect(
    page.getByRole("heading", { name: "Event webhook deliveries" }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Webhook delivery history data grid"),
  ).toBeVisible();
  await activate(page.getByRole("tab", { name: /Maintenance/ }));
  await expect(
    page.getByRole("heading", { name: "Event webhook maintenance" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toHaveCount(0);
});

test("malformed notification channels remain visible and fail closed", async ({
  page,
}) => {
  const channelId = "fcfcfcfc-2222-4222-8222-222222222222";
  await installConsoleApiMock(page, {
    fleetAlertNotificationChannelsOverride: [
      {
        actor_id: null,
        categories: [],
        configuration_error: "fleet_alert_notification_channel_filters_invalid",
        cooldown_secs: 3600,
        created_at: "2026-06-02T10:00:00Z",
        delivery_kind: "webhook",
        enabled: true,
        id: channelId,
        min_severity: "warning",
        name: "invalid-stored-channel",
        notes: null,
        operator_states: [],
        scope_kind: "global",
        scope_value: null,
        target: "https://hooks.example/vpsman/fleet",
        updated_at: "2026-06-02T10:00:00Z",
      },
    ],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Alerts");
  await activate(page.getByRole("tab", { name: /Destinations/ }));

  const grid = page.getByLabel("Alert notification channels data grid");
  await expect(grid).toContainText("invalid-stored-channel");
  await expect(grid).toContainText("Invalid stored filters");
  await expect(grid).toContainText("Channel is skipped until replaced");
  await expect(
    page.getByRole("button", { name: "Preview match" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Queue dispatch" }),
  ).toBeDisabled();

  await grid
    .getByLabel(`Select Alert notification channels row ${channelId}`)
    .check();
  await grid.getByRole("button", { name: "Actions" }).click();
  await expect(page.getByRole("menuitem", { name: "Edit" })).toHaveAttribute(
    "data-disabled",
    "",
  );
  await expect(page.getByRole("menuitem", { name: "Enable" })).toHaveAttribute(
    "data-disabled",
    "",
  );
  await expect(page.getByRole("menuitem", { name: "Disable" })).toHaveAttribute(
    "data-disabled",
    "",
  );
  await expect(
    page.getByRole("menuitem", { name: "Review deletion" }),
  ).not.toHaveAttribute("data-disabled", "");
  await activate(page.getByRole("menuitem", { name: "Details" }));
  await expect(
    page.getByText("Notification channel details", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("invalid — skipped", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Stored filters are invalid; delete and replace this channel",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Edit channel" }),
  ).toBeDisabled();
});

test("observability alert policy editor is a focused create flow", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Alerts");

  await activate(page.getByRole("button", { name: "Create policy" }));
  const editor = page.locator(".consoleDetailPanel", {
    hasText: "Create alert policy",
  });
  await expect(editor).toBeVisible();
  await expect(page.getByLabel("Alert routing summary")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open triage" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("tablist", { name: "Alert configuration sections" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Notification channels" }),
  ).toHaveCount(0);
  await expect(editor).toContainText("Enable after creation");
  await expect(editor).toContainText("Preview matches before saving");
  await expect(
    editor.getByRole("button", { name: "Preview matches" }),
  ).toBeVisible();
  await expect(
    editor.getByRole("button", { name: "Create policy" }),
  ).toBeVisible();
  await expect(editor.getByRole("button", { name: "Dry-run" })).toHaveCount(0);
  await expect(
    editor.getByRole("button", { name: "Review create" }),
  ).toHaveCount(0);
  await expect(editor.getByRole("button", { name: "New policy" })).toHaveCount(
    0,
  );

  const policySelector = editor.getByLabel("Policy VPS selector expression");
  await policySelector.fill("tag:edge &&");
  await expect(
    editor.getByRole("button", { name: "Preview matches" }),
  ).toBeDisabled();
  await expect(
    editor.getByRole("button", { name: "Preview matches" }),
  ).toHaveAttribute("title", /Invalid policy VPS selector/);
  await policySelector.fill("fo01");
  await activate(
    page.getByRole("option", {
      name: /edge-sfo-01.*ID.*agent-sfo-01/,
    }),
  );
  await expect(policySelector).toHaveValue("id:agent-sfo-01");
  await policySelector.fill("tag:edge");
  await activate(
    page.getByRole("option", {
      name: "tag:role:edge",
      exact: true,
    }),
  );
  await expect(policySelector).toHaveValue("tag:role:edge");
  await expect(
    editor.getByLabel("Alert policy local VPS preview"),
  ).toContainText("edge-sfo-01");
  await editor
    .getByLabel("Rule condition expression")
    .fill("traffic.cycle.total >= traffic.quota.total * 0.8");
  await activate(editor.getByRole("button", { name: "Preview matches" }));
  await expect(editor).toContainText("Matches 1 VPS");
  await expect(editor).toContainText("Match preview");
  await expect(
    page.locator(".fleetPolicyActionFeedback.actionFeedbackSuccess"),
  ).toHaveText("dry-run matched 1 VPS");
  await expect(editor).toContainText("0 incomplete VPSs; 0 invalid rule rows.");
  await expect(
    page.locator(".observabilityAlertsPanel > .fleetPolicyStatus"),
  ).toHaveCount(0);
  await expect(editor.getByLabel("Policy name")).toHaveValue("");
  await expect(editor.getByLabel("Rule name")).toHaveValue("");
  await editor.getByLabel("Policy name").fill("edge-resource-policy");
  await editor.getByLabel("Rule name").fill("80% total quota");
  await activate(editor.getByRole("button", { name: "Create policy" }));
  const confirmation = page.getByLabel("Confirm alert policy save");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Matched VPS");
  await activate(confirmation.getByRole("button", { name: "Cancel" }));
  await activate(editor.getByRole("button", { name: "Create policy" }));
  await activate(
    page
      .getByLabel("Confirm alert policy save")
      .getByRole("button", { name: "Create alert policy" }),
  );
  await expect(
    page.locator(".fleetPolicyActionFeedback.actionFeedbackSuccess"),
  ).toHaveText("saved edge-resource-policy");
  const savedEditor = page.locator(".consoleDetailPanel", {
    hasText: "Edit alert policy",
  });
  await expect(savedEditor).toContainText("Match preview");
  await activate(
    savedEditor.getByRole("button", { name: "Close detail panel" }),
  );
  await expect(page.getByLabel("Alert routing summary")).toBeVisible();
});

test("observability webhook rule editor retains registry and navigation context", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Event webhooks");

  await activate(page.getByRole("button", { name: "Create rule" }));
  const editor = page.locator(".consoleDetailPanel", {
    hasText: "Create webhook rule",
  });
  const ruleGrid = page.getByLabel("Webhook rules data grid");
  await expect(editor).toBeVisible();
  await expect(page.getByLabel("Webhook routing summary")).toBeVisible();
  await expect(page.getByLabel("Event webhook sections")).toBeVisible();
  await expect(ruleGrid).toBeVisible();
  expect(
    await ruleGrid.evaluate(
      (grid, detail) =>
        Boolean(
          grid.compareDocumentPosition(detail as Node) &
          Node.DOCUMENT_POSITION_FOLLOWING,
        ),
      await editor.elementHandle(),
    ),
  ).toBe(true);
  await expect(page.getByText("Event webhook tests")).toHaveCount(0);
  await expect(editor).toContainText("Enable after creation");
  await expect(editor).toContainText("Signing secret");
  await expect(editor).toContainText("Sample payload");
  await expect(editor).toContainText("Test before saving");
  const bodyTemplateEditor = editor.getByRole("textbox", {
    name: "Webhook body template",
  });
  await expect(bodyTemplateEditor).toBeVisible();
  const bodyTemplateLines = await bodyTemplateEditor
    .locator(".cm-line")
    .allTextContents();
  expect(bodyTemplateLines).toEqual([
    "{#",
    "Alert: [{alert.severity}] {alert.title} on {vps.display_name} ({event.id})",
    "Traffic threshold: {vps.display_name} used {traffic.cycle_percent}% in {policy.name}; source rule {policy_rule.name}",
    "Resource threshold: [{alert.severity}] {alert.title} on {vps.display_name}; condition {policy_rule.condition_expression}",
    "VPS status event: [{event.kind}] {vps.display_name} is {vps.status}",
    'Interval fleet summary: [{event.kind}] {matched_vps.length} VPSs: {matched_vps.map(vps.name).join(", ")}',
    "#}",
    "[{event.kind}] {rule.name}: {vps.display_name} ({vps.id}) is {vps.status}",
  ]);
  await expect(editor).toContainText(
    "The multiline block between standalone {# and #} markers contains non-rendering examples",
  );
  const cooldown = editor.getByLabel("Webhook cooldown seconds");
  await expect(cooldown).toHaveAttribute("min", "0");
  await expect(cooldown).toHaveAttribute("max", "2592000");
  await editor
    .getByLabel("Webhook signing secret")
    .fill("fixture-webhook-secret");
  await editor.getByLabel("Webhook rule name").fill("edge-status-webhook");
  await editor
    .getByLabel("Webhook expression")
    .fill("interval.30sec && tag:edge");
  await editor
    .getByLabel("Webhook target")
    .fill("https://hooks.example.net/vpsman");
  await expect(editor.getByRole("button", { name: "Test" })).toBeVisible();
  await expect(
    editor.getByRole("button", { name: "Create rule" }),
  ).toBeVisible();
  await expect(
    editor.getByRole("button", { name: "Review create" }),
  ).toHaveCount(0);
  await expect(editor.getByRole("button", { name: "New rule" })).toHaveCount(0);

  await activate(editor.getByRole("button", { name: "Test" }));
  await expect(editor).toContainText("VPSs matched");
  await expect(editor).toContainText("Rendered message");
  await expect(editor).toContainText("Delivery status");
  await expect(page.getByLabel("Webhook routing summary")).toBeVisible();

  await activate(editor.getByRole("button", { name: "Close detail panel" }));
  await expect(page.getByLabel("Webhook routing summary")).toBeVisible();

  await ruleGrid
    .getByLabel("Select Webhook rules row fefefefe-1111-4111-8111-111111111111")
    .check();
  await ruleGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Disable" }).click();
  await expect(
    page.locator(".fleetPolicyActionFeedback.actionFeedbackSuccess"),
  ).toHaveText("Disabled 1 webhook rule");
  await expect(page.getByLabel("Webhook routing summary")).toContainText(
    "1 disabled rule",
  );
});

test("raw editors do not echo drafts while API evidence remains available in tooltips", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the adapter registry and dense delivery grids are covered on desktop",
  );
  const rawJsonSentinel = "raw-json-tooltip-sentinel";
  const rawTomlSentinel = "raw-toml-tooltip-sentinel";
  const argvSentinel = "argv-tooltip-sentinel";
  const environmentSentinel = "environment-tooltip-sentinel";
  const adapterSentinel = "adapter-tooltip-sentinel";
  const fixtureDestination = "https://hooks.example/vpsman/edge-capacity";
  const fixtureBody =
    "{rule.name} {event.kind} count={matched_vps.length} {matched_vps.0.display_name}";
  const fixtureDeliveryError = "fixture receiver returned 503";
  const expectControlDoesNotEcho = async (
    control: import("@playwright/test").Locator,
    value: string,
  ) => {
    expect((await control.getAttribute("title")) ?? "").not.toContain(value);
  };

  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Config", "VPS override patch");
  const jsonEditor = page.getByLabel("Patch generator values JSON");
  await jsonEditor.fill(`{"probe":"${rawJsonSentinel}"}`);
  await expectControlDoesNotEcho(jsonEditor, rawJsonSentinel);
  await activate(
    page.getByRole("button", { name: "Temporary patch", exact: true }),
  );
  const tomlEditor = page.getByLabel(
    "Temporary bulk runtime config patch TOML",
  );
  await tomlEditor.fill(`[probe]\nvalue = "${rawTomlSentinel}"`);
  await expectControlDoesNotEcho(tomlEditor, rawTomlSentinel);

  await openConsoleSubpage(page, "Config", "Sources");
  const sources = page.locator(".configurationSourcesPanel");
  await activate(sources.getByRole("tab", { name: "Configuration presets" }));
  await activate(sources.getByRole("button", { name: "New preset" }));
  const presetDrawer = page.getByRole("complementary", {
    name: "New configuration preset",
  });
  await presetDrawer
    .getByLabel("Preset behavior")
    .selectOption("command_execution");
  const argvEditor = presetDrawer.getByLabel("Shell command arguments");
  const environmentEditor = presetDrawer.getByLabel(
    "Command environment values",
  );
  await argvEditor.fill(`/opt/operator/${argvSentinel}`);
  await environmentEditor.fill(`PROBE=${environmentSentinel}`);
  await expectControlDoesNotEcho(argvEditor, argvSentinel);
  await expectControlDoesNotEcho(environmentEditor, environmentSentinel);

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const adapterRegistry = page.getByLabel("Network adapter definitions");
  await activate(
    adapterRegistry.getByRole("button", { name: "Tunnel runtime adapter" }),
  );
  const adapterDrawer = page.getByRole("complementary", {
    name: "New tunnel runtime adapter",
  });
  const adapterEditor = adapterDrawer.getByLabel("Status adapter command");
  await adapterEditor.fill(`/opt/operator/${adapterSentinel}`);
  await expectControlDoesNotEcho(adapterEditor, adapterSentinel);

  await openConsoleSubpage(page, "Observability", "Event webhooks");
  const ruleGrid = page.getByLabel("Webhook rules data grid");
  await expect(ruleGrid).toContainText(fixtureDestination);
  await activate(ruleGrid.getByLabel(/Expand Webhook rules row/).first());
  await expect(ruleGrid).toContainText(fixtureBody);
  const ruleTitles = await ruleGrid
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(ruleTitles).toContain(fixtureDestination);
  expect(ruleTitles).toContain(fixtureBody);

  await activate(page.getByRole("tab", { name: /Deliveries/ }));
  const deliveryGrid = page.getByLabel("Webhook delivery history data grid");
  await expect(deliveryGrid).toContainText(fixtureDestination);
  await expect(deliveryGrid).toContainText(fixtureDeliveryError);
  const deliveryTitles = await deliveryGrid
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(deliveryTitles).toContain(fixtureDeliveryError);
});

test("config bulk patch requires reviewed scope and privilege before apply", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk patch review is a dense desktop workflow covered by desktop release tests",
  );
  await gotoConsoleHome(page);
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.config.bulk.selectorExpression"),
  );
  await openConsoleSubpage(page, "Config", "VPS override patch");

  await expect(
    page.getByRole("heading", { name: "VPS override patch" }),
  ).toBeVisible();
  const bulk = page.locator(".configApplyGrid:visible");
  await expect(
    bulk.getByRole("combobox", { name: "Bulk patch target expression" }),
  ).toBeVisible();
  await expect(
    bulk.getByRole("button", { name: "Preview changes" }),
  ).toBeDisabled();
  await expect(
    bulk.getByRole("button", { exact: true, name: "Apply override patch" }),
  ).toBeDisabled();
  await expect(bulk.locator(".privilegeManager")).toHaveCount(0);
  await expect(bulk).not.toContainText("Clear local vault");
  await expect(bulk).not.toContainText("Clear local session");

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "VPS override patch");
  const unlockedBulk = page.locator(".configApplyGrid:visible");
  await unlockedBulk
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await activate(unlockedBulk.getByRole("button", { name: "Preview changes" }));
  await expect(unlockedBulk).toContainText("1 VPS verified");
  await expect(
    unlockedBulk.getByLabel("Bulk patch change summary"),
  ).toContainText("edge-sfo-01");
  await expect(
    unlockedBulk.getByRole("button", {
      exact: true,
      name: "Apply override patch",
    }),
  ).toBeEnabled();

  await activate(
    unlockedBulk.getByRole("button", {
      exact: true,
      name: "Apply override patch",
    }),
  );
  const confirmation = page.getByLabel("Confirm VPS override patch");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("id:agent-sfo-01");
  await expect(confirmation).toContainText("Targets");
  await expect(confirmation).toContainText("Payload");
  await activate(
    confirmation.getByRole("button", { name: "Apply VPS override patch" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });
});

test("config per-vps preserves reviewed one-vps replacement workflow", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "one-VPS config replacement is a dense desktop workflow covered by desktop release tests",
  );
  await gotoConsoleHome(page);
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.config.single.clientId"),
  );
  await openConsoleSubpage(page, "Config", "Per-VPS");

  await expect(
    page.getByRole("heading", { name: "Per-VPS desired config" }),
  ).toBeVisible();
  const panel = page.locator(".singleConfigWorkspace");
  await expect(
    panel.getByRole("combobox", { name: "VPS config target" }),
  ).toBeVisible();
  await expect(panel.getByLabel("Per-VPS config start")).toContainText(
    "One desired configuration, edited in place",
  );
  await expect(panel.getByRole("tab", { name: "Advanced" })).toHaveCount(0);
  await expect(panel.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(page.getByLabel("Patch generators data grid")).toHaveCount(0);

  await chooseVpsBySearch(
    panel,
    "VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  const interval = panel.getByLabel("Telemetry interval", { exact: true });
  await expect(interval).toHaveValue("30");
  await interval.fill("60");
  await activate(panel.getByRole("button", { name: "Review changes" }));
  await expect(panel.getByLabel("Reviewed VPS config changes")).toContainText(
    "telemetry_interval_secs",
  );
  await expect(panel.locator(".privilegeManager")).toHaveCount(0);
  await expect(panel).not.toContainText("Clear local vault");
  await expect(panel).not.toContainText("Clear local session");
  await activate(panel.getByRole("button", { name: "Unlock to apply" }));
  await expect(
    page.getByRole("dialog", { name: "Unlock privilege" }),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Close privilege unlock" }));
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const unlockedPanel = page.locator(".singleConfigWorkspace");
  const unlockedInterval = unlockedPanel.getByLabel("Telemetry interval", {
    exact: true,
  });
  await expect(unlockedInterval).toHaveValue("60");
  await expect(
    unlockedPanel.getByLabel("Reviewed VPS config changes"),
  ).toBeVisible();
  await activate(unlockedPanel.getByRole("button", { name: "Apply reviewed" }));

  const confirmation = page.getByLabel("Confirm VPS runtime override");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("core-fra-02");
  await expect(confirmation).toContainText("runtime changes");
  await activate(
    confirmation.getByRole("button", { name: "Apply VPS override" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: Record<string, unknown>;
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(request?.pathname).toBe(
    "/api/v1/runtime-config/clients/agent-fra-02/override/apply",
  );
  expect(request?.body).toMatchObject({
    candidate: {
      type: "structured",
      value: { telemetry_interval_secs: 60 },
    },
    confirmed: true,
    expected_override_revision: "0",
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
});

test("observability hides unfinished process metrics from normal navigation", async ({
  page,
}) => {
  expect(normalizeSubpage("Observability", "process_metrics")).toBe(
    "fleet_metrics",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Fleet metrics");

  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet metrics" }),
  ).toBeVisible();
  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  if ((await nav.count()) > 0) {
    await expect(nav).not.toContainText("Process metrics");
  }
  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    await expect(mobilePageSelector).not.toContainText("Process metrics");
  }
  await expect(
    page.locator(
      '[aria-label*="Process metrics"][aria-label*="release status"]',
    ),
  ).toHaveCount(0);
});

test("observability fleet metrics owns resource charts and read-only analysis controls", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Fleet metrics");

  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet metrics" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "CPU load by VPS" }),
  ).toBeVisible();
  await expect(
    page.getByText(
      /Metric definition: Each chart point averages available Linux 1-minute load evidence/,
    ),
  ).toBeVisible();

  const controls = page.getByLabel("Fleet metrics controls");
  const rangeControls = controls.getByLabel("Fleet metrics time range");
  await expect(rangeControls).toContainText("1d");
  const realtimeRange = rangeControls.getByRole("button", {
    name: "Realtime · last 15 minutes",
  });
  await expect(realtimeRange).toHaveText("15m");
  await expect(realtimeRange).toHaveAttribute(
    "title",
    "Realtime · last 15 minutes",
  );
  for (const name of [
    "Last 1 hour",
    "Last 8 hours",
    "Last 1 day",
    "Last 7 days",
    "Last 30 days",
    "Last 90 days",
    "Last 180 days",
    "Last 1 year",
    "All retained time",
  ]) {
    await expect(rangeControls.getByRole("button", { name })).toBeVisible();
  }
  await expect(controls.getByRole("button", { name: "CPU" })).toHaveClass(
    /active/,
  );
  await expect(controls.getByLabel("Fleet metrics group by")).toBeVisible();
  await expect(page.getByText("Scope: All VPS", { exact: true })).toBeVisible();
  await expect(page.getByText(/Telemetry .* behind/).first()).toBeVisible();

  const advancedFilters = page.locator(".fleetMetricsAdvancedFilters");
  await advancedFilters.getByText("Advanced filters", { exact: true }).click();
  await expect(
    advancedFilters.getByLabel("Fleet metrics scope kind"),
  ).toBeVisible();
  await expect(
    advancedFilters.getByLabel("Fleet metrics point density"),
  ).toHaveValue("balanced");
  await advancedFilters
    .getByLabel("Fleet metrics scope kind")
    .selectOption("provider");
  await advancedFilters
    .getByLabel("Fleet metrics scope value")
    .selectOption({ index: 1 });
  await expect(page.getByText(/Scope: provider:/).first()).toBeVisible();
  await advancedFilters.getByRole("button", { name: "Reset filters" }).click();
  await expect(page.getByText("Scope: All VPS", { exact: true })).toBeVisible();

  const summary = page.getByLabel("Fleet metrics summary");
  await expect(summary).toContainText("Current metric");
  await expect(summary).toContainText("Telemetry freshness");
  await expect(summary).toContainText("Selected range");
  await expect(summary).toContainText("Data available");
  await expect(summary).toContainText("1 VPS unavailable");

  const definitions = page.getByLabel("Fleet metrics availability definitions");
  await expect(definitions).toContainText("Active alerts");
  await expect(definitions).toContainText("VPS in shown evidence");
  await expect(definitions).toContainText("Alerts in shown groups");
  await expect(definitions).toContainText("Unavailable VPS");

  await expect(page.locator(".timeSeriesChartShell")).toBeVisible();
  await expect(page.locator(".timeSeriesChartShell").first()).toHaveAttribute(
    "data-gap-policy",
    "preserve",
  );
  await expect(page.locator(".timeSeriesChartShell").first()).toHaveAttribute(
    "data-render-mode",
    "points",
  );
  await expect(page.getByText(/Selected: 1d .* Data available:/)).toBeVisible();
  await expect(page.getByText(/Last sample:/)).toBeVisible();
  await expect(
    page.getByText(/Sparse data: .* retained time bucket.* across 3 VPS/),
  ).toBeVisible();
  await expect(
    page.getByLabel("Fleet resource usage curve data coverage"),
  ).toContainText("points present in selected range");
  await expect(
    page.getByLabel("Fleet resource usage curve data coverage"),
  ).toContainText("2026");
  await expect(
    page.getByLabel("Fleet resource usage curve data coverage"),
  ).toContainText(/GMT|UTC/);
  await expect(
    page.getByLabel("Fleet resource usage curve data coverage"),
  ).toContainText(/latest sample (current|stale)/);
  await expect(page.locator(".timeSeriesLegend")).toContainText("core-fra-02");
  const resourceChart = page.locator(".timeSeriesChartShell").first();
  const coreSeries = resourceChart.getByRole("button", {
    name: "Hide core-fra-02 series",
  });
  await coreSeries.click();
  await expect(
    resourceChart.getByRole("button", { name: "Show core-fra-02 series" }),
  ).toHaveAttribute("aria-pressed", "false");
  await expect(
    resourceChart.getByText("2/3 series", { exact: true }),
  ).toBeVisible();

  const downloadPromise = page.waitForEvent("download");
  await resourceChart.getByRole("button", { name: "Export CSV" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("fleet-cpu-load.csv");
  const downloadPath = await download.path();
  expect(downloadPath).not.toBeNull();
  const csv = await readFile(downloadPath!, "utf8");
  expect(csv).toContain("timestamp,edge-sfo-01");
  expect(csv).not.toContain("core-fra-02");

  await resourceChart.getByRole("button", { name: "Show all" }).click();
  await expect(
    resourceChart.getByRole("button", { name: "Hide core-fra-02 series" }),
  ).toHaveAttribute("aria-pressed", "true");
  await resourceChart.locator(".timeSeriesChart").focus();
  await resourceChart.locator(".timeSeriesChart").press("ArrowLeft");
  await expect(resourceChart.locator(".timeSeriesHover")).toContainText(
    "core-fra-02",
  );
  await expect(resourceChart.locator(".timeSeriesHover")).toContainText(
    /GMT|UTC/,
  );
  await expect(page.getByLabel("Top resource VPS list")).toContainText(
    "edge-sfo-01",
  );
  await expect(page.getByLabel("Fleet metrics group breakdown")).toContainText(
    "country:US",
  );
  await expect(page.getByLabel("Fleet metrics group breakdown")).toContainText(
    "alerts",
  );
  await expect(page.getByLabel("Fleet metrics group breakdown")).toContainText(
    "2/3 online",
  );
  await expect(page.getByLabel("Fleet metrics group breakdown")).toContainText(
    "0 access revoked",
  );

  await controls.getByRole("button", { name: "Memory" }).click();
  await expect(
    page.getByRole("heading", { name: "Memory used by VPS" }),
  ).toBeVisible();
  await expect(
    page.getByText(
      /Metric definition: Each chart point averages available used-memory ratio evidence/,
    ),
  ).toBeVisible();
  const memoryFigureLabel = await page
    .getByRole("figure", { name: /Fleet resource usage curve/ })
    .locator("figcaption")
    .textContent();
  expect(memoryFigureLabel).toMatch(/Latest values: .+%/);
  expect(memoryFigureLabel).not.toMatch(/\b[1-9]\d{2,}%/);
  await controls.getByRole("button", { name: "Disk" }).click();
  await expect(
    page.getByText(
      /Metric definition: Each chart point derives free space from available aggregate-filesystem used-ratio evidence/,
    ),
  ).toBeVisible();
  const diskFigureLabel = await page
    .getByRole("figure", { name: /Fleet resource usage curve/ })
    .locator("figcaption")
    .textContent();
  expect(diskFigureLabel).toMatch(/Latest values: .+%/);
  expect(diskFigureLabel).not.toMatch(/\b[1-9]\d{2,}%/);

  await expect(
    page
      .locator(".observabilityMetricsPanel")
      .getByRole("button", { name: /Run tests|Apply|Dispatch|Delete|Create/ }),
  ).toHaveCount(0);

  await page
    .getByRole("button", { name: "Open edge-sfo-01 instance detail" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Instance detail" }),
  ).toBeVisible();
});

test("fleet metrics freshness uses exact sample time instead of the coarse chart bucket", async ({
  page,
}) => {
  await installConsoleApiMock(page, {
    dashboardLatestSampleAtOverride: "2026-06-05T20:44:30Z",
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Fleet metrics");

  const summary = page.getByLabel("Fleet metrics summary");
  await expect(summary).toContainText("Telemetry freshness");
  await expect(summary).toContainText("Current");
  await expect(page.locator(".observabilityStaleBanner")).toHaveCount(0);
});

test("fleet metrics exposes persisted scope range and density instead of applying hidden filters", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "vpsman.dashboardPreferences",
      JSON.stringify({
        endAt: "2026-06-06T04:44:00.000Z",
        groupBy: "providers",
        networkView: "speed",
        pointDensity: "dense",
        refreshIntervalSecs: 30,
        resourceMetric: "memory_used",
        scopeKind: "provider",
        scopeValue: "alpha",
        startAt: "2026-06-05T04:44:00.000Z",
        trafficSort: "total",
        window: "1d",
      }),
    );
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Fleet metrics");

  await expect(
    page.getByText("Scope: provider:alpha", { exact: true }),
  ).toBeVisible();
  const summary = page.getByLabel("Fleet metrics summary");
  await expect(summary).toContainText("Custom");
  await expect(summary).toContainText("provider:alpha");
  const advancedFilters = page.locator(".fleetMetricsAdvancedFilters");
  await expect(advancedFilters.locator("summary b")).toHaveText("3");
  await advancedFilters.locator("summary").click();
  await expect(
    advancedFilters.getByLabel("Fleet metrics scope kind"),
  ).toHaveValue("provider");
  await expect(
    advancedFilters.getByLabel("Fleet metrics scope value"),
  ).toHaveValue("alpha");
  await expect(
    advancedFilters.getByLabel("Fleet metrics point density"),
  ).toHaveValue("dense");
  await expect(
    advancedFilters.getByLabel("Fleet metrics start date"),
  ).not.toHaveValue("");
  await expect(
    advancedFilters.getByLabel("Fleet metrics end date"),
  ).not.toHaveValue("");
});

test("observability network metrics is chart-first and mutation-free", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Network metrics");
  await page.reload();

  await expect(
    page.getByRole("heading", { level: 1, name: "Network metrics" }),
  ).toBeVisible();
  const panel = page.locator(".observabilityNetworkMetricsPanel");
  await expect(
    panel.getByRole("heading", { name: "Latency, loss, and throughput" }),
  ).toBeVisible();
  await expect(panel.getByText("Stale network evidence")).toBeVisible();
  await expect(panel.getByText(/Time filter: retained evidence/)).toBeVisible();
  const summary = panel.getByLabel("Network metrics summary");
  await expect(summary).toContainText("Observations");
  await expect(summary).toContainText("Degraded signals");
  await expect(summary).toContainText("OSPF review");
  await expect(
    panel.getByLabel("Network metrics count definitions"),
  ).toHaveCount(0);
  const metricSelector = panel.getByLabel("Network metric selector");
  await expect(metricSelector).toContainText("Latency");
  await expect(metricSelector).toContainText("Packet loss");
  await expect(metricSelector).toContainText("Throughput");
  await expect(
    panel.getByText(/Metric definition: Each point is the mean RTT/),
  ).toBeVisible();
  await expect(
    panel.getByText(/Sparse data: 1\/4 measured points present/),
  ).toBeVisible();
  await expect(
    panel.getByLabel("Network metrics latency chart data coverage"),
  ).toContainText("1/2 points present");
  await expect(
    panel.getByLabel("Network metrics latency chart data coverage"),
  ).toContainText("1 gap");
  await expect(panel.locator(".timeSeriesLegend").first()).toContainText(
    "sfo-fra-gre",
  );
  await expect(panel.locator(".timeSeriesChartShell").first()).toHaveAttribute(
    "data-render-mode",
    "points",
  );
  await panel.getByRole("button", { name: "Throughput" }).click();
  await expect(page).toHaveURL(
    /\?network_metric=throughput#\/observability\/network-metrics$/,
  );
  await expect(panel.getByText(/Time filter: retained evidence/)).toBeVisible();
  await expect(
    panel.getByText(/Metric definition: Each point is average TCP throughput/),
  ).toBeVisible();
  await page.reload();
  await expect(
    panel.getByText(/Metric definition: Each point is average TCP throughput/),
  ).toBeVisible();
  await page.goBack();
  await expect(
    panel.getByText(/Metric definition: Each point is the mean RTT/),
  ).toBeVisible();
  await page.goForward();
  await expect(
    panel.getByText(/Metric definition: Each point is average TCP throughput/),
  ).toBeVisible();
  await expect(panel.getByLabel("Network throughput benchmark")).toContainText(
    "Average throughput 10.1 Mbps",
  );
  await expect(panel.getByLabel("Network throughput benchmark")).toContainText(
    "expected 100 Mbps",
  );
  await expect(panel.getByLabel("Network throughput benchmark")).toContainText(
    "degraded",
  );

  await expect(
    panel.getByLabel("Network metrics tunnel grouping"),
  ).toContainText("sfo-fra-gre");
  await expect(
    panel.getByLabel("Network metrics tunnel grouping"),
  ).toContainText("agent-fra-02 -> agent-sfo-01");
  await expect(panel.getByLabel("Network endpoint comparison")).toContainText(
    "agent-sfo-01 ->",
  );
  await expect(panel.getByLabel("Network endpoint comparison")).toContainText(
    "External observed",
  );
  await expect(
    panel.getByLabel("Network endpoint comparison"),
  ).not.toContainText("wg-import");
  await expect(
    panel.getByLabel("Network endpoint comparison"),
  ).not.toContainText(/no measurement/i);
  await expect(panel.getByLabel("Network endpoint comparison")).toContainText(
    "Unverified",
  );
  await expect(
    panel.getByLabel("Network endpoint comparison"),
  ).not.toContainText("Down");
  await expect(
    panel.getByLabel("Network metrics review signals"),
  ).toContainText("OSPF delta");
  await expect(
    panel.getByRole("button", {
      name: /Run status|Run probe|Run speed|Apply|Rollback|Dispatch|Delete|Create/,
    }),
  ).toHaveCount(0);

  await panel.getByRole("button", { name: "Open Network tests" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network tests" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Observability", "Network metrics");
  await page
    .locator(".observabilityNetworkMetricsPanel")
    .getByRole("button", { name: "Open OSPF review" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network OSPF" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Manage plan overrides" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Configure VPS presets" }),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Manage plan overrides" }));
  await expect(page.getByText("vpsman / Network / Tunnel plans")).toBeVisible();
  const routingAdapterDrawer = page.getByRole("complementary", {
    name: "New routing cost adapter",
  });
  await expect(routingAdapterDrawer).toBeVisible();
  await expect(
    routingAdapterDrawer.getByLabel("Read cost adapter command"),
  ).toHaveValue(
    "/opt/operator/routing-cost\nstatus\n--plan-id\n{plan_id}\n--interface\n{interface}\n--side\n{endpoint_side}",
  );
  await expect(
    routingAdapterDrawer.getByLabel("Update cost adapter command"),
  ).toHaveValue(
    "/opt/operator/routing-cost\napply\n--plan-id\n{plan_id}\n--interface\n{interface}\n--side\n{endpoint_side}\n--cost\n{desired_cost}",
  );
  await expect(routingAdapterDrawer).toContainText(
    "Read cost must print one number from 1 to 65535",
  );
  await expect(routingAdapterDrawer).toContainText(
    "Update reports failure by exit code",
  );
  await expect(routingAdapterDrawer).toContainText(
    "Routing cost adapters require both Read cost and Update cost",
  );
  await expect(routingAdapterDrawer).not.toContainText(
    "Tunnel runtimes require",
  );
});

test("observability dashboards manages read-only dashboard presets", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Dashboards");

  await expect(
    page.getByRole("heading", { level: 1, name: "Dashboards" }),
  ).toBeVisible();
  const panel = page.locator(".observabilityDashboardsPanel");
  await expect(panel.getByLabel("Dashboard manager summary")).toContainText(
    "Dashboard presets",
  );
  await expect(panel.getByLabel("Dashboard manager summary")).toContainText(
    "Data freshness",
  );
  await expect(panel.getByLabel("Dashboard manager summary")).toContainText(
    "Source counts",
  );
  await expect(panel.getByLabel("Dashboard source coverage")).toContainText(
    "Fleet source",
  );
  await expect(panel.getByLabel("Dashboard source coverage")).toContainText(
    "Sparse 1 day",
  );
  await expect(panel).not.toContainText("Saved dashboards");

  const registry = panel.getByLabel("Dashboard preset registry");
  if (testInfo.project.name.includes("mobile")) {
    const presetSelector = panel.getByLabel("Dashboard preset", {
      exact: true,
    });
    for (const name of [
      "Fleet operations",
      "Resource capacity",
      "Network traffic",
      "Group posture",
    ]) {
      await expect(presetSelector).toContainText(name);
    }
  } else {
    for (const name of [
      "Fleet operations",
      "Resource capacity",
      "Network traffic",
      "Group posture",
    ]) {
      await expect(
        registry.getByRole("button", { name: new RegExp(name) }),
      ).toBeVisible();
    }
  }

  await expect(
    panel.getByLabel("Fleet operations dashboard widgets"),
  ).toContainText("Recent alerts");
  await expect(
    panel.getByLabel("Fleet operations dashboard widgets"),
  ).toContainText("Degraded VPS");

  if (testInfo.project.name.includes("mobile")) {
    await panel
      .getByLabel("Dashboard preset", { exact: true })
      .selectOption({ label: "Resource capacity" });
  } else {
    await registry.getByRole("button", { name: /Resource capacity/ }).click();
  }
  await expect(
    panel.getByLabel("Resource capacity dashboard widgets"),
  ).toContainText("Top resource VPS");
  await expect(
    panel.getByLabel("Resource capacity chart widget"),
  ).toBeVisible();
  await expect(
    panel.getByLabel("Resource capacity chart widget"),
  ).toContainText("Sparse 1 day");
  await expect(panel.locator(".timeSeriesChartShell")).toBeVisible();

  if (testInfo.project.name.includes("mobile")) {
    await panel
      .getByLabel("Dashboard preset", { exact: true })
      .selectOption({ label: "Network traffic" });
  } else {
    await registry.getByRole("button", { name: /Network traffic/ }).click();
  }
  await expect(
    panel.getByLabel("Network traffic dashboard widgets"),
  ).toContainText("Network rate chart");
  await expect(panel.getByLabel("Network rate chart widget")).toContainText(
    "Sparse 1 day",
  );
  await expect(panel.getByLabel("Top network VPS widget table")).toContainText(
    "edge-sfo-01",
  );

  if (testInfo.project.name.includes("mobile")) {
    await panel
      .getByLabel("Dashboard preset", { exact: true })
      .selectOption({ label: "Group posture" });
  } else {
    await registry.getByRole("button", { name: /Group posture/ }).click();
  }
  await expect(
    panel.getByLabel("Group posture dashboard widgets"),
  ).toContainText("country:US");
  await expect(page).toHaveURL(
    /\?dashboard=group_posture#\/observability\/dashboards$/,
  );
  await page.reload();
  await expect(
    page
      .locator(".observabilityDashboardsPanel")
      .getByLabel("Group posture dashboard widgets"),
  ).toContainText("country:US");

  await expect(
    page
      .locator(".observabilityDashboardsPanel")
      .getByRole("button", { exact: true, name: "Copy link" }),
  ).toBeVisible();
  await expect(
    page
      .locator(".observabilityDashboardsPanel")
      .getByRole("button", { name: "Export JSON" }),
  ).toBeVisible();
  const reloadedPanel = page.locator(".observabilityDashboardsPanel");
  await reloadedPanel
    .getByLabel("Dashboard section selector")
    .getByRole("button", { name: /Copy \/ Export/ })
    .click();
  await expect(
    reloadedPanel.getByLabel("Dashboard copy and export details"),
  ).toContainText("Read-only");
  const downloadPromise = page.waitForEvent("download");
  await reloadedPanel.getByRole("button", { name: "Export JSON" }).click();
  await downloadPromise;
  await expect(
    reloadedPanel.locator(".dashboardActionFeedback.actionFeedbackSuccess"),
  ).toContainText("Exported Group posture");
  await expect(reloadedPanel.locator(".dashboardManagerStatus")).toHaveCount(0);
  await expect(
    reloadedPanel.getByRole("button", {
      name: /Open terminal|Run backup|Dispatch|Apply|Delete|Restart|Stop|Create/,
    }),
  ).toHaveCount(0);
});

test("observability dashboards use safe labels when summary counts are missing", async ({
  page,
}) => {
  await installConsoleApiMock(page, {
    dashboardSummaryOverride: {
      offline: undefined as unknown as number,
    },
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Observability", "Dashboards");

  const panel = page.locator(".observabilityDashboardsPanel");
  await expect(
    panel.getByLabel("Fleet operations dashboard widgets"),
  ).toContainText("0 offline");
  await expect(panel).not.toContainText("undefined");
  await expect(panel).not.toContainText("1 records");
});

test("audit events stays read-only with filters and event detail", async ({
  page,
}, testInfo) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Events");

  await expect(
    page.getByRole("heading", { level: 1, name: "Audit events" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Audit log" }),
  ).toBeVisible();
  const filters = page.getByLabel("Audit event filters");
  for (const label of [
    "Audit actor filter",
    "Audit action filter",
    "Audit resource filter",
    "Audit result filter",
    "Audit IP filter",
    "Audit session filter",
    "Audit privilege scope filter",
    "Audit from date",
    "Audit to date",
  ]) {
    await expect(filters.getByLabel(label)).toBeVisible();
  }

  await filters.getByLabel("Audit action filter").fill("privilege.unlock");
  const grid = page.getByLabel("Audit records data grid");
  await expect(page.getByLabel("Audit event summary")).toContainText(
    "Latest visible",
  );
  await expect(page.getByLabel("Audit event summary")).toContainText(
    "Known actors",
  );
  for (const header of [
    "Time",
    "Actor",
    "Action",
    "Target",
    "Outcome",
    "Evidence",
  ]) {
    await expect(grid).toContainText(header);
  }
  await expect(grid).toContainText("Privilege vault");
  if (testInfo.project.name.includes("mobile")) {
    const mobileCard = grid.getByLabel(
      "Audit records mobile card audit-privilege-unlock-0001",
    );
    await expect(mobileCard.locator(".gridMobileState")).toContainText(
      "Succeeded",
    );
    await expect(mobileCard.locator(".gridMobileActions")).toHaveCount(0);
    await activate(mobileCard);
  } else {
    await expect(grid).toContainText("Privilege unlock");
    await grid.getByText("Privilege unlock").click();
  }

  const detail = page.getByLabel("Audit event detail");
  await expect(detail).toBeVisible();
  await expect(detail).toContainText("console-admin");
  await expect(detail).toContainText("127.0.0.1");
  await expect(detail).toContainText("privilege.unlock");
  await expect(detail).toContainText("Succeeded");
  await expect(detail).toContainText("Exact time");
  await expect(detail).toContainText(/GMT|UTC/);
  await expect(detail).toContainText("Raw action");
  await expect(detail).toContainText("privilege.unlock");
  await detail.getByRole("button", { name: "Open event" }).click();
  await expect(page).toHaveURL(
    /#\/audit\/events\/audit-privilege-unlock-0001$/,
  );
  await expect(
    page.getByRole("heading", { level: 2, name: "Audit event" }),
  ).toBeVisible();
  await expect(
    page.locator('[aria-label="Audit event detail"]:visible'),
  ).toContainText("audit-privilege-unlock-0001");
  await page.goBack();
  await expect(page).toHaveURL(/#\/audit\/events$/);
  await expect(detail).toBeVisible();
  await expect(filters.getByLabel("Audit action filter")).toHaveValue(
    "privilege.unlock",
  );
  await detail.getByRole("button", { name: "Open event" }).click();
  await page.getByRole("button", { name: "Audit events" }).click();
  await expect(page).toHaveURL(/#\/audit\/events$/);
  await expect(filters.getByLabel("Audit action filter")).toHaveValue("");
  await page.goBack();
  await expect(page).toHaveURL(
    /#\/audit\/events\/audit-privilege-unlock-0001$/,
  );
  await page.goBack();
  await expect(page).toHaveURL(/#\/audit\/events$/);
  await expect(filters.getByLabel("Audit action filter")).toHaveValue(
    "privilege.unlock",
  );

  const eventsPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Audit log" }),
  });
  for (const name of [
    "Save retention policy",
    "Preview prune",
    "Review prune apply",
    "Apply prune",
    "Export history",
    "Delete",
    "Create",
    "Revoke",
    "Unlock",
    "Dispatch",
  ]) {
    await expect(
      eventsPanel.getByRole("button", { exact: true, name }),
    ).toHaveCount(0);
  }
});

test("audit event route loads one exact ID outside the list page", async ({
  page,
}) => {
  const auditId = "abababab-abab-4bab-8bab-abababababab";
  await installConsoleApiMock(page, {
    auditDetailOverride: {
      action: "job.target_result",
      actor_id: null,
      command_hash: null,
      created_at: "2026-07-31T08:00:00Z",
      id: auditId,
      metadata: {
        client_id: "agent-sfo-01",
        component: "job-dispatcher",
        job_id: "8b021452-b292-4eae-8735-8474b3c7faab",
        origin_kind: "control_plane",
        result: "skipped",
      },
      target: "client:agent-sfo-01",
    },
    auditLogsOverride: [],
  });
  await page.goto(`/#/audit/events/${auditId}`);
  await waitForConsoleShell(page);

  const detail = page.getByLabel("Audit event detail");
  await expect(detail).toContainText(auditId);
  await expect(detail).toContainText("Job target result");
  const requests = await page.evaluate(() => {
    const trackedWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
    };
    return trackedWindow.__vpsmanFetchRequests ?? [];
  });
  expect(
    requests.some(
      (request) =>
        request.method === "GET" &&
        new URL(request.url, "http://localhost").pathname ===
          `/api/v1/audit/${auditId}`,
    ),
  ).toBe(true);
});

test("failed audit event lookup stays scoped to its detail page", async ({
  page,
}) => {
  const missingAuditId = "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd";
  await installConsoleApiMock(page, { auditLogsOverride: [] });
  await page.goto(`/#/audit/events/${missingAuditId}`);
  await waitForConsoleShell(page);

  const detailPanel = page.locator(".auditEventRoutePanel");
  await expect(detailPanel).toContainText("Audit event is unavailable");
  await expect(detailPanel).toContainText("Audit event not found");
  await detailPanel.getByRole("button", { name: "Audit events" }).click();

  const listPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Audit log" }),
  });
  await expect(listPanel).toBeVisible();
  await expect(listPanel).not.toContainText("Audit event not found");
});

test("audit latest visible event uses newest timestamp instead of row order", async ({
  page,
}) => {
  const olderCreatedAt = "2026-06-02T09:00:00Z";
  const newerCreatedAt = "2026-06-02T11:30:00Z";
  await installConsoleApiMock(page, {
    auditLogsOverride: [
      {
        action: "operator_auth.login_success",
        actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
        command_hash: null,
        created_at: olderCreatedAt,
        id: "audit-unsorted-older-0001",
        metadata: {
          component: "operator-auth",
          operator_username: "console-admin",
          origin_kind: "authentication",
          result: "success",
        },
        target: "auth:login",
      },
      {
        action: "job.dispatch_requested",
        actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
        command_hash: "8".repeat(64),
        created_at: newerCreatedAt,
        id: "audit-unsorted-newer-0001",
        metadata: {
          command_type: "shell_argv",
          component: "job-submission-controller",
          operator_username: "console-admin",
          origin_kind: "operator_request",
          result: "accepted",
          target_count: 1,
        },
        target: "api:/api/v1/jobs",
      },
    ],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Events");

  const latestTime = await page.evaluate(
    (createdAt) =>
      new Date(createdAt).toLocaleString(undefined, {
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
        month: "numeric",
        second: "2-digit",
        timeZoneName: "short",
        year: "numeric",
      }),
    newerCreatedAt,
  );
  const olderTime = await page.evaluate(
    (createdAt) =>
      new Date(createdAt).toLocaleString(undefined, {
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
        month: "numeric",
        second: "2-digit",
        timeZoneName: "short",
        year: "numeric",
      }),
    olderCreatedAt,
  );
  const auditSummary = page.getByLabel("Audit event summary");
  await expect(auditSummary).toContainText(latestTime);
  await expect(auditSummary).not.toContainText(olderTime);
});

test("audit identifies control-plane events without inventing an unknown actor", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page, {
    auditLogsOverride: [
      {
        action: "job.target_result",
        actor_id: null,
        command_hash: "9".repeat(64),
        created_at: "2026-06-02T11:30:00Z",
        id: "audit-control-plane-0001",
        metadata: {
          component: "job-dispatcher",
          job_id: "job-control-plane-0001",
          origin_kind: "control_plane",
          result: "succeeded",
        },
        target: "job:job-control-plane-0001",
      },
    ],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Events");

  const grid = page.getByLabel("Audit records data grid");
  await expect(grid).toContainText("Control plane");
  await expect(grid).toContainText("Job dispatcher");
  await expect(grid).not.toContainText("Unknown actor");
  if (testInfo.project.name.includes("mobile")) {
    await grid
      .getByLabel("Audit records mobile card audit-control-plane-0001")
      .click();
  } else {
    await grid.getByText("Job target result").click();
  }

  const detail = page.getByLabel("Audit event detail");
  await expect(detail).toContainText("Control plane");
  await expect(detail).not.toContainText("unknown");
});

test("audit does not coerce malformed metadata into provenance or evidence", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page, {
    auditLogsOverride: [
      {
        action: "job.dispatch_requested",
        actor_id: "173d16db-ca37-4385-9190-5b0bed72bd4e",
        command_hash: null,
        created_at: "2026-07-31T08:00:00Z",
        id: "audit-malformed-scalars-0001",
        metadata: {
          client_id: 789,
          component: 41,
          job_id: 456,
          operator_session_id: 123,
          operator_username: false,
          origin_kind: true,
          result: 7,
          target_count: 2,
        },
        target: "api:/api/v1/jobs",
      },
      {
        action: "job.target_result",
        actor_id: null,
        command_hash: null,
        created_at: "2026-07-31T07:59:00Z",
        id: "audit-literal-null-0001",
        metadata: {
          component: "null",
          origin_kind: "null",
          result: "null",
        },
        target: "job:unlinked",
      },
    ],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Events");

  const grid = page.getByLabel("Audit records data grid");
  await expect(grid).toContainText("Outcome not recorded");
  await expect(grid).toContainText("2 targets");
  await expect(grid).toContainText("Null");

  if (testInfo.project.name.includes("mobile")) {
    await grid
      .getByLabel("Audit records mobile card audit-malformed-scalars-0001")
      .click();
  } else {
    await grid.getByText("Job dispatch requested").click();
  }

  const detail = page.getByLabel("Audit event detail");
  await expect(detail).toContainText("Outcome not recorded");
  await expect(detail).toContainText("Origin not recorded");
  await expect(detail).toContainText("Operator session not recorded");
  await expect(detail.getByRole("button", { name: "Open job" })).toHaveCount(0);
});

test("audit presents operator and control-plane provenance with one explicit model", async ({
  page,
}, testInfo) => {
  const operatorId = "173d16db-ca37-4385-9190-5b0bed72bd4e";
  const operatorSessionId = "eaa8fae8-6ad6-4284-bb23-f341e6173153";
  const jobId = "8b021452-b292-4eae-8735-8474b3c7faab";
  await installConsoleApiMock(page, {
    auditLogsOverride: [
      {
        action: "fleet.vps_rules_updated",
        actor_id: operatorId,
        command_hash: "a".repeat(64),
        created_at: "2026-07-31T07:45:46Z",
        id: "audit-vps-rules-provenance",
        metadata: {
          changed_row_count: 2,
          matched_vps_count: 3,
          component: "vps-rules-controller",
          operator_role: "admin",
          operator_session_id: operatorSessionId,
          operator_username: "console-admin",
          origin_kind: "operator_request",
          preview_hash: "a".repeat(64),
          result: "succeeded",
        },
        target: "vps_rules",
      },
      {
        action: "operator_auth.login_success",
        actor_id: operatorId,
        command_hash: null,
        created_at: "2026-07-31T08:51:32Z",
        id: "audit-login-provenance",
        metadata: {
          remote_ip: "2001:db8::40",
          result: "success",
          component: "operator-auth",
          operator_session_id: operatorSessionId,
          operator_username: "console-admin",
          origin_kind: "authentication",
          attempted_username: "console-admin",
          user_agent: "operator-browser",
        },
        target: `operator:${operatorId}`,
      },
      {
        action: "job.target_result",
        actor_id: null,
        command_hash: "b".repeat(64),
        created_at: "2026-07-31T07:36:01Z",
        id: "audit-target-result-provenance",
        metadata: {
          accepted: false,
          component: "job-dispatcher",
          job_id: jobId,
          message: "target has never connected",
          origin_kind: "control_plane",
          result: "skipped",
          status: "skipped",
        },
        target: "client:1",
      },
      {
        action: "schedule.due_materialized",
        actor_id: operatorId,
        command_hash: "c".repeat(64),
        created_at: "2026-07-31T07:35:00Z",
        id: "audit-schedule-worker-provenance",
        metadata: {
          component: "schedule-dispatch-worker",
          job_id: jobId,
          operator_id: operatorId,
          operator_role: "admin",
          operator_username: "console-admin",
          origin_kind: "worker",
          result: "requested",
          schedule_id: "51515151-6161-4717-8abc-defdefdefdef",
          schedule_name: "Nightly review",
        },
        target: "schedule:51515151-6161-4717-8abc-defdefdefdef",
      },
    ],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Events");

  const grid = page.getByLabel("Audit records data grid");
  await expect(grid).toContainText("Fleet VPS rules updated");
  await expect(grid).toContainText("console-admin");
  await expect(grid).toContainText("Operator session eaa8fae8");
  await expect(grid).toContainText("Operator login succeeded");
  await expect(grid).toContainText("Job 8b021452");
  await expect(grid).toContainText("Skipped");
  await expect(grid).not.toContainText(operatorId);
  await expect(grid).not.toContainText("Recorded");
  await expect(grid).not.toContainText("No linked evidence");

  if (testInfo.project.name.includes("mobile")) {
    await grid
      .getByLabel("Audit records mobile card audit-target-result-provenance")
      .click();
  } else {
    await grid.getByText("Job target result").click();
  }
  let detail = page.getByLabel("Audit event detail");
  await expect(detail).toContainText("Control plane · Job dispatcher");
  await expect(detail).toContainText("Operator session not recorded");
  await expect(detail.getByRole("button", { name: "Open job" })).toBeVisible();

  if (testInfo.project.name.includes("mobile")) {
    await grid
      .getByLabel("Audit records mobile card audit-schedule-worker-provenance")
      .click();
  } else {
    await grid.getByText("Schedule due materialized").click();
  }
  detail = page.getByLabel("Audit event detail");
  await expect(detail).toContainText("console-admin · Admin");
  await expect(detail).toContainText("Worker · Schedule dispatch worker");
  await expect(detail).toContainText("Not recorded for this event");
  await expect(detail).toContainText("Schedule 51515151");
  await expect(detail.getByRole("button", { name: "Open job" })).toBeVisible();
  await expect(
    detail.getByRole("button", { name: "Open schedule" }),
  ).toHaveCount(0);
});

test("audit job evidence proves who ran what without leaving Audit", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Job evidence");

  await expect(
    page.getByRole("heading", { level: 1, name: "Job evidence" }),
  ).toBeVisible();
  const panel = page.locator(".auditJobEvidencePanel");
  await expect(
    panel.getByRole("heading", { level: 2, name: "Job audit evidence" }),
  ).toBeVisible();
  await expect(panel.getByLabel("Job evidence summary")).toContainText(
    "Jobs in ledger",
  );
  await expect(panel.getByLabel("Job evidence summary")).toContainText(
    "Jobs with audit rows",
  );
  await expect(panel.getByLabel("Job evidence summary")).toContainText(
    "Audit gaps",
  );

  const grid = panel.getByLabel("Job evidence ledger data grid");
  await expect(grid).toContainText("shell argv");
  await expect(grid).toContainText("system scheduler");
  await expect(grid).toContainText("matched");
  await expect(grid).toContainText("Audit event missing");
  await expect(grid).toContainText("Not loaded");
  await expect(grid).toContainText("network speed test");

  await selectEvidenceGridRecord(grid, "agent update");
  let detail = panel.getByLabel("Selected job evidence detail");
  await expect(detail).toContainText("Audit event missing");
  await expect(detail).toContainText("Output unavailable");
  await expect(
    detail.getByLabel("Audit context for selected job"),
  ).toContainText("Audit event missing");
  await expect(detail.getByLabel("Job outputs for selected job")).toContainText(
    "No output artifact or inline output row was returned",
  );

  await selectEvidenceGridRecord(grid, "network speed test");

  detail = panel.getByLabel("Selected job evidence detail");
  await expect(detail).toContainText("console-admin");
  await expect(detail).toContainText("privileged command");
  await expect(detail).toContainText("1 matched");
  await expect(detail).toContainText("Inline output");
  await expect(detail).toContainText("no approval record exposed");
  await expect(
    detail.getByLabel("Audit context for selected job"),
  ).toContainText("Job dispatch requested");
  await expect(detail.getByLabel("Job targets for selected job")).toContainText(
    "edge-sfo-01",
  );
  await expect(detail.getByLabel("Job outputs for selected job")).toContainText(
    "tcp_throughput",
  );

  for (const name of [
    "Dispatch",
    "Apply",
    "Delete",
    "Create",
    "Revoke",
    "Unlock",
    "Approve",
    "Reject",
    "Run",
  ]) {
    await expect(panel.getByRole("button", { exact: true, name })).toHaveCount(
      0,
    );
  }
});

test("audit sessions correlates terminal and auth evidence without emulator controls", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Sessions");

  await expect(
    page.getByRole("heading", { level: 1, name: "Session evidence" }),
  ).toBeVisible();
  const panel = page.locator(".auditSessionEvidencePanel");
  await expect(
    panel.getByRole("heading", { level: 2, name: "Session evidence" }),
  ).toBeVisible();
  await expect(
    panel.getByRole("button", { name: "Sign out", exact: true }),
  ).toBeVisible();
  await expect(panel.getByLabel("Session evidence summary")).toContainText(
    "Terminal sessions",
  );
  await expect(panel.getByLabel("Session evidence summary")).toContainText(
    "Audit-linked terminals",
  );
  await expect(panel.getByLabel("Session evidence summary")).toContainText(
    /stale terminal states? hidden from open count/,
  );
  await expect(panel.getByLabel("Session evidence summary")).toContainText(
    "expired bearer sessions",
  );
  await expect(panel.getByLabel("Session evidence summary")).toContainText(
    "Authentication signals",
  );
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Started");
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Last activity");
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Expiry");
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("console-admin");
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("edge-sfo-01");
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Stale state");
  await expect(
    panel.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Replayable transcript");

  const terminalGrid = panel.getByLabel("Terminal session evidence data grid");
  await expect(
    terminalGrid.getByLabel("Selected terminal session evidence"),
  ).toHaveCount(0);
  const terminalRecords = terminalGrid
    .locator(".gridBody [role=row], .gridMobileCard")
    .filter({ hasText: "Stale state" })
    .filter({ hasText: "Replayable transcript" });
  await expect(terminalRecords).toHaveCount(1);
  const terminalRecord = terminalRecords.first();
  await expect(terminalRecord).toHaveAttribute("aria-expanded", "false");
  await terminalRecord.click();
  await expect(terminalRecord).toHaveAttribute("aria-expanded", "true");
  const detail = terminalGrid
    .locator(".gridExpandedRow")
    .getByLabel("Selected terminal session evidence");
  await expect(detail).toContainText("61616161");
  await expect(detail).toContainText("Started");
  await expect(detail).toContainText("Last activity");
  await expect(detail).toContainText("Expiry");
  await expect(detail).toContainText("Terminal opened");
  await expect(detail).toContainText("Terminal input");
  await expect(
    detail.getByLabel("Transcript references for selected session"),
  ).toContainText("Advanced replay path");
  await expect(
    detail.getByLabel("Operator auth evidence for selected session"),
  ).toContainText("127.0.0.1");
  await expect(
    detail.getByLabel("Operator auth evidence for selected session"),
  ).toContainText("Playwright");
  await expect(
    detail.getByLabel("Operator auth evidence for selected session"),
  ).toContainText("local test");
  await expect(
    detail.getByLabel("Operator auth evidence for selected session"),
  ).toContainText("test automation");
  await expect(panel.getByLabel("Operator session evidence")).toContainText(
    "bearer sessions",
  );
  await expect(panel.getByLabel("Operator session evidence")).toContainText(
    "select active non-current sessions to revoke",
  );
  await expect(panel.getByLabel("Operator session evidence")).toContainText(
    "Expired",
  );
  await expect(panel.getByLabel("Operator session evidence")).toContainText(
    "Demo/test",
  );

  await expect(panel.getByLabel("Active terminal emulator")).toHaveCount(0);
  for (const name of [
    "Prepare terminal review",
    "Input",
    "Replay",
    "Revoke session",
    "Revoke selected",
    "Dispatch",
    "Create",
    "Delete",
  ]) {
    await expect(panel.getByRole("button", { exact: true, name })).toHaveCount(
      0,
    );
  }

  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await page
    .locator(".terminalSessionsPanel")
    .getByRole("button", { name: "Evidence" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Session evidence" }),
  ).toBeVisible();
  await expect(page.locator(".auditSessionEvidencePanel")).toBeVisible();
});

test("audit retention explains export scope and prune impact separately from maintenance cleanup", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Audit", "Retention & export");

  await expect(
    page.getByRole("heading", { level: 1, name: "Retention & export" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "History retention" }),
  ).toBeVisible();
  await expect(
    page.getByText(/Missing fine detail is not fabricated/),
  ).toBeVisible();
  await expect(
    page.getByText(/Retention days is the final history horizon/),
  ).toBeVisible();

  const summary = page.getByLabel("History retention summary");
  await expect(summary).toContainText("Policy domains");
  await expect(summary).toContainText("Selected domain");
  await expect(summary).toContainText("Cleanup review");

  const policies = page.getByLabel("History retention policy table");
  await expect(policies).toContainText("Domain");
  await expect(policies).toContainText("Retention days");
  await expect(policies).toContainText("Final horizon");
  await expect(policies).toContainText("Metadata only");
  await expect(policies).toContainText("Export enabled");
  await expect(policies).toContainText("Audit logs");

  const editor = page.getByLabel("Selected retention domain editor");
  await expect(editor).toContainText("Audit logs");
  await expect(
    editor.getByRole("button", { name: "Save policy" }),
  ).toBeVisible();

  const cleanup = page.getByLabel("History retention cleanup workflow");
  await expect(cleanup).toContainText("Evidence retention only");
  await expect(cleanup).toContainText("System / Maintenance");
  await expect(cleanup).toContainText("Preview required");

  const scope = page.getByLabel("History retention export scope");
  await expect(scope).toContainText("Export scope");
  await expect(scope).toContainText("Audit logs");
  await expect(scope).toContainText("All retained records");

  await expect(
    page.getByRole("button", { name: "Export history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Preview cleanup" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Delete reviewed rows" }),
  ).toBeDisabled();
  await expect(page.getByRole("button", { name: "Queue cleanup" })).toHaveCount(
    0,
  );

  await page.getByRole("button", { name: "Preview cleanup" }).click();
  await expect(summary).toContainText("0 matched rows / 0 objects");
  await expect(cleanup).toContainText("Would delete 0 metadata rows");
  const deleteReviewedRows = page.getByRole("button", {
    name: "Delete reviewed rows",
  });
  await expect(deleteReviewedRows).toBeDisabled();
  await expect(deleteReviewedRows).toHaveAttribute(
    "title",
    "No reviewed rows match; deletion is not needed",
  );
  await expect(page.getByLabel("Confirm history prune")).toHaveCount(0);
});

test("access overview routes to release authority pages", async ({ page }) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Access", "Overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "Access overview" }),
  ).toBeVisible();
  const actions = page.getByLabel("Access actions required");
  await expect(actions).toContainText("Policy recommends MFA");
  await expect(actions).toContainText("Recommended");
  await expect(actions).toContainText("Expired bearer sessions");
  await expect(actions).not.toContainText("No saved local vault");

  const responsibilities = page.getByLabel("Access overview responsibilities");
  await expect(responsibilities).toContainText("Operators");
  await expect(responsibilities).toContainText("VPS identities");
  await expect(responsibilities).toContainText(
    "Bearer sessions are listed under Session scopes.",
  );
  await expect(responsibilities).not.toContainText("current session Expired");

  const sessionScopes = page.getByLabel("Access session scopes");
  await expect(sessionScopes).toContainText("Console/browser session");
  await expect(sessionScopes).toContainText("API bearer sessions");
  await expect(sessionScopes).toContainText("current bearer record Expired");
  await expect(sessionScopes).toContainText("Privilege unlock");
  await expect(sessionScopes).toContainText("Terminal sessions");
  await expect(sessionScopes).toContainText("Gateway sessions");
  await expect(sessionScopes).not.toContainText("current session Expired");

  await responsibilities
    .getByRole("button", { name: "Open Operators" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Operators" }),
  ).toBeVisible();
  await expect(page.getByLabel("Operator governance overview")).toBeVisible();

  await openConsoleSubpage(page, "Access", "Overview");
  await page
    .getByLabel("Access overview responsibilities")
    .getByRole("button", { name: "Open identities" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "VPS identities" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "VPS identities" }),
  ).toBeVisible();
  await expect(page.getByText("VPS keys")).toHaveCount(0);
  await expect(page.getByLabel("Access posture overview")).toHaveCount(0);
  await expect(page.getByLabel("Agent identity lifecycle")).toHaveCount(0);
  const identityGrid = page.getByLabel("VPS identities data grid");
  await expect(identityGrid).toContainText("Register VPS");
  await identityGrid.getByRole("button", { name: "Register VPS" }).click();
  await expect(page.locator(".accessInspector")).toContainText("Register VPS");
  await page
    .getByRole("button", { name: "Close VPS identity workflow" })
    .click();
  await expect(page.locator(".accessInspector")).toBeHidden();

  await openConsoleSubpage(page, "Access", "Overview");
  await page
    .getByLabel("Access session scopes")
    .getByRole("button", { name: "Open sessions" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Gateway sessions" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Gateway sessions" }),
  ).toBeVisible();
  const emptyState = page.getByLabel("Gateway sessions empty state");
  await expect(emptyState).toContainText(
    "No active gateway sessions. Configure the gateway endpoint and server key.",
  );
  await expect(page.getByLabel("Gateway installer defaults")).toBeVisible();
  await expect(
    emptyState.getByRole("button", { name: "Gateway settings" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Copy transcript" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Download transcript" }),
  ).toHaveCount(0);

  await openConsoleSubpage(page, "Access", "Overview");
  await page
    .getByLabel("Access session scopes")
    .getByRole("button", { name: "Unlock" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Privilege vault" }),
  ).toBeVisible();
  const vaultPanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  });
  await expect(vaultPanel).toContainText("request-bound assertions");
});

test("access privilege vault is the locked handoff for privileged workflows", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  const lockedWorkflows: PrivilegeHandoffSpec[] = [
    {
      heading: "Command dispatch",
      subpage: "Dispatch",
      view: "Jobs",
    },
    {
      heading: "Files",
      subpage: "Files",
      view: "Remote Operations",
    },
    {
      heading: "Bulk files",
      subpage: "Bulk files",
      view: "Remote Operations",
    },
    {
      heading: "Schedules",
      subpage: "Schedules",
      view: "Automation",
      evidence: "apply now, target updates, enable, disable, and delete",
    },
    {
      heading: "Bulk groups",
      subpage: "Bulk groups",
      view: "Fleet",
      evidence: /Bulk group mutation/i,
    },
    {
      heading: "Network tests",
      prepare: async (routePage) => {
        await expect(
          routePage.getByText("Loading network workspace"),
        ).toHaveCount(0, { timeout: 15000 });
      },
      subpage: "Tests",
      view: "Network",
    },
    {
      heading: "Restore",
      prepare: async (routePage) => {
        await activate(
          routePage.getByRole("button", {
            name: "Choose restore artifact",
            exact: true,
          }),
        );
        await expect(
          routePage.getByRole("complementary", {
            name: "Choose restore artifact",
          }),
        ).toBeVisible();
      },
      root: (routePage) =>
        routePage.getByRole("complementary", {
          name: "Choose restore artifact",
        }),
      subpage: "Restore",
      view: "Backups",
    },
    {
      heading: "Suite config",
      prepare: async (routePage) => {
        await routePage
          .getByLabel("Suite config sections")
          .getByRole("button", { name: /Capacity/ })
          .click();
        await routePage.getByLabel("API DB pool").fill("40");
        await expect(
          routePage.getByLabel("Suite config impact summary"),
        ).toContainText("Draft impact");
        await expect(
          routePage.getByLabel("Suite config validation and save review"),
        ).toContainText("Next: unlock privilege");
      },
      root: (routePage) =>
        routePage.getByLabel("Suite config validation and save review"),
      subpage: "Suite config",
      view: "System",
    },
  ];

  for (const workflow of lockedWorkflows) {
    await expectLockedWorkflowPrivilegeHandoff(page, workflow);
  }
});

test("access operators are separate from vps identities and system navigation", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    await expect(mobilePageSelector).not.toContainText("System / Users");
    await expect(mobilePageSelector).toContainText("Access / Operators");
    await expect(mobilePageSelector).toContainText("Access / VPS identities");
  } else {
    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await activate(
      nav.getByRole("button", { name: "System", exact: true }).first(),
    );
    const systemSections = nav.getByLabel("System sections");
    await expect(
      systemSections.getByRole("button", { name: "Users", exact: true }),
    ).toHaveCount(0);
    await expect(
      systemSections.getByRole("button", { name: "Operators", exact: true }),
    ).toHaveCount(0);

    await activate(
      nav.getByRole("button", { name: "Access", exact: true }).first(),
    );
    const accessSections = nav.getByLabel("Access sections");
    await expect(
      accessSections.getByRole("button", { name: "Operators", exact: true }),
    ).toBeVisible();
    await expect(
      accessSections.getByRole("button", {
        name: "VPS identities",
        exact: true,
      }),
    ).toBeVisible();
  }

  await openConsoleSubpage(page, "Access", "Operators");
  await expect(
    page.getByRole("heading", { level: 1, name: "Operators" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operator accounts" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Access", "VPS identities");
  await expect(
    page.getByRole("heading", { level: 1, name: "VPS identities" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "VPS identities" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operator accounts" }),
  ).toHaveCount(0);
});

test("backups overview explains recoverability and links backup workflows", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "Backup overview" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Backup overview" }),
  ).toBeVisible();

  const decision = page.getByLabel("Backup recovery decision");
  await expect(decision).toContainText("Recoverability decision");
  await expect(decision).toContainText("No recent backups");
  await expect(decision).toContainText("Recent");
  await expect(decision).toContainText("Overdue");
  await expect(decision).toContainText("Unknown");
  await expect(decision).toContainText("Artifacts");
  await expect(decision).toContainText("Restore tests");

  const primaryActions = page.getByLabel("Backup overview primary actions");
  await expect(primaryActions).toContainText("3 VPSs need backup evidence");
  await expect(primaryActions).toContainText("Back up now");
  await expect(primaryActions).toContainText("Create policy");
  await expect(primaryActions).toContainText("Restore");

  const supportingRecords = page.getByLabel(
    "Backup overview supporting records",
  );
  await expect(supportingRecords).toContainText("Supporting records");
  await expect(supportingRecords).toContainText("Migration");
  await expect(supportingRecords).toContainText("not used");

  const postureDetails = page.locator(".backupPostureDisclosure");
  await expect(postureDetails).toContainText("Detailed posture");
  await postureDetails.locator("summary").click();
  const posture = page.getByLabel("Backup posture overview");
  await expect(posture).toContainText("Recent backups");
  await expect(posture).toContainText("0/3");
  await expect(posture).toContainText("Overdue");
  await expect(posture).toContainText("Unknown");
  await expect(posture).toContainText("1");
  await expect(posture).toContainText("Failed requests");
  await expect(posture).toContainText("Artifact storage");
  await expect(posture).toContainText("Restore test");
  await expect(posture).toContainText("Retention/security");

  const evidence = page.getByLabel("Backup overview evidence summary");
  await expect(evidence).toContainText("Latest backup");
  await expect(evidence).toContainText("Package linked");
  await expect(evidence).toContainText("Artifact states");
  await expect(evidence).toContainText("1 available");
  await expect(evidence).toContainText("Restore verification");
  await expect(evidence).toContainText("No restore plan");
  await expect(evidence).toContainText("Run a restore rehearsal");
  await expect(evidence).toContainText("Migration readiness");
  await expect(evidence).toContainText("Not used");

  const links = page.getByLabel("Backup overview supporting records");
  for (const label of [
    "Requests",
    "Policies",
    "Artifacts",
    "Restore",
    "Migration",
  ]) {
    await expect(
      links.getByRole("button", { name: new RegExp(label) }),
    ).toBeVisible();
  }

  await links.getByRole("button", { name: /Requests/ }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup requests" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page
    .getByLabel("Backup overview supporting records")
    .getByRole("button", { name: /Policies/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup policies" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page
    .getByLabel("Backup overview supporting records")
    .getByRole("button", { name: /Artifacts/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup artifacts" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page
    .getByLabel("Backup overview supporting records")
    .getByRole("button", { name: /Restore/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Restore" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page
    .getByLabel("Backup overview supporting records")
    .getByRole("button", { name: /Migration/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Migration" }),
  ).toBeVisible();
});

test("backups artifacts keep transfer packages separate from job cleanup", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup artifact package controls are covered through the desktop drawer workflow",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Artifacts");

  await expect(
    page.getByRole("heading", { level: 1, name: "Backup artifacts" }),
  ).toBeVisible();
  const guide = page.getByLabel("Backup artifact inventory summary");
  await expect(guide).toContainText("Artifact inventory actions");
  await expect(guide).toContainText("Artifact inventory");
  await expect(guide).toContainText("Backup linkage");
  await expect(guide).toContainText("Upload package");
  await expect(guide).toContainText("Transfer package");
  await expect(guide).toContainText("Retained job output");
  await expect(guide).toContainText("Lineage details");

  const records = page.locator(".fleetPanel");
  await expect(records).toContainText("Artifact inventory records");
  await expect(records).toContainText("Available package");
  await expect(records).toContainText("Linked");
  const artifactGrid = page.getByLabel("Artifact inventory records data grid");
  const artifactRow = artifactGrid
    .locator(".gridBody [role=row]", { hasText: "Available package" })
    .first();
  await artifactRow.getByLabel(/Select Artifact inventory records row/).check();
  await artifactGrid.getByRole("button", { name: "Actions" }).click();
  const restoreArtifactAction = page.getByRole("menuitem", {
    name: "Restore",
    exact: true,
  });
  await expect(restoreArtifactAction).not.toHaveAttribute("data-disabled", "");
  await expect(restoreArtifactAction).toHaveAttribute(
    "title",
    /verified package source/,
  );
  const downloadArtifactAction = page.getByRole("menuitem", {
    name: "Download",
    exact: true,
  });
  await expect(downloadArtifactAction).not.toHaveAttribute("data-disabled", "");
  await expect(downloadArtifactAction).toHaveAttribute(
    "title",
    /verified backup artifact package/,
  );
  await expect(records).not.toContainText("Artifact cleanup");
  await expect(records).not.toContainText("Queue cleanup");
  await expect(records).not.toContainText("Type DELETE");

  await activate(restoreArtifactAction);
  await expect(
    page.getByRole("heading", { level: 1, name: "Restore" }),
  ).toBeVisible();
  const restoreDrawer = page.getByRole("complementary", {
    name: "Choose restore artifact",
  });
  await expect(restoreDrawer).toBeVisible();
  await expect(
    restoreDrawer.getByLabel("Restore source backup request"),
  ).toHaveValue(backupId);

  await openConsoleSubpage(page, "Backups", "Artifacts");
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup artifacts" }),
  ).toBeVisible();

  await activate(
    page.getByRole("button", { name: "Open artifact workflow", exact: true }),
  );
  const drawer = page.getByRole("complementary", {
    name: "Open artifact workflow",
  });
  await expect(
    drawer.getByRole("heading", { name: "Upload artifact" }),
  ).toBeVisible();
  await expect(drawer.getByLabel("Artifact backup request")).toBeVisible();
  await expect(
    drawer.getByLabel("Backup artifact transfer package source job ID"),
  ).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Review upload" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Review transfer package" }),
  ).toBeVisible();
  await expect(drawer).not.toContainText("Artifact cleanup");
  await expect(drawer).not.toContainText("Queue cleanup");

  await activate(
    page.getByRole("button", { name: "Close Open artifact workflow" }),
  );
  await guide
    .getByRole("button", { name: "Open Jobs artifacts inventory" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Job artifacts" }),
  ).toBeVisible();
  await expect(page.getByLabel("Job artifact inventory summary")).toContainText(
    "System / Maintenance",
  );
  await expect(page.getByRole("button", { name: "Queue cleanup" })).toHaveCount(
    0,
  );
});

test("backups requests keep request review separate from policy and restore work", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup request role separation is covered through the desktop drawer workflow",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Requests");

  await expect(
    page.getByRole("heading", { level: 1, name: "Backup requests" }),
  ).toBeVisible();
  const records = page.locator(".fleetPanel");
  const requestSummary = page.getByLabel("Backup request summary");
  await expect(requestSummary).toContainText("recent");
  await expect(requestSummary).toContainText("need evidence");
  await expect(requestSummary).toContainText("failed");
  await expect(requestSummary).toContainText("artifact-backed");
  await expect(records).toContainText("Backup request records");
  for (const label of [
    "VPS",
    "Paths",
    "State",
    "Size",
    "Requested",
    "Artifact",
  ]) {
    await expect(records).toContainText(label);
  }
  await expect(records).toContainText("Ready");
  await expect(records).toContainText("verified package available");
  await expect(records).toContainText("512 B");
  await expect(records).not.toContainText("Duration");
  const requestGrid = page.getByLabel("Backup request records data grid");
  const readyRequest = requestGrid
    .locator(".gridBody [role=row]", { hasText: "Ready" })
    .first();
  await readyRequest.getByLabel(/Select Backup request records row/).check();
  await requestGrid.getByRole("button", { name: "Actions" }).click();
  await expect(
    page.getByRole("menuitem", { name: "Open artifact" }),
  ).toBeVisible();
  await expect(records).not.toContainText("Backup policy records");
  await expect(records).not.toContainText("Restore plan records");
  await expect(records).not.toContainText(
    "Artifact metadata linked to backup requests",
  );
  await expect(records).not.toContainText("Backup posture");

  await page.getByRole("menuitem", { name: "Open artifact" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup artifacts" }),
  ).toBeVisible();
  await openConsoleSubpage(page, "Backups", "Requests");

  await expect(
    page.getByRole("button", { name: "Run backup", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Create policy", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Choose restore artifact", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Open artifact workflow", exact: true }),
  ).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Run backup", exact: true }));
  const drawer = page.getByRole("complementary", { name: "Run backup" });
  await expect
    .poll(async () => {
      const box = await drawer.boundingBox();
      return box?.y ?? Number.POSITIVE_INFINITY;
    })
    .toBeLessThan(280);
  await expect(
    drawer.getByRole("heading", { name: "Backup scope" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("heading", { name: "Backup policy" }),
  ).toHaveCount(0);
  await expect(
    drawer.getByRole("heading", { name: "Policy prune" }),
  ).toHaveCount(0);
  await expect(
    drawer.getByRole("heading", { name: "Draft restore" }),
  ).toHaveCount(0);
});

test("backup package availability consistently gates posture and recovery actions", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop backup inventory exposes the full restore and download action set",
  );
  await installConsoleApiMock(page, {
    backupArtifactsOverride: [
      {
        client_id: "agent-sfo-01",
        content_available: false,
        created_at: "2026-05-31T10:01:00Z",
        id: "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
        object_key: `backups/agent-sfo-01/${backupId}.tar`,
        sha256_hex: "b".repeat(64),
        size_bytes: 512,
        status: "missing",
      },
    ],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Requests");

  const summary = page.getByLabel("Backup request summary");
  await expect(summary).toContainText("0 recent");
  const requests = page.getByLabel("Backup request records data grid");
  await expect(requests).toContainText("Recorded");
  await expect(requests).toContainText("stored package unavailable");

  await openConsoleSubpage(page, "Backups", "Artifacts");
  const artifacts = page.getByLabel("Artifact inventory records data grid");
  await expect(artifacts).toContainText("Package unavailable");
  const artifactRow = artifacts
    .locator(".gridBody [role=row]", { hasText: "Package unavailable" })
    .first();
  await artifactRow.click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Restore", exact: true }),
  ).toHaveAttribute("data-disabled", "");
  await expect(
    page.getByRole("menuitem", { name: "Download", exact: true }),
  ).toHaveAttribute("data-disabled", "");
});

test("backup request presets keep collection policy compact and explicit", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Requests");
  await activate(page.getByRole("button", { name: "Run backup", exact: true }));
  const drawer = page.getByRole("complementary", { name: "Run backup" });
  const options = drawer.getByLabel("Backup collection options");
  await expect(options).toBeVisible();
  const optionLayout = await options.evaluate((element) => ({
    height: element.getBoundingClientRect().height,
    overflow: element.scrollWidth - element.clientWidth,
  }));
  expect(optionLayout.overflow).toBeLessThanOrEqual(1);
  expect(optionLayout.height).toBeLessThanOrEqual(90);

  await drawer.getByRole("button", { name: "Docker config" }).click();
  await expect(drawer.getByLabel("Backup selected paths")).toHaveValue(
    "/etc/docker",
  );
  await expect(
    drawer.getByRole("checkbox", { name: "Skip missing roots" }),
  ).toBeChecked();
  await drawer.getByRole("button", { name: "Identity" }).click();
  await expect(
    drawer.getByRole("checkbox", { name: "Skip missing roots" }),
  ).not.toBeChecked();
});

test("backup action drawers stay owned by the subpage that opened them", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Requests");
  await activate(page.getByRole("button", { name: "Run backup", exact: true }));
  await expect(
    page.getByRole("complementary", { name: "Run backup" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Policies");
  await expect(
    page.getByRole("complementary", { name: "Create policy" }),
  ).toHaveCount(0);

  await activate(
    page.getByRole("button", { name: "Create policy", exact: true }).first(),
  );
  await expect(
    page.getByRole("complementary", { name: "Create policy" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Artifacts");
  await expect(
    page.getByRole("complementary", { name: "Open artifact workflow" }),
  ).toHaveCount(0);
});

test("backups policies keep authoring separate and review prune preview before apply", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup policy prune review is covered through the desktop drawer workflow",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Policies");

  await expect(
    page.getByRole("heading", { level: 1, name: "Backup policies" }),
  ).toBeVisible();
  const records = page.locator(".fleetPanel");
  const policySummary = page.getByLabel("Backup policy summary");
  await expect(policySummary).toContainText("automatic");
  await expect(policySummary).toContainText("paused");
  await expect(policySummary).toContainText("invalid cadence");
  await expect(policySummary).toContainText("execution failures");
  await expect(records).toContainText("Backup policy records");
  await expect(records).toContainText("Scheduled backup policies");
  await expect(records).toContainText(
    "Enabled policies with a valid cadence run automatically",
  );
  await expect(records).toContainText("No scheduled backups");
  await expect(records).toContainText("Create a policy for automatic backups");
  await expect(records).not.toContainText("Backup request records");
  await expect(records).not.toContainText("Restore plan records");
  await expect(records).not.toContainText("Backup posture");
  await expect(records).not.toContainText("approval-required jobs");
  await expect(records).not.toContainText(
    "Artifact metadata linked to backup requests",
  );

  await expect(
    page.getByRole("button", { name: "Create policy", exact: true }),
  ).toHaveCount(2);
  const policyEmptyState = page.locator(".emptyState", {
    hasText: "No scheduled backups",
  });
  await expect(
    policyEmptyState.getByRole("button", {
      name: "Create policy",
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Run backup", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Choose restore artifact", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Open artifact workflow", exact: true }),
  ).toHaveCount(0);

  await activate(
    policyEmptyState.getByRole("button", {
      name: "Create policy",
      exact: true,
    }),
  );
  const drawer = page.getByLabel("Create policy");
  await expect(
    drawer.getByRole("heading", { name: "Backup policy" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("heading", { name: "Policy prune" }),
  ).toHaveCount(0);
  await expect(drawer.getByLabel("Backup policy name")).toHaveValue("");
  await expect(
    drawer.getByRole("combobox", {
      name: "Backup policy target expression",
    }),
  ).toHaveText("");
  await expect(
    drawer.getByRole("button", { name: "Review policy" }),
  ).toBeDisabled();
  await expect(
    drawer.getByRole("heading", { name: "Backup scope" }),
  ).toHaveCount(0);
  await expect(
    drawer.getByRole("heading", { name: "Draft restore" }),
  ).toHaveCount(0);
  await expect(
    drawer.getByRole("heading", { name: "Artifact upload" }),
  ).toHaveCount(0);

  await activate(drawer.getByRole("button", { name: "Close Create policy" }));
  await activate(
    page.getByRole("button", { name: "Prune policies", exact: true }),
  );
  const pruneDrawer = page.getByLabel("Prune policies");
  await expect(
    page.getByRole("button", { name: "Create policy", exact: true }),
  ).toHaveCount(2);
  await expect(
    page.getByRole("button", { name: "Prune policies", exact: true }),
  ).toHaveCount(1);
  await expect(
    pruneDrawer.getByRole("heading", { name: "Backup policy" }),
  ).toHaveCount(0);
  await expect(
    pruneDrawer.getByRole("heading", { name: "Policy prune" }),
  ).toBeVisible();
  await expect(
    pruneDrawer.getByLabel("Backup policy prune review state"),
  ).toContainText("Preview only");

  await pruneDrawer.getByLabel("Dry run").uncheck();
  await expect(
    pruneDrawer.getByLabel("Backup policy prune review state"),
  ).toContainText("Preview required before apply");
  await activate(
    pruneDrawer.getByRole("button", { name: "Review prune apply" }),
  );
  const confirmation = pruneDrawer.getByLabel("Confirm policy prune apply");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Preview hash");
  await expect(confirmation).toContainText("Reviewed rows");
  await activate(confirmation.getByRole("button", { name: "Apply prune" }));

  const pruneRequests = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          backupPolicyPrunes: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.backupPolicyPrunes;
  });
  expect(pruneRequests).toHaveLength(2);
  expect(pruneRequests[0]).toMatchObject({
    confirmed: false,
    dry_run: true,
    preview_hash: null,
  });
  expect(pruneRequests[1]).toMatchObject({
    confirmed: true,
    dry_run: false,
    preview_hash:
      "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  });
});

test("backups restore starts from artifact readiness, destination, and confirmation", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "restore source selection and drawer reviews are covered through the desktop workflow",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Restore");

  await expect(
    page.getByRole("heading", { level: 1, name: "Restore" }),
  ).toBeVisible();
  const summary = page.getByLabel("Backup restore summary");
  await expect(summary).toContainText("restore-ready package");
  await expect(summary).toContainText("unverified");
  await expect(summary).toContainText("draft restore");

  const records = page.locator(".fleetPanel");
  await expect(records).toContainText("Restore source records");
  for (const label of [
    "Artifact",
    "Readiness",
    "Destination",
    "Path behavior",
    "Draft restore",
  ]) {
    await expect(records).toContainText(label);
  }
  await expect(records).toContainText("Available package");
  await expect(records).toContainText("Choose destination");
  await expect(records).toContainText("1 path");
  await expect(records).not.toContainText("Backup posture");
  await expect(records).not.toContainText("Guided restore workflow");

  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(
    page.getByRole("button", { name: "Choose restore artifact", exact: true }),
  );
  const drawer = page.getByRole("complementary", {
    name: "Choose restore artifact",
  });
  await expect(
    drawer.getByRole("heading", { name: "Draft restore" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("heading", { name: "Confirm restore" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("heading", { name: "Rollback restore" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Review draft restore" }),
  ).toBeVisible();
  await expect(drawer.getByLabel("Dry-run rehearsal")).toBeChecked();
  const dryRunReviewButton = drawer.getByRole("button", {
    name: "Review dry run",
  });
  await expect(dryRunReviewButton).toBeVisible();
  await expect(dryRunReviewButton).not.toHaveClass(/dangerPrimary/);
  await expect(
    drawer.getByRole("button", { name: "Review rollback" }),
  ).toBeVisible();
});

test("backups migration starts from source artifact to replacement mapping", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "migration mapping and drawer are covered through the desktop workflow",
  );
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Backups", "Migration");

  await expect(
    page.getByRole("heading", { level: 1, name: "Migration" }),
  ).toBeVisible();
  const summary = page.getByLabel("Migration relationship summary");
  await expect(summary).toContainText("Source VPS/artifact");
  await expect(summary).toContainText("Choose source VPS/artifact");
  await expect(summary).toContainText("Replacement VPS");
  await expect(summary).toContainText("Choose replacement VPS");
  await expect(summary).toContainText("1 active artifact");
  await expect(summary).toContainText("0 draft restores");
  await expect(summary).toContainText("0 saved mappings");

  const records = page.locator(".fleetPanel");
  await expect(records).toContainText("Migration mappings");
  await expect(records).toContainText("Migration mapping records");
  await expect(records).toContainText("No migration mappings");
  await expect(records).toContainText("source artifact and replacement VPS");
  await expect(records).not.toContainText("Backup posture");
  await expect(records).not.toContainText("Migration cutover checklist");

  await openConsoleSubpage(page, "Backups", "Migration");
  await activate(
    page.getByRole("button", { name: "Create migration mapping", exact: true }),
  );
  const drawer = page.getByRole("complementary", {
    name: "Create migration mapping",
  });
  await expect(
    drawer.getByRole("heading", { name: "Migration mapping", exact: true }),
  ).toBeVisible();
  await expect(drawer).toContainText("Source -> replacement");
  await expect(drawer).toContainText("Source artifact");
  await expect(drawer).toContainText("Privilege");
  await expect(drawer).toContainText("Cutover mode");
  await expect(drawer).toContainText("Service check");
  await expect(drawer).toContainText("Identity policy");
  await expect(drawer.getByLabel("Migration draft restore")).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Review mapping" }),
  ).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Review dry run" }),
  ).toBeVisible();
});

test("network overview links to release network workflows", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Network", "Overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "Network overview" }),
  ).toBeVisible();
  await expect(page.getByLabel("Network posture summary")).toContainText(
    "Plans",
  );
  await expect(page.getByLabel("Network posture summary")).toContainText(
    "Declared observations",
  );
  await expect(page.getByLabel("Network posture summary")).toContainText(
    "Latest evidence",
  );
  await expect(page.getByLabel("Network posture summary")).toContainText(
    "Stale",
  );
  await expect(page.getByRole("button", { name: "Create plan" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Create plan" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Create tunnel plan" }),
  ).toBeVisible();
  const tunnelPlanName = page.getByLabel("Tunnel plan name");
  await expect(tunnelPlanName).toBeFocused();
  await expect(tunnelPlanName).toHaveCSS("border-radius", "6px");
  const tunnelControlHeights = await page
    .locator(
      '.tunnelPlanComposer .topologyFormGrid input:not([type="checkbox"]):not([type="radio"]), .tunnelPlanComposer .topologyFormGrid select',
    )
    .evaluateAll((controls) =>
      controls.map((control) => control.getBoundingClientRect().height),
    );
  expect(tunnelControlHeights.length).toBeGreaterThan(0);
  expect(Math.min(...tunnelControlHeights)).toBeGreaterThanOrEqual(36);

  await openConsoleSubpage(page, "Network", "Overview");
  const links = page.getByLabel("Network overview workflow links");
  for (const label of ["Graph", "Tunnel plans", "Tests", "OSPF", "Evidence"]) {
    await expect(
      links.getByRole("button", { name: new RegExp(`^${label}\\b`) }),
    ).toBeVisible();
  }

  await links.getByRole("button", { name: /^Graph\b/ }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network graph" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page
    .getByLabel("Network overview workflow links")
    .getByRole("button", { name: /^Tunnel plans\b/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page
    .getByLabel("Network overview workflow links")
    .getByRole("button", { name: /^Tests\b/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network tests" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page
    .getByLabel("Network overview workflow links")
    .getByRole("button", { name: /^OSPF\b/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network OSPF" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page
    .getByLabel("Network overview workflow links")
    .getByRole("button", { name: /^Evidence\b/ })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network evidence" }),
  ).toBeVisible();
});

test("network graph stays focused on visual topology inspection", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Network", "Graph");

  await expect(
    page.getByRole("heading", { level: 1, name: "Network graph" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Topology graph" }),
  ).toBeVisible();
  const graphPanel = page.locator(".topologyGraphPanel");
  await expect(graphPanel).toContainText("Last topology evidence");
  await expect(graphPanel).toContainText("stale");
  await expect(page.getByLabel("Topology health filter")).toBeVisible();
  await expect(
    graphPanel.getByRole("button", { name: "Zoom in topology graph" }),
  ).toContainText("Zoom in");
  const graphNode = graphPanel.locator('[aria-label^="Select edge-sfo-01"]');
  if (await graphNode.isVisible()) {
    await graphNode.click({ force: true });
  }
  const nodeInspector = graphPanel.locator(".topologyNodeInspector");
  await expect(nodeInspector).toContainText("Contact unknown");
  await expect(nodeInspector).toContainText("1 visible tunnel");
  await expect(nodeInspector).not.toContainText("online; 1 visible tunnel");
  await expect(graphPanel.getByLabel("Topology minimap")).toHaveCount(0);
  await expect(page.getByLabel("Tunnel plans data grid")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Create tunnel plan" }),
  ).toHaveCount(0);
  await expect(page.getByLabel("OSPF updater plans data grid")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Apply" })).toHaveCount(0);
});

test("network tests keeps diagnostics and trends mutation-free", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Network", "Tests");

  await expect(
    page.getByRole("heading", { level: 1, name: "Network tests" }),
  ).toBeVisible();
  await expect(page.getByText("Loading network workspace")).toHaveCount(0, {
    timeout: 15000,
  });
  await expect(
    page.getByRole("heading", { level: 2, name: "Network tests" }),
  ).toBeVisible();
  const endpointVisibility = page.getByLabel("Plan endpoint visibility");
  await expect(
    endpointVisibility.getByText("-", { exact: true }),
  ).toBeVisible();
  await expect(
    endpointVisibility.getByText("Unavailable", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Network test review contract")).toHaveCount(0);
  const trendCharts = page.getByLabel("Network test trend charts");
  await expect(trendCharts).toBeVisible();
  await expect(trendCharts).toContainText(
    "Retained tiered history · 5m coarsest source resolution",
  );
  await expect(trendCharts).toContainText(
    /Metric definition: Each point is the mean RTT from one exact bounded ICMP probe run or from the source runs represented by one retained evidence bucket/,
  );
  await expect(trendCharts).toContainText(
    "No trend line yet; capture another run to compare movement.",
  );
  await expect(trendCharts).toContainText(
    "10.1 Mbps avg - 10% of expected 100 Mbps",
  );
  await expect(
    page.getByRole("button", { name: "Inspect status" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Run probe" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Review speed test" }),
  ).toBeVisible();

  await expect(page.getByLabel("Tunnel plans data grid")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Create tunnel plan" }),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save plan" })).toHaveCount(0);
  await expect(page.getByLabel("OSPF updater plans data grid")).toHaveCount(0);
});

test("network evidence stays read-mostly and links to network action pages", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "Network", "Evidence");

  await expect(
    page.getByRole("heading", { level: 1, name: "Network evidence" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Network evidence" }),
  ).toBeVisible();
  const evidenceControls = page.getByLabel("Network evidence controls");
  await expect(
    evidenceControls.getByLabel("Network evidence time range", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    evidenceControls.getByText("Advanced filters", { exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Network evidence timeline")).toBeVisible();
  await expect(page.getByText(/outputs not loaded/)).toBeVisible();
  for (const label of [
    "Recommendation evidence",
    "Measurement evidence",
    "Status and probe results",
    "Related command jobs",
  ]) {
    await expect(page.getByLabel(label, { exact: true })).toBeVisible();
  }
  await expect(
    page.getByText(/10\.1 Mbps avg - 10% of expected 100 Mbps/).first(),
  ).toBeVisible();
  const actions = page.getByLabel("Network evidence actions");
  for (const label of [
    "Open graph",
    "Run tests",
    "Tunnel plans",
    "Open OSPF",
  ]) {
    await expect(actions.getByRole("button", { name: label })).toBeVisible();
  }
  await expect(
    actions.getByRole("button", { name: /Load output|Reload output/ }),
  ).toBeVisible();
  await expect(
    actions.getByRole("button", { name: "Compare to previous" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Apply cost" })).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Inspect status" }),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save plan" })).toHaveCount(0);

  await actions.getByRole("button", { name: "Open graph" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network graph" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Evidence");
  await page
    .getByLabel("Network evidence actions")
    .getByRole("button", { name: "Run tests" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network tests" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Evidence");
  await page
    .getByLabel("Network evidence actions")
    .getByRole("button", { name: "Tunnel plans" })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Evidence");
  const ospfButton = page
    .getByLabel("Network evidence actions")
    .getByRole("button", { name: "Open OSPF" });
  await expect(ospfButton).toBeEnabled();
  await ospfButton.click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Network OSPF" }),
  ).toBeVisible();
  const ospfTable = page.getByLabel("OSPF updater plans data grid");
  await expect(ospfTable).toBeVisible();
  await expect(
    ospfTable.getByText("Review required", { exact: true }),
  ).toBeVisible();
  if ((page.viewportSize()?.width ?? 0) <= 720) {
    const ospfPlanCard = ospfTable
      .locator(".gridMobileCard", { hasText: "sfo-fra-gre" })
      .first();
    await expect(
      ospfPlanCard.getByRole("button", { name: "Apply cost", exact: true }),
    ).toHaveCount(0);
    for (const header of ["Current cost", "Recommendation", "Evidence"]) {
      await expect(
        ospfTable.getByRole("columnheader", { name: header, exact: true }),
      ).toHaveCount(0);
    }
    await activate(ospfPlanCard);
    await expect(ospfTable.getByText("Left endpoint")).toBeVisible();
    await ospfPlanCard.getByRole("checkbox").check();
    const actionButton = ospfTable
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true });
    const actionBounds = await actionButton.boundingBox();
    const gridBounds = await ospfTable.boundingBox();
    expect(actionBounds).not.toBeNull();
    expect(gridBounds).not.toBeNull();
    expect(actionBounds!.x).toBeGreaterThanOrEqual(gridBounds!.x);
    expect(actionBounds!.x + actionBounds!.width).toBeLessThanOrEqual(
      gridBounds!.x + gridBounds!.width + 1,
    );
    await actionButton.click();
    await expect(
      page.getByRole("menuitem", { name: "Apply cost", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
  } else {
    const ospfPlanRow = ospfTable
      .getByRole("row")
      .filter({ hasText: "sfo-fra-gre" })
      .first();
    await ospfPlanRow.click({ button: "right" });
    await expect(
      page.getByRole("menuitem", { name: "Apply cost", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await ospfPlanRow.getByRole("checkbox").check();
    await ospfTable
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await expect(
      page.getByRole("menuitem", { name: "Apply cost", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
  }
});

test("network tunnel plans expose only explicit plan-owned runtime and routing controls", async ({
  page,
}) => {
  await gotoConsoleHome(page);

  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    await expect(mobilePageSelector).not.toContainText("Network / Promotion");
  } else {
    const networkSections = page
      .getByRole("navigation", { name: "Primary console navigation" })
      .getByLabel("Network sections");
    await expect(
      networkSections.getByRole("button", { name: "Promotion", exact: true }),
    ).toHaveCount(0);
  }

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();
  const tunnelPlanGrid = page.getByLabel("Tunnel plans data grid");
  await expect(tunnelPlanGrid).toBeVisible();
  const isMobileViewport = (page.viewportSize()?.width ?? 0) <= 720;
  if (!isMobileViewport) {
    for (const header of [
      "Plan",
      "Endpoints",
      "Bandwidth",
      "Runtime owner",
      "Runtime",
      "Connectivity",
      "OSPF",
    ]) {
      await expect(
        tunnelPlanGrid.getByRole("button", {
          name: header,
          exact: true,
        }),
      ).toBeVisible();
    }
  }
  await expect(tunnelPlanGrid).toContainText("Agent builtin");
  await expect(tunnelPlanGrid).toContainText("L1476 · R1476");
  await expect(tunnelPlanGrid).toContainText("External observed");
  await expect(tunnelPlanGrid).toContainText("Reviewed");
  await expect(tunnelPlanGrid).toContainText("Tunnel only");
  await expect(page.getByText(/promotion workflow/i)).toHaveCount(0);
  await expect(page.getByText(/generated config/i)).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Create tunnel plan" }),
  ).toHaveCount(0);

  const savedPlan = isMobileViewport
    ? tunnelPlanGrid.getByLabel(
        "Tunnel plans mobile card dddddddd-eeee-4fff-8000-111111111111",
      )
    : tunnelPlanGrid
        .locator(".gridBody [role=row]", { hasText: "sfo-fra-gre" })
        .first();
  if (isMobileViewport) {
    await savedPlan
      .getByLabel(
        "Select Tunnel plans row dddddddd-eeee-4fff-8000-111111111111",
      )
      .check();
    const actionButton = tunnelPlanGrid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true });
    const [gridBox, actionBox] = await Promise.all([
      tunnelPlanGrid.boundingBox(),
      actionButton.boundingBox(),
    ]);
    expect(gridBox).not.toBeNull();
    expect(actionBox).not.toBeNull();
    expect(actionBox!.x).toBeGreaterThanOrEqual(gridBox!.x);
    expect(actionBox!.x + actionBox!.width).toBeLessThanOrEqual(
      gridBox!.x + gridBox!.width,
    );
  }

  await activate(savedPlan);
  const planDetail = tunnelPlanGrid.locator(".gridExpandedRow");
  await expect(planDetail).toContainText("Declared interfaces");
  await expect(planDetail).toContainText("Left 1476 · Right 1476");
  await expect(planDetail).toContainText("Reviewed · cost 22");
  await expect(planDetail).toContainText(
    "Partially verified · Peer probe failed; not proof of disconnect",
  );
  if (isMobileViewport) {
    const runtimeOwnership = planDetail
      .locator(".tunnelPlanFacts > span")
      .filter({ hasText: "Runtime ownership" });
    await expect(
      runtimeOwnership.getByText("Agent builtin", { exact: true }),
    ).toBeVisible();
    await expect(
      planDetail.getByLabel("Endpoints: agent-sfo-01 / agent-fra-02", {
        exact: true,
      }),
    ).toBeVisible();
    const applyState = planDetail
      .locator(".tunnelPlanFacts > span")
      .filter({ hasText: "Apply state" });
    await expect(
      applyState.getByText("L Healthy · R Healthy", { exact: true }),
    ).toBeVisible();
  }
  await expect(
    planDetail.getByRole("button", {
      name: "Close Tunnel plans row details",
    }),
  ).toBeVisible();
  await activate(
    planDetail.getByRole("button", {
      name: "Close Tunnel plans row details",
    }),
  );

  if (isMobileViewport) {
    await invokeGridRowAction(page, tunnelPlanGrid, savedPlan, "Edit");
  } else {
    await savedPlan.click({ button: "right" });
    await activate(page.getByRole("menuitem", { name: "Edit", exact: true }));
  }
  const editor = page.locator(".tunnelPlanComposer");
  await expect(
    editor.getByRole("heading", { name: "Update tunnel plan" }),
  ).toBeVisible();
  await expect(editor.getByLabel("Tunnel plan name")).toHaveValue(
    "sfo-fra-gre",
  );
  await expect(editor.getByLabel("Tunnel plan name")).toHaveAttribute(
    "readonly",
    "",
  );
  await expect(
    editor.getByLabel("Tunnel interface", { exact: true }),
  ).toHaveValue("tunab");
  await expect(editor.getByLabel("Tunnel bandwidth")).toHaveValue("100");
  await expect(editor.getByLabel("Left tunnel MTU")).toHaveValue("1476");
  await expect(editor.getByLabel("Right tunnel MTU")).toHaveValue("1476");
  await expect(
    editor.getByRole("button", { name: "Review update" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Create plan" }),
  ).toBeDisabled();
  await editor.getByLabel("Tunnel bandwidth").fill("250");
  await expect(
    editor.getByRole("button", { name: "Review update" }),
  ).toBeEnabled();
  await activate(editor.getByRole("button", { name: "Review update" }));
  const updatePrompt = page.locator(".confirmationPrompt", {
    hasText: "Confirm tunnel plan update",
  });
  await expect(updatePrompt).toContainText("revision 3");
  await activate(updatePrompt.getByRole("button", { name: "Update plan" }));
  await expect(editor).toHaveCount(0);
  const updateRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { tunnelPlans: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.tunnelPlans.at(-1);
  });
  expect(updateRequest).toMatchObject({
    bandwidth_mbps: 250,
    expected_revision: 3,
    left_mtu: 1476,
    name: "sfo-fra-gre",
    right_mtu: 1476,
  });

  await activate(page.getByRole("button", { name: "Create plan" }));
  await expect(tunnelPlanGrid).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Create tunnel plan" }),
  ).toBeVisible();
  await expect(
    page.getByRole("radiogroup", { name: "Tunnel runtime ownership" }),
  ).toBeVisible();
  await expect(
    page.getByText("Agent builtin routes and cleanup"),
  ).toBeVisible();
  await expect(
    page.getByText("OSPF cost control", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(
      /Each endpoint uses its Configuration preset unless this plan selects an override/,
    ),
  ).toBeVisible();
  await expect(
    page.getByText(/operator-owned adapter definition/),
  ).toBeVisible();
  if (isMobileViewport) {
    const closeGeometry = await page.evaluate(() => {
      const header = document.querySelector(".tunnelPlanComposerHeader");
      const close = header?.querySelector<HTMLButtonElement>(
        '[aria-label="Close tunnel plan editor"]',
      );
      if (!header || !close) return null;
      const headerRect = header.getBoundingClientRect();
      const closeRect = close.getBoundingClientRect();
      return {
        closeRight: closeRect.right,
        closeTop: closeRect.top,
        headerRight: headerRect.right,
        headerTop: headerRect.top,
      };
    });
    expect(closeGeometry).not.toBeNull();
    expect(closeGeometry!.closeTop).toBeLessThanOrEqual(
      closeGeometry!.headerTop + 20,
    );
    expect(closeGeometry!.closeRight).toBeGreaterThanOrEqual(
      closeGeometry!.headerRight - 20,
    );
  }
  await activate(page.getByRole("button", { name: "External observed" }));
  await expect(page.getByText("Agent builtin routes and cleanup")).toHaveCount(
    0,
  );
  await expect(
    page.getByLabel("Left runtime adapter", { exact: true }),
  ).toHaveCount(0);
  await activate(page.getByRole("button", { name: "Custom adapter" }));
  await expect(page.getByText("Agent builtin routes and cleanup")).toHaveCount(
    0,
  );
  await expect(
    page.getByLabel("Left runtime adapter", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Right runtime adapter", { exact: true }),
  ).toBeVisible();
  await activate(
    page.getByRole("button", { name: "Close tunnel plan editor" }),
  );
  await expect(
    page.getByRole("heading", { name: "Create tunnel plan" }),
  ).toHaveCount(0);
  await expect(tunnelPlanGrid).toBeVisible();
});

test("system overview keeps platform health separate from fleet monitoring", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "System", "Overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "System overview" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Control-plane overview" }),
  ).toBeVisible();
  const systemOverview = page.getByLabel("System overview operations overview");
  await expect(systemOverview).toContainText("Service health");
  await expect(systemOverview).toContainText("Database");
  await expect(systemOverview).toContainText("Control-plane queue");
  await expect(systemOverview).toContainText("Gateway");
  await expect(systemOverview).toContainText("Worker");
  await expect(systemOverview).toContainText("Diagnostics");
  await expect(systemOverview).not.toContainText("Capacity forecast");

  const systemChart = page.getByLabel(
    "Selected chart - Dispatch queue system metrics data coverage",
  );
  await expect(systemChart).toContainText("gaps");
  await expect(
    page
      .getByRole("figure", {
        name: /Selected chart - Dispatch queue system metrics/,
      })
      .first(),
  ).toHaveAttribute("data-gap-policy", "preserve");

  const main = page.getByRole("main");
  await expect(main.locator(".vpsMonitorGrid")).toHaveCount(0);
  await expect(main.locator(".vpsMonitorCard")).toHaveCount(0);
  await expect(main).not.toContainText("VPS cards");

  await openConsoleSubpage(page, "Fleet", "Monitor");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet monitor" }),
  ).toBeVisible();
  await expect(page.getByLabel("VPS monitor cards")).toBeVisible();
  await expect(
    page.getByLabel("VPS monitor cards").locator(".vpsMonitorCard"),
  ).toHaveCount(3);
});

test("system capacity focuses on telemetry-backed control-plane limits", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "System", "Capacity");

  await expect(
    page.getByRole("heading", { level: 1, name: "System capacity" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Capacity telemetry", exact: true }),
  ).toBeVisible();

  const posture = page.getByLabel("System capacity posture overview");
  await expect(posture).toContainText("Subsystem capacity");
  await expect(posture).toContainText("Database");
  await expect(posture).toContainText("Dispatch");
  await expect(posture).toContainText("Gateway");
  await expect(posture.getByRole("tab")).toHaveCount(3);
  await expect(posture).toContainText("Queue growth");
  await expect(posture).toContainText("Worker availability");
  await expect(posture).toContainText("Suite Config fields");
  await expect(posture).not.toContainText("Telemetry gaps");

  await expect(
    page.getByRole("heading", { name: "Dispatch capacity", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Dispatch capacity thresholds")).toContainText(
    "queue is growing",
  );
  await expect(
    page.getByRole("heading", { name: "Gateway capacity", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText("capacity.dispatcher_in_flight")).toBeVisible();
  await posture.getByRole("tab", { name: /Database/ }).click();
  await expect(
    page.getByRole("heading", { name: "Database capacity", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("capacity.api_db_pool")).toBeVisible();
  await expect(page.getByText(/dashboard API|backend fields/i)).toHaveCount(0);

  const main = page.getByRole("main");
  await expect(main.locator(".vpsMonitorGrid")).toHaveCount(0);
  await expect(main.locator(".vpsMonitorCard")).toHaveCount(0);
  await expect(main).not.toContainText("CPU usage");
  await expect(main).not.toContainText("Memory usage");
  await expect(main).not.toContainText("Disk usage");
});

test("system suite config owns control-plane config and excludes per-VPS editors", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "System", "Suite config");

  await expect(
    page.getByRole("heading", { level: 1, name: "Suite config" }),
  ).toBeVisible();
  await expect(
    page
      .locator(".systemConfigOverview")
      .getByRole("heading", { name: "Suite config" }),
  ).toBeVisible();
  await expect(page.getByLabel("Suite config impact summary")).toContainText(
    "Configuration inventory",
  );
  await expect(page.getByLabel("Suite config impact summary")).toContainText(
    "hot-reload fields",
  );
  await expect(page.locator(".systemConfigOverview")).toContainText(
    "Inventory hot reload",
  );

  const boundary = page.getByLabel("Suite config ownership boundary");
  await expect(boundary).toContainText("System scope");
  await expect(boundary).toContainText(
    "Suite TOML controls API, gateway, network, worker, capacity, storage, secrets, and control-plane timeouts.",
  );
  await expect(boundary).toContainText("Runtime config scope");
  await expect(boundary).toContainText(
    "Per-VPS runtime reads, overrides, patches, configuration presets, and rules stay in Config workflows.",
  );
  await expect(boundary).toContainText("Save contract");

  const sections = page.getByLabel("Suite config sections");
  for (const label of [
    "API",
    "Gateway",
    "Network",
    "Worker",
    "Capacity",
    "Storage",
    "Secrets",
    "Timeouts",
    "Review",
  ]) {
    await expect(sections).toContainText(label);
  }
  await expect(page.getByLabel("Suite config editor mode")).toBeVisible();
  await expect(page.getByLabel("Suite config sticky save bar")).toContainText(
    "No draft changes",
  );
  await page
    .getByLabel("Suite config editor mode")
    .getByRole("button", { name: "Advanced TOML" })
    .click();
  const suiteToml = page.getByLabel("Suite config TOML");
  const originalSuiteToml = await suiteToml.inputValue();
  await expect(suiteToml).not.toHaveAttribute("title", originalSuiteToml);
  await suiteToml.fill(`${originalSuiteToml}\n# operator maintenance note\n`);
  await expect(suiteToml).not.toHaveAttribute(
    "title",
    `${originalSuiteToml}\n# operator maintenance note\n`,
  );
  await expect(page.getByLabel("Suite config sticky save bar")).toContainText(
    "Advanced TOML text changed",
  );
  await expect(
    page.getByLabel("Suite config validation and save review"),
  ).toContainText("Formatting or comments only");
  await suiteToml.fill(originalSuiteToml);
  await expect(page.getByLabel("Suite config sticky save bar")).toContainText(
    "No draft changes",
  );
  await page
    .getByLabel("Suite config editor mode")
    .getByRole("button", { name: "Fields" })
    .click();
  await sections.getByRole("button", { name: /Gateway/ }).click();
  const apiUrl = page.getByLabel("API URL");
  const apiUrlValue = await apiUrl.inputValue();
  expect((await apiUrl.getAttribute("title")) ?? "").not.toContain(apiUrlValue);
  const apiUrlMetadata = page
    .locator(".systemConfigFieldRow", { has: apiUrl })
    .locator(".systemConfigFieldMeta summary span");
  const apiUrlMetadataIsShortened = await apiUrlMetadata.evaluate(
    (element) => element.scrollWidth > element.clientWidth + 1,
  );
  if (apiUrlMetadataIsShortened) {
    await expect(apiUrlMetadata).toHaveAttribute(
      "title",
      new RegExp(apiUrlValue.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
    );
  } else {
    await expect(apiUrlMetadata).not.toHaveAttribute("title", /\S/);
  }
  await expect(
    page.getByLabel("Suite config validation and save review"),
  ).toContainText("Edit");
  await expect(page.getByText("Advanced redacted JSON diff")).toBeVisible();

  await page.getByLabel("Search suite config settings").fill("dispatcher");
  await expect(page.getByText("2 matching settings")).toBeVisible();
  await expect(page.getByLabel("Capacity suite config fields")).toContainText(
    "Dispatcher batch",
  );
  await expect(page.getByLabel("API suite config fields")).toHaveCount(0);
  await page.getByLabel("Search suite config settings").fill("");
  await expect(page.getByLabel("Capacity suite config fields")).toBeVisible();

  await sections.getByRole("button", { name: /API/ }).click();
  const artifactMaxBytesField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Artifact max bytes"),
  });
  await expect(
    artifactMaxBytesField.getByRole("button", { name: "Use default" }),
  ).toBeDisabled();
  await page.getByLabel("Artifact max bytes").fill("1048576");
  await expect(
    artifactMaxBytesField.getByRole("button", { name: "Use default" }),
  ).toBeEnabled();
  await artifactMaxBytesField
    .getByRole("button", { name: "Use default" })
    .click();
  await expect(page.getByLabel("Artifact max bytes")).toHaveValue("");
  await expect(page.getByLabel("Suite config sticky save bar")).toContainText(
    "No draft changes",
  );

  await sections.getByRole("button", { name: /Capacity/ }).click();
  const apiDbField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("API DB pool"),
  });
  await page.getByLabel("API DB pool").fill("40");
  await expect(apiDbField).toContainText("Changed metadata");
  await expect(page.getByLabel("Suite config sticky save bar")).toContainText(
    "1 changed key",
  );
  await expect(
    apiDbField.getByRole("button", { name: "Use default" }),
  ).toBeEnabled();
  await apiDbField.getByRole("button", { name: "Use default" }).click();
  await expect(page.getByLabel("API DB pool")).toHaveValue("");
  await expect(apiDbField).toContainText("Default 32");
  await page.getByLabel("API DB pool").fill("40");
  await expect(
    apiDbField.getByRole("button", { name: "Reset current" }),
  ).toBeEnabled();
  await apiDbField.getByRole("button", { name: "Reset current" }).click();
  await expect(page.getByLabel("API DB pool")).toHaveValue("32");

  await expect(page.getByLabel("VPS config target")).toHaveCount(0);
  await expect(page.getByLabel("Saved desired runtime TOML")).toHaveCount(0);
  await expect(page.getByLabel("VPS replacement override TOML")).toHaveCount(0);
  await expect(page.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(
    page.getByLabel("Rendered bulk runtime config patch TOML"),
  ).toHaveCount(0);
  await expect(
    page.getByLabel("Temporary bulk runtime config patch TOML"),
  ).toHaveCount(0);

  await boundary.getByRole("button", { name: "Open Config / Per-VPS" }).click();
  await expect(
    page.getByRole("heading", { name: "Per-VPS desired config" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "System", "Suite config");
  await page
    .getByLabel("Suite config ownership boundary")
    .getByRole("button", { name: "Open Config / VPS override patch" })
    .click();
  await expect(
    page.getByRole("heading", { name: "VPS override patch" }),
  ).toBeVisible();
});

test("system maintenance owns artifact cleanup and maintenance job records", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  const mobilePageSelector = await openMobilePageSelector(page);
  if (mobilePageSelector) {
    await expect(mobilePageSelector).not.toContainText("Jobs / Server jobs");
  } else {
    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await activate(
      nav.getByRole("button", { name: "Jobs", exact: true }).first(),
    );
    await expect(
      nav
        .getByLabel("Jobs sections")
        .getByRole("button", { name: "Server jobs", exact: true }),
    ).toHaveCount(0);
  }

  await openConsoleSubpage(page, "System", "Maintenance");
  await activateSystemMaintenanceSubpanel(page, "Artifact cleanup");

  await expect(
    page.getByRole("heading", { level: 1, name: "System maintenance" }),
  ).toBeVisible();
  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await expect(cleanupPanel).toBeVisible();
  await expect(cleanupPanel.getByText("Preview gate")).toBeVisible();
  await expect(
    cleanupPanel.getByText("Artifact types", { exact: true }),
  ).toBeVisible();
  await expect(
    cleanupPanel.getByLabel("Artifact cleanup readiness"),
  ).toContainText("Preview required");
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeDisabled();

  await cleanupPanel.getByRole("button", { name: "Preview" }).click();
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    "1 artifact",
  );
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    "1 representative",
  );
  await expect(
    cleanupPanel.getByLabel("Representative cleanup objects"),
  ).toContainText("file-transfer-sources/");
  await expect(
    cleanupPanel.getByLabel("Artifact cleanup readiness"),
  ).toContainText("Ready for confirmation");
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeEnabled();
  await cleanupPanel.getByRole("button", { name: "Delete artifacts" }).click();
  await expect(
    page.getByRole("region", { name: "Confirm artifact deletion" }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Type DELETE to confirm artifact deletion"),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "Confirm artifact deletion" }),
  ).toContainText("1 representative");
  await activate(page.getByRole("button", { name: "Close confirmation" }));

  await activateSystemMaintenanceSubpanel(page, "Maintenance jobs");

  const maintenanceJobs = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Maintenance jobs" }),
  });
  await expect(maintenanceJobs).toContainText(
    "retained control-plane maintenance jobs",
  );
  await expect(maintenanceJobs).toContainText("Maintenance job records");
  await expect(page.getByRole("heading", { name: "Server jobs" })).toHaveCount(
    0,
  );
  await expect(page.getByRole("heading", { name: "Job history" })).toHaveCount(
    0,
  );
});

test("system maintenance presents an empty cleanup preview as a neutral no-op", async ({
  page,
}) => {
  await installConsoleApiMock(page, {
    fileTransferSourceArtifactsOverride: [],
  });
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "System", "Maintenance");
  await activateSystemMaintenanceSubpanel(page, "Artifact cleanup");

  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await cleanupPanel.getByRole("button", { name: "Preview" }).click();

  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    "No matching artifacts",
  );
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toBeFocused();
  const readiness = cleanupPanel.getByLabel("Artifact cleanup readiness");
  await expect(readiness).toContainText("Nothing to delete");
  await expect(readiness).toContainText("No artifacts match");
  await expect(readiness).toContainText("Evidence status");
  await expect(readiness).not.toContainText("Delete blocked");
  await expect(readiness).not.toContainText("Not reported by API");
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeDisabled();
});

test("system preferences separates personal display from shared defaults", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "System", "Preferences");

  await expect(
    page.getByRole("heading", { level: 1, name: "System preferences" }),
  ).toBeVisible();
  const preferencesScope = page.getByLabel("Preferences scope overview");
  await expect(preferencesScope).toContainText("Personal display");
  await expect(preferencesScope).toContainText(
    "Personal — stored for this operator",
  );
  await expect(preferencesScope).toContainText("review prompt display");
  await expect(preferencesScope).toContainText("Browser state");
  await expect(preferencesScope).toContainText(
    "Browser — stored on this device",
  );
  await expect(preferencesScope).toContainText("System-linked defaults");
  await expect(preferencesScope).toContainText(
    "System — shared workflow settings",
  );
  await expect(preferencesScope).toContainText(
    "not personal display preferences",
  );

  const personal = page.getByLabel("Personal display preferences");
  await expect(personal).toContainText("Review prompts");
  await expect(personal).toContainText("does not weaken required review");
  await expect(personal).toContainText("Bulk execution summaries");
  await expect(personal).toContainText("Home chart presentation");
  await expect(
    personal.getByLabel("Home telemetry curve exclusions"),
  ).toBeVisible();
  await expect(page.getByLabel("Preferences sticky save bar")).toContainText(
    "No preference changes",
  );
  await expect(
    page.getByLabel("Reset Home chart presentation to default"),
  ).toBeVisible();

  await page.getByRole("button", { name: /System-linked defaults/ }).click();
  const shared = page.getByLabel("System-linked defaults");
  await expect(shared).toContainText("Gateway install material");
  await expect(shared).toContainText("Tunnel allocation pools");
  await expect(shared).toContainText("Open gateway settings");
  await expect(shared).toContainText("Open Suite Config");
  await expect(shared).toContainText("Open VPS identities");
  await expect(shared).not.toContainText("Home telemetry curves");
  await expect(shared).not.toContainText("operator-stored defaults");
  await expect(shared).not.toContainText("Server public key hex");
});

test("invalid preference drafts expose accessible save feedback", async ({
  page,
}) => {
  await gotoConsoleHome(page);
  await openConsoleSubpage(page, "System", "Preferences");

  const timezone = page.getByLabel("Display timezone");
  await timezone.fill("Mars/Olympus_Mons");

  const validation = page.locator("#preferences-draft-validation-error");
  await expect(validation).toBeVisible();
  await expect(validation).toHaveAttribute("role", "status");
  await expect(validation).toHaveAttribute("aria-live", "polite");
  await expect(validation).toContainText(
    "Timezone must be a valid IANA identifier",
  );
  await expect(timezone).toHaveAttribute("aria-invalid", "true");
  await expect(timezone).toHaveAttribute(
    "aria-describedby",
    "preferences-draft-validation-error",
  );

  for (const name of ["Save changes", "Save preferences"]) {
    const save = page.getByRole("button", { name });
    await expect(save).toBeEnabled();
    await expect(save).toHaveAttribute(
      "aria-describedby",
      "preferences-draft-validation-error",
    );
  }

  const stickySave = page.getByRole("button", { name: "Save changes" });
  await stickySave.focus();
  await expect(stickySave).toBeFocused();
  await stickySave.click();
  await expect(validation).toBeVisible();
  await expect(timezone).toHaveValue("Mars/Olympus_Mons");
  const preferenceRequestCount = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: { operatorPreferences: unknown[] };
        }
      ).__vpsmanTestRequests.operatorPreferences.length,
  );
  expect(preferenceRequestCount).toBe(0);
});

async function expectCommandPaletteGroup(
  page: Page,
  group: string,
  query: string,
) {
  await page.getByRole("button", { name: "Open command palette" }).click();
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await page.getByLabel("Command palette search").fill(query);
  const result = palette.locator(`[data-command-group="${group}"]`).first();
  await expect(result).toBeVisible();
  await expect(result).toContainText(group);
  await page.keyboard.press("Escape");
  await expect(palette).toBeHidden();
}

async function expectShorterElement(shorter: Locator, taller: Locator) {
  await expect
    .poll(async () => {
      const shorterBox = await shorter.boundingBox();
      const tallerBox = await taller.boundingBox();
      if (!shorterBox || !tallerBox) return Number.NEGATIVE_INFINITY;
      return tallerBox.height - shorterBox.height;
    })
    .toBeGreaterThanOrEqual(8);
}

function makeMonitorAgentFixtures(count: number) {
  const rootCapabilities = {
    can_apply_process_limits: true,
    can_attempt_privileged_ops: true,
    can_manage_runtime_tunnels: true,
    effective_uid: 0,
    privilege_mode: "root",
    unprivileged_hint: null,
  };
  const unprivilegedCapabilities = {
    can_apply_process_limits: false,
    can_attempt_privileged_ops: true,
    can_manage_runtime_tunnels: false,
    effective_uid: 1000,
    privilege_mode: "unprivileged",
    unprivileged_hint: "fixture unprivileged VPS",
  };
  return Array.from({ length: count }, (_, index) => {
    const number = index + 1;
    const region = ["US", "DE", "SG", "JP", "NL"][index % 5];
    const provider = ["alpha", "beta", "gamma", "delta"][index % 4];
    const status =
      index % 17 === 0 ? "offline" : index % 11 === 0 ? "stale" : "online";
    return {
      capabilities:
        index % 9 === 0 ? unprivilegedCapabilities : rootCapabilities,
      display_name: `fleet-${String(number).padStart(3, "0")}-${region.toLowerCase()}`,
      id: `fixture-agent-${String(number).padStart(3, "0")}`,
      last_ip: status === "offline" ? null : `198.51.100.${(number % 220) + 1}`,
      registration_ip: `192.0.2.${(number % 220) + 1}`,
      status,
      tags: [
        `country:${region}`,
        `provider:${provider}`,
        index % 2 === 0 ? "role:edge" : "role:worker",
      ],
    };
  });
}

async function monitorFirstRowCount(monitor: Locator): Promise<number> {
  return monitor.locator(".vpsMonitorCard").evaluateAll((cards) => {
    if (cards.length === 0) return 0;
    const firstTop = cards[0].getBoundingClientRect().top;
    return cards.filter(
      (card) => Math.abs(card.getBoundingClientRect().top - firstTop) <= 1,
    ).length;
  });
}

async function expectMonitorCardsToFit(page: Page, label: string) {
  const overflow = await page.locator(".vpsMonitorCard").evaluateAll((cards) =>
    cards.flatMap((card, cardIndex) => {
      const cardRect = card.getBoundingClientRect();
      return Array.from(card.querySelectorAll<HTMLElement>("*"))
        .map((element) => {
          const rect = element.getBoundingClientRect();
          const style = window.getComputedStyle(element);
          if (
            style.display === "none" ||
            style.visibility === "hidden" ||
            (rect.width === 0 && rect.height === 0)
          ) {
            return null;
          }
          const text =
            element.textContent?.trim().replace(/\s+/g, " ").slice(0, 80) ?? "";
          const elementOverflow =
            element.scrollWidth > element.clientWidth + 1 &&
            style.overflowX !== "visible" &&
            !(
              style.textOverflow === "ellipsis" &&
              Boolean(element.getAttribute("title"))
            );
          const escapesCard =
            rect.right > cardRect.right + 1 || rect.left < cardRect.left - 1;
          return elementOverflow || escapesCard
            ? {
                cardIndex,
                className:
                  element.getAttribute("class") ??
                  element.tagName.toLowerCase(),
                elementOverflow,
                escapesCard,
                text,
              }
            : null;
        })
        .filter(Boolean);
    }),
  );
  expect(overflow, `${label} monitor card text/layout overflow`).toEqual([]);
  const pageOverflow = await page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  );
  expect(pageOverflow, `${label} page horizontal overflow`).toBeLessThanOrEqual(
    1,
  );
}

async function expectHomeOverviewToFit(page: Page, label: string) {
  const overflow = await page.locator(".homeWorkspace").evaluate((workspace) =>
    Array.from(
      workspace.querySelectorAll<HTMLElement>(
        ".homeCommandBand, .homePostureStrip, .homeReviewPanel, .homeActionRow, .homeActivityRow, button, input",
      ),
    )
      .map((element) => {
        const rect = element.getBoundingClientRect();
        const style = window.getComputedStyle(element);
        if (
          style.display === "none" ||
          style.visibility === "hidden" ||
          (rect.width === 0 && rect.height === 0)
        ) {
          return null;
        }
        const text =
          element.textContent?.trim().replace(/\s+/g, " ").slice(0, 80) ?? "";
        const elementOverflow =
          element.scrollWidth > element.clientWidth + 1 &&
          style.overflowX !== "visible";
        let parent = element.parentElement;
        let insideFittingHorizontalScroller = false;
        while (parent) {
          const parentStyle = window.getComputedStyle(parent);
          const parentRect = parent.getBoundingClientRect();
          if (
            ["auto", "scroll"].includes(parentStyle.overflowX) &&
            parent.scrollWidth > parent.clientWidth + 1 &&
            parentRect.left >= -1 &&
            parentRect.right <= document.documentElement.clientWidth + 1
          ) {
            insideFittingHorizontalScroller = true;
            break;
          }
          if (parent === workspace) break;
          parent = parent.parentElement;
        }
        const pageOverflow =
          !insideFittingHorizontalScroller &&
          (rect.right > document.documentElement.clientWidth + 1 ||
            rect.left < -1);
        return elementOverflow || pageOverflow
          ? {
              className:
                element.getAttribute("class") ?? element.tagName.toLowerCase(),
              elementOverflow,
              pageOverflow,
              text,
            }
          : null;
      })
      .filter(Boolean),
  );
  expect(overflow, `${label} home text/layout overflow`).toEqual([]);
  const pageOverflow = await page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  );
  expect(pageOverflow, `${label} page horizontal overflow`).toBeLessThanOrEqual(
    1,
  );
}

async function selectCommandPaletteResult(
  page: Page,
  group: string,
  query: string,
) {
  await page.getByRole("button", { name: "Open command palette" }).click();
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await page.getByLabel("Command palette search").fill(query);
  const result = palette.locator(`[data-command-group="${group}"]`).first();
  await expect(result).toBeVisible();
  await result.click();
  await expect(palette).toBeHidden();
}

async function chooseVpsBySearch(
  root: Locator,
  label: string,
  query: string,
  optionName: RegExp,
) {
  await root.getByRole("combobox", { name: label }).fill(query);
  const option = root.page().locator(".vpsComboboxMenu").getByRole("option", {
    name: optionName,
  });
  await expect(option).toBeVisible();
  await option.click();
}

async function expectCanonicalVpsDetail(page: Page, vpsName: string) {
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Instance detail",
      exact: true,
    }),
  ).toBeVisible();
  const detail = page.getByLabel("Canonical VPS detail");
  await expect(detail).toContainText(vpsName);
  await expect(
    page.locator(".consoleHeader").getByLabel("Fleet status summary"),
  ).toHaveCount(0);
  await expect(page.locator(".consoleHeader")).not.toContainText(
    "Entire fleet",
  );
  await expect(detail.getByLabel("Selected VPS identity")).toContainText(
    vpsName,
  );
  const detailActions = detail.locator(".sectionActions").first();
  for (const label of [
    "Terminal",
    "Files",
    "Processes",
    "Run command",
    "Back up",
    "Config",
  ]) {
    await expect(
      detailActions.getByRole("button", { name: label, exact: true }),
    ).toBeVisible();
  }
  const resourceFacts = detail.getByLabel("VPS resource facts");
  await expect(resourceFacts).toContainText("State");
  await expect(resourceFacts).toContainText("Last contact");
  await expect(resourceFacts).toContainText("Last IP");
  await expect(resourceFacts).toContainText("Agent version");
  await expect(resourceFacts).toContainText("Alerts");
  await expect(resourceFacts).toContainText("Active jobs");
  await expect(detail).not.toContainText("Fleet status");
  await expect(detail).not.toContainText("scheduled_shell_argv");
  for (const tab of [
    "Summary",
    "Remote access",
    "Files",
    "Processes",
    "Config",
    "Backups",
    "Network",
    "Activity",
  ]) {
    await activate(detail.getByRole("tab", { name: tab }));
    await expect(detail.getByRole("tabpanel", { name: tab })).toBeVisible();
  }
}

function homeAttentionPanel(page: Page) {
  return homePanel(page, "Needs attention");
}

function homeActivityPanel(page: Page) {
  return homePanel(page, "Recent activity");
}

function homePanel(page: Page, heading: string) {
  return page.locator(".homeReviewPanel").filter({
    has: page.getByRole("heading", { name: heading }),
  });
}

async function expectJobHistoryDetailOpen(page: Page) {
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "Output" })).toBeVisible();
}

type PrivilegeHandoffSpec = {
  evidence?: string | RegExp;
  heading: string;
  prepare?: (page: Page) => Promise<void>;
  root?: (page: Page) => Locator;
  subpage: string;
  view: string;
};

async function expectLockedWorkflowPrivilegeHandoff(
  page: Page,
  workflow: PrivilegeHandoffSpec,
) {
  await openConsoleSubpage(page, workflow.view, workflow.subpage);
  await expect(
    page.getByRole("heading", { name: workflow.heading }).first(),
  ).toBeVisible();
  await workflow.prepare?.(page);

  const root = workflow.root?.(page) ?? page.getByRole("main");
  if (workflow.evidence) {
    await expect(root).toContainText(workflow.evidence);
  }

  const handoff = root
    .getByRole("button", { name: /Unlock privilege/ })
    .first();
  await expect(handoff).toBeVisible();
  await activate(handoff);

  const privilegeDialog = page.getByRole("dialog", {
    name: "Unlock privilege",
  });
  await expect(privilegeDialog).toContainText("request-bound assertions");
  await expect(page.locator("#root")).toHaveAttribute("inert", "");
  await privilegeDialog
    .getByRole("button", { name: "Close privilege unlock" })
    .click();
  await expect(page.locator("#root")).not.toHaveAttribute("inert", "");
  await expect(
    page.getByRole("heading", { name: workflow.heading }).first(),
  ).toBeVisible();
}

async function clickHomeQuickAction(page: Page, name: string) {
  await gotoConsoleHome(page);
  const quickActions = page.getByLabel("Home quick actions");
  await expect(quickActions).toBeVisible();
  await quickActions.getByRole("button", { name }).click();
}
