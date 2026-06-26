import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, openConsoleSubpage } from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual selector visual audit screenshots only");
test.setTimeout(90_000);

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures exact VPS selector states", async ({ page }, testInfo) => {
  const outputDir = testInfo.outputPath("selector-visual-audit");
  mkdirSync(outputDir, { recursive: true });
  const manifest: Array<Record<string, unknown>> = [];

  await page.goto("/");
  await page.getByLabel("Home scope kind").selectOption("client");
  await openVpsMenu(page.locator(".dashboardControlBar"), "Home scope value", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "dashboard-client-scope");

  await openConsoleSubpage(page, "Config", "Per-VPS");
  await openVpsMenu(page.locator(".configApplyGrid"), "VPS config target", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "config-single-target");

  await openConsoleSubpage(page, "Remote Operations", "Files");
  await openVpsMenu(page.locator(".fileBrowserPanel"), "File browser target VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "file-browser-target");

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const tunnelComposer = page.locator(".scheduleComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await openVpsMenu(tunnelComposer, "Left VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await openVpsMenu(tunnelComposer, "Right VPS", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "topology-tunnel-targets");

  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Open restore workflow" }));
  const restoreWorkflow = page.getByLabel("Open restore workflow");
  await restoreWorkflow.getByLabel("Restore source backup request").selectOption({ index: 1 });
  await openVpsMenu(restoreWorkflow, "Restore target client", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "restore-target");
  await restoreWorkflow.getByRole("option", { name: /core-fra-02.*agent-fra-02/ }).click();
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

  await openConsoleSubpage(page, "Fleet", "Alert policies");
  await activate(page.getByRole("button", { name: "Create policy" }).first());
  const policyEditor = page.locator(".consoleDetailPanel", {
    hasText: "Create alert policy",
  }).last();
  await expect(policyEditor).toBeVisible();
  const policyExpression = policyEditor.getByRole("searchbox", {
    name: "Policy VPS selector expression",
  });
  await policyExpression.fill("tag:edge && status:online");
  await expect(policyExpression).toContainText("tag:edge && status:online");
  await capture(page, outputDir, manifest, "fleet-policy-expression-filter");
  await activate(policyEditor.getByLabel("Close detail panel"));
  const policyGrid = page.getByLabel("Policy groups data grid");
  await openExpressionMenu(policyGrid, "Policy groups search", "enabled", /^enabled$/);
  await capture(page, outputDir, manifest, "fleet-policy-grid-search-suggestion");

  await openConsoleSubpage(page, "Fleet", "Notifications");
  const notifications = page.locator(".consoleCrudPanel", {
    has: page.getByText("Alert notification channels", { exact: true }),
  });
  await activate(notifications.getByRole("button", { name: "Create channel" }).first());
  const channelEditor = notifications.locator(".consoleDetailPanel", {
    hasText: "Create notification channel",
  }).last();
  await expect(channelEditor).toBeVisible();
  await channelEditor.getByLabel("Notification scope kind").selectOption("client");
  await openVpsMenu(channelEditor, "Notification scope value", "fra", /core-fra-02.*agent-fra-02/);
  await capture(page, outputDir, manifest, "fleet-notification-client-scope");
  await activate(channelEditor.getByLabel("Close detail panel"));
  await activate(page.getByRole("tab", { name: "Webhooks" }));
  await expect(page.getByText("Webhook rules", { exact: true }).first()).toBeVisible();
  await activate(page.getByRole("button", { name: "Create rule" }).first());
  await openExpressionMenu(
    page.locator("main"),
    "Webhook expression",
    "interval.",
    /^interval\.30sec$/,
  );
  await capture(page, outputDir, manifest, "fleet-webhook-expression-event-search");

  await openConsoleSubpage(page, "Jobs", "Dispatch");
  const dispatchComposer = page.locator(".commandComposer");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "name:s", /edge-sfo-01.*Name.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "dispatch-expression-name-search");
  await page.keyboard.press("Enter");
  await expect(dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" })).toContainText("name:edge-sfo-01");
  await capture(page, outputDir, manifest, "dispatch-expression-name-selected");
  await dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "fo01", /edge-sfo-01.*ID.*agent-sfo-01/);
  await capture(page, outputDir, manifest, "dispatch-expression-id-suffix-search");
  await dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "status:on", /^status:online$/);
  await capture(page, outputDir, manifest, "dispatch-expression-status-search");
  await dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "vps.status:on", /^vps\.status:online$/);
  await capture(page, outputDir, manifest, "dispatch-expression-vps-status-search");
  await dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "role:e", /^role:edge$/);
  await capture(page, outputDir, manifest, "dispatch-expression-unknown-namespace-search");
  await dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" }).fill("");
  await openExpressionMenu(dispatchComposer, "Bulk target selector expression", "*", /^\*$/);
  await capture(page, outputDir, manifest, "dispatch-expression-all-wildcard-search");
  const longExpression = dispatchComposer.getByRole("searchbox", { name: "Bulk target selector expression" });
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
  await expect(root.getByRole("option", { name: expectedOption })).toBeVisible();
}

async function openExpressionMenu(
  root: Locator,
  label: string,
  query: string,
  expectedOption: RegExp,
) {
  const searchbox = root.getByRole("searchbox", { name: label });
  await expect(searchbox).toBeVisible();
  await searchbox.click();
  await searchbox.fill("");
  await searchbox.click();
  await searchbox.pressSequentially(query);
  await expect(root.getByRole("option", { name: expectedOption })).toBeVisible();
}

async function capture(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
  name: string,
  options: { fullPage?: boolean; scrollToTop?: boolean } = {},
) {
  if (options.scrollToTop ?? true) {
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
        return {
          className: element instanceof HTMLElement ? element.className : "",
          clippedByScroller: hasHorizontalScroller(element),
          right: Math.round(rect.right),
          tagName: element.tagName.toLowerCase(),
          text: (element.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 100),
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
  await page.screenshot({ fullPage: options.fullPage ?? true, path: screenshot });
  manifest.push({ name, screenshot, ...layout });
}
