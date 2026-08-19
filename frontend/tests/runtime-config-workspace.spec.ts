import { expect, test, type Locator } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

const mixedBulkApplyTest =
  "completes mixed changed and no-op bulk override apply without waiting for the no-op target";
const bulkPreviewDriftTest =
  "invalidates bulk apply when desired state drifts after the displayed preview";
const singleApplyRefreshFailureTest =
  "keeps a committed VPS override successful when evidence refresh fails";
const shadowedOverrideTest =
  "removes a saved shadowed override without unlocking its preset-owned value";
const storageOnlyBulkApplyTest =
  "presents and applies storage-only bulk targets without a false sync warning";

test.beforeEach(async ({ page }, testInfo) => {
  await installConsoleApiMock(
    page,
    testInfo.title === mixedBulkApplyTest
      ? { runtimeConfigBulkNoOpClientIds: ["agent-fra-02"] }
      : testInfo.title === bulkPreviewDriftTest
        ? { runtimeConfigBulkPreviewStateDrift: true }
        : testInfo.title === singleApplyRefreshFailureTest
          ? {
              runtimeConfigApplyFailure: true,
              runtimeConfigWorkspaceRefreshFailureAfterApply: true,
            }
          : testInfo.title === shadowedOverrideTest
            ? { runtimeConfigShadowedOverrideClientIds: ["agent-sfo-01"] }
            : testInfo.title === storageOnlyBulkApplyTest
              ? { runtimeConfigBulkStorageOnlyClientIds: ["agent-sfo-01"] }
              : undefined,
  );
  await page.goto("/");
  await page.evaluate(() =>
    localStorage.removeItem("vpsman.config.single.tree.expanded"),
  );
  await page.reload();
});

async function chooseVps(root: Locator, query: string, optionName: RegExp) {
  const input = root.getByRole("combobox", { name: "VPS config target" });
  await input.fill(query);
  const option = root.page().locator(".vpsComboboxMenu").getByRole("option", {
    name: optionName,
  });
  await expect(option).toBeVisible();
  await option.click();
}

function configBranch(root: Locator, label: string) {
  return root
    .getByText(label, { exact: true })
    .first()
    .locator(
      "xpath=ancestor::section[contains(concat(' ', normalize-space(@class), ' '), ' singleConfigBranch ')][1]",
    );
}

test("edits the sparse VPS override hierarchy without confusing empty and inherited arrays", async ({
  page,
}) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  const refreshDesired = workspace.getByRole("button", {
    name: "Refresh desired",
  });
  await expect(refreshDesired).toBeDisabled();
  await expect(refreshDesired).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "Select one VPS before refreshing its desired configuration.",
  );
  const refreshLive = workspace.getByRole("button", { name: "Refresh live" });
  await expect(refreshLive).toBeDisabled();
  await expect(refreshLive).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "Load one VPS configuration before refreshing its live state.",
  );
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);

  const cleanReview = workspace.getByRole("button", {
    name: "Review changes",
  });
  await expect(cleanReview).toBeDisabled();
  await expect(cleanReview).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "Edit the VPS configuration before reviewing changes.",
  );

  await expect(
    page.getByRole("heading", { name: "Per-VPS desired config" }),
  ).toBeVisible();
  await expect(workspace).toContainText("revision 4");
  await expect(workspace).toContainText("applied");
  await expect(
    workspace.getByLabel("Telemetry interval", { exact: true }),
  ).toHaveValue("45");
  await expect(workspace).toContainText("Configuration Preset");
  const lockedProcessSource = workspace.getByLabel("Process inventory source");
  await expect(lockedProcessSource).toBeDisabled();
  await expect(lockedProcessSource).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "This value is read-only because its configuration source does not allow a VPS override.",
  );

  const runtimeIp = configBranch(workspace, "Runtime IP arguments");
  await runtimeIp
    .getByRole("button", { name: "Expand Runtime IP arguments" })
    .click();
  await expect(runtimeIp).toContainText("Explicit empty list []");
  const setEmpty = runtimeIp.getByRole("button", { name: "Set []" });
  await expect(setEmpty).toBeDisabled();
  await expect(setEmpty).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "The array is already explicitly empty.",
  );
  await runtimeIp.getByLabel("New array item").fill("/usr/local/sbin/ip");
  await runtimeIp.getByRole("button", { name: "Add item" }).click();
  await expect(runtimeIp.getByLabel("Array item 1")).toHaveValue(
    "/usr/local/sbin/ip",
  );
  const moveFirstUp = runtimeIp.getByLabel("Move item 1 up");
  const moveFirstDown = runtimeIp.getByLabel("Move item 1 down");
  await expect(moveFirstUp).toBeDisabled();
  await expect(moveFirstUp).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "This item is already first.",
  );
  await expect(moveFirstDown).toBeDisabled();
  await expect(moveFirstDown).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "This item is already last.",
  );
  await runtimeIp.getByLabel("Array item 1").fill("");
  await runtimeIp.getByLabel("Array item 1").pressSequentially("/usr/bin/ip");
  await runtimeIp.getByLabel("New array item").fill("123");
  await runtimeIp.getByRole("button", { name: "Add item" }).click();
  await expect(runtimeIp.getByLabel("Array item 2")).toHaveAttribute(
    "type",
    "text",
  );
  await runtimeIp.getByLabel("Array item 2").fill("-force");
  await runtimeIp.getByRole("button", { name: "Move item 2 up" }).click();
  await expect(runtimeIp.getByLabel("Array item 1")).toHaveValue("-force");
  await expect(runtimeIp.getByLabel("Array item 2")).toHaveValue("/usr/bin/ip");
  await runtimeIp.getByRole("button", { name: "Delete item 2" }).click();
  await runtimeIp.getByRole("button", { name: "Delete item 1" }).click();
  await expect(runtimeIp).toContainText("Explicit empty list []");
  await runtimeIp.getByRole("button", { name: "Use inherited" }).click();
  await expect(runtimeIp).toContainText("1 item");

  const environment = configBranch(workspace, "Environment set");
  await environment
    .getByRole("button", { name: "Expand Environment set" })
    .click();
  await expect(environment).toContainText("Locked");
  await expect(
    environment.getByRole("button", { name: "Add field" }),
  ).toHaveCount(0);

  const interval = workspace.getByLabel("Telemetry interval", { exact: true });
  await interval.fill("60");
  await expect(workspace.getByText("Will change").first()).toBeVisible();
  await workspace.getByRole("button", { name: "Refresh live" }).click();
  await expect(workspace).toContainText("matches saved desired");
  await expect(workspace).toContainText("Draft will change saved desired");

  await workspace.getByRole("button", { name: "Review changes" }).click();
  await expect(
    workspace.getByLabel("Reviewed VPS config changes"),
  ).toContainText("Server-reviewed runtime changes");
  await workspace.getByRole("button", { name: "Apply reviewed" }).click();
  const confirmation = page.getByLabel("Confirm VPS runtime override");
  await expect(confirmation).toContainText("runtime changes");
  await confirmation
    .getByRole("button", { name: "Apply VPS override" })
    .click();

  const lastRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: Record<string, unknown>;
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches;
    return requests.at(-1);
  });
  expect(lastRequest?.pathname).toBe(
    "/api/v1/runtime-config/clients/agent-sfo-01/override/apply",
  );
  expect(lastRequest?.body).toMatchObject({
    candidate: {
      type: "structured",
      value: { telemetry_interval_secs: 60 },
    },
    confirmed: true,
    expected_override_revision: "4",
    preview_hash: "e".repeat(64),
  });
  expect(JSON.stringify(lastRequest)).not.toContain("local-super-password");
});

test("reviews deleting the complete sparse override as an explicit reset", async ({
  page,
}) => {
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);
  await workspace.getByRole("tab", { name: "Advanced" }).click();
  await workspace.getByRole("button", { name: "Delete override" }).click();
  await expect(workspace.getByLabel("VPS config sticky review")).toContainText(
    "Delete the saved VPS override",
  );
  await expect(
    workspace.getByLabel("VPS replacement override TOML"),
  ).toHaveValue("");
  await workspace.getByRole("tab", { name: "Tree" }).click();
  await expect(
    workspace.getByLabel("Telemetry interval", { exact: true }),
  ).toHaveValue("30");
  await expect(workspace.getByLabel("VPS config sticky review")).toContainText(
    "Delete the saved VPS override",
  );
  await workspace.getByRole("button", { name: "Review changes" }).click();
  await expect(
    workspace.getByLabel("Reviewed VPS config changes"),
  ).toContainText("Saved override deletion");

  const previewRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: Record<string, unknown>;
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches;
    return requests.find((request) =>
      request.pathname.endsWith("/override/preview"),
    );
  });
  expect(previewRequest?.body).toMatchObject({ candidate: { type: "reset" } });
});

test("uses server truth for last-Inherit and blank Advanced override deletion", async ({
  page,
}) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);

  const runtimeIp = configBranch(workspace, "Runtime IP arguments");
  await runtimeIp.getByRole("button", { name: "Use inherited" }).click();
  const intervalField = workspace
    .getByText("Telemetry interval", { exact: true })
    .first()
    .locator(
      "xpath=ancestor::div[contains(concat(' ', normalize-space(@class), ' '), ' singleConfigField ')][1]",
    );
  await intervalField.getByRole("button", { name: "Use inherited" }).click();
  await workspace.getByRole("button", { name: "Review changes" }).click();
  await expect(workspace.getByLabel("VPS config sticky review")).toContainText(
    "Delete the saved VPS override",
  );
  await expect(
    workspace.getByLabel("Reviewed VPS config changes"),
  ).toContainText("Saved override deletion");
  await workspace.getByRole("button", { name: "Delete reviewed" }).click();
  let confirmation = page.getByLabel("Confirm VPS override deletion");
  await expect(confirmation).toHaveClass(/danger/u);
  await expect(confirmation).toContainText("Delete saved override");
  await expect(
    confirmation.getByRole("button", { name: "Delete VPS override" }),
  ).toBeVisible();
  await confirmation.getByRole("button", { name: "Cancel" }).click();

  await workspace.getByRole("button", { name: "Discard draft" }).click();
  await workspace.getByRole("tab", { name: "Advanced" }).click();
  const editor = workspace.getByLabel("VPS replacement override TOML");
  await editor.fill("\n# remove the saved override\n");
  await editor.blur();
  await expect(editor).not.toHaveAttribute("aria-invalid", "true");
  await workspace.getByRole("button", { name: "Review changes" }).click();
  await expect(
    workspace.getByLabel("Reviewed VPS config changes"),
  ).toContainText("Saved override deletion");
  await workspace.getByRole("button", { name: "Delete reviewed" }).click();
  confirmation = page.getByLabel("Confirm VPS override deletion");
  await expect(confirmation).toHaveClass(/danger/u);
  await expect(confirmation).toContainText("Delete saved override");

  const previewCandidates = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: { candidate?: { type?: string; value?: unknown } };
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches
      .filter((request) => request.pathname.endsWith("/override/preview"))
      .map((request) => request.body.candidate);
  });
  expect(previewCandidates).toMatchObject([
    { type: "structured", value: {} },
    { type: "toml" },
  ]);
});

test(singleApplyRefreshFailureTest, async ({ page }) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);
  await workspace.getByLabel("Telemetry interval", { exact: true }).fill("60");
  await workspace.getByRole("button", { name: "Review changes" }).click();
  await workspace.getByRole("button", { name: "Apply reviewed" }).click();
  await page
    .getByLabel("Confirm VPS runtime override")
    .getByRole("button", { name: "Apply VPS override" })
    .click();

  await expect(
    workspace.locator(".singleConfigWorkspaceFeedback"),
  ).toContainText(
    "VPS override saved and runtime sync queued. Desired state refresh failed; use Refresh desired before editing again.",
  );
  await expect(page.locator(".configActionFeedback")).toHaveCount(0);
  await expect(workspace.getByLabel("VPS config sticky review")).toContainText(
    "No draft changes",
  );
  await expect(workspace.getByLabel("Reviewed VPS config changes")).toHaveCount(
    0,
  );
  await expect(
    workspace.getByLabel("Telemetry interval", { exact: true }),
  ).toHaveValue("60");

  await workspace.getByRole("button", { name: "Refresh desired" }).click();
  await expect(
    workspace.locator(".singleConfigWorkspaceFeedback"),
  ).toContainText("Workspace refreshed");
  await expect(workspace).toContainText("revision 5");
});

test("keeps a completed apply scoped to its original VPS when the target changes", async ({
  page,
}) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);
  await workspace.getByLabel("Telemetry interval", { exact: true }).fill("60");
  await workspace.getByRole("button", { name: "Review changes" }).click();
  await workspace.getByRole("button", { name: "Apply reviewed" }).click();

  await page.evaluate(() => {
    (
      window as unknown as {
        __vpsmanGateNextRuntimeConfigApply: () => void;
      }
    ).__vpsmanGateNextRuntimeConfigApply();
  });
  await page
    .getByLabel("Confirm VPS runtime override")
    .getByRole("button", { name: "Apply VPS override" })
    .click();
  await expect
    .poll(async () =>
      page.evaluate(
        () =>
          (
            window as unknown as {
              __vpsmanTestRequests: {
                runtimeConfigPatches: Array<{ pathname: string }>;
              };
            }
          ).__vpsmanTestRequests.runtimeConfigPatches.filter((request) =>
            request.pathname.endsWith("/override/apply"),
          ).length,
      ),
    )
    .toBe(1);

  await chooseVps(workspace, "core-fra", /core-fra-02.*agent-fra-02/);
  await expect(workspace).toContainText("core-fra-02");
  await expect(workspace).toContainText("inherited only");
  await expect(
    workspace.getByLabel("Telemetry interval", { exact: true }),
  ).toHaveValue("30");

  await page.evaluate(() => {
    (
      window as unknown as {
        __vpsmanReleaseRuntimeConfigApply: () => void;
      }
    ).__vpsmanReleaseRuntimeConfigApply();
  });
  await expect(workspace).toContainText("core-fra-02");
  await expect(workspace).toContainText("inherited only");
  await expect(
    workspace.getByLabel("Telemetry interval", { exact: true }),
  ).toHaveValue("30");
});

test("preserves invalid Advanced TOML exactly and reviews storage-only replacement text", async ({
  page,
}) => {
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);
  await workspace.getByRole("tab", { name: "Advanced" }).click();
  const editor = workspace.getByLabel("VPS replacement override TOML");
  const invalid = "[network]\nruntime_ip_argv = [\n# keep this exact text";
  await editor.fill(invalid);
  await editor.blur();
  await expect(editor).toHaveValue(invalid);
  await expect(editor).toHaveAttribute("aria-invalid", "true");
  await workspace.getByRole("tab", { name: "Tree" }).click();
  await expect(editor).toBeVisible();
  await expect(editor).toHaveValue(invalid);

  const locked = '[telemetry]\nsource = "linux_procfs"\n';
  await editor.fill(locked);
  await editor.blur();
  await expect(editor).toHaveValue(locked);
  await expect(workspace).toContainText(
    "Locked runtime configuration field: telemetry.source",
  );
  await expect(
    workspace.getByRole("button", { name: "Review changes" }),
  ).toBeDisabled();
  await expect(
    workspace.getByRole("button", { name: "Review changes" }),
  ).toHaveAttribute(
    "data-tooltip-disabled-reason",
    "Repair the replacement TOML before reviewing changes.",
  );

  const unknown = "made_up_runtime_field = true\n";
  await editor.fill(unknown);
  await editor.blur();
  await expect(editor).toHaveValue(unknown);
  await expect(workspace).toContainText(
    "Unknown runtime configuration field: made_up_runtime_field",
  );

  const storageOnly =
    "telemetry_interval_secs = 45\n\n[network]\nruntime_ip_argv = []\n\n# operator note\n";
  await editor.fill(storageOnly);
  await editor.blur();
  await expect(editor).not.toHaveAttribute("aria-invalid", "true");
  await workspace.getByRole("button", { name: "Review changes" }).click();
  await expect(
    workspace.getByLabel("Reviewed VPS config changes"),
  ).toContainText("Storage-only review");
  await expect(editor).toHaveValue(storageOnly);
});

test("keeps bulk VPS override patches Advanced-only and previews deletion directives per VPS", async ({
  page,
}) => {
  await openConsoleSubpage(page, "Config", "VPS override patch");
  await expect(
    page.getByRole("heading", { name: "VPS override patch" }),
  ).toBeVisible();
  await expect(page.getByText("Advanced · VPS override patch")).toBeVisible();
  await expect(page.getByRole("tab", { name: "Tree" })).toHaveCount(0);
  await page.getByRole("button", { name: "Temporary patch" }).click();
  await page
    .getByLabel("Temporary bulk runtime config patch TOML")
    .fill("-telemetry_interval_secs\n-[update]\n-network.latency_down_windows");
  await page.getByLabel("Bulk patch target expression").fill("id:agent-sfo-01");
  await page.getByRole("button", { name: "Preview changes" }).click();
  await expect(page.getByText(/Apply submits exactly this set/)).toBeVisible();
  await expect(page.getByLabel("Bulk patch change summary")).toContainText(
    "1 change",
  );
  await expect(page.getByLabel("Bulk patch change summary")).toContainText(
    "edge-sfo-01",
  );
  await expect(
    page
      .getByLabel("Bulk patch change summary")
      .locator(".bulkPatchPreviewMeta"),
  ).toContainText("network");

  const previewRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: { patch?: string };
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches;
    return requests.find((request) =>
      request.pathname.endsWith("/bulk/preview"),
    );
  });
  expect(previewRequest?.body.patch).toContain("-telemetry_interval_secs");
  expect(previewRequest?.body.patch).toContain("-[update]");
  expect(previewRequest?.body.patch).toContain("-network.latency_down_windows");
  expect(
    await page.evaluate(
      () =>
        (
          window as unknown as {
            __vpsmanTestRequests: { bulkResolve: unknown[] };
          }
        ).__vpsmanTestRequests.bulkResolve.length,
    ),
  ).toBe(0);
});

test(mixedBulkApplyTest, async ({ page }) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "VPS override patch");
  const bulk = page.locator(".configApplyGrid");

  await bulk.getByRole("button", { name: "Temporary patch" }).click();
  await bulk
    .getByLabel("Temporary bulk runtime config patch TOML")
    .fill("telemetry_interval_secs = 60");
  await bulk
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("status:online");
  await bulk.getByRole("button", { name: "Preview changes" }).click();

  await expect(bulk.getByText("2 VPSs verified")).toBeVisible();
  const summary = bulk.getByLabel("Bulk patch change summary");
  await expect(summary).toContainText("1 VPS change; 1 VPS no-op");
  await expect(summary).toContainText("core-fra-02");
  await expect(summary).toContainText("No change");

  await bulk.getByRole("button", { name: "Apply override patch" }).click();
  const confirmation = page.getByLabel("Confirm VPS override patch");
  await expect(confirmation).toContainText("2");
  const previewRequests = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: { target_client_ids?: string[] };
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches.filter((request) =>
      request.pathname.endsWith("/bulk/preview"),
    );
  });
  expect(previewRequests).toHaveLength(2);
  expect(previewRequests[0].body.target_client_ids).toEqual([]);
  expect(previewRequests[1].body.target_client_ids).toEqual([
    "agent-fra-02",
    "agent-sfo-01",
  ]);
  await confirmation
    .getByRole("button", { name: "Apply VPS override patch" })
    .click();

  const result = page.getByLabel("Execution result");
  await expect(result).toContainText("completed on 1 VPS");
  await expect(result).toContainText("1/1");
  await expect(
    result.getByRole("button", { name: "Clear results" }),
  ).toBeEnabled();
});

test(bulkPreviewDriftTest, async ({ page }) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "VPS override patch");
  const bulk = page.locator(".configApplyGrid");

  await bulk.getByRole("button", { name: "Temporary patch" }).click();
  await bulk
    .getByLabel("Temporary bulk runtime config patch TOML")
    .fill("telemetry_interval_secs = 60");
  await bulk
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await bulk.getByRole("button", { name: "Preview changes" }).click();
  await expect(bulk.getByText("1 VPS verified")).toBeVisible();

  await bulk.getByRole("button", { name: "Apply override patch" }).click();
  await expect(page.locator(".configActionFeedback")).toContainText(
    "Desired or override state changed since the displayed preview; preview changes again",
  );
  await expect(page.getByLabel("Confirm VPS override patch")).toHaveCount(0);
  await expect(
    bulk.getByRole("button", { name: "Apply override patch" }),
  ).toBeDisabled();

  const requests = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: { target_client_ids?: string[] };
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches;
  });
  expect(
    requests.filter((request) => request.pathname.endsWith("/bulk/preview")),
  ).toHaveLength(2);
  expect(requests[1].body.target_client_ids).toEqual(["agent-sfo-01"]);
  expect(
    requests.some((request) => request.pathname.endsWith("/bulk/apply")),
  ).toBe(false);

  await bulk.getByRole("button", { name: "Preview changes" }).click();
  await expect(bulk.getByText("1 VPS verified")).toBeVisible();
  await expect(
    bulk.getByRole("button", { name: "Apply override patch" }),
  ).toBeEnabled();
});

test("uses hash-routed owner links instead of leaving the console", async ({
  page,
}) => {
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);

  await workspace
    .getByRole("link", { name: /Configuration Preset/ })
    .first()
    .click();
  await expect(page).toHaveURL(/#\/config\/sources$/u);
  await expect(
    page.getByRole("heading", { name: "Configuration sources" }),
  ).toBeVisible();
});

test(shadowedOverrideTest, async ({ page }) => {
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);

  const source = workspace
    .getByLabel("Source", { exact: true })
    .locator(
      "xpath=ancestor::div[contains(concat(' ', normalize-space(@class), ' '), ' singleConfigField ')][1]",
    );
  await expect(source).toContainText("Override shadowed");
  await expect(source.getByLabel("Source", { exact: true })).toBeDisabled();
  await source.getByRole("button", { name: "Use inherited" }).click();
  await expect(source.getByLabel("Source", { exact: true })).toHaveValue(
    "linux_procfs",
  );
  await expect(source.getByLabel("Source", { exact: true })).toBeDisabled();

  await workspace.getByRole("button", { name: "Review changes" }).click();
  const previewRequest = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          runtimeConfigPatches: Array<{
            body: Record<string, unknown>;
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.runtimeConfigPatches.find((request) =>
      request.pathname.endsWith("/override/preview"),
    );
  });
  expect(previewRequest?.body).toMatchObject({
    candidate: {
      type: "structured",
      value: {
        network: { runtime_ip_argv: [] },
        telemetry_interval_secs: 45,
      },
    },
  });
});

test("remembers explicit Collapse All and keeps search expansion temporary", async ({
  page,
}) => {
  await page.evaluate(() =>
    localStorage.setItem("vpsman.config.single.tree.expanded", "[]"),
  );
  await page.reload();
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);
  const topLevelDisclosures = workspace.locator(
    ".singleConfigTree > .singleConfigBranch > .singleConfigBranchHeader > .singleConfigDisclosure",
  );
  await expect(topLevelDisclosures.first()).toHaveAttribute(
    "aria-expanded",
    "false",
  );

  await workspace
    .getByLabel("Search VPS runtime config fields")
    .fill("process inventory");
  await expect(workspace.getByLabel("Process inventory source")).toBeVisible();
  await expect(
    workspace.getByRole("button", {
      name: "Search expands matching config sections",
    }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() =>
      localStorage.getItem("vpsman.config.single.tree.expanded"),
    ),
  ).toBe("[]");

  await workspace.getByLabel("Search VPS runtime config fields").fill("");
  await expect(topLevelDisclosures.first()).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  await page.reload();
  await expect(
    page
      .locator(".singleConfigTree > .singleConfigBranch")
      .first()
      .getByRole("button", { name: /^Expand /u }),
  ).toBeVisible();
});

test(storageOnlyBulkApplyTest, async ({ page }) => {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "VPS override patch");
  const bulk = page.locator(".configApplyGrid");
  await bulk.getByRole("button", { name: "Temporary patch" }).click();
  await bulk
    .getByLabel("Temporary bulk runtime config patch TOML")
    .fill("telemetry_interval_secs = 45\n# operator note");
  await bulk
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await bulk.getByRole("button", { name: "Preview changes" }).click();
  const summary = bulk.getByLabel("Bulk patch change summary");
  await expect(summary).toContainText("1 VPS stored TOML only");
  await expect(summary).toContainText("Stored TOML only");

  await bulk.getByRole("button", { name: "Apply override patch" }).click();
  await page
    .getByLabel("Confirm VPS override patch")
    .getByRole("button", { name: "Apply VPS override patch" })
    .click();
  const feedback = bulk.locator(".configReviewFeedback");
  await expect(feedback).toContainText("no runtime sync was required");
  await expect(feedback).toHaveClass(/actionFeedbackSuccess/u);
  await expect(page.getByLabel("Execution result")).toHaveCount(0);
});

test("keeps the 320px Config Overview document within its local table scroller", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 700 });
  await openConsoleSubpage(page, "Config", "Overview");
  const recentChanges = page.getByLabel("Recent config changes");
  await recentChanges.locator("summary").click();
  const historyTable = recentChanges.getByLabel("Recent config change records");
  await expect(historyTable).toBeVisible();
  expect(
    await historyTable.evaluate((element) => ({
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
    })),
  ).toMatchObject({ clientWidth: expect.any(Number) });
  expect(
    await historyTable.evaluate(
      (element) => element.scrollWidth > element.clientWidth,
    ),
  ).toBe(true);
  expect(
    await page.evaluate(() => ({
      clientWidth: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
    })),
  ).toEqual({ clientWidth: 320, scrollWidth: 320 });
});

test("keeps dirty review actions in flow at 320px", async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 700 });
  await openConsoleSubpage(page, "Config", "Per-VPS");
  const workspace = page.locator(".singleConfigWorkspace");
  await chooseVps(workspace, "edge-sfo", /edge-sfo-01.*agent-sfo-01/);
  await workspace.getByLabel("Telemetry interval", { exact: true }).fill("60");

  const review = workspace.getByLabel("VPS config sticky review");
  await expect(review).toHaveCSS("position", "static");
  const refreshBox = await workspace
    .getByRole("button", { name: "Refresh live" })
    .boundingBox();
  const reviewBox = await review.boundingBox();
  expect(refreshBox).not.toBeNull();
  expect(reviewBox).not.toBeNull();
  expect(reviewBox!.y).toBeGreaterThanOrEqual(
    refreshBox!.y + refreshBox!.height,
  );
});
