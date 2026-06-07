import { expect, test, type Locator } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

test("creates server-side handoff for a completed download session", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense transfer handoff controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Transfer history");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("core-fra-02")).toBeVisible();
  await activate(panel.getByRole("button", { name: "Create transfer handoff session 51515151" }));

  const requests = await page.evaluate(() => (window as any).__vpsmanTestRequests.fileTransferHandoffs);
  expect(requests).toEqual([
    {
      body: { confirmed: true },
      client_id: "agent-fra-02",
      session_id: "51515151-2222-4333-8444-555555555555",
    },
  ]);
});

test("downloads selected handoffs for multiple completed download sessions", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense transfer handoff controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Transfer history");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByText("2 completed downloads available, 0 selected")).toBeVisible();
  await activate(panel.getByRole("button", { name: "Select all" }));
  await expect(panel.getByText("2 completed downloads available, 2 selected")).toBeVisible();
  await activate(panel.getByRole("button", { name: "Download selected handoffs" }));

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

test("streams a handoff artifact to a browser file handle", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense transfer handoff controls are covered in desktop layout");

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
  await openConsoleSubpage(page, "Jobs", "Transfer history");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await panel.getByLabel("Transfer handoff save method").selectOption("stream-to-file");
  await activate(panel.getByRole("button", { name: "Create transfer handoff session 51515151" }));

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

test("uploads a confirmed source artifact for transfer reuse", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense transfer source controls are covered in desktop layout");

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Transfer history");

  const panel = page.locator(".fleetPanel", { hasText: "File transfer sessions" });
  await expect(panel.getByRole("heading", { name: "Source artifacts" })).toBeVisible();
  await expect(panel.getByText("payload.bin")).toBeVisible();

  const payload = Buffer.from("source artifact payload");
  await panel.getByLabel("Source file").setInputFiles({
    name: "source.bin",
    mimeType: "application/octet-stream",
    buffer: payload,
  });
  await panel.getByLabel("Artifact name").fill("source.bin");
  await activate(panel.getByRole("button", { name: "Upload source artifact" }));

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
