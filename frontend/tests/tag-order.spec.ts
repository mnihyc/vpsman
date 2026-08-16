import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import {
  buildTagOrderBlocks,
  compareTagNamesNaturally,
  insertNewTagIntoOrder,
  moveTagOrderBlock,
  moveTagOrderLeaf,
  naturallySortTagOrderBlock,
  naturallySortedTagNames,
  normalizeNaturalTagOrder,
  reconcileTagOrderDraft,
  tagNamespace,
} from "../src/tagOrder";

const tagOrderCases = JSON.parse(
  readFileSync(
    new URL(
      "../../crates/common/tests/fixtures/tag-order-cases.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as {
  cases: Array<{ expected: string[]; input: string[]; name: string }>;
};

test("matches the shared backend natural-order fixtures", async () => {
  for (const testCase of tagOrderCases.cases) {
    await test.step(testCase.name, () => {
      expect(naturallySortedTagNames(testCase.input)).toEqual(
        testCase.expected,
      );
    });
  }
});

test("groups only contiguous first-colon namespaces case-insensitively", () => {
  const blocks = buildTagOrderBlocks([
    "Provider:A",
    "provider:B",
    "edge",
    "provider:C",
    "provider:D:archive",
    "country:US",
  ]);

  expect(blocks.map(({ names, namespace }) => ({ names, namespace }))).toEqual([
    {
      names: ["Provider:A", "provider:B"],
      namespace: "provider",
    },
    { names: ["edge"], namespace: null },
    {
      names: ["provider:C", "provider:D:archive"],
      namespace: "provider",
    },
    { names: ["country:US"], namespace: "country" },
  ]);
  expect(tagNamespace("plain")).toBeNull();
  expect(tagNamespace("provider:")).toBe("provider");
  expect(tagNamespace(":alpha")).toBeNull();
});

test("uses deterministic ASCII natural order with intuitive zero padding", () => {
  const names = [
    "provider:A10",
    "provider:A002",
    "provider:a002",
    "provider:a1",
    "provider:a2",
    "provider:A02",
    "provider:a02",
    "provider:A2",
  ];

  expect(naturallySortedTagNames(names)).toEqual([
    "provider:a1",
    "provider:A2",
    "provider:a2",
    "provider:A02",
    "provider:a02",
    "provider:A002",
    "provider:a002",
    "provider:A10",
  ]);
  expect(compareTagNamesNaturally("provider:a", "provider:A")).toBeGreaterThan(
    0,
  );
  expect(naturallySortedTagNames(["provider:A02b", "provider:A2a"])).toEqual([
    "provider:A2a",
    "provider:A02b",
  ]);
  expect(
    naturallySortedTagNames(["provider:A02B1", "provider:A2B0001"]),
  ).toEqual(["provider:A2B0001", "provider:A02B1"]);
});

test("natural normalization sorts each duplicate namespace run independently", () => {
  expect(
    normalizeNaturalTagOrder([
      "provider:A10",
      "provider:A2",
      "edge",
      "provider:B10",
      "provider:B2",
      "country:US",
      "country:DE",
    ]),
  ).toEqual([
    "provider:A2",
    "provider:A10",
    "edge",
    "provider:B2",
    "provider:B10",
    "country:DE",
    "country:US",
  ]);
});

test("one-shot sorting changes only the selected contiguous block", () => {
  const order = [
    "provider:A10",
    "provider:A2",
    "edge",
    "provider:B10",
    "provider:B2",
  ];
  const secondProviderBlock = buildTagOrderBlocks(order)[2];
  expect(secondProviderBlock).toBeDefined();

  expect(naturallySortTagOrderBlock(order, secondProviderBlock!.id)).toEqual([
    "provider:A10",
    "provider:A2",
    "edge",
    "provider:B2",
    "provider:B10",
  ]);
});

test("block identity follows the exact member set, not child order", () => {
  const [original] = buildTagOrderBlocks(["provider:B", "provider:A"]);
  const [sorted] = buildTagOrderBlocks(["provider:A", "provider:B"]);
  const [changed] = buildTagOrderBlocks([
    "provider:A",
    "provider:B",
    "provider:C",
  ]);

  expect(original?.id).toBe(sorted?.id);
  expect(changed?.id).not.toBe(original?.id);
});

test("block drag preserves children and merges only when made contiguous", () => {
  const order = [
    "provider:A",
    "provider:B",
    "country:US",
    "country:DE",
    "edge",
  ];
  const blocks = buildTagOrderBlocks(order);

  expect(moveTagOrderBlock(order, blocks[0]!.id, blocks[2]!.id, false)).toEqual(
    ["country:US", "country:DE", "edge", "provider:A", "provider:B"],
  );
});

test("leaf drag can split a block and automatic mode normalizes resulting runs", () => {
  const order = ["country:US", "country:DE", "provider:A10", "provider:A2"];

  expect(moveTagOrderLeaf(order, "provider:A10", "country:DE", true)).toEqual([
    "country:US",
    "provider:A10",
    "country:DE",
    "provider:A2",
  ]);
});

test("new matching tags join the last matching run", () => {
  const order = [
    "provider:A",
    "country:US",
    "provider:C",
    "provider:A10",
    "edge",
  ];

  expect(insertNewTagIntoOrder(order, "provider:A2", false)).toEqual([
    "provider:A",
    "country:US",
    "provider:C",
    "provider:A10",
    "provider:A2",
    "edge",
  ]);
  expect(insertNewTagIntoOrder(order, "provider:A2", true)).toEqual([
    "provider:A",
    "country:US",
    "provider:A2",
    "provider:A10",
    "provider:C",
    "edge",
  ]);
});

test("draft reconciliation removes deleted tags and inserts new tags without losing edits", () => {
  expect(
    reconcileTagOrderDraft(
      ["edge", "provider:A10", "provider:A2", "removed"],
      ["provider:A10", "provider:A2", "provider:A3", "country:US", "edge"],
      false,
    ),
  ).toEqual([
    "edge",
    "provider:A10",
    "provider:A2",
    "provider:A3",
    "country:US",
  ]);
});
