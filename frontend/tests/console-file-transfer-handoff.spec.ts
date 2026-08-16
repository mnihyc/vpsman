import { expect, test, type Locator, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function invokeTransferAction(
  page: Page,
  rowId: string,
  action: string,
) {
  const grid = page.getByLabel("Transfer sessions data grid");
  await grid
    .getByLabel(`Select Transfer sessions row ${rowId}`)
    .check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await activate(page.getByRole("menuitem", { name: action, exact: true }));
}

test("downloads a completed transfer when it is ready", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense ready-download controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("Upload file").first()).toBeVisible();
  await expect(panel.getByText("Ready downloads").first()).toBeVisible();
  await expect(panel.getByText("Transfer sessions").first()).toBeVisible();
  await expect(panel.getByText("Upload to VPS").first()).toBeVisible();
  await expect(panel.getByText("Download from VPS").first()).toBeVisible();
  const transferGrid = panel.getByLabel("Transfer sessions data grid");
  const completedUpload = transferGrid
    .locator(".gridBody [role=row]", { hasText: "/opt/vpsman/app.bin" })
    .first();
  await expect(completedUpload).toContainText("Upload to VPS");
  await expect(completedUpload).toContainText("Completed");
  await expect(completedUpload).toContainText("Fresh session");
  await expect(panel.getByText("100 Mbps cap")).toBeVisible();
  await expect(panel.getByText("No transfer cap").first()).toBeVisible();
  await expect(panel.getByText("No handoff")).toHaveCount(0);
  await expect(panel.getByText("core-fra-02 (ra02)").first()).toBeVisible();
  await expect(panel.getByText("51515151").first()).toBeVisible();
  await expect(panel.getByText("Ready to download").first()).toBeVisible();
  await invokeTransferAction(
    page,
    "agent-fra-02:51515151-2222-4333-8444-555555555555",
    "Download",
  );
  await expect(panel.getByLabel("Confirm ready download")).toBeVisible();
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("remote-operations-transfers-ready-download.png"),
  });
  await activate(
    panel
      .getByLabel("Confirm ready download")
      .getByRole("button", { name: "Download selected files" }),
  );

  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileTransferHandoffs);
  expect(requests).toEqual([
    {
      body: { confirmed: true },
      client_id: "agent-fra-02",
      session_id: "51515151-2222-4333-8444-555555555555",
    },
  ]);
});

test("downloads selected ready transfers for multiple completed sessions", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense ready-download controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("2 ready, 0 unavailable")).toBeVisible();
  const grid = panel.getByLabel("Transfer sessions data grid");
  const uploadRowId =
    "agent-sfo-01:41414141-2222-4333-8444-555555555555";
  const firstDownloadRowId =
    "agent-fra-02:51515151-2222-4333-8444-555555555555";
  const secondDownloadRowId =
    "agent-sfo-01:52525252-2222-4333-8444-555555555555";
  await grid.getByLabel(`Select Transfer sessions row ${uploadRowId}`).check();
  await grid
    .getByLabel(`Select Transfer sessions row ${firstDownloadRowId}`)
    .check();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review downloads", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");

  await grid.getByLabel(`Select Transfer sessions row ${uploadRowId}`).uncheck();
  await grid
    .getByLabel(`Select Transfer sessions row ${secondDownloadRowId}`)
    .check();
  await expect(grid.getByText("2 selected", { exact: true })).toBeVisible();
  await expect(panel.getByRole("button", { name: "Select all" })).toHaveCount(0);
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await activate(
    page.getByRole("menuitem", { name: "Review downloads", exact: true }),
  );
  await expect(panel.getByLabel("Confirm ready download")).toBeVisible();
  await activate(
    panel
      .getByLabel("Confirm ready download")
      .getByRole("button", { name: "Download selected files" }),
  );

  await expect
    .poll(() => page.evaluate(() => (window as any).__vpsmanTestRequests.fileTransferHandoffs.length))
    .toBe(2);
  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileTransferHandoffs);
  expect(requests).toEqual([
    {
      body: { confirmed: true },
      client_id: "agent-fra-02",
      session_id: "51515151-2222-4333-8444-555555555555",
    },
    {
      body: { confirmed: true },
      client_id: "agent-sfo-01",
      session_id: "52525252-2222-4333-8444-555555555555",
    },
  ]);
});

test("starts the default upload flow in resumable dispatch", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "desktop covers quick upload dispatch handoff");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  const reviewUpload = panel.getByRole("button", { name: "Review upload" });
  await expect(reviewUpload).toBeDisabled();
  await expect(reviewUpload).toHaveAttribute(
    "title",
    "Choose a local file, VPS, and absolute destination path",
  );
  const payload = Buffer.from("quick upload payload");
  await panel.getByLabel("Transfer upload local file").setInputFiles({
    name: "quick-upload.bin",
    mimeType: "application/octet-stream",
    buffer: payload,
  });
  await panel.getByLabel("Transfer upload destination path").fill("/tmp/quick-upload.bin");
  await expect(panel.getByTitle("quick-upload.bin · 20 B", { exact: true })).toBeVisible();
  const target = panel.getByRole("combobox", { name: "Transfer target VPS" });
  await target.fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ }).click();
  await activate(reviewUpload);

  await expect(page.getByRole("heading", { level: 1, name: "Transfers" })).toBeVisible();
  const composer = page.locator(".consoleDetailPanel", { hasText: "File transfer" });
  await expect(composer.getByLabel("Dispatch mode boundary")).toContainText("File transfer mode");
  await expect(composer.getByLabel("Bulk target selector expression")).toHaveValue("id:agent-sfo-01");
  await expect(composer.getByLabel("Resumable upload path")).toHaveValue("/tmp/quick-upload.bin");
  await expect(composer.locator(".dispatchSelectedFile")).toHaveText(
    "quick-upload.bin · 20 B",
  );
  await expect(composer.locator(".dispatchSelectedFile")).toHaveAttribute(
    "title",
    "quick-upload.bin",
  );
  await expect(
    composer.locator(".dispatchFilePicker").getByText("Replace", { exact: true }),
  ).toBeVisible();
  const transferHeader = composer.locator(".fileTransferOperationHeader");
  await expect(transferHeader.locator("strong")).toHaveText("Resumable upload");
  await expect(transferHeader.locator("span")).toContainText(
    "Streamed ACK-tracked browser upload",
  );
  expect(
    await transferHeader.evaluate((element) => {
      const title = element.querySelector("strong")?.getBoundingClientRect();
      const detail = element.querySelector("span")?.getBoundingClientRect();
      return Boolean(title && detail && title.bottom <= detail.top + 1);
    }),
  ).toBe(true);
});

test("starts the default download flow with the reviewed remote path", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "desktop covers quick download dispatch handoff");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await activate(panel.getByRole("button", { name: "Download", exact: true }));
  await panel.getByLabel("Transfer download source path").fill("/var/log/nginx/access.log");
  const target = panel.getByRole("combobox", { name: "Transfer target VPS" });
  await target.fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ }).click();
  await activate(panel.getByRole("button", { name: "Review download" }));

  const composer = page.locator(".consoleDetailPanel", { hasText: "File transfer" });
  await expect(composer.getByLabel("Bulk target selector expression")).toHaveValue("id:agent-sfo-01");
  await expect(composer.getByLabel("Resumable download path")).toHaveValue(
    "/var/log/nginx/access.log",
  );
  await expect(composer.getByLabel("Resumable download filename")).toHaveValue("access.log");
});

test("opens failed transfer retry metadata in resumable dispatch", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense transfer retry review is covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("1 failed sessions need metadata review")).toBeVisible();
  await expect(panel.getByText("aborted")).toBeVisible();
  await expect(panel.getByText("/var/log/nginx/error.log")).toBeVisible();

  await invokeTransferAction(
    page,
    "agent-sfo-01:53535353-2222-4333-8444-555555555555",
    "Retry",
  );
  const review = panel.getByRole("region", { name: "Transfer retry review" });
  await expect(review).toContainText("Failed transfer retry review");
  await expect(review).toContainText("edge-sfo-01 (fo01)");
  await expect(review).toContainText("Download from VPS");
  await expect(review).toContainText("/var/log/nginx/error.log");
  await expect(review).toContainText("328 KB / 1.0 MB (31%)");
  await expect(review).toContainText("50 Mbps cap");
  await expect(review).toContainText("Checksum not reported by session");
  await expect(review).toContainText("chunk 66 KB, last 33 KB");
  await expect(review).toContainText("session aborted");
  await expect(review).toContainText("file_transfer_download_chunk");
  await expect(review).toContainText("57575757");
  await expect(review).toContainText("Continue requires the original resume token");

  await expect(review.getByRole("button", { name: "Continue transfer" })).toBeEnabled();
  await expect(review.getByRole("button", { name: "Start fresh transfer" })).toBeEnabled();
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("remote-operations-transfers-failed-retry.png"),
  });
  await activate(review.getByRole("button", { name: "Continue transfer" }));

  await expect(page.getByRole("heading", { level: 1, name: "Transfers" })).toBeVisible();
  const composer = page.locator(".consoleDetailPanel", { hasText: "File transfer" });
  await expect(composer.getByLabel("Dispatch mode boundary")).toContainText("File transfer mode");
  await expect(composer.getByLabel("Bulk target selector expression")).toHaveValue("id:agent-sfo-01");
  await expect(composer.getByLabel("Resumable download path")).toHaveValue("/var/log/nginx/error.log");
  await expect(composer.getByLabel("Resumable download filename")).toHaveValue("error.log");
  await expect(composer.getByLabel("Resumable download chunk bytes")).toHaveValue("65536");
  await expect(
    composer.getByLabel("Resumable download rate limit Mbps"),
  ).toHaveValue("50");
  await expect(composer.getByLabel("Resumable download session")).toHaveValue(
    "53535353-2222-4333-8444-555555555555",
  );
  await expect(composer.getByLabel("Resumable download resume token")).toHaveValue("");
});

test("streams a ready download to a browser file handle", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense ready-download controls are covered in desktop layout");

  await page.addInitScript(() => {
    Object.defineProperty(window, "__vpsmanStreamedArtifact", {
      configurable: true,
      value: { chunks: [] as number[][], closed: false, suggestedName: "" },
    });
    Object.defineProperty(window, "showSaveFilePicker", {
      configurable: true,
      value: async (options?: { suggestedName?: string }) => {
        (window as any).__vpsmanStreamedArtifact.suggestedName = options?.suggestedName ?? "";
        return {
          createWritable: async () => ({
            abort: async () => {
              (window as any).__vpsmanStreamedArtifact.aborted = true;
            },
            close: async () => {
              (window as any).__vpsmanStreamedArtifact.closed = true;
            },
            write: async (chunk: Uint8Array) => {
              (window as any).__vpsmanStreamedArtifact.chunks.push(Array.from(chunk));
            },
          }),
        };
      },
    });
  });
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await panel.getByLabel("Ready download save method").selectOption("stream-to-file");
  await invokeTransferAction(
    page,
    "agent-fra-02:51515151-2222-4333-8444-555555555555",
    "Download",
  );
  await expect(panel.getByLabel("Confirm ready download")).toBeVisible();
  await activate(
    panel
      .getByLabel("Confirm ready download")
      .getByRole("button", { name: "Download selected files" }),
  );

  await expect
    .poll(() => page.evaluate(() => (window as any).__vpsmanStreamedArtifact.closed))
    .toBe(true);
  const streamed = await page.evaluate(() => {
    const state = (window as any).__vpsmanStreamedArtifact;
    return {
      suggestedName: state.suggestedName,
      text: new TextDecoder().decode(new Uint8Array(state.chunks.flat())),
    };
  });
  expect(streamed.suggestedName).toBe("core-fra-02 (ra02)-51515151-routing.log");
  expect(streamed.text).toContain("server-side transfer handoff agent-fra-02");
});

test("uploads a confirmed reusable source for transfer reuse", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense reusable source controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await panel.getByText("Advanced: source artifacts").click();
  await expect(panel.getByRole("heading", { name: "Source artifacts" })).toBeVisible();
  await expect(panel.getByText("payload.bin")).toBeVisible();

  const payload = Buffer.from("reusable source payload");
  await panel.getByLabel("Source file").setInputFiles({
    name: "source.bin",
    mimeType: "application/octet-stream",
    buffer: payload,
  });
  await panel.getByLabel("Source artifact name").fill("source.bin");
  await activate(panel.getByRole("button", { name: "Review source artifact" }));
  await expect(panel.getByLabel("Confirm source artifact upload")).toBeVisible();
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("remote-operations-transfers-reusable-source-upload.png"),
  });
  await activate(
    panel
      .getByLabel("Confirm source artifact upload")
      .getByRole("button", { name: "Upload source artifact" }),
  );

  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileTransferSourceUploads);
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({
    confirmed: true,
    name: "source.bin",
    size_bytes: payload.byteLength,
    source_base64: payload.toString("base64"),
  });
  expect(requests[0].sha256_hex).toMatch(/^[a-f0-9]{64}$/);
});
