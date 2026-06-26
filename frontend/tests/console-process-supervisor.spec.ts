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
  const summary = inventory.getByLabel("Process supervisor health summary");
  await expect(inventory.getByText("ospf-worker")).toBeVisible();
  await expect(summary.getByText("1 / 1")).toBeVisible();
  await expect(summary.getByText("Desired-only limits")).toBeVisible();
  await expect(inventory.getByText("Limits desired only; Restarted 1 time; last exit code 7")).toBeVisible();
  await expect(inventory.getByText("2 processes, 2 PIDs")).toBeVisible();
  await expect(inventory.getByText("CPU weight 39; 1.0 MiB memory; cgroup available")).toBeVisible();
  await expect(inventory.getByText("Status snapshot")).toBeVisible();
  await expect(inventory.getByText("Source job 41414141")).toBeVisible();
  await expect(inventory.getByText("stdout + stderr logs")).toBeVisible();
  await expect(inventory.getByText("Request contents through Dispatch > Supervisor > Logs")).toBeVisible();
  await activate(inventory.getByText("ospf-worker"));
  await expect(inventory.getByText("Supervisor config")).toBeVisible();
  await expect(inventory.getByText("Not reported by process supervisor API").first()).toBeVisible();
  await expect(inventory.getByText("Long-term CPU, memory, restart history, and recent exits belong in Observability / Process Metrics when backend time series exist")).toBeVisible();
  await expect(inventory.getByText("Logs, Restart, and Stop prepare reviewed Dispatch / Supervisor jobs with this VPS and process scope")).toBeVisible();
  await expect(inventory.locator(".timeSeriesChartShell")).toHaveCount(0);
  await activate(inventory.getByRole("button", { name: "Open process metrics" }));
  await expect(page.getByRole("heading", { level: 1, name: "Process metrics" })).toBeVisible();
  await expect(page.getByLabel("Process metrics release status")).toContainText("Long-term process history is not exposed by the backend yet.");
});

test("prepares process logs restart and stop as reviewed dispatch actions", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense process action handoff is covered in desktop layout");

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Processes");

  const inventory = page.locator(".fleetPanel", { hasText: "Process supervisor inventory" });
  await expect(inventory.getByRole("button", { name: "Review logs for process ospf-worker" })).toBeVisible();
  await expect(inventory.getByRole("button", { name: "Review restart for process ospf-worker" })).toBeVisible();
  await expect(inventory.getByRole("button", { name: "Review stop for process ospf-worker" })).toBeVisible();

  await activate(inventory.getByRole("button", { name: "Review logs for process ospf-worker" }));
  await expectProcessDispatchPreset(page, "logs");
  await expect(page.locator(".commandComposer").getByLabel("Supervisor log bytes")).toHaveValue("65536");
  await reviewProcessDispatch(page, "Read retained stdout/stderr logs", "Standard");

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  await activate(inventory.getByRole("button", { name: "Review restart for process ospf-worker" }));
  await expectProcessDispatchPreset(page, "restart");
  await reviewProcessDispatch(page, "Restart supervised process", "Privileged mutation");

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  await activate(inventory.getByRole("button", { name: "Review stop for process ospf-worker" }));
  await expectProcessDispatchPreset(page, "stop");
  const composer = page.locator(".commandComposer");
  await reviewProcessDispatch(page, "Stop supervised process", "Privileged mutation");
  await activate(composer.locator(".confirmationPrompt").getByRole("button", { name: "Dispatch job" }));

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, any>> } })
      .__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    command: "process_stop",
    selector_expression: "id:agent-sfo-01",
    operation: {
      name: "ospf-worker",
      type: "process_stop",
    },
  });
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
  await activate(composer.getByRole("button", { name: "Review dispatch" }));
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
