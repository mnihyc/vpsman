import { expect, test } from "@playwright/test";
import { buildNetworkTrafficImportOperation } from "../src/panels/jobDispatchModel";

test("vnStat import accepts retained history older than thirty-five days", () => {
  expect(
    buildNetworkTrafficImportOperation("eth0, ens3", "2020-01-01", 1_722_470_400),
  ).toEqual({
    type: "network_traffic_import_vnstat",
    interfaces: ["eth0", "ens3"],
    start_unix: 1_577_836_800,
  });
});

test("vnStat import retains interface, date, and past-time validation", () => {
  expect(() => buildNetworkTrafficImportOperation("eth 0", "2020-01-01", 1_722_470_400)).toThrow(
    "Interface names may contain only",
  );
  expect(() => buildNetworkTrafficImportOperation("eth0", "2024-08-02", 1_722_470_400)).toThrow(
    "before the current UTC minute",
  );
});
