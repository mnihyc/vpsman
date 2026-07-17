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
  const startProcess = inventory.getByRole("button", { name: "Start process", exact: true });
  await expect(startProcess).toBeDisabled();
  await inventory.getByLabel("Process target VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();
  await expect(startProcess).toBeEnabled();
  await expect(startProcess).toHaveCSS("color", "rgb(255, 255, 255)");
  await expect(startProcess.locator("span")).toHaveCSS("color", "rgb(255, 255, 255)");
  await expect(grid.getByText("ospf-worker")).toBeVisible();
  await expect(summary.getByText("1 / 1")).toBeVisible();
  await expect(summary.getByText("Desired-only limits")).toBeVisible();
  await expect(summary.getByText("With automatic restarts")).toBeVisible();
  await expect(inventory.getByText("1 process with automatic restarts")).toBeVisible();
  await expect(summary.getByText("1 warning")).toBeVisible();
  await expect(grid.getByText("Timestamp inconsistent")).toBeVisible();
  await expect(grid.getByText("Unknown", { exact: true })).toBeVisible();
  await expect(grid.getByText("started after observed").first()).toBeVisible();
  await expect(grid.getByText("Weight · Limits desired")).toBeVisible();
  await expect(grid.getByText("1.0 MiB")).toBeVisible();
  await expect(grid.getByText("2 processes, 2 PIDs")).toBeVisible();
  await expect(grid.getByText("1 restart", { exact: true })).toBeVisible();
  await expect(grid.getByText("Time unknown; after observed").first()).toBeVisible();
  await expect(grid.getByText("Code 7")).toBeVisible();
  await expect(grid.getByText("Time unknown; after observed").last()).toBeVisible();
  await activate(grid.getByText("ospf-worker"));
  await expect(grid.getByText("CPU weight 39; 1.0 MiB memory; cgroup available")).toBeVisible();
  await expect(grid.getByText("Supervisor config")).toHaveCount(0);
  await expect(grid.getByText(/backend process time series/i)).toHaveCount(0);
  await expect(grid.getByText(/Logs open Dispatch/i)).toHaveCount(0);
  await expect(grid.getByText("Raw source job ID")).toBeVisible();
  await expect(grid.getByText("41414141-2222-4333-8444-555555555555")).toBeVisible();
  await expect(grid.locator(".timeSeriesChartShell")).toHaveCount(0);
  await expect(inventory.getByRole("button", { name: "Open process metrics" })).toHaveCount(0);
  const actionButtons = inventory.getByLabel("Process ospf-worker actions").getByRole("button");
  expect(
    await actionButtons.evaluateAll((buttons) => {
      const tops = buttons.map((button) => button.getBoundingClientRect().top);
      return Math.max(...tops) - Math.min(...tops);
    }),
  ).toBeLessThan(2);
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
  await activate(page.getByRole("button", { name: "Close detail panel" }));

  const beforeRestart = await processJobRequestCount(page);
  await activate(inventory.getByRole("button", { name: "Restart process ospf-worker" }));
  const restartPrompt = inventory.locator(".confirmationPrompt", {
    hasText: "Confirm process restart",
  });
  await expect(restartPrompt).toContainText("ospf-worker");
  await expect(restartPrompt).toContainText("edge-sfo-01");
  await expect(
    restartPrompt.getByText("edge-sfo-01 (fo01)", { exact: true }),
  ).toBeVisible();
  await expect(restartPrompt).toContainText("Submit one privileged process_restart job");
  await activate(restartPrompt.getByRole("button", { name: "Restart process" }));
  await expect.poll(() => processJobRequestCount(page)).toBe(beforeRestart + 1);
  await expect(restartPrompt).toHaveCount(0);
  await expect(inventory.getByText("Restarted ospf-worker on edge-sfo-01 (fo01).")).toBeVisible();
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

  const beforeStop = await processJobRequestCount(page);
  await activate(inventory.getByRole("button", { name: "Stop process ospf-worker" }));
  const prompt = inventory.locator(".confirmationPrompt");
  await expect(prompt.getByText("Confirm process stop")).toBeVisible();
  await expect(prompt).toContainText("ospf-worker");
  await expect(prompt).toContainText("edge-sfo-01");
  await expect(prompt).toContainText("Submit one privileged process_stop job");
  await activate(prompt.getByRole("button", { name: "Stop process" }));
  await expect.poll(() => processJobRequestCount(page)).toBe(beforeStop + 1);
  await expect(prompt).toHaveCount(0);
  await expect(inventory.getByText("Stopped ospf-worker on edge-sfo-01 (fo01).")).toBeVisible();
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

test("refreshes process observations on every scoped VPS and reports partial failure", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "desktop covers fleet process status refresh");

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const inventory = page.locator(".fleetPanel", { hasText: "Process supervisor inventory" });
  const beforeRefresh = await processJobRequestCount(page);
  await activate(inventory.getByRole("button", { name: "Refresh status", exact: true }));

  await expect.poll(() => processJobRequestCount(page)).toBe(beforeRefresh + 1);
  const refreshRequest = await lastProcessJobRequest(page);
  expect(refreshRequest).toMatchObject({
    command: "process_status",
    confirmed: false,
    destructive: false,
    privileged: true,
    selector_expression:
      "id:agent-fra-02 || id:agent-nyc-03 || id:agent-sfo-01",
    target_client_ids: ["agent-fra-02", "agent-nyc-03", "agent-sfo-01"],
    operation: {
      name: null,
      type: "process_status",
    },
  });
  await expect(inventory.getByText("Status refreshed from 2/3 VPS; 1 VPS did not complete successfully.")).toBeVisible();
});

test("reviews the exact process start command without exposing environment values", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "desktop covers process start review detail");

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const inventory = page.locator(".fleetPanel", { hasText: "Process supervisor inventory" });
  await expect(
    inventory.getByRole("button", { name: "Start process", exact: true }),
  ).toBeDisabled();
  await inventory.getByLabel("Process target VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();
  await activate(inventory.getByRole("button", { name: "Start process", exact: true }));
  const composer = page.locator(".consoleDetailPanel", { hasText: "Process operation" });
  await expect(
    composer.getByLabel("Bulk target selector expression"),
  ).toHaveValue("id:agent-sfo-01");
  await composer.getByLabel("Supervisor process name").fill("report-worker");
  await composer
    .getByLabel("Supervisor command argv")
    .fill("/usr/bin/env sh -c 'printf ready'");
  await composer.getByLabel("Supervisor cwd").fill("/srv/reporting");
  await composer
    .getByLabel("Supervisor environment")
    .fill("REGION=eu-west\nREPORT_TOKEN=must-not-render");
  await activate(composer.getByRole("button", { name: "Dispatch", exact: true }));

  const prompt = composer.getByLabel("Confirm job dispatch");
  await expect(prompt).toContainText("report-worker");
  await expect(prompt).toContainText("/usr/bin/env sh -c 'printf ready'");
  await expect(prompt).toContainText("/srv/reporting");
  await expect(prompt).toContainText("REGION, REPORT_TOKEN (values hidden)");
  await expect(prompt).not.toContainText("must-not-render");
});

test("renders process operation cards on mobile with resource usage and actions", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.includes("mobile"), "mobile process card layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const cards = page.getByLabel("Process supervisor mobile cards");
  await expect(cards).toBeVisible();
  await expect(cards.getByText("ospf-worker")).toBeVisible();
  await expect(cards.getByText("Timestamp inconsistent")).toBeVisible();
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
  await expect(page.getByRole("heading", { level: 1, name: "Processes" })).toBeVisible();
  const detail = page.locator(".consoleDetailPanel", { hasText: "Process operation" });
  await expect(detail).toBeVisible();
  await expect(detail.getByLabel("Dispatch mode boundary")).toContainText(
    "Process operation mode",
  );
  const composer = detail.locator(".commandComposer");
  await expect(composer.getByRole("heading", { name: "Dispatch command" })).toBeVisible();
  await expect(composer.getByLabel("Supervisor action")).toHaveValue(action);
  await expect(composer.getByLabel("Supervisor process name")).toHaveValue("ospf-worker");
  await expect(composer.getByLabel("Bulk target selector expression")).toHaveValue("id:agent-sfo-01");
}

async function reviewProcessDispatch(page: Page, effect: string, execution: string) {
  const composer = page.locator(".commandComposer");
  await activate(composer.getByRole("button", { name: "Dispatch", exact: true }));
  const prompt = composer.locator(".confirmationPrompt");
  await expect(prompt.getByText("Confirm job dispatch")).toBeVisible();
  await expect(prompt.getByText("Managed process", { exact: true })).toBeVisible();
  await expect(prompt).toContainText("Process");
  await expect(prompt).toContainText("ospf-worker");
  await expect(prompt).toContainText("Effect");
  await expect(prompt).toContainText(effect);
  await expect(prompt).toContainText("Selector");
  await expect(prompt).toContainText("id:agent-sfo-01");
  await expect(prompt).toContainText(execution);
  await activate(prompt.getByRole("button", { name: "Cancel" }));
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
