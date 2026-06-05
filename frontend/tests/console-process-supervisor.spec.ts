import { expect, test, type Locator } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

test("shows restart and desired-only limit evidence in process supervisor inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense process inventory evidence is covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Processes");

  const inventory = page.locator(".fleetPanel", { hasText: "Process supervisor inventory" });
  await expect(inventory.getByText("ospf-worker")).toBeVisible();
  await expect(inventory.getByText("restarts 1; last exit 7")).toBeVisible();
  await expect(inventory.getByText("limits desired; 2 procs, cpu 39, 1.0M mem")).toBeVisible();
});
