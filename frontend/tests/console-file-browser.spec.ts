import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function unlockPrivilege(page: Page, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Jobs", subpage);
}

test("browses a VPS filesystem and saves a highlighted text file", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "file browser is a dense desktop operations panel");

  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.fileBrowser.state"));
  await openConsoleSubpage(page, "Jobs", "Files");
  await expect(page.getByRole("heading", { name: "File browser", exact: true })).toBeVisible();
  await unlockPrivilege(page, "Files");

  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByRole("button", { name: /etc dir/ })).toBeVisible();
  await page.getByRole("button", { name: /app\.conf/ }).dblclick();
  await expect(page.locator(".codeMirrorShell")).toContainText("listen=443");

  const editor = page.locator(".cm-content").first();
  await editor.click();
  await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
  await page.keyboard.type("listen=8443\n");
  await activate(page.getByRole("button", { name: "Review save", exact: true }));
  await expect(page.getByText("Save file")).toBeVisible();
  await activate(page.getByRole("button", { name: "Confirm" }));

  await expect(page.getByText("Save /etc/app.conf completed", { exact: true })).toBeVisible();
  await page.locator(".fileDetailsToolbar .iconButton").nth(1).click();
  await page.getByLabel("Single file upload").setInputFiles({
    name: "upload.conf",
    mimeType: "text/plain",
    buffer: Buffer.from("listen=9443\n"),
  });
  await activate(page.getByRole("button", { name: "Review upload", exact: true }));
  await expect(page.locator(".confirmationPrompt")).toContainText("Upload /upload.conf on edge-sfo-01_agent-sf");
  await expect(page.locator(".confirmationPrompt")).toContainText("Existing file");
  await expect(page.locator(".confirmationPrompt")).toContainText("skip");
  await activate(page.getByRole("button", { name: "Confirm" }));

  await activate(page.getByTitle("Create file or folder"));
  await page.locator(".fileCommandPopover").getByLabel("Name").fill("new.conf");
  await expect(page.locator(".fileCommandPopover").getByLabel("Type")).toHaveValue("file");
  await expect(page.locator(".fileCommandPopover").getByRole("button", { name: "Review write" })).toBeVisible();
  await page.getByLabel("New file text content").fill("listen=9443\n");
  await activate(page.locator(".fileCommandPopover").getByRole("button", { name: "Review write" }));
  await expect(page.locator(".confirmationPrompt").getByText("Write text", { exact: true })).toBeVisible();
  await expect(page.locator(".confirmationPrompt")).toContainText("Write text /new.conf on edge-sfo-01_agent-sf");
  await expect(page.locator(".confirmationPrompt")).toContainText("Policy");
  await activate(page.getByRole("button", { name: "Confirm" }));

  await page.locator(".fileCommandPopover").getByLabel("Name").fill("conf.d");
  await page.locator(".fileCommandPopover").getByLabel("Type").selectOption("directory");
  await expect(page.locator(".fileCommandPopover").getByLabel("Mode")).toHaveValue("0755");
  await expect(page.getByLabel("New file text content")).toHaveCount(0);
  await page.locator(".fileCommandPopover").getByLabel("Create parents").check();
  await activate(page.locator(".fileCommandPopover").getByRole("button", { name: "Review create" }));
  await expect(page.locator(".confirmationPrompt").getByText("Create folder", { exact: true })).toBeVisible();
  await expect(page.locator(".confirmationPrompt")).toContainText("Create folder /conf.d on edge-sfo-01_agent-sf");
  await expect(page.locator(".confirmationPrompt")).toContainText("Recursive");
  await expect(page.locator(".confirmationPrompt")).toContainText("yes");
  await activate(page.getByRole("button", { name: "Confirm" }));

  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileBrowserJobs);
  expect(requests.some((request: any) => request.operation?.type === "file_list_dir")).toBe(true);
  const save = requests.find((request: any) => request.operation?.type === "file_write_text");
  expect(save.operation.path).toBe("/etc/app.conf");
  expect(save.operation.expected_sha256_hex).toMatch(/^[a-f0-9]{64}$/);
  const upload = requests.find((request: any) => request.operation?.type === "file_push");
  expect(upload.operation.path).toBe("/upload.conf");
  expect(upload.operation.existing_policy).toBe("skip");
  const createdFile = requests.find((request: any) => request.operation?.type === "file_write_text" && request.operation?.path === "/new.conf");
  expect(createdFile.operation.create).toBe(true);
  expect(createdFile.operation.size_bytes).toBe(12);
  const createdFolder = requests.find((request: any) => request.operation?.type === "file_mkdir" && request.operation?.path === "/conf.d");
  expect(createdFolder.operation.mode).toBe(0o755);
  expect(createdFolder.operation.recursive).toBe(true);
});

test("runs bulk file download and upload workflows with grouped summaries", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "multi-file workflow is covered in desktop layout");

  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.multiFile.selectorExpression"));
  await openConsoleSubpage(page, "Jobs", "Multi files");
  await expect(page.getByRole("heading", { name: "Multi files" })).toBeVisible();
  await unlockPrivilege(page, "Multi files");

  await activate(page.getByRole("button", { name: "Review targets" }));
  await expect(page.getByText("3 VPSs resolved")).toBeVisible();
  await page.getByLabel("Bulk file path").fill("/etc/app.conf");
  await activate(page.getByRole("button", { name: "Review download" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Run bulk action" }));

  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "2 VPSs" })).toBeVisible();
  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "1 VPS" }).filter({ hasText: "stale" })).toBeVisible();
  await expect(page.getByLabel("Execution result").getByText("partial success: 2 done, 1 failed", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Download Archive" })).toHaveCount(1);

  await activate(page.getByRole("button", { name: "Upload files" }));
  await page.getByLabel("Bulk file destination path").fill("/etc/app.conf");
  await page.getByLabel("Bulk upload file").setInputFiles({
    name: "app.conf",
    mimeType: "text/plain",
    buffer: Buffer.from("listen=9443\n"),
  });
  await page.getByLabel("Existing file").selectOption("skip");
  await activate(page.getByRole("button", { name: "Review upload" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  await activate(page.getByRole("button", { name: "Run bulk action" }));

  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "2 VPSs" })).toBeVisible();
  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "1 VPS" }).filter({ hasText: "stale" })).toBeVisible();
  await expect(page.getByText("skip existing")).toBeVisible();
  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileBrowserJobs);
  const read = requests.find((request: any) => request.operation?.type === "file_download");
  expect(read.selector_expression).toBe("id:*");
  const upload = requests.find((request: any) => request.operation?.type === "file_push");
  expect(upload.selector_expression).toBe("id:*");
  expect(upload.operation.existing_policy).toBe("skip");
  expect(upload.operation.size_bytes).toBe(12);

  await page.locator(".advancedFileActions").evaluate((element) => {
    (element as HTMLDetailsElement).open = true;
  });
  const advancedAction = page.getByLabel("Action");
  await expect.poll(async () => advancedAction.locator("option").allTextContents()).toEqual([
    "Choose action",
    "Copy",
    "Move",
    "Delete path",
    "Chmod",
    "Chown",
    "Create folder",
    "Write text",
  ]);
  await expect(page.getByText("Delete is intentionally not primary")).toHaveCount(0);

  await advancedAction.selectOption("copy");
  await expect(page.getByLabel("Destination path")).toBeVisible();
  await expect(page.getByLabel("Policy")).toBeVisible();
  await expect(page.getByRole("button", { name: "Review copy" })).toHaveClass(/secondaryAction/);

  await advancedAction.selectOption("delete");
  await page.getByLabel("Recursive").check();
  await expect(page.getByRole("button", { name: "Review delete" })).toHaveClass(/dangerAction/);
  await activate(page.getByRole("button", { name: "Review delete" }));
  const confirmation = page.locator(".confirmationPrompt.danger");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Delete /etc/app.conf on 3 VPSs. Policy: fail.");
  await expect(confirmation).toContainText("Selector");
  await expect(confirmation).toContainText("id:*");
  await expect(confirmation).toContainText("Targets");
  await expect(confirmation).toContainText("3");
  await expect(confirmation).toContainText("Path");
  await expect(confirmation).toContainText("/etc/app.conf");
  await expect(confirmation).toContainText("Recursive");
  await expect(confirmation).toContainText("yes");
  await expect(confirmation).toContainText("Policy");
  await expect(page.getByRole("button", { name: "Delete path" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Cancel" }));
});
