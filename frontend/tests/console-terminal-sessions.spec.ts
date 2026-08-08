import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { terminalSessions } from "./support/jobSessionFixtures";
import {
  lockPrivilegeFromVault,
  openConsoleSubpage,
} from "./support/consoleNavigation";

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
  const switchableTerminalSessions = terminalSessions.map((session, index) =>
    index === 1
      ? {
          ...session,
          close_reason: null,
          last_event: "terminal_input",
          last_status: "accepted",
          state: "open",
        }
      : session,
  );
  await installConsoleApiMock(
    page,
    testInfo.title.includes("explicit stop after switching")
      ? { terminalSessionsOverride: switchableTerminalSessions }
      : testInfo.title.includes("closed replayable terminal")
      ? {
          terminalSessionsOverride: [
            {
              ...terminalSessions[0],
              close_reason: "operator",
              last_status: "closed",
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

async function terminalControlRequests(page: Page) {
  return page.evaluate(() =>
    (
      window as unknown as {
        __vpsmanTestRequests: {
          terminalControls: Array<{
            type: string;
            data_base64?: string;
            cols?: number;
            rows?: number;
            reason?: string;
            request_id: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.terminalControls,
  );
}

async function terminalInputText(page: Page) {
  return page.evaluate(() =>
    (
      window as unknown as {
        __vpsmanTestRequests: {
          terminalControls: Array<{
            type: string;
            data_base64?: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.terminalControls
      .filter((request) => request.type === "input")
      .map((request) => atob(request.data_base64 ?? ""))
      .join(""),
  );
}

test("uses retained session actions and sends exact xterm input without creating jobs", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal keyboard controls are covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  const grid = page.getByLabel("Session inventory and controls data grid");
  await expect(page.getByText("Session inventory and controls")).toBeVisible();
  await expect(page.getByText("Seq 1-3 retained, next 4").first()).toBeVisible();
  await expect(page.getByText("Seq 4 retained").first()).toBeVisible();
  await expect(page.getByText("Active session - accepted").first()).toBeVisible();
  await expect(page.getByText("Closed session - operator").first()).toBeVisible();
  await expect(page.getByText("Idle timeout 10m; 64.0 KiB flow window").first()).toBeVisible();
  const sessionContext = page.getByLabel("Active terminal session context");
  const terminalEmulator = page.getByLabel("Active terminal emulator");
  const [sessionContextBox, terminalEmulatorBox] = await Promise.all([
    sessionContext.boundingBox(),
    terminalEmulator.boundingBox(),
  ]);
  expect(sessionContextBox).not.toBeNull();
  expect(terminalEmulatorBox).not.toBeNull();
  expect(
    terminalEmulatorBox!.y -
      (sessionContextBox!.y + sessionContextBox!.height),
  ).toBeGreaterThanOrEqual(12);
  await expect(page.getByText("1 -> 4")).toHaveCount(0);
  await expect(page.getByText("4 -> 5")).toHaveCount(0);
  await openTerminalActionMenu(page, activeTerminalRow);
  for (const action of ["Stop follow", "Replay", "Attach", "Input", "Close"]) {
    await expect(
      page.getByRole("menuitem", { name: action, exact: true }),
    ).toBeVisible();
  }
  await activate(
    page.getByRole("menuitem", { name: "Stop follow", exact: true }),
  );
  await expect(page.getByText("Not following", { exact: true })).toBeVisible();
  await page.waitForTimeout(250);
  await openTerminalActionMenu(page, activeTerminalRow);
  await expect(
    page.getByRole("menuitem", { name: "Follow", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Stop follow", exact: true }),
  ).toHaveCount(0);
  await activate(page.getByRole("menuitem", { name: "Follow", exact: true }));
  await activate(grid.getByText("Seq 1-3 retained, next 4").first());
  await expect(page.getByText("Opened by")).toBeVisible();
  await expect(page.getByText("Not reported by terminal API").first()).toBeVisible();
  await openTerminalActionMenu(page, closedTerminalRow);
  for (const action of ["Attach", "Input", "Close", "Follow"]) {
    await expect(
      page.getByRole("menuitem", { name: action, exact: true }),
    ).toBeDisabled();
  }
  await expect(
    page.getByRole("menuitem", { name: "Replay", exact: true }),
  ).toBeEnabled();
  const disabledCloseColor = await page
    .getByRole("menuitem", { name: "Close", exact: true })
    .evaluate((element) => getComputedStyle(element).color);
  const disabledInputColor = await page
    .getByRole("menuitem", { name: "Input", exact: true })
    .evaluate((element) => getComputedStyle(element).color);
  await page.keyboard.press("Escape");
  await openTerminalActionMenu(page, activeTerminalRow);
  const activeCloseColor = await page
    .getByRole("menuitem", { name: "Close", exact: true })
    .evaluate((element) => getComputedStyle(element).color);
  expect(disabledCloseColor).toBe(disabledInputColor);
  expect(activeCloseColor).not.toBe(disabledCloseColor);
  await page.keyboard.press("Escape");
  await invokeTerminalAction(page, activeTerminalRow, "Attach");
  const focused = page.getByRole("dialog", {
    name: "Focused terminal workspace",
  });
  const terminalInput = focused.locator(".xterm-helper-textarea");
  await expect(focused).toBeVisible();
  await expect(terminalInput).toBeFocused();
  await expect(
    page.getByLabel("Terminal transcript availability"),
  ).toContainText("Live terminal connected");
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      }
    ).__vpsmanFetchRequests?.splice(0);
  });
  await page.keyboard.type("uptime");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Control+C");
  await page.keyboard.press("Tab");
  await page.keyboard.press("Escape");
  await expect(focused).toBeVisible();

  await expect.poll(() => terminalInputText(page)).toContain(
    "uptime\r\u0003\t\u001b",
  );
  await expect(sessionContext).toContainText(/Last input seq [3-9]/);
  await page.evaluate(() => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget & { url: string }>;
      }
    ).__vpsmanTestWebSockets.find(
      (candidate) => new URL(candidate.url, window.location.href).pathname === "/ws",
    );
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "terminal_output_recorded",
          job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
          client_id: "agent-sfo-01",
          session_id: "61616161-2222-4333-8444-555555555555",
          terminal_seq: null,
          done: false,
        }),
      }),
    );
  });
  await page.waitForTimeout(250);
  const hotPathFetches = await page.evaluate(() =>
    (
      (
        window as typeof window & {
          __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
        }
      ).__vpsmanFetchRequests ?? []
    ).filter(({ url }) => {
      const pathname = new URL(url, window.location.href).pathname;
      return (
        pathname === "/api/v1/terminal-sessions" ||
        /^\/api\/v1\/terminal-sessions\/[^/]+\/[^/]+\/(?:control|replay)$/.test(
          pathname,
        ) ||
        pathname === "/api/v1/jobs" ||
        pathname.startsWith("/api/v1/audit") ||
        pathname === "/api/v1/agents" ||
        pathname.startsWith("/api/v1/fleet")
      );
    }),
  );
  expect(hotPathFetches).toEqual([]);
  const controls = await terminalControlRequests(page);
  expect(JSON.stringify(controls)).not.toContain("local-super-password");
  expect(JSON.stringify(controls)).not.toContain("privilege_assertion");
  expect(JSON.stringify(controls)).not.toContain("input_seq");
  expect(
    controls
      .filter((request) => request.type === "input")
      .every((request) => /^[0-9a-f-]{36}$/.test(request.request_id)),
  ).toBe(true);
  expect(
    await page.evaluate(
      () =>
        (
          window as unknown as {
            __vpsmanTestRequests: { jobs: unknown[] };
          }
        ).__vpsmanTestRequests.jobs.length,
    ),
  ).toBe(0);
});

test("keeps explicit stop after switching followed terminals", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal follow state is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await invokeTerminalAction(page, closedTerminalRow, "Follow");
  await invokeTerminalAction(page, closedTerminalRow, "Stop follow");
  await expect(page.getByText("Not following", { exact: true })).toBeVisible();
  await page.waitForTimeout(250);
  await openTerminalActionMenu(page, closedTerminalRow);
  await expect(
    page.getByRole("menuitem", { name: "Follow", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Stop follow", exact: true }),
  ).toHaveCount(0);
});

test("pipelines terminal input without waiting one RTT per frame", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal input transport is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanTerminalControlAckDelayMs?: number;
      }
    ).__vpsmanTerminalControlAckDelayMs = 300;
  });
  await invokeTerminalAction(page, activeTerminalRow, "Attach");
  const focused = page.getByRole("dialog", {
    name: "Focused terminal workspace",
  });
  await expect(focused.locator(".xterm-helper-textarea")).toBeFocused();
  await page.keyboard.type("a");
  await page.waitForTimeout(40);
  await page.keyboard.type("b");
  await page.waitForTimeout(40);

  const beforeFirstAck = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          terminalControlAcks: Array<{ action: string }>;
          terminalControls: Array<{ type: string }>;
        };
      }
    ).__vpsmanTestRequests;
    return {
      inputAcks: requests.terminalControlAcks.filter(
        (ack) => ack.action === "input",
      ).length,
      inputFrames: requests.terminalControls.filter(
        (request) => request.type === "input",
      ).length,
    };
  });
  expect(beforeFirstAck).toEqual({ inputAcks: 0, inputFrames: 2 });
  await expect(page.getByLabel("Active terminal session context")).toContainText(
    "Last input seq 4",
  );
});

test("keeps a closed replayable terminal out of live-follow state", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal lifecycle controls are covered on desktop");

  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
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
  await panel.getByRole("button", { name: "Copy transcript" }).click();
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("durable replay line 1\nprompt$ € ready\n");
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

test("requires privilege only for terminal open and keeps an authorized session usable", async ({
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

  await lockPrivilegeFromVault(page);
  await expect(
    panel.getByRole("button", { name: "Unlock privilege" }),
  ).toBeVisible();
  await invokeTerminalAction(page, activeTerminalRow, "Input");
  const focused = page.getByRole("dialog", {
    name: "Focused terminal workspace",
  });
  await expect(focused).toBeVisible();
  await focused.locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Control+C");
  await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(
    0,
  );
  await expect.poll(() => terminalInputText(page)).toContain("\u0003");
});

test("presents one exact review and directly closes the authorized session", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal close privilege and review sequencing is covered in the desktop workspace",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  const jobsBefore = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: { jobs: unknown[] };
        }
      ).__vpsmanTestRequests.jobs.length,
  );
  await invokeTerminalAction(page, activeTerminalRow, "Close");
  const prompt = panel.getByLabel("Confirm terminal close");
  await expect(prompt).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(
    0,
  );
  await expect(prompt.locator("dd").nth(0)).toHaveText("edge-sfo-01 (fo01)");
  await expect(prompt.locator("dd").nth(1)).toHaveText(
    "61616161-2222-4333-8444-555555555555",
  );
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanRejectNextTerminalControl?: string;
      }
    ).__vpsmanRejectNextTerminalControl = "close";
  });
  await prompt.getByRole("button", { name: "Close terminal" }).click();
  await expect(prompt).toContainText("terminal_close_rejected_for_test");
  await expect(
    prompt.getByRole("button", { name: "Close terminal" }),
  ).toBeEnabled();
  await prompt.getByRole("button", { name: "Close terminal" }).click();
  await expect(prompt).toBeHidden();
  await expect(panel).toContainText("Terminal 61616161 closed.");

  const controls = await terminalControlRequests(page);
  const request = controls.at(-1);
  expect(request).toMatchObject({
    type: "close",
    reason: "operator_closed",
  });
  expect(
    await page.evaluate(
      () =>
        (
          window as unknown as {
            __vpsmanTestRequests: { jobs: unknown[] };
          }
        ).__vpsmanTestRequests.jobs.length,
    ),
  ).toBe(jobsBefore);
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
  await expect(preview).toContainText("3 chunks");
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

test("deduplicates terminal output and resets split UTF-8 across retention gaps", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal live replay merging is covered in the desktop session workspace",
  );

  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const panel = page.locator(".terminalSessionsPanel");
  const preview = panel.getByLabel("Durable terminal replay status");
  await expect(preview).toContainText("3 chunks, 40 B");

  await page.evaluate(() => {
    const sockets = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget & { url: string }>;
      }
    ).__vpsmanTestWebSockets;
    const socket = sockets.find((candidate) =>
      candidate.url.includes("/ws/terminal/"),
    );
    const firstMessage = JSON.stringify({
      type: "output",
      terminal_seq: 4,
      data_base64: btoa(String.fromCharCode(0xe2)),
    });
    const secondMessage = JSON.stringify({
      type: "output",
      terminal_seq: 5,
      data_base64: btoa(String.fromCharCode(0x82, 0xac)),
    });
    socket?.dispatchEvent(
      new MessageEvent("message", { data: firstMessage }),
    );
    socket?.dispatchEvent(
      new MessageEvent("message", { data: secondMessage }),
    );
    socket?.dispatchEvent(
      new MessageEvent("message", { data: secondMessage }),
    );
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "output",
          terminal_seq: 6,
          data_base64: btoa(String.fromCharCode(0xe2)),
        }),
      }),
    );
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "ready",
          session: { state: "open" },
          from_seq: 7,
          available_first_seq: 8,
          next_seq: 9,
          replay_truncated: true,
        }),
      }),
    );
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          type: "output",
          terminal_seq: 8,
          data_base64: btoa(String.fromCharCode(0x82, 0xac)),
        }),
      }),
    );
  });

  await expect(preview).toContainText("7 chunks, 46 B");
  await expect(preview).toContainText("truncated");
  await expect(page.getByLabel("Active terminal session context")).toContainText(
    "Seq 8 retained",
  );
  await panel.getByRole("button", { name: "Copy transcript" }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(
    "durable replay line 1\nprompt$ € ready\n€��",
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
  const terminalFitHost = terminal.locator(".xtermFitHost");
  const terminalScreen = terminalFitHost.locator(".xterm-screen");
  const expectFittedRowsInsideShell = async () => {
    const geometry = await terminal.evaluate((shell) => {
      const fitHost = shell.querySelector<HTMLElement>(".xtermFitHost");
      const screen = shell.querySelector<HTMLElement>(".xterm-screen");
      if (!fitHost || !screen) {
        return null;
      }
      const shellBox = shell.getBoundingClientRect();
      const hostBox = fitHost.getBoundingClientRect();
      const screenBox = screen.getBoundingClientRect();
      return {
        bottomInset: shellBox.bottom - hostBox.bottom,
        hostBottom: hostBox.bottom,
        screenBottom: screenBox.bottom,
      };
    });
    expect(geometry).not.toBeNull();
    expect(geometry!.screenBottom).toBeLessThanOrEqual(
      geometry!.hostBottom + 0.5,
    );
    expect(geometry!.bottomInset).toBeGreaterThanOrEqual(12);
  };
  await expect(terminalFitHost).toBeVisible();
  await expect(terminalScreen).toBeVisible();
  await expectFittedRowsInsideShell();
  await terminal.evaluate((element) => {
    (element as HTMLElement).style.height = "460px";
  });
  await expect
    .poll(async () =>
      (await terminalControlRequests(page)).some(
        (request) =>
          request.type === "resize" &&
          Number(request.cols) > 0 &&
          Number(request.rows) > 0,
      ),
    )
    .toBe(true);
  await expectFittedRowsInsideShell();

  await openConsoleSubpage(page, "Jobs", "Dispatch");
  const impact = page.locator(".commandComposer .targetImpactPreview");
  await expect(impact.locator(".targetImpactGroup")).toHaveCount(3);
  await expect(impact.getByText("Ready", { exact: true })).toBeVisible();
  await expect(impact.getByText("Needs review", { exact: true })).toBeVisible();
  await expect(impact.getByText("Unavailable", { exact: true })).toBeVisible();
});

test("keeps focused xterm input bound to the active session and restores focus on exit", async ({
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
  const input = focused.locator(".xterm-helper-textarea");
  await expect(focused).toBeVisible();
  await expect(input).toBeFocused();
  await page.keyboard.type("hostname");
  await page.keyboard.press("Enter");
  await expect.poll(() => terminalInputText(page)).toContain("hostname\r");
  await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(
    0,
  );

  const inputBeforeBlur = await terminalInputText(page);
  await focused.locator("header").click({ position: { x: 8, y: 8 } });
  await expect(input).not.toBeFocused();
  await page.keyboard.type("ignored");
  await expect.poll(() => terminalInputText(page)).toBe(inputBeforeBlur);

  await focused
    .getByLabel("Focused terminal emulator")
    .click({ position: { x: 100, y: 100 } });
  await expect(input).toBeFocused();
  await page.keyboard.type("accepted");
  await expect
    .poll(() => terminalInputText(page))
    .toBe(`${inputBeforeBlur}accepted`);

  await focused.getByRole("button", { name: "Exit focused terminal view" }).click();
  await expect(focused).toBeHidden();
  await expect(focusTrigger).toBeFocused();
});
