import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { backupId, installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
  waitForConsoleShell,
} from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual confirmation prompt screenshots only");
test.setTimeout(120_000);

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
  await waitForConsoleShell(page, 15_000);
  await captureSystemConfigSavePrompt(page, outputDir, manifest);
  await captureTopologyLifecyclePrompt(page, outputDir, manifest);
  await captureTopologySpeedTestPrompt(page, outputDir, manifest);
  await captureTopologySavePrompt(page, outputDir, manifest);
  await captureArtifactDeletionPrompt(page, outputDir, manifest);
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
  await activate(
    page
      .getByLabel("Suite config sections")
      .getByRole("button", { name: /Capacity/ }),
  );
  await page.getByLabel("API DB pool").fill("40");
  await expect(page.getByLabel("Suite config sticky save bar")).toContainText(
    "1 changed key",
  );
  await activate(
    page.getByRole("button", { name: "Review changes", exact: true }).first(),
  );
  await expect(page.getByLabel("Confirm suite config save")).toBeVisible();
  await capture(page, outputDir, manifest, "system-config-save-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureTopologyLifecyclePrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Disable sfo-fra-gre" }));
  await expect(page.getByLabel("Confirm tunnel plan disable")).toBeVisible();
  await capture(page, outputDir, manifest, "topology-lifecycle-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureTopologySpeedTestPrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Network", "Tests");
  await activate(page.getByRole("button", { name: "Review speed test" }));
  const prompt = page.getByLabel("Confirm speed test");
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("Speed test");
  await expect(prompt).toContainText("2 VPSs");
  await capture(page, outputDir, manifest, "topology-speed-test-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureTopologySavePrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Create plan" }));
  const composer = page.locator(".tunnelPlanComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await composer.scrollIntoViewIfNeeded();
  await composer.getByLabel("Tunnel plan name").fill("visual-gre");
  await composer.getByLabel("Tunnel interface", { exact: true }).fill("visgre0");
  await chooseVpsBySearch(composer, "Left tunnel VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await chooseVpsBySearch(composer, "Right tunnel VPS", "fra", /core-fra-02.*agent-fra-02/);
  await composer.getByLabel("Left remote underlay destination").fill("203.0.113.20");
  await composer.getByLabel("Left local underlay source").fill("10.0.0.10");
  await composer.getByLabel("Right remote underlay destination").fill("198.51.100.10");
  await composer.getByLabel("Right local underlay source").fill("10.0.1.20");
  await composer.getByLabel("IPv4 allocation pool").fill("10.255.60.0/30");
  await activate(composer.getByRole("button", { name: "Allocate" }));
  await expect(composer.getByLabel("Left tunnel IPv4")).toHaveValue("10.255.50.0");
  await activate(composer.getByRole("button", { name: "Review plan" }));
  const prompt = page.getByLabel("Confirm tunnel plan creation");
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("Left outer path");
  await expect(prompt).toContainText("Source 10.0.0.10 -> destination 203.0.113.20");
  await expect(prompt).toContainText("Right outer path");
  await expect(prompt).toContainText("Source 10.0.1.20 -> destination 198.51.100.10");
  await capture(page, outputDir, manifest, "topology-save-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureArtifactDeletionPrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await openConsoleSubpage(page, "System", "Maintenance");
  const cleanupPanel = page.locator(".fleetPanel").filter({
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  await cleanupPanel.getByLabel("Older than days").fill("");
  await cleanupPanel.getByText("Advanced expression").click();
  await cleanupPanel
    .getByRole("textbox", { name: "Expression", exact: true })
    .fill('artifact.domain = "file_transfer_source"');
  await cleanupPanel.getByRole("button", { name: "Preview" }).click();
  await expect(cleanupPanel.getByLabel("Artifact cleanup readiness")).toContainText("Ready for confirmation");
  await cleanupPanel.getByRole("button", { name: "Delete artifacts" }).click();
  await expect(page.getByRole("region", { name: "Confirm artifact deletion" })).toBeVisible();
  await capture(page, outputDir, manifest, "artifact-cleanup-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
}

async function captureBackupRestoreRunPrompt(
  page: Page,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Choose restore artifact" }));
  const restoreWorkflow = page.getByLabel("Choose restore artifact");
  await restoreWorkflow.getByLabel("Restore source backup request").selectOption(backupId);
  await chooseVpsBySearch(restoreWorkflow, "Restore target client", "fra", /core-fra-02.*agent-fra-02/);
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review draft restore" }),
  );
  await activate(
    restoreWorkflow
      .getByLabel("Confirm draft restore")
      .getByRole("button", { name: "Save draft restore" }),
  );
  await expect(restoreWorkflow.getByLabel("Staged archive")).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("120");
  await activate(restoreWorkflow.getByRole("button", { name: "Review dry run" }));
  await expect(restoreWorkflow.getByLabel("Confirm restore")).toBeVisible();
  await capture(page, outputDir, manifest, "backup-restore-run-confirm");
  await activate(page.getByRole("button", { name: "Close confirmation" }));
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

async function capture(
  page: Page,
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
  await page.screenshot({ fullPage: false, path });
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
      if (!viewport) {
        return "missing viewport";
      }
      return prompt.evaluate((element, viewportHeight) => {
        const box = element.getBoundingClientRect();
        const isOverlay = element.classList.contains("overlayPrompt");
        if (isOverlay) {
          return box.top >= 0 && box.bottom <= viewportHeight
            ? "ready"
            : JSON.stringify({
                bottom: Math.round(box.bottom),
                height: Math.round(box.height),
                mode: "overlay",
                top: Math.round(box.top),
                viewportHeight,
              });
        }

        const content = element.closest<HTMLElement>(".content");
        const topbar = content?.querySelector<HTMLElement>(":scope > .topbar");
        const topbarPosition = topbar
          ? window.getComputedStyle(topbar).position
          : "static";
        const topbarBottom =
          topbar && (topbarPosition === "sticky" || topbarPosition === "fixed")
            ? Math.max(0, topbar.getBoundingClientRect().bottom)
            : 0;
        let clippingAncestor = element.parentElement;
        while (clippingAncestor) {
          const style = window.getComputedStyle(clippingAncestor);
          const clipsVertically = ["auto", "clip", "hidden", "scroll"].includes(
            style.overflowY,
          );
          if (
            clipsVertically &&
            clippingAncestor.scrollHeight > clippingAncestor.clientHeight + 1
          ) {
            break;
          }
          clippingAncestor = clippingAncestor.parentElement;
        }
        const clippingBox = clippingAncestor?.getBoundingClientRect();
        const visibleTop = Math.max(0, topbarBottom, clippingBox?.top ?? 0);
        const visibleBottom = Math.min(
          viewportHeight,
          clippingBox?.bottom ?? viewportHeight,
        );
        const topInset = visibleTop + 12;
        const bottomInset = visibleBottom - 12;
        const availableHeight = Math.max(0, bottomInset - topInset);
        const ready =
          box.height > availableHeight
            ? box.top >= visibleTop && box.top <= topInset + 4
            : box.top >= visibleTop && box.bottom <= bottomInset + 1;
        if (ready) {
          return "ready";
        }
        const drawerBody = element.closest<HTMLElement>(".actionDrawerBody");
        const drawerBodyBox = drawerBody?.getBoundingClientRect();
        return JSON.stringify({
          bottom: Math.round(box.bottom),
          contentScrollTop: content?.scrollTop ?? null,
          drawerBodyBottom: drawerBodyBox ? Math.round(drawerBodyBox.bottom) : null,
          drawerBodyScrollTop: drawerBody?.scrollTop ?? null,
          drawerBodyTop: drawerBodyBox ? Math.round(drawerBodyBox.top) : null,
          height: Math.round(box.height),
          mode: "inline",
          top: Math.round(box.top),
          visibleTop: Math.round(visibleTop),
          viewportHeight,
        });
      }, viewport.height);
    })
    .toBe("ready");
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
