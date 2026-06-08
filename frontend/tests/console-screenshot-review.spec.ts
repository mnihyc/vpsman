import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";

const desktopViews = [
  { heading: "Dashboard", id: "dashboard", view: "Dashboard" },
  { heading: "Fleet overview", id: "fleet", view: "Fleet" },
  { heading: "Config", id: "config-overview", view: "Config" },
  { heading: "Config", id: "config-rules", subpage: "Rules", view: "Config" },
  { heading: "Config", id: "config-bulk", subpage: "Bulk apply", view: "Config" },
  { heading: "Config", id: "config-single", subpage: "Single VPS", view: "Config" },
  { heading: "Tags management", id: "tags-registry", view: "Tags" },
  { heading: "Tags management", id: "tags-assignments", subpage: "Assignments", view: "Tags" },
  { heading: "Tags management", id: "tags-bulk", subpage: "Bulk", view: "Tags" },
  { heading: "Job history", id: "jobs", view: "Jobs" },
  { heading: "Schedules", id: "schedules", view: "Schedules" },
  { heading: "Topology management", id: "topology", view: "Topology" },
  { heading: "Backups management", id: "backups", view: "Backups" },
  { heading: "Audit log", id: "audit", view: "Audit" },
  { heading: "Access management", id: "access", view: "Access" },
] as const;

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures main console screenshots for regression review", async ({
  page,
}, testInfo) => {
  const reviewRoot =
    process.env.VPSMAN_SCREENSHOT_REVIEW_DIR ??
    testInfo.outputPath("screenshots");
  const projectDir = join(reviewRoot, testInfo.project.name);
  mkdirSync(projectDir, { recursive: true });

  await page.goto("/");
  const views = testInfo.project.name.includes("mobile")
    ? desktopViews.slice(0, 1)
    : desktopViews;
  const manifest: Array<Record<string, unknown>> = [];

  for (const entry of views) {
    if (entry.view !== "Dashboard") {
      const nav = page.getByRole("navigation", { name: "Primary console navigation" });
      await activate(
        nav.getByRole("button", {
          name: entry.view,
          exact: true,
        }),
      );
      if ("subpage" in entry) {
        await activate(nav.getByRole("button", { name: entry.subpage, exact: true }));
      }
    }
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

    manifest.push({
      heading: entry.heading,
      horizontal_overflow_px: layout.horizontalOverflowPx,
      project: testInfo.project.name,
      screenshot: screenshotPath,
      subpage: "subpage" in entry ? entry.subpage : null,
      view: entry.view,
      visible_text_length: layout.visibleTextLength,
    });
  }

  const manifestPath = join(
    projectDir,
    `manifest-${testInfo.project.name}.json`,
  );
  writeFileSync(
    manifestPath,
    `${JSON.stringify({ generated_by: "console-screenshot-review", views: manifest }, null, 2)}\n`,
  );
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
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
