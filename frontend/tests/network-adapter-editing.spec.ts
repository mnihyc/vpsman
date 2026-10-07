import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { networkAdapterDefinitions } from "./support/configurationSourceFixtures";
import { openConsoleSubpage, unlockPrivilegeFromTop, waitForConsoleShell } from "./support/consoleNavigation";

const routingId = "44444444-4444-4444-8444-444444444444";
const runtimeId = "33333333-3333-4333-8333-333333333333";
const forwardingId = "36363636-3636-4636-8636-363636363636";
const forwardingCommands = Object.fromEntries(["apply", "remove", "status"].map((action) => [
  `${action}_command`,
  {
    argv: ["/opt/operator/forwarding", action, "{forwarding_type}", "{rule_config_json}"],
    max_timeout_secs: 60,
    max_output_bytes: 16384,
  },
]));

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
  await installConsoleApiMock(page, {
    networkAdapterDefinitionsOverride: [...networkAdapterDefinitions, {
      adapter_kind: "port_forward",
      id: forwardingId,
      name: "Forwarding commands",
      description: "Operator-owned forwarding commands",
      created_at: "2026-06-02T10:00:00Z",
      updated_at: "2026-06-02T10:00:00Z",
      definition: { contract_version: 1, ...forwardingCommands, pool_capabilities: {} },
    }],
  });
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

test("forwarding command edits submit only the current definition through review", async ({ page }, testInfo) => {
  await adapterAction(page, forwardingId, "Edit commands");
  let drawer = page.getByRole("complementary", { name: "Edit commands: Forwarding commands" });
  await drawer.getByText("Advanced contract preview", { exact: true }).click();
  const expected = { contract_version: 2, ...forwardingCommands };
  expect(JSON.parse(await drawer.locator("pre").innerText())).toEqual(expected);
  for (const [action, label] of [["apply", "Apply"], ["remove", "Remove"], ["status", "Status"]]) {
    await expect(drawer.getByLabel(`${label} adapter command`, { exact: true })).toHaveValue(
      forwardingCommands[`${action}_command`].argv.join("\n"),
    );
    await expect(drawer.getByLabel(`${label} timeout seconds`, { exact: true })).toHaveValue("60");
    await expect(drawer.getByLabel(`${label} maximum output bytes`, { exact: true })).toHaveValue("16384");
  }
  await drawer.getByRole("button", { name: "Close Edit commands: Forwarding commands", exact: true }).click();
  expect(await adapterRequests(page)).toEqual([]);
  await adapterAction(page, forwardingId, "Edit commands");
  drawer = page.getByRole("complementary", { name: "Edit commands: Forwarding commands" });
  const changedApply = {
    ...forwardingCommands.apply_command,
    argv: ["/opt/operator/updated-forwarding", "apply", "{forwarding_type}", "{rule_config_json}"],
    max_timeout_secs: 90,
  };
  await drawer.getByLabel("Apply adapter command", { exact: true }).fill(changedApply.argv.join("\n"));
  await drawer.getByLabel("Apply timeout seconds", { exact: true }).fill("90");
  const savedDefinition = { ...expected, apply_command: changedApply };
  await unlockPrivilegeFromTop(page);
  await drawer.getByRole("button", { name: "Review changes", exact: true }).click();
  const review = page.getByLabel("Review adapter command changes", { exact: true });
  await expect(review).toContainText("No bindings; no runtime resources will be restarted.");
  await review.getByRole("button", { name: "Apply adapter changes", exact: true }).click();
  await expect(drawer).toBeHidden();
  const mutations = await adapterRequests(page);
  expect(mutations.map((mutation) => mutation.action)).toEqual(["preview", "update"]);
  expect(mutations[0].body.definition).toEqual(savedDefinition);
  expect(mutations[1].body).toMatchObject({
    definition: savedDefinition,
    review_hash: "1".padStart(64, "0"),
    privilege_assertion: expect.objectContaining({}),
  });
  await adapterAction(page, forwardingId, "Edit commands");
  await drawer.getByRole("button", { name: "Review changes", exact: true }).click();
  await expect(drawer).toContainText("No command changes. No restart or runtime job is needed.");
  await drawer.getByText("Advanced contract preview", { exact: true }).click();
  expect(JSON.parse(await drawer.locator("pre").innerText())).toEqual(savedDefinition);
  await page.screenshot({ path: testInfo.outputPath("forwarding-command-definition.png"), fullPage: true });
});
