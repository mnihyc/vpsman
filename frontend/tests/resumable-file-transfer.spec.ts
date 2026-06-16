import { expect, test } from "@playwright/test";

import { resumableTransferWaitTimeoutMs } from "../src/resumableFileTransfer";

test("resumable transfer wait budget follows backend command timeout", () => {
  expect(resumableTransferWaitTimeoutMs(120)).toBe(155_000);
  expect(resumableTransferWaitTimeoutMs(30)).toBe(90_000);
  expect(resumableTransferWaitTimeoutMs(0)).toBe(90_000);
});
