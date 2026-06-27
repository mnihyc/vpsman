import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

test("shows restart and desired-only limit evidence in process supervisor inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense process inventory evidence is covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const inventory = page.locator(".fleetPanel", { hasText: "Process supervisor inventory" });
  const grid = inventory.getByLabel("Process health inventory data");
  const summary = inventory.getByLabel("Process supervisor health summary");
  await expect(grid.getByText("ospf-worker")).toBeVisible();
  await expect(summary.getByText("1 / 1")).toBeVisible();
  await expect(summary.getByText("Desired-only limits")).toBeVisible();
  await expect(summary.getByText("1 warning")).toBeVisible();
  await expect(grid.getByText("Timeline inconsistent")).toBeVisible();
  await expect(grid.getByText("Unknown", { exact: true })).toBeVisible();
  await expect(grid.getByText("started after observed").first()).toBeVisible();
  await expect(grid.getByText("CPU weight; Limits desired only")).toBeVisible();
  await expect(grid.getByText("1.0 MiB")).toBeVisible();
  await expect(grid.getByText("2 processes, 2 PIDs")).toBeVisible();
  await expect(grid.getByText("1 restart")).toBeVisible();
  await expect(grid.getByText("Time unknown; after observed").first()).toBeVisible();
  await expect(grid.getByText("Code 7")).toBeVisible();
  await expect(grid.getByText("Time unknown; after observed").last()).toBeVisible();
  await activate(grid.getByText("ospf-worker"));
  await expect(grid.getByText("CPU weight 39; 1.0 MiB memory; cgroup available")).toBeVisible();
  await expect(grid.getByText("Supervisor config")).toBeVisible();
  await expect(grid.getByText("Not reported by process supervisor API").first()).toBeVisible();
  await expect(grid.getByText("Not available yet; backend process time series for CPU, memory, restart history, and recent exits are not exposed.")).toBeVisible();
  await expect(grid.getByText("Logs open Dispatch for retained output. Restart submits directly after privilege unlock. Stop uses one confirmation on this page.")).toBeVisible();
  await expect(grid.getByText("Raw source job ID")).toBeVisible();
  await expect(grid.getByText("41414141-2222-4333-8444-555555555555")).toBeVisible();
  await expect(grid.locator(".timeSeriesChartShell")).toHaveCount(0);
  await expect(inventory.getByRole("button", { name: "Open process metrics" })).toHaveCount(0);
});

test("runs restart directly and confirms stop from process inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense process action handling is covered in desktop layout");

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const inventory = page.locator(".fleetPanel", { hasText: "Process supervisor inventory" });
  await expect(inventory.getByRole("button", { name: "Open logs for process ospf-worker" })).toBeVisible();
  await expect(inventory.getByRole("button", { name: "Restart process ospf-worker" })).toBeVisible();
  await expect(inventory.getByRole("button", { name: "Stop process ospf-worker" })).toBeVisible();

  await activate(inventory.getByRole("button", { name: "Open logs for process ospf-worker" }));
  await expectProcessDispatchPreset(page, "logs");
  await expect(page.locator(".commandComposer").getByLabel("Supervisor log bytes")).toHaveValue("65536");
  await reviewProcessDispatch(page, "Read retained stdout/stderr logs", "Standard");

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  const beforeRestart = await processJobRequestCount(page);
  await activate(inventory.getByRole("button", { name: "Restart process ospf-worker" }));
  await expect(page.getByRole("heading", { name: "Process supervisor inventory" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Command dispatch" })).toHaveCount(0);
  await expect.poll(() => processJobRequestCount(page)).toBe(beforeRestart + 1);
  const restartRequest = await lastProcessJobRequest(page);
  expect(JSON.stringify(restartRequest)).not.toContain("local-super-password");
  expect(restartRequest).toMatchObject({
    command: "process_restart",
    confirmed: true,
    destructive: true,
    privileged: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    operation: {
      name: "ospf-worker",
      type: "process_restart",
    },
  });

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  const beforeStop = await processJobRequestCount(page);
  await activate(inventory.getByRole("button", { name: "Stop process ospf-worker" }));
  const prompt = inventory.locator(".confirmationPrompt");
  await expect(prompt.getByText("Confirm process stop")).toBeVisible();
  await expect(prompt).toContainText("ospf-worker");
  await expect(prompt).toContainText("edge-sfo-01");
  await expect(prompt).toContainText("Submit one privileged process_stop job");
  await activate(prompt.getByRole("button", { name: "Stop process" }));
  await expect.poll(() => processJobRequestCount(page)).toBe(beforeStop + 1);
  const stopRequest = await lastProcessJobRequest(page);
  expect(JSON.stringify(stopRequest)).not.toContain("local-super-password");
  expect(stopRequest).toMatchObject({
    command: "process_stop",
    confirmed: true,
    destructive: true,
    privileged: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    operation: {
      name: "ospf-worker",
      type: "process_stop",
    },
  });
});

test("renders process operation cards on mobile with resource usage and actions", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.includes("mobile"), "mobile process card layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const cards = page.getByLabel("Process supervisor mobile cards");
  await expect(cards).toBeVisible();
  await expect(cards.getByText("ospf-worker")).toBeVisible();
  await expect(cards.getByText("Timeline inconsistent")).toBeVisible();
  await expect(cards.getByText("39")).toBeVisible();
  await expect(cards.getByText("1.0 MiB")).toBeVisible();
  await expect(cards.getByText("Unknown")).toBeVisible();
  await expect(cards.getByText("1 restart")).toBeVisible();
  await expect(cards.getByRole("button", { name: "Open logs for process ospf-worker" })).toBeVisible();
  await expect(cards.getByRole("button", { name: "Restart process ospf-worker" })).toBeVisible();
  await expect(cards.getByRole("button", { name: "Stop process ospf-worker" })).toBeVisible();
  await expect(page.locator(".processInventoryGridShell")).toBeHidden();
});

async function expectProcessDispatchPreset(page: Page, action: string) {
  await expect(page.getByRole("heading", { name: "Command dispatch" })).toBeVisible();
  const composer = page.locator(".commandComposer");
  await expect(composer.getByRole("heading", { name: "Dispatch command" })).toBeVisible();
  await expect(composer.getByLabel("Supervisor action")).toHaveValue(action);
  await expect(composer.getByLabel("Supervisor process name")).toHaveValue("ospf-worker");
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");
}

async function reviewProcessDispatch(page: Page, effect: string, execution: string) {
  const composer = page.locator(".commandComposer");
  await activate(composer.getByRole("button", { name: "Dispatch", exact: true }));
  const prompt = composer.locator(".confirmationPrompt");
  await expect(prompt.getByText("Confirm job dispatch")).toBeVisible();
  await expect(prompt).toContainText("Process");
  await expect(prompt).toContainText("ospf-worker");
  await expect(prompt).toContainText("Effect");
  await expect(prompt).toContainText(effect);
  await expect(prompt).toContainText("Selector");
  await expect(prompt).toContainText("id:agent-sfo-01");
  await expect(prompt).toContainText(execution);
}

async function processJobRequestCount(page: Page) {
  return page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, any>> } })
      .__vpsmanTestRequests.jobs;
    return requests.filter((request) => String(request.command).startsWith("process_")).length;
  });
}

async function lastProcessJobRequest(page: Page) {
  return page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, any>> } })
      .__vpsmanTestRequests.jobs;
    return requests
      .filter((request) => String(request.command).startsWith("process_"))
      .at(-1);
  });
}
