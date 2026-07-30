import { expect, test } from "@playwright/test";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  retainMutationSuccessAfterRefresh,
  timestampMillis,
} from "../src/utils";

test("formats Unix-second and Unix-millisecond timestamp strings", () => {
  const seconds = "1700000000";
  const milliseconds = "1700000000000";

  expect(timestampMillis(seconds)).toBe(1_700_000_000_000);
  expect(timestampMillis(milliseconds)).toBe(1_700_000_000_000);
  for (const formatter of [formatTime, formatCompactTime, formatFullTime]) {
    expect(formatter(seconds, "UTC")).not.toBe(seconds);
    expect(formatter(milliseconds, "UTC")).not.toBe(milliseconds);
  }
});

test("keeps a completed mutation successful when its visible refresh fails", async () => {
  let refreshAttempted = false;

  await expect(
    retainMutationSuccessAfterRefresh(async () => {
      refreshAttempted = true;
      throw new Error("refresh unavailable");
    }),
  ).resolves.toBeUndefined();
  expect(refreshAttempted).toBe(true);
});
