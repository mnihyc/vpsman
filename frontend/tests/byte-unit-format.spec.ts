import { expect, test } from "@playwright/test";
import {
  formatByteCount,
  formatByteRateFromBitsPerSecond,
} from "../src/telemetryMetrics";

test("uses decimal byte units by default", () => {
  expect(formatByteCount(1_000)).toBe("1.0 KB");
  expect(formatByteCount(1_000_000)).toBe("1.0 MB");
  expect(formatByteCount(1_000_000_000)).toBe("1.0 GB");
  expect(formatByteCount(1_000_000_000_000)).toBe("1.0 TB");
  expect(formatByteRateFromBitsPerSecond(8_000_000)).toBe("1.0 MB/s");
});

test("supports binary byte units as an explicit preference", () => {
  expect(formatByteCount(1_024, "binary")).toBe("1.0 KiB");
  expect(formatByteCount(1_048_576, "binary")).toBe("1.0 MiB");
  expect(formatByteCount(1_073_741_824, "binary")).toBe("1.0 GiB");
  expect(formatByteCount(1_099_511_627_776, "binary")).toBe("1.0 TiB");
  expect(formatByteRateFromBitsPerSecond(8_388_608, "binary")).toBe(
    "1.0 MiB/s",
  );
});

test("keeps TB and TiB as the largest display units", () => {
  expect(formatByteCount(1_000_000_000_000_000)).toBe("1000 TB");
  expect(formatByteCount(1_125_899_906_842_624, "binary")).toBe("1024 TiB");
});
