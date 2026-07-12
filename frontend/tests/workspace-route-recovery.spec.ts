import { expect, test } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, waitForConsoleShell } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("a hung workspace chunk becomes recoverable instead of loading forever", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared lazy workspace boundary is exercised once on desktop",
  );

  let chunkRequests = 0;
  let releaseFirstRequest: (() => void) | undefined;
  await page.route("**/*BackupsPanel*", async (route) => {
    chunkRequests += 1;
    if (chunkRequests === 1) {
      await new Promise<void>((resolve) => {
        releaseFirstRequest = resolve;
      });
      await route.abort("timedout").catch(() => undefined);
      return;
    }
    await route.continue();
  });

  await page.goto("/");
  await waitForConsoleShell(page);
  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  await activate(nav.getByRole("button", { name: "Backups", exact: true }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup overview" }),
  ).toBeVisible();
  await expect(page.getByText("Loading backups workspace")).toBeVisible();

  const recovery = page.locator(".workspaceRouteError");
  await expect(
    recovery.getByRole("heading", { name: "Workspace did not load" }),
  ).toBeVisible({ timeout: 20_000 });
  await expect(recovery).toContainText(
    "Workspace module load timed out after 15000ms",
  );

  releaseFirstRequest?.();
  await activate(recovery.getByRole("button", { name: "Reload console" }));
  await waitForConsoleShell(page);
  await expect(
    page.getByRole("heading", { level: 1, name: "Backup overview" }),
  ).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText("Loading backups workspace")).toHaveCount(0);
  await expect(recovery).toHaveCount(0);
  expect(chunkRequests).toBeGreaterThanOrEqual(2);
});
