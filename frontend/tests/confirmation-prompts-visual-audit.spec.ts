import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { backupId, installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual confirmation prompt screenshots only");

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures reviewed confirmation prompts in operator workflows", async ({ page }, testInfo) => {
  const outputDir = process.env.VPSMAN_VISUAL_AUDIT_DIR
    ? join(process.env.VPSMAN_VISUAL_AUDIT_DIR, testInfo.project.name)
    : testInfo.outputPath("confirmation-prompts-visual-audit");
  mkdirSync(outputDir, { recursive: true });
  const manifest: Array<Record<string, unknown>> = [];

  await page.goto("/");
  await captureSystemConfigSavePrompt(page, outputDir, manifest);
  await captureTopologyLifecyclePrompt(page, outputDir, manifest);
  await captureTopologySpeedTestPrompt(page, outputDir, manifest);
  await captureTopologySavePrompt(page, outputDir, manifest);
  await captureServerJobCancelPrompt(page, outputDir, manifest);
  await captureBackupRestoreRunPrompt(page, outputDir, manifest);

  writeFileSync(
    join(outputDir, `manifest-${testInfo.project.name}.json`),
    `${JSON.stringify({ screenshots: manifest }, null, 2)}\n`,
  );
});

async function captureSystemConfigSavePrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "System", "Suite config");
  await page.getByLabel("API DB pool").fill("40");
  await page.getByRole("button", { name: "Validate" }).click();
  await expect(page.getByText(/Validation passed/)).toBeVisible();
  await activate(page.getByRole("button", { name: "Review save", exact: true }).first());
  await expect(page.getByLabel("Confirm suite config save")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "system-config-save-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureTopologyLifecyclePrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await openConsoleSubpage(page, "Topology", "Tunnel plans");
  const planGrid = page.getByLabel("Tunnel plans data grid");
  const savedPlanRow = planGrid
    .locator(".gridBody [role=row]", { hasText: "sfo-fra-gre" })
    .first();
  await savedPlanRow.getByLabel("Select Tunnel plans row").check();
  await planGrid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Disable plan" }).click();
  await expect(page.getByLabel("Confirm tunnel plan lifecycle")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "topology-lifecycle-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureTopologySpeedTestPrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Topology", "Tests");
  await activate(page.getByRole("button", { name: "Review speed test" }));
  const prompt = page.getByLabel("Confirm speed test");
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("Speed test");
  await expect(prompt).toContainText("2 VPSs");
  await capture(page, page.locator("main.content"), outputDir, manifest, "topology-speed-test-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureTopologySavePrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await openConsoleSubpage(page, "Topology", "Tunnel plans");
  const composer = page.locator(".scheduleComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await composer.scrollIntoViewIfNeeded();
  await composer.getByLabel("Name", { exact: true }).fill("visual-gre");
  await composer.getByLabel("Interface", { exact: true }).fill("visgre0");
  await chooseVpsBySearch(composer, "Left VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await chooseVpsBySearch(composer, "Right VPS", "fra", /core-fra-02.*agent-fra-02/);
  await expect(composer.getByLabel("Left underlay", { exact: true })).toHaveValue("198.51.100.10");
  await expect(composer.getByLabel("Right underlay", { exact: true })).toHaveValue("203.0.113.20");
  await composer.getByText("Allocation overrides").click();
  await composer.getByLabel("IPv4 pool override", { exact: true }).fill("10.255.60.0/30");
  await activate(composer.getByRole("button", { name: "Allocate endpoints" }));
  await expect(composer.getByLabel("Left IPv4 CIDR", { exact: true })).toHaveValue("10.255.50.0/31");
  await activate(composer.getByRole("button", { name: "Save plan" }));
  await expect(page.getByLabel("Confirm tunnel plan save")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "topology-save-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureServerJobCancelPrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await openConsoleSubpage(page, "Jobs", "Server jobs");
  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await cleanupPanel.getByLabel("Expression").fill('artifact.domain = "file_transfer_source"');
  await cleanupPanel.getByRole("button", { name: "Preview" }).click();
  await expect(cleanupPanel.getByLabel("Preview hash")).toHaveValue(/^[0-9a-f]{64}$/);
  await cleanupPanel.getByRole("button", { name: "Queue cleanup" }).click();
  await expect(page.getByLabel("Confirm artifact cleanup")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "artifact-cleanup-confirm");
  await activate(page.getByLabel("Confirm artifact cleanup").getByRole("button", { name: "Queue cleanup" }));

  const serverJobsPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Server jobs" }),
  });
  await expect(serverJobsPanel).toContainText("queued");
  await activate(serverJobsPanel.getByRole("button", { name: "Cancel" }).first());
  await expect(page.getByLabel("Confirm server job cancellation")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "server-job-cancel-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureBackupRestoreRunPrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Open restore workflow" }));
  const restoreWorkflow = page.getByLabel("Open restore workflow");
  await restoreWorkflow.getByLabel("Restore source backup request").selectOption(backupId);
  await chooseVpsBySearch(restoreWorkflow, "Restore target client", "fra", /core-fra-02.*agent-fra-02/);
  await expect(restoreWorkflow.getByLabel("Staged archive")).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("120");
  await activate(restoreWorkflow.getByRole("button", { name: "Review restore" }));
  await expect(restoreWorkflow.getByLabel("Confirm restore run")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "backup-restore-run-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
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

async function capture(
  page: Page,
  locator: Locator,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
  name: string,
) {
  const prompt = page.locator(".confirmationPrompt").last();
  await assertPromptReady(page, prompt);
  const layout = await collectLayoutSignals(page);
  expect(
    layout.uncontainedOverflowCandidates,
    `${name} uncontained horizontal overflow candidates: ${JSON.stringify(layout.overflowCandidates)}`,
  ).toHaveLength(0);
  const path = join(outputDir, `${name}-${page.viewportSize()?.width ?? "viewport"}.png`);
  await locator.screenshot({ path });
  manifest.push({ name, path, ...layout });
}

async function assertPromptReady(page: Page, prompt: Locator) {
  await expect(prompt).toBeVisible();
  await releaseLiveToolbarFocus(page, prompt);
  await expect
    .poll(() =>
      prompt.evaluate((element) => element === document.activeElement || element.contains(document.activeElement)),
    )
    .toBe(true);
  const viewport = page.viewportSize();
  await expect
    .poll(async () => {
      const box = await prompt.boundingBox();
      if (!box || !viewport) {
        return false;
      }
      return box.y >= 0 && box.y + box.height <= viewport.height;
    })
    .toBe(true);
}

async function releaseLiveToolbarFocus(page: Page, prompt: Locator) {
  const liveToolbarHasFocus = await page.evaluate(() => {
    const active = document.activeElement;
    const toolbar = document.getElementById("impeccable-live-global-bar");
    return Boolean(active && toolbar?.contains(active));
  });
  if (liveToolbarHasFocus) {
    await prompt.evaluate((element) => (element as HTMLElement).focus({ preventScroll: true }));
  }
}

async function collectLayoutSignals(page: Page) {
  return page.evaluate(() => {
    const viewportWidth = document.documentElement.clientWidth;
    const hasHorizontalScroller = (element: Element) => {
      let current: Element | null = element;
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
      .filter((entry) => entry.right > viewportWidth + 1)
      .slice(0, 10);
    return {
      horizontalOverflowPx: Math.max(0, document.documentElement.scrollWidth - viewportWidth),
      overflowCandidates,
      uncontainedOverflowCandidates: overflowCandidates.filter((entry) => !entry.clippedByScroller),
      viewportWidth,
    };
  });
}
