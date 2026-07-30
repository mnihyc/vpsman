import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { terminalSessions } from "./support/jobSessionFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.beforeEach(async ({ page }, testInfo) => {
  const retainedRangeSessions = terminalSessions.map((session, index) =>
    index === 0
      ? {
          ...session,
          output_first_seq: 3,
          output_retained_first_seq: 1,
        }
      : session,
  );
  await installConsoleApiMock(
    page,
    testInfo.title.includes("closed replayable terminal")
      ? {
          terminalSessionsOverride: [
            {
              ...terminalSessions[0],
              close_reason: "operator",
              last_status: "closed",
              session_exited: true,
              state: "closed",
            },
          ],
        }
      : testInfo.title.includes("full retained terminal range")
        ? { terminalSessionsOverride: retainedRangeSessions }
        : {},
  );
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

async function selectNewTerminalTarget(page: Page) {
  const target = page.getByRole("combobox", { name: "New terminal target" });
  await target.fill("edge-sfo-01");
  await page
    .getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ })
    .click();
  await expect(target).toHaveValue("edge-sfo-01 (fo01)");
}

const activeTerminalRow =
  "agent-sfo-01:61616161-2222-4333-8444-555555555555";
const closedTerminalRow =
  "agent-fra-02:71717171-2222-4333-8444-555555555555";

async function openTerminalActionMenu(page: Page, rowId: string) {
  const grid = page.getByLabel("Session inventory and controls data grid");
  const checkedRows = grid.locator(
    'input[aria-label^="Select Session inventory and controls row "]:checked',
  );
  while ((await checkedRows.count()) > 0) {
    await checkedRows.first().uncheck();
  }
  await grid
    .getByLabel(`Select Session inventory and controls row ${rowId}`)
    .check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
}

async function invokeTerminalAction(
  page: Page,
  rowId: string,
  action: string,
) {
  await openTerminalActionMenu(page, rowId);
  await activate(page.getByRole("menuitem", { name: action, exact: true }));
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
  await openTerminalActionMenu(page, activeTerminalRow);
  for (const action of ["Stop follow", "Replay", "Attach", "Close"]) {
    await expect(
      page.getByRole("menuitem", { name: action, exact: true }),
    ).toBeVisible();
  }
  await page.keyboard.press("Escape");
  await activate(grid.getByText("Seq 1-3 retained, next 4").first());
  await expect(page.getByText("Opened by")).toBeVisible();
  await expect(page.getByText("Not reported by terminal API").first()).toBeVisible();
  await openTerminalActionMenu(page, closedTerminalRow);
  for (const action of ["Poll", "Input", "Close", "Follow"]) {
    await expect(
      page.getByRole("menuitem", { name: action, exact: true }),
    ).toBeDisabled();
  }
  const disabledCloseColor = await page
    .getByRole("menuitem", { name: "Close", exact: true })
    .evaluate((element) => getComputedStyle(element).color);
  const disabledPollColor = await page
    .getByRole("menuitem", { name: "Poll", exact: true })
    .evaluate((element) => getComputedStyle(element).color);
  await page.keyboard.press("Escape");
  await openTerminalActionMenu(page, activeTerminalRow);
  const activeCloseColor = await page
    .getByRole("menuitem", { name: "Close", exact: true })
    .evaluate((element) => getComputedStyle(element).color);
  expect(disabledCloseColor).toBe(disabledPollColor);
  expect(activeCloseColor).not.toBe(disabledCloseColor);
  await page.keyboard.press("Escape");
  await unlockTerminalPrivilege(page);

  const composer = page.locator(".commandComposer");
  await invokeTerminalAction(page, activeTerminalRow, "Attach");
  await expect(composer.getByLabel("Terminal action")).toHaveValue("open");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal argv")).toHaveValue("/bin/sh -l");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("Bulk target selector expression")).toHaveValue("id:agent-sfo-01");

  await invokeTerminalAction(page, activeTerminalRow, "Poll");
  await expect(composer.getByLabel("Terminal action")).toHaveValue("poll");
  await expect(composer.getByLabel("Terminal session id")).toHaveValue("61616161-2222-4333-8444-555555555555");
  await expect(composer.getByLabel("Terminal replay from sequence")).toHaveValue("1");
  await expect(composer.getByLabel("Bulk target selector expression")).toHaveValue("id:agent-sfo-01");

  await invokeTerminalAction(page, activeTerminalRow, "Input");
  const terminalInput = page.getByLabel("Terminal input composer");
  const inputBytes = terminalInput.getByLabel("Terminal input bytes");
  await expect(inputBytes).toBeFocused();
  await expect(terminalInput).toContainText("Input to edge-sfo-01");
  await expect(
    terminalInput.getByRole("checkbox", { name: "Press Enter after input" }),
  ).toBeChecked();
  await expect(composer.getByLabel("Terminal action")).toHaveValue("poll");

  await inputBytes.fill("uptime");
  await terminalInput.getByRole("button", { name: "Send input" }).click();
  await expect(terminalInput).toContainText("Input 3 queued.");

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
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
});

test("keeps a closed replayable terminal out of live-follow state", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal lifecycle controls are covered on desktop");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  await expect(panel.getByText("Not following", { exact: true })).toBeVisible();
  await openTerminalActionMenu(page, activeTerminalRow);
  await expect(
    page.getByRole("menuitem", { name: "Follow", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await panel.locator(".terminalActiveHeader").getByRole("button", { name: "Replay" }).click();
  await expect(panel.getByLabel("Durable terminal replay status")).toContainText("retained replay");
  await expect(panel.getByLabel("Durable terminal replay status")).not.toContainText("following live output");
});

test("labels the full retained terminal range instead of only the latest output event", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal replay range semantics are covered in the desktop session inventory",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  await expect(panel.getByText("Seq 1-3 retained, next 4").first()).toBeVisible();
  await expect(panel.getByText("Seq 3 retained")).toHaveCount(0);
});

test("clears stale terminal input when the operator changes or opens a session", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal session input binding is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await unlockTerminalPrivilege(page);

  const panel = page.locator(".terminalSessionsPanel");
  const input = panel.getByLabel("Terminal input bytes");
  const inventory = panel.getByLabel("Session inventory and controls data grid");

  await input.fill("stale input for the previous session");
  await inventory.getByRole("button", { name: /core-fra-02/ }).click();
  await expect(input).toHaveValue("");

  await selectNewTerminalTarget(page);
  await input.fill("another stale draft");
  await panel.getByRole("button", { name: "Open terminal" }).click();
  await expect(panel).toContainText("terminal open job submitted");
  await expect(input).toHaveValue("");
});

test("reconciles terminal launcher and input feedback after privilege unlock", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal launcher privilege feedback is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  await selectNewTerminalTarget(page);
  await panel.getByRole("button", { name: "Unlock privilege" }).click();
  await expect(panel).toContainText("Unlock privilege, then open the terminal from this launcher.");

  const dialog = page.getByRole("dialog", { name: "Unlock privilege" });
  await dialog.getByLabel(/super password/i).fill("local-super-password");
  await dialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    dialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: /Unlock( privilege)?/ }),
  );

  await expect(dialog).toBeHidden();
  await expect(panel).toContainText("Privilege unlocked. Terminal controls are ready.");
  await expect(panel).not.toContainText(
    "Unlock privilege, then open the terminal from this launcher.",
  );

  await page.locator(".topbar").getByRole("button", { name: "Lock privilege" }).click();
  const input = panel.getByLabel("Terminal input bytes");
  await input.fill("preserved terminal input");
  await panel.getByRole("button", { name: "Send input" }).click();
  await expect(panel).toContainText("Unlock local privilege, then send this preserved input.");

  await dialog.getByLabel(/super password/i).fill("local-super-password");
  await dialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    dialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: /Unlock( privilege)?/ }),
  );

  await expect(dialog).toBeHidden();
  await expect(input).toHaveValue("preserved terminal input");
  await expect(panel).toContainText("Privilege unlocked. Preserved input is ready to send.");
  await expect(panel).not.toContainText(
    "Unlock local privilege, then send this preserved input.",
  );
});

test("unlocks before presenting one exact terminal close review", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal close privilege and review sequencing is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  await invokeTerminalAction(page, activeTerminalRow, "Close");
  const dialog = page.getByRole("dialog", { name: "Unlock privilege" });
  await expect(dialog).toBeVisible();
  await expect(panel.getByLabel("Confirm terminal close")).toHaveCount(0);

  await dialog.getByLabel(/super password/i).fill("local-super-password");
  await dialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    dialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: /Unlock( privilege)?/ }),
  );

  const prompt = panel.getByLabel("Confirm terminal close");
  await expect(dialog).toBeHidden();
  await expect(prompt).toBeVisible();
  await expect(prompt.locator("dd").nth(0)).toHaveText("edge-sfo-01 (fo01)");
  await expect(prompt.locator("dd").nth(1)).toHaveText(
    "61616161-2222-4333-8444-555555555555",
  );
  await prompt.getByRole("button", { name: "Close terminal" }).click();
  await expect(prompt).toBeHidden();
  await expect(panel).toContainText("Terminal 61616161 close job submitted.");

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(request).toMatchObject({
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    operation: {
      type: "terminal_close",
      session_id: "61616161-2222-4333-8444-555555555555",
      reason: "operator_closed",
    },
  });
});

test("dispatches terminal poll from retained session inventory", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal reconnect actions are covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await unlockTerminalPrivilege(page);

  const composer = page.locator(".commandComposer");
  await invokeTerminalAction(page, activeTerminalRow, "Poll");
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

  const preview = terminalPanel.getByLabel("Durable terminal replay status");
  await expect(preview).toContainText("Durable replay 61616161");
  await expect(preview).toContainText("2 chunks");
  await expect(preview).toContainText("Seq 1-3 retained, next 4");
  await expect(preview).not.toContainText("durable replay line 1");
  await expect(preview).not.toContainText("prompt$");
  await expect(preview.locator("pre")).toHaveCount(0);

  await activate(terminalPanel.getByRole("button", { name: "Copy transcript" }));
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("durable replay line 1");

  const downloadEvent = page.waitForEvent("download");
  await activate(terminalPanel.getByRole("button", { name: "Download transcript" }));
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe("terminal-61616161-replay.txt");
});

test("deduplicates overlapping live terminal replay refreshes", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal live replay merging is covered in the desktop session workspace",
  );

  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  const preview = panel.getByLabel("Durable terminal replay status");
  await expect(preview).toContainText("2 chunks, 30 B");

  await page.evaluate(() => {
    const sockets = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets;
    const socket = sockets.at(-1);
    const message = JSON.stringify({
      type: "terminal_output_recorded",
      job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
      client_id: "agent-sfo-01",
      session_id: "61616161-2222-4333-8444-555555555555",
      terminal_seq: 3,
      done: false,
    });
    socket?.dispatchEvent(new MessageEvent("message", { data: message }));
    socket?.dispatchEvent(new MessageEvent("message", { data: message }));
  });

  await expect(preview).toContainText("2 chunks, 30 B");
  await panel.getByRole("button", { name: "Copy transcript" }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(
    "durable replay line 1\nprompt$ ",
  );
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

  await openConsoleSubpage(page, "Jobs", "Dispatch");
  const impact = page.locator(".commandComposer .targetImpactPreview");
  await expect(impact.locator(".targetImpactGroup")).toHaveCount(3);
  await expect(impact.getByText("Ready", { exact: true })).toBeVisible();
  await expect(impact.getByText("Needs review", { exact: true })).toBeVisible();
  await expect(impact.getByText("Unavailable", { exact: true })).toBeVisible();
});

test("keeps focused terminal input modal, unlockable, and bound to the active session", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "focused terminal keyboard behavior is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  const focusTrigger = page.getByRole("button", { name: "Focus terminal" });
  await focusTrigger.click();

  const focused = page.getByRole("dialog", {
    name: "Focused terminal workspace",
  });
  const focusedSurface = page.locator(".terminalFocusOverlay");
  const input = focused.getByLabel("Terminal input bytes");
  await expect(focused).toBeVisible();
  await expect(input).toBeFocused();
  await expect(page.getByLabel("Terminal input composer")).toHaveCount(1);
  await input.fill("hostname");
  await focused.getByRole("button", { name: "Send input" }).click();

  const unlock = page.getByRole("dialog", { name: "Unlock privilege" });
  await expect(unlock).toBeVisible();
  await expect(focusedSurface).toHaveAttribute("inert", "");
  await unlock.getByLabel(/super password/i).fill("local-super-password");
  await unlock
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await unlock
    .getByLabel("Unlock with privilege material")
    .getByRole("button", { name: "Unlock", exact: true })
    .click();

  await expect(unlock).toBeHidden();
  await expect(focusedSurface).not.toHaveAttribute("inert", "");
  await expect(input).toHaveValue("hostname");
  await focused.getByRole("button", { name: "Send input" }).click();
  await expect(focused).toContainText("Input 3 queued.");

  await focused.getByRole("button", { name: "Exit focused terminal view" }).click();
  await expect(focused).toBeHidden();
  await expect(focusTrigger).toBeFocused();
});
