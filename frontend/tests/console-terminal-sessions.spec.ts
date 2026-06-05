import { expect, test, type Locator } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

test("prepares terminal reconnect actions from retained session inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal reconnect actions are covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Terminal sessions");
  await expect(page.getByRole("button", { name: "Poll terminal session 71717171" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Input terminal session 71717171" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Close terminal session 71717171" })).toBeDisabled();

  const composer = page.locator(".commandComposer");
  await activate(page.getByRole("button", { name: "Attach terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("open");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal argv")).toHaveValue("/bin/sh -l");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("edge-sfo-01")).toBeChecked();

  await activate(page.getByRole("button", { name: "Poll terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("poll");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("edge-sfo-01")).toBeChecked();

  await activate(page.getByRole("button", { name: "Input terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("input");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal input sequence")).toHaveValue("3");
  await expect(composer.getByLabel("edge-sfo-01")).toBeChecked();

  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await composer.getByRole("textbox", { name: "Terminal input" }).fill("uptime\n");
  await activate(composer.getByRole("button", { name: "Dispatch" }));

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> } })
      .__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    clients: ["agent-sfo-01"],
    command: "terminal_session",
    operation: {
      input_seq: 3,
      session_id: "61616161-2222-4333-8444-555555555555",
      type: "terminal_input",
    },
    privileged: true,
  });
});

test("dispatches terminal poll from retained session inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal reconnect actions are covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Terminal sessions");

  const composer = page.locator(".commandComposer");
  await activate(page.getByRole("button", { name: "Poll terminal session 61616161" }));
  await expect(composer.getByLabel("Terminal action")).toHaveValue("poll");
  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await activate(composer.getByRole("button", { name: "Dispatch" }));

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> } })
      .__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    clients: ["agent-sfo-01"],
    command: "terminal_session",
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

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Terminal sessions");

  await activate(page.getByRole("button", { name: "Durable replay terminal session 61616161" }));

  const preview = page.getByLabel("Durable terminal replay preview");
  await expect(preview).toContainText("Durable replay 61616161");
  await expect(preview).toContainText("2 chunks");
  await expect(preview).toContainText("durable replay line 1");
  await expect(preview).toContainText("prompt$");
});
