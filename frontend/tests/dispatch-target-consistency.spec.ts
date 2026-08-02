import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  backupId,
  installConsoleApiMock,
  tunnelPlans,
} from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

async function activate(locator: Locator) {
  await expect(locator).toBeVisible();
  await expect(locator).toBeEnabled();
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function expectFocusInside(locator: Locator) {
  await expect
    .poll(
      async () =>
        locator.evaluate((element) => element.contains(document.activeElement)),
      { message: "focus should remain inside the active modal" },
    )
    .toBe(true);
}

async function chooseVpsBySearch(
  root: Locator,
  label: string,
  query: string,
  optionName: RegExp,
) {
  const combobox = root.getByRole("combobox", { name: label });
  await combobox.fill(query);
  const option = root.page().locator(".vpsComboboxMenu").getByRole("option", {
    name: optionName,
  });
  await expect(option).toBeVisible();
  await expect
    .poll(
      async () => {
        const menuBox = await root
          .page()
          .locator(".vpsComboboxMenu")
          .boundingBox();
        const viewport = root.page().viewportSize();
        return Boolean(
          menuBox &&
          viewport &&
          menuBox.x >= 0 &&
          menuBox.y >= 0 &&
          menuBox.x + menuBox.width <= viewport.width &&
          menuBox.y + menuBox.height <= viewport.height,
        );
      },
      { message: `${label} options should remain inside the viewport` },
    )
    .toBe(true);
  const selectedLabel = (await option.locator("strong").innerText()).trim();
  await option.click();
  await expect(combobox).toHaveValue(selectedLabel);
}

async function includeBulkTagReviewTargets(page: Page) {
  const includeReviewTargets = page.getByLabel(
    "Include targets needing review",
  );
  await expect(includeReviewTargets).toBeVisible();
  await includeReviewTargets.check();
  await expect(includeReviewTargets).toBeChecked();
}

async function reviewBulkTagMutation(page: Page) {
  await activate(
    page.locator(".bulkTagApplyGrid").getByRole("button", {
      name: /maintenance:test/,
    }),
  );
}

async function unlockPrivilege(page: Page, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Jobs", subpage);
}

async function unlockPrivilegeFor(page: Page, view: string, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, view, subpage);
}

async function reviewOspfCostUpdate(page: Page) {
  const plans = page.getByLabel("OSPF updater plans data grid");
  await plans
    .getByRole("row", { name: /sfo-fra-gre/ })
    .click({ button: "right" });
  await activate(
    page.getByRole("menuitem", { name: "Apply cost", exact: true }),
  );
}

function expectPrivilegeAssertion(request: unknown) {
  expect((request as { envelope?: unknown }).envelope).toBeUndefined();
  expect((request as { envelopes?: unknown }).envelopes).toBeUndefined();
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
}

test("job target verification distinguishes API outage from invalid syntax", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "target verification semantics are shared with the mobile composer",
  );
  await installConsoleApiMock(page, { bulkResolveFailure: true });
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  await page
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");

  await expect(page.getByText("Unavailable", { exact: true })).toBeVisible();
  const feedback = page.locator(".commandComposer .actionFeedbackDanger");
  await expect(feedback).toContainText("Target inventory could not be read");
  await expect(feedback).toContainText("no success is assumed");
  await expect(feedback).not.toContainText("Invalid");
});

test("job dispatch submits backend-resolved targets when dashboard inventory is stale", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dispatch consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page, { agentListOverride: [] });
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");
  await unlockPrivilege(page, "Dispatch");

  await page.getByLabel("Command argv").fill("/usr/bin/uptime");
  await page
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");
  await activate(
    page
      .locator(".commandComposer")
      .getByRole("button", { name: "Dispatch", exact: true }),
  );
  const firstPrompt = page.getByLabel("Confirm job dispatch");
  await expect(firstPrompt).toBeVisible();
  await expect(firstPrompt).toContainText("Resolved VPS");
  await expect(firstPrompt).toContainText("edge-sfo-01 (agent-sfo-01)");
  await page.getByLabel("Command argv").fill("/usr/bin/id");
  await expect(page.getByText("Confirm job dispatch")).toBeHidden();
  await activate(
    page
      .locator(".commandComposer")
      .getByRole("button", { name: "Dispatch", exact: true }),
  );
  await expect(page.getByText("Confirm job dispatch")).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Dispatch job" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    argv: ["/usr/bin/id"],
    command: "shell_argv",
    operation: {
      argv: ["/usr/bin/id"],
      type: "shell",
    },
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });
});

test("bulk file run resolves targets again instead of executing cached preview", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk file consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.multiFile.selectorExpression"),
  );
  await openConsoleSubpage(page, "Remote Operations", "Bulk files");
  await unlockPrivilegeFor(page, "Remote Operations", "Bulk files");

  await activate(page.getByRole("button", { name: "Refresh scope" }));
  await expect(page.getByText("3 VPSs resolved")).toBeVisible();
  const resolveCountAfterPreview = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { bulkResolve: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.bulkResolve.length;
  });
  expect(resolveCountAfterPreview).toBe(1);

  await page.getByLabel("Bulk file path").fill("/etc/app.conf");
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Close confirmation" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeHidden();
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeVisible();
  await page.getByLabel("Bulk file path").fill("/etc/app.conf.d/current");
  await expect(page.getByText("Confirm bulk file operation")).toBeHidden();
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeVisible();
  const resolveCountAfterReview = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { bulkResolve: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.bulkResolve.length;
  });
  expect(resolveCountAfterReview).toBe(4);

  await activate(
    page
      .getByLabel("Confirm bulk file operation")
      .getByRole("button", { name: "Download files" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.find(
      (entry) =>
        (entry as { operation?: { type?: string } }).operation?.type ===
        "file_download",
    );
  });
  expect(request).toMatchObject({
    operation: { path: "/etc/app.conf.d/current", type: "file_download" },
    selector_expression: "id:*",
    target_client_ids: ["agent-fra-02", "agent-nyc-03", "agent-sfo-01"],
  });
});

test("bulk file async run preparation ignores stale path edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk file async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.multiFile.selectorExpression"),
  );
  await openConsoleSubpage(page, "Remote Operations", "Bulk files");
  await unlockPrivilegeFor(page, "Remote Operations", "Bulk files");

  await page.getByLabel("Bulk file path").fill("/etc/app.conf");
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Preparing bulk file run")).toBeVisible();
  await page.getByLabel("Bulk file path").fill("/etc/app.conf.next");
  await expect(page.getByText("Preparing bulk file run")).toBeHidden();
  await expect(page.getByText("Confirm bulk file operation")).toBeHidden();

  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm bulk file operation")
      .getByRole("button", { name: "Download files" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.find(
      (entry) =>
        (entry as { operation?: { type?: string } }).operation?.type ===
        "file_download",
    );
  });
  expect(request).toMatchObject({
    operation: { path: "/etc/app.conf.next", type: "file_download" },
    selector_expression: "id:*",
    target_client_ids: ["agent-fra-02", "agent-nyc-03", "agent-sfo-01"],
  });
});

test("bulk config review uses the current backend-resolved selector instead of a stale preview", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk config consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Bulk patch");
  await unlockPrivilegeFor(page, "Config", "Bulk patch");

  const panel = page.locator(".configApplyGrid");
  await panel
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Preview changes" }));
  await expect(panel.getByText("1 VPS verified")).toBeVisible();
  await panel
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("id:agent-fra-02");
  await expect(
    panel.getByRole("button", { name: "Apply patch" }),
  ).toBeDisabled();
  await activate(panel.getByRole("button", { name: "Preview changes" }));
  await activate(panel.getByRole("button", { name: "Apply patch" }));
  await expect(page.getByText("Confirm bulk patch")).toBeVisible();
  await activate(
    page.getByRole("button", { name: "Apply runtime config patch" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { runtimeConfigPatches: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("bulk tag mutation requires a fresh preview after selector edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk tag consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await unlockPrivilegeFor(page, "Fleet", "Bulk groups");

  await page.getByLabel("Bulk group", { exact: true }).fill("maintenance:test");
  await page
    .getByRole("combobox", { name: "Bulk group selector expression" })
    .fill("id:agent-sfo-01");
  await expect(page.getByLabel("Bulk group local VPS preview")).toContainText(
    "edge-sfo-01",
  );
  await includeBulkTagReviewTargets(page);
  await reviewBulkTagMutation(page);
  await expect(page.locator(".bulkTagPreview")).toContainText("edge-sfo-01");
  await expect(page.getByText("Confirm tag mutation")).toBeVisible();
  await page
    .getByRole("combobox", { name: "Bulk group selector expression" })
    .fill("id:agent-fra-02");
  await expect(page.getByText("Confirm tag mutation")).toBeHidden();
  await reviewBulkTagMutation(page);
  await expect(page.getByText("Confirm tag mutation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Apply tag mutation" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { bulkTagMutations: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.bulkTagMutations.at(-1);
  });
  expect(request).toMatchObject({
    action: "add",
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("affected schedule review keeps its navigation action above the evidence table", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page, {
    bulkTagScheduleImpacts: [
      {
        schedule_id: "60000000-0000-4000-8000-000000000001",
        name: "Edge maintenance",
        command_type: "shell",
        selector_expression: "tag:maintenance:test",
        before_target_count: 1,
        after_target_count: 2,
        added_target_count: 1,
        removed_target_count: 0,
        unchanged_target_count: 1,
        added_targets: [],
        removed_targets: [],
        summary: "One VPS now matches",
      },
    ],
  });
  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await unlockPrivilegeFor(page, "Fleet", "Bulk groups");
  await page.getByLabel("Bulk group", { exact: true }).fill("maintenance:test");
  const selector = page.getByRole("combobox", {
    name: "Bulk group selector expression",
  });
  await selector.fill("id:agent-sfo-01");
  await selector.press("Escape");
  await includeBulkTagReviewTargets(page);
  await reviewBulkTagMutation(page);

  const confirmation = page.getByLabel("Confirm tag mutation");
  const table = confirmation.getByRole("table", { name: "Affected schedules" });
  await expect(table).toBeVisible();
  const impactHeader = table.getByRole("columnheader", { name: "Impact" });
  if (testInfo.project.name.includes("mobile")) {
    await expect(impactHeader).toHaveCount(1);
  } else {
    await expect(impactHeader).toBeVisible();
  }
  await expect(table.getByRole("columnheader", { name: "Manual action" })).toHaveCount(0);
  await expect(table.getByRole("button", { name: "Open schedules" })).toHaveCount(0);
  const openSchedules = confirmation.getByRole("button", { name: "Open schedules" });
  await expect(openSchedules).toHaveCount(1);
  await activate(openSchedules);
  await expect(page.getByRole("heading", { name: "Schedules", exact: true })).toBeVisible();
});

test("job dispatch async review preparation ignores stale edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dispatch async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");
  await unlockPrivilege(page, "Dispatch");

  await page.getByLabel("Command argv").fill("/usr/bin/uptime");
  await page
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");
  await activate(
    page
      .locator(".commandComposer")
      .getByRole("button", { name: "Dispatch", exact: true }),
  );
  await expect(page.getByText("Preparing dispatch confirmation")).toBeVisible();
  await page.getByLabel("Command argv").fill("/usr/bin/id");
  await expect(page.getByText("Preparing dispatch confirmation")).toBeHidden();
  await expect(page.getByText("Confirm job dispatch")).toBeHidden();

  await activate(
    page
      .locator(".commandComposer")
      .getByRole("button", { name: "Dispatch", exact: true }),
  );
  await expect(page.getByText("Confirm job dispatch")).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Dispatch job" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    argv: ["/usr/bin/id"],
    command: "shell_argv",
    operation: {
      argv: ["/usr/bin/id"],
      type: "shell",
    },
  });
});

test("bulk tag async preview ignores stale selector edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk tag async preview consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Bulk groups");
  await unlockPrivilegeFor(page, "Fleet", "Bulk groups");

  await page.getByLabel("Bulk group", { exact: true }).fill("maintenance:test");
  const selector = page.getByRole("combobox", {
    name: "Bulk group selector expression",
  });
  await selector.fill("id:agent-sfo-01");
  await includeBulkTagReviewTargets(page);
  await reviewBulkTagMutation(page);
  await expect(page.getByText("Confirm tag mutation")).toBeVisible();
  await selector.fill("id:agent-fra-02");
  await expect(page.getByText("Confirm tag mutation")).toBeHidden();
  await expect(page.locator(".bulkTagPreview")).toHaveCount(0);

  await reviewBulkTagMutation(page);
  await expect(page.locator(".bulkTagPreview")).toContainText("core-fra-02");
  await expect(page.getByText("Confirm tag mutation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Apply tag mutation" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { bulkTagMutations: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.bulkTagMutations.at(-1);
  });
  expect(request).toMatchObject({
    action: "add",
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("expanded VPS detail scopes row actions and preview-binds inline tag changes", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "expanded-row geometry and context actions are covered in the desktop grid",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await unlockPrivilegeFor(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  const row = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await activate(row.getByLabel("Expand VPS instance records row"));
  const detail = grid
    .locator(".gridExpandedRow", { hasText: "edge-sfo-01" })
    .first();
  await expect(detail).toBeVisible();

  await detail.locator(".fleetNodeDetailHeader").click({ button: "right" });
  await expect(page.locator(".consoleMenu:visible")).toHaveCount(0);
  await row.click({ button: "right" });
  await expect(page.locator(".consoleMenu:visible")).toContainText(
    "Row actions",
  );
  await page.keyboard.press("Escape");

  await detail.getByLabel("VPS display name").fill("edge-sfo-renamed");
  await activate(detail.getByRole("button", { name: "Rename" }));
  const controls = detail.locator(".fleetNodeDetailControls");
  const prompt = detail.getByLabel("Confirm VPS rename");
  await expect(prompt).toBeVisible();
  const promptLayout = await Promise.all([
    controls.boundingBox(),
    prompt.boundingBox(),
    prompt.evaluate((element) => {
      const style = getComputedStyle(element);
      return {
        alignSelf: style.alignSelf,
        borderTopStyle: style.borderTopStyle,
      };
    }),
  ]);
  expect(promptLayout[0]).not.toBeNull();
  expect(promptLayout[1]).not.toBeNull();
  expect(
    Math.abs((promptLayout[1]?.x ?? 0) - (promptLayout[0]?.x ?? 0)),
  ).toBeLessThanOrEqual(1);
  expect(
    Math.abs((promptLayout[1]?.width ?? 0) - (promptLayout[0]?.width ?? 0)),
  ).toBeLessThanOrEqual(2);
  expect(promptLayout[2]).toMatchObject({
    alignSelf: "start",
    borderTopStyle: "solid",
  });
  await activate(prompt.getByRole("button", { name: "Cancel" }));

  await detail.getByLabel("Fleet inline tag").fill("maintenance:inline");
  await activate(detail.getByRole("button", { name: "Add", exact: true }));
  await expect(detail).toContainText(
    "add maintenance:inline: 1 changed, 0 skipped",
  );
  const requests = await page.evaluate(() =>
    (
      window as unknown as {
        __vpsmanTestRequests: { bulkTagMutations: unknown[] };
      }
    ).__vpsmanTestRequests.bulkTagMutations.slice(-2),
  );
  expect(requests).toHaveLength(2);
  expect(requests[0]).toMatchObject({
    action: "add",
    confirmed: false,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    tag: "maintenance:inline",
  });
  expect(requests[1]).toMatchObject({
    action: "add",
    confirmed: true,
    preview_hash: "7".repeat(64),
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    tag: "maintenance:inline",
  });
  expectPrivilegeAssertion(requests[1]);
});

test("artifact cleanup async preview ignores stale expression edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "server cleanup async preview consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Maintenance");

  const cleanupPanel = page.locator(".fleetPanel", {
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  const objectPrefix = cleanupPanel.getByLabel("Object path prefix");
  await objectPrefix.fill("job-outputs/");
  await activate(cleanupPanel.getByRole("button", { name: "Preview" }));
  await expect(page.getByText("Preparing cleanup preview")).toBeVisible();
  await objectPrefix.fill("file-transfer-sources/");
  await expect(page.getByText("Preparing cleanup preview")).toBeHidden();
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    "Preview required",
  );
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeDisabled();

  await activate(cleanupPanel.getByRole("button", { name: "Preview" }));
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toContainText(
    "1 artifact / 22 B",
  );
  await expect(cleanupPanel.getByLabel("Cleanup preview result")).toBeFocused();
  await expect(
    cleanupPanel.getByLabel("Artifact cleanup readiness"),
  ).toContainText("Ready for confirmation");
  await expect(
    cleanupPanel.getByLabel("Representative cleanup objects"),
  ).toContainText("file-transfer-sources/");
  await expect(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  ).toBeEnabled();
  await activate(
    cleanupPanel.getByRole("button", { name: "Delete artifacts" }),
  );
  await expect(
    page.getByRole("region", { name: "Confirm artifact deletion" }),
  ).toBeVisible();
  await page
    .getByLabel("Type DELETE to confirm artifact deletion")
    .fill("DELETE");
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Delete artifacts" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { artifactCleanupJobs: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.artifactCleanupJobs.at(-1);
  });
  expect(request).toMatchObject({
    domains: ["job_output", "file_transfer"],
    preview_hash:
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  });
  expect((request as { expression?: string }).expression).toContain(
    'artifact.object = "file-transfer-sources/*"',
  );
});

test("backup policy review submits a frozen target list and privilege assertion", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup policy consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Policies");
  await unlockPrivilegeFor(page, "Backups", "Policies");

  await activate(page.getByRole("button", { name: "Create policy" }).first());
  await page.getByLabel("Backup policy name").fill("nightly system backup");
  const policySelector = page.getByRole("combobox", {
    name: "Backup policy target expression",
  });
  await policySelector.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("id:agent-fra-02");
  await page.keyboard.press("Escape");
  await expect(
    page.getByLabel("Backup policy local VPS preview"),
  ).toContainText("core-fra-02");
  await page.getByRole("checkbox", { name: "Skip missing roots" }).check();
  await page.getByRole("checkbox", { name: "Enabled" }).uncheck();
  await activate(page.getByRole("button", { name: "Review policy" }));
  await expect(page.getByText("Confirm backup policy")).toBeVisible();
  await expect(
    page.getByText("Disabled - saved without scheduled runs"),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Save policy" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { backupPolicies: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.backupPolicies.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    enabled: false,
    missing_path_policy: "skip",
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
});

test("backup workflow confirmations clear when switching backup subpages", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup confirmation lifecycle is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Requests");
  await unlockPrivilegeFor(page, "Backups", "Requests");

  await activate(page.getByRole("button", { name: "Run backup", exact: true }));
  const requestWorkflow = page.getByLabel("Run backup");
  await chooseVpsBySearch(
    requestWorkflow,
    "Backup client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(
    requestWorkflow.getByRole("button", { name: "Review backup run" }),
  );
  await expect(requestWorkflow.getByLabel("Confirm backup run")).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Policies");
  await expect(page.getByLabel("Confirm backup run")).toBeHidden();
});

test("backup run dispatches one audited backup job for the reviewed VPS", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup dispatch request integrity is covered in the desktop workflow",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Requests");
  await unlockPrivilegeFor(page, "Backups", "Requests");

  await activate(page.getByRole("button", { name: "Run backup", exact: true }));
  const workflow = page.getByRole("complementary", { name: "Run backup" });
  await chooseVpsBySearch(
    workflow,
    "Backup client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(workflow.getByRole("button", { name: "Review backup run" }));
  const confirmation = workflow.getByLabel("Confirm backup run");
  await expect(confirmation).toContainText(
    "Completion records archive metadata; a verified upload or retained-output transfer package is required before restore or download.",
  );
  await activate(confirmation.getByRole("button", { name: "Run backup" }));
  await expect(workflow).toContainText(/Backup job .* (queued|running)/);

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(request).toMatchObject({
    command: "backup",
    confirmed: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    operation: {
      include_config: true,
      type: "backup",
    },
    privileged: true,
  });
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
});

test("bulk config async review preparation ignores stale selector edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "bulk config async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Bulk patch");
  await unlockPrivilegeFor(page, "Config", "Bulk patch");

  const panel = page.locator(".configApplyGrid");
  const selector = panel.getByRole("combobox", {
    name: "Bulk patch target expression",
  });
  await selector.fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Preview changes" }));
  await expect(
    panel.locator(".configReviewFeedback.actionFeedbackProgress"),
  ).toContainText("Previewing bulk patch changes");
  await expect(
    panel.locator(".formHint", { hasText: "Previewing bulk patch changes" }),
  ).toHaveCount(0);
  await selector.fill("id:agent-fra-02");
  await expect(
    panel.locator(".configReviewFeedback.actionFeedbackProgress"),
  ).toHaveCount(0);
  await expect(page.getByText("Confirm bulk patch")).toBeHidden();

  await activate(panel.getByRole("button", { name: "Preview changes" }));
  await activate(panel.getByRole("button", { name: "Apply patch" }));
  await expect(page.getByText("Confirm bulk patch")).toBeVisible();
  await activate(
    page.getByRole("button", { name: "Apply runtime config patch" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { runtimeConfigPatches: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("access key lifecycle async reviews ignore stale field edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "access key lifecycle async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Access", "VPS identities");
  await unlockPrivilegeFor(page, "Access", "VPS identities");

  const inspector = page.locator(".accessInspector");
  await activate(page.getByRole("button", { name: "Register VPS" }));
  await inspector.getByLabel("Agent identity client ID").fill("agent-tokyo-04");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("a".repeat(64));
  await inspector
    .getByLabel("Agent identity display name")
    .fill("edge-tokyo-a");
  await activate(
    inspector.getByRole("button", { name: "Review registration" }),
  );
  await expect(inspector.getByText("Preparing review")).toBeVisible();
  await inspector
    .getByLabel("Agent identity display name")
    .fill("edge-tokyo-b");
  await expect(inspector.getByText("Preparing review")).toBeHidden();
  await expect(
    page.getByLabel("Confirm VPS identity registration"),
  ).toBeHidden();

  await activate(
    inspector.getByRole("button", { name: "Review registration" }),
  );
  await expect(
    page.getByLabel("Confirm VPS identity registration"),
  ).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm VPS identity registration")
      .getByRole("button", { name: "Register VPS" }),
  );
  const identityRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { agentIdentities: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.agentIdentities.at(-1);
  });
  expect(identityRequest).toMatchObject({
    client_id: "agent-tokyo-04",
    display_name: "edge-tokyo-b",
  });

  const identityGrid = page.getByLabel("VPS identities data grid");
  await identityGrid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first()
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Revoke" }).click();
  await expect(
    inspector.getByRole("heading", { name: "Revoke VPS key" }),
  ).toBeVisible();
  await expect(
    inspector.getByRole("combobox", { name: "VPS identity revoke VPS ID" }),
  ).toHaveValue(/edge-sfo-01/);
  await inspector.getByLabel("VPS identity revoke reason").fill("reason-a");
  await activate(inspector.getByRole("button", { name: "Revoke current key" }));
  await expect(inspector.locator(".accessRevokeActionFeedback")).toContainText(
    "Preparing key revoke review",
  );
  await inspector.getByLabel("VPS identity revoke reason").fill("reason-b");
  await expect(inspector.locator(".accessRevokeActionFeedback")).toBeHidden();
  await expect(page.getByLabel("Confirm current key revocation")).toBeHidden();

  await activate(inspector.getByRole("button", { name: "Revoke current key" }));
  await expect(page.getByLabel("Confirm current key revocation")).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm current key revocation")
      .getByRole("button", { name: "Revoke key" }),
  );
  const revokeRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { clientKeyRevocations: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.clientKeyRevocations.at(-1);
  });
  expect(revokeRequest).toMatchObject({
    reason: "reason-b",
  });
});

test("fleet delete review clears on selection changes and ignores stale review completion", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "fleet delete review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await unlockPrivilegeFor(page, "Fleet", "Instances");

  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const backupRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  const sfoRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await backupRow.getByLabel("Select VPS instance records row").check();
  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions" })
    .click();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  await sfoRow.getByLabel("Select VPS instance records row").check();
  await page.waitForTimeout(180);
  await expect(page.getByText("Delete VPS from panel")).toBeHidden();

  await backupRow.getByLabel("Select VPS instance records row").uncheck();
  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions" })
    .click();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  const prompt = page.locator(".fleetInstancesPanel > .confirmationPrompt");
  await expect(prompt.getByText("Delete VPS from panel")).toBeVisible();
  await activate(prompt.getByRole("button", { name: "Delete VPS" }));

  const deleteRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { agentDeletes: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.agentDeletes.at(-1);
  });
  expect(deleteRequest).toMatchObject({
    confirmed: true,
    reason: "Deleted from fleet inventory selection action",
  });
});

test("overlay confirmations trap focus and close before accepted delete settles", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "overlay modal focus behavior is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page, { agentDeleteDelayMs: 750 });
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Preferences");
  await page.getByLabel("Review prompt display mode").selectOption("overlay");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await unlockPrivilegeFor(page, "Fleet", "Instances");

  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const backupRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await backupRow.getByLabel("Select VPS instance records row").check();
  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions" })
    .click();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();

  const dialog = page.getByRole("dialog", { name: "Delete VPS from panel" });
  await expect(dialog).toBeVisible();
  await expect(page.locator(".confirmationPromptOverlay")).toBeVisible();
  await expectFocusInside(dialog);
  for (let index = 0; index < 6; index += 1) {
    await page.keyboard.press("Tab");
    await expectFocusInside(dialog);
  }
  await page.keyboard.press("Shift+Tab");
  await expectFocusInside(dialog);

  await activate(dialog.getByRole("button", { name: "Delete VPS" }));
  await expect(dialog).toBeHidden();
  await expect(page.locator(".confirmationPromptOverlay")).toBeHidden();

  await expect
    .poll(async () =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: { agentDeletes: unknown[] };
          }
        ).__vpsmanTestRequests;
        return requests.agentDeletes.length;
      }),
    )
    .toBe(1);
});

test("overlay confirmations restore focus after asynchronous review preparation", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "overlay modal focus behavior is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Preferences");
  await page.getByLabel("Review prompt display mode").selectOption("overlay");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await openConsoleSubpage(page, "Fleet", "Bulk groups");

  await page.getByLabel("Bulk group", { exact: true }).fill("maintenance:test");
  await page
    .getByRole("combobox", { name: "Bulk group selector expression" })
    .fill("id:agent-sfo-01");
  await includeBulkTagReviewTargets(page);
  const reviewButton = page
    .locator(".bulkTagApplyGrid")
    .getByRole("button", { name: "Add maintenance:test to 1 VPS" });
  await reviewButton.focus();
  await reviewButton.click();

  const dialog = page.getByRole("dialog", { name: "Confirm tag mutation" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  await expect(reviewButton).toBeFocused();
});

test("overlay confirmations restore focus when the prompt mounts synchronously", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "overlay modal focus behavior is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Preferences");
  await page.getByLabel("Review prompt display mode").selectOption("overlay");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await unlockPrivilegeFor(page, "Jobs", "Dispatch");

  const main = page.locator("#console-main-content");
  await main.getByLabel("Command argv").fill("/bin/echo focus-check");
  await main
    .getByRole("combobox", { name: "Bulk target selector expression" })
    .fill("id:agent-sfo-01");
  const reviewButton = main.getByRole("button", {
    name: "Dispatch",
    exact: true,
  });
  await reviewButton.focus();
  await reviewButton.click();

  const dialog = page.getByRole("dialog", { name: "Confirm job dispatch" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  await expect(reviewButton).toBeFocused();
});

test("topology network test confirmation closes on edit and submits a fresh snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "network confirmation consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tests");
  await unlockPrivilegeFor(page, "Network", "Tests");

  await page.getByLabel("Network test max timeout seconds").fill("90");
  await activate(page.getByRole("button", { name: "Review speed test" }));
  await expect(page.getByText("Confirm speed test")).toBeVisible();
  await page.getByLabel("Network test max timeout seconds").fill("120");
  await expect(page.getByText("Confirm speed test")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review speed test" }));
  await expect(page.getByText("Confirm speed test")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Run speed test",
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "network_speed_test",
    confirmed: true,
    selector_expression: "id:agent-sfo-01 || id:agent-fra-02",
    target_client_ids: ["agent-sfo-01", "agent-fra-02"],
    max_timeout_secs: 120,
    operation: {
      plan_id: tunnelPlans[0].id,
      server_side: "left",
      type: "network_speed_test",
    },
  });
});

test("topology async review preparation ignores stale edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "network async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tests");
  await unlockPrivilegeFor(page, "Network", "Tests");

  await activate(page.getByRole("button", { name: "Review speed test" }));
  await expect(page.getByText("Preparing speed test review")).toBeVisible();
  await page.getByLabel("Network test max timeout seconds").fill("135");
  await expect(page.getByText("Preparing speed test review")).toBeHidden();
  await expect(page.getByText("Confirm speed test")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review speed test" }));
  await expect(page.getByText("Confirm speed test")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Run speed test",
    }),
  );

  const speedRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(speedRequest).toMatchObject({
    command: "network_speed_test",
    confirmed: true,
    max_timeout_secs: 135,
    operation: {
      plan_id: tunnelPlans[0].id,
      server_side: "left",
      type: "network_speed_test",
    },
  });

  await openConsoleSubpage(page, "Network", "OSPF");
  await reviewOspfCostUpdate(page);
  await expect(page.getByText("Confirm OSPF cost update")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Apply routing cost",
    }),
  );

  const ospfRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          tunnelPlanOspfCostUpdates: Array<{ plan_id: string; body: unknown }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanOspfCostUpdates.at(-1);
  });
  expect(ospfRequest).toMatchObject({
    body: {
      confirmed: true,
      desired_ospf_cost: 22,
      left_adapter_definition_hash: "c".repeat(64),
      left_current_ospf_cost: 14,
      plan_revision: tunnelPlans[0].revision,
      recommendation_id: "ospf-1234abcd5678ef90",
      right_adapter_definition_hash: "d".repeat(64),
      right_current_ospf_cost: 14,
    },
  });
  expectPrivilegeAssertion((ospfRequest as { body: unknown }).body);
});

test("privileged confirmation closes when the local assertion expires", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "privilege assertion expiry is covered in desktop workflow tests",
  );
  await page.clock.install({
    time: new Date("2026-06-18T00:00:00Z"),
  });
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tests");
  await unlockPrivilegeFor(page, "Network", "Tests");

  await activate(page.getByRole("button", { name: "Review speed test" }));
  await expect(page.getByText("Confirm speed test")).toBeVisible();
  await page.clock.fastForward(301_000);
  await expect(page.getByText("Confirm speed test")).toBeHidden();
});

test("OSPF cost update submits a frozen endpoint-updater snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "OSPF confirmation consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "OSPF");
  await unlockPrivilegeFor(page, "Network", "OSPF");

  await reviewOspfCostUpdate(page);
  const applyPrompt = page.locator(".confirmationPrompt").last();
  await expect(applyPrompt).toContainText("Confirm OSPF cost update");
  await expect(applyPrompt).toContainText("Current costs");
  await expect(applyPrompt).toContainText("14 / 14");
  await expect(applyPrompt).toContainText("Desired cost");
  await expect(applyPrompt).toContainText("Updater snapshots");
  await activate(
    applyPrompt.getByRole("button", {
      name: "Apply routing cost",
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          tunnelPlanOspfCostUpdates: Array<{ plan_id: string; body: unknown }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanOspfCostUpdates.at(-1);
  });
  expect(request).toMatchObject({
    body: {
      confirmed: true,
      desired_ospf_cost: 22,
      left_adapter_definition_hash: "c".repeat(64),
      left_current_ospf_cost: 14,
      plan_revision: tunnelPlans[0].revision,
      recommendation_id: "ospf-1234abcd5678ef90",
      right_adapter_definition_hash: "d".repeat(64),
      right_current_ospf_cost: 14,
    },
  });
  expectPrivilegeAssertion((request as { body: unknown }).body);
  await expect(page.getByText("Jobs in progress")).toBeVisible();
  await expect(page.getByRole("button", { name: /rollback/i })).toHaveCount(0);
});

test("tunnel plan submits a fresh explicit declaration after reopening review", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "tunnel declaration consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Create plan" }));

  const composer = page.locator(".tunnelPlanComposer");
  await composer.getByLabel("Tunnel plan name").fill("managed-edge-link");
  const interfaceInput = composer.getByLabel("Tunnel interface", {
    exact: true,
  });
  await interfaceInput.fill("tun-old");
  await chooseVpsBySearch(
    composer,
    "Left tunnel VPS",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await chooseVpsBySearch(
    composer,
    "Right tunnel VPS",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await composer
    .getByLabel("Left remote underlay destination")
    .fill("203.0.113.20");
  await composer
    .getByLabel("Right remote underlay destination")
    .fill("198.51.100.10");
  await composer.getByLabel("Left tunnel IPv4").fill("10.255.60.0");
  await composer.getByLabel("Right tunnel IPv4").fill("10.255.60.1");
  await activate(composer.getByRole("button", { name: "External adapter" }));
  await composer
    .getByLabel("Left runtime adapter", { exact: true })
    .selectOption("33333333-3333-4333-8333-333333333333");
  await composer
    .getByLabel("Right runtime adapter", { exact: true })
    .selectOption("33333333-3333-4333-8333-333333333333");

  await activate(composer.getByRole("button", { name: "Review plan" }));
  const confirmation = page.locator(".confirmationPrompt", {
    hasText: "Confirm tunnel plan creation",
  });
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("managed-edge-link · GRE · tun-old");
  await activate(
    confirmation.getByRole("button", { name: "Close confirmation" }),
  );
  await expect(confirmation).toBeHidden();

  await interfaceInput.fill("tun-new");
  await activate(composer.getByRole("button", { name: "Review plan" }));
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("managed-edge-link · GRE · tun-new");
  await activate(confirmation.getByRole("button", { name: "Save plan" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { tunnelPlans: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlans.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    interface_name: "tun-new",
    left_client_id: "agent-sfo-01",
    name: "managed-edge-link",
    right_client_id: "agent-fra-02",
    runtime_control: {
      left_adapter_template_id: "33333333-3333-4333-8333-333333333333",
      manager: "external_managed_adapter",
      right_adapter_template_id: "33333333-3333-4333-8333-333333333333",
    },
  });
});

test("single config applies one-VPS override from a frozen exact target", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "single config apply workflow is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.config.single.clientId"),
  );
  await openConsoleSubpage(page, "Config", "Per-VPS");
  await unlockPrivilegeFor(page, "Config", "Per-VPS");

  const panel = page.locator(".configApplyGrid");
  await chooseVpsBySearch(
    panel,
    "VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await activate(panel.getByRole("button", { name: "Read current config" }));
  const editor = panel.getByLabel("VPS redacted runtime config TOML");
  await expect(editor).toHaveValue(/client_id = "agent-fra-02"/);
  await expect(editor).toHaveAttribute("readonly", "");

  await panel
    .getByLabel("One-VPS runtime config override TOML")
    .fill("[update]\nunmanaged_enabled = true\n");
  await activate(panel.getByRole("button", { name: "Apply patch" }));
  await expect(
    page.getByLabel("Confirm one-VPS runtime config override"),
  ).toBeVisible();
  await chooseVpsBySearch(
    panel,
    "VPS config target",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await expect(
    page.getByLabel("Confirm one-VPS runtime config override"),
  ).toBeHidden();
  await expect(panel.getByRole("button", { name: "Apply patch" })).toHaveCount(
    0,
  );

  await chooseVpsBySearch(
    panel,
    "VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(panel.locator(".configTargetMeta")).toContainText("core-fra-02");
  await activate(panel.getByRole("button", { name: "Read current config" }));
  await expect(editor).toHaveValue(/client_id = "agent-fra-02"/);
  await expect(panel.getByLabel("One-VPS config override guard")).toContainText(
    "Current base",
  );
  await panel
    .getByLabel("One-VPS runtime config override TOML")
    .fill("[update]\nunmanaged_enabled = true\n");
  await activate(panel.getByRole("button", { name: "Apply patch" }));
  const confirmation = page.getByLabel(
    "Confirm one-VPS runtime config override",
  );
  await expect(confirmation).toBeVisible();
  await activate(
    confirmation.getByRole("button", { name: "Apply one-VPS override" }),
  );

  const readRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.find(
      (request) => (request as { command?: string }).command === "config_read",
    );
  });
  expect(readRequest).toMatchObject({
    command: "config_read",
    force_unprivileged: true,
    privileged: false,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
    operation: {
      type: "config_read",
    },
  });

  const patchRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(patchRequest).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
  expect(patchRequest?.toml).toContain("unmanaged_enabled = true");
});

test("backup restore confirmations close on edit and submit fresh snapshots", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup restore consistency is covered in desktop workflow tests",
  );
  const archivePath =
    "/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.tar";
  const archiveSizeBytes = 512;
  const archiveSha256Hex = "b".repeat(64);
  const destinationRoot = `/var/lib/vpsman/restores/${backupId}/agent-fra-02`;

  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Choose restore artifact" }));
  const restoreWorkflow = page.getByLabel("Choose restore artifact");

  await restoreWorkflow
    .getByLabel("Restore source backup request")
    .selectOption(backupId);
  await chooseVpsBySearch(
    restoreWorkflow,
    "Restore target client",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await restoreWorkflow.getByLabel("Restore note").fill("restore-a");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review draft restore" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm draft restore"),
  ).toBeVisible();
  await restoreWorkflow.getByLabel("Restore note").fill("restore-b");
  await expect(
    restoreWorkflow.getByLabel("Confirm draft restore"),
  ).toBeHidden();
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review draft restore" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm draft restore"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm draft restore")
      .getByRole("button", { name: "Save draft restore" }),
  );

  const restorePlanRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { restorePlans: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.restorePlans.at(-1);
  });
  expect(restorePlanRequest).toMatchObject({
    destination_root: destinationRoot,
    note: "restore-b",
    source_backup_request_id: backupId,
    target_client_id: "agent-fra-02",
  });

  const stagedArchive = restoreWorkflow.getByLabel("Staged archive");
  await expect(stagedArchive).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await expect(stagedArchive).toHaveAttribute("title", archivePath);
  const dryRunToggle = restoreWorkflow.getByLabel("Dry-run rehearsal");
  await expect(dryRunToggle).toBeChecked();
  await expect(
    restoreWorkflow.getByRole("button", { name: "Review dry run" }),
  ).not.toHaveClass(/dangerPrimary/);
  await dryRunToggle.setChecked(false);
  await expect(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  ).toHaveClass(/dangerPrimary/);
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("120");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  );
  await expect(restoreWorkflow.getByLabel("Confirm restore")).toBeVisible();
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("45");
  await expect(restoreWorkflow.getByLabel("Confirm restore")).toBeHidden();
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  );
  await expect(restoreWorkflow.getByLabel("Confirm restore")).toBeVisible();
  const restoreRunConfirmation = restoreWorkflow.getByLabel("Confirm restore");
  await expect(
    restoreRunConfirmation.locator("dd", { hasText: archivePath }),
  ).toHaveAttribute("title", archivePath);
  await expect(
    restoreRunConfirmation.locator("dd", {
      hasText: archiveSha256Hex.slice(0, 12),
    }),
  ).toHaveAttribute("title", archiveSha256Hex);
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore")
      .getByRole("button", { name: "Run restore" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "restore",
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
    max_timeout_secs: 45,
    operation: {
      archive_path: archivePath,
      archive_sha256_hex: archiveSha256Hex,
      archive_size_bytes: archiveSizeBytes,
      archive_transfer_session_id: "50505050-2222-4333-8444-555555555555",
      destination_root: destinationRoot,
      source_backup_request_id: backupId,
      type: "restore",
    },
  });
});

test("backup restore async review preparation ignores stale edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup restore async review consistency is covered in desktop workflow tests",
  );
  const archivePath =
    "/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.tar";
  const destinationRoot = `/var/lib/vpsman/restores/${backupId}/agent-fra-02`;

  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Choose restore artifact" }));
  const restoreWorkflow = page.getByLabel("Choose restore artifact");

  await restoreWorkflow
    .getByLabel("Restore source backup request")
    .selectOption(backupId);
  await chooseVpsBySearch(
    restoreWorkflow,
    "Restore target client",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await restoreWorkflow.getByLabel("Restore note").fill("restore-stale-a");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review draft restore" }),
  );
  await expect(page.getByText("Preparing draft restore review")).toBeVisible();
  await restoreWorkflow.getByLabel("Restore note").fill("restore-stale-b");
  await expect(page.getByText("Preparing draft restore review")).toBeHidden();
  await expect(
    restoreWorkflow.getByLabel("Confirm draft restore"),
  ).toBeHidden();

  await activate(
    restoreWorkflow.getByRole("button", { name: "Review draft restore" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm draft restore"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm draft restore")
      .getByRole("button", { name: "Save draft restore" }),
  );

  await expect(restoreWorkflow.getByLabel("Staged archive")).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  const dryRunToggle = restoreWorkflow.getByLabel("Dry-run rehearsal");
  await expect(dryRunToggle).toBeChecked();
  await dryRunToggle.setChecked(false);
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("150");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  );
  await expect(page.getByText("Preparing restore run review")).toBeVisible();
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("55");
  await expect(page.getByText("Preparing restore run review")).toBeHidden();
  await expect(restoreWorkflow.getByLabel("Confirm restore")).toBeHidden();

  await activate(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  );
  await expect(restoreWorkflow.getByLabel("Confirm restore")).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore")
      .getByRole("button", { name: "Run restore" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "restore",
    max_timeout_secs: 55,
    operation: {
      archive_path: archivePath,
      destination_root: destinationRoot,
      type: "restore",
    },
  });
});
