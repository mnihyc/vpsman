import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  backupId,
  installConsoleApiMock,
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

async function chooseVpsBySearch(
  root: Locator,
  label: string,
  query: string,
  optionName: RegExp,
) {
  const combobox = root.getByRole("combobox", { name: label });
  await combobox.fill(query);
  const option = root.getByRole("option", { name: optionName });
  await expect(option).toBeVisible();
  const selectedLabel = (await option.locator("strong").innerText()).trim();
  await option.click();
  await expect(combobox).toHaveValue(selectedLabel);
}

async function includeBulkTagReviewTargets(page: Page) {
  const includeReviewTargets = page.getByLabel("Include targets needing review");
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

async function openSourceTemplateWorkflow(
  page: Page,
  tab: "Assign" | "Render" = "Assign",
) {
  const panel = page.locator(".sourceTemplatePanel");
  const row = panel
    .locator(".gridBody [role=row]", { hasText: "shared:vnstat-json" })
    .first();
  await expect(row).toBeVisible();
  await row.click();
  const drawer = page.getByRole("complementary", {
    name: "shared:vnstat-json",
  });
  await expect(drawer).toBeVisible();
  await activate(drawer.getByRole("tab", { name: tab }));
  return drawer;
}

async function unlockPrivilege(page: Page, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Jobs", subpage);
}

async function unlockPrivilegeFor(page: Page, view: string, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, view, subpage);
}

function expectPrivilegeAssertion(request: unknown) {
  expect((request as { envelope?: unknown }).envelope).toBeUndefined();
  expect((request as { envelopes?: unknown }).envelopes).toBeUndefined();
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
}

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
  await activate(page.locator(".commandComposer").getByRole("button", { name: "Dispatch", exact: true }));
  await expect(page.getByText("Confirm job dispatch")).toBeVisible();
  await page.getByLabel("Command argv").fill("/usr/bin/id");
  await expect(page.getByText("Confirm job dispatch")).toBeHidden();
  await activate(page.locator(".commandComposer").getByRole("button", { name: "Dispatch", exact: true }));
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
    .getByRole("searchbox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Preview changes" }));
  await expect(panel.getByText("1 VPS resolved")).toBeVisible();
  await panel
    .getByRole("searchbox", { name: "Bulk patch target expression" })
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

  await page.getByLabel("Bulk tag", { exact: true }).fill("maintenance:test");
  await page
    .getByRole("searchbox", { name: "Bulk tag selector expression" })
    .fill("id:agent-sfo-01");
  await includeBulkTagReviewTargets(page);
  await reviewBulkTagMutation(page);
  await expect(page.locator(".bulkTagPreview")).toContainText("edge-sfo-01");
  await expect(page.getByText("Confirm tag mutation")).toBeVisible();
  await page
    .getByRole("searchbox", { name: "Bulk tag selector expression" })
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
  await activate(page.locator(".commandComposer").getByRole("button", { name: "Dispatch", exact: true }));
  await expect(page.getByText("Preparing dispatch confirmation")).toBeVisible();
  await page.getByLabel("Command argv").fill("/usr/bin/id");
  await expect(page.getByText("Preparing dispatch confirmation")).toBeHidden();
  await expect(page.getByText("Confirm job dispatch")).toBeHidden();

  await activate(page.locator(".commandComposer").getByRole("button", { name: "Dispatch", exact: true }));
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

  await page.getByLabel("Bulk tag", { exact: true }).fill("maintenance:test");
  const selector = page.getByRole("searchbox", {
    name: "Bulk tag selector expression",
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
    "1 artifacts / 22 B",
  );
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
  const policySelector = page.getByRole("searchbox", {
    name: "Backup policy target expression",
  });
  await policySelector.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("id:agent-fra-02");
  await page.keyboard.press("Escape");
  await activate(page.getByRole("button", { name: "Review policy" }));
  await expect(page.getByText("Confirm backup policy")).toBeVisible();
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

  await activate(
    page.getByRole("button", { name: "Open backup request", exact: true }),
  );
  const requestWorkflow = page.getByLabel("Open backup request");
  await chooseVpsBySearch(
    requestWorkflow,
    "Backup client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(
    requestWorkflow.getByRole("button", { name: "Review backup" }),
  );
  await expect(
    requestWorkflow.getByLabel("Confirm backup request"),
  ).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Policies");
  await expect(page.getByLabel("Confirm backup request")).toBeHidden();
});

test("template render preview follows the selected VPS without submitting apply jobs", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "template render consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Source templates");
  await unlockPrivilegeFor(page, "Automation", "Source templates");

  const panel = await openSourceTemplateWorkflow(page, "Render");
  await chooseVpsBySearch(
    panel,
    "Template runtime config preview VPS",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(panel.getByRole("button", { name: "Render config" }));
  await expect(
    panel.getByLabel("Rendered template runtime config TOML"),
  ).toHaveValue(/agent-sfo-01/);
  await chooseVpsBySearch(
    panel,
    "Template runtime config preview VPS",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await activate(panel.getByRole("button", { name: "Render config" }));
  await expect(
    panel.getByLabel("Rendered template runtime config TOML"),
  ).toHaveValue(/agent-fra-02/);

  const jobCount = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.length;
  });
  expect(jobCount).toBe(0);
  await expect(panel.getByRole("button", { name: "Apply patch" })).toHaveCount(
    0,
  );
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
  const selector = panel.getByRole("searchbox", {
    name: "Bulk patch target expression",
  });
  await selector.fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Preview changes" }));
  await expect(page.getByText("Previewing bulk patch changes")).toBeVisible();
  await selector.fill("id:agent-fra-02");
  await expect(page.getByText("Previewing bulk patch changes")).toBeHidden();
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

test("template assignment async review ignores stale selector edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "template assignment async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Source templates");
  await unlockPrivilegeFor(page, "Automation", "Source templates");

  const panel = await openSourceTemplateWorkflow(page, "Assign");
  const selector = panel.getByRole("searchbox", {
    name: "Template assignment target expression",
  });
  await selector.fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Review assignment" }));
  await expect(
    page.getByText("Preparing template assignment review"),
  ).toBeVisible();
  await selector.fill("id:agent-fra-02");
  await expect(
    page.getByText("Preparing template assignment review"),
  ).toBeHidden();
  await expect(
    page.getByRole("region", { name: "Confirm template assignment" }),
  ).toBeHidden();

  await activate(panel.getByRole("button", { name: "Review assignment" }));
  await expect(
    page.getByRole("region", { name: "Confirm template assignment" }),
  ).toBeVisible();
  await activate(
    page.getByRole("region", { name: "Confirm template assignment" }).getByRole("button", {
      name: "Apply template assignment",
      exact: true,
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { sourceTemplateAssignments: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.sourceTemplateAssignments.at(-1);
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
  await page.getByRole("menuitem", { name: "Prepare revoke" }).click();
  await expect(
    inspector.getByRole("heading", { name: "Revoke VPS key" }),
  ).toBeVisible();
  await expect(
    inspector.getByRole("combobox", { name: "VPS identity revoke VPS ID" }),
  ).toHaveValue(/edge-sfo-01/);
  await inspector.getByLabel("VPS identity revoke reason").fill("reason-a");
  await activate(inspector.getByRole("button", { name: "Revoke current key" }));
  await expect(inspector.getByText("Preparing review")).toBeVisible();
  await inspector.getByLabel("VPS identity revoke reason").fill("reason-b");
  await expect(inspector.getByText("Preparing review")).toBeHidden();
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
      server_side: "left",
      type: "network_speed_test",
    },
  });

  await openConsoleSubpage(page, "Network", "OSPF");
  await activate(page.getByRole("button", { name: "Apply cost" }));
  await expect(page.getByText("Confirm OSPF cost update")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Update cost",
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
      mutation_intent: "apply",
      recommendation_id: "ospf-1234abcd5678ef90",
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

test("OSPF cost update and rollback submit reviewed server-side plan mutations", async ({
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

  await expect(
    page.getByRole("button", { name: "Rollback cost" }),
  ).toBeDisabled();
  await activate(page.getByRole("button", { name: "Apply cost" }));
  const applyPrompt = page.locator(".confirmationPrompt").last();
  await expect(applyPrompt).toContainText("Confirm OSPF cost update");
  await expect(applyPrompt).toContainText("Apply recommended cost");
  await expect(applyPrompt).toContainText("Recommendation ID");
  await expect(applyPrompt).toContainText("14 -> 22 (+8)");
  await expect(applyPrompt).toContainText("network.ospf_cost.apply");
  await expect(applyPrompt).toContainText(
    "client:agent-sfo-01, client:agent-fra-02",
  );
  await activate(
    applyPrompt.getByRole("button", {
      name: "Update cost",
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
      mutation_intent: "apply",
      recommendation_id: "ospf-1234abcd5678ef90",
      current_ospf_cost: 14,
      recommended_ospf_cost: 22,
    },
  });
  expectPrivilegeAssertion((request as { body: unknown }).body);

  await activate(page.getByRole("button", { name: "Rollback cost" }));
  const rollbackPrompt = page.locator(".confirmationPrompt").last();
  await expect(rollbackPrompt).toContainText("Confirm OSPF rollback");
  await expect(rollbackPrompt).toContainText("Rollback applied recommendation");
  await expect(rollbackPrompt).toContainText("22 -> 14 (-8)");
  await expect(rollbackPrompt).toContainText("network.ospf_cost.rollback");
  await expect(rollbackPrompt).toContainText(
    "client:agent-sfo-01, client:agent-fra-02",
  );
  await activate(
    rollbackPrompt.getByRole("button", {
      name: "Rollback cost",
    }),
  );

  const rollbackRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          tunnelPlanOspfCostUpdates: Array<{ plan_id: string; body: unknown }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanOspfCostUpdates.at(-1);
  });
  expect(rollbackRequest).toMatchObject({
    body: {
      confirmed: true,
      mutation_intent: "rollback",
      recommendation_id: "ospf-1234abcd5678ef90",
      current_ospf_cost: 22,
      recommended_ospf_cost: 14,
    },
  });
  expectPrivilegeAssertion((rollbackRequest as { body: unknown }).body);
});

test("custom adapter submits a fresh snapshot after reopening review", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "custom adapter consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Promotion workflow" }));

  const promotionPanel = page.getByLabel("Tunnel plan promotion workflow");
  const adapterForm = promotionPanel.locator("form", {
    has: page.getByRole("heading", { name: "Custom adapter" }),
  });
  await expect(
    promotionPanel.getByText("Promotion diff workflow"),
  ).toBeVisible();
  await activate(
    promotionPanel.getByText("Advanced: custom adapter promotion"),
  );
  for (const argvLabel of [
    "Status argv",
    "Start argv",
    "Restart argv",
    "Stop argv",
    "Cleanup argv",
    "Traffic argv",
  ]) {
    await expect(
      adapterForm.getByLabel(argvLabel, { exact: true }),
    ).toHaveAttribute("title", /Command and arguments executed by the adapter/);
  }
  await adapterForm
    .getByLabel("Observed plan")
    .selectOption("eeeeeeee-ffff-4000-8111-222222222222");
  const statusArgv = adapterForm.getByLabel("Status argv", { exact: true });
  await statusArgv.fill(
    "/usr/local/libexec/vpsman-openvpn-adapter\nstatus-a\n{interface}",
  );
  await activate(
    adapterForm.getByRole("button", { name: "Review custom adapter" }),
  );
  const promotionConfirmation = promotionPanel.locator(".confirmationPrompt", {
    hasText: "Confirm custom adapter",
  });
  await expect(promotionConfirmation).toBeVisible();
  await expect(
    promotionConfirmation.locator("dd", { hasText: "status-a" }),
  ).toHaveAttribute("title", /status-a/);
  await activate(
    promotionConfirmation.getByRole("button", { name: "Close confirmation" }),
  );
  await expect(promotionConfirmation).toBeHidden();
  await statusArgv.fill(
    "/usr/local/libexec/vpsman-openvpn-adapter\nstatus-b\n{interface}",
  );
  await activate(
    adapterForm.getByRole("button", { name: "Review custom adapter" }),
  );
  await expect(promotionConfirmation).toBeVisible();
  await activate(
    promotionConfirmation.getByRole("button", {
      name: "Save custom adapter",
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { tunnelPlanAdapterPromotions: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanAdapterPromotions.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    plan_id: "eeeeeeee-ffff-4000-8111-222222222222",
    runtime_control: {
      status: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "status-b",
          "{interface}",
        ],
      },
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
  const restoreRunConfirmation = restoreWorkflow.getByLabel(
    "Confirm restore",
  );
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
  await expect(restoreWorkflow.getByLabel("Confirm draft restore")).toBeHidden();

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
