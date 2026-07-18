import { expect, test, type Page } from "@playwright/test";
import {
  hostServiceInventory,
  installConsoleApiMock,
} from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

test("keeps host services routable and exposes logs plus snapshot-bound actions", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Services");

  const panel = page.locator(".hostServicesPanel");
  await expect(panel.getByText("Choose a VPS")).toBeVisible();
  await panel.getByLabel("Service inventory VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();

  await expect(page).toHaveURL(/service_client=agent-sfo-01/);
  const summary = panel.getByLabel("Service capability summary");
  await expect(summary.getByText("systemd", { exact: true })).toBeVisible();
  await expect(summary.getByText("1 / 3", { exact: true })).toBeVisible();
  await expect(summary.getByText("1", { exact: true }).first()).toBeVisible();
  await expect(panel.getByText("sshd.service", { exact: true }).first()).toBeVisible();
  await expect(panel.getByText("example-worker.service", { exact: true }).first()).toBeVisible();

  const beforeRefresh = await serviceInventoryRequestCount(page);
  await panel
    .getByRole("button", { name: "Refresh inventory", exact: true })
    .evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
  await expect.poll(() => serviceInventoryRequestCount(page)).toBe(
    beforeRefresh + 1,
  );
  await expect(
    panel.getByText(/Service inventory refreshed from edge-sfo-01/),
  ).toBeVisible();

  const grid = panel.getByLabel("Host service inventory data grid");
  await expect(grid.locator(".status.danger", { hasText: "Failed" })).toBeVisible();
  if (testInfo.project.name.includes("mobile")) {
    const card = grid.getByLabel(
      "Host service inventory mobile card sshd.service",
    );
    await activate(
      card.getByRole("button", {
        name: "Show details for Host service inventory row sshd.service",
      }),
    );
  } else {
    await activate(grid.getByText("sshd.service", { exact: true }).first());
  }
  await expect(grid.getByText("Provider evidence")).toBeVisible();
  await expect(
    grid.getByRole("button", {
      name: "Close Host service inventory row details",
    }),
  ).toBeVisible();

  await invokeServiceAction(page, grid, "sshd.service", "Logs", testInfo.project.name);
  const logs = page.locator(".consoleDetailPanel", { hasText: "sshd.service logs" });
  await expect(logs).toBeVisible();
  await expect(logs).toContainText("Server listening on 0.0.0.0 port 22");
  await expect(logs).toContainText("Accepted publickey for operator");
  await activate(logs.getByRole("button", { name: "Close detail panel" }));
  await expect(logs).toBeHidden();

  await unlockPrivilegeFromTop(page);
  await invokeServiceAction(
    page,
    grid,
    "sshd.service",
    "Restart",
    testInfo.project.name,
  );
  const prompt = page.locator(".confirmationPrompt", {
    hasText: "Confirm service action",
  });
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("edge-sfo-01");
  await expect(prompt).toContainText("systemd");
  await expect(prompt).toContainText("sshd.service");
  await expect(prompt).toContainText("Active");
  await expect(prompt).toContainText("Enabled");
  await activate(prompt.getByRole("button", { name: "Restart service" }));
  await expect(prompt).toBeHidden();
  await expect(
    panel.getByText(/Restarted sshd\.service on edge-sfo-01/),
  ).toBeVisible();

  expect(await lastServiceActionRequest(page)).toMatchObject({
    command: "service_action",
    confirmed: true,
    operation: {
      action: "restart",
      expected_active_state: "active",
      expected_enabled_state: "enabled",
      provider: "systemd",
      service: "sshd.service",
      type: "service_action",
    },
    privileged: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });

  await page.reload();
  await expect(page.getByLabel("Service inventory VPS")).toHaveValue(
    /edge-sfo-01/,
  );
  await expect(
    page.getByText("sshd.service", { exact: true }).first(),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
  await settleScreenshot(page);
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("host-services.png"),
  });
});

async function settleScreenshot(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1, 1);
  await page.waitForTimeout(300);
}

test("keeps an unsupported service provider visible and non-mutating", async ({
  page,
}) => {
  const unsupported = hostServiceInventory("agent-sfo-01");
  unsupported.capability = {
    can_enable_disable: false,
    can_inventory: false,
    can_read_logs: false,
    can_start_stop_restart: false,
    enable_backend: null,
    provider: null,
    provider_version: null,
    reason:
      "PID 1 is \"tini\"; no active supported provider was confirmed (systemd, OpenRC, or SysV init)",
    status: "unsupported",
  };
  unsupported.services = [];
  unsupported.source_job_id = null;
  unsupported.observed_at = null;
  await installConsoleApiMock(page, {
    hostServiceInventoryOverride: unsupported,
  });
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Services");
  await page.getByLabel("Service inventory VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();

  const panel = page.locator(".hostServicesPanel");
  await expect(panel.getByText(/^Unsupported:/)).toContainText("PID 1 is \"tini\"");
  await expect(panel.getByText("Not detected", { exact: true })).toBeVisible();
  await expect(panel.getByText("Service provider not checked")).toBeVisible();
  await expect(panel.getByRole("button", { name: /Actions for/ })).toHaveCount(0);
  expect(await serviceMutationRequestCount(page)).toBe(0);
});

async function invokeServiceAction(
  page: Page,
  grid: ReturnType<Page["locator"]>,
  service: string,
  action: string,
  projectName: string,
) {
  if (projectName.includes("mobile")) {
    const card = grid.getByLabel(
      `Host service inventory mobile card ${service}`,
    );
    await activate(card.getByRole("button", { name: action, exact: true }));
    return;
  }
  await grid.getByRole("button", { name: `Actions for ${service}` }).click();
  await activate(page.getByRole("menuitem", { name: action, exact: true }));
}

async function lastServiceActionRequest(page: Page) {
  return page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests
      .filter((request) => request.command === "service_action")
      .at(-1);
  });
}

async function serviceMutationRequestCount(page: Page) {
  return page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.filter((request) => request.command === "service_action")
      .length;
  });
}

async function serviceInventoryRequestCount(page: Page) {
  return page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.filter((request) => request.command === "service_inventory")
      .length;
  });
}
