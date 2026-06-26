import { expect, test, type Locator, type Page } from "@playwright/test";
import { viewSubpages } from "../src/constants";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";
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
const releaseAccessibilityRoutes: Array<{ view: ActiveView; subpage: string }> = [
  { view: "Jobs", subpage: "Scheduled runs" },
  { view: "Config", subpage: "Bulk patch" },
  { view: "Config", subpage: "Per-VPS" },
  { view: "Config", subpage: "Rules" },
  { view: "Observability", subpage: "Alerts" },
  { view: "Observability", subpage: "Webhooks" },
  { view: "System", subpage: "Suite config" },
];
const customMockTests = new Set([
  "home shows a useful empty state when no VPS agents are loaded",
  "fleet monitor cards remain readable for 0 generated VPS fixtures",
  "fleet monitor cards remain readable for 3 generated VPS fixtures",
  "fleet monitor cards remain readable for 20 generated VPS fixtures",
  "fleet monitor cards remain readable for 50 generated VPS fixtures",
  "fleet monitor cards remain readable for 100 generated VPS fixtures",
]);

test.beforeEach(async ({ page }, testInfo) => {
  if (customMockTests.has(testInfo.title)) {
    return;
  }
  await installConsoleApiMock(page);
});

async function expectReachableByTab(
  page: Page,
  locator: Locator,
  label: string,
  maxTabs = 120,
  resetFocus = true,
) {
  await expect(locator, `${label} is visible before keyboard traversal`).toBeVisible();
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
    ].filter(Boolean).join(" / ");
  });
  throw new Error(`Keyboard traversal did not reach ${label}; active element was ${activeLabel}`);
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
      document.querySelectorAll<HTMLElement>('button:disabled, [role="button"][aria-disabled="true"]'),
    )
      .filter(isVisible)
      .map((element) => {
        const reason = [element.getAttribute("title") ?? "", describedText(element)]
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

async function contrastFailures(page: Page) {
  return page.evaluate(() => {
    type Rgba = { a: number; b: number; g: number; r: number };
    const samples = [
      { label: "body text", selector: "body, .consoleHeader h1, .consoleDataGrid", min: 4.5 },
      { label: "labels", selector: "label span, .compactForm strong, .consoleInlineDetailGrid strong", min: 4.5 },
      { label: "badges", selector: ".consoleStatusBadge", min: 4.5 },
      { label: "disabled controls", selector: "button:disabled, [role='button'][aria-disabled='true']", min: 4.5 },
      { label: "help text", selector: ".formHint, .compactForm small, .consoleField small, .configOverrideEditor > span", min: 4.5 },
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
        r: (foreground.r * foreground.a + background.r * background.a * (1 - foreground.a)) / alpha,
        g: (foreground.g * foreground.a + background.g * background.a * (1 - foreground.a)) / alpha,
        b: (foreground.b * foreground.a + background.b * background.a * (1 - foreground.a)) / alpha,
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
        const parsed = parseColor(window.getComputedStyle(item).backgroundColor);
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
      return 0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b);
    }

    function contrast(foreground: Rgba, background: Rgba) {
      const lighter = Math.max(luminance(foreground), luminance(background));
      const darker = Math.min(luminance(foreground), luminance(background));
      return (lighter + 0.05) / (darker + 0.05);
    }

    const failures: string[] = [];
    for (const sample of samples) {
      const elements = Array.from(document.querySelectorAll(sample.selector)).filter(visible).slice(0, 12);
      for (const element of elements) {
        const style = window.getComputedStyle(element);
        const foreground = parseColor(style.color);
        if (!foreground) continue;
        const background = effectiveBackground(element);
        const effectiveForeground = foreground.a < 1 ? blend(foreground, background) : foreground;
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

test("release IA exposes the intended top-level product areas", async ({ page }) => {
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "Primary console navigation" });
  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    for (const label of releaseTopLevel) {
      await expect(mobilePageSelector).toContainText(`${label} /`);
    }
    for (const label of legacyTopLevel) {
      await expect(mobilePageSelector).not.toContainText(`${label} /`);
    }
  } else {
    for (const label of releaseTopLevel) {
      await expect(nav.getByRole("button", { name: label, exact: true })).toBeVisible();
    }
    for (const label of legacyTopLevel) {
      await expect(nav.getByRole("button", { name: label, exact: true })).toHaveCount(0);
    }
  }
});

test("keyboard navigation reaches release shell controls and page primary action", async ({
  page,
}) => {
  await page.goto("/");

  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await expectReachableByTab(page, mobilePageSelector, "mobile page selector", 80);
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
    page.getByRole("searchbox", { name: "Search fleet" }),
    "global fleet search",
    80,
  );
  await expectReachableByTab(
    page,
    page.getByRole("button", { name: "Open privilege unlock" }),
    "privilege lock control",
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

test("release IA reaches every configured page and subpage", async ({ page }) => {
  await page.goto("/");

  expect([...Object.keys(viewSubpages)].sort()).toEqual([...releaseTopLevel].sort());

  for (const view of releaseTopLevel as ActiveView[]) {
    for (const subpage of viewSubpages[view]) {
      await openConsoleSubpage(page, view, subpage.label);

      const header = page.locator(".consoleHeader");
      await expect(header.getByText(`vpsman / ${view} / ${subpage.label}`)).toBeVisible();
      await expect(header.getByLabel("Page operational context")).toContainText(subpage.label);
      await expect(header.getByLabel("Fleet status summary")).toBeVisible();
      await expect(page.getByText(/Http 404 \(404\)|HTTP 404 \(404\)/)).toHaveCount(0);
      await expect(page.getByText(/Loading .* workspace/)).toHaveCount(0);
    }
  }
});

test("release pages use operational page headers", async ({ page }) => {
  await page.goto("/");

  const defaultRoutes = [
    { view: "Home", subpage: "Overview", title: "Home", section: "Overview" },
    { view: "Fleet", subpage: "Instances", title: "Fleet instances", section: "Instances" },
    { view: "Remote Operations", subpage: "Terminal", title: "Terminal", section: "Terminal" },
    { view: "Jobs", subpage: "History", title: "Job history", section: "History" },
    { view: "Automation", subpage: "Schedules", title: "Schedules", section: "Schedules" },
    { view: "Network", subpage: "Overview", title: "Network overview", section: "Overview" },
    { view: "Backups", subpage: "Overview", title: "Backup overview", section: "Overview" },
    { view: "Config", subpage: "Overview", title: "Config", section: "Overview" },
    { view: "Observability", subpage: "Fleet metrics", title: "Fleet metrics", section: "Fleet metrics" },
    { view: "Audit", subpage: "Events", title: "Audit events", section: "Events" },
    { view: "Access", subpage: "Overview", title: "Access overview", section: "Overview" },
    { view: "System", subpage: "Overview", title: "System overview", section: "Overview" },
  ];

  for (const route of defaultRoutes) {
    await openConsoleSubpage(page, route.view, route.subpage);
    const header = page.locator(".consoleHeader");
    await expect(header.getByRole("heading", { name: route.title })).toBeVisible();
    await expect(header.getByText(`vpsman / ${route.view} / ${route.section}`)).toBeVisible();

    const context = header.getByLabel("Page operational context");
    await expect(context).toContainText("Scope");
    await expect(context).toContainText("Resources");
    await expect(context).toContainText("Section");
    await expect(context).toContainText(route.section);
    await expect(header.getByLabel("Fleet status summary")).toBeVisible();
  }
});

test("remote operations owns terminal, files, transfers, processes, and bulk files", async ({ page }) => {
  await page.goto("/");

  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await expect(page.getByRole("heading", { name: "Terminal", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Terminal sessions" })).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(page.getByRole("heading", { name: "Files" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "File browser" })).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Transfers");
  await expect(page.getByRole("heading", { name: "Transfers" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "File transfer sessions" })).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  await expect(page.getByRole("heading", { name: "Processes" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Process supervisor" })).toBeVisible();

  await openConsoleSubpage(page, "Remote Operations", "Bulk files");
  await expect(page.getByRole("heading", { name: "Bulk files" })).toBeVisible();
});

test("jobs history links to operational owners without embedding their workflows", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "History");

  await expect(page.getByRole("heading", { level: 1, name: "Job history" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Job history" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Target results" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Terminal sessions" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "File browser" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "File transfer sessions" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Process supervisor" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Artifact cleanup" })).toHaveCount(0);

  const relatedLinks = page.getByLabel("Related Remote Operations pages");
  await expect(relatedLinks).toContainText("Related workflow owners");
  for (const link of [
    { button: "Terminal", heading: "Terminal" },
    { button: "Files", heading: "Files" },
    { button: "Transfers", heading: "Transfers" },
    { button: "Processes", heading: "Processes" },
    { button: "Bulk files", heading: "Bulk files" },
  ]) {
    await openConsoleSubpage(page, "Jobs", "History");
    await relatedLinks.getByRole("button", { name: link.button, exact: true }).click();
    await expect(
      page.getByRole("heading", { level: 1, name: link.heading, exact: true }),
    ).toBeVisible();
  }
});

test("job detail opens from release evidence pages", async ({ page }) => {
  test.slow();
  await page.goto("/");

  await homeActivityPanel(page).getByRole("button", { name: /scheduled_shell_argv job completed/ }).click();
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Remote Operations", "Transfers");
  await activate(page.getByLabel(/Open transfer job details/).first());
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Network", "Evidence");
  await page.getByLabel("Network evidence actions").getByRole("button", { name: /Load output|Reload output/ }).click();
  await activate(page.getByRole("button", { name: "Open job details" }).first());
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Backups", "Artifacts");
  await activate(page.getByRole("button", { name: "Open source job details" }));
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Automation", "Agent updates");
  await activate(page.getByRole("button", { name: "Open last update job" }));
  await expectJobHistoryDetailOpen(page);

  await openConsoleSubpage(page, "Audit", "Job evidence");
  const evidencePanel = page.locator(".auditJobEvidencePanel");
  await evidencePanel
    .getByLabel("Job evidence ledger data grid")
    .getByText("network speed test")
    .first()
    .click();
  await activate(evidencePanel.getByRole("button", { name: "Open in Jobs / History" }));
  await expectJobHistoryDetailOpen(page);
});

test("terminal open and resume are prepared from Remote Operations without Jobs", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal action composer details are covered through the desktop release workflow",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const launcher = page.getByLabel("New terminal composer");
  await expect(launcher.getByRole("heading", { name: "New terminal" })).toBeVisible();
  await expect(launcher.getByLabel("New terminal target")).toBeVisible();
  await launcher.getByRole("button", { name: "Prepare terminal review" }).click();

  const composer = page.locator(".terminalCommandComposer");
  await expect(composer.getByRole("heading", { name: "Terminal review composer" })).toBeVisible();
  await expect(composer.getByLabel("Dispatch mode boundary")).toContainText("Terminal mode only");
  await expect(composer.getByRole("button", { name: "Argv" })).toHaveCount(0);
  await expect(composer.getByLabel("Terminal action")).toHaveValue("open");
  await expect(composer.getByLabel("Terminal argv")).toHaveValue("/bin/sh -l");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("");
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");

  await page.getByRole("button", { name: "Attach terminal session 61616161" }).click();
  await expect(composer.getByLabel("Terminal action")).toHaveValue("open");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");

  const terminalPanel = page.locator(".terminalSessionsPanel");
  await expect(terminalPanel.getByRole("button", { name: "Copy transcript" })).toBeDisabled();
  await expect(terminalPanel.getByRole("button", { name: "Download transcript" })).toBeDisabled();
  await expect(terminalPanel.getByLabel("Terminal transcript availability")).toContainText("Load Replay first");
  await terminalPanel.locator(".terminalActiveHeader").getByRole("button", { name: "Replay" }).click();
  await expect(terminalPanel.getByLabel("Durable terminal replay preview")).toContainText("Durable replay 61616161");
  await expect(terminalPanel.getByRole("button", { name: "Copy transcript" })).toBeEnabled();
  await expect(terminalPanel.getByRole("button", { name: "Download transcript" })).toBeEnabled();
  await activate(terminalPanel.getByRole("button", { name: "Audit evidence" }));
  await expect(page.getByRole("heading", { level: 1, name: "Session evidence" })).toBeVisible();
});

test("jobs dispatch keeps terminal creation in remote operations", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const jobsComposer = page.locator(".commandComposer", {
    has: page.getByRole("heading", { name: "Dispatch command" }),
  });
  await expect(page.getByRole("heading", { level: 1, name: "Command dispatch" })).toBeVisible();
  await expect(jobsComposer.getByLabel("Dispatch mode boundary")).toContainText("Advanced dispatch");
  await expect(jobsComposer.getByLabel("Dispatch mode boundary")).toContainText("Remote Operations / Terminal");
  await expect(jobsComposer.locator(".segmented").getByRole("button", { exact: true, name: "Terminal" })).toHaveCount(0);
  await expect(jobsComposer.locator(".segmented").getByRole("button", { exact: true, name: "Argv" })).toBeVisible();

  await jobsComposer.getByRole("button", { name: "Remote terminal" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Terminal" })).toBeVisible();

  const terminalComposer = page.locator(".terminalCommandComposer");
  await expect(terminalComposer.getByRole("heading", { name: "Terminal review composer" })).toBeVisible();
  await expect(terminalComposer.getByLabel("Dispatch mode boundary")).toContainText("Terminal mode only");
  await terminalComposer.getByRole("button", { name: "Jobs / Dispatch" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Command dispatch" })).toBeVisible();
});

test("file browser reads a selected VPS path from Remote Operations without Jobs", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "file browser path and editor behavior is a dense desktop operations workflow",
  );
  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.fileBrowser.state"));
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(page.getByRole("heading", { name: "Files", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "File browser", exact: true })).toBeVisible();

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(page.getByRole("heading", { name: "File browser", exact: true })).toBeVisible();
  const targetPicker = page.getByRole("combobox", { name: "File browser target VPS" });
  await expect(targetPicker).toHaveValue("edge-sfo-01 (fo01)");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByRole("button", { name: /etc dir/ })).toBeVisible();
  await expect(page.getByLabel("Remote path")).toHaveValue("/");
  await expect(page.getByLabel("File browser directory state")).toContainText("Path");
  await expect(page.getByLabel("File browser directory state")).toContainText("/");
  await expect(page.getByLabel("File browser directory state")).toContainText("6 of 6 entries");
  await expect(page.getByLabel("File browser directory state")).toContainText("Complete");

  await page.getByRole("button", { name: /app\.conf/ }).dblclick();
  await expect(page.locator(".codeMirrorShell")).toContainText("listen=443");

  const deleteRequestsBeforeReview = await page.evaluate(
    () => {
      const requests = (window as unknown as {
        __vpsmanTestRequests: { fileBrowserJobs: Array<{ operation?: { type?: string } }> };
      }).__vpsmanTestRequests.fileBrowserJobs;
      return requests.filter((request) => request.operation?.type === "file_delete").length;
    },
  );
  const deleteButton = page.getByRole("button", { name: "Review delete selected" });
  await expect(deleteButton).toBeEnabled();
  await activate(deleteButton);
  const deletePrompt = page.locator(".confirmationPrompt").last();
  await expect(deletePrompt).toContainText("Delete path");
  await expect(deletePrompt).toContainText("/etc/app.conf");
  await expect(deletePrompt).toContainText("Privilege");
  await expect(
    page.evaluate(
      () => {
        const requests = (window as unknown as {
          __vpsmanTestRequests: { fileBrowserJobs: Array<{ operation?: { type?: string } }> };
        }).__vpsmanTestRequests.fileBrowserJobs;
        return requests.filter((request) => request.operation?.type === "file_delete").length;
      },
    ),
  ).resolves.toBe(deleteRequestsBeforeReview);
  await activate(deletePrompt.getByRole("button", { name: "Delete path", exact: true }));
  await expect(page.getByText("Delete /etc/app.conf completed", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Refresh", exact: true })).toBeEnabled();

  await page.getByLabel("Remote path").fill("/empty");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByText("No entries under /empty").first()).toBeVisible();
  await expect(page.getByLabel("File browser directory state")).toContainText("0 of 0 entries");

  await page.getByLabel("Remote path").fill("/large");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByLabel("Current directory entries").getByRole("button", { name: /log-249\.txt/ })).toBeVisible();
  await expect(page.getByLabel("File browser directory state")).toContainText("250 of 320 entries");
  await expect(page.getByLabel("File browser directory state")).toContainText("320 scanned, capped at 320");

  await page.getByLabel("Remote path").fill("/root/blocked");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByText(/Permission denied loading \/root\/blocked/).first()).toBeVisible();
  await expect(page.getByLabel("File browser directory state")).toContainText("permission denied");

  await page.getByLabel("Remote path").fill("/var/log/bird");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await page.getByLabel("Current directory entries").getByRole("button", { name: /bird\.log 1\.0 MiB/ }).click();
  const downloadEvent = page.waitForEvent("download");
  await activate(page.getByRole("button", { name: "Download selected" }));
  await downloadEvent;
  await expect(page.getByLabel("File transfer output handoff")).toContainText("1 related transfer sessions, 1 handoff ready");
  await activate(page.getByRole("button", { name: "Open transfer handoffs" }));
  await expect(page.getByRole("heading", { level: 1, name: "Transfers" })).toBeVisible();
  await expect(page.getByLabel("Focused transfer handoff")).toContainText("/var/log/bird/bird.log");
  await expect(page.getByLabel("Focused transfer handoff")).toContainText("2 matching sessions");
  await expect(page.getByLabel("Focused transfer handoff")).toContainText("2 handoff ready");
  await expect(page.getByLabel("Transfer sessions data grid")).toContainText("5 of 5 transfers");
  await expect(page.getByLabel("Transfer sessions data grid")).toContainText("/var/log/bird/bird.log");
  await expect(page.getByLabel("Transfer sessions data grid")).toContainText("edge-sfo-01");

  const fileBrowserRequests = await page.evaluate(() => {
    const requests = (window as unknown as {
      __vpsmanTestRequests: { fileBrowserJobs: Array<Record<string, unknown>> };
    }).__vpsmanTestRequests.fileBrowserJobs;
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
    const requests = (window as unknown as {
      __vpsmanTestRequests: {
        fileBrowserJobs: Array<{
          confirmed?: boolean;
          destructive?: boolean;
          operation?: { path?: string; type?: string };
          privileged?: boolean;
          selector_expression?: string;
        }>;
      };
    }).__vpsmanTestRequests.fileBrowserJobs;
    return requests.find((request) => request.operation?.type === "file_delete");
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

test("home exposes fleet scan, target quick actions, attention, and activity", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Fleet command home" })).toBeVisible();
  await expect(page.getByLabel("Home posture strip")).toContainText("Online");
  const quickActions = page.getByLabel("Home quick actions");
  await expect(quickActions.getByLabel("Home quick action target")).toBeVisible();
  for (const action of [
    "Open terminal",
    "Browse files",
    "Dispatch command",
    "Run backup",
    "View network",
  ]) {
    await expect(quickActions.getByRole("button", { name: action })).toBeEnabled();
  }

  const scan = page.getByLabel("Home fleet scan");
  await expect(scan).toBeVisible();
  await expect(scan.getByRole("button", { name: /edge-sfo-01/ }).first()).toBeVisible();
  await expect(scan.getByLabel(/Operational signals for edge-sfo-01/)).toContainText("Jobs");
  await expect(scan.getByLabel(/Operational signals for edge-sfo-01/)).toContainText("3 running");
  await expect(scan.getByLabel(/Operational signals for edge-sfo-01/)).toContainText("Backup");
  await expect(scan.getByLabel(/Operational signals for edge-sfo-01/)).toContainText("1 recorded");
  await expect(scan.getByLabel(/Operational signals for edge-sfo-01/)).toContainText("Transfer");
  await expect(scan.getByLabel(/Operational signals for edge-sfo-01/)).toContainText("1 failed");
  await expect(scan.getByLabel(/Operational signals for core-fra-02/)).toContainText("1 critical");
  await expect(scan.getByLabel(/Operational signals for backup-nyc-03/)).toContainText("1 warning");
  await expect(scan.getByRole("button", { name: /Terminal/ }).first()).toBeVisible();
  await expect(scan.getByRole("button", { name: /Files/ }).first()).toBeVisible();
  await expect(scan.getByLabel("More actions for edge-sfo-01")).toBeVisible();
  await expect(scan.getByRole("button", { name: /Backup|Network/ })).toHaveCount(0);

  await expect(page.getByRole("heading", { name: "Needs attention" })).toBeVisible();
  const attentionPanel = homeAttentionPanel(page);
  await expect(attentionPanel.getByRole("button", { name: /Tunnel adapter status failed/ })).toBeVisible();
  await expect(attentionPanel.getByRole("button", { name: /backup-nyc-03 needs review/ })).toBeVisible();
  await expect(attentionPanel.getByRole("button", { name: /Gateway event drops need review/ })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Recent activity" })).toBeVisible();
  await expect(homeActivityPanel(page).getByRole("button", { name: /privilege unlock/ })).toBeVisible();

  const homeTelemetry = page.getByLabel("Home telemetry widgets");
  await expect(homeTelemetry).toBeVisible();
  await expect(homeTelemetry.getByLabel("Home statistic controls")).toBeVisible();
  await expect(homeTelemetry.getByLabel("Dashboard statistic controls")).toHaveCount(0);
});

test("home quick actions route to release pages with selected VPS scope", async ({ page }) => {
  await clickHomeQuickAction(page, "Open terminal");
  await expect(page.getByRole("heading", { name: "Terminal", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Terminal sessions" })).toBeVisible();

  await clickHomeQuickAction(page, "Browse files");
  await expect(page.getByRole("heading", { name: "Files", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "File browser" })).toBeVisible();

  await clickHomeQuickAction(page, "Dispatch command");
  await expect(page.getByRole("heading", { name: "Command dispatch" })).toBeVisible();
  await expect(
    page.getByRole("searchbox", { name: "Bulk target selector expression" }),
  ).toHaveText("id:agent-sfo-01");

  await clickHomeQuickAction(page, "Run backup");
  await expect(page.getByRole("heading", { name: "Backup requests" })).toBeVisible();

  await clickHomeQuickAction(page, "View network");
  await expect(page.getByRole("heading", { name: "Network graph" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Topology graph" })).toBeVisible();
});

test("home attention queue links to release evidence pages", async ({ page }) => {
  await page.goto("/");

  await homeAttentionPanel(page).getByRole("button", { name: /Tunnel adapter status failed/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network evidence" })).toBeVisible();

  await page.goto("/");
  await homeAttentionPanel(page).getByRole("button", { name: /Transfer .*error\.log/ }).click();
  await expect(page.getByRole("heading", { name: "Transfers", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "File transfer sessions" })).toBeVisible();

  await page.goto("/");
  await homeAttentionPanel(page).getByRole("button", { name: /backup-nyc-03 needs review/ }).click();
  await expect(page.getByRole("heading", { name: "Instance detail" })).toBeVisible();

  await page.goto("/");
  await homeAttentionPanel(page).getByRole("button", { name: /Gateway event drops need review/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "System capacity" })).toBeVisible();
});

test("home shows a useful empty state when no VPS agents are loaded", async ({ page }) => {
  await installConsoleApiMock(page, { agentListOverride: [] });
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Fleet command home" })).toBeVisible();
  await expect(page.getByLabel("Home quick action target")).toBeVisible();
  await expect(page.getByLabel("Home quick actions").getByRole("button", { name: "Open terminal" })).toBeDisabled();
  await expect(page.getByLabel("Home posture strip")).toContainText("0/0");
  await expect(page.getByText("No VPS cards to show")).toBeVisible();
  await expect(page.getByText("Adjust the fleet scope or wait for agents to report telemetry.")).toBeVisible();
});

test("home monitor card text fits desktop tablet and mobile widths", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "viewport sweep explicitly covers mobile width from the desktop project",
  );
  for (const viewport of [
    { height: 900, label: "desktop", width: 1440 },
    { height: 900, label: "tablet", width: 900 },
    { height: 844, label: "mobile", width: 390 },
  ]) {
    await page.setViewportSize({ height: viewport.height, width: viewport.width });
    await page.goto("/");
    const scan = page.getByLabel("Home fleet scan");
    await expect(scan).toBeVisible();
    await expect(scan.locator(".vpsMonitorCard").first()).toBeVisible();
    await expectMonitorCardsToFit(page, viewport.label);
  }
});

test("fleet monitor renders VPS card workflow actions", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop card action routing covers the detailed monitor interaction model",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Monitor");

  await expect(page.getByRole("heading", { name: "Fleet monitor" })).toBeVisible();
  const monitor = page.getByLabel("VPS monitor cards");
  await expect(monitor).toBeVisible();
  await expect(page.getByLabel("VPS cards controls")).toBeVisible();
  await page.getByLabel("VPS cards sort").selectOption("traffic");
  await expect(monitor).toHaveAttribute("data-sort", "traffic");
  await page.getByLabel("VPS cards density").getByRole("button", { name: "Compact" }).click();
  await expect(monitor).toHaveAttribute("data-density", "compact");

  const edgeCard = monitor.locator(".vpsMonitorCard", { hasText: "edge-sfo-01" }).first();
  await expect(edgeCard.getByText("online").first()).toBeVisible();
  await expect(edgeCard.getByLabel("Tags for edge-sfo-01")).toContainText("provider:alpha");
  await expect(edgeCard.getByLabel("Operational signals for edge-sfo-01")).toContainText("Jobs");
  await expect(edgeCard.getByRole("button", { name: /Terminal/ })).toBeVisible();
  await expect(edgeCard.getByRole("button", { name: /Files/ })).toBeVisible();
  await expect(edgeCard.getByRole("button", { name: /Processes/ })).toBeVisible();
  await expect(edgeCard.getByLabel("More actions for edge-sfo-01")).toBeVisible();
  await expect(edgeCard.getByRole("button", { name: /Backup|Network/ })).toHaveCount(0);

  for (const action of [
    {
      click: async (card: Locator) => card.getByRole("button", { name: /edge-sfo-01/ }).first().click(),
      heading: "Instance detail",
    },
    {
      click: async (card: Locator) => card.getByRole("button", { name: "Terminal" }).click(),
      heading: "Terminal",
    },
    {
      click: async (card: Locator) => card.getByRole("button", { name: "Files" }).click(),
      heading: "Files",
    },
    {
      click: async (card: Locator) => card.getByRole("button", { name: "Processes" }).click(),
      heading: "Processes",
    },
    {
      click: async (card: Locator) => {
        await card.getByLabel("More actions for edge-sfo-01").click();
        await card.getByRole("button", { name: "Backup" }).click();
      },
      heading: "Backup requests",
    },
    {
      click: async (card: Locator) => {
        await card.getByLabel("More actions for edge-sfo-01").click();
        await card.getByRole("button", { name: "Network" }).click();
      },
      heading: "Network graph",
    },
  ]) {
    await page.goto("/");
    await openConsoleSubpage(page, "Fleet", "Monitor");
    const card = page
      .getByLabel("VPS monitor cards")
      .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" })
      .first();
    await action.click(card);
    await expect(
      page.getByRole("heading", { level: 1, name: action.heading, exact: true }),
    ).toBeVisible();
    await expect(page.locator("body")).toContainText("edge-sfo-01");
  }
});

for (const fixtureCount of [0, 3, 20, 50, 100]) {
  test(`fleet monitor cards remain readable for ${fixtureCount} generated VPS fixtures`, async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "desktop fixture-count sweep covers high-card rendering without doubling suite time",
    );
    await installConsoleApiMock(page, {
      agentListOverride: makeMonitorAgentFixtures(fixtureCount),
    });
    await page.setViewportSize({ height: 900, width: 1280 });
    await page.goto("/");
    await openConsoleSubpage(page, "Fleet", "Monitor");

    const monitor = page.getByLabel("VPS monitor cards");
    if (fixtureCount === 0) {
      await expect(page.getByText("No VPS cards to show")).toBeVisible();
      await expect(monitor).toHaveCount(0);
      return;
    }
    await expect(monitor).toBeVisible();
    await expect(monitor.locator(".vpsMonitorCard")).toHaveCount(fixtureCount);
    await page.getByLabel("VPS cards density").getByRole("button", { name: "Compact" }).click();
    await expect(monitor).toHaveAttribute("data-density", "compact");
    await expectMonitorCardsToFit(page, `${fixtureCount} generated VPS`);
  });
}

test("fleet groups expose registry assignments and reviewed bulk mutation evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk group mutation review is covered through the desktop operator workflow",
  );
  await page.goto("/");

  await openConsoleSubpage(page, "Fleet", "Groups");
  await expect(page.getByRole("heading", { level: 1, name: "Fleet groups" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Fleet groups" })).toBeVisible();
  await expect(page.getByLabel("Fleet group counts")).toContainText("provider groups");
  await expect(page.getByLabel("Fleet group counts")).toContainText("country groups");
  await expect(page.getByText("Tag registry")).toBeVisible();

  await openConsoleSubpage(page, "Fleet", "Assignments");
  await expect(page.getByRole("heading", { level: 1, name: "Group assignments" })).toBeVisible();
  await expect(page.getByText("VPS tag assignments")).toBeVisible();

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await expect(page.getByRole("heading", { level: 1, name: "Bulk groups" })).toBeVisible();
  await page.getByLabel("Bulk tag", { exact: true }).fill("maintenance:test");
  await page
    .getByRole("searchbox", { name: "Bulk tag selector expression" })
    .fill("id:agent-sfo-01");
  await activate(page.getByRole("button", { name: "Preview targets" }));

  const evidence = page.getByLabel("Bulk group preview evidence");
  await expect(evidence).toBeVisible();
  await expect(evidence).toContainText("selected");
  await expect(evidence).toContainText("changed");
  await expect(evidence).toContainText("schedule impacts");
  await expect(evidence).toContainText("preview hash");
  await expect(evidence).toContainText("7".repeat(64));
  await expect(page.locator(".bulkTagPreview")).toContainText("edge-sfo-01");

  await activate(page.getByRole("button", { name: "Review mutation" }));
  const confirmation = page.getByLabel("Confirm tag mutation");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Preview hash");
  await activate(confirmation.getByRole("button", { name: "Apply tag mutation" }));

  const requests = await page.evaluate(() => {
    const requestLog = (
      window as unknown as {
        __vpsmanTestRequests: { bulkTagMutations: Array<Record<string, unknown>> };
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

test("fleet instance row actions expose release VPS workflows", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "fleet grid action menu is covered through desktop data-grid behavior",
  );
  for (const action of [
    { label: "Open detail", heading: "Instance detail" },
    { label: "Open terminal", heading: "Terminal" },
    { label: "Open files", heading: "Files" },
    { label: "Open processes", heading: "Processes" },
    { label: "Open backups", heading: "Backup requests" },
    { label: "Open network", heading: "Network graph" },
  ]) {
    await page.goto("/");
    await openConsoleSubpage(page, "Fleet", "Instances");

    const grid = page.getByLabel("VPS instance records data grid");
    const edgeRow = grid.locator(".gridBody [role=row]", { hasText: "edge-sfo-01" }).first();
    await edgeRow.getByLabel("Select VPS instance records row").check();
    await grid.getByRole("button", { name: /^Actions$/ }).click();
    await page.getByRole("menuitem", { name: action.label }).click();
    await expect(
      page.getByRole("heading", { level: 1, name: action.heading, exact: true }),
    ).toBeVisible();
    await expect(page.locator("body")).toContainText("edge-sfo-01");
  }
});

test("fleet instance detail is the canonical VPS route from release workflows", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "cross-page dense grid and graph entry points are covered in desktop workflow tests",
  );

  await page.goto("/");
  await page
    .getByLabel("Home fleet scan")
    .locator(".vpsMonitorCard", { hasText: "edge-sfo-01" })
    .first()
    .getByRole("button", { name: /edge-sfo-01/ })
    .first()
    .click();
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Fleet", "Instances");
  const grid = page.getByLabel("VPS instance records data grid");
  const edgeRow = grid.locator(".gridBody [role=row]", { hasText: "edge-sfo-01" }).first();
  await edgeRow.getByLabel("Select VPS instance records row").check();
  await grid.getByRole("button", { name: /^Actions$/ }).click();
  await page.getByRole("menuitem", { name: "Open detail" }).click();
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Fleet", "Alerts");
  await activate(page.getByLabel("Expand Fleet alerts row fleet-alert-network-agent-fra-02-tun0"));
  await activate(page.getByRole("button", { name: "Open VPS detail" }));
  await expectCanonicalVpsDetail(page, "core-fra-02");

  await openConsoleSubpage(page, "Network", "Graph");
  await expect(page.getByRole("heading", { level: 1, name: "Network graph" })).toBeVisible();
  await activate(page.locator(".topologyNodeInspector").getByRole("button", { name: "Open VPS detail" }));
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Jobs", "History");
  const jobsGrid = page.getByLabel("Job records data grid");
  await activate(jobsGrid.locator(".gridBody [role=row]").first());
  const targetGrid = page.getByLabel("Target result records data grid");
  const targetRow = targetGrid.locator(".gridBody [role=row]", { hasText: "edge-sfo-01" }).first();
  await activate(targetRow.getByRole("button", { name: "Open VPS detail" }));
  await expectCanonicalVpsDetail(page, "edge-sfo-01");

  await openConsoleSubpage(page, "Backups", "Requests");
  await activate(page.getByRole("button", { name: "Open backup request", exact: true }));
  const backupWorkflow = page.getByLabel("Open backup request");
  await chooseVpsBySearch(backupWorkflow, "Backup client", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await activate(backupWorkflow.locator(".backupContextActions").getByRole("button", { name: "Open VPS detail" }));
  await expectCanonicalVpsDetail(page, "edge-sfo-01");
});

test("fleet instances table keeps dense grid controls and routes card view separately", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop grid controls are covered separately from mobile navigation",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  await expect(page.getByLabel("Saved fleet views")).toBeVisible();
  await expect(grid.getByLabel("VPS instance records search")).toBeVisible();
  await expect(grid.getByLabel("Fleet instance view mode")).toContainText("Table");
  await expect(grid.getByRole("button", { name: "Cards" })).toBeVisible();

  await grid.getByLabel("VPS instance records search").fill("edge-sfo-01");
  await expect(grid.locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })).toBeVisible();
  await expect(grid.locator(".gridBody [role=row]", { hasText: "core-fra-02" })).toHaveCount(0);
  await grid.getByLabel("VPS instance records search").fill("");

  await grid.getByRole("button", { name: "Name", exact: true }).click();
  await grid.getByRole("button", { name: "VPS instance records columns" }).click();
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  await page.keyboard.press("Escape");
  await expect(grid.getByRole("columnheader", { name: /Provider/ })).toHaveCount(0);

  const edgeRow = grid.locator(".gridBody [role=row]", { hasText: "edge-sfo-01" }).first();
  await edgeRow.getByLabel("Select VPS instance records row").check();
  await expect(grid).toContainText("1 selected");
  await expect(grid.locator(".gridExpandedRow")).toHaveCount(0);
  await expect(grid.getByRole("heading", { name: /Terminal sessions|Files|Processes/ })).toHaveCount(0);

  await grid.getByRole("button", { name: "Cards" }).click();
  await expect(page.getByRole("heading", { name: "Fleet monitor" })).toBeVisible();

  await openConsoleSubpage(page, "Fleet", "Instances");
  const restoredGrid = page.getByLabel("VPS instance records data grid");
  await expect(restoredGrid.getByRole("columnheader", { name: /Provider/ })).toHaveCount(0);
});

test("command palette indexes release pages and fixture entities", async ({ page }) => {
  await page.goto("/");

  await page.keyboard.press("Control+K");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  const search = page.getByLabel("Command palette search");
  await search.fill("Remote Operations Terminal");
  await expect(
    palette.locator('[data-command-group="Page"]').filter({ hasText: "Remote Operations / Terminal" }),
  ).toBeVisible();
  await palette.getByRole("option", { name: /Page: Remote Operations \/ Terminal/ }).click();
  await expect(page.getByRole("heading", { name: "Terminal" })).toBeVisible();

  await expectCommandPaletteGroup(page, "VPS", "edge-sfo");
  await expectCommandPaletteGroup(page, "Job", "network_speed_test");
  await expectCommandPaletteGroup(page, "Terminal", "61616161");
  await expectCommandPaletteGroup(page, "Transfer", "bird.log");
  await expectCommandPaletteGroup(page, "Backup", "fixture backup");
  await expectCommandPaletteGroup(page, "Audit", "privilege_unlock");
  await expectCommandPaletteGroup(page, "Schedule", "edge-health-hourly");
});

test("command palette entity selections use release route helpers", async ({ page }) => {
  await page.goto("/");

  await selectCommandPaletteResult(page, "VPS", "edge-sfo");
  await expect(page.getByRole("heading", { name: "Instance detail" })).toBeVisible();

  await selectCommandPaletteResult(page, "Job", "network_speed_test");
  await expect(page.getByRole("heading", { level: 1, name: "Job history" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Target results" })).toBeVisible();

  await selectCommandPaletteResult(page, "Terminal", "61616161");
  await expect(page.getByRole("heading", { name: "Terminal", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Terminal sessions" })).toBeVisible();

  await selectCommandPaletteResult(page, "Audit", "privilege_unlock");
  await expect(page.getByRole("heading", { name: "Audit events" })).toBeVisible();
});

test("jobs approvals and scheduled runs stay separate", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Approvals");

  await expect(page.getByRole("heading", { level: 1, name: "Approvals" })).toBeVisible();
  await expect(page.getByText("1 pending · 1 reviewed requests")).toBeVisible();
  await expect(page.getByText("Job approval queue")).toBeVisible();
  await expect(page.getByText("noc-operator")).toBeVisible();
  await expect(page.getByText("destructive")).toBeVisible();
  await expect(page.getByText("worker-created due runs")).toHaveCount(0);

  await openConsoleSubpage(page, "Jobs", "Scheduled runs");
  await expect(page.getByRole("heading", { level: 1, name: "Scheduled runs" })).toBeVisible();
  await expect(page.getByText("1 worker-created due runs")).toBeVisible();
  await expect(page.getByText("Worker automation")).toBeVisible();
  await expect(page.getByText("Retry/worker health not exposed")).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Review create" })).toHaveCount(0);

  await page.getByRole("button", { name: "Open schedule registry" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Schedules" })).toBeVisible();
  await page.getByRole("button", { name: "Scheduled runs" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Scheduled runs" })).toBeVisible();
});

test("advanced release labels provide inline expert help", async ({ page }) => {
  await page.goto("/");

  await openConsoleSubpage(page, "Config", "Bulk patch");
  await expect(page.getByLabel("Incremental patch help")).toHaveAttribute("title", /Incremental TOML patches/);
  await expect(page.getByLabel("Targets help")).toHaveAttribute("title", /Selector expressions freeze/);
  await expect(page.getByLabel("Max timeout seconds help").first()).toHaveAttribute("title", /Per-target command timeout/);
  await expect(page.getByText(/Saved generators render incremental TOML/)).toBeVisible();

  await openConsoleSubpage(page, "Config", "Per-VPS");
  await expect(page.getByLabel("Redacted runtime TOML help")).toHaveAttribute("title", /secret material removed/);
  await expect(page.getByLabel("Guarded one-VPS override help")).toHaveAttribute("title", /base hash/);
  await expect(page.locator('strong[title*="Hash of the redacted config read"]')).toBeVisible();
  await expect(page.locator('strong[title*="Top-level TOML sections"]')).toBeVisible();
  await expect(page.locator('strong[title*="Hash of the exact override payload"]')).toBeVisible();

  await openConsoleSubpage(page, "Config", "Rules");
  await expect(page.getByLabel("Bulk rule editor help")).toHaveAttribute("title", /accounting and alert policies/);
  await expect(page.locator('h4[title*="Fleet selector used for the dry-run"]')).toBeVisible();
  await expect(page.locator('h4[title*="Key=value lines become typed"]')).toBeVisible();
  await expect(page.locator('h4[title*="Explicit rule keys removed"]')).toBeVisible();
});

test("visible disabled release controls explain their disabled reason", async ({ page }) => {
  await page.goto("/");

  for (const route of releaseAccessibilityRoutes) {
    await openConsoleSubpage(page, route.view, route.subpage);
    const missingReasons = await visibleDisabledControlsWithoutReason(page);
    expect(missingReasons, `${route.view} / ${route.subpage}`).toEqual([]);
  }
});

test("release console text colors preserve WCAG AA contrast", async ({ page }) => {
  await page.goto("/");

  for (const route of releaseAccessibilityRoutes) {
    await openConsoleSubpage(page, route.view, route.subpage);
    const failures = await contrastFailures(page);
    expect(failures, `${route.view} / ${route.subpage}`).toEqual([]);
  }
});

test("automation runbooks promotes command templates into reviewed catalog", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Runbooks");

  await expect(page.getByRole("heading", { level: 1, name: "Runbooks" })).toBeVisible();
  await expect(page.getByLabel("Runbook catalog summary")).toContainText("Runbooks");
  await expect(page.getByLabel("Runbook catalog summary")).toContainText("Ready");

  const catalog = page.getByLabel("Runbook catalog", { exact: true });
  await expect(catalog).toContainText("Default shell command");
  await expect(catalog).toContainText("edge-health-check");
  await expect(catalog.getByLabel("Required review for edge-health-check")).toContainText("target scope");
  await expect(catalog.getByLabel("Required review for edge-health-check")).toContainText("argv review");
  await expect(catalog).toContainText("Last evidence");
  await expect(page.getByLabel("Runbook filters")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Dispatch command" })).toHaveCount(0);

  const edgeRunbook = catalog.locator(".runbookCard", { hasText: "edge-health-check" });
  await activate(edgeRunbook.getByRole("button", { name: "Review in Dispatch" }));
  await expect(page.getByRole("heading", { level: 1, name: "Command dispatch" })).toBeVisible();

  const composer = page.locator(".commandComposer", {
    has: page.getByRole("heading", { name: "Dispatch command" }),
  });
  await expect(composer.getByLabel("Template selector")).toHaveValue("46464646-5656-4789-8abc-defdefdefdef");
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("tag:provider:alpha");
});

test("jobs artifacts is read-only inventory linked to source workflows", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Artifacts");

  await expect(page.getByRole("heading", { level: 1, name: "Job artifacts" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Job artifacts" })).toBeVisible();

  const summary = page.getByLabel("Job artifact inventory summary");
  await expect(summary).toContainText("Domains");
  await expect(summary).toContainText("Records");
  await expect(summary).toContainText("Stored bytes");
  await expect(summary).toContainText("Cleanup boundary");
  await expect(summary).toContainText("System / Maintenance");

  const grid = page.getByLabel("Job artifact inventory data grid");
  await expect(grid).toContainText("backup");
  await expect(grid).toContainText("file_transfer_source");
  await expect(grid).toContainText("agent_update");
  await expect(grid).toContainText("Backups / Artifacts");
  await expect(grid).toContainText("Remote Operations / Transfers");
  await expect(grid).toContainText("Automation / Agent updates");

  await expect(page.getByRole("button", { name: "Queue cleanup" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Preview cleanup" })).toHaveCount(0);

  await grid.getByRole("button", { name: "Backups / Artifacts" }).first().click();
  await expect(page.getByRole("heading", { level: 1, name: "Backup artifacts" })).toBeVisible();

  await openConsoleSubpage(page, "Jobs", "Artifacts");
  const sourceLinks = page.getByLabel("Artifact source workflow links");
  await sourceLinks.getByRole("button", { name: "Remote Operations / Transfers" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Transfers" })).toBeVisible();

  await openConsoleSubpage(page, "Jobs", "Artifacts");
  await page.getByLabel("Artifact source workflow links").getByRole("button", { name: "Automation / Agent updates" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Agent updates" })).toBeVisible();
});

test("automation owns agent update rollout, health, and rollback posture", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Agent updates");

  await expect(page.getByRole("heading", { level: 1, name: "Agent updates" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Agent update registry" })).toBeVisible();

  const posture = page.getByLabel("Agent update rollout posture");
  await expect(posture).toContainText("Registered releases");
  await expect(posture).toContainText("Current fleet versions");
  await expect(posture).toContainText("Canary / staging");
  await expect(posture).toContainText("Rollout policy");
  await expect(posture).toContainText("Health checks");
  await expect(posture).toContainText("Rollback");
  await expect(posture).toContainText("Version telemetry unavailable");
  await expect(posture).toContainText("agent update");

  await posture.getByRole("button", { name: "Open update jobs" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Job history" })).toBeVisible();

  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await expect(mobilePageSelector).not.toContainText("Jobs / Update registry");
  } else {
    const nav = page.getByRole("navigation", { name: "Primary console navigation" });
    await activate(nav.getByRole("button", { name: "Jobs", exact: true }).first());
    await expect(
      nav.getByLabel("Jobs sections").getByRole("button", { name: "Update registry", exact: true }),
    ).toHaveCount(0);
  }
});

test("config overview focuses on drift risk and routes to config workflows", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Overview");

  await expect(page.getByRole("heading", { name: "Runtime config overview" })).toBeVisible();
  const health = page.getByLabel("Config health posture");
  await expect(health).toContainText("Config health");
  await expect(health).toContainText("Action required");
  await expect(health).toContainText("failed runtime syncs");
  await expect(health).toContainText("source checks needing review");

  const drift = page.getByLabel("Config drift summary");
  await expect(drift).toContainText("Runtime apply drift");
  await expect(drift).toContainText("Source readiness drift");
  await expect(drift).toContainText("Rule validation drift");

  const coverage = page.getByLabel("Config template coverage");
  await expect(coverage).toContainText("Template coverage");
  await expect(coverage).toContainText("templates");
  await expect(coverage).toContainText("assignments");
  await expect(coverage).toContainText("Automation / Source Templates");

  const applyState = page.getByLabel("Config apply-state summary");
  await expect(applyState).toContainText("Applied runtime state");
  await expect(applyState).toContainText("Queued apply");
  await expect(applyState).toContainText("Failed apply");

  await expect(page.getByLabel("Recent config changes")).toContainText("Apply state");
  await expect(page.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(page.getByLabel("VPS config target")).toHaveCount(0);
  await expect(page.getByLabel("Patch generators data grid")).toHaveCount(0);
  await expect(page.getByRole("button", { exact: true, name: "Review apply" })).toHaveCount(0);

  const links = page.getByLabel("Config overview workflow links");
  for (const label of ["Per-VPS", "Bulk patch", "Templates", "Rules"]) {
    await expect(links.getByRole("button", { name: new RegExp(label) })).toBeVisible();
  }

  await links.getByRole("button", { name: /Per-VPS/ }).click();
  await expect(page.getByRole("heading", { name: "Per-VPS config" })).toBeVisible();

  await openConsoleSubpage(page, "Config", "Overview");
  await page.getByLabel("Config overview workflow links").getByRole("button", { name: /Bulk patch/ }).click();
  await expect(page.getByRole("heading", { name: "Bulk patch" })).toBeVisible();

  await openConsoleSubpage(page, "Config", "Overview");
  await page.getByLabel("Config overview workflow links").getByRole("button", { name: /Templates/ }).click();
  await expect(page.getByRole("heading", { name: "Templates" })).toBeVisible();
  await expect(page.getByLabel("Config template summary")).toBeVisible();

  await openConsoleSubpage(page, "Config", "Overview");
  await page.getByLabel("Config overview workflow links").getByRole("button", { name: /Rules/ }).click();
  await expect(page.getByRole("heading", { name: "VPS Rules" })).toBeVisible();
});

test("config templates summarizes coverage and links to canonical automation authoring", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Templates");

  await expect(page.getByRole("heading", { name: "Templates" })).toBeVisible();
  await expect(page.getByLabel("Config template summary")).toContainText("Template coverage");
  await expect(page.getByLabel("Source template canonical home")).toContainText("Automation / Source Templates");
  await expect(page.getByLabel("Template domain coverage data grid")).toBeVisible();
  await expect(page.getByLabel("Config source readiness exceptions")).toBeVisible();
  await expect(page.getByLabel("Template registry data grid")).toHaveCount(0);
  await expect(page.getByLabel("Template assignment target expression")).toHaveCount(0);
  await expect(page.getByText("Template definition")).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Open Source Templates" }));
  await expect(page.getByText("vpsman / Automation / Source templates")).toBeVisible();
  await expect(page.locator(".sourceTemplatePanel").getByRole("heading", { name: "Templates" })).toBeVisible();
  await expect(page.getByLabel("Template registry data grid")).toBeVisible();
  await expect(page.getByLabel("Template assignment target expression")).toBeVisible();
});

test("config rules show affected alert policy context and route to alerts", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Rules");

  await expect(page.getByRole("heading", { name: "VPS Rules" })).toBeVisible();
  await expect(page.getByLabel("VPS rule values data grid")).toBeVisible();
  const alertContext = page.getByLabel("Affected alert policy context");
  await expect(alertContext).toContainText("Affected alert policies");
  await expect(alertContext).toContainText("edge-resource-policy");
  await expect(alertContext).toContainText("80% total quota");
  await expect(alertContext).toContainText("traffic.quota.total");
  await activate(alertContext.getByRole("button", { name: "Open Observability alerts" }));
  await expect(page.getByText("vpsman / Observability / Alerts")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1, name: "Alerts" })).toBeVisible();
});

test("observability alerts and webhooks are explicit separate pages", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "alert and webhook registries are dense desktop operator workflows",
  );
  await page.goto("/");

  await openConsoleSubpage(page, "Fleet", "Alerts");
  await expect(page.getByRole("heading", { level: 1, name: "Fleet alerts" })).toBeVisible();
  await expect(page.getByLabel("Fleet alerts", { exact: true })).toContainText("Tunnel adapter status failed");
  await expect(page.getByRole("button", { name: "Create policy" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Notification channels" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Webhook rules" })).toHaveCount(0);
  await page.getByLabel("Select Fleet alerts row fleet-alert-network-agent-fra-02-tun0").check();
  await page.locator(".fleetAlertList").getByRole("button", { name: /Actions/ }).click();
  await page.getByRole("menuitem", { name: "Acknowledge open" }).click();
  const triageConfirmation = page.getByLabel("Confirm fleet alert triage");
  await expect(triageConfirmation).toBeVisible();
  await expect(triageConfirmation).toContainText("Tunnel adapter status failed");
  await activate(triageConfirmation.getByRole("button", { name: "Acknowledge" }));
  const triageRequests = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { fleetAlertStates: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests;
    return requests.fleetAlertStates;
  });
  expect(triageRequests.at(-1)).toMatchObject({
    action: "acknowledge",
    alert_id: "fleet-alert-network-agent-fra-02-tun0",
    confirmed: true,
  });

  await activate(page.getByLabel("Expand Fleet alerts row fleet-alert-network-agent-fra-02-tun0"));
  await activate(page.getByRole("button", { name: "Open VPS detail" }));
  await expect(page.getByRole("heading", { level: 1, name: "Instance detail" })).toBeVisible();
  await expect(page.locator("body")).toContainText("core-fra-02");

  await openConsoleSubpage(page, "Fleet", "Alerts");
  await activate(page.getByLabel("Expand Fleet alerts row fleet-alert-network-agent-fra-02-tun0"));
  await activate(page.getByLabel("Fleet alerts data grid").getByRole("button", { name: "Open alert policies" }));
  await expect(page.getByRole("heading", { level: 1, name: "Alerts" })).toBeVisible();

  await expect(page.getByText("vpsman / Observability / Alerts")).toBeVisible();
  const alertSummary = page.getByLabel("Alert routing summary");
  await expect(alertSummary).toContainText("Active fleet alerts");
  await expect(alertSummary).toContainText("Policy groups");
  await expect(alertSummary).toContainText("Channels");
  await expect(page.getByRole("heading", { name: "Alert policies" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Create policy" })).toBeVisible();
  await expect(page.getByText("edge-resource-policy")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Notification channels" })).toBeVisible();
  await expect(page.getByLabel("Alert notification channels data grid").getByText("edge-webhook-channel")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Webhook rules" })).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Active triage queue" }));
  await expect(page.getByRole("heading", { level: 1, name: "Fleet alerts" })).toBeVisible();

  await openConsoleSubpage(page, "Observability", "Webhooks");
  await expect(page.getByText("vpsman / Observability / Webhooks")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1, name: "Webhooks" })).toBeVisible();
  await expect(page.getByLabel("Webhook routing summary")).toContainText("Webhook rules");
  await expect(page.getByRole("heading", { name: "Webhook rules" })).toBeVisible();
  await expect(page.getByLabel("Webhook rules data grid").getByText("edge-interval-webhook")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Webhook deliveries" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Webhook delivery maintenance" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Notification channels" })).toHaveCount(0);
});

test("config bulk patch requires reviewed scope and privilege before apply", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk patch review is a dense desktop workflow covered by desktop release tests",
  );
  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.config.bulk.selectorExpression"));
  await openConsoleSubpage(page, "Config", "Bulk patch");

  await expect(page.getByRole("heading", { name: "Bulk patch" })).toBeVisible();
  const bulk = page.locator(".configApplyGrid");
  await expect(bulk.getByRole("searchbox", { name: "Bulk patch target expression" })).toBeVisible();
  await expect(bulk.getByRole("button", { name: "Review targets" })).toBeDisabled();
  await expect(bulk.getByRole("button", { exact: true, name: "Review apply" })).toBeDisabled();
  await expect(bulk.locator(".privilegeManager")).toContainText(/Locked|locked/);

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Bulk patch");
  const unlockedBulk = page.locator(".configApplyGrid");
  await unlockedBulk
    .getByRole("searchbox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await activate(unlockedBulk.getByRole("button", { name: "Review targets" }));
  await expect(unlockedBulk).toContainText("1/3");
  await expect(unlockedBulk.getByRole("button", { exact: true, name: "Review apply" })).toBeEnabled();

  await activate(unlockedBulk.getByRole("button", { exact: true, name: "Review apply" }));
  const confirmation = page.getByLabel("Confirm bulk patch");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("id:agent-sfo-01");
  await expect(confirmation).toContainText("Targets");
  await expect(confirmation).toContainText("Payload");
  await activate(confirmation.getByRole("button", { name: "Apply runtime config patch" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { runtimeConfigPatches: Array<Record<string, unknown>> };
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

test("config per-vps preserves guarded one-vps override workflow", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "one-VPS config override is a dense desktop workflow covered by desktop release tests",
  );
  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.config.single.clientId"));
  await openConsoleSubpage(page, "Config", "Per-VPS");

  await expect(page.getByRole("heading", { name: "Per-VPS config" })).toBeVisible();
  const panel = page.locator(".configApplyGrid");
  await expect(panel.getByRole("combobox", { name: "VPS config target" })).toBeVisible();
  await expect(panel.getByLabel("One-VPS runtime config override TOML")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Validate override" })).toBeDisabled();
  await expect(panel.getByRole("button", { name: "Review one-VPS apply" })).toBeDisabled();
  await expect(panel.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(page.getByLabel("Patch generators data grid")).toHaveCount(0);

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const unlockedPanel = page.locator(".configApplyGrid");
  await chooseVpsBySearch(
    unlockedPanel,
    "VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await activate(unlockedPanel.getByRole("button", { name: "Read runtime config" }));
  await expect(unlockedPanel.getByLabel("VPS redacted runtime config TOML")).toHaveValue(/client_id = "agent-fra-02"/);
  await expect(unlockedPanel.getByLabel("One-VPS config override guard")).toContainText("Current base");

  await unlockedPanel.getByLabel("One-VPS runtime config override TOML").fill("[telemetry]\ninterval_secs = 60\n");
  await activate(unlockedPanel.getByRole("button", { name: "Validate override" }));
  await expect(unlockedPanel.getByLabel("One-VPS config override guard")).toContainText("telemetry");
  await expect(unlockedPanel.getByRole("button", { name: "Review one-VPS apply" })).toBeEnabled();
  await activate(unlockedPanel.getByRole("button", { name: "Review one-VPS apply" }));

  const confirmation = page.getByLabel("Confirm one-VPS runtime config override");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("agent-fra-02");
  await expect(confirmation).toContainText("Base hash");
  await expect(confirmation).toContainText("Payload");
  await expect(confirmation).toContainText("telemetry");
  await activate(confirmation.getByRole("button", { name: "Apply one-VPS override" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { runtimeConfigPatches: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
  expect(request?.toml).toContain("interval_secs = 60");
  expect(JSON.stringify(request)).not.toContain("local-super-password");
});

test("observability process metrics does not invent backend history", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Observability", "Process metrics");

  const status = page.getByLabel("Process metrics release status");
  await expect(status).toBeVisible();
  await expect(status).toContainText("Process history and restart trends when backend data exists");
  await expect(status).toContainText("Long-term process history is not exposed by the backend yet.");
  await expect(status.locator(".timeSeriesChartShell")).toHaveCount(0);
});

test("observability fleet metrics owns resource charts and read-only analysis controls", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Observability", "Fleet metrics");

  await expect(page.getByRole("heading", { level: 1, name: "Fleet metrics" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "CPU load by VPS" })).toBeVisible();

  const controls = page.getByLabel("Fleet metrics controls");
  await expect(controls.getByLabel("Fleet metrics time range")).toContainText("24h");
  await expect(controls.getByRole("button", { name: "CPU" })).toHaveClass(/active/);
  await expect(controls.getByLabel("Fleet metrics group by")).toBeVisible();

  const summary = page.getByLabel("Fleet metrics summary");
  await expect(summary).toContainText("Current metric");
  await expect(summary).toContainText("Telemetry freshness");
  await expect(summary).toContainText("Grouping");

  await expect(page.locator(".timeSeriesChartShell")).toBeVisible();
  await expect(page.locator(".timeSeriesLegend")).toContainText("core-fra-02");
  await expect(page.getByLabel("Top resource VPS list")).toContainText("edge-sfo-01");
  await expect(page.getByLabel("Fleet metrics group breakdown")).toContainText("country:US");

  await controls.getByRole("button", { name: "Memory" }).click();
  await expect(page.getByRole("heading", { name: "Memory used by VPS" })).toBeVisible();

  await expect(
    page.locator(".observabilityMetricsPanel").getByRole("button", { name: /Run tests|Apply|Dispatch|Delete|Create/ }),
  ).toHaveCount(0);
});

test("observability network metrics is chart-first and mutation-free", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Observability", "Network metrics");

  await expect(page.getByRole("heading", { level: 1, name: "Network metrics" })).toBeVisible();
  const panel = page.locator(".observabilityNetworkMetricsPanel");
  await expect(panel.getByRole("heading", { name: "Latency, loss, and speed" })).toBeVisible();
  await expect(panel.getByLabel("Network metrics charts")).toContainText("Latency");
  await expect(panel.getByLabel("Network metrics charts")).toContainText("Packet loss");
  await expect(panel.getByLabel("Network metrics charts")).toContainText("Throughput");
  await expect(panel.locator(".timeSeriesLegend").first()).toContainText("sfo-fra-gre");

  await expect(panel.getByLabel("Network metrics tunnel grouping")).toContainText("sfo-fra-gre");
  await expect(panel.getByLabel("Network endpoint comparison")).toContainText("agent-sfo-01");
  await expect(panel.getByLabel("Network metrics alert overlays")).toContainText("OSPF delta");
  await expect(panel.getByRole("button", { name: /Run status|Run probe|Run speed|Apply|Rollback|Dispatch|Delete|Create/ })).toHaveCount(0);

  await panel.getByRole("button", { name: "Open Network tests" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network tests" })).toBeVisible();

  await openConsoleSubpage(page, "Observability", "Network metrics");
  await page.locator(".observabilityNetworkMetricsPanel").getByRole("button", { name: "Open OSPF review" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network OSPF" })).toBeVisible();
});

test("observability dashboards manages saved read-only widget layouts", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Observability", "Dashboards");

  await expect(page.getByRole("heading", { level: 1, name: "Dashboards" })).toBeVisible();
  const panel = page.locator(".observabilityDashboardsPanel");
  await expect(panel.getByLabel("Dashboard manager summary")).toContainText("Saved dashboards");
  await expect(panel.getByLabel("Dashboard manager summary")).toContainText("Widgets");

  const registry = panel.getByLabel("Saved dashboard registry");
  for (const name of ["Fleet operations", "Resource capacity", "Network traffic", "Group posture"]) {
    await expect(registry.getByRole("button", { name: new RegExp(name) })).toBeVisible();
  }

  await expect(panel.getByLabel("Fleet operations dashboard widgets")).toContainText("Recent alerts");
  await expect(panel.getByLabel("Fleet operations dashboard widgets")).toContainText("Degraded VPS");

  await registry.getByRole("button", { name: /Resource capacity/ }).click();
  await expect(panel.getByLabel("Resource capacity dashboard widgets")).toContainText("Top resource VPS");
  await expect(panel.getByLabel("Resource capacity chart widget")).toBeVisible();
  await expect(panel.locator(".timeSeriesChartShell")).toBeVisible();

  await registry.getByRole("button", { name: /Network traffic/ }).click();
  await expect(panel.getByLabel("Network traffic dashboard widgets")).toContainText("Network speed chart");
  await expect(panel.getByLabel("Top network VPS widget table")).toContainText("edge-sfo-01");

  await registry.getByRole("button", { name: /Group posture/ }).click();
  await expect(panel.getByLabel("Group posture dashboard widgets")).toContainText("country:US");

  await expect(panel.getByRole("button", { name: "Share link" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "Export JSON" })).toBeVisible();
  await expect(panel.getByLabel("Dashboard share and export details")).toContainText("No privileged operation controls");
  await expect(panel.getByRole("button", { name: /Open terminal|Run backup|Dispatch|Apply|Delete|Restart|Stop|Create/ })).toHaveCount(0);
});

test("audit events stays read-only with filters and event detail", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Events");

  await expect(page.getByRole("heading", { level: 1, name: "Audit events" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Audit log" })).toBeVisible();
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

  await filters.getByLabel("Audit action filter").fill("privilege_unlock");
  const grid = page.getByLabel("Audit records data grid");
  await expect(grid).toContainText("privilege_unlock");
  await expect(grid).toContainText("access/privilege-vault");
  await grid.getByText("privilege_unlock").click();

  const detail = page.getByLabel("Audit event detail");
  await expect(detail).toBeVisible();
  await expect(detail).toContainText("console-admin");
  await expect(detail).toContainText("127.0.0.1");
  await expect(detail).toContainText("browser_memory");
  await expect(detail).toContainText("success");

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
    await expect(eventsPanel.getByRole("button", { exact: true, name })).toHaveCount(0);
  }
});

test("audit job evidence proves who ran what without leaving Audit", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Job evidence");

  await expect(page.getByRole("heading", { level: 1, name: "Job evidence" })).toBeVisible();
  const panel = page.locator(".auditJobEvidencePanel");
  await expect(panel.getByRole("heading", { level: 2, name: "Job audit evidence" })).toBeVisible();
  await expect(panel.getByLabel("Job evidence summary")).toContainText("Jobs in ledger");
  await expect(panel.getByLabel("Job evidence summary")).toContainText("Jobs with audit rows");

  const grid = panel.getByLabel("Job evidence ledger data grid");
  await expect(grid).toContainText("shell argv");
  await expect(grid).toContainText("system scheduler");
  await expect(grid).toContainText("matched");
  await expect(grid).toContainText("network speed test");

  await grid.getByText("network speed test").first().click();

  const detail = panel.getByLabel("Selected job evidence detail");
  await expect(detail).toContainText("console-admin");
  await expect(detail).toContainText("privileged command");
  await expect(detail).toContainText("no approval record exposed");
  await expect(detail.getByLabel("Audit context for selected job")).toContainText("job.dispatch_requested");
  await expect(detail.getByLabel("Job targets for selected job")).toContainText("edge-sfo-01");
  await expect(detail.getByLabel("Job outputs for selected job")).toContainText("network_speed_test");

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
    await expect(panel.getByRole("button", { exact: true, name })).toHaveCount(0);
  }
});

test("audit sessions correlates terminal and auth evidence without emulator controls", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Sessions");

  await expect(page.getByRole("heading", { level: 1, name: "Session evidence" })).toBeVisible();
  const panel = page.locator(".auditSessionEvidencePanel");
  await expect(panel.getByRole("heading", { level: 2, name: "Session evidence" })).toBeVisible();
  await expect(panel.getByLabel("Session evidence summary")).toContainText("Terminal sessions");
  await expect(panel.getByLabel("Session evidence summary")).toContainText("Audit-linked terminals");
  await expect(panel.getByLabel("Terminal session evidence data grid")).toContainText("console-admin");
  await expect(panel.getByLabel("Terminal session evidence data grid")).toContainText("edge-sfo-01");
  await expect(panel.getByLabel("Terminal session evidence data grid")).toContainText("seq 1-3");

  const detail = panel.getByLabel("Selected terminal session evidence");
  await expect(detail).toContainText("terminal.open");
  await expect(detail).toContainText("terminal.input");
  await expect(detail.getByLabel("Transcript references for selected session")).toContainText("Replay API");
  await expect(detail.getByLabel("Operator auth evidence for selected session")).toContainText("127.0.0.1");
  await expect(detail.getByLabel("Operator auth evidence for selected session")).toContainText("Playwright");
  await expect(panel.getByLabel("Operator session evidence")).toContainText("bearer sessions");

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
    await expect(panel.getByRole("button", { exact: true, name })).toHaveCount(0);
  }

  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await page.getByRole("button", { name: "Audit evidence" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Session evidence" })).toBeVisible();
  await expect(page.locator(".auditSessionEvidencePanel")).toBeVisible();
});

test("audit retention explains export scope and prune impact separately from maintenance cleanup", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Retention & export");

  await expect(page.getByRole("heading", { level: 1, name: "Retention & export" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "History retention" })).toBeVisible();

  const scope = page.getByLabel("History retention export scope");
  await expect(scope).toContainText("Export scope");
  await expect(scope).toContainText("Cleanup review");
  await expect(scope).toContainText("Preview required");

  const boundary = page.getByLabel("Retention cleanup boundary");
  await expect(boundary).toContainText("Evidence retention only");
  await expect(boundary).toContainText("selected history domains");
  await expect(boundary).toContainText("System / Maintenance");

  await expect(page.getByRole("button", { name: "Export history" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Preview prune" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Review cleanup" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Queue cleanup" })).toHaveCount(0);

  await page.getByRole("button", { name: "Preview prune" }).click();
  await expect(page.getByLabel("History retention compliance overview")).toContainText("Dry run 0 rows");
  await expect(scope).toContainText("0 matched rows / 0 objects");
});

test("access overview routes to release authority pages", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Access", "Overview");

  await expect(page.getByRole("heading", { level: 1, name: "Access overview" })).toBeVisible();
  await expect(page.getByLabel("Access posture overview")).toContainText("VPS identities");
  await expect(page.getByLabel("Access posture overview")).toContainText("Gateway sessions");
  await expect(page.getByLabel("Access posture overview")).toContainText("Privilege vault");

  const links = page.getByLabel("Access overview authority links");
  for (const label of ["Operators", "VPS identities", "Gateway sessions", "Privilege vault"]) {
    await expect(links.getByRole("button", { name: new RegExp(label) })).toBeVisible();
  }

  await links.getByRole("button", { name: /Operators/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Operators" })).toBeVisible();
  await expect(page.getByLabel("Operator governance overview")).toBeVisible();

  await openConsoleSubpage(page, "Access", "Overview");
  await page.getByLabel("Access overview authority links").getByRole("button", { name: /VPS identities/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "VPS identities" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "VPS identities" })).toBeVisible();
  await expect(page.getByText("VPS keys")).toHaveCount(0);

  await openConsoleSubpage(page, "Access", "Overview");
  await page.getByLabel("Access overview authority links").getByRole("button", { name: /Gateway sessions/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Gateway sessions" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Gateway sessions" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Copy transcript" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Download transcript" })).toHaveCount(0);

  await openConsoleSubpage(page, "Access", "Overview");
  await page.getByLabel("Access overview authority links").getByRole("button", { name: /Privilege vault/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Privilege vault" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Privilege vault" })).toBeVisible();
  const vaultPanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  });
  await expect(vaultPanel).toContainText("request-bound assertions");
});

test("access privilege vault is the locked handoff for privileged workflows", async ({ page }) => {
  await page.goto("/");

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
      evidence: "bulk tag mutation",
    },
    {
      heading: "Per-VPS config",
      subpage: "Per-VPS",
      view: "Config",
      evidence: "runtime config",
    },
    {
      heading: "Bulk patch",
      subpage: "Bulk patch",
      view: "Config",
      evidence: "runtime config",
    },
    {
      heading: "Network tests",
      prepare: async (routePage) => {
        await expect(routePage.getByText("Loading network workspace")).toHaveCount(0, { timeout: 15000 });
      },
      subpage: "Tests",
      view: "Network",
    },
    {
      heading: "Restore",
      prepare: async (routePage) => {
        await activate(routePage.getByRole("button", { name: "Open restore workflow", exact: true }));
        await expect(routePage.getByRole("complementary", { name: "Open restore workflow" })).toBeVisible();
      },
      root: (routePage) => routePage.getByRole("complementary", { name: "Open restore workflow" }),
      subpage: "Restore",
      view: "Backups",
    },
    {
      heading: "Suite config",
      prepare: async (routePage) => {
        await routePage.getByLabel("API DB pool").fill("40");
        await routePage.locator(".systemConfigOverview").getByRole("button", { name: "Validate" }).click();
        await expect(routePage.getByText(/Validation passed/)).toBeVisible();
        await expect(routePage.getByLabel("Suite config validation and save review")).toContainText(
          "Next: unlock privilege",
        );
      },
      root: (routePage) => routePage.getByLabel("Suite config validation and save review"),
      subpage: "Suite config",
      view: "System",
    },
  ];

  for (const workflow of lockedWorkflows) {
    await expectLockedWorkflowPrivilegeHandoff(page, workflow);
  }
});

test("access operators are separate from vps identities and system navigation", async ({ page }) => {
  await page.goto("/");

  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await expect(mobilePageSelector).not.toContainText("System / Users");
    await expect(mobilePageSelector).toContainText("Access / Operators");
    await expect(mobilePageSelector).toContainText("Access / VPS identities");
  } else {
    const nav = page.getByRole("navigation", { name: "Primary console navigation" });
    await activate(nav.getByRole("button", { name: "System", exact: true }).first());
    const systemSections = nav.getByLabel("System sections");
    await expect(systemSections.getByRole("button", { name: "Users", exact: true })).toHaveCount(0);
    await expect(systemSections.getByRole("button", { name: "Operators", exact: true })).toHaveCount(0);

    await activate(nav.getByRole("button", { name: "Access", exact: true }).first());
    const accessSections = nav.getByLabel("Access sections");
    await expect(accessSections.getByRole("button", { name: "Operators", exact: true })).toBeVisible();
    await expect(accessSections.getByRole("button", { name: "VPS identities", exact: true })).toBeVisible();
  }

  await openConsoleSubpage(page, "Access", "Operators");
  await expect(page.getByRole("heading", { level: 1, name: "Operators" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();

  await openConsoleSubpage(page, "Access", "VPS identities");
  await expect(page.getByRole("heading", { level: 1, name: "VPS identities" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "VPS identities" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Users" })).toHaveCount(0);
});

test("backups overview explains recoverability and links backup workflows", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Overview");

  await expect(page.getByRole("heading", { level: 1, name: "Backup overview" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Backup overview" })).toBeVisible();

  const decision = page.getByLabel("Backup recovery decision");
  await expect(decision).toContainText("Recoverability decision");
  await expect(decision).toContainText("Needs restore test");
  await expect(decision).toContainText("Coverage");
  await expect(decision).toContainText("Artifacts");
  await expect(decision).toContainText("Restore tests");

  const posture = page.getByLabel("Backup posture overview");
  await expect(posture).toContainText("Protected VPSs");
  await expect(posture).toContainText("Failed requests");
  await expect(posture).toContainText("Artifact storage");
  await expect(posture).toContainText("Restore test");
  await expect(posture).toContainText("Retention/security");

  const evidence = page.getByLabel("Backup overview evidence summary");
  await expect(evidence).toContainText("Latest backup");
  await expect(evidence).toContainText("Artifact metadata recorded");
  await expect(evidence).toContainText("Restore verification");
  await expect(evidence).toContainText("No restore plan");
  await expect(evidence).toContainText("Run a restore rehearsal");

  const links = page.getByLabel("Backup overview workflow links");
  for (const label of ["Requests", "Policies", "Artifacts", "Restore", "Migration"]) {
    await expect(links.getByRole("button", { name: new RegExp(label) })).toBeVisible();
  }

  await links.getByRole("button", { name: /Requests/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Backup requests" })).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page.getByLabel("Backup overview workflow links").getByRole("button", { name: /Policies/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Backup policies" })).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page.getByLabel("Backup overview workflow links").getByRole("button", { name: /Artifacts/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Backup artifacts" })).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page.getByLabel("Backup overview workflow links").getByRole("button", { name: /Restore/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Restore" })).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Overview");
  await page.getByLabel("Backup overview workflow links").getByRole("button", { name: /Migration/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Migration" })).toBeVisible();
});

test("backups artifacts keep backup handoff separate from job cleanup", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup artifact ownership and handoff controls are covered through the desktop drawer workflow",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Artifacts");

  await expect(page.getByRole("heading", { level: 1, name: "Backup artifacts" })).toBeVisible();
  const guide = page.getByLabel("Backup artifact ownership guide");
  await expect(guide).toContainText("Backup artifact ownership");
  await expect(guide).toContainText("Artifact inventory");
  await expect(guide).toContainText("Backup linkage");
  await expect(guide).toContainText("Upload/import");
  await expect(guide).toContainText("Handoff source");
  await expect(guide).toContainText("Retained job output");
  await expect(guide).toContainText("Restore consumers");

  const records = page.locator(".fleetPanel");
  await expect(records).toContainText("Artifact records");
  await expect(records).not.toContainText("Artifact cleanup");
  await expect(records).not.toContainText("Queue cleanup");
  await expect(records).not.toContainText("Type DELETE");

  await activate(page.getByRole("button", { name: "Open artifact workflow", exact: true }));
  const drawer = page.getByRole("complementary", { name: "Open artifact workflow" });
  await expect(drawer.getByRole("heading", { name: "Upload artifact" })).toBeVisible();
  await expect(drawer.getByLabel("Artifact backup request")).toBeVisible();
  await expect(drawer.getByLabel("Backup artifact handoff source job ID")).toBeVisible();
  await expect(drawer.getByRole("button", { name: "Review upload" })).toBeVisible();
  await expect(drawer.getByRole("button", { name: "Review handoff" })).toBeVisible();
  await expect(drawer).not.toContainText("Artifact cleanup");
  await expect(drawer).not.toContainText("Queue cleanup");

  await activate(page.getByRole("button", { name: "Close Open artifact workflow" }));
  await guide.getByRole("button", { name: "Open Jobs artifacts inventory" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Job artifacts" })).toBeVisible();
  await expect(page.getByLabel("Job artifact inventory summary")).toContainText("System / Maintenance");
  await expect(page.getByRole("button", { name: "Queue cleanup" })).toHaveCount(0);
});

test("backups requests keep request review separate from policy and restore work", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup request role separation is covered through the desktop drawer workflow",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Requests");

  await expect(page.getByRole("heading", { level: 1, name: "Backup requests" })).toBeVisible();
  const records = page.locator(".fleetPanel");
  await expect(records).toContainText("Backup request records");
  await expect(records).not.toContainText("Backup policy records");
  await expect(records).not.toContainText("Restore plan records");
  await expect(records).not.toContainText("Artifact metadata linked to backup requests");

  await expect(page.getByRole("button", { name: "Open backup request", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Open policy editor", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open restore workflow", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open artifact workflow", exact: true })).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Open backup request", exact: true }));
  const drawer = page.getByLabel("Open backup request");
  await expect(drawer.getByRole("heading", { name: "Request backup" })).toBeVisible();
  await expect(drawer.getByRole("heading", { name: "Backup policy" })).toHaveCount(0);
  await expect(drawer.getByRole("heading", { name: "Policy prune" })).toHaveCount(0);
  await expect(drawer.getByRole("heading", { name: "Restore plan" })).toHaveCount(0);
});

test("backups policies keep authoring separate and review prune preview before apply", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup policy prune review is covered through the desktop drawer workflow",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Policies");

  await expect(page.getByRole("heading", { level: 1, name: "Backup policies" })).toBeVisible();
  const records = page.locator(".fleetPanel");
  await expect(records).toContainText("Backup policy records");
  await expect(records).not.toContainText("Backup request records");
  await expect(records).not.toContainText("Restore plan records");
  await expect(records).not.toContainText("Artifact metadata linked to backup requests");

  await expect(page.getByRole("button", { name: "Open policy editor", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Open backup request", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open restore workflow", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open artifact workflow", exact: true })).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Open policy editor", exact: true }));
  const drawer = page.getByLabel("Open policy editor");
  await expect(drawer.getByRole("heading", { name: "Backup policy" })).toBeVisible();
  await expect(drawer.getByRole("heading", { name: "Policy prune" })).toBeVisible();
  await expect(drawer.getByLabel("Backup policy prune review state")).toContainText("Preview only");
  await expect(drawer.getByRole("heading", { name: "Request backup" })).toHaveCount(0);
  await expect(drawer.getByRole("heading", { name: "Restore plan" })).toHaveCount(0);
  await expect(drawer.getByRole("heading", { name: "Artifact upload" })).toHaveCount(0);

  await drawer.getByLabel("Dry run").uncheck();
  await expect(drawer.getByLabel("Backup policy prune review state")).toContainText("Preview required before apply");
  await activate(drawer.getByRole("button", { name: "Review prune apply" }));
  const confirmation = drawer.getByLabel("Confirm policy prune apply");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Preview hash");
  await expect(confirmation).toContainText("Reviewed rows");
  await activate(confirmation.getByRole("button", { name: "Apply prune" }));

  const pruneRequests = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { backupPolicyPrunes: Array<Record<string, unknown>> };
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

test("backups restore presents guided review, verification, and rollback states", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "restore workflow guide and drawer reviews are covered through the desktop workflow",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");

  await expect(page.getByRole("heading", { level: 1, name: "Restore" })).toBeVisible();
  const guide = page.getByLabel("Restore workflow guide");
  await expect(guide).toContainText("Guided restore workflow");
  await expect(guide).toContainText("Plan restore");
  await expect(guide).toContainText("Stage archive");
  await expect(guide).toContainText("Run restore");
  await expect(guide).toContainText("Verify outcome");
  await expect(guide).toContainText("Rollback readiness");
  await expect(guide).toContainText("Review plan required");
  await expect(guide).toContainText("Review restore required");
  await expect(guide).toContainText("No verified restore");

  await guide.getByRole("button", { name: "Open artifact inventory" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Backup artifacts" })).toBeVisible();
  await openConsoleSubpage(page, "Backups", "Restore");
  await page.getByLabel("Restore related evidence links").getByRole("button", { name: "Open migration checklist" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Migration" })).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Open restore workflow", exact: true }));
  const drawer = page.getByRole("complementary", { name: "Open restore workflow" });
  await expect(drawer.getByRole("heading", { name: "Plan restore" })).toBeVisible();
  await expect(drawer.getByRole("heading", { name: "Run restore" })).toBeVisible();
  await expect(drawer.getByRole("heading", { name: "Rollback restore" })).toBeVisible();
  await expect(drawer.getByRole("button", { name: "Review plan" })).toBeVisible();
  await expect(drawer.getByRole("button", { name: "Review restore" })).toBeVisible();
  await expect(drawer.getByRole("button", { name: "Review rollback" })).toBeVisible();
});

test("backups migration presents cutover checklist and related evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "migration cutover guide and assistant are covered through the desktop workflow",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Migration");

  await expect(page.getByRole("heading", { level: 1, name: "Migration" })).toBeVisible();
  const guide = page.getByLabel("Migration cutover guide");
  await expect(guide).toContainText("Migration cutover checklist");
  await expect(guide).toContainText("Source restore plan");
  await expect(guide).toContainText("Staged archive");
  await expect(guide).toContainText("DNS, network, config");
  await expect(guide).toContainText("Identity and keys");
  await expect(guide).toContainText("Cutover evidence");
  await expect(guide).toContainText("No restore-ready plan");
  await expect(guide).toContainText("1 active artifact");

  await guide.getByRole("button", { name: "Open restore evidence" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Restore" })).toBeVisible();
  await openConsoleSubpage(page, "Backups", "Migration");
  await page.getByLabel("Migration related evidence links").getByRole("button", { name: "Open artifact inventory" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Backup artifacts" })).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Migration");
  await activate(page.getByRole("button", { name: "Open migration workflow", exact: true }));
  const drawer = page.getByRole("complementary", { name: "Open migration workflow" });
  await expect(drawer.getByRole("heading", { name: "Migration assistant" })).toBeVisible();
  await expect(drawer).toContainText("Restore plan");
  await expect(drawer).toContainText("Archive metadata");
  await expect(drawer).toContainText("Privilege");
  await expect(drawer).toContainText("Rehearsal mode");
  await expect(drawer).toContainText("Post-restore");
  await expect(drawer).toContainText("Privilege policy");
});

test("network overview links to release network workflows", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Overview");

  await expect(page.getByRole("heading", { level: 1, name: "Network overview" })).toBeVisible();
  await expect(page.getByLabel("Network posture summary")).toContainText("Saved plans");
  const links = page.getByLabel("Network overview workflow links");
  for (const label of ["Graph", "Tunnel Plans", "Tests", "OSPF", "Evidence"]) {
    await expect(links.getByRole("button", { name: new RegExp(label) })).toBeVisible();
  }

  await links.getByRole("button", { name: /Graph/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network graph" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page.getByLabel("Network overview workflow links").getByRole("button", { name: /Tunnel Plans/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Tunnel plans" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page.getByLabel("Network overview workflow links").getByRole("button", { name: /Tests/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network tests" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page.getByLabel("Network overview workflow links").getByRole("button", { name: /OSPF/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network OSPF" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Overview");
  await page.getByLabel("Network overview workflow links").getByRole("button", { name: /Evidence/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network evidence" })).toBeVisible();
});

test("network graph stays focused on visual topology inspection", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Graph");

  await expect(page.getByRole("heading", { level: 1, name: "Network graph" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Topology graph" })).toBeVisible();
  await expect(page.getByLabel("Tunnel plans data grid")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Create tunnel plan" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Latency and auto OSPF" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Review cost update" })).toHaveCount(0);
});

test("network tests keeps diagnostics and trends mutation-free", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tests");

  await expect(page.getByRole("heading", { level: 1, name: "Network tests" })).toBeVisible();
  await expect(page.getByText("Loading network workspace")).toHaveCount(0, { timeout: 15000 });
  await expect(page.getByRole("heading", { level: 2, name: "Network tests" })).toBeVisible();
  await expect(page.getByLabel("Network test review contract")).toBeVisible();
  await expect(page.getByLabel("Network test trend charts")).toBeVisible();
  await expect(page.getByRole("button", { name: "Review inspect" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Review probe" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Review speed test" })).toBeVisible();

  await expect(page.getByLabel("Tunnel plans data grid")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Create tunnel plan" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save plan" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Review cost update" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Review rollback" })).toHaveCount(0);
});

test("network evidence stays read-mostly and links to network action pages", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Evidence");

  await expect(page.getByRole("heading", { level: 1, name: "Network evidence" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Network evidence" })).toBeVisible();
  await expect(page.getByLabel("Network evidence timeline")).toBeVisible();
  const actions = page.getByLabel("Network evidence actions");
  for (const label of ["Open graph", "Run tests", "Tunnel plans", "Open OSPF"]) {
    await expect(actions.getByRole("button", { name: label })).toBeVisible();
  }
  await expect(actions.getByRole("button", { name: /Load output|Reload output/ })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Compare to previous" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Review cost update" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Review inspect" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save plan" })).toHaveCount(0);

  await actions.getByRole("button", { name: "Open graph" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network graph" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Evidence");
  await page.getByLabel("Network evidence actions").getByRole("button", { name: "Run tests" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Network tests" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Evidence");
  await page.getByLabel("Network evidence actions").getByRole("button", { name: "Tunnel plans" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Tunnel plans" })).toBeVisible();

  await openConsoleSubpage(page, "Network", "Evidence");
  const ospfButton = page.getByLabel("Network evidence actions").getByRole("button", { name: "Open OSPF" });
  await expect(ospfButton).toBeEnabled();
  await ospfButton.click();
  await expect(page.getByRole("heading", { level: 1, name: "Network OSPF" })).toBeVisible();
});

test("network tunnel plans owns promotion without a standalone promotion subpage", async ({ page }) => {
  await page.goto("/");

  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await expect(mobilePageSelector).not.toContainText("Network / Promotion");
  } else {
    const networkSections = page
      .getByRole("navigation", { name: "Primary console navigation" })
      .getByLabel("Network sections");
    await expect(networkSections.getByRole("button", { name: "Promotion", exact: true })).toHaveCount(0);
  }

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await expect(page.getByRole("heading", { level: 1, name: "Tunnel plans" })).toBeVisible();
  await expect(page.getByLabel("Tunnel plans data grid")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Create tunnel plan" })).toHaveCount(0);
  await expect(page.getByLabel("Tunnel plan promotion workflow")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Latency and auto OSPF" })).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Create tunnel plan" }));
  await expect(page.getByRole("heading", { name: "Create tunnel plan" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Close create tunnel plan workflow" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Close create tunnel plan workflow" }));
  await expect(page.getByRole("heading", { name: "Create tunnel plan" })).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Promotion workflow" }));
  const promotion = page.getByLabel("Tunnel plan promotion workflow");
  await expect(promotion.getByRole("heading", { name: "Tunnel promotion" })).toBeVisible();
  await expect(promotion.getByRole("button", { name: "Close tunnel promotion workflow" })).toBeVisible();
  await expect(promotion.getByText("Promotion diff workflow")).toBeVisible();
  await expect(promotion.getByLabel("Topology promotion diff workflow")).toContainText("Observed topology");
  await activate(promotion.getByRole("button", { name: "Close tunnel promotion workflow" }));
  await expect(page.getByLabel("Tunnel plan promotion workflow")).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Generated config" }));
  await expect(page.getByRole("heading", { name: "Latest generated config" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Close generated config workflow" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Close generated config workflow" }));
  await expect(page.getByRole("heading", { name: "Latest generated config" })).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Latency and auto OSPF" }));
  await expect(page.getByRole("heading", { name: "Latency and auto OSPF" })).toBeVisible();
  await expect(page.getByLabel("Automation state data grid")).toBeVisible();
  await expect(page.getByRole("button", { name: "Close latency and auto OSPF workflow" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Close latency and auto OSPF workflow" }));
  await expect(page.getByRole("heading", { name: "Latency and auto OSPF" })).toHaveCount(0);
});

test("system overview keeps platform health separate from fleet monitoring", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Overview");

  await expect(page.getByRole("heading", { level: 1, name: "System overview" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Control-plane metrics" })).toBeVisible();
  const systemOverview = page.getByLabel("System overview operations overview");
  await expect(systemOverview).toContainText("Control-plane posture");
  await expect(systemOverview).toContainText("DB capacity");
  await expect(systemOverview).toContainText("Dispatch pressure");
  await expect(systemOverview).toContainText("Gateway queue");
  await expect(systemOverview).toContainText("Capacity forecast");

  const main = page.getByRole("main");
  await expect(main.locator(".vpsMonitorGrid")).toHaveCount(0);
  await expect(main.locator(".vpsMonitorCard")).toHaveCount(0);
  await expect(main).not.toContainText("Komari-style");
  await expect(main).not.toContainText("VPS cards");

  await openConsoleSubpage(page, "Fleet", "Monitor");
  await expect(page.getByRole("heading", { level: 1, name: "Fleet monitor" })).toBeVisible();
  await expect(page.getByLabel("VPS monitor cards")).toBeVisible();
  await expect(page.getByLabel("VPS monitor cards").locator(".vpsMonitorCard")).toHaveCount(3);
});

test("system capacity focuses on control-plane limits and API gaps", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Capacity");

  await expect(page.getByRole("heading", { level: 1, name: "System capacity" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Capacity telemetry", exact: true })).toBeVisible();

  const posture = page.getByLabel("System capacity posture overview");
  await expect(posture).toContainText("DB pool capacity");
  await expect(posture).toContainText("Dispatch capacity");
  await expect(posture).toContainText("Gateway backlog");
  await expect(posture).toContainText("Artifact storage");
  await expect(posture).toContainText("Retention pressure");
  await expect(posture).toContainText("Worker lag");
  await expect(posture).toContainText("API gap");

  await expect(page.getByRole("heading", { name: "Capacity", exact: true })).toBeVisible();
  await expect(page.getByLabel("Capacity thresholds", { exact: true })).toContainText("70% pool pressure");
  await expect(page.getByRole("heading", { name: "Dispatch capacity", exact: true })).toBeVisible();
  await expect(page.getByLabel("Dispatch capacity thresholds")).toContainText("in-flight");
  await expect(page.getByRole("heading", { name: "Gateway queue", exact: true })).toBeVisible();
  await expect(page.getByLabel("Gateway queue thresholds")).toContainText("queue full");

  const gaps = page.getByLabel("System capacity unavailable telemetry");
  await expect(gaps).toContainText("Artifact storage bytes");
  await expect(gaps).toContainText("Retention prune backlog");
  await expect(gaps).toContainText("Worker lag seconds");
  await expect(gaps).toContainText("System / Maintenance");

  const main = page.getByRole("main");
  await expect(main.locator(".vpsMonitorGrid")).toHaveCount(0);
  await expect(main.locator(".vpsMonitorCard")).toHaveCount(0);
  await expect(main).not.toContainText("CPU usage");
  await expect(main).not.toContainText("Memory usage");
  await expect(main).not.toContainText("Disk usage");
});

test("system suite config owns control-plane config and excludes per-VPS editors", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Suite config");

  await expect(page.getByRole("heading", { level: 1, name: "Suite config" })).toBeVisible();
  await expect(page.locator(".systemConfigOverview").getByRole("heading", { name: "Suite config" })).toBeVisible();

  const boundary = page.getByLabel("Suite config ownership boundary");
  await expect(boundary).toContainText("System scope");
  await expect(boundary).toContainText("Suite TOML controls API, gateway, worker, capacity, storage, secrets, and control-plane timeouts.");
  await expect(boundary).toContainText("Runtime config scope");
  await expect(boundary).toContainText("Per-VPS runtime reads, overrides, patches, templates, and rules stay in Config workflows.");
  await expect(boundary).toContainText("Save contract");

  const sections = page.getByLabel("Suite config sections");
  for (const label of ["API", "Gateway", "Worker", "Capacity", "Storage", "Secrets", "Timeouts", "Review"]) {
    await expect(sections).toContainText(label);
  }
  await expect(page.getByLabel("Suite config editor mode")).toBeVisible();
  await expect(page.getByLabel("Suite config validation and save review")).toContainText("Edit");
  await expect(page.getByText("Advanced redacted JSON diff")).toBeVisible();

  await expect(page.getByLabel("VPS config target")).toHaveCount(0);
  await expect(page.getByLabel("VPS redacted runtime config TOML")).toHaveCount(0);
  await expect(page.getByLabel("One-VPS runtime config override TOML")).toHaveCount(0);
  await expect(page.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(page.getByLabel("Rendered bulk runtime config patch TOML")).toHaveCount(0);
  await expect(page.getByLabel("Temporary bulk runtime config patch TOML")).toHaveCount(0);

  await boundary.getByRole("button", { name: "Open Config / Per-VPS" }).click();
  await expect(page.getByRole("heading", { name: "Per-VPS config" })).toBeVisible();

  await openConsoleSubpage(page, "System", "Suite config");
  await page.getByLabel("Suite config ownership boundary").getByRole("button", { name: "Open Config / Bulk patch" }).click();
  await expect(page.getByRole("heading", { name: "Bulk patch" })).toBeVisible();
});

test("system maintenance owns artifact cleanup and maintenance job records", async ({ page }) => {
  await page.goto("/");
  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await expect(mobilePageSelector).not.toContainText("Jobs / Server jobs");
  } else {
    const nav = page.getByRole("navigation", { name: "Primary console navigation" });
    await activate(nav.getByRole("button", { name: "Jobs", exact: true }).first());
    await expect(
      nav.getByLabel("Jobs sections").getByRole("button", { name: "Server jobs", exact: true }),
    ).toHaveCount(0);
  }

  await openConsoleSubpage(page, "System", "Maintenance");

  await expect(page.getByRole("heading", { level: 1, name: "System maintenance" })).toBeVisible();
  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await expect(cleanupPanel).toBeVisible();
  await expect(cleanupPanel.getByText("Dry-run gate")).toBeVisible();
  await expect(cleanupPanel.getByLabel("Artifact cleanup readiness")).toContainText("Blocked until dry run");
  await expect(cleanupPanel.getByRole("button", { name: "Queue cleanup" })).toBeDisabled();

  await cleanupPanel.getByRole("button", { name: "Preview" }).click();
  await expect(cleanupPanel.getByLabel("Preview hash")).toHaveValue(/^[0-9a-f]{64}$/);
  await expect(cleanupPanel.getByLabel("Artifact cleanup readiness")).toContainText("Ready after dry run");
  await cleanupPanel.getByRole("button", { name: "Queue cleanup" }).click();
  await expect(page.getByRole("region", { name: "Confirm artifact cleanup" })).toBeVisible();
  await expect(page.getByLabel("Type DELETE to confirm artifact cleanup")).toBeVisible();
  await activate(page.getByRole("button", { name: "Close confirmation" }));

  const maintenanceJobs = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Maintenance jobs" }),
  });
  await expect(maintenanceJobs).toContainText("retained control-plane maintenance jobs");
  await expect(maintenanceJobs).toContainText("Maintenance job records");
  await expect(page.getByRole("heading", { name: "Server jobs" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Job history" })).toHaveCount(0);
});

test("system preferences separates personal display from shared defaults", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Preferences");

  await expect(page.getByRole("heading", { level: 1, name: "System preferences" })).toBeVisible();
  const preferencesScope = page.getByLabel("Preferences scope overview");
  await expect(preferencesScope).toContainText("Personal display");
  await expect(preferencesScope).toContainText("Operator scoped");
  await expect(preferencesScope).toContainText("review prompt display");
  await expect(preferencesScope).toContainText("Browser state");
  await expect(preferencesScope).toContainText("Local only");
  await expect(preferencesScope).toContainText("Fleet/system defaults");
  await expect(preferencesScope).toContainText("Needs shared scope");
  await expect(preferencesScope).toContainText("temporarily stored per operator");

  const personal = page.getByLabel("Personal display preferences");
  await expect(personal).toContainText("Review prompts");
  await expect(personal).toContainText("does not weaken required review");
  await expect(personal).toContainText("Bulk execution summaries");

  const shared = page.getByLabel("Fleet and system defaults");
  await expect(shared).toContainText("Gateway install defaults");
  await expect(shared).toContainText("Home telemetry curves");
  await expect(shared).not.toContainText("Dashboard curves");
  await expect(shared.getByLabel("Home telemetry curve exclusions")).toBeVisible();
  await expect(shared).toContainText("operator-stored defaults");
  await expect(shared).toContainText("not personal display settings");
  await expect(shared).toContainText("shared pool policy");
});

async function expectCommandPaletteGroup(page: Page, group: string, query: string) {
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
    const status = index % 17 === 0 ? "offline" : index % 11 === 0 ? "stale" : "online";
    return {
      capabilities: index % 9 === 0 ? unprivilegedCapabilities : rootCapabilities,
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
          const text = element.textContent?.trim().replace(/\s+/g, " ").slice(0, 80) ?? "";
          const elementOverflow =
            element.scrollWidth > element.clientWidth + 1 &&
            style.overflowX !== "visible";
          const escapesCard = rect.right > cardRect.right + 1 || rect.left < cardRect.left - 1;
          return elementOverflow || escapesCard
            ? {
                cardIndex,
                className: element.getAttribute("class") ?? element.tagName.toLowerCase(),
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
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(pageOverflow, `${label} page horizontal overflow`).toBeLessThanOrEqual(1);
}

async function selectCommandPaletteResult(page: Page, group: string, query: string) {
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
  const option = root.getByRole("option", { name: optionName });
  await expect(option).toBeVisible();
  await option.click();
}

async function expectCanonicalVpsDetail(page: Page, vpsName: string) {
  await expect(
    page.getByRole("heading", { level: 1, name: "Instance detail", exact: true }),
  ).toBeVisible();
  const detail = page.getByLabel("Canonical VPS detail");
  await expect(detail).toContainText(vpsName);
  await expect(detail.getByLabel("Selected VPS health card")).toContainText(vpsName);
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
    await expect(detail.getByRole("tabpanel", { name: `${tab} tab` })).toBeVisible();
  }
}

function homeAttentionPanel(page: Page) {
  return page.locator(".homeReviewPanel").filter({
    has: page.getByRole("heading", { name: "Needs attention" }),
  });
}

function homeActivityPanel(page: Page) {
  return page.locator(".homeReviewPanel").filter({
    has: page.getByRole("heading", { name: "Recent activity" }),
  });
}

async function expectJobHistoryDetailOpen(page: Page) {
  await expect(page.getByRole("heading", { level: 1, name: "Job history" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Target results" })).toBeVisible();
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

async function expectLockedWorkflowPrivilegeHandoff(page: Page, workflow: PrivilegeHandoffSpec) {
  await openConsoleSubpage(page, workflow.view, workflow.subpage);
  await expect(page.getByRole("heading", { name: workflow.heading }).first()).toBeVisible();
  await workflow.prepare?.(page);

  const root = workflow.root?.(page) ?? page.getByRole("main");
  if (workflow.evidence) {
    await expect(root).toContainText(workflow.evidence);
  }

  const handoff = root.getByRole("button", { name: /Open Privilege Vault/ }).first();
  await expect(handoff).toBeVisible();
  await activate(handoff);

  await expect(page.getByRole("heading", { level: 1, name: "Privilege vault" })).toBeVisible();
  const vaultPanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  });
  await expect(vaultPanel).toContainText("request-bound assertions");
}

async function clickHomeQuickAction(page: Page, name: string) {
  await page.goto("/");
  const quickActions = page.getByLabel("Home quick actions");
  await expect(quickActions).toBeVisible();
  await quickActions.getByRole("button", { name }).click();
}
