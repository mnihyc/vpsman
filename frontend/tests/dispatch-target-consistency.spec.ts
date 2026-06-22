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
  await root.getByRole("combobox", { name: label }).fill(query);
  const option = root.getByRole("option", { name: optionName });
  await expect(option).toBeVisible();
  await option.click();
}

async function unlockPrivilege(page: Page, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Jobs", subpage);
}

async function unlockPrivilegeFor(page: Page, view: string, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, view, subpage);
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
  await activate(page.getByRole("button", { name: "Review dispatch" }));
  await expect(page.getByText("Confirm job dispatch")).toBeVisible();
  await page.getByLabel("Command argv").fill("/usr/bin/id");
  await expect(page.getByText("Confirm job dispatch")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review dispatch" }));
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

test("multi-file review resolves targets again instead of executing cached preview", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "multi-file consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.multiFile.selectorExpression"),
  );
  await openConsoleSubpage(page, "Jobs", "Multi files");
  await unlockPrivilege(page, "Multi files");

  await activate(page.getByRole("button", { name: "Review targets" }));
  await expect(page.getByText("3 VPSs resolved")).toBeVisible();
  const resolveCountAfterPreview = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { bulkResolve: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.bulkResolve.length;
  });
  expect(resolveCountAfterPreview).toBe(1);

  await page.getByLabel("Bulk file path").fill("/etc/app.conf");
  await activate(page.getByRole("button", { name: "Review download" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Close confirmation" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review download" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  await page.getByLabel("Bulk file path").fill("/etc/app.conf.d/current");
  await expect(page.getByText("Confirm multi-file operation")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review download" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  const resolveCountAfterReview = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { bulkResolve: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.bulkResolve.length;
  });
  expect(resolveCountAfterReview).toBe(4);

  await activate(page.getByRole("button", { name: "Run bulk action" }));

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

test("multi-file async review preparation ignores stale path edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "multi-file async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.multiFile.selectorExpression"),
  );
  await openConsoleSubpage(page, "Jobs", "Multi files");
  await unlockPrivilege(page, "Multi files");

  await page.getByLabel("Bulk file path").fill("/etc/app.conf");
  await activate(page.getByRole("button", { name: "Review download" }));
  await expect(page.getByText("Preparing bulk file review")).toBeVisible();
  await page.getByLabel("Bulk file path").fill("/etc/app.conf.next");
  await expect(page.getByText("Preparing bulk file review")).toBeHidden();
  await expect(page.getByText("Confirm multi-file operation")).toBeHidden();

  await activate(page.getByRole("button", { name: "Review download" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Run bulk action" }));

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
  await openConsoleSubpage(page, "Config", "Bulk apply");
  await unlockPrivilegeFor(page, "Config", "Bulk apply");

  const panel = page.locator(".configApplyGrid");
  await panel
    .getByRole("searchbox", { name: "Bulk config selector expression" })
    .fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Review targets" }));
  await expect(panel.getByText("1/3")).toBeVisible();
  await panel
    .getByRole("searchbox", { name: "Bulk config selector expression" })
    .fill("id:agent-fra-02");
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm bulk config apply")).toBeVisible();
  await activate(page.getByRole("button", { name: "Apply config patch" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "data_source_config_patch",
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
  await openConsoleSubpage(page, "Tags", "Bulk");
  await unlockPrivilegeFor(page, "Tags", "Bulk");

  await page.getByLabel("Bulk tag", { exact: true }).fill("maintenance:test");
  await page
    .getByRole("searchbox", { name: "Bulk tag selector expression" })
    .fill("id:agent-sfo-01");
  await activate(page.getByRole("button", { name: "Preview targets" }));
  await expect(page.locator(".bulkTagPreview")).toContainText("edge-sfo-01");
  await page
    .getByRole("searchbox", { name: "Bulk tag selector expression" })
    .fill("id:agent-fra-02");
  await expect(page.getByRole("button", { name: "Review mutation" })).toBeDisabled();
  await activate(page.getByRole("button", { name: "Preview targets" }));
  await activate(page.getByRole("button", { name: "Review mutation" }));
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
  await activate(page.getByRole("button", { name: "Review dispatch" }));
  await expect(page.getByText("Preparing dispatch review")).toBeVisible();
  await page.getByLabel("Command argv").fill("/usr/bin/id");
  await expect(page.getByText("Preparing dispatch review")).toBeHidden();
  await expect(page.getByText("Confirm job dispatch")).toBeHidden();

  await activate(page.getByRole("button", { name: "Review dispatch" }));
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
  await openConsoleSubpage(page, "Tags", "Bulk");
  await unlockPrivilegeFor(page, "Tags", "Bulk");

  await page.getByLabel("Bulk tag", { exact: true }).fill("maintenance:test");
  const selector = page.getByRole("searchbox", {
    name: "Bulk tag selector expression",
  });
  await selector.fill("id:agent-sfo-01");
  await activate(page.getByRole("button", { name: "Preview targets" }));
  await expect(page.getByText("Preparing tag preview")).toBeVisible();
  await selector.fill("id:agent-fra-02");
  await expect(page.getByText("Preparing tag preview")).toBeHidden();
  await expect(page.locator(".bulkTagPreview")).toHaveCount(0);

  await activate(page.getByRole("button", { name: "Preview targets" }));
  await expect(page.locator(".bulkTagPreview")).toContainText("core-fra-02");
  await activate(page.getByRole("button", { name: "Review mutation" }));
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
  await openConsoleSubpage(page, "Jobs", "Server jobs");

  const cleanupPanel = page.locator(".fleetPanel", {
    has: page.getByRole("heading", { name: "Artifact cleanup" }),
  });
  const expression = cleanupPanel.getByLabel("Expression");
  await expression.fill('artifact.domain = "job_output"');
  await activate(cleanupPanel.getByRole("button", { name: "Preview" }));
  await expect(page.getByText("Preparing cleanup preview")).toBeVisible();
  await expression.fill('artifact.domain = "file_transfer_source"');
  await expect(page.getByText("Preparing cleanup preview")).toBeHidden();
  await expect(cleanupPanel.getByLabel("Preview hash")).toHaveValue("");

  await activate(cleanupPanel.getByRole("button", { name: "Preview" }));
  await expect(cleanupPanel.getByLabel("Preview hash")).toHaveValue(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  );
  await activate(cleanupPanel.getByRole("button", { name: "Queue cleanup" }));
  await expect(page.getByText("Confirm artifact cleanup")).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Queue cleanup" }),
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
    expression: 'artifact.domain = "file_transfer_source"',
    preview_hash:
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  });
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

  await activate(page.getByRole("button", { name: "Open policy workflow" }));
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

  await activate(page.getByRole("button", { name: "Open backup request", exact: true }));
  const requestWorkflow = page.getByLabel("Open backup request");
  await chooseVpsBySearch(
    requestWorkflow,
    "Backup client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(requestWorkflow.getByRole("button", { name: "Review backup" }));
  await expect(requestWorkflow.getByLabel("Confirm backup request")).toBeVisible();

  await openConsoleSubpage(page, "Backups", "Policies");
  await expect(page.getByLabel("Confirm backup request")).toBeHidden();
});

test("data-source apply confirmation closes on edit and submits a fresh snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "data-source apply consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Templates");
  await unlockPrivilegeFor(page, "Config", "Templates");

  const panel = page.locator(".dataSourcePresetPanel");
  await chooseVpsBySearch(
    panel,
    "Hot-config preview VPS",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(panel.getByRole("button", { name: "Render config" }));
  await expect(
    panel.getByLabel("Rendered data-source config patch TOML"),
  ).toHaveValue(/agent-sfo-01/);
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(panel.getByText("Apply data-source patch")).toBeVisible();
  await panel.getByLabel("Data-source apply max timeout seconds").fill("75");
  await expect(panel.getByText("Apply data-source patch")).toBeHidden();
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(panel.getByText("Apply data-source patch")).toBeVisible();
  await activate(
    panel.locator(".confirmationPrompt").getByRole("button", {
      name: "Confirm",
      exact: true,
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "data_source_config_patch",
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    max_timeout_secs: 75,
    operation: {
      apply_mode: "incremental_patch",
      type: "data_source_config_patch",
    },
  });
  expect(
    (request as { operation: { toml: string } }).operation.toml,
  ).toContain('client_id = "agent-sfo-01"');
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
  await openConsoleSubpage(page, "Config", "Bulk apply");
  await unlockPrivilegeFor(page, "Config", "Bulk apply");

  const panel = page.locator(".configApplyGrid");
  const selector = panel.getByRole("searchbox", {
    name: "Bulk config selector expression",
  });
  await selector.fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Preparing bulk config review")).toBeVisible();
  await selector.fill("id:agent-fra-02");
  await expect(page.getByText("Preparing bulk config review")).toBeHidden();
  await expect(page.getByText("Confirm bulk config apply")).toBeHidden();

  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm bulk config apply")).toBeVisible();
  await activate(page.getByRole("button", { name: "Apply config patch" }));

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "data_source_config_patch",
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("data-source assignment async review ignores stale selector edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "data-source assignment async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Templates");
  await unlockPrivilegeFor(page, "Config", "Templates");

  const panel = page.locator(".dataSourcePresetPanel");
  const selector = panel.getByRole("searchbox", {
    name: "Data-source assignment target expression",
  });
  await selector.fill("id:agent-sfo-01");
  await activate(panel.getByRole("button", { name: "Review assignment" }));
  await expect(
    page.getByText("Preparing data-source assignment review"),
  ).toBeVisible();
  await selector.fill("id:agent-fra-02");
  await expect(
    page.getByText("Preparing data-source assignment review"),
  ).toBeHidden();
  await expect(panel.getByText("Assign data-source preset")).toBeHidden();

  await activate(panel.getByRole("button", { name: "Review assignment" }));
  await expect(panel.getByText("Assign data-source preset")).toBeVisible();
  await activate(
    panel.locator(".confirmationPrompt").getByRole("button", {
      name: "Confirm",
      exact: true,
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { dataSourcePresetAssignments: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.dataSourcePresetAssignments.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("data-source apply async review ignores stale target edits", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "data-source apply async review consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Templates");
  await unlockPrivilegeFor(page, "Config", "Templates");

  const panel = page.locator(".dataSourcePresetPanel");
  await chooseVpsBySearch(
    panel,
    "Hot-config preview VPS",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(
    page.getByText("Preparing data-source apply review"),
  ).toBeVisible();
  await chooseVpsBySearch(
    panel,
    "Hot-config preview VPS",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(
    page.getByText("Preparing data-source apply review"),
  ).toBeHidden();
  await expect(panel.getByText("Apply data-source patch")).toBeHidden();

  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(panel.getByText("Apply data-source patch")).toBeVisible();
  await activate(
    panel.locator(".confirmationPrompt").getByRole("button", {
      name: "Confirm",
      exact: true,
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "data_source_config_patch",
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
  expect(
    (request as { operation: { toml: string } }).operation.toml,
  ).toContain('client_id = "agent-fra-02"');
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
  await openConsoleSubpage(page, "Access", "VPS keys");
  await unlockPrivilegeFor(page, "Access", "VPS keys");

  const inspector = page.locator(".accessInspector");
  await inspector.getByLabel("Agent identity client ID").fill("agent-tokyo-04");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("a".repeat(64));
  await inspector
    .getByLabel("Agent identity display name")
    .fill("edge-tokyo-a");
  await activate(
    inspector.getByRole("button", { name: "Import gateway identity" }),
  );
  await expect(inspector.getByText("Preparing review")).toBeVisible();
  await inspector
    .getByLabel("Agent identity display name")
    .fill("edge-tokyo-b");
  await expect(inspector.getByText("Preparing review")).toBeHidden();
  await expect(
    page.getByLabel("Confirm direct gateway identity import"),
  ).toBeHidden();

  await activate(
    inspector.getByRole("button", { name: "Import gateway identity" }),
  );
  await expect(
    page.getByLabel("Confirm direct gateway identity import"),
  ).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm direct gateway identity import")
      .getByRole("button", { name: "Import identity" }),
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

  await chooseVpsBySearch(
    inspector,
    "VPS key revoke VPS ID",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await inspector.getByLabel("VPS key revoke reason").fill("reason-a");
  await activate(inspector.getByRole("button", { name: "Revoke current key" }));
  await expect(inspector.getByText("Preparing review")).toBeVisible();
  await inspector.getByLabel("VPS key revoke reason").fill("reason-b");
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
  await fleetGrid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  await sfoRow.getByLabel("Select VPS instance records row").check();
  await page.waitForTimeout(180);
  await expect(page.getByText("Delete VPS from panel")).toBeHidden();

  await backupRow.getByLabel("Select VPS instance records row").uncheck();
  await fleetGrid.getByRole("button", { name: "Action" }).click();
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

test("topology network confirmation closes on edit and submits a fresh snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "network confirmation consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Topology", "Apply / rollback");
  await unlockPrivilegeFor(page, "Topology", "Apply / rollback");

  await page.getByLabel("Network apply max timeout seconds").fill("90");
  await activate(page.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm apply")).toBeVisible();
  await page.getByLabel("Network apply max timeout seconds").fill("120");
  await expect(page.getByText("Confirm apply")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm apply")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Apply side",
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "network_apply",
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    max_timeout_secs: 120,
    operation: {
      side: "left",
      type: "network_apply",
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
  await openConsoleSubpage(page, "Topology", "Apply / rollback");
  await unlockPrivilegeFor(page, "Topology", "Apply / rollback");

  await activate(page.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Preparing apply review")).toBeVisible();
  await page.getByLabel("Network apply max timeout seconds").fill("135");
  await expect(page.getByText("Preparing apply review")).toBeHidden();
  await expect(page.getByText("Confirm apply")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm apply")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Apply side",
    }),
  );

  const applyRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(applyRequest).toMatchObject({
    command: "network_apply",
    max_timeout_secs: 135,
  });

  await openConsoleSubpage(page, "Topology", "OSPF");
  await activate(page.getByRole("button", { name: "Review cost apply" }));
  await expect(page.getByText("Preparing OSPF review")).toBeVisible();
  await page.getByLabel("OSPF update max timeout seconds").fill("105");
  await expect(page.getByText("Preparing OSPF review")).toBeHidden();
  await expect(page.getByText("Confirm OSPF cost update")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review cost apply" }));
  await expect(page.getByText("Confirm OSPF cost update")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Apply cost",
    }),
  );

  const ospfRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(ospfRequest).toMatchObject({
    command: "network_ospf_cost_update",
    max_timeout_secs: 105,
  });
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
  await openConsoleSubpage(page, "Topology", "Apply / rollback");
  await unlockPrivilegeFor(page, "Topology", "Apply / rollback");

  await activate(page.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm apply")).toBeVisible();
  await page.clock.fastForward(301_000);
  await expect(page.getByText("Confirm apply")).toBeHidden();
});

test("OSPF confirmation closes on edit and submits a fresh snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "OSPF confirmation consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Topology", "OSPF");
  await unlockPrivilegeFor(page, "Topology", "OSPF");

  await page.getByLabel("OSPF update max timeout seconds").fill("45");
  await activate(page.getByRole("button", { name: "Review cost apply" }));
  await expect(page.getByText("Confirm OSPF cost update")).toBeVisible();
  await page.getByLabel("OSPF update max timeout seconds").fill("75");
  await expect(page.getByText("Confirm OSPF cost update")).toBeHidden();
  await activate(page.getByRole("button", { name: "Review cost apply" }));
  await expect(page.getByText("Confirm OSPF cost update")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Apply cost",
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "network_ospf_cost_update",
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    max_timeout_secs: 75,
    operation: {
      side: "left",
      type: "network_ospf_cost_update",
    },
  });
});

test("adapter promotion submits a fresh snapshot after reopening review", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "adapter promotion consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Topology", "Promotion");

  const promotionPanel = page.locator(".scheduleComposer", {
    has: page.getByRole("heading", { name: "Tunnel promotion" }),
  });
  const adapterForm = promotionPanel.locator("form", {
    has: page.getByRole("heading", { name: "Adapter contract" }),
  });
  for (const argvLabel of [
    "Status argv",
    "Start argv",
    "Restart argv",
    "Stop argv",
    "Cleanup argv",
    "Traffic argv",
  ]) {
    await expect(adapterForm.getByLabel(argvLabel, { exact: true })).toHaveAttribute(
      "title",
      /Command and arguments executed by the adapter/,
    );
  }
  await adapterForm
    .getByLabel("Observed plan")
    .selectOption("eeeeeeee-ffff-4000-8111-222222222222");
  const statusArgv = adapterForm.getByLabel("Status argv", { exact: true });
  await statusArgv.fill(
    "/usr/local/libexec/vpsman-openvpn-adapter\nstatus-a\n{interface}",
  );
  await activate(adapterForm.getByRole("button", { name: "Review promotion" }));
  const promotionConfirmation = promotionPanel.locator(".confirmationPrompt", {
    hasText: "Promote tunnel adapter",
  });
  await expect(promotionConfirmation).toBeVisible();
  await expect(promotionConfirmation.locator("dd", { hasText: "status-a" })).toHaveAttribute(
    "title",
    /status-a/,
  );
  await activate(
    promotionConfirmation.getByRole("button", { name: "Close confirmation" }),
  );
  await expect(promotionConfirmation).toBeHidden();
  await statusArgv.fill(
    "/usr/local/libexec/vpsman-openvpn-adapter\nstatus-b\n{interface}",
  );
  await activate(adapterForm.getByRole("button", { name: "Review promotion" }));
  await expect(promotionConfirmation).toBeVisible();
  await activate(
    promotionConfirmation.getByRole("button", {
      name: "Promote adapter",
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

test("single config confirmation closes on edit and submits a fresh snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "single config confirmation consistency is covered in desktop workflow tests",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Single VPS");
  await unlockPrivilegeFor(page, "Config", "Single VPS");

  const panel = page.locator(".configApplyGrid");
  await chooseVpsBySearch(
    panel,
    "Single VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await activate(panel.getByRole("button", { name: "Read config" }));
  const editor = panel.getByLabel("Single VPS redacted config TOML");
  await expect(editor).toHaveValue(/client_id = "agent-fra-02"/);
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm single-VPS config apply")).toBeVisible();
  await editor.fill(`${await editor.inputValue()}\n# reviewed edit\n`);
  await expect(page.getByText("Confirm single-VPS config apply")).toBeHidden();
  await activate(panel.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm single-VPS config apply")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", {
      name: "Apply config",
    }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "hot_config",
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
    operation: {
      apply_mode: "full_override",
      base_config_sha256_hex: "b".repeat(64),
      preserve_redacted: true,
      type: "hot_config",
    },
  });
  expect((request as { operation: { toml: string } }).operation.toml).toContain(
    "# reviewed edit",
  );
});

test("backup restore confirmations close on edit and submit fresh snapshots", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup restore consistency is covered in desktop workflow tests",
  );
  const archivePath = "/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.tar";
  const archiveSizeBytes = 512;
  const archiveSha256Hex = "b".repeat(64);
  const destinationRoot = `/var/lib/vpsman/restores/${backupId}/agent-fra-02`;

  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Open restore workflow" }));
  const restoreWorkflow = page.getByLabel("Open restore workflow");

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
  await activate(restoreWorkflow.getByRole("button", { name: "Review plan" }));
  await expect(
    restoreWorkflow.getByLabel("Confirm restore plan"),
  ).toBeVisible();
  await restoreWorkflow.getByLabel("Restore note").fill("restore-b");
  await expect(
    restoreWorkflow.getByLabel("Confirm restore plan"),
  ).toBeHidden();
  await activate(restoreWorkflow.getByRole("button", { name: "Review plan" }));
  await expect(
    restoreWorkflow.getByLabel("Confirm restore plan"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore plan")
      .getByRole("button", { name: "Create restore plan" }),
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
    restoreWorkflow.getByRole("button", { name: "Review restore" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm restore run"),
  ).toBeVisible();
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("45");
  await expect(
    restoreWorkflow.getByLabel("Confirm restore run"),
  ).toBeHidden();
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review restore" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm restore run"),
  ).toBeVisible();
  const restoreRunConfirmation = restoreWorkflow.getByLabel("Confirm restore run");
  await expect(
    restoreRunConfirmation.locator("dd", { hasText: archivePath }),
  ).toHaveAttribute("title", archivePath);
  await expect(
    restoreRunConfirmation.locator("dd", { hasText: archiveSha256Hex.slice(0, 12) }),
  ).toHaveAttribute("title", archiveSha256Hex);
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore run")
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
  const archivePath = "/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.tar";
  const destinationRoot = `/var/lib/vpsman/restores/${backupId}/agent-fra-02`;

  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Open restore workflow" }));
  const restoreWorkflow = page.getByLabel("Open restore workflow");

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
  await activate(restoreWorkflow.getByRole("button", { name: "Review plan" }));
  await expect(page.getByText("Preparing restore plan review")).toBeVisible();
  await restoreWorkflow.getByLabel("Restore note").fill("restore-stale-b");
  await expect(page.getByText("Preparing restore plan review")).toBeHidden();
  await expect(
    restoreWorkflow.getByLabel("Confirm restore plan"),
  ).toBeHidden();

  await activate(restoreWorkflow.getByRole("button", { name: "Review plan" }));
  await expect(
    restoreWorkflow.getByLabel("Confirm restore plan"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore plan")
      .getByRole("button", { name: "Create restore plan" }),
  );

  await expect(restoreWorkflow.getByLabel("Staged archive")).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("150");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review restore" }),
  );
  await expect(page.getByText("Preparing restore run review")).toBeVisible();
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("55");
  await expect(page.getByText("Preparing restore run review")).toBeHidden();
  await expect(
    restoreWorkflow.getByLabel("Confirm restore run"),
  ).toBeHidden();

  await activate(
    restoreWorkflow.getByRole("button", { name: "Review restore" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm restore run"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore run")
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
