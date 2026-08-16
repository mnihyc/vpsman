import { expect, test, type Locator, type Page } from "@playwright/test";
import type { TagOrderState } from "../src/types";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

const initialNames = [
  "provider:Zeta",
  "provider:A10",
  "provider:A2",
  "maintenance",
  "country:US",
  "country:DE",
  "provider:Beta",
  "provider:Alpha",
  "backup",
];

function tagOrderState(
  names = initialNames,
  namespaceNaturalSortEnabled = false,
): TagOrderState {
  return {
    namespace_natural_sort_enabled: namespaceNaturalSortEnabled,
    tags: names.map((name, displayOrder) => ({
      clients: [],
      display_order: displayOrder,
      name,
    })),
  };
}

async function openTagOrderEditor(
  page: Page,
  options: NonNullable<Parameters<typeof installConsoleApiMock>[1]> = {},
): Promise<Locator> {
  await installConsoleApiMock(page, {
    tagOrderStateOverride: tagOrderState(),
    ...options,
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Groups");
  const editor = page.getByLabel("Manage display order", { exact: true });
  await expect(editor).toBeVisible();
  return editor;
}

async function tagOrderUpdates(page: Page) {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: {
            tagOrderUpdates: Array<Record<string, unknown>>;
          };
        }
      ).__vpsmanTestRequests.tagOrderUpdates,
  );
}

async function reorderHandleLabels(editor: Locator) {
  return editor
    .locator('button[aria-label^="Reorder "]')
    .evaluateAll((buttons) =>
      buttons.map((button) => button.getAttribute("aria-label") ?? ""),
    );
}

test("tag order exposes layer two by default and remembers only current expanded groups", async ({
  page,
}) => {
  const editor = await openTagOrderEditor(page);
  const registryBounds = await page
    .getByLabel("Group registry data grid")
    .boundingBox();
  const editorBounds = await editor.boundingBox();
  expect(registryBounds).not.toBeNull();
  expect(editorBounds).not.toBeNull();
  expect(editorBounds?.y ?? 0).toBeGreaterThanOrEqual(
    (registryBounds?.y ?? 0) + (registryBounds?.height ?? 0),
  );
  await expect(
    editor.getByRole("button", { name: "Collapse Total tag order" }),
  ).toBeVisible();
  await expect(
    editor.getByRole("button", { name: "Expand provider: tag group" }),
  ).toHaveCount(2);
  await expect(
    editor.getByRole("button", { name: "Expand country: tag group" }),
  ).toHaveCount(1);
  const firstProviderBlock = editor
    .getByRole("button", { name: "Expand provider: tag group" })
    .first()
    .locator('xpath=ancestor::*[@role="listitem"][1]');
  await expect(firstProviderBlock).toContainText("1–3");
  await expect(firstProviderBlock).toContainText("3 tags · 0 assignments");
  await expect(editor.getByText("maintenance", { exact: true })).toBeVisible();
  await expect(editor.getByText("provider:Zeta", { exact: true })).toBeHidden();

  await editor
    .getByRole("button", { name: "Collapse Total tag order" })
    .click();
  await expect(
    editor.getByRole("button", { name: "Expand provider: tag group" }),
  ).toBeHidden();
  await page.reload();
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Groups");
  const collapsedEditor = page.getByLabel("Manage display order", {
    exact: true,
  });
  await expect(
    collapsedEditor.getByRole("button", { name: "Expand Total tag order" }),
  ).toBeVisible();
  await collapsedEditor
    .getByRole("button", { name: "Expand Total tag order" })
    .click();

  await collapsedEditor
    .getByRole("button", { name: "Expand provider: tag group" })
    .first()
    .click();
  await expect(
    collapsedEditor.getByText("provider:Zeta", { exact: true }),
  ).toBeVisible();
  await expect(
    collapsedEditor.getByText("provider:Beta", { exact: true }),
  ).toBeHidden();

  await page.reload();
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Groups");
  const restoredEditor = page.getByLabel("Manage display order", {
    exact: true,
  });
  await expect(
    restoredEditor.getByRole("button", {
      name: "Collapse provider: tag group",
    }),
  ).toHaveCount(1);
  await expect(
    restoredEditor.getByRole("button", { name: "Expand provider: tag group" }),
  ).toHaveCount(1);

  await restoredEditor
    .getByRole("button", { name: "Expand all tag groups" })
    .click();
  await expect(
    restoredEditor.getByRole("button", { name: "Collapse all tag groups" }),
  ).toBeVisible();
  await expect(
    restoredEditor.getByText("provider:Alpha", { exact: true }),
  ).toBeVisible();

  const nextState = tagOrderState([
    ...initialNames,
    "region:apac",
    "region:emea",
  ]);
  await page.evaluate((state) => {
    (
      window as typeof window & {
        __vpsmanSetTagOrderState: (next: TagOrderState) => void;
      }
    ).__vpsmanSetTagOrderState(state);
  }, nextState);
  await page
    .locator(".fleetPanel > .sectionHeader")
    .getByRole("button", { name: "Refresh" })
    .click();
  await expect(
    restoredEditor.getByRole("button", { name: "Expand region: tag group" }),
  ).toBeVisible();
  await expect(
    restoredEditor.getByText("region:apac", { exact: true }),
  ).toBeHidden();
  await expect(
    restoredEditor.getByText("provider:Alpha", { exact: true }),
  ).toBeVisible();
});

test("tag order stages natural sorting and sends exactly one authoritative save", async ({
  page,
}) => {
  const editor = await openTagOrderEditor(page);
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      }
    ).__vpsmanFetchRequests = [];
  });
  const topLevelBefore = await reorderHandleLabels(editor);
  await editor
    .getByLabel("Automatically sort tags within namespace groups")
    .check();
  await expect(
    editor.getByText("Unsaved changes", { exact: true }),
  ).toBeVisible();
  expect(await tagOrderUpdates(page)).toHaveLength(0);
  expect(await reorderHandleLabels(editor)).toEqual(topLevelBefore);

  await expect(
    editor
      .getByRole("button", { name: "Sort provider: tag group naturally" })
      .first(),
  ).toBeDisabled();
  await editor
    .getByRole("button", { name: "Expand all tag groups" })
    .click();
  const exactHandleNames = (await reorderHandleLabels(editor))
    .filter((label) => !label.endsWith(" tag group"))
    .map((label) => label.replace(/^Reorder /, ""));
  expect(exactHandleNames.indexOf("provider:A2")).toBeLessThan(
    exactHandleNames.indexOf("provider:A10"),
  );

  await editor.getByRole("button", { name: "Save order" }).click();
  await expect(editor.getByText("Saved", { exact: true })).toBeVisible();
  await expect
    .poll(async () => (await tagOrderUpdates(page)).length)
    .toBe(1);
  const [request] = await tagOrderUpdates(page);
  expect(request).toEqual({
    namespace_natural_sort_enabled: true,
    ordered_tags: exactHandleNames,
  });
  const orderRequests = await page.evaluate(() =>
    (
      (
        window as typeof window & {
          __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
        }
      ).__vpsmanFetchRequests ?? []
    ).filter((request) => request.url.includes("/api/v1/tags/order")),
  );
  expect(orderRequests).toEqual([
    expect.objectContaining({ method: "PUT" }),
  ]);
});

test("Revert restores the saved draft without a request", async ({
  page,
}) => {
  const editor = await openTagOrderEditor(page);
  await editor
    .getByRole("button", { name: "Sort provider: tag group naturally" })
    .first()
    .click();
  await editor
    .getByRole("button", { name: "Expand provider: tag group" })
    .first()
    .click();
  const firstRun = (await reorderHandleLabels(editor))
    .filter((label) => /^Reorder provider:(?:A2|A10|Zeta)$/.test(label))
    .map((label) => label.replace(/^Reorder /, ""));
  expect(firstRun).toEqual(["provider:A2", "provider:A10", "provider:Zeta"]);
  const autoSort = editor.getByLabel(
    "Automatically sort tags within namespace groups",
  );
  await autoSort.check();
  await editor.getByRole("button", { name: "Revert" }).click();
  await expect(autoSort).not.toBeChecked();
  await expect(editor.getByText("Saved", { exact: true })).toBeVisible();
  expect(await tagOrderUpdates(page)).toHaveLength(0);
});

test("failed tag order save keeps the draft available for retry", async ({
  page,
}) => {
  const editor = await openTagOrderEditor(page, {
    tagOrderUpdateFailure: true,
  });
  const autoSort = editor.getByLabel(
    "Automatically sort tags within namespace groups",
  );
  await autoSort.check();
  await editor.getByRole("button", { name: "Save order" }).click();
  await expect
    .poll(async () => (await tagOrderUpdates(page)).length)
    .toBe(1);
  await expect(autoSort).toBeChecked();
  await expect(
    editor.getByRole("button", { name: "Save order" }),
  ).toBeEnabled();
  await expect(editor.getByRole("alert")).toBeVisible();
});

test("saving locks the staged hierarchy until the authoritative response", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the save lock contract is viewport independent",
  );
  const editor = await openTagOrderEditor(page, {
    tagOrderUpdateDelayMs: 500,
  });
  const autoSort = editor.getByLabel(
    "Automatically sort tags within namespace groups",
  );
  await autoSort.check();
  await editor.getByRole("button", { name: "Save order" }).click();
  await expect(editor.getByText("Saving order", { exact: true })).toBeVisible();
  await expect(autoSort).toBeDisabled();
  await expect(editor.getByRole("button", { name: "Revert" })).toBeDisabled();
  await expect(editor.getByRole("button", { name: "Saving" })).toBeDisabled();
  await expect(editor.getByText("Saved", { exact: true })).toBeVisible();
  expect(await tagOrderUpdates(page)).toHaveLength(1);
});

test("a GET started during Save cannot overwrite the authoritative PUT response", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the request ordering contract is viewport independent",
  );
  const editor = await openTagOrderEditor(page, {
    tagOrderUpdateDelayMs: 500,
  });
  await page.evaluate(() => {
    const fixtureWindow = window as typeof window & {
      __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
      __vpsmanTestRequests: { tagOrderReads: unknown[] };
    };
    fixtureWindow.__vpsmanFetchRequests = [];
    fixtureWindow.__vpsmanTestRequests.tagOrderReads = [];
  });
  const autoSort = editor.getByLabel(
    "Automatically sort tags within namespace groups",
  );
  await autoSort.check();
  await editor.getByRole("button", { name: "Save order" }).click();
  await expect(editor.getByText("Saving order", { exact: true })).toBeVisible();

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanGateNextTagOrderGet: () => void;
      }
    ).__vpsmanGateNextTagOrderGet();
  });
  const refresh = page
    .locator(".fleetPanel > .sectionHeader")
    .getByRole("button", { name: "Refresh" });
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            (
              window as typeof window & {
                __vpsmanFetchRequests?: Array<{
                  method: string;
                  url: string;
                }>;
              }
            ).__vpsmanFetchRequests ?? []
          ).filter(
            (request) =>
              request.method === "GET" &&
              request.url.includes("/api/v1/tags/order"),
          ).length,
      ),
    )
    .toBe(1);
  await expect(editor.getByText("Saved", { exact: true })).toBeVisible();
  await expect(autoSort).toBeChecked();
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanReleaseTagOrderGet: () => void;
      }
    ).__vpsmanReleaseTagOrderGet();
  });
  await expect(refresh).toBeEnabled();
  await expect(autoSort).toBeChecked();
  await expect(editor.getByText("Saved", { exact: true })).toBeVisible();

  const orderRequests = await page.evaluate(() =>
    (
      (
        window as typeof window & {
          __vpsmanFetchRequests?: Array<{ method: string; url: string }>;
        }
      ).__vpsmanFetchRequests ?? []
    )
      .filter((request) => request.url.includes("/api/v1/tags/order"))
      .map((request) => request.method),
  );
  expect(orderRequests).toEqual(["PUT", "GET"]);
  expect(await tagOrderUpdates(page)).toEqual([
    {
      namespace_natural_sort_enabled: true,
      ordered_tags: [
        "provider:A2",
        "provider:A10",
        "provider:Zeta",
        "maintenance",
        "country:DE",
        "country:US",
        "provider:Alpha",
        "provider:Beta",
        "backup",
      ],
    },
  ]);
});

test("tag order supports keyboard reordering without an eager request", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "keyboard ordering is covered in the desktop hierarchy",
  );
  const editor = await openTagOrderEditor(page);
  await editor
    .getByRole("button", { name: "Expand provider: tag group" })
    .first()
    .click();
  const handle = editor.getByRole("button", { name: "Reorder provider:Zeta" });
  await handle.focus();
  await page.keyboard.press("Space");
  await page.waitForTimeout(100);
  await page.keyboard.press("ArrowDown");
  await expect(
    editor.getByRole("status").filter({ hasText: /^Move (?:before|after) / }),
  ).toBeVisible();
  await page.keyboard.press("Space");
  await expect(
    editor.getByText("Unsaved changes", { exact: true }),
  ).toBeVisible();
  expect(await tagOrderUpdates(page)).toHaveLength(0);
});

test("tag order stages a pointer-driven block move without an eager request", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "mouse ordering is covered in the desktop hierarchy",
  );
  const editor = await openTagOrderEditor(page);
  await editor.scrollIntoViewIfNeeded();
  const before = await reorderHandleLabels(editor);
  const source = editor
    .getByRole("button", { name: "Reorder provider: tag group" })
    .first();
  const target = editor.getByRole("button", {
    name: "Reorder country: tag group",
  });
  const sourceBox = await source.boundingBox();
  const targetBox = await target.boundingBox();
  expect(sourceBox).not.toBeNull();
  expect(targetBox).not.toBeNull();
  await page.mouse.move(
    (sourceBox?.x ?? 0) + (sourceBox?.width ?? 0) / 2,
    (sourceBox?.y ?? 0) + (sourceBox?.height ?? 0) / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    (targetBox?.x ?? 0) + (targetBox?.width ?? 0) / 2,
    (targetBox?.y ?? 0) + (targetBox?.height ?? 0) / 2,
    { steps: 12 },
  );
  await page.mouse.up();
  await expect(
    editor.getByText("Unsaved changes", { exact: true }),
  ).toBeVisible();
  expect(await reorderHandleLabels(editor)).not.toEqual(before);
  expect(await tagOrderUpdates(page)).toHaveLength(0);
});

test("dirty incoming reconciliation preserves the draft and joins the final namespace run", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "incoming draft reconciliation is viewport independent",
  );
  const editor = await openTagOrderEditor(page);
  await editor.scrollIntoViewIfNeeded();
  const source = editor.getByRole("button", { name: "Reorder maintenance" });
  const target = editor.getByRole("button", {
    name: "Reorder country: tag group",
  });
  const sourceBox = await source.boundingBox();
  const targetBox = await target.boundingBox();
  expect(sourceBox).not.toBeNull();
  expect(targetBox).not.toBeNull();
  await page.mouse.move(
    (sourceBox?.x ?? 0) + (sourceBox?.width ?? 0) / 2,
    (sourceBox?.y ?? 0) + (sourceBox?.height ?? 0) / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    (targetBox?.x ?? 0) + (targetBox?.width ?? 0) / 2,
    (targetBox?.y ?? 0) + (targetBox?.height ?? 0) / 2,
    { steps: 12 },
  );
  await page.mouse.up();
  await expect(
    editor.getByText("Unsaved changes", { exact: true }),
  ).toBeVisible();

  const incoming = tagOrderState([...initialNames, "provider:C"]);
  await page.evaluate((state) => {
    (
      window as typeof window & {
        __vpsmanSetTagOrderState: (next: TagOrderState) => void;
      }
    ).__vpsmanSetTagOrderState(state);
  }, incoming);
  await page
    .locator(".fleetPanel > .sectionHeader")
    .getByRole("button", { name: "Refresh" })
    .click();
  await expect(
    editor.getByText("Unsaved changes", { exact: true }),
  ).toBeVisible();
  await editor
    .getByRole("button", { name: "Expand all tag groups" })
    .click();
  const reconciled = (await reorderHandleLabels(editor))
    .filter((label) => !label.endsWith(" tag group"))
    .map((label) => label.replace(/^Reorder /, ""));
  expect(reconciled).toEqual([
    "provider:Zeta",
    "provider:A10",
    "provider:A2",
    "country:US",
    "country:DE",
    "maintenance",
    "provider:Beta",
    "provider:Alpha",
    "provider:C",
    "backup",
  ]);
  expect(await tagOrderUpdates(page)).toHaveLength(0);

  await editor.getByRole("button", { name: "Save order" }).click();
  await expect(editor.getByText("Saved", { exact: true })).toBeVisible();
  expect(await tagOrderUpdates(page)).toEqual([
    {
      namespace_natural_sort_enabled: false,
      ordered_tags: reconciled,
    },
  ]);
});

test("tag order remains contained and touch-draggable on the mobile viewport", async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes("mobile"),
    "mobile containment is covered in the touch viewport",
  );
  const editor = await openTagOrderEditor(page);
  await editor
    .getByRole("button", { name: "Expand provider: tag group" })
    .first()
    .click();
  const handle = editor.getByRole("button", { name: "Reorder provider:Zeta" });
  await expect(handle).toHaveCSS("touch-action", "none");
  const bounds = await editor.boundingBox();
  const viewport = page.viewportSize();
  expect(bounds).not.toBeNull();
  expect(viewport).not.toBeNull();
  expect(bounds?.x ?? -1).toBeGreaterThanOrEqual(0);
  expect((bounds?.x ?? 0) + (bounds?.width ?? 0)).toBeLessThanOrEqual(
    (viewport?.width ?? 0) + 1,
  );
  const overflow = await editor.evaluate(
    (element) => element.scrollWidth - element.clientWidth,
  );
  expect(overflow).toBeLessThanOrEqual(1);
  const preciseRowHeight = await handle.evaluate(
    (button) =>
      button.closest('[role="listitem"]')?.getBoundingClientRect().height ?? 0,
  );
  expect(preciseRowHeight).toBeGreaterThanOrEqual(48);
  expect(preciseRowHeight).toBeLessThanOrEqual(52);
});
