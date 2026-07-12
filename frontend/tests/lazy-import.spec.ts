import { expect, test } from "@playwright/test";
import {
  isTransientModuleLoadError,
  retryTransientModuleLoad,
} from "../src/lazyImport";

test("retries a transient workspace import within one bounded deadline", async () => {
  let attempts = 0;
  const value = await retryTransientModuleLoad(
    async () => {
      attempts += 1;
      if (attempts === 1) {
        throw new TypeError("Failed to fetch dynamically imported module");
      }
      return "loaded";
    },
    { retries: 1, retryDelayMs: 1, timeoutMs: 100 },
  );

  expect(value).toBe("loaded");
  expect(attempts).toBe(2);
});

test("turns a hung workspace import into a recoverable transient error", async () => {
  const startedAt = Date.now();
  const error = await retryTransientModuleLoad(
    () => new Promise<never>(() => undefined),
    { retries: 3, retryDelayMs: 1, timeoutMs: 25 },
  ).catch((reason: unknown) => reason);

  expect(error).toBeInstanceOf(Error);
  expect((error as Error).message).toBe(
    "Workspace module load timed out after 25ms",
  );
  expect(isTransientModuleLoadError(error)).toBe(true);
  expect(Date.now() - startedAt).toBeLessThan(250);
});

test("does not retry a non-transient workspace import failure", async () => {
  let attempts = 0;
  const error = await retryTransientModuleLoad(
    async () => {
      attempts += 1;
      throw new Error("module contract is invalid");
    },
    { retries: 3, retryDelayMs: 1, timeoutMs: 100 },
  ).catch((reason: unknown) => reason);

  expect((error as Error).message).toBe("module contract is invalid");
  expect(attempts).toBe(1);
});
