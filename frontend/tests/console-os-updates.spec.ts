import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

test("reviews and applies native OS package candidates without hiding distro limits", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "OS updates");

  const panel = page.locator(".osUpdatesPanel");
  await expect(panel.getByText("OS update posture")).toBeVisible();
  const summary = panel.getByLabel("OS update fleet summary");
  await expect(summary.getByText("2 / 3", { exact: true })).toBeVisible();
  await expect(summary.getByText("3", { exact: true })).toBeVisible();

  const grid = panel.getByLabel("Fleet package posture data grid");
  await expect(grid.getByText("edge-sfo-01", { exact: true }).first()).toBeVisible();
  await expect(grid.getByText("core-fra-02", { exact: true }).first()).toBeVisible();
  await expect(grid.getByText("Unchecked", { exact: true })).toBeVisible();
  await expect(grid.locator(".status.warn", { hasText: "Updates" }).first()).toBeVisible();
  await settleScreenshot(page);
  await page.screenshot({
    path: testInfo.outputPath("os-update-posture.png"),
  });

  await invokeOsUpdateAction(
    page,
    grid,
    "agent-sfo-01",
    "Review plan",
    testInfo.project.name,
  );
  await expect(page).toHaveURL(/os_update_client=agent-sfo-01/);
  const detail = page.locator(".consoleDetailPanel", {
    hasText: "edge-sfo-01 package plan",
  });
  await expect(detail).toBeVisible();
  await expect(detail).toBeFocused();
  await expect
    .poll(() =>
      detail.evaluate((element) => {
        const topbar = document.querySelector<HTMLElement>(".topbar");
        if (!topbar) return Number.POSITIVE_INFINITY;
        const topbarPosition = getComputedStyle(topbar).position;
        const visibleTopbarBottom =
          topbarPosition === "sticky" || topbarPosition === "fixed"
            ? Math.max(0, topbar.getBoundingClientRect().bottom)
            : 0;
        return Math.abs(
          element.getBoundingClientRect().top -
            visibleTopbarBottom -
            16,
        );
      }),
    )
    .toBeLessThanOrEqual(2);
  await expect(detail.getByText("Reviewed package candidates")).toBeVisible();
  await expect(detail.getByText("openssl", { exact: true }).first()).toBeVisible();
  await expect(detail.getByText("systemd", { exact: true }).first()).toBeVisible();
  await expect(
    detail.getByRole("button", { name: "Apply all updates" }),
  ).toBeDisabled();

  await unlockPrivilegeFromTop(page);
  const beforeMetadataRefresh = await packageRequestCount(
    page,
    "package_update_plan",
  );
  await detail
    .getByRole("button", { name: "Refresh metadata", exact: true })
    .evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
  await expect.poll(() =>
    packageRequestCount(page, "package_update_plan"),
  ).toBe(beforeMetadataRefresh + 1);
  await expect(
    page.getByText(/reported 2 available updates after refreshing repository metadata/),
  ).toBeVisible();
  expect(await lastPackageRequest(page, "package_update_plan")).toMatchObject({
    confirmed: false,
    operation: {
      expected_provider: "apt",
      refresh_metadata: true,
      type: "package_update_plan",
    },
    privileged: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });

  await activate(
    detail.getByRole("button", { name: "Apply all updates", exact: true }),
  );
  const prompt = page.locator(".confirmationPrompt", {
    hasText: "Confirm OS package update",
  });
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("edge-sfo-01");
  await expect(prompt).toContainText("Ubuntu 22.04");
  await expect(prompt).toContainText("2");
  await expect(prompt).toContainText("Automatic reboot");
  await expect(prompt).toContainText("Never");
  await activate(
    prompt.getByRole("button", { name: "Apply all updates", exact: true }),
  );
  await expect(prompt).toBeHidden();
  await expect(detail.getByText(/2 packages applied; 0 remaining/)).toBeVisible();
  await expect(detail.getByText("No updates in this plan")).toBeVisible();
  expect(await lastPackageRequest(page, "package_update_apply")).toMatchObject({
    command: "package_update_apply",
    confirmed: true,
    destructive: true,
    operation: {
      plan_hash: "a".repeat(64),
      provider: "apt",
      type: "package_update_apply",
    },
    privileged: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });

  await activate(detail.getByRole("button", { name: "Close detail panel" }));
  await expect(page).not.toHaveURL(/os_update_client=/);
  await invokeOsUpdateAction(
    page,
    grid,
    "agent-fra-02",
    "Review plan",
    testInfo.project.name,
  );
  const archDetail = page.locator(".consoleDetailPanel", {
    hasText: "core-fra-02 package plan",
  });
  const archRefresh = archDetail.getByRole("button", {
    name: "Refresh metadata",
    exact: true,
  });
  await expect(archRefresh).toBeDisabled();
  await expect(archRefresh).toHaveAttribute("title", /full system upgrade/);
  await expect(archDetail).toContainText(
    "Pacman metadata refresh is unsupported as a separate action",
  );

  await page.reload();
  await expect(page).toHaveURL(/os_update_client=agent-fra-02/);
  const reloadedDetail = page.locator(".consoleDetailPanel", {
    hasText: "core-fra-02 package plan",
  });
  await expect(reloadedDetail).toBeVisible();
  await expect(reloadedDetail).toBeFocused();
  await expect
    .poll(() =>
      reloadedDetail.evaluate((element) => {
        const topbar = document.querySelector<HTMLElement>(".topbar");
        if (!topbar) return Number.POSITIVE_INFINITY;
        const topbarPosition = getComputedStyle(topbar).position;
        const visibleTopbarBottom =
          topbarPosition === "sticky" || topbarPosition === "fixed"
            ? Math.max(0, topbar.getBoundingClientRect().bottom)
            : 0;
        return Math.abs(
          element.getBoundingClientRect().top -
            visibleTopbarBottom -
            16,
        );
      }),
    )
    .toBeLessThanOrEqual(2);
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
  await settleScreenshot(page);
  await page.screenshot({
    path: testInfo.outputPath("os-updates.png"),
  });
});

test("keeps a stale apply rejection beside the reviewed VPS plan", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "OS updates");
  await unlockPrivilegeFromTop(page);

  const panel = page.locator(".osUpdatesPanel");
  const grid = panel.getByLabel("Fleet package posture data grid");
  await invokeOsUpdateAction(
    page,
    grid,
    "agent-sfo-01",
    "Review plan",
    testInfo.project.name,
  );
  const detail = page.locator(".consoleDetailPanel", {
    hasText: "edge-sfo-01 package plan",
  });
  await activate(
    detail.getByRole("button", { name: "Apply all updates", exact: true }),
  );
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = request?.url ?? String(input);
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (
        method === "POST" &&
        new URL(url, window.location.href).pathname === "/api/v1/jobs"
      ) {
        return new Response(
          JSON.stringify({
            error: "package_update_plan_stale",
            message: "The reviewed package candidate set changed after confirmation.",
            recovery:
              "Check package posture again and review the new plan before applying.",
          }),
          {
            headers: { "content-type": "application/json" },
            status: 409,
          },
        );
      }
      return originalFetch(input, init);
    };
  });
  await activate(
    page
      .getByLabel("Confirm OS package update")
      .getByRole("button", { name: "Apply all updates", exact: true }),
  );

  await expect(page.getByLabel("Confirm OS package update")).toBeHidden();
  await expect(detail.getByText(/Package Update Plan Stale/)).toContainText(
    "review the new plan before applying",
  );
  await expect(
    panel.locator(":scope > .sectionHeader"),
  ).not.toContainText("Package Update Plan Stale");
  await expect(detail.getByText("openssl", { exact: true }).first()).toBeVisible();
});

async function settleScreenshot(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1, 1);
  await page.waitForTimeout(300);
}

async function invokeOsUpdateAction(
  page: Page,
  grid: ReturnType<Page["locator"]>,
  clientId: string,
  action: string,
  projectName: string,
) {
  if (projectName.includes("mobile")) {
    const card = grid.getByLabel(
      `Fleet package posture mobile card ${clientId}`,
    );
    await activate(card.getByRole("button", { name: action, exact: true }));
    return;
  }
  const selectedRows = grid.locator(
    'input[aria-label^="Select Fleet package posture row "]:checked',
  );
  while ((await selectedRows.count()) > 0) {
    await selectedRows.first().uncheck();
  }
  await grid
    .getByLabel(`Select Fleet package posture row ${clientId}`)
    .check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await activate(page.getByRole("menuitem", { name: action, exact: true }));
}

async function lastPackageRequest(page: Page, command: string) {
  return page.evaluate((commandType) => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests
      .filter((request) => request.command === commandType)
      .at(-1);
  }, command);
}

async function packageRequestCount(page: Page, command: string) {
  return page.evaluate((commandType) => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.filter((request) => request.command === commandType).length;
  }, command);
}
