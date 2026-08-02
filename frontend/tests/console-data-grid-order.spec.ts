import { expect, test } from "@playwright/test";
import { reconcileColumnOrder } from "../src/components/ConsoleDataGrid";

test("pins structural grid columns ahead of persisted data order", () => {
  const defaults = ["__select", "__expand", "name", "state", "provider"];

  expect(reconcileColumnOrder(["state", "name"], defaults)).toEqual([
    "__select",
    "__expand",
    "state",
    "name",
    "provider",
  ]);
  expect(
    reconcileColumnOrder(["__expand", "state", "__select", "name"], defaults),
  ).toEqual(["__select", "__expand", "state", "name", "provider"]);
});
