import { expect, test, type Locator } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function checkControl(locator: Locator) {
  await locator.evaluate((element) => {
    const input = element as HTMLInputElement;
    if (!input.checked) {
      input.click();
    }
  });
}

async function dispatchWithPrompt(composer: Locator) {
  await activate(composer.getByRole("button", { name: "Dispatch" }));
  await expect(composer.getByText("Confirm job dispatch")).toBeVisible();
  await activate(composer.locator(".confirmationPrompt").getByRole("button", { name: "Dispatch job" }));
}

test("orchestrates browser resumable upload with ACK progress", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "browser resumable upload flow is covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const composer = page.locator(".commandComposer");
  await expect(composer.getByRole("heading", { name: "Dispatch command" })).toBeVisible();
  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await activate(composer.getByRole("button", { name: "Resumable upload" }));
  await composer.getByLabel("Resumable upload source").setInputFiles({
    name: "payload.bin",
    mimeType: "application/octet-stream",
    buffer: Buffer.from("resumable browser upload payload"),
  });
  await composer.getByLabel("Resumable upload path").fill("/tmp/browser-upload.bin");
  await composer.getByLabel("Resumable upload mode").fill("0600");
  await composer.getByLabel("Resumable upload chunk bytes").fill("8");
  await expect(composer.getByLabel("Resumable upload multi-target policy")).toHaveValue("same-offset");
  await composer.getByLabel("Resumable upload multi-target policy").selectOption("independent-offsets");
  await checkControl(composer.getByLabel("edge-sfo-01"));
  await checkControl(composer.getByLabel("Confirmed"));
  await dispatchWithPrompt(composer);

  await expect(composer.getByLabel("Resumable upload progress")).toContainText("Upload complete");
  await expect(composer.getByLabel("Resumable upload progress")).toContainText("independent-offsets");
  await expect(composer.getByLabel("Resumable upload session")).toHaveValue(/[0-9a-f-]{36}/);
  await expect(composer.getByLabel("Resumable upload resume token")).toHaveValue(/^[0-9a-f]{64}$/);

  const requests = await page.evaluate(() => {
    return (window as unknown as { __vpsmanTestRequests: { jobs: Array<{ operation?: { type?: string } }> } })
      .__vpsmanTestRequests.jobs;
  });
  const transferRequests = requests.filter((request) => request.operation?.type?.startsWith("file_transfer_"));
  expect(transferRequests.map((request) => request.operation?.type)).toEqual([
    "file_transfer_start",
    "file_transfer_chunk",
    "file_transfer_chunk",
    "file_transfer_chunk",
    "file_transfer_chunk",
    "file_transfer_commit",
  ]);
  expect(JSON.stringify(transferRequests)).not.toContain("local-super-password");
  expect(JSON.stringify(transferRequests)).not.toContain("resumable browser upload payload");
  expect(transferRequests[0]).toMatchObject({
    clients: ["agent-sfo-01"],
    command: "file_transfer_start",
    operation: {
      mode: 0o600,
      path: "/tmp/browser-upload.bin",
      resume_token_hash: expect.stringMatching(/^[0-9a-f]{64}$/),
      size_bytes: 32,
      type: "file_transfer_start",
    },
    tags: [],
  });
  expect((transferRequests[1].operation as { offset: number }).offset).toBe(0);
  expect((transferRequests[2].operation as { offset: number }).offset).toBe(8);
  expect((transferRequests[3].operation as { offset: number }).offset).toBe(16);
  expect((transferRequests[4].operation as { offset: number }).offset).toBe(24);
});

test("orchestrates browser resumable upload from retained source artifact", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "browser source-artifact upload flow is covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const composer = page.locator(".commandComposer");
  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await activate(composer.getByRole("button", { name: "Resumable upload" }));
  await composer.getByLabel("Resumable upload producer").selectOption("source-artifact");
  await composer.getByLabel("Resumable upload source artifact").selectOption("62626262-2222-4333-8444-555555555555");
  await composer.getByLabel("Resumable upload path").fill("/tmp/artifact-upload.bin");
  await composer.getByLabel("Resumable upload mode").fill("0644");
  await composer.getByLabel("Resumable upload chunk bytes").fill("8");
  await checkControl(composer.getByLabel("edge-sfo-01"));
  await checkControl(composer.getByLabel("Confirmed"));
  await dispatchWithPrompt(composer);

  await expect(composer.getByLabel("Resumable upload progress")).toContainText("Upload complete");

  const requests = await page.evaluate(() => {
    return (window as unknown as { __vpsmanTestRequests: { jobs: Array<{ operation?: { type?: string } }> } })
      .__vpsmanTestRequests.jobs;
  });
  const transferRequests = requests.filter((request) => request.operation?.type?.startsWith("file_transfer_"));
  expect(transferRequests.map((request) => request.operation?.type)).toEqual([
    "file_transfer_start",
    "file_transfer_chunk",
    "file_transfer_chunk",
    "file_transfer_chunk",
    "file_transfer_commit",
  ]);
  expect(JSON.stringify(transferRequests)).not.toContain("local-super-password");
  expect(JSON.stringify(transferRequests)).not.toContain("stored source artifact");
  expect(transferRequests[0]).toMatchObject({
    clients: ["agent-sfo-01"],
    command: "file_transfer_start",
    operation: {
      path: "/tmp/artifact-upload.bin",
      sha256_hex: "18212166996cdc1b0e123404503daba5feeac0fc3ba358e338009ee018ee9580",
      size_bytes: 22,
      type: "file_transfer_start",
    },
  });
  expect((transferRequests[1].operation as { offset: number }).offset).toBe(0);
  expect((transferRequests[2].operation as { offset: number }).offset).toBe(8);
  expect((transferRequests[3].operation as { offset: number }).offset).toBe(16);
});

test("orchestrates browser resumable download with artifact chunks", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "browser resumable download flow is covered in the desktop job composer");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const composer = page.locator(".commandComposer");
  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await activate(composer.getByRole("button", { name: "Resumable download" }));
  await composer.getByLabel("Resumable download path").fill("/tmp/browser-download.bin");
  await composer.getByLabel("Resumable download filename").fill("browser-download.bin");
  await composer.getByLabel("Resumable download chunk bytes").fill("8");
  await checkControl(composer.getByLabel("edge-sfo-01"));
  await checkControl(composer.getByLabel("Confirmed"));

  await activate(composer.getByRole("button", { name: "Dispatch" }));
  await expect(composer.getByText("Confirm job dispatch")).toBeVisible();
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    activate(composer.locator(".confirmationPrompt").getByRole("button", { name: "Dispatch job" })),
  ]);

  await expect(composer.getByLabel("Resumable download progress")).toContainText("Download complete");
  await expect(composer.getByLabel("Resumable download session")).toHaveValue(/[0-9a-f-]{36}/);
  await expect(composer.getByLabel("Resumable download resume token")).toHaveValue(/^[0-9a-f]{64}$/);
  expect(download.suggestedFilename()).toBe("browser-download.bin");
  const downloadPath = await download.path();
  expect(downloadPath).toBeTruthy();
  expect((await readFile(downloadPath!)).toString()).toBe("resumable browser download payload");

  const requests = await page.evaluate(() => {
    return (window as unknown as { __vpsmanTestRequests: { jobs: Array<{ operation?: { type?: string } }> } })
      .__vpsmanTestRequests.jobs;
  });
  const transferRequests = requests.filter((request) => request.operation?.type?.startsWith("file_transfer_download_"));
  expect(transferRequests.map((request) => request.operation?.type)).toEqual([
    "file_transfer_download_start",
    "file_transfer_download_chunk",
    "file_transfer_download_chunk",
    "file_transfer_download_chunk",
    "file_transfer_download_chunk",
    "file_transfer_download_chunk",
  ]);
  expect(JSON.stringify(transferRequests)).not.toContain("local-super-password");
  expect(JSON.stringify(transferRequests)).not.toContain("resumable browser download payload");
  expect(transferRequests[0]).toMatchObject({
    clients: ["agent-sfo-01"],
    command: "file_transfer_download_start",
    operation: {
      path: "/tmp/browser-download.bin",
      resume_token_hash: expect.stringMatching(/^[0-9a-f]{64}$/),
      type: "file_transfer_download_start",
    },
    tags: [],
  });
  expect((transferRequests[1].operation as { offset: number }).offset).toBe(0);
  expect((transferRequests[2].operation as { offset: number }).offset).toBe(8);
  expect((transferRequests[3].operation as { offset: number }).offset).toBe(16);
  expect((transferRequests[4].operation as { offset: number }).offset).toBe(24);
  expect((transferRequests[5].operation as { offset: number }).offset).toBe(32);
});

test("streams browser resumable download through writable file handle", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "stream-to-file save mode is covered in the desktop job composer");

  await page.addInitScript(() => {
    const state = {
      aborted: false,
      chunks: [] as number[][],
      closed: false,
      suggestedName: "",
    };
    Object.defineProperty(window, "__vpsmanStreamedDownload", {
      configurable: true,
      value: state,
    });
    Object.defineProperty(window, "showSaveFilePicker", {
      configurable: true,
      value: async (options?: { suggestedName?: string }) => {
        state.suggestedName = options?.suggestedName ?? "";
        return {
          createWritable: async () => ({
            abort: async () => {
              state.aborted = true;
            },
            close: async () => {
              state.closed = true;
            },
            write: async (chunk: Uint8Array) => {
              state.chunks.push(Array.from(chunk));
            },
          }),
        };
      },
    });
  });

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const composer = page.locator(".commandComposer");
  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await activate(composer.getByRole("button", { name: "Resumable download" }));
  await composer.getByLabel("Resumable download path").fill("/tmp/browser-download.bin");
  await composer.getByLabel("Resumable download filename").fill("streamed-download.bin");
  await composer.getByLabel("Resumable download chunk bytes").fill("8");
  await composer.getByLabel("Resumable download save method").selectOption("stream-to-file");
  await checkControl(composer.getByLabel("edge-sfo-01"));
  await checkControl(composer.getByLabel("Confirmed"));
  await dispatchWithPrompt(composer);

  await expect(composer.getByLabel("Resumable download progress")).toContainText("Download complete");
  await expect(composer.getByLabel("Resumable download progress")).toContainText("stream-to-file");

  const streamed = await page.evaluate(() => {
    const state = (window as unknown as {
      __vpsmanStreamedDownload: { aborted: boolean; chunks: number[][]; closed: boolean; suggestedName: string };
    }).__vpsmanStreamedDownload;
    return {
      aborted: state.aborted,
      bytes: state.chunks.flat(),
      closed: state.closed,
      suggestedName: state.suggestedName,
    };
  });
  expect(streamed.suggestedName).toBe("streamed-download.bin");
  expect(streamed.closed).toBe(true);
  expect(streamed.aborted).toBe(false);
  expect(Buffer.from(streamed.bytes).toString()).toBe("resumable browser download payload");
});
