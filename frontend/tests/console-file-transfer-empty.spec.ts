import { expect, test } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page, {
    fileTransferSourceArtifactsOverride: [],
    fileTransfersOverride: [],
  });
});

test("file transfer inventory empty state is covered by screenshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense transfer empty-state screenshot is covered in desktop layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("0 downloads, 0 uploads tracked")).toBeVisible();
  await expect(panel.getByText("No source artifacts")).toBeVisible();
  await expect(panel.getByText("No file transfer sessions")).toBeVisible();
  await expect(panel.getByLabel("File transfer lifecycle summary")).toContainText("0 reusable objects");
  await expect(panel.getByLabel("File transfer lifecycle summary")).toContainText("0 ready, 0 unavailable");

  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("remote-operations-transfers-empty.png"),
  });
});
