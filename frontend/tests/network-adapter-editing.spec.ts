import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop, waitForConsoleShell } from "./support/consoleNavigation";

const routingId = "44444444-4444-4444-8444-444444444444";
const runtimeId = "33333333-3333-4333-8333-333333333333";

async function adapterAction(page: Page, id: string, action: string) {
  const grid = page.getByLabel("Adapter definitions data grid");
  await grid.getByRole("checkbox", { name: `Select Adapter definitions row ${id}`, exact: true }).check();
  await grid.locator(".gridToolbarActions").getByRole("button", { name: "Actions", exact: true }).click();
  await page.getByRole("menuitem", { name: action, exact: true }).click();
}

async function adapterRequests(page: Page) {
  return page.evaluate(() => (window as typeof window & {
    __vpsmanTestRequests: { networkAdapterMutations: Array<{ action: string; body: Record<string, unknown> }> };
  }).__vpsmanTestRequests.networkAdapterMutations);
}

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Network", "Tunnel plans");
});

test("bound adapter details save only metadata while deletion remains blocked", async ({ page }) => {
  await adapterAction(page, routingId, "Edit details");
  const drawer = page.getByRole("complementary", { name: "Edit details: SFO routing cost" });
  await expect(drawer).toContainText("Saving details does not run commands or restart resources");
  await expect(drawer.getByRole("textbox", { name: "Read cost adapter command", exact: true })).toHaveCount(0);
  await drawer.getByLabel("Adapter definition name").fill("SFO routing metadata");
  await drawer.getByLabel("Adapter definition description").fill("Operators own these commands");
  await drawer.getByRole("button", { name: "Save details", exact: true }).click();
  await expect(drawer).toBeHidden();
  await expect(page.getByLabel("Network adapter definitions")).toContainText("Commands and running resources are unchanged");
  const mutations = await adapterRequests(page);
  expect(mutations).toHaveLength(1);
  expect(mutations[0].action).toBe("metadata");
  expect(Object.keys(mutations[0].body).sort()).toEqual(["description", "expected_updated_at", "name"]);
  expect(mutations[0].body).toMatchObject({ name: "SFO routing metadata", description: "Operators own these commands" });
  const grid = page.getByLabel("Adapter definitions data grid");
  await grid.locator(".gridToolbarActions").getByRole("button", { name: "Actions", exact: true }).click();
  await expect(page.getByRole("menuitem", { name: "Delete", exact: true })).toHaveAttribute("aria-disabled", "true");
});

test("routing command drafts survive refresh and require a fresh privileged review without runtime teardown", async ({ page }) => {
  await adapterAction(page, routingId, "Edit commands");
  const drawer = page.getByRole("complementary", { name: "Edit commands: SFO routing cost" });
  await expect(drawer.getByLabel("Adapter definition name")).toHaveAttribute("readonly", "");
  const command = drawer.getByLabel("Update cost adapter command", { exact: true });
  const changedCommand = `${await command.inputValue()}\n--reviewed`;
  await command.fill(changedCommand);
  await page.getByLabel("Tunnel plans data grid").getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(command).toHaveValue(changedCommand);
  await drawer.getByRole("button", { name: "Review changes", exact: true }).click();
  const review = page.getByLabel("Review adapter command changes", { exact: true });
  await expect(review).toContainText("sfo-fra-gre");
  await expect(review).toContainText("agent-sfo-01");
  await expect(review).toContainText("does not clean up tunnels or immediately apply a cost");
  await expect(review.getByRole("button", { name: "Apply adapter changes", exact: true })).toBeDisabled();
  await review.getByRole("button", { name: "Cancel", exact: true }).click();
  const finalCommand = `${changedCommand}\n--latest-draft`;
  await command.fill(finalCommand);
  await unlockPrivilegeFromTop(page);
  await expect(command).toHaveValue(finalCommand);
  await drawer.getByRole("button", { name: "Review changes", exact: true }).click();
  await review.getByRole("button", { name: "Apply adapter changes", exact: true }).click();
  await expect(drawer).toBeHidden();
  await expect(page.getByLabel("Network adapter definitions")).toContainText("no tunnel cleanup or immediate cost update was queued");
  const mutations = await adapterRequests(page);
  expect(mutations.map((mutation) => mutation.action)).toEqual(["preview", "preview", "update"]);
  expect(mutations[2].body).toMatchObject({
    review_hash: "2".padStart(64, "0"),
    definition: { update_command: { argv: finalCommand.split("\n") } },
    privilege_assertion: expect.objectContaining({}),
  });
  const dispatches = await page.evaluate(() => (window as typeof window & {
    __vpsmanFetchRequests: Array<{ method: string; url: string }>;
  }).__vpsmanFetchRequests.filter((request) => request.method === "POST" && /runtime-config|ospf.*apply/.test(request.url)));
  expect(dispatches).toEqual([]);
});

test("unchanged commands do not request privilege, mutate an adapter, or queue work", async ({ page }) => {
  await adapterAction(page, runtimeId, "Edit commands");
  const drawer = page.getByRole("complementary", { name: "Edit commands: Tunnel lifecycle v1" });
  await drawer.getByRole("button", { name: "Review changes", exact: true }).click();
  await expect(drawer).toContainText("No command changes. No restart or runtime job is needed.");
  await expect(page.getByLabel("Review adapter command changes", { exact: true })).toHaveCount(0);
  expect((await adapterRequests(page)).map((mutation) => mutation.action)).toEqual(["preview"]);
});
