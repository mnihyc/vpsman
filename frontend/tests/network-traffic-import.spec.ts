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

test("vnStat import leaves interfaces empty for agent-side discovery", () => {
  expect(buildNetworkTrafficImportOperation("", "2020-01-01", 1_722_470_400)).toEqual({
    type: "network_traffic_import_vnstat",
    interfaces: [],
    start_unix: 1_577_836_800,
  });
});

test("vnStat import retains interface, date, and past-time validation", () => {
  expect(() => buildNetworkTrafficImportOperation("eth 0", "2020-01-01", 1_722_470_400)).toThrow(
    "Use interface names",
  );
  expect(() => buildNetworkTrafficImportOperation("eth0", "2024-08-02", 1_722_470_400)).toThrow(
    "before the current UTC minute",
  );
});

test("vnStat import accepts prefix selectors without changing exact names or blank discovery", () => {
  expect(buildNetworkTrafficImportOperation("e*, eth0, absent0", "2020-01-01", 1_722_470_400).interfaces)
    .toEqual(["e*", "eth0", "absent0"]);
  for (const invalid of ["e**", "*e", "e*h", "e?", "../eth0"]) {
    expect(() => buildNetworkTrafficImportOperation(invalid, "2020-01-01", 1_722_470_400)).toThrow();
  }
});
