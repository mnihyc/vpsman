import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
  waitForConsoleShell,
} from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual selector visual audit screenshots only");
test.setTimeout(180_000);

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures exact VPS selector states", async ({ page }, testInfo) => {
  const outputDir = testInfo.outputPath("selector-visual-audit");
  mkdirSync(outputDir, { recursive: true });
  const manifest: Array<Record<string, unknown>> = [];

  await page.goto("/");
  await waitForConsoleShell(page, 15_000);
  await openVpsMenu(
    page.locator(".homeQuickActions"),
    "Home quick action target",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await capture(page, outputDir, manifest, "home-quick-action-target");

  await openConsoleSubpage(page, "Config", "Per-VPS");
  await openVpsMenu(page.locator(".configApplyGrid"), "VPS config target", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "config-single-target");

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Sources");
  const sourcesPanel = page.locator(".configurationSourcesPanel");
  await activate(
    sourcesPanel.getByRole("button", { name: "Change configuration" }),
  );
  const assignmentDrawer = page.getByRole("complementary", {
    name: "Change effective configuration",
  });
  await expect(
    assignmentDrawer.getByRole("button", { name: "Review reset to system default" }),
  ).toBeDisabled();
  await assignmentDrawer.scrollIntoViewIfNeeded();
  await capture(
    page,
    outputDir,
    manifest,
    "configuration-source-assignment-empty",
    { fullPage: false, scrollToTop: false },
  );
  await openVpsMenu(
    assignmentDrawer,
    "Add configuration target VPS",
    "edge",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await capture(
    page,
    outputDir,
    manifest,
    "configuration-source-assignment-direct-menu",
    { fullPage: false, scrollToTop: false },
  );
  await page
    .getByRole("listbox", { name: "Add configuration target VPS options" })
    .getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ })
    .click();
  await assignmentDrawer
    .getByText("Add targets by selector", { exact: true })
    .click();
  const assignmentSelector = assignmentDrawer.getByLabel(
    "Configuration target selector",
  );
  await assignmentSelector.fill("country:DE");
  await expect(
    assignmentDrawer.getByLabel("Configuration target preview"),
  ).toContainText("edge-sfo-01");
  await expect(
    assignmentDrawer.getByLabel("Configuration target preview"),
  ).toContainText("core-fra-02");
  await assignmentSelector.press("Escape");
  await assignmentDrawer
    .getByLabel("Configuration target preview")
    .scrollIntoViewIfNeeded();
  await capture(
    page,
    outputDir,
    manifest,
    "configuration-source-assignment-union-preview",
    { fullPage: false, scrollToTop: false },
  );

  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await page
    .getByLabel("Bulk group selector expression")
    .fill("status:online");
  await expect(
    page.getByLabel("Bulk group local VPS preview"),
  ).toContainText("edge-sfo-01");
  await capture(page, outputDir, manifest, "fleet-bulk-group-target-preview");

  await openConsoleSubpage(page, "Remote Operations", "Bulk files");
  await page.getByLabel("Bulk file target selector").fill("status:online");
  await expect(
    page.getByLabel("Bulk file local VPS preview"),
  ).toContainText("edge-sfo-01");
  await capture(page, outputDir, manifest, "bulk-file-target-preview");

  await openConsoleSubpage(page, "Remote Operations", "Files");
  await openVpsMenu(page.locator(".fileBrowserPanel"), "File browser target VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "file-browser-target");

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Create plan", exact: true }));
  const tunnelComposer = page.locator(".tunnelPlanComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await openVpsMenu(tunnelComposer, "Left tunnel VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await tunnelComposer
    .getByRole("combobox", { name: "Left tunnel VPS" })
    .press("Enter");
  await openVpsMenu(tunnelComposer, "Right tunnel VPS", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "topology-tunnel-targets");

  await openConsoleSubpage(page, "Automation", "Schedules");
  await activate(page.getByRole("button", { name: "Expand Create schedule" }));
  await page.getByLabel("Schedule target expression").fill("country:US");
  await expect(
    page.getByLabel("Schedule local VPS preview"),
  ).toContainText("edge-sfo-01");
  await capture(page, outputDir, manifest, "schedule-target-preview");

  await openConsoleSubpage(page, "Backups", "Policies");
  await activate(
    page.getByRole("button", { name: "Create policy", exact: true }).first(),
  );
  const backupPolicyDrawer = page.getByRole("complementary", {
    name: "Create policy",
  });
  await backupPolicyDrawer
    .getByLabel("Backup policy target expression")
    .fill("country:US");
  await expect(
    backupPolicyDrawer.getByLabel("Backup policy local VPS preview"),
  ).toContainText("edge-sfo-01");
  await capture(page, outputDir, manifest, "backup-policy-target-preview");

  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Choose restore artifact" }));
  const restoreWorkflow = page.getByLabel("Choose restore artifact");
  await restoreWorkflow.getByLabel("Restore source backup request").selectOption({ index: 1 });
  await openVpsMenu(restoreWorkflow, "Restore target client", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "restore-target");
  await restoreWorkflow.getByLabel("Restore target client").press("Enter");
  await expect(restoreWorkflow.getByText("/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee/agent-fra-02")).toBeVisible();
  await expect(restoreWorkflow.getByLabel("Staged archive")).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await capture(page, outputDir, manifest, "restore-record-selected");
  await restoreWorkflow.getByLabel("Staged archive").scrollIntoViewIfNeeded();
  await capture(page, outputDir, manifest, "restore-staged-archive-selected", {
    fullPage: false,
    scrollToTop: false,
  });

  await openConsoleSubpage(page, "Observability", "Alerts");
  await activate(page.getByRole("button", { name: "Create policy" }).first());
  const policyEditor = page.locator(".consoleDetailPanel", {
    hasText: "Create alert policy",
  }).last();
  await expect(policyEditor).toBeVisible();
  const policyExpression = policyEditor.getByRole("combobox", {
    name: "Policy VPS selector expression",
  });
  await policyExpression.fill("country:US && status:online");
  await expect(policyExpression).toHaveValue("country:US && status:online");
  await expect(
    policyEditor.getByLabel("Alert policy local VPS preview"),
  ).toContainText("edge-sfo-01");
  await capture(
    page,
    outputDir,
    manifest,
    "observability-alert-policy-expression-filter",
  );
  await activate(policyEditor.getByLabel("Close detail panel"));
  const policyGrid = page.getByLabel("Policy groups data grid");
  await openExpressionMenu(policyGrid, "Policy groups search", "enabled", /^enabled$/);
  await capture(
    page,
    outputDir,
    manifest,
    "observability-alert-policy-grid-search-suggestion",
  );

  await activate(page.getByRole("tab", { name: /Destinations/ }));
  await activate(page.getByRole("button", { name: "Create channel" }).first());
  const channelEditor = page.locator(".consoleDetailPanel", {
    hasText: "Create notification channel",
  }).last();
  await expect(channelEditor).toBeVisible();
  await channelEditor.getByLabel("Notification scope kind").selectOption("client");
  await openVpsMenu(channelEditor, "Notification scope value", "fra", /core-fra-02.*agent-fra-02/);
  await capture(
    page,
    outputDir,
    manifest,
    "observability-notification-client-scope",
  );
  await activate(channelEditor.getByLabel("Close detail panel"));
  await openConsoleSubpage(page, "Observability", "Event webhooks");
  await expect(
    page.getByText("Event webhook rules", { exact: true }).first(),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Create rule" }).first());
  await openExpressionMenu(
    page.locator("main"),
    "Webhook expression",
    "interval.",
    /^interval\.30sec$/,
  );
  await capture(
    page,
    outputDir,
    manifest,
    "observability-webhook-expression-event-search",
  );

  await openConsoleSubpage(page, "Jobs", "Dispatch");
  const dispatchComposer = page.locator(".commandComposer");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "name:s", /edge-sfo-01.*Name.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "dispatch-expression-name-search");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "name:s", /edge-sfo-01.*Name.*agent-sfo-01/);
  await page.keyboard.press("Enter");
  await expect(dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" })).toHaveValue("name:edge-sfo-01");
  await capture(page, outputDir, manifest, "dispatch-expression-name-selected");
  await dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "fo01", /edge-sfo-01.*ID.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "dispatch-expression-id-suffix-search");
  await dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "status:on", /^status:online$/);
  await capture(page, outputDir, manifest, "dispatch-expression-status-search");
  await dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "vps.status:on", /^vps\.status:online$/);
  await capture(page, outputDir, manifest, "dispatch-expression-vps-status-search");
  await dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "role:e", /^role:edge$/);
  await capture(page, outputDir, manifest, "dispatch-expression-unknown-namespace-search");
  await dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "*", /^\*$/);
  await capture(page, outputDir, manifest, "dispatch-expression-all-wildcard-search");
  const longExpression = dispatchComposer.getByRole("combobox", { name: "Bulk target selector expression" });
  await longExpression.fill(
    "provider:alpha && country:US && status:online && role:edge && id:agent-sfo-01 || id:agent-fra-02 || id:agent-nyc-03 || " +
      "vps.status:online && vps.provider:alpha && vps.country:US && tag:role:edge && name:edge-sfo-01 || " +
      "id:agent-sfo-01 || id:agent-fra-02 || id:agent-nyc-03",
  );
  await longExpression.press("End");
  await expect
    .poll(() => longExpression.evaluate((element) => element.scrollLeft))
    .toBeGreaterThan(20);
  await capture(page, outputDir, manifest, "dispatch-expression-long-scrolled-end");

  writeFileSync(
    join(outputDir, `manifest-${testInfo.project.name}.json`),
    `${JSON.stringify({ screenshots: manifest }, null, 2)}\n`,
  );
});

async function openVpsMenu(
  root: Locator,
  label: string,
  query: string,
  expectedOption: RegExp,
) {
  const combobox = root.getByRole("combobox", { name: label });
  await expect(combobox).toBeVisible();
  await combobox.fill(query);
  await expect(
    root.page().locator(".vpsComboboxMenu").getByRole("option", { name: expectedOption }),
  ).toBeVisible();
  await expectMenuAdjacentToControl(
    combobox.locator("xpath=.."),
    root.page().locator(".vpsComboboxMenu"),
  );
}

async function openExpressionMenu(
  root: Locator,
  label: string,
  query: string,
  expectedOption: RegExp,
) {
  const searchbox = root.getByRole("combobox", { name: label });
  await expect(searchbox).toBeVisible();
  await searchbox.click();
  await searchbox.fill("");
  await searchbox.click();
  await searchbox.pressSequentially(query);
  await expect(
    root.page().locator(".searchExpressionAutocomplete").getByRole("option", { name: expectedOption }),
  ).toBeVisible();
  await expectMenuAdjacentToControl(
    searchbox.locator(
      "xpath=ancestor::*[contains(concat(' ', normalize-space(@class), ' '), ' searchExpressionInput ')][1]",
    ),
    root.page().locator(".searchExpressionAutocomplete"),
  );
}

async function expectMenuAdjacentToControl(control: Locator, menu: Locator) {
  const [controlBox, menuBox] = await Promise.all([
    control.boundingBox(),
    menu.boundingBox(),
  ]);
  expect(controlBox).not.toBeNull();
  expect(menuBox).not.toBeNull();
  const verticalGap =
    menuBox!.y >= controlBox!.y + controlBox!.height
      ? menuBox!.y - (controlBox!.y + controlBox!.height)
      : controlBox!.y - (menuBox!.y + menuBox!.height);
  expect(verticalGap).toBeGreaterThanOrEqual(0);
  expect(verticalGap).toBeLessThanOrEqual(6);
}

async function capture(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
  name: string,
  options: { fullPage?: boolean; scrollToTop?: boolean } = {},
) {
  const hasOpenMenu = await page
    .locator(".vpsComboboxMenu, .searchExpressionAutocomplete")
    .first()
    .isVisible()
    .catch(() => false);
  if (options.scrollToTop ?? !hasOpenMenu) {
    await page.evaluate(() => window.scrollTo(0, 0));
  }
  await page.waitForTimeout(150);
  const layout = await page.evaluate(() => {
    const viewportWidth = document.documentElement.clientWidth;
    const hasHorizontalScroller = (element: Element) => {
      let current: Element | null = element.parentElement;
      while (current) {
        const style = window.getComputedStyle(current);
        const allowsHorizontalScroll =
          style.overflowX === "auto" ||
          style.overflowX === "scroll" ||
          style.overflow === "auto" ||
          style.overflow === "scroll";
        if (allowsHorizontalScroll && current.scrollWidth > current.clientWidth + 1) {
          return true;
        }
        current = current.parentElement;
      }
      return false;
    };
    const overflowCandidates = Array.from(document.querySelectorAll("*"))
      .map((element) => {
        const rect = element.getBoundingClientRect();
        const style = window.getComputedStyle(element);
        return {
          ariaLabel: element.getAttribute("aria-label") ?? "",
          className: element instanceof HTMLElement ? element.className : "",
          clippedByScroller: hasHorizontalScroller(element),
          display: style.display,
          left: Math.round(rect.left),
          overflowWrap: style.overflowWrap,
          parentClassName:
            element.parentElement instanceof HTMLElement
              ? element.parentElement.className
              : "",
          right: Math.round(rect.right),
          tagName: element.tagName.toLowerCase(),
          text: (element.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 100),
          title: element.getAttribute("title") ?? "",
          whiteSpace: style.whiteSpace,
          width: Math.round(rect.width),
        };
      })
      .filter((entry) => entry.width > 0 && entry.right > viewportWidth + 1)
      .sort((left, right) => right.right - left.right)
      .slice(0, 10);
    const uncontainedOverflowCandidates = overflowCandidates.filter(
      (entry) => !entry.clippedByScroller,
    );
    return {
      horizontalOverflowPx: Math.max(0, document.documentElement.scrollWidth - viewportWidth),
      overflowCandidates,
      uncontainedOverflowCandidates,
      viewportWidth,
    };
  });
  expect(
    layout.uncontainedOverflowCandidates,
    `${name} uncontained horizontal overflow candidates: ${JSON.stringify(layout.overflowCandidates)}`,
  ).toHaveLength(0);
  const screenshot = join(outputDir, `${name}-${page.viewportSize()?.width ?? "viewport"}.png`);
  await page.screenshot({ fullPage: options.fullPage ?? !hasOpenMenu, path: screenshot });
  manifest.push({ name, screenshot, ...layout });
}
