import { expect, test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, openConsoleSubpage } from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual visual audit screenshots only");

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures network telemetry placements", async ({ page }, testInfo) => {
  const outputDir = testInfo.outputPath("network-telemetry-visual-audit");
  mkdirSync(outputDir, { recursive: true });
  const manifest: Array<Record<string, unknown>> = [];

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");
  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const coreRow = fleetGrid.locator(".gridBody [role=row]", { hasText: "core-fra-02" }).first();
  await expect(coreRow).toBeVisible();
  await activate(coreRow.getByLabel("Expand VPS instance records row"));
  const coreDetail = fleetGrid.locator(".gridExpandedRow", { hasText: "core-fra-02" }).first();
  await expect(coreDetail.getByRole("heading", { name: "core-fra-02 (ra02)" })).toBeVisible();
  await activate(coreDetail.getByRole("tab", { name: "Network" }));
  await expect(coreDetail.getByText("Runtime tunnels", { exact: true })).toBeVisible();
  await expect(coreDetail.getByText("Latency Down")).toBeVisible();
  await expect(coreDetail.getByText("OSPF Report only")).toBeVisible();
  await expect(coreDetail.getByText("latest interface rate bucket")).toBeVisible();
  await capture(page, coreDetail, outputDir, manifest, "fleet-network-detail");

  await openConsoleSubpage(page, "Topology", "Graph");
  await expect(page.getByRole("heading", { name: "Topology graph" })).toBeVisible();
  await expect(page.getByText("Latency and auto OSPF")).toBeVisible();
  await expect(page.getByText("down 1")).toBeVisible();
  await expect(page.getByText("2 latest tunnel reports")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "topology-automation");

  writeFileSync(
    join(outputDir, `manifest-${testInfo.project.name}.json`),
    `${JSON.stringify({ screenshots: manifest }, null, 2)}\n`,
  );
});

async function capture(
  page: import("@playwright/test").Page,
  locator: import("@playwright/test").Locator,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
  name: string,
) {
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
    const candidates = Array.from(document.querySelectorAll("*"))
      .map((element) => {
        const rect = element.getBoundingClientRect();
        return {
          className: element instanceof HTMLElement ? element.className : "",
          clippedByScroller: hasHorizontalScroller(element),
          right: Math.round(rect.right),
          tagName: element.tagName.toLowerCase(),
          text: (element.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 100),
        };
      })
      .filter((entry) => entry.right > viewportWidth + 1)
      .slice(0, 10);
    const uncontainedOverflowCandidates = candidates.filter(
      (entry) => !entry.clippedByScroller,
    );
    return {
      horizontalOverflowPx: Math.max(0, document.documentElement.scrollWidth - viewportWidth),
      overflowCandidates: candidates,
      uncontainedOverflowCandidates,
      viewportWidth,
    };
  });
  expect(
    layout.uncontainedOverflowCandidates,
    `${name} uncontained horizontal overflow candidates: ${JSON.stringify(layout.overflowCandidates)}`,
  ).toHaveLength(0);
  const path = join(outputDir, `${name}-${page.viewportSize()?.width ?? "viewport"}.png`);
  await locator.screenshot({ path });
  manifest.push({ name, path, ...layout });
}
