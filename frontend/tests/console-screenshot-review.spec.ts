import { expect, test, type Page } from "@playwright/test";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

const desktopViews = [
  { heading: "Home", id: "home-overview", subpage: "Overview", view: "Home" },
  { heading: "Fleet instances", id: "fleet-instances", subpage: "Instances", view: "Fleet" },
  { heading: "Terminal", id: "remote-operations-terminal", subpage: "Terminal", view: "Remote Operations" },
  { heading: "Job history", id: "jobs-history", subpage: "History", view: "Jobs" },
  { heading: "Schedules", id: "automation-schedules", subpage: "Schedules", view: "Automation" },
  { heading: "Network overview", id: "network-overview", subpage: "Overview", view: "Network" },
  { heading: "Backup overview", id: "backups-overview", subpage: "Overview", view: "Backups" },
  { heading: "Config", id: "config-overview", subpage: "Overview", view: "Config" },
  { heading: "Fleet metrics", id: "observability-fleet-metrics", subpage: "Fleet metrics", view: "Observability" },
  { heading: "Audit events", id: "audit-events", subpage: "Events", view: "Audit" },
  { heading: "Access overview", id: "access-overview", subpage: "Overview", view: "Access" },
  { heading: "System overview", id: "system-overview", subpage: "Overview", view: "System" },
] as const;

type ManifestEntry = {
  heading: string;
  horizontal_overflow_px: number;
  project: string;
  screenshot: string;
  subpage: string;
  view: string;
  visible_text_length: number;
};

for (const entry of desktopViews) {
  test(`captures ${entry.id} for regression review`, async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile") && entry !== desktopViews[0],
      "the mobile review covers the home overview",
    );

    await installConsoleApiMock(page);
    const reviewRoot =
      process.env.VPSMAN_SCREENSHOT_REVIEW_DIR ??
      join(testInfo.project.outputDir, "console-screenshot-review");
    const projectDir = join(reviewRoot, testInfo.project.name);
    mkdirSync(projectDir, { recursive: true });

    await page.goto("/");
    await openConsoleSubpage(page, entry.view, entry.subpage);
    await expect(
      page
        .locator(".consoleHeader")
        .getByRole("heading", { name: entry.heading }),
    ).toBeVisible();
    await expect(
      page.getByText(/Http 404 \(404\)|HTTP 404 \(404\)/),
    ).toHaveCount(0);
    const layout = await collectLayoutSignals(page);
    expect(
      layout.horizontalOverflowPx,
      `${entry.view} horizontal overflow candidates: ${JSON.stringify(layout.overflowCandidates)}`,
    ).toBeLessThanOrEqual(1);
    expect(
      layout.visibleTextLength,
      `${entry.view} visible text length`,
    ).toBeGreaterThan(200);
    expect(
      layout.blankMain,
      `${entry.view} main content should not be blank`,
    ).toBe(false);

    const screenshotPath = join(
      projectDir,
      `${entry.id}-${testInfo.project.name}.png`,
    );
    const screenshot = await page.screenshot({
      fullPage: true,
      path: screenshotPath,
    });
    expect(
      screenshot.length,
      `${entry.view} screenshot should not be empty`,
    ).toBeGreaterThan(12_000);

    upsertManifest(projectDir, testInfo.project.name, {
      heading: entry.heading,
      horizontal_overflow_px: layout.horizontalOverflowPx,
      project: testInfo.project.name,
      screenshot: screenshotPath,
      subpage: entry.subpage,
      view: entry.view,
      visible_text_length: layout.visibleTextLength,
    });
  });
}

function upsertManifest(
  projectDir: string,
  projectName: string,
  entry: ManifestEntry,
) {
  const manifestPath = join(projectDir, `manifest-${projectName}.json`);
  let manifest: ManifestEntry[] = [];
  if (existsSync(manifestPath)) {
    const parsed = JSON.parse(readFileSync(manifestPath, "utf8")) as {
      views?: ManifestEntry[];
    };
    manifest = parsed.views ?? [];
  }
  const byView = new Map(manifest.map((item) => [item.view, item]));
  byView.set(entry.view, entry);
  const orderedManifest = desktopViews.flatMap((view) => {
    const item = byView.get(view.view);
    return item ? [item] : [];
  });
  writeFileSync(
    manifestPath,
    `${JSON.stringify({ generated_by: "console-screenshot-review", views: orderedManifest }, null, 2)}\n`,
  );
}

async function collectLayoutSignals(page: Page) {
  return page.evaluate(() => {
    const main = document.querySelector("main.content");
    const visibleText = main?.textContent?.replace(/\s+/g, " ").trim() ?? "";
    const viewportWidth = document.documentElement.clientWidth;
    const overflowCandidates = Array.from(document.querySelectorAll("*"))
      .map((element) => {
        const rect = element.getBoundingClientRect();
        return {
          className: element instanceof HTMLElement ? element.className : "",
          right: Math.round(rect.right),
          tagName: element.tagName.toLowerCase(),
          text: (element.textContent ?? "")
            .replace(/\s+/g, " ")
            .trim()
            .slice(0, 80),
          width: Math.round(rect.width),
        };
      })
      .filter((item) => item.right > viewportWidth + 1 && item.width > 0)
      .sort((left, right) => right.right - left.right)
      .slice(0, 8);
    return {
      blankMain: visibleText.length === 0,
      horizontalOverflowPx:
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
      overflowCandidates,
      visibleTextLength: visibleText.length,
    };
  });
}
