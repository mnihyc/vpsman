import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
  waitForConsoleShell,
} from "./support/consoleNavigation";

test("presents admin-only records as role boundaries without forbidden config reads", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "role-boundary request behavior is covered in the desktop console",
  );
  await installConsoleApiMock(page, { operatorRoleOverride: "operator" });
  await page.goto("/");
  await waitForConsoleShell(page);

  await openConsoleSubpage(page, "Access", "Overview");
  const requiredActions = page.getByLabel("Access actions required");
  await expect(requiredActions).toContainText("No immediate access actions");
  await expect(requiredActions).not.toContainText("Privilege state");
  const responsibilities = page.getByLabel("Access overview responsibilities");
  await expect(responsibilities).toContainText("Operators");
  await expect(responsibilities).toContainText("Admin only");
  await expect(responsibilities).not.toContainText("0 operators");
  const sessionScopes = page.getByLabel("Access session scopes");
  await expect(sessionScopes).toContainText("API bearer sessions");
  await expect(sessionScopes).toContainText("Admin only");
  await expect(sessionScopes).not.toContainText("0 listed");

  await openConsoleSubpage(page, "Audit", "Sessions");
  const evidenceSummary = page.getByLabel("Session evidence summary");
  await expect(evidenceSummary).toContainText("Bearer-session inventory");
  await expect(evidenceSummary).toContainText("Admin only");
  const operatorEvidence = page.getByLabel("Operator session evidence");
  await expect(operatorEvidence).toContainText("Admin only");
  await expect(operatorEvidence).not.toContainText("0 bearer sessions");

  await openConsoleSubpage(page, "Access", "Operators");
  await expect(page.getByText("Admin role required")).toBeVisible();
  await expect(page.getByText("Current role: operator.")).toBeVisible();
  await expect(page.getByRole("button", { name: "New", exact: true })).toHaveCount(0);

  await openConsoleSubpage(page, "Access", "VPS identities");
  await expect(
    page
      .getByLabel("VPS identities access boundary")
      .getByText("Admin role required"),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Register VPS", exact: true }),
  ).toHaveCount(0);

  await openConsoleSubpage(page, "Automation", "Agent updates");
  await expect(page.getByText(/Release metadata, update checks/)).toBeVisible();
  const updatePanel = page.locator(".agentReleasesPanel");
  await expect(updatePanel).toContainText("Registry policy unavailable");
  await expect(updatePanel).toContainText("Admin only");
  await expect(updatePanel).not.toContainText("Release registry advisory");
  await expect(
    updatePanel.getByRole("button", { name: "Update jobs" }),
  ).toBeVisible();
  await expect(
    updatePanel.getByRole("button", { name: "Latest job" }),
  ).toBeVisible();

  await openConsoleSubpage(page, "System", "Suite config");
  await expect(page.getByText("Admin role required")).toBeVisible();
  await expect(page.getByLabel("Suite config editor")).toHaveCount(0);

  await openConsoleSubpage(page, "System", "Maintenance");
  await expect(page.getByText("Admin role required")).toBeVisible();
  await expect(
    page.getByRole("button", { name: /Preview cleanup/i }),
  ).toHaveCount(0);

  expect(await suiteConfigReadCount(page)).toBe(0);
});

test("uses role-aware operator defaults and reports successful creation", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "operator mutation behavior is covered in the desktop console",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "Operators");

  await activate(page.getByRole("button", { name: "New", exact: true }));
  await page.getByLabel("Operator role").selectOption("admin");
  await expect(page.getByLabel("Session refresh TTL days")).toHaveValue("30");
  await page.getByLabel("Session refresh TTL days").fill("31");
  await expect(page.getByText(/above the 30-day policy target/)).toBeVisible();

  await activate(page.getByRole("button", { name: "New", exact: true }));
  await expect(page.getByLabel("Session refresh TTL days")).toHaveValue("365");
  await page.getByLabel("Operator username").fill("release-operator");
  await page.getByLabel("Operator password").fill("release-password-123");
  await activate(page.getByRole("button", { name: "Create", exact: true }));
  const prompt = page.getByLabel("Confirm user action");
  await expect(prompt).toBeVisible();
  await activate(prompt.getByRole("button", { name: "Create user" }));

  await expect(page.getByText("Created operator release-operator")).toBeVisible();
  await expect(page.getByText("3 operator records")).toBeVisible();
});

test("submits the privilege unlock form with Enter", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "keyboard unlock behavior is covered in the desktop console",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);

  await activate(
    page.locator(".topbar").getByRole("button", {
      name: "Open privilege unlock",
    }),
  );
  const dialog = page.getByRole("dialog", { name: "Unlock privilege" });
  await dialog.getByLabel(/privilege secret/i).fill("local-super-password");
  const verifier = dialog.getByLabel(/(privilege salt|verifier salt hex)/i);
  await verifier.fill("00112233445566778899aabbccddeeff");
  await verifier.press("Enter");

  await expect(dialog).toBeHidden();
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
});

test("privilege unlock reaches refreshable session actions while Audit stays read-only", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "privileged table action state is covered in the desktop console",
  );
  await installConsoleApiMock(page);
  await page.clock.setFixedTime(new Date("2026-01-02T12:00:00Z"));
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Access", "Operators");

  const unlockCurrentDialog = async () => {
    const dialog = page.getByRole("dialog", { name: "Unlock privilege" });
    await expect(dialog).toBeVisible();
    await dialog
      .getByLabel(/privilege secret/i)
      .fill("local-super-password");
    await dialog
      .getByLabel(/(privilege salt|verifier salt hex)/i)
      .fill("00112233445566778899aabbccddeeff");
    await activate(
      dialog
        .getByLabel("Unlock with privilege material")
        .getByRole("button", { name: /Unlock( privilege)?/ }),
    );
    await expect(dialog).toBeHidden();
  };

  const operatorRevoke = page
    .getByLabel("Operator accounts data grid")
    .getByRole("button", { name: "Revoke sessions" })
    .first();
  await expect(operatorRevoke).toBeEnabled();
  await activate(operatorRevoke);
  await unlockCurrentDialog();
  await activate(operatorRevoke);
  await expect(page.getByLabel("Confirm admin user action")).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm admin user action")
      .getByRole("button", { name: "Cancel" }),
  );

  await activate(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  );
  await openConsoleSubpage(page, "Audit", "Sessions");
  await expect(
    page.getByRole("button", { name: "Revoke session for console-admin" }),
  ).toHaveCount(0);
  await expect(
    page.getByLabel("Operator session evidence").getByText("Refreshable"),
  ).toHaveCount(2);
  await expect(page.getByText("2 expired bearer sessions")).toHaveCount(0);
});

test("keeps cross-route job evidence below the mobile toolbar", async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes("mobile"),
    "mobile sticky-offset and compact-card behavior is viewport-specific",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Automation", "Agent updates");

  await activate(page.getByRole("button", { name: "Latest job" }));
  const targetDetails = page.getByRole("region", {
    name: "Job target details",
  });
  await expect(targetDetails).toBeVisible();
  await expect(targetDetails).toBeFocused();

  const topbar = page.locator(".topbar");
  await expect
    .poll(async () => {
      const [detailBounds, topbarBounds] = await Promise.all([
        targetDetails.boundingBox(),
        topbar.boundingBox(),
      ]);
      return Boolean(
        detailBounds &&
          topbarBounds &&
          detailBounds.y >= topbarBounds.y + topbarBounds.height,
      );
    })
    .toBe(true);

  const detailHeader = targetDetails.locator(".targetDetailHeader");
  const closeButton = targetDetails.getByRole("button", {
    name: "Close job target details",
  });
  await expect
    .poll(async () => {
      const [headerBounds, closeBounds] = await Promise.all([
        detailHeader.boundingBox(),
        closeButton.boundingBox(),
      ]);
      return Boolean(
        headerBounds &&
          closeBounds &&
          closeBounds.x >= headerBounds.x + headerBounds.width / 2 &&
          closeBounds.y < headerBounds.y + headerBounds.height / 2,
      );
    })
    .toBe(true);

  const compactCell = targetDetails
    .locator(".gridMobileFieldValue > .gridCellContent")
    .first();
  await expect(compactCell).toHaveCSS("display", "block");
  const horizontalOverflowPx = await page.evaluate(() =>
    Math.max(
      0,
      document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  );
  expect(horizontalOverflowPx).toBeLessThanOrEqual(1);
});

test("shows the effective agent update policy without inferring from optional TOML", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the effective policy contract is viewport-independent",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Automation", "Agent updates");

  const posture = page.getByLabel("Agent update rollout posture");
  await expect(posture).toContainText("Registry policy");
  await expect(posture).toContainText("Advisory metadata");
  await expect(posture).toContainText(
    "suite config currently does not enforce them",
  );
  await expect(posture).not.toContainText("Unknown");

  await activate(
    page.getByRole("button", { name: "Check update", exact: true }).first(),
  );
  const composer = page.locator(".commandComposer");
  const manifestUrl = composer.getByLabel("Agent update version manifest URL");
  await expect(manifestUrl).toHaveValue(
    "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
  );
  await expect(manifestUrl).toHaveAttribute(
    "title",
    "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
  );
  await expect(composer).toContainText(
    "stages a newer checksum-verified artifact without activating or restarting it",
  );
  await expect(composer).toContainText(
    "Activation is a separate reviewed action",
  );
  await expect
    .poll(async () => {
      const [composerBounds, inputBounds] = await Promise.all([
        composer.boundingBox(),
        manifestUrl.boundingBox(),
      ]);
      if (!composerBounds || !inputBounds) return 0;
      return inputBounds.width / composerBounds.width;
    })
    .toBeGreaterThan(0.8);
});

test("uses persisted terminal-open evidence when retained audit rows are insufficient", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal evidence source selection is viewport-independent",
  );
  await installConsoleApiMock(page, {
    terminalSessionsOverride: [
      {
        session_id: "61616161-2222-4333-8444-555555555555",
        client_id: "agent-sfo-01",
        state: "open",
        last_status: "accepted",
        argv: ["/bin/sh", "-l"],
        cwd: "/root",
        cols: 100,
        rows: 30,
        idle_timeout_secs: 600,
        flow_window_bytes: 65536,
        output_first_seq: 1,
        output_next_seq: 4,
        output_retained_first_seq: 1,
        output_retained_bytes: 96,
        output_dropped_bytes: 0,
        output_dropped_chunks: 0,
        output_replay_truncated: false,
        last_input_seq: 2,
        session_exited: false,
        close_reason: null,
        last_event: "terminal_input",
        last_job_id: "61616161-aaaa-4bbb-8ccc-dddddddddddd",
        last_command_type: "terminal_input",
        last_seq: 0,
        opened_at: "2030-01-02T03:04:05Z",
        observed_at: "2030-01-02T03:10:00Z",
      },
    ],
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Audit", "Sessions");

  const detail = page.getByLabel("Selected terminal session evidence");
  await expect(detail).toContainText("2030");
  await expect(detail).not.toContainText("Open time unavailable");
});

test("keeps focused transfer review local, explicit, and frozen", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the focused transfer confirmation contract is covered on desktop",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const quickTransfer = page.getByLabel("New file transfer");
  await quickTransfer.getByLabel("Transfer upload local file").setInputFiles({
    name: "operator-simulation.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("operator transfer fixture\n"),
  });
  const target = quickTransfer.getByRole("combobox", {
    name: "Transfer target VPS",
  });
  await target.fill("edge-sfo-01");
  await page
    .getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ })
    .click();
  await quickTransfer
    .getByLabel("Transfer upload destination path")
    .fill("/tmp/operator-simulation.txt");
  await activate(
    quickTransfer.getByRole("button", { name: "Review upload" }),
  );

  const detail = page.locator(".consoleDetailPanel", {
    hasText: "File transfer",
  });
  await expect(detail).toBeVisible();
  await expect(detail.getByLabel("Dispatch mode boundary")).toContainText(
    "File transfer mode",
  );
  await expect(detail.getByLabel("Dispatch mode boundary")).not.toContainText(
    "Terminal mode",
  );
  await activate(
    detail.getByRole("button", { name: "Dispatch", exact: true }),
  );

  const prompt = page.getByLabel("Confirm job dispatch");
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("operator-simulation.txt");
  await expect(prompt).toContainText("/tmp/operator-simulation.txt");
  await expect(prompt).toContainText("edge-sfo-01 (agent-sfo-01)");
  await expect(prompt).toContainText("Skip upload if the file already exists");
  await expect(prompt).toContainText("Shared offset across all targets");
  const hashValue = prompt.locator("dt", { hasText: "Source SHA-256" })
    .locator("..").locator("dd");
  await expect(hashValue).toHaveAttribute("title", /^[0-9a-f]{64}$/);
  await expect(hashValue).toContainText("...");
  await expect(prompt.getByText("Symlinks", { exact: true })).toHaveCount(0);
});

test("invalidates a VPS rules preview as soon as its draft changes", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "preview invalidation is viewport-independent",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Config", "Rules");

  const editor = page.locator(".consoleDetailPanel", {
    hasText: "Bulk rule editor",
  });
  await editor.getByLabel("Total quota").fill("4TB");
  await activate(
    editor.getByRole("button", { name: "Preview changes", exact: true }),
  );
  await expect(page.locator(".vpsRulesPreviewBlock")).toBeVisible();

  await editor.getByLabel("Total quota").fill("5TB");
  await expect(page.locator(".vpsRulesPreviewBlock")).toHaveCount(0);
  await expect(page.locator(".vpsRulesActionFeedback")).toHaveCount(0);
});

test("keeps compact fleet and transfer controls usable on mobile", async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes("mobile"),
    "this regression is specific to the compact viewport",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);

  await openConsoleSubpage(page, "Fleet", "Assignments");
  const addGroup = page.locator(".inlineTagAdd").first();
  await expect(addGroup.getByRole("button")).toBeVisible();
  const addGroupBounds = await addGroup.boundingBox();
  const addGroupContainerBounds = await addGroup.locator("..").boundingBox();
  expect(addGroupBounds).not.toBeNull();
  expect(addGroupContainerBounds).not.toBeNull();
  expect(addGroupBounds!.width).toBeLessThanOrEqual(
    addGroupContainerBounds!.width + 1,
  );
  expect(addGroupBounds!.x + addGroupBounds!.width).toBeLessThanOrEqual(
    addGroupContainerBounds!.x + addGroupContainerBounds!.width + 1,
  );
  expect(
    await addGroup.evaluate(
      (element) => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);

  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  const reviewCheck = page.getByLabel("Include targets needing review");
  await expect(reviewCheck).toBeVisible();
  const reviewCheckBounds = await reviewCheck.boundingBox();
  expect(reviewCheckBounds?.width ?? 0).toBeLessThanOrEqual(20);
  expect(reviewCheckBounds?.height ?? 0).toBeLessThanOrEqual(20);

  await openConsoleSubpage(page, "Remote Operations", "Transfers");
  const direction = page.getByLabel("Transfer direction");
  await expect(direction.getByRole("button", { name: "Download" })).toBeVisible();
  const transferHeader = direction.locator("..");
  const transferHeadingBounds = await transferHeader
    .getByRole("heading")
    .boundingBox();
  const directionBounds = await direction.boundingBox();
  expect(transferHeadingBounds).not.toBeNull();
  expect(directionBounds).not.toBeNull();
  expect(directionBounds!.y).toBeGreaterThanOrEqual(
    transferHeadingBounds!.y + transferHeadingBounds!.height,
  );
  expect(
    await direction.evaluate(
      (element) => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
});

test("reports group creation beside the registry action", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the mutation feedback contract is viewport-independent",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Fleet", "Groups");

  await page.getByLabel("Group name").fill("simulation:created");
  await activate(page.getByRole("button", { name: "Create group" }));
  await expect(page.getByText("Created group simulation:created")).toBeVisible();
});

test("deleting a webhook rule retains its delivery evidence", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the retained-history mutation contract is viewport-independent",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Observability", "Event webhooks");

  const ruleGrid = page.getByLabel("Webhook rules data grid");
  const ruleRow = ruleGrid.locator(".gridBody [role=row]", {
    hasText: "edge-interval-webhook",
  });
  await ruleRow
    .getByRole("checkbox", { name: /Select Webhook rules row/ })
    .check();
  await ruleGrid.getByRole("button", { name: /Actions/ }).click();
  await page
    .getByRole("menuitem", { name: "Review deletion", exact: true })
    .click();
  const confirmation = page.getByLabel("Delete webhook rules");
  await expect(confirmation).toContainText(
    "Retained delivery history is not removed.",
  );
  await activate(
    confirmation.getByRole("button", { name: "Delete webhook rules" }),
  );
  await expect(ruleRow).toHaveCount(0);
  await expect(page.getByText("Deleted 1 webhook rule")).toBeVisible();

  await activate(page.getByRole("tab", { name: /Deliveries/ }));
  const deliveryGrid = page.getByLabel("Webhook delivery history data grid");
  await expect(deliveryGrid.locator(".gridBody [role=row]")).toHaveCount(3);
  await expect(deliveryGrid).toContainText("edge-interval-webhook");
  await expect(deliveryGrid).toContainText("canceled disabled");
  await expect(deliveryGrid).toContainText("webhook rule deleted");
});

async function suiteConfigReadCount(page: Page): Promise<number> {
  return page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { suiteConfigReads: number };
      }
    ).__vpsmanTestRequests;
    return requests.suiteConfigReads;
  });
}
