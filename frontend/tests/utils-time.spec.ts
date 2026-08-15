import { expect, test } from "@playwright/test";
import {
  formatCompactTime,
  formatBillingRenewal,
  formatFullTime,
  formatTime,
  retainMutationSuccessAfterRefresh,
  trafficLimitingQuota,
  trafficUnlimitedQuota,
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

test("selects the most-used finite RX quota even when total traffic is unlimited", () => {
  expect(
    trafficLimitingQuota({
      quota_rx_bytes: 4_000,
      quota_total_bytes: -1,
      quota_tx_bytes: 8_000,
      rx_bytes: 3_000,
      total_bytes: 5_000,
      tx_bytes: 2_000,
    }),
  ).toEqual({ direction: "RX", percent: 75, quota: 4_000, used: 3_000 });
});

test("keeps an unlimited directional quota tied to its counted bytes", () => {
  expect(
    trafficUnlimitedQuota({
      quota_rx_bytes: null,
      quota_total_bytes: null,
      quota_tx_bytes: -1,
      rx_bytes: 9_000,
      total_bytes: 12_000,
      tx_bytes: 3_000,
    }),
  ).toEqual({ direction: "TX", used: 3_000 });
});

test("formats every full billing anchor as MM-DD", () => {
  expect(formatBillingRenewal("15", "m")).toBe("Renews day 15");
  expect(formatBillingRenewal("06-15", "q")).toBe("Renews 06-15");
  expect(formatBillingRenewal("06-15", "y")).toBe("Renews 06-15");
});
