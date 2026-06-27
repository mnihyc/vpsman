import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await expect(locator).toBeVisible();
  await expect(locator).toBeEnabled();
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function unlockPrivilege(page: Page, subpage: string) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", subpage);
}

test("browses a VPS filesystem and saves a highlighted text file", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "file browser is a dense desktop operations panel");

  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.fileBrowser.state"));
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(page.getByRole("heading", { name: "File browser", exact: true })).toBeVisible();
  await expect(page.getByText("Select a VPS and file to begin.")).toBeVisible();
  await expect(page.locator(".codeMirrorShell")).toHaveCount(0);
  await unlockPrivilege(page, "Files");
  const targetPicker = page.getByRole("combobox", { name: "File browser target VPS" });
  await expect(targetPicker).toHaveValue("edge-sfo-01 (fo01)");
  await targetPicker.fill("sfo");
  await expect(page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ })).toBeVisible();
  await page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ }).click();
  await expect(targetPicker).toHaveValue("edge-sfo-01 (fo01)");
  await targetPicker.fill("not-a-real-vps");
  await targetPicker.blur();
  await expect(targetPicker).toHaveValue("edge-sfo-01 (fo01)");

  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByRole("button", { name: /etc dir/ })).toBeVisible();
  await page.getByRole("button", { name: /app\.conf/ }).dblclick();
  await expect(page.locator(".codeMirrorShell")).toContainText("listen=443");

  const editor = page.locator(".cm-content").first();
  await editor.click();
  await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
  await page.keyboard.type("listen=8443\n");
  await activate(page.getByRole("button", { name: "Review save", exact: true }));
  const savePrompt = page.locator(".confirmationPrompt").last();
  await expect(savePrompt.getByRole("button", { name: "Save file", exact: true })).toBeVisible();
  await expect(savePrompt).toContainText("Diff");
  await expect(savePrompt.locator(".fileDiffPreview")).toContainText("+ listen=8443");
  await activate(
    savePrompt.getByRole("button", { name: "Save file", exact: true }),
  );

  await expect(page.getByText("Save /etc/app.conf completed", { exact: true })).toBeVisible();
  await activate(
    page
      .getByLabel("Selected file actions")
      .getByRole("button", { name: "Upload here" }),
  );
  await page.getByLabel("Single file upload").setInputFiles({
    name: "upload.conf",
    mimeType: "text/plain",
    buffer: Buffer.from("listen=9443\n"),
  });
  await activate(page.getByRole("button", { name: "Review upload", exact: true }));
  const uploadPrompt = page.locator(".confirmationPrompt").last();
  await expect(uploadPrompt).toContainText("Upload file on edge-sfo-01_agent-sf");
  await expect(uploadPrompt).toContainText("Existing file");
  await expect(uploadPrompt).toContainText("Skip upload if the file already exists");
  await activate(uploadPrompt.getByRole("button", { name: "Upload file", exact: true }));

  await activate(page.getByTitle("Create file or folder"));
  await page.locator(".fileCommandPopover").getByLabel("Name").fill("new.conf");
  await expect(page.locator(".fileCommandPopover").getByLabel("Type")).toHaveValue("file");
  await expect(page.locator(".fileCommandPopover").getByRole("button", { name: "Review file write" })).toBeVisible();
  await page.getByLabel("New file text content").fill("listen=9443\n");
  await activate(page.locator(".fileCommandPopover").getByRole("button", { name: "Review file write" }));
  await expect(page.locator(".confirmationPrompt").getByText("Write text", { exact: true })).toBeVisible();
  await expect(page.locator(".confirmationPrompt")).toContainText("Write text file on edge-sfo-01_agent-sf");
  await expect(page.locator(".confirmationPrompt")).toContainText("/new.conf");
  await expect(page.locator(".confirmationPrompt")).toContainText("Policy");
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Write file", exact: true }),
  );

  await page.locator(".fileCommandPopover").getByLabel("Name").fill("conf.d");
  await page.locator(".fileCommandPopover").getByLabel("Type").selectOption("directory");
  await expect(page.locator(".fileCommandPopover").getByLabel("Mode")).toHaveValue("0755");
  await expect(page.getByLabel("New file text content")).toHaveCount(0);
  await page.locator(".fileCommandPopover").getByLabel("Create parents").check();
  await activate(page.locator(".fileCommandPopover").getByRole("button", { name: "Review folder create" }));
  await expect(
    page.locator(".confirmationPrompt strong").filter({ hasText: /^Create folder$/ }),
  ).toBeVisible();
  await expect(page.locator(".confirmationPrompt")).toContainText("Create folder on edge-sfo-01_agent-sf");
  await expect(page.locator(".confirmationPrompt")).toContainText("/conf.d");
  await expect(page.locator(".confirmationPrompt")).toContainText("Recursive");
  await expect(page.locator(".confirmationPrompt")).toContainText("Include child paths");
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Create folder", exact: true }),
  );

  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileBrowserJobs);
  expect(requests.some((request: any) => request.operation?.type === "file_list_dir")).toBe(true);
  const list = requests.find((request: any) => request.operation?.type === "file_list_dir");
  expect(list.selector_expression).toBe("id:agent-sfo-01");
  const listJob = await page.evaluate(() => {
    const requests = (window as any).__vpsmanTestRequests.jobs;
    return requests.find((request: any) => request.operation?.type === "file_list_dir");
  });
  expect(listJob.target_client_ids).toEqual(["agent-sfo-01"]);
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

test("single-file operation confirmation closes on operation edits", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "file browser is a dense desktop operations panel");

  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.fileBrowser.state"));
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await unlockPrivilege(page, "Files");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await expect(page.getByRole("button", { name: /etc dir/ })).toBeVisible();

  await activate(page.getByTitle("Create file or folder"));
  const popover = page.locator(".fileCommandPopover");
  await popover.getByLabel("Name").fill("stale-a.conf");
  await activate(popover.getByRole("button", { name: "Review file write" }));
  await expect(page.locator(".confirmationPrompt").getByText("Write text", { exact: true })).toBeVisible();

  await popover.getByLabel("Name").fill("stale-b.conf");
  await expect(page.locator(".confirmationPrompt").getByText("Write text", { exact: true })).toBeHidden();
  await activate(popover.getByRole("button", { name: "Review file write" }));
  await expect(page.locator(".confirmationPrompt").getByText("Write text", { exact: true })).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Write file", exact: true }),
  );

  const createdFile = await page.evaluate(() => {
    const requests = (window as any).__vpsmanTestRequests.fileBrowserJobs;
    return requests.find(
      (request: any) =>
        request.operation?.type === "file_write_text" &&
        request.operation?.path === "/stale-b.conf",
    );
  });
  expect(createdFile).toMatchObject({
    operation: {
      create: true,
      path: "/stale-b.conf",
      type: "file_write_text",
    },
    selector_expression: "id:agent-sfo-01",
  });
});

test("mobile file browser opens text files as a focused editor", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.includes("mobile"), "mobile editor behavior is covered only on mobile");

  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.fileBrowser.state"));
  await openConsoleSubpage(page, "Remote Operations", "Files");
  await expect(page.getByText("Select a VPS and file to begin.")).toBeVisible();
  await expect(page.locator(".codeMirrorShell")).toHaveCount(0);
  await unlockPrivilege(page, "Files");
  await activate(page.getByRole("button", { name: "Refresh", exact: true }));
  await page.getByRole("button", { name: /app\.conf/ }).dblclick();

  const workspace = page.locator(".fileBrowserWorkspace.editorOpen");
  await expect(workspace).toBeVisible();
  await expect(workspace.locator(".fileTreePane")).toBeHidden();
  await expect(workspace.locator(".fileDetailsPane")).toBeHidden();
  await expect(page.getByRole("button", { name: "Back to files" })).toBeVisible();
  await expect(page.locator(".codeMirrorShell")).toContainText("listen=443");

  await activate(page.getByRole("button", { name: "Back to files" }));
  await expect(page.getByText("Select a VPS and file to begin.")).toBeVisible();
});

test("runs bulk file download and upload workflows with grouped summaries", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "bulk file workflow is covered in desktop layout");

  await page.goto("/");
  await page.evaluate(() => localStorage.removeItem("vpsman.multiFile.selectorExpression"));
  await openConsoleSubpage(page, "Remote Operations", "Bulk files");
  await expect(page.getByRole("heading", { name: "Bulk files" })).toBeVisible();
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Remote Operations", "Bulk files");

  const preflight = page.getByLabel("Bulk file preflight checks");
  await expect(preflight).toContainText("Size estimate");
  await expect(preflight).toContainText("16.0 MiB cap per target");
  await expect(preflight).toContainText("Remote matched-file size is only known after agents read the path");
  await expect(preflight).toContainText("Retry and retention");
  await expect(page.getByLabel("Bulk file path")).toHaveValue("");
  await expect(preflight).toContainText("Enter an absolute path before running download.");
  await page.getByLabel("Bulk file path").fill("/");
  await expect(page.getByText("Filesystem root selected")).toBeVisible();
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(preflight).toContainText("Root path is blocked until you explicitly allow filesystem root operations.");
  await page.getByLabel("Allow filesystem root path").check();
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByLabel("Confirm bulk file operation")).toContainText("Root path");
  await expect(page.getByLabel("Confirm bulk file operation")).toContainText("Explicitly allowed before run");
  await activate(page.getByLabel("Confirm bulk file operation").getByRole("button", { name: "Cancel" }));

  await activate(page.getByRole("button", { name: "Refresh scope" }));
  await expect(page.getByText("3 VPSs resolved")).toBeVisible();
  await expect(page.getByLabel("Bulk file live match summary")).toContainText("3 resolved · 2 ready");
  await expect(page.getByLabel("Bulk file attention targets")).toContainText("backup-nyc-03");
  await expect(preflight).toContainText("Server target preview");
  await expect(preflight).toContainText("3 resolved (2 online, 1 stale)");
  await expect(preflight).toContainText("1 stale");
  await page.getByLabel("Bulk file path").fill("/etc/app.conf");
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeVisible();
  await activate(page.getByLabel("Confirm bulk file operation").getByRole("button", { name: "Download files" }));

  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "2 VPSs" })).toBeVisible();
  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "1 VPS" }).filter({ hasText: "stale" })).toBeVisible();
  await expect(page.getByLabel("Execution result").getByText("partial success: 2 completed, 1 unsuccessful", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Download Archive" })).toHaveCount(1);
  const postRun = page.getByLabel("Bulk file post-run handling");
  await expect(postRun).toContainText("1 VPS retry candidates");
  await expect(postRun).toContainText("2 downloadable");

  await activate(page.getByRole("button", { name: "Upload files" }));
  await page.getByLabel("Bulk file destination path").fill("/etc/app.conf");
  await page.getByLabel("Bulk upload file").setInputFiles({
    name: "app.conf",
    mimeType: "text/plain",
    buffer: Buffer.from("listen=9443\n"),
  });
  await expect(preflight).toContainText("12 B per target");
  await expect(preflight).toContainText("36 B estimated dispatch across 3 VPSs");
  await page.getByLabel("Existing file").selectOption("skip");
  await activate(page.getByRole("button", { name: "Run upload" }));
  await expect(page.getByText("Confirm bulk file operation")).toBeVisible();
  await activate(page.getByLabel("Confirm bulk file operation").getByRole("button", { name: "Upload file" }));

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
    "Change permissions",
    "Change owner/group",
    "Create folder",
    "Write text",
  ]);
  await expect(page.getByText("Delete is intentionally not primary")).toHaveCount(0);

  await advancedAction.selectOption("copy");
  await expect(page.getByLabel("Destination path")).toBeVisible();
  await expect(page.getByLabel("Policy")).toBeVisible();
  await expect(page.getByRole("button", { name: "Run copy" })).toHaveClass(/secondaryAction/);

  await advancedAction.selectOption("delete");
  await page.getByLabel("Recursive").check();
  await expect(page.getByRole("button", { name: "Run delete" })).toHaveClass(/dangerAction/);
  await activate(page.getByRole("button", { name: "Run delete" }));
  const confirmation = page.locator(".confirmationPrompt.danger");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("Delete paths on 3 VPSs");
  await expect(confirmation).toContainText("Stop if the target state is unsafe");
  await expect(confirmation).toContainText("Selector");
  await expect(confirmation).toContainText("id:*");
  await expect(confirmation).toContainText("Targets");
  await expect(confirmation).toContainText("3");
  await expect(confirmation).toContainText("Path");
  await expect(confirmation).toContainText("/etc/app.conf");
  await expect(confirmation).toContainText("Recursive");
  await expect(confirmation).toContainText("Include child paths");
  await expect(confirmation).toContainText("Policy");
  await expect(page.getByRole("button", { name: "Delete paths" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Cancel" }));
});
