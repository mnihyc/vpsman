import { expect, test, type Locator } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
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
  await expect(panel.getByText("Upload complete").first()).toBeVisible();
  await expect(panel.getByText("100 Mbps cap")).toBeVisible();
  await expect(panel.getByText("No transfer cap").first()).toBeVisible();
  await expect(panel.getByText("No handoff")).toHaveCount(0);
  await expect(panel.getByText("core-fra-02 (ra02)").first()).toBeVisible();
  await expect(panel.getByText("51515151").first()).toBeVisible();
  await expect(panel.getByText("Ready to download").first()).toBeVisible();
  await activate(panel.getByRole("button", { name: "Ready to download session 51515151" }));
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
  await expect(panel.getByText("2 ready to download, 0 unavailable, 0 selected")).toBeVisible();
  await activate(panel.getByRole("button", { name: "Select all" }));
  await expect(panel.getByText("2 ready to download, 0 unavailable, 2 selected")).toBeVisible();
  await activate(panel.getByRole("button", { name: "Review selected downloads" }));
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
  const payload = Buffer.from("quick upload payload");
  await panel.getByLabel("Transfer upload local file").setInputFiles({
    name: "quick-upload.bin",
    mimeType: "application/octet-stream",
    buffer: payload,
  });
  await panel.getByLabel("Transfer upload destination path").fill("/tmp/quick-upload.bin");
  await expect(panel.getByText("quick-upload.bin · 20 B")).toBeVisible();
  await activate(panel.getByRole("button", { name: "Upload", exact: true }));

  await expect(page.getByRole("heading", { level: 1, name: "Command dispatch" })).toBeVisible();
  const composer = page.locator(".fleetPanel", { hasText: "Dispatch command" });
  await expect(composer.getByRole("button", { name: "Resumable upload", exact: true })).toHaveClass(/selected/);
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");
  await expect(composer.getByLabel("Resumable upload path")).toHaveValue("/tmp/quick-upload.bin");
  await expect(composer.getByLabel("Resumable upload source").locator("..")).toContainText(
    "Selected quick-upload.bin",
  );
});

test("opens failed transfer retry metadata in resumable dispatch", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense transfer retry review is covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("1 failed sessions need metadata review")).toBeVisible();
  await expect(panel.getByText("aborted")).toBeVisible();
  await expect(panel.getByText("/var/log/nginx/error.log")).toBeVisible();

  await activate(panel.getByRole("button", { name: "Retry transfer session 53535353" }));
  const review = panel.getByRole("region", { name: "Transfer retry review" });
  await expect(review).toContainText("Failed transfer retry review");
  await expect(review).toContainText("edge-sfo-01 (fo01)");
  await expect(review).toContainText("Download from VPS");
  await expect(review).toContainText("/var/log/nginx/error.log");
  await expect(review).toContainText("320.0 KiB / 1.0 MiB (31%)");
  await expect(review).toContainText("50 Mbps cap");
  await expect(review).toContainText("Checksum not reported by session");
  await expect(review).toContainText("chunk 64.0 KiB, last 32.0 KiB");
  await expect(review).toContainText("session aborted");
  await expect(review).toContainText("file_transfer_download_chunk");
  await expect(review).toContainText("57575757");
  await expect(review).toContainText("Continue requires the original resume token");

  await expect(review.getByRole("button", { name: "Continue in Dispatch" })).toBeEnabled();
  await expect(review.getByRole("button", { name: "Start fresh in Dispatch" })).toBeEnabled();
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("remote-operations-transfers-failed-retry.png"),
  });
  await activate(review.getByRole("button", { name: "Continue in Dispatch" }));

  await expect(page.getByRole("heading", { level: 1, name: "Command dispatch" })).toBeVisible();
  const composer = page.locator(".fleetPanel", { hasText: "Dispatch command" });
  await expect(composer.getByRole("button", { name: "Resumable download" })).toHaveClass(/selected/);
  await expect(composer.getByLabel("Bulk target selector expression")).toContainText("id:agent-sfo-01");
  await expect(composer.getByLabel("Resumable download path")).toHaveValue("/var/log/nginx/error.log");
  await expect(composer.getByLabel("Resumable download filename")).toHaveValue("error.log");
  await expect(composer.getByLabel("Resumable download chunk bytes")).toHaveValue("65536");
  await expect(composer.getByLabel("Resumable download rate limit")).toHaveValue("50000");
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
  await activate(panel.getByRole("button", { name: "Ready to download session 51515151" }));
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
  expect(streamed.suggestedName).toBe("core-fra-02 (ra02)-51515151-bird.log");
  expect(streamed.text).toContain("server-side transfer handoff agent-fra-02");
});

test("uploads a confirmed reusable source for transfer reuse", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense reusable source controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Transfers");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await panel.getByText("Advanced: reusable upload sources").click();
  await expect(panel.getByRole("heading", { name: "Reusable upload sources" })).toBeVisible();
  await expect(panel.getByText("payload.bin")).toBeVisible();

  const payload = Buffer.from("reusable source payload");
  await panel.getByLabel("Source file").setInputFiles({
    name: "source.bin",
    mimeType: "application/octet-stream",
    buffer: payload,
  });
  await panel.getByLabel("Reusable source name").fill("source.bin");
  await activate(panel.getByRole("button", { name: "Review reusable source" }));
  await expect(panel.getByLabel("Confirm reusable source upload")).toBeVisible();
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("remote-operations-transfers-reusable-source-upload.png"),
  });
  await activate(
    panel
      .getByLabel("Confirm reusable source upload")
      .getByRole("button", { name: "Upload reusable source" }),
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
