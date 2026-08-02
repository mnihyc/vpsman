import { expect, test, type Locator } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual visual audit screenshots only");

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures network telemetry placements", async ({ page }, testInfo) => {
  const outputDir = testInfo.outputPath("network-telemetry-visual-audit");
  mkdirSync(outputDir, { recursive: true });
  const manifest: Array<Record<string, unknown>> = [];

  await page.goto("/");
  await waitForConsoleShell(page, 15_000);
  await openConsoleSubpage(page, "Fleet", "Instances");
  const fleetGrid = page.getByLabel("VPS instance records data grid");
  let coreDetail: Locator;
  if (testInfo.project.name.includes("mobile")) {
    const coreCard = fleetGrid.getByLabel(
      "VPS instance records mobile card agent-fra-02",
    );
    await expect(coreCard).toBeVisible();
    await coreCard.getByRole("checkbox").check();
    await fleetGrid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await page
      .getByRole("menuitem", { name: "Open detail", exact: true })
      .click();
    coreDetail = page.getByLabel("Canonical VPS detail");
    await coreDetail.getByLabel("VPS detail section").selectOption("Network");
  } else {
    const coreRow = fleetGrid
      .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
      .first();
    await expect(coreRow).toBeVisible();
    await activate(coreRow.getByLabel("Expand VPS instance records row"));
    coreDetail = fleetGrid
      .locator(".gridExpandedRow", { hasText: "core-fra-02" })
      .first();
    await activate(coreDetail.getByRole("tab", { name: "Network" }));
  }
  if (testInfo.project.name.includes("mobile")) {
    await expect(coreDetail).toContainText("core-fra-02");
    await expect(coreDetail).toContainText("agent-fra-02");
    await expect(coreDetail.getByText("Network workflow", { exact: true })).toBeVisible();
    await expect(coreDetail.getByText("Observed interfaces", { exact: true })).toBeVisible();
    await expect(coreDetail.getByText("Tunnel records", { exact: true })).toBeVisible();
    await expect(coreDetail.getByText("Latest observations", { exact: true })).toBeVisible();
  } else {
    await expect(coreDetail).toContainText("core-fra-02 (ra02)");
    await expect(coreDetail.getByText("Runtime tunnels", { exact: true })).toBeVisible();
    await expect(coreDetail.getByText("Latency Probe failed")).toBeVisible();
    await expect(coreDetail.getByText("sfo-fra-gre", { exact: true })).toBeVisible();
    const declaredGre = coreDetail
      .locator(".telemetryTunnelRow", { hasText: "sfo-fra-gre" })
      .first();
    await expect(
      declaredGre.getByText(/right endpoint; peer agent-sfo-01/),
    ).toBeVisible();
    await expect(coreDetail.getByText("latest interface rate bucket")).toBeVisible();
  }
  await capture(page, coreDetail, outputDir, manifest, "fleet-network-detail");

  await openConsoleSubpage(page, "Network", "Graph");
  await expect(page.getByRole("heading", { name: "Topology graph" })).toBeVisible();
  const graphPanel = page.locator(".topologyGraphPanel");
  await expect(
    graphPanel.getByText("1 visible tunnel", { exact: true }),
  ).toBeVisible();
  await expect(
    graphPanel
      .getByLabel("Topology graph legend")
      .getByText("OSPF 22 (+8)", { exact: true }),
  ).toBeVisible();
  await expect(
    graphPanel.getByText("Why OSPF cost changed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByLabel("OSPF updater plans data grid"),
  ).toHaveCount(0);
  await capture(page, page.locator("main.content"), outputDir, manifest, "topology-graph");

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
