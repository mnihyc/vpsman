import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function dispatchWithPrompt(composer: Locator) {
  await activate(composer.getByRole("button", { name: "Dispatch", exact: true }));
  await expect(composer.getByText("Confirm job dispatch")).toBeVisible();
  await activate(composer.locator(".confirmationPrompt").getByRole("button", { name: "Dispatch job" }));
}

async function unlockTerminalPrivilege(page: Page) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
}

test("prepares terminal reconnect actions from retained session inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal reconnect actions are covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  const grid = page.getByLabel("Session inventory and controls data grid");
  await expect(page.getByText("Session inventory and controls")).toBeVisible();
  await expect(page.getByText("Seq 1-3 retained, next 4").first()).toBeVisible();
  await expect(page.getByText("Seq 4 retained").first()).toBeVisible();
  await expect(page.getByText("Active session - accepted").first()).toBeVisible();
  await expect(page.getByText("Closed session - operator").first()).toBeVisible();
  await expect(page.getByText("Idle timeout 10m; 64.0 KiB flow window").first()).toBeVisible();
  await expect(page.getByText("1 -> 4")).toHaveCount(0);
  await expect(page.getByText("4 -> 5")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Stop following terminal session 61616161" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Durable replay terminal session 61616161" })).toContainText("Replay");
  await expect(page.getByRole("button", { name: "Attach terminal session 61616161" })).toContainText("Attach");
  await expect(page.getByRole("button", { name: "Close terminal session 61616161" })).toContainText("Close");
  await activate(grid.getByText("Seq 1-3 retained, next 4").first());
  await expect(page.getByText("Opened by")).toBeVisible();
  await expect(page.getByText("Not reported by terminal API").first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Poll terminal session 71717171" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Input terminal session 71717171" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Close terminal session 71717171" })).toBeDisabled();
  await unlockTerminalPrivilege(page);

  const composer = page.locator(".commandComposer");
  await activate(page.getByRole("button", { name: "Attach terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("open");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal argv")).toHaveValue("/bin/sh -l");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");

  await activate(page.getByRole("button", { name: "Poll terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("poll");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");

  await activate(page.getByRole("button", { name: "Input terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("input");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal input sequence")).toHaveCount(0);
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");

  await composer.getByRole("textbox", { name: "Terminal input" }).fill("uptime\n");
  await dispatchWithPrompt(composer);

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { terminalInputs: Array<Record<string, unknown>> } })
      .__vpsmanTestRequests.terminalInputs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    text: "uptime\n",
    confirmed: true,
    max_timeout_secs: 30,
  });
  expect(JSON.stringify(request)).not.toContain("input_seq");
});

test("dispatches terminal poll from retained session inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal reconnect actions are covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await unlockTerminalPrivilege(page);

  const composer = page.locator(".commandComposer");
  await activate(page.getByRole("button", { name: "Poll terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("poll");
  await dispatchWithPrompt(composer);

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> } })
      .__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    selector_expression: "id:agent-sfo-01",
    command: "terminal_poll",
    operation: {
      replay_from_seq: 1,
      session_id: "61616161-2222-4333-8444-555555555555",
      type: "terminal_poll",
    },
    privileged: true,
  });
});

test("loads durable terminal replay from persisted output history", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal replay preview is covered in the desktop session table");

  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const terminalPanel = page.locator(".terminalSessionsPanel");
  await activate(terminalPanel.locator(".terminalActiveHeader").getByRole("button", { name: "Replay" }));

  const preview = terminalPanel.getByLabel("Durable terminal replay preview");
  await expect(preview).toContainText("Durable replay 61616161");
  await expect(preview).toContainText("2 chunks");
  await expect(preview).toContainText("Seq 1-3 retained, next 4");
  await expect(preview).toContainText("durable replay line 1");
  await expect(preview).toContainText("prompt$");

  await activate(terminalPanel.getByRole("button", { name: "Copy transcript" }));
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("durable replay line 1");

  const downloadEvent = page.waitForEvent("download");
  await activate(terminalPanel.getByRole("button", { name: "Download transcript" }));
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe("terminal-61616161-replay.txt");
});

test("keeps terminal emulator resizable and target impact compact", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop terminal emulator sizing is covered in the desktop layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const terminal = page.getByLabel("Active terminal emulator");
  await expect(terminal).toBeVisible();
  await expect(
    terminal.evaluate((element) => getComputedStyle(element).resize),
  ).resolves.toBe("vertical");
  await expect(
    terminal.evaluate((element) => getComputedStyle(element).overflow),
  ).resolves.toBe("hidden");

  const impact = page.locator(".commandComposer .targetImpactPreview");
  await expect(impact.locator(".targetImpactGroup")).toHaveCount(3);
  await expect(impact.getByText("Ready", { exact: true })).toBeVisible();
  await expect(impact.getByText("Needs review", { exact: true })).toBeVisible();
  await expect(impact.getByText("Unavailable", { exact: true })).toBeVisible();
});
