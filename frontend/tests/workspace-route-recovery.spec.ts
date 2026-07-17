import { expect, test } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, waitForConsoleShell } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("console shell recovers from two interrupted entry-module loads", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared shell recovery is exercised once on desktop",
  );

  let mainModuleRequests = 0;
  await page.route("**/src/main.tsx", async (route) => {
    mainModuleRequests += 1;
    if (mainModuleRequests <= 2) {
      await route.abort("connectionreset");
      return;
    }
    await route.continue();
  });

  await page.goto("/");
  await waitForConsoleShell(page);

  await expect(page.locator(".shell")).toBeVisible();
  expect(mainModuleRequests).toBe(3);
});

test("console startup presents recovery after its bounded retries", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared startup recovery is exercised once on desktop",
  );

  let mainModuleRequests = 0;
  await page.route("**/src/main.tsx", async (route) => {
    mainModuleRequests += 1;
    if (mainModuleRequests <= 3) {
      await route.abort("connectionreset");
      return;
    }
    await route.continue();
  });

  await page.goto("/#/system/preferences");

  const recovery = page.getByRole("alert");
  await expect(
    recovery.getByRole("heading", { name: "Console could not load" }),
  ).toBeVisible({ timeout: 10_000 });
  await expect(recovery).toContainText(
    "Startup was interrupted after two automatic retries",
  );
  await expect(recovery).toContainText("No operation was submitted");
  expect(mainModuleRequests).toBe(3);

  await waitForConsoleShell(page);
  await expect(recovery).toHaveCount(0);
  await expect(page).toHaveURL(/#\/system\/preferences$/);
  expect(mainModuleRequests).toBe(4);
});

test("console checks do not hide non-transient startup defects", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared startup failure classification is exercised once on desktop",
  );

  let mainModuleRequests = 0;
  await page.route("**/src/main.tsx", async (route) => {
    mainModuleRequests += 1;
    await route.fulfill({
      body: 'throw new Error("synthetic non-transient startup defect");',
      contentType: "text/javascript",
    });
  });

  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Console could not load" }),
  ).toBeVisible({ timeout: 10_000 });
  await expect(waitForConsoleShell(page, 500)).rejects.toThrow(
    "synthetic non-transient startup defect",
  );
  expect(mainModuleRequests).toBe(3);
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

test.describe("without bootstrap JavaScript", () => {
  test.use({ javaScriptEnabled: false });

  test("console startup still exposes a working reload action", async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "the shared static startup fallback is exercised once on desktop",
    );

    await page.goto("/");

    const recovery = page.getByRole("alert");
    await expect(
      recovery.getByRole("heading", { name: "Console could not load" }),
    ).toBeVisible({ timeout: 12_000 });
    await expect(recovery).toContainText(
      "Startup was interrupted. No operation was submitted.",
    );
    await expect(
      recovery.getByRole("link", { name: "Reload console" }),
    ).toHaveAttribute("href", "./");
  });
});
