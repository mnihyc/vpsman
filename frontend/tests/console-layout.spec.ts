import { expect, test, type Locator } from "@playwright/test";
import {
  backupId,
  installConsoleApiMock,
  ospfUpdatePlans,
  tunnelPlans,
} from "./support/consoleLayoutFixtures";
import { DEFAULT_UPDATE_VERSION_URL } from "../src/jobDispatchPreset";
import {
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function checkControl(locator: Locator) {
  await locator.evaluate((element) => {
    const input = element as HTMLInputElement;
    if (!input.checked) {
      input.click();
    }
  });
}

async function selectGridRow(
  page: import("@playwright/test").Page,
  title: string,
  rowId: string,
) {
  const grid = page.getByLabel(`${title} data grid`);
  await grid.getByLabel(`Select ${title} row ${rowId}`).check();
}

async function unselectGridRow(
  page: import("@playwright/test").Page,
  title: string,
  rowId: string,
) {
  const grid = page.getByLabel(`${title} data grid`);
  await grid.getByLabel(`Select ${title} row ${rowId}`).uncheck();
}

async function runGridAction(
  page: import("@playwright/test").Page,
  title: string,
  action: string,
) {
  const grid = page.getByLabel(`${title} data grid`);
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("menuitem", { name: action }).click();
}

async function openDeleteVpsReview(page: import("@playwright/test").Page) {
  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const backupRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await backupRow.getByLabel("Select VPS instance records row").check();
  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
}

async function chooseVpsBySearch(
  root: Locator,
  label: string,
  query: string,
  optionName: RegExp,
) {
  await root.getByRole("combobox", { name: label }).fill(query);
  const option = root.getByRole("option", { name: optionName });
  await expect(option).toBeVisible();
  await option.click();
}

async function dispatchWithPrompt(composer: Locator) {
  const reviewButton = composer.getByRole("button", {
    name: "Dispatch",
  });
  await expect(reviewButton).toBeEnabled();
  await activate(reviewButton);
  await expect(composer.getByText("Confirm job dispatch")).toBeVisible({
    timeout: 15_000,
  });
  await activate(
    composer
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Dispatch job" }),
  );
}

async function confirmVisiblePrompt(
  page: import("@playwright/test").Page,
  label: string,
) {
  const prompt = page.locator(".confirmationPrompt").last();
  await expect(prompt).toBeVisible();
  await expect
    .poll(() =>
      prompt.evaluate((element) => document.activeElement === element),
    )
    .toBe(true);
  const viewport = page.viewportSize();
  await expect
    .poll(async () => {
      const box = await prompt.boundingBox();
      if (!box || !viewport) {
        return false;
      }
      return box.y >= 0 && box.y + box.height <= viewport.height;
    })
    .toBe(true);
  await activate(prompt.getByRole("button", { name: label, exact: true }));
}

async function unlockPrivilegeFor(
  page: import("@playwright/test").Page,
  view: string,
  subpage: string,
) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, view, subpage);
}

function expectPrivilegeAssertion(request: unknown) {
  expect((request as { envelope?: unknown }).envelope).toBeUndefined();
  expect((request as { envelopes?: unknown }).envelopes).toBeUndefined();
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
}

async function openFleetFromDashboard(page: import("@playwright/test").Page) {
  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { name: "Fleet instances" }),
  ).toBeVisible();
}

test("renders an operational cloud-console fleet workspace", async ({
  page,
}, testInfo) => {
  await page.goto("/");

  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fleet command home" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home quick actions")).toBeVisible();
  await expect(page.getByLabel("Home posture strip")).toContainText("Online");
  await expect(
    page.getByRole("heading", { name: "Running work" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Recent failures" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Needs attention" }),
  ).toBeVisible();
  await expect(
    page
      .locator(".homeReviewPanel")
      .filter({ has: page.getByRole("heading", { name: "Running work" }) })
      .getByRole("button", { name: /3 fleet jobs running/ }),
  ).toBeVisible();
  await expect(
    page
      .locator(".homeReviewPanel")
      .filter({ has: page.getByRole("heading", { name: "Recent failures" }) })
      .getByRole("button", { name: /Tunnel adapter status failed/ }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Home telemetry widgets"),
  ).toHaveCount(0);
  if (testInfo.project.name.includes("mobile")) {
    await openFleetFromDashboard(page);
  } else {
    await openConsoleSubpage(page, "Fleet", "Instances");
  }

  await expect(
    page.getByRole("heading", { name: "Fleet instances" }),
  ).toBeVisible();
  if (testInfo.project.name.includes("desktop")) {
    await expect(
      page.getByRole("searchbox", { name: "Search fleet" }),
    ).toBeVisible();
  }
  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const edgeRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await expect(edgeRow).toBeVisible();
  await expect(edgeRow).toContainText("edge-sfo-01 (fo01)");
  for (const column of [
    "VPS",
    "State",
    "IP",
    "Last contact",
    "Agent",
    "CPU",
    "Memory",
    "Disk",
    "Alerts",
    "Action",
  ]) {
    await expect(
      fleetGrid
        .locator(".gridHeaderCell")
        .filter({ hasText: new RegExp(`^${column}$`) })
        .first(),
    ).toBeVisible();
  }
  await expect(page.getByText("Console stream connected")).toBeVisible();
  await expect(edgeRow).not.toContainText("alpha");
  await expect(edgeRow).not.toContainText("agent-sfo-01");
  if (testInfo.project.name.includes("desktop")) {
    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await openConsoleSubpage(page, "System", "Preferences");
    await expect(
      page.getByRole("heading", { name: "System preferences", exact: true }),
    ).toBeVisible();
    const preferencesScope = page.getByLabel("Preferences scope overview");
    await expect(preferencesScope).toContainText("Personal display");
    await expect(preferencesScope).toContainText("Browser state");
    await expect(preferencesScope).toContainText("System-linked defaults");
    await expect(page.getByLabel("Personal display preferences")).toContainText(
      "Bulk execution summaries",
    );
    await expect(page.getByLabel("Personal display preferences")).toContainText(
      "Binary exact compares bytes",
    );
    await expect(page.getByLabel("Personal display preferences")).toContainText(
      "Home chart presentation",
    );
    await page.getByRole("button", { name: /Browser state/ }).click();
    await expect(page.getByLabel("Local browser state")).toContainText(
      "Local console selections",
    );
    await page.getByRole("button", { name: /System-linked defaults/ }).click();
    await expect(page.getByLabel("System-linked defaults")).toContainText(
      "Gateway install material",
    );
    await expect(page.getByLabel("System-linked defaults")).toContainText(
      "Tunnel allocation pools",
    );
    await expect(page.getByLabel("System-linked defaults")).not.toContainText(
      "Server public key hex",
    );
    await page.getByRole("button", { name: /Personal display/ }).click();
    await expect(
      page.getByLabel("Reset VPS name format to default"),
    ).toBeVisible();
    await page.getByLabel("Name display").selectOption("name");
    await page
      .getByLabel("Bulk output comparison default")
      .selectOption("text");
    await page.getByRole("button", { name: "Save preferences" }).click();
    const savedPreferences = await page.evaluate(() => {
      const requests = (
        window as unknown as {
          __vpsmanTestRequests: { operatorPreferences: unknown[] };
        }
      ).__vpsmanTestRequests;
      return requests.operatorPreferences.at(-1);
    });
    expect(savedPreferences).toMatchObject({
      bulk_output_compare_mode: "text",
      vps_name_display_mode: "name",
    });
    await nav.getByRole("button", { name: "Fleet", exact: true }).click();
    await expect(edgeRow).toContainText("edge-sfo-01");
    await expect(edgeRow).not.toContainText("(fo01)");
    await openConsoleSubpage(page, "System", "Preferences");
    await page.getByLabel("Name display").selectOption("name_id_suffix");
    await page
      .getByLabel("Bulk output comparison default")
      .selectOption("binary");
    await page.getByRole("button", { name: "Save preferences" }).click();
    await nav.getByRole("button", { name: "Fleet", exact: true }).click();
  }
  await expect(
    page.locator(".consoleHeader").getByText("0 online / 3 total"),
  ).toBeVisible();
  await expect(page.getByText("VPS instances")).toBeVisible();
  await expect(fleetGrid).toContainText("Contact unknown");
  await expect(page.getByLabel("Fleet alerts")).toHaveCount(0);
  if (testInfo.project.name.includes("desktop")) {
    await openConsoleSubpage(page, "Fleet", "Alerts");
    await expect(
      page.getByLabel("Fleet alerts", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Tunnel adapter status failed")).toBeVisible();
    await expect(page.getByText("Agent is not online")).toBeVisible();
    await openConsoleSubpage(page, "Fleet", "Instances");
  }

  const coreRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();
  await activate(coreRow);
  await expect(
    page.getByRole("heading", { level: 1, name: "Instance detail" }),
  ).toBeVisible();
  const coreDetail = page.getByLabel("Canonical VPS detail");
  await expect(coreDetail).toContainText("core-fra-02");
  await expect(coreDetail).toContainText("agent-fra-02");
  await expect(coreDetail).toContainText("Contact unknown");
  await expect(coreDetail).toContainText(
    "Registered as online, but no last contact has been reported by the gateway.",
  );

  await activate(coreDetail.getByRole("tab", { name: "Network" }));
  await expect(
    coreDetail.getByRole("tabpanel", { name: "Network tab" }),
  ).toBeVisible();
  await expect(coreDetail).toContainText("Network workflow");
  await expect(
    coreDetail.getByRole("button", { name: "Open network graph" }),
  ).toBeVisible();
  await expect(
    coreDetail.getByRole("button", { name: "Open network evidence" }),
  ).toBeVisible();
  await expect(coreDetail).toContainText("Latest observations");

  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet instances" }),
  ).toBeVisible();
});

test("deletes a VPS through grid actions and explicit confirmation", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "delete confirmation layout is covered in desktop grid actions",
  );

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Fleet", "Instances");

  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const backupRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await expect(backupRow.getByRole("button", { name: /Open .* detail/ })).toBeVisible();

  await backupRow.getByLabel("Select VPS instance records row").check();
  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review VPS deletion" }),
  ).toBeVisible();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  const prompt = page.locator(".fleetInstancesPanel > .confirmationPrompt");
  await expect(prompt.getByText("Delete VPS from panel")).toBeVisible();
  await expect(prompt).toContainText("deactivates VPS access immediately");
  await activate(prompt.getByRole("button", { name: "Cancel" }));
  await expect(
    fleetGrid.locator(".gridBody [role=row]", { hasText: "backup-nyc-03" }),
  ).toBeVisible();

  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review VPS deletion" }),
  ).toBeVisible();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  await activate(prompt.getByRole("button", { name: "Delete VPS" }));
  await expect(
    fleetGrid.locator(".gridBody [role=row]", { hasText: "backup-nyc-03" }),
  ).toHaveCount(0);
  await expect(
    page.locator(".consoleHeader").getByText("0 online / 2 total"),
  ).toBeVisible();

  const deleteRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { agentDeletes: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.agentDeletes.at(-1);
  });
  expect(deleteRequest).toMatchObject({
    confirmed: true,
    reason: "Deleted from fleet inventory selection action",
  });
  expectPrivilegeAssertion(deleteRequest);
});

test("review prompt display mode follows operator preference", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "grid action prompt display mode is covered in desktop layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "System", "Preferences");
  await expect(page.getByLabel("Personal display preferences")).toContainText(
    "Review prompts",
  );
  await page.getByLabel("Review prompt display mode").selectOption("overlay");
  await page.getByRole("button", { name: "Save preferences" }).click();
  let savedPreferences = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { operatorPreferences: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.operatorPreferences.at(-1);
  });
  expect(savedPreferences).toMatchObject({ review_prompt_mode: "overlay" });

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Fleet", "Instances");
  await openDeleteVpsReview(page);
  await expect(
    page.getByRole("dialog", { name: "Delete VPS from panel" }),
  ).toBeVisible();
  await expect(page.locator(".confirmationPromptOverlay")).toBeVisible();
  await expect(
    page.locator(".fleetInstancesPanel > .confirmationPrompt"),
  ).toHaveCount(0);
  await activate(page.getByRole("button", { name: "Cancel" }));

  await openConsoleSubpage(page, "System", "Preferences");
  await page.getByLabel("Review prompt display mode").selectOption("inline");
  await page.getByRole("button", { name: "Save preferences" }).click();
  savedPreferences = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { operatorPreferences: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.operatorPreferences.at(-1);
  });
  expect(savedPreferences).toMatchObject({ review_prompt_mode: "inline" });

  await openConsoleSubpage(page, "Fleet", "Instances");
  await openDeleteVpsReview(page);
  await expect(
    page.getByRole("region", { name: "Delete VPS from panel" }),
  ).toBeVisible();
  await expect(page.locator(".confirmationPromptOverlay")).toHaveCount(0);
});

test("reviews notification and webhook queue mutations before commit", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "queue mutation confirmations are covered in the desktop notifications panel",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Notifications");
  const notifications = page.locator("main");

  await activate(
    notifications.getByRole("button", { name: "Review queue dispatch" }),
  );
  await expect(
    notifications.getByLabel("Confirm notification queue dispatch"),
  ).toBeVisible();
  await activate(
    notifications
      .getByLabel("Confirm notification queue dispatch")
      .getByRole("button", { name: "Queue dispatch" }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: {
              fleetAlertNotificationDispatches: Array<Record<string, unknown>>;
            };
          }
        ).__vpsmanTestRequests;
        return requests.fleetAlertNotificationDispatches.at(-1);
      }),
    )
    .toMatchObject({ confirmed: true, dry_run: false });

  await activate(
    notifications.getByRole("button", { name: "Review delivery" }),
  );
  await expect(
    notifications.getByLabel("Confirm notification delivery"),
  ).toBeVisible();
  await activate(
    notifications
      .getByLabel("Confirm notification delivery")
      .getByRole("button", { name: "Deliver queued" }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: {
              fleetAlertNotificationProcesses: Array<Record<string, unknown>>;
            };
          }
        ).__vpsmanTestRequests;
        return requests.fleetAlertNotificationProcesses.at(-1);
      }),
    )
    .toMatchObject({ confirmed: true, dry_run: false });

  await activate(notifications.getByRole("tab", { name: "Webhooks" }));
  await expect(
    notifications.getByText("Webhook rules", { exact: true }).first(),
  ).toBeVisible();
  await activate(
    notifications.getByRole("button", { name: "Create rule" }).first(),
  );
  const webhookExpression = notifications.getByRole("searchbox", {
    name: "Webhook expression",
  });
  await webhookExpression.click();
  await webhookExpression.fill("");
  await page.keyboard.type("interval.");
  await expect(
    page.getByRole("option", { name: /^interval\.30sec$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(webhookExpression).toContainText("interval.30sec");
  await activate(notifications.getByLabel("Close detail panel"));

  await activate(
    notifications.getByRole("button", { name: "Review queue dispatch" }),
  );
  await expect(
    notifications.getByLabel("Confirm webhook queue dispatch"),
  ).toBeVisible();
  await activate(
    notifications
      .getByLabel("Confirm webhook queue dispatch")
      .getByRole("button", { name: "Queue dispatch" }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: {
              webhookRuleDispatches: Array<Record<string, unknown>>;
            };
          }
        ).__vpsmanTestRequests;
        return requests.webhookRuleDispatches.at(-1);
      }),
    )
    .toMatchObject({ confirmed: true, dry_run: false });

  await activate(
    notifications.getByRole("button", { name: "Review delivery" }),
  );
  await expect(
    notifications.getByLabel("Confirm webhook delivery"),
  ).toBeVisible();
  await activate(
    notifications
      .getByLabel("Confirm webhook delivery")
      .getByRole("button", { name: "Deliver queued" }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: {
              webhookRuleProcesses: Array<Record<string, unknown>>;
            };
          }
        ).__vpsmanTestRequests;
        return requests.webhookRuleProcesses.at(-1);
      }),
    )
    .toMatchObject({ confirmed: true, dry_run: false });
});

test("clears browser-local console selections without deleting vault records", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "local reset control is covered in the desktop preferences layout",
  );

  await page.goto("/");
  await page.evaluate(() => {
    window.localStorage.setItem("vpsman.authVault", "preserved-auth");
    window.localStorage.setItem("vpsman.privilegeVault", "preserved-privilege");
    window.localStorage.setItem(
      "vpsman.dashboardPreferences",
      JSON.stringify({
        groupBy: "countries",
        pointDensity: "dense",
        refreshIntervalSecs: 5,
      }),
    );
    window.localStorage.setItem(
      "vpsman.sidebarSubpanels",
      JSON.stringify({ state: { Jobs: true } }),
    );
    window.localStorage.setItem(
      "vpsman.grid.example",
      JSON.stringify({ pageSize: 50 }),
    );
  });

  await openConsoleSubpage(page, "System", "Preferences");
  await expect(
    page.getByRole("heading", { name: "Operator preferences" }),
  ).toBeVisible();
  await page.getByRole("button", { name: /Browser state/ }).click();
  const reloaded = page.waitForEvent("load");
  await page.getByRole("button", { name: "Clear local selections" }).click();
  await reloaded;
  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fleet command home" }),
  ).toBeVisible();

  const storage = await page.evaluate(() => ({
    authVault: window.localStorage.getItem("vpsman.authVault"),
    dashboardPreferences: window.localStorage.getItem(
      "vpsman.dashboardPreferences",
    ),
    grid: window.localStorage.getItem("vpsman.grid.example"),
    privilegeVault: window.localStorage.getItem("vpsman.privilegeVault"),
    sidebarSubpanels: window.localStorage.getItem("vpsman.sidebarSubpanels"),
  }));
  expect(storage).toMatchObject({
    authVault: "preserved-auth",
    dashboardPreferences: null,
    grid: null,
    privilegeVault: "preserved-privilege",
    sidebarSubpanels: null,
  });
});

test("scopes duplicate sidebar subpage labels to their parent view", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop sidebar state is not visible in the mobile layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "System", "Preferences");
  await page.getByLabel("Default expansion").selectOption("all");
  await page.getByRole("button", { name: "Save preferences" }).click();
  await expect(
    page.locator(".consoleStatusBadge", { hasText: /^Saved$/ }),
  ).toBeVisible();

  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  const fleetAlertPolicies = nav
    .getByLabel("Fleet sections")
    .getByRole("button", { name: "Alert policies", exact: true });
  const backupPolicies = nav
    .getByLabel("Backups sections")
    .getByRole("button", { name: "Policies", exact: true });

  await openConsoleSubpage(page, "Fleet", "Alert policies");
  await expect(fleetAlertPolicies).toHaveAttribute("aria-current", "page");
  await expect(backupPolicies).not.toHaveAttribute("aria-current", "page");
  await expect(backupPolicies).not.toHaveClass(/active/);

  await backupPolicies.click();
  await expect(
    page.getByRole("heading", { name: "Backup policies" }),
  ).toBeVisible();
  await expect(backupPolicies).toHaveAttribute("aria-current", "page");
  await expect(fleetAlertPolicies).not.toHaveAttribute("aria-current", "page");
  await expect(fleetAlertPolicies).not.toHaveClass(/active/);
});

test("supports interactive fleet data grid controls", async ({
  page,
}, testInfo) => {
  await page.goto("/");
  if (testInfo.project.name.includes("mobile")) {
    await openFleetFromDashboard(page);
  } else {
    await openConsoleSubpage(page, "Fleet", "Instances");
  }

  const grid = page.getByLabel("VPS instance records data grid");
  await expect(grid.getByText("3 of 3 instances")).toBeVisible();
  await grid.getByLabel("VPS instance records search").fill("fra");
  await expect(grid.getByText("1 of 3 instances")).toBeVisible();
  await expect(
    grid.locator("[role=row]", { hasText: "core-fra-02" }),
  ).toBeVisible();
  await grid.getByLabel("VPS instance records search").fill("");

  const coreRow = grid
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();
  await coreRow.getByLabel("Select VPS instance records row").check();
  await expect(grid.getByText("1 selected", { exact: true })).toBeVisible();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Copy client IDs" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");

  await grid.getByLabel("VPS instance records columns").click();
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toHaveCount(0);
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toBeVisible();
  await page.keyboard.press("Escape");

  await coreRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await page.getByRole("menuitem", { name: "Open detail" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Instance detail" }),
  ).toBeVisible();
  await expect(page.getByLabel("Canonical VPS detail")).toContainText(
    "core-fra-02",
  );
});

test("exposes traffic columns and the VPS Traffic & Rules drilldown", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "column chooser and expanded traffic drilldown are covered in desktop navigation",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  await expect(
    grid.getByRole("columnheader", { name: /Traffic Now/ }),
  ).toHaveCount(0);
  for (const columnName of ["Traffic Now", "Cycle Usage", "Traffic State"]) {
    await grid.getByLabel("VPS instance records columns").click();
    await page.getByRole("menuitemcheckbox", { name: columnName }).click();
  }
  await expect(
    grid.getByRole("columnheader", { name: /Traffic Now/ }),
  ).toBeVisible();
  await expect(
    grid.getByRole("columnheader", { name: /Cycle Usage/ }),
  ).toBeVisible();
  await expect(
    grid.getByRole("columnheader", { name: /Traffic State/ }),
  ).toBeVisible();

  const edgeRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await edgeRow.getByLabel("Expand VPS instance records row").click();
  const edgeDetail = grid
    .locator(".gridExpandedRow", { hasText: "edge-sfo-01" })
    .first();
  await edgeDetail.getByRole("tab", { name: "Traffic & Rules" }).click();
  await expect(
    edgeDetail.getByRole("heading", { name: "Traffic & Rules" }),
  ).toBeVisible();
  await expect(edgeDetail).toContainText("traffic.reset_day");
  await expect(edgeDetail).toContainText("traffic.quota.total");
  await expect(edgeDetail).toContainText("eth0+tx,ens3");
  await expect(edgeDetail).toContainText("Selected traffic");
  await expect(edgeDetail).toContainText("Latest RX");
  await expect(edgeDetail).toContainText("Cycle Total");
  await expect(edgeDetail).toContainText("Matched policies");
  await expect(edgeDetail).toContainText("Recent policy alerts");
  await expect(edgeDetail).toContainText("edge-resource-policy");
  await expect(edgeDetail).toContainText("80% total quota");

  await edgeDetail.getByRole("button", { name: "Open Alert Policy" }).click();
  await expect(
    page.getByRole("heading", { name: "Alert policies" }),
  ).toBeVisible();
  await expect(
    page.locator(".consoleDetailPanelHeader strong", {
      hasText: "Alert policy details",
    }),
  ).toBeVisible();
  await expect(page.locator(".consoleDetailPanel").last()).toContainText(
    "edge-resource-policy",
  );
});

test("supports Config VPS Rules dry-run, confirm, and explicit unset", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "VPS Rules bulk editor is covered in desktop layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Rules");

  await expect(page.getByRole("heading", { name: "VPS Rules" })).toBeVisible();
  const grid = page.getByLabel("VPS rule values data grid");
  await expect(grid.getByText("3 of 3 rules")).toBeVisible();
  await expect(grid).toContainText("traffic.reset_day");
  await expect(grid).toContainText("traffic.selectors");
  const alertContext = page.getByLabel("Affected alert policy context");
  await expect(alertContext).toContainText("edge-resource-policy");
  await expect(alertContext).toContainText("80% total quota");
  await expect(alertContext).toContainText(
    "traffic.cycle.total >= traffic.quota.total * 0.8",
  );
  await expect(
    alertContext.getByRole("button", { name: "Open Observability alerts" }),
  ).toBeVisible();

  const editor = page.locator(".consoleDetailPanel", {
    hasText: "Bulk rule editor",
  });
  await expect(editor.getByText("Common rule cards")).toBeVisible();
  await expect(editor.getByText("Advanced raw key/value")).toBeVisible();
  await expect(
    editor.getByRole("button", { name: "Preview changes", exact: true }),
  ).toHaveCount(1);
  await expect(
    editor.getByRole("button", { name: "Preview unset" }),
  ).toHaveCount(0);
  await editor.getByLabel("Reset day").fill("14");
  await editor.getByLabel("Total quota").fill("3TB");
  await editor.getByLabel("Interfaces / selectors").fill("ens3, eth0+tx");
  await editor
    .getByRole("button", { name: "Preview changes", exact: true })
    .click();
  const previewBlock = page.locator(".vpsRulesPreviewBlock");
  await expect(
    previewBlock.getByText("No changes detected", { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".confirmationPrompt", { hasText: "Confirm VPS rule write" }),
  ).toHaveCount(0);

  await editor.getByLabel("Total quota").fill("4TB");
  await editor
    .getByRole("button", { name: "Preview changes", exact: true })
    .click();
  await expect(previewBlock).toContainText("Effective changes");
  await expect(previewBlock).toContainText("No-op rows hidden");
  const previewGrid = page.getByLabel("Preview changes data grid");
  await expect(previewGrid).toBeVisible();
  await expect(previewGrid).toContainText("traffic.quota.total");
  await expect(previewGrid).not.toContainText("traffic.reset_day");
  await expect(previewGrid).not.toContainText("traffic.selectors");
  await expect(page.getByText("Confirm VPS rule write")).toBeVisible();
  await page.getByRole("button", { name: "Apply 1 change" }).click();
  await expect(page.getByText("applied 1 VPS rule changes")).toBeVisible();

  await editor.getByRole("button", { name: "Unset values" }).click();
  await checkControl(editor.getByLabel("Unset traffic.quota.total"));
  await editor
    .getByRole("button", { name: "Preview changes", exact: true })
    .click();
  const unsetPrompt = page.locator(".confirmationPrompt", {
    hasText: "Confirm VPS rule write",
  });
  await expect(unsetPrompt).toBeVisible();
  await expect(unsetPrompt.getByTitle("unset")).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
});

test("opens manual update check dispatch from fleet selection", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "fleet grid action handoff is covered in desktop navigation",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  const coreRow = grid
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();
  await checkControl(coreRow.getByLabel(/Select VPS instance records row/));
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Check update" }).click();

  await expect(
    page
      .locator(".consoleHeader")
      .getByRole("heading", { name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("searchbox", { name: "Bulk target selector expression" }),
  ).toContainText("id:agent-fra-02");
  await expect(
    page.getByLabel("Agent update version manifest URL"),
  ).toHaveValue(DEFAULT_UPDATE_VERSION_URL);
  await expect(page.getByLabel("Max timeout seconds")).toHaveValue("300");
  await expect(page.getByText("Version manifest")).toBeVisible();
});

test("opens dispatch from fleet selection with selected VPS ids", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "fleet grid action handoff is covered in desktop navigation",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");

  const grid = page.getByLabel("VPS instance records data grid");
  const coreRow = grid
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();
  await checkControl(coreRow.getByLabel(/Select VPS instance records row/));
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(page.locator(".consoleMenuSeparator")).toHaveCount(5);
  await page.getByRole("menuitem", { name: "Open dispatch" }).click();

  await expect(
    page
      .locator(".consoleHeader")
      .getByRole("heading", { name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("searchbox", { name: "Bulk target selector expression" }),
  ).toContainText("id:agent-fra-02");
  await expect(page.getByRole("button", { name: "Argv" })).toHaveClass(
    /selected/,
  );
});

test("keeps fleet alert policy actions selection-scoped", async ({ page }) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Alert policies");

  const grid = page.getByLabel("Policy groups data grid");
  await expect(grid.getByText("1 of 1 policies")).toBeVisible();
  await expect(grid.getByRole("columnheader", { name: "Actions" })).toHaveCount(
    0,
  );
  await expect(page.getByText("Policy detail")).toHaveCount(0);
  const policySearch = grid.getByRole("searchbox", {
    name: "Policy groups search",
  });
  await policySearch.click();
  await page.keyboard.type("enabled");
  await expect(page.getByRole("option", { name: /^enabled$/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(policySearch).toContainText("enabled");
  await policySearch.fill("");

  const policyRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-resource-policy" })
    .first();
  await checkControl(policyRow.getByLabel("Select Policy groups row"));
  await grid.getByRole("button", { name: "Action" }).click();
  await expect(page.getByRole("menuitem", { name: "Details" })).toBeVisible();
  await page.getByRole("menuitem", { name: "Details" }).click();
  await expect(
    page.locator(".consoleDetailPanelHeader strong", {
      hasText: "Alert policy details",
    }),
  ).toBeVisible();
  const belowDetail = page.locator(".consoleDetailPanel");
  await expect(belowDetail).toContainText("edge-resource-policy");
  await expect(belowDetail).toContainText("traffic.cycle.total");
  await expect(belowDetail).toContainText("traffic.quota.total * 0.8");
  await expect(belowDetail).toContainText("Traffic quota threshold reached");
  await page.getByLabel("Close detail panel").click();
  await expect(page.getByText("Alert policy details")).toHaveCount(0);

  await policyRow.getByLabel("Expand Policy groups row").click();
  const inlineDetail = grid.locator(".gridExpandedRow");
  await expect(inlineDetail).toContainText("edge-resource-policy");
  await expect(inlineDetail).toContainText("traffic.cycle.total");
  await policyRow.getByLabel("Collapse Policy groups row").click();
  await expect(inlineDetail).toHaveCount(0);

  await checkControl(policyRow.getByLabel("Select Policy groups row"));
  await grid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();
  const editor = page.locator(".consoleDetailPanel", {
    hasText: "Edit alert policy",
  });
  await expect(
    editor.getByLabel("Policy VPS selector expression"),
  ).toContainText("tag:edge");
  await expect(editor.getByLabel("Rule condition expression")).toHaveValue(
    "traffic.cycle.total >= traffic.quota.total * 0.8",
  );
  await editor.getByRole("button", { name: "Dry-run" }).click();
  await expect(editor.getByText("Dry-run preview")).toBeVisible();
  await expect(editor).toContainText("80% total quota");
  await expect(editor).toContainText("edge-sfo-01");
  await editor.getByRole("button", { name: "Review update" }).click();
  await expect(page.getByText("Confirm alert policy save")).toBeVisible();
  await page.getByRole("button", { name: "Update alert policy" }).click();
  await expect(page.getByText("saved edge-resource-policy")).toBeVisible();
  await page.getByLabel("Close detail panel").click();

  await policyRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Details" })).toBeVisible();
  await page.keyboard.press("Escape");
});

test("shows issued policy alerts in Fleet Alerts and webhook rule fixtures", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "alert and notification registry detail is covered in desktop navigation",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Alerts");
  await expect(page.getByLabel("Fleet alerts", { exact: true })).toContainText(
    "Traffic quota threshold reached",
  );
  await expect(page.getByLabel("Fleet alerts", { exact: true })).toContainText(
    "traffic",
  );

  await openConsoleSubpage(page, "Fleet", "Notifications");
  await page.getByRole("tab", { name: "Webhooks" }).click();
  await expect(page.getByText("Webhook rules", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Webhook rules data grid")).toContainText(
    "edge-interval-webhook",
  );
});

test("keeps console layout usable on desktop and mobile widths", async ({
  page,
}, testInfo) => {
  await page.goto("/");

  const overflow = await page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  );
  expect(overflow).toBeLessThanOrEqual(1);

  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(page.locator(".topbar")).toBeVisible();
  await expect(page.locator(".quickStats")).toBeVisible();
  if (testInfo.project.name.includes("desktop")) {
    await expect(page.locator(".sidebar")).toBeVisible();
    await expect(
      page.getByRole("navigation", { name: "Primary console navigation" }),
    ).toBeVisible();
    const sidebarBox = await page.locator(".sidebar").boundingBox();
    expect(sidebarBox?.x).toBe(0);
    expect(sidebarBox?.y).toBe(0);
    await expect(
      page.locator(".navSectionTitle", { hasText: "Operate" }),
    ).toBeVisible();
    await expect(
      page.locator(".navSectionTitle", { hasText: "Infrastructure" }),
    ).toBeVisible();
    await expect(
      page.locator(".navSectionTitle", { hasText: "Governance" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /All VPS resources/ }),
    ).toBeVisible();
    await expect(
      page.locator(".controlPlanePill", { hasText: "Live control plane" }),
    ).toBeVisible();
  } else {
    await expect(page.locator(".sidebar")).toBeHidden();
    await expect(
      page.getByRole("button", { name: /Edit fleet scope: All VPS resources/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Clear fleet scope" }),
    ).toBeDisabled();
    await page
      .getByRole("combobox", { name: "Console page" })
      .selectOption("Config::templates");
    await expect(
      page.getByRole("heading", { name: "Config", exact: true }),
    ).toBeVisible();
    await expect(page.getByLabel("Config template coverage")).toContainText(
      "Template coverage",
    );
  }
});

test("keeps control-plane metrics in System pages", async ({ page }) => {
  await page.goto("/");

  const dashboard = page.locator(".dashboardWorkspace");
  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fleet command home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operational Health" }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Home telemetry widgets")).toHaveCount(0);
  await expect(dashboard.getByText("DB pool", { exact: true })).toHaveCount(0);
  await expect(
    dashboard.getByText("Gateway events", { exact: true }),
  ).toHaveCount(0);

  await openConsoleSubpage(page, "System", "Overview");
  await expect(
    page.getByRole("heading", { name: "System overview", exact: true }),
  ).toBeVisible();
  const systemOverview = page.getByLabel("System overview operations overview");
  await expect(systemOverview).toContainText("Service health");
  await expect(systemOverview).toContainText("Database");
  await expect(systemOverview).toContainText("Control-plane queue");
  await expect(systemOverview).toContainText("Gateway");
  await expect(systemOverview).toContainText("Worker");
  await expect(systemOverview).toContainText("What needs attention");
  await expect(systemOverview).toContainText("Diagnostics");
  await expect(systemOverview).not.toContainText("Capacity forecast");
  await expect(systemOverview).not.toContainText("Drilldown coverage");
  await expect(
    page.getByRole("heading", {
      name: "Selected chart - Dispatch queue",
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Gateway Events", exact: true }),
  ).toHaveCount(0);

  await openConsoleSubpage(page, "System", "Capacity");
  await expect(
    page.getByRole("heading", { name: "System capacity", exact: true }),
  ).toBeVisible();
  const systemCapacity = page.getByLabel("System capacity posture overview");
  await expect(systemCapacity).toContainText("Subsystem capacity");
  await expect(systemCapacity).toContainText("Database");
  await expect(systemCapacity).toContainText("Dispatch");
  await expect(systemCapacity).toContainText("Queue growth");
  await expect(systemCapacity).toContainText("Warning threshold");
  await expect(systemCapacity).toContainText("Worker availability");
  await expect(systemCapacity).toContainText("Suite Config fields");
  await expect(
    page.getByLabel("dispatch capacity health factors"),
  ).toContainText("queue is growing");
  await expect(
    page.getByRole("heading", { name: "Dispatch capacity", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Gateway capacity", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: /Dispatcher in-flight/ }),
  ).toBeVisible();
  await expect(page.getByText("capacity.dispatcher_in_flight")).toBeVisible();
  await expect(
    page.getByLabel("System capacity unavailable telemetry"),
  ).toContainText("Artifact bytes");

  await openConsoleSubpage(page, "System", "Suite config");
  await expect(
    page
      .locator(".consoleHeader")
      .getByRole("heading", { name: "Suite config", exact: true }),
  ).toBeVisible();
  await expect(
    page
      .locator(".systemConfigOverview")
      .getByRole("heading", { name: "Suite config", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Suite config impact summary")).toContainText(
    "Configuration inventory",
  );
  const configSections = page.getByLabel("Suite config sections");
  await expect(configSections).toContainText("API");
  await expect(configSections).toContainText("Gateway");
  await expect(configSections).toContainText("Worker");
  await expect(configSections).toContainText("Capacity");
  await expect(configSections).toContainText("Storage");
  await expect(configSections).toContainText("Secrets");
  await expect(configSections).toContainText("Timeouts");
  await expect(configSections).toContainText("Review");
  const suiteConfigBoundary = page.getByLabel(
    "Suite config ownership boundary",
  );
  await expect(suiteConfigBoundary).toContainText("System scope");
  await expect(suiteConfigBoundary).toContainText("Runtime config scope");
  await expect(suiteConfigBoundary).toContainText("Save contract");
  await expect(
    suiteConfigBoundary.getByRole("button", { name: "Open Config / Per-VPS" }),
  ).toBeVisible();
  await expect(
    suiteConfigBoundary.getByRole("button", {
      name: "Open Config / Bulk patch",
    }),
  ).toBeVisible();
  await expect(page.getByLabel("API suite config fields")).toContainText(
    "Private HTTP API bind address",
  );
  await configSections.getByRole("button", { name: /Capacity/ }).click();
  await expect(page.getByLabel("Capacity suite config fields")).toContainText(
    "Current",
  );
  await expect(page.getByLabel("Capacity suite config fields")).toContainText(
    "Default",
  );
  await expect(page.getByLabel("Capacity suite config fields")).toContainText(
    "Validation",
  );
  await expect(page.getByLabel("Capacity suite config fields")).toContainText(
    "Restart required",
  );
  await configSections.getByRole("button", { name: /Timeouts/ }).click();
  await expect(page.getByLabel("Timeouts suite config fields")).toContainText(
    "Dispatch ack seconds",
  );
  await expect(page.getByLabel("Suite config save flow")).toContainText("Edit");
  await expect(page.getByText("Advanced redacted JSON diff")).toBeVisible();
  await expect(page.getByText("Current redacted")).toBeHidden();
  await configSections.getByRole("button", { name: /API/ }).click();
  await expect(page.getByLabel("Private API bind")).toBeVisible();
  await configSections.getByRole("button", { name: /Capacity/ }).click();
  await page.getByLabel("API DB pool").fill("40");
  await expect(page.getByLabel("Suite config impact summary")).toContainText(
    "Draft impact",
  );
  await expect(page.getByLabel("Suite config impact summary")).toContainText(
    "1 changed",
  );
  await expect(page.locator(".systemConfigOverview")).toContainText(
    "Draft restart",
  );
  await expect(
    page
      .locator(".systemConfigReview")
      .getByText("capacity.api_db_pool")
      .first(),
  ).toBeVisible();
  await expect(
    page.getByLabel("Suite config reload and restart plan"),
  ).toContainText("Restart required after save");
  const suiteConfigReview = page.getByLabel(
    "Suite config validation and save review",
  );
  await expect(
    suiteConfigReview.getByText("Next: unlock privilege"),
  ).toBeVisible();
  await expect(
    page.getByLabel("Suite config validation and save review"),
  ).toBeVisible();
  await expect(
    suiteConfigReview.getByText("Open Privilege Vault").first(),
  ).toBeVisible();
  await expect(page.getByLabel(/super password/i)).toHaveCount(0);
  await expect(page.getByLabel(/super salt/i)).toHaveCount(0);
  await expect(page.getByLabel("VPS config target")).toHaveCount(0);
  await expect(page.getByLabel("VPS redacted runtime config TOML")).toHaveCount(
    0,
  );
  await expect(
    page.getByLabel("One-VPS runtime config override TOML"),
  ).toHaveCount(0);
  await expect(page.getByLabel("Bulk patch target expression")).toHaveCount(0);
  await expect(
    page.getByLabel("Rendered bulk runtime config patch TOML"),
  ).toHaveCount(0);
  await expect(
    page.getByLabel("Temporary bulk runtime config patch TOML"),
  ).toHaveCount(0);

  await suiteConfigReview
    .getByRole("button", { name: "Open Privilege Vault" })
    .first()
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Privilege vault" }),
  ).toBeVisible();
  await page.getByLabel(/privilege secret/i).fill("local-super-password");
  await page
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    page
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: "Unlock", exact: true }),
  );
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await openConsoleSubpage(page, "System", "Suite config");
  await page
    .getByLabel("Suite config sections")
    .getByRole("button", { name: /Capacity/ })
    .click();
  await page.getByLabel("API DB pool").fill("40");
  await expect(
    page
      .getByLabel("Suite config validation and save review")
      .getByText("Next: review changes"),
  ).toBeVisible();
  await activate(
    page.getByRole("button", { name: "Review changes", exact: true }).first(),
  );
  await expect(page.getByText("Confirm suite config save")).toBeVisible();
  await confirmVisiblePrompt(page, "Save suite config");
  const suiteConfigRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { suiteConfigs: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.suiteConfigs.at(-1);
  });
  expect(suiteConfigRequest).toMatchObject({
    confirmed: true,
  });
  expectPrivilegeAssertion(suiteConfigRequest);
  expect((suiteConfigRequest as { toml: string }).toml).toContain(
    "api_db_pool = 40",
  );
});

test("surfaces operator users under Access and session evidence under Audit", async ({
  page,
}) => {
  await page.goto("/");

  await unlockPrivilegeFor(page, "Access", "Operators");
  await expect(
    page.getByRole("heading", { name: "Operators", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operator accounts", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("2 operator records")).toBeVisible();
  const governance = page.getByLabel("Operator governance overview");
  await expect(governance).toContainText("MFA policy");
  await expect(governance).toContainText("1 admin needs MFA");
  await expect(governance).toContainText("recommended instead of enforced");
  await expect(governance).toContainText("Refresh TTL policy");
  await expect(governance).toContainText("1 admin over target");
  await expect(governance).toContainText("Role model");
  await expect(governance).toContainText("3 standard roles");
  await expect(governance).toContainText("Viewer");
  await expect(governance).toContainText("Operator");
  await expect(governance).toContainText("Admin");
  await expect(governance).toContainText("Bearer sessions");
  await expect(governance).toContainText("2 active");
  await expect(governance).toContainText("Auth failures in loaded history");
  await expect(governance).toContainText("2 loaded failures");
  await expect(governance).toContainText(
    "Per-user counts below use the same loaded auth history",
  );
  await expect(governance).toContainText("Policy evidence boundary");
  await expect(page.getByText("Last login").first()).toBeVisible();
  await expect(page.getByText("Actions").first()).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Manage" }).first(),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Revoke sessions" }).first(),
  ).toBeVisible();
  await selectGridRow(
    page,
    "Operator accounts",
    "99999999-aaaa-4bbb-8ccc-000000000001",
  );
  await runGridAction(page, "Operator accounts", "Edit selected");
  await expect(page.getByLabel("Operator username")).toHaveValue(
    "console-admin",
  );
  const adminEvidence = page.getByLabel("Operator access evidence");
  await expect(adminEvidence).toContainText("Policy recommends MFA");
  await expect(adminEvidence).toContainText("365d - over admin target");
  await expect(adminEvidence).toContainText(
    "not exposed by the current operator API",
  );
  await activate(
    adminEvidence.getByRole("button", { name: "Revoke sessions" }),
  );
  const userSessionRevokePrompt = page.getByLabel("Confirm admin user action");
  await expect(userSessionRevokePrompt).toBeVisible();
  await expect(userSessionRevokePrompt).toContainText(
    "Revoke 1 non-current active sessions",
  );
  await activate(
    userSessionRevokePrompt.getByRole("button", { name: "Cancel" }),
  );
  await activate(page.getByRole("button", { name: "Disable" }));
  await expect(page.getByText("Preparing review")).toBeVisible();
  await expect(page.getByLabel("Confirm admin user action")).toBeVisible();
  await expect(
    page.getByText(/targets or grants admin privileges/),
  ).toBeVisible();
  await page.getByLabel("Session refresh TTL days").fill("31");
  await expect(page.getByLabel("Confirm admin user action")).toBeHidden();
  await activate(page.getByRole("button", { name: "Disable" }));
  await expect(page.getByLabel("Confirm admin user action")).toBeVisible();
  await activate(page.getByRole("button", { name: "Cancel" }));

  await unselectGridRow(
    page,
    "Operator accounts",
    "99999999-aaaa-4bbb-8ccc-000000000001",
  );
  await selectGridRow(
    page,
    "Operator accounts",
    "99999999-aaaa-4bbb-8ccc-000000000002",
  );
  await runGridAction(page, "Operator accounts", "Edit selected");
  await expect(page.getByLabel("Operator username")).toHaveValue(
    "noc-operator",
  );
  await expect(page.getByLabel("Operator password")).toHaveAttribute(
    "title",
    /Save does not read or send this field/,
  );
  await expect(page.getByLabel("Session refresh TTL days")).toHaveAttribute(
    "title",
    /Refresh-token\/session lifetime/,
  );
  await expect(
    page.getByRole("button", { name: "Save", exact: true }),
  ).toHaveAttribute("title", /never changes the password/);
  await page.getByLabel("Operator role").selectOption("admin");
  await expect(page.getByText(/Admin role grants require/)).toBeVisible();
  await activate(page.getByRole("button", { name: "Save", exact: true }));
  const adminGrantPrompt = page.getByLabel("Confirm admin user action");
  await expect(adminGrantPrompt).toBeVisible();
  await expect(adminGrantPrompt).toContainText(
    "targets or grants admin privileges",
  );
  await activate(adminGrantPrompt.getByRole("button", { name: "Cancel" }));
  await page.getByLabel("Operator role").selectOption("operator");
  await page.getByLabel("Operator password").fill("replacement-password-123");
  await activate(page.getByRole("button", { name: "Save", exact: true }));
  await expect(page.getByText("Preparing review")).toBeVisible();
  const savePrompt = page.getByLabel("Confirm user action");
  await expect(savePrompt).toBeVisible();
  await expect(savePrompt).not.toContainText("replacement-password-123");
  await activate(savePrompt.getByRole("button", { name: "Save user" }));
  const operatorUpdate = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { operatorActions: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.operatorActions.at(-1);
  });
  expect(JSON.stringify(operatorUpdate)).not.toContain(
    "replacement-password-123",
  );
  expect(operatorUpdate).toMatchObject({
    action: "update",
    body: { confirmed: true },
    operator_id: "99999999-aaaa-4bbb-8ccc-000000000002",
  });
  expectPrivilegeAssertion((operatorUpdate as { body?: unknown }).body);

  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Sessions");
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Session evidence",
      exact: true,
    }),
  ).toBeVisible();
  const auditSessions = page.locator(".auditSessionEvidencePanel");
  await expect(
    auditSessions.getByLabel("Session evidence summary"),
  ).toContainText("Terminal sessions");
  await expect(
    auditSessions.getByLabel("Session evidence summary"),
  ).toContainText("stale terminal states hidden from open count");
  await expect(
    auditSessions.getByLabel("Session evidence summary"),
  ).toContainText("expired bearer sessions");
  await expect(
    auditSessions.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Stale state");
  await expect(
    auditSessions.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Trace only; small retained transcript");
  await expect(
    auditSessions.getByLabel("Operator session evidence"),
  ).toContainText("Expired");
  await expect(
    auditSessions.getByRole("button", { name: "Revoke session", exact: true }),
  ).toHaveCount(0);
  await expect(
    auditSessions.getByRole("button", { name: "Revoke selected", exact: true }),
  ).toHaveCount(0);
});

test("packs dense metric rows by label length", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop metric-row packing is the production density target",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "System", "Overview");
  const metricRows = page.locator(".dashboardTopClients.systemMetricTable").first();
  await expect(metricRows.getByText("Current")).toBeVisible();

  const layout = await metricRows.evaluate((container) => {
    const rows = Array.from(
      container.querySelectorAll<HTMLElement>(".dashboardClientRow"),
    );
    const labels = [
      "db-a",
      "edge-observability-relay-long-production-name-us-west",
      "cache-02",
    ];
    rows.forEach((row, index) => {
      row.querySelector("strong")!.textContent =
        labels[index] ?? `vps-${index}`;
    });
    return {
      display: getComputedStyle(container).display,
      gridTemplateColumns: getComputedStyle(container).gridTemplateColumns,
      rows: rows.map((row) => {
        const label = row.querySelector<HTMLElement>("strong")!;
        return {
          clipped: label.scrollWidth > label.clientWidth + 1,
          width: Math.round(row.getBoundingClientRect().width),
        };
      }),
    };
  });

  expect(layout.display).toBe("flex");
  expect(layout.gridTemplateColumns).toBe("none");
  expect(layout.rows.some((row) => row.clipped)).toBe(false);
  const widths = layout.rows.map((row) => row.width);
  const shortest = Math.min(...widths);
  const longest = Math.max(...widths);
  expect(longest - shortest).toBeGreaterThan(40);
  expect(shortest / longest).toBeLessThan(0.6);
});

test("manages template assignments from automation source templates", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense template management is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Source templates");

  const panel = page.locator(".sourceTemplatePanel");
  const sourceStatusSection = panel.locator(".sourceStatusSection");
  const activeSourcesSearch = panel.getByRole("searchbox", {
    name: "Active sources search",
  });
  await expect(sourceStatusSection.locator("summary")).toContainText(
    "Active source status",
  );
  await expect(sourceStatusSection.locator("summary")).toContainText(
    "need review",
  );
  await expect(activeSourcesSearch).toBeHidden();
  await activate(sourceStatusSection.locator("summary"));
  const activeSourcesGrid = panel.getByLabel("Active sources data grid");
  const activeSourceRows = activeSourcesGrid.locator(".gridBody .gridRow");
  await expect(activeSourcesGrid).toBeVisible();
  await expect(activeSourcesSearch).toBeVisible();
  await expect(activeSourcesGrid.locator(".gridCounts")).not.toContainText(
    "selected",
  );
  await expect(
    activeSourcesGrid.locator('.gridHeaderGroup input[type="checkbox"]'),
  ).toHaveCount(0);
  await activeSourcesSearch.fill("not-a-real-source");
  await expect(
    panel.getByText("No active source records match the current search."),
  ).toBeVisible();
  await expect(panel.getByText(/No selected source records/)).toHaveCount(0);
  await activeSourcesSearch.fill("");
  await expect(panel.getByText(/\d+ of \d+ sources/)).toBeVisible();
  await expect(panel.getByText(/1 \/ \d+/).first()).toBeVisible();
  await expect(
    activeSourceRows.filter({ hasText: "shared:vnstat-json" }),
  ).toBeVisible();
  await expect(panel.locator(".sourceStatusSection")).toContainText(
    "shared:vnstat-json",
  );
  await expect(
    panel
      .locator(".sourceStatusSection")
      .getByText("no server store, 2 artifacts"),
  ).toBeVisible();
  await expect(panel.locator(".sourceStatusSection")).toContainText(
    "Source selected; server storage not configured",
  );
  await expect(panel.locator(".sourceStatusSection")).not.toContainText(
    "selected_no_store",
  );
  await expect(
    activeSourceRows
      .filter({ hasText: "Update artifact source" })
      .filter({ hasText: "Ready" }),
  ).toBeVisible();
  await activeSourcesSearch.fill("vnstat");
  await expect(
    activeSourceRows.filter({ hasText: "shared:vnstat-json" }),
  ).toBeVisible();
  await activeSourcesSearch.fill("");

  const templatePanel = page.locator(".sourceTemplatePanel");
  const templateRegistrySearch = templatePanel.getByRole("searchbox", {
    name: "Template registry search",
  });
  await expect(
    templatePanel
      .locator(".sectionHeader")
      .getByRole("heading", { name: "Source templates" }),
  ).toBeVisible();
  const templateRegistryGrid = templatePanel.getByLabel(
    "Template registry data grid",
  );
  const templateRows = templateRegistryGrid.locator(".gridBody .gridRow");
  await expect(templateRegistryGrid).toBeVisible();
  await expect(templateRegistrySearch).toBeVisible();
  await expect(templateRegistryGrid.locator(".gridCounts")).toContainText(
    "0 selected",
  );
  await expect(
    templateRegistryGrid.locator('.gridHeaderGroup input[type="checkbox"]'),
  ).toHaveCount(1);
  await expect(
    templateRegistryGrid.getByRole("button", { name: "New template" }),
  ).toBeVisible();
  await expect(
    templateRegistryGrid.getByRole("button", { name: "Action" }),
  ).toBeDisabled();
  await expect(
    templatePanel.getByLabel("Template assignment target expression"),
  ).toHaveCount(0);
  await expect(
    templatePanel.getByLabel("Template definition JSON"),
  ).toHaveCount(0);
  await expect(templatePanel.getByText("Assign selected template")).toHaveCount(
    0,
  );
  await expect(templatePanel.getByText("Render selected config")).toHaveCount(
    0,
  );
  await expect(templatePanel.getByText(/selected templates/)).toHaveCount(0);
  await expect(
    templatePanel.getByText(/selected template records/),
  ).toHaveCount(0);
  await expect(
    templateRows.filter({ hasText: "builtin:interface_counters" }),
  ).toBeVisible();
  await expect(
    templateRows.filter({ hasText: "shared:vnstat-json" }),
  ).toBeVisible();
  await activate(
    templateRegistryGrid.getByRole("button", { name: "New template" }),
  );
  await expect(
    templatePanel.getByLabel("New source template", { exact: true }),
  ).toBeVisible();
  await expect(
    templatePanel.getByLabel("Template definition JSON"),
  ).toBeVisible();
  await activate(
    templatePanel.getByRole("button", { name: "Close New source template" }),
  );
  await expect(
    templatePanel.getByLabel("New source template", { exact: true }),
  ).toHaveCount(0);

  await activate(
    templateRows.filter({ hasText: "shared:vnstat-json" }).first(),
  );
  await expect(
    templatePanel.getByLabel("shared:vnstat-json", { exact: true }),
  ).toBeVisible();
  await expect(
    templatePanel.getByRole("tab", { name: "Assign" }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    templatePanel.getByLabel("Template assignment target expression"),
  ).toBeVisible();
  await expect(
    templatePanel.getByLabel("Template definition JSON"),
  ).toHaveCount(0);
  await templateRegistrySearch.fill("runtime_traffic_accounting_source");
  await expect(
    templateRows.filter({ hasText: "builtin:interface_counters" }),
  ).toBeVisible();
  await templateRegistrySearch.fill("");
  await templatePanel
    .getByLabel("Assignment domain")
    .selectOption("runtime_traffic_accounting_source");
  await templatePanel
    .getByLabel("Template assignment template")
    .selectOption("11111111-1111-4111-8111-111111111111");
  await templatePanel
    .getByRole("searchbox", {
      name: "Template assignment target expression",
    })
    .fill("(provider:alpha && country:US) || id:agent-fra-02");
  await expect(templatePanel.getByText("2/3 matching VPSs")).toBeVisible();
  await activate(
    templatePanel.getByRole("button", { name: "Review assignment" }),
  );
  await expect(
    templatePanel.getByText("Confirm template assignment"),
  ).toBeVisible();
  await confirmVisiblePrompt(page, "Apply template assignment");

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { sourceTemplateAssignments: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.sourceTemplateAssignments.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    domain: "runtime_traffic_accounting_source",
    template_id: "11111111-1111-4111-8111-111111111111",
    selector_expression: "(provider:alpha && country:US) || id:agent-fra-02",
    target_client_ids: ["agent-fra-02", "agent-sfo-01"],
  });
});

test("prefills registered agent update shortcuts into dispatch", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "agent update shortcuts are covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Agent updates");

  await expect(
    page.getByRole("heading", { name: "Agent update registry" }),
  ).toBeVisible();
  const posture = page.getByLabel("Agent update rollout posture");
  await expect(posture).toContainText("Available version");
  await expect(posture).toContainText("Current fleet versions");
  await expect(posture).toContainText("Registered artifact");
  await expect(posture).toContainText("Targets");
  await expect(posture).toContainText("Registry policy");
  await expect(posture).toContainText("Health checks");
  await expect(posture).toContainText("Rollback");
  const shortcuts = page.getByLabel("Agent update dispatch shortcuts");
  await expect(
    page.getByText("Latest release has no rollback artifact."),
  ).toBeVisible();
  await expect(
    shortcuts.getByRole("button", { name: "Rollback" }),
  ).toBeDisabled();
  const updateShortcut = shortcuts.getByRole("button", {
    name: "Start update",
  });
  await expect(updateShortcut).toBeEnabled();
  await activate(updateShortcut);

  await expect(
    page.getByRole("heading", { name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Activate", exact: true }),
  ).toHaveClass(/selected/);
  await expect(page.getByLabel("Agent update staged SHA-256")).toHaveValue(
    "d".repeat(64),
  );
});

test("renders patch generators and submits explicit runtime config patch modes", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "runtime config patch generator editing is covered in the desktop console layout",
  );

  await page.goto("/");
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Bulk patch");
  await expect(page.getByLabel("Patch generators data grid")).toHaveCount(0);
  await activate(page.getByRole("button", { name: "Manage generators" }));
  const templateGrid = page.getByLabel("Patch generators data grid");
  await expect(templateGrid).toBeVisible();
  await expect(
    templateGrid
      .locator(".gridBody .gridRow")
      .filter({ hasText: "Autonomous updater enabled" }),
  ).toBeVisible();
  await expect(
    templateGrid
      .locator(".gridBody .gridRow")
      .filter({ hasText: "Autonomous updater disabled" }),
  ).toBeVisible();

  await unlockPrivilegeFor(page, "Config", "Bulk patch");
  const bulk = page.locator(".configApplyGrid");
  await bulk
    .getByLabel("Patch generator", { exact: true })
    .selectOption({ label: "Autonomous updater disabled" });
  await expect(bulk.getByLabel("Patch generator values JSON")).toHaveValue(
    /github\.com\/mnihyc\/vpsman\/releases\/latest\/download\/version\.json/,
  );
  await bulk
    .getByRole("searchbox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await expect(
    page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await activate(bulk.getByRole("button", { name: "Preview changes" }));
  await expect(
    bulk.getByLabel("Rendered bulk runtime config patch TOML"),
  ).toHaveValue(
    /\[update\][\s\S]*unmanaged_enabled = false[\s\S]*version\.json/,
  );
  await expect(bulk.getByText("1 VPS resolved")).toBeVisible();
  await expect(bulk.getByLabel("Bulk patch change summary")).toContainText(
    "edge-sfo-01",
  );
  await page.keyboard.press("Escape");
  await expect(bulk.getByRole("button", { name: "Apply patch" })).toBeEnabled();
  await activate(bulk.getByRole("button", { name: "Apply patch" }));
  await expect(page.getByText("Confirm bulk patch")).toBeVisible();
  await confirmVisiblePrompt(page, "Apply runtime config patch");

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { runtimeConfigPatches: any[] };
      }
    ).__vpsmanTestRequests;
    return requests.runtimeConfigPatches.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });
  expect((request as { toml: string }).toml).toContain("[update]");
  expect((request as { toml: string }).toml).toContain(
    "unmanaged_enabled = false",
  );
  expect(JSON.stringify(request)).not.toContain("local-super-password");
});

test("uses an exact VPS combobox for single config jobs", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "single config combobox behavior is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Per-VPS");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Per-VPS");

  const targetPicker = page.getByRole("combobox", {
    name: "VPS config target",
  });
  await expect(targetPicker).toHaveValue("");
  await targetPicker.fill("not-a-real-vps");
  await targetPicker.blur();
  await expect(targetPicker).toHaveValue("");
  await chooseVpsBySearch(
    page.locator(".configApplyGrid"),
    "VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(targetPicker).toHaveValue("core-fra-02 (ra02)");
  await activate(page.getByRole("button", { name: "Read current config" }));

  await expect
    .poll(async () =>
      page.evaluate(() => {
        const requests = (
          window as unknown as { __vpsmanTestRequests: { jobs: any[] } }
        ).__vpsmanTestRequests;
        return requests.jobs.some((item) => item.command === "config_read");
      }),
    )
    .toBe(true);
  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: any[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.find((item) => item.command === "config_read");
  });
  expect(request).toMatchObject({
    command: "config_read",
    force_unprivileged: true,
    privileged: false,
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });

  const configEditor = page.getByLabel("VPS redacted runtime config TOML");
  await expect(configEditor).toHaveValue(/client_id = "agent-fra-02"/);
  await expect(configEditor).toHaveValue(
    /unmanaged_version_url = "https:\/\/github\.com\/mnihyc\/vpsman\/releases\/latest\/download\/version\.json"/,
  );
  await expect(
    page.getByText(
      "This immutable redacted base is the guard for the one-VPS patch.",
    ),
  ).toBeVisible();
  await expect(page.getByLabel("One-VPS config override guard")).toContainText(
    "Current base",
  );
  await page
    .getByLabel("One-VPS runtime config override TOML")
    .fill("[update]\nunmanaged_enabled = true\n");
  await expect(page.getByLabel("One-VPS config override guard")).toContainText(
    "update",
  );
  await expect(page.getByRole("button", { name: "Apply patch" })).toBeEnabled();
});

test("creates a cron schedule from a command template with target preview", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense schedule composition is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Scheduled runs");
  await expect(page.getByText("1 schedule-created run")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "Scheduled runs" }),
  ).toBeVisible();
  const scheduledRunsGrid = page.getByLabel("Schedule run records data grid");
  await expect(scheduledRunsGrid).toContainText("edge-health-hourly");
  await expect(scheduledRunsGrid).toContainText("Hourly at minute 0");
  await expect(scheduledRunsGrid).toContainText("Scheduled shell command");
  await expect(scheduledRunsGrid).toContainText("Not reported");
  await expect(page.getByText("due not exposed")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Retry" })).toHaveCount(0);
  await activate(page.getByRole("button", { name: "Open schedule registry" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Schedules" }),
  ).toBeVisible();
  await unlockPrivilegeFor(page, "Automation", "Schedules");
  await expect(page.getByText("Hourly at minute 0")).toBeVisible();
  await expect(page.getByText("0 * * * * · UTC")).toBeVisible();
  const schedulesGrid = page.getByLabel("Schedule records data grid");
  await expect(page.getByLabel("Schedule execution policy")).toContainText(
    "Enabled schedules automatically dispatch future jobs",
  );
  await activate(
    schedulesGrid
      .getByRole("button", { name: /Expand Schedule records row/ })
      .first(),
  );
  await expect(
    page.getByText("Run only one missed run; retry after 5m"),
  ).toBeVisible();

  await activate(page.getByRole("button", { name: "Expand Create schedule" }));
  await page
    .getByLabel("Schedule job template")
    .selectOption("46464646-5656-4789-8abc-defdefdefdef");
  await page.getByLabel("Schedule cron expression").fill("*/15 * * * *");
  await page.getByLabel("Schedule target expression").fill("country:US");
  await expect(page.getByText("2 VPSs in local preview")).toBeVisible();
  await expect(
    page.getByText(/UTC schedule, displayed in browser timezone/),
  ).toBeVisible();
  await expect(page.getByText("Every 15 minutes")).toBeVisible();
  await expect(
    page.getByText(/2 matching VPSs in local preview; edge-health-check/),
  ).toBeVisible();
  await activate(
    page.getByRole("button", { name: "Review save", exact: true }),
  );
  await expect(page.getByText("Confirm schedule")).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Save schedule" }),
  );

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { schedules: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.schedules.at(-1);
  });
  expect(request).toMatchObject({
    cron_expr: "*/15 * * * *",
    name: "edge-health-check schedule",
    operation: { argv: ["uptime"], pty: false, type: "shell" },
    selector_expression: "country:US",
    timezone: "UTC",
  });
});

test("registers VPS identities and revokes current keys from the access panel", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense access administration is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Access", "VPS identities");
  const accessTabs = page.locator(".accessTabs");
  await activate(accessTabs.getByRole("button", { name: "VPS identities" }));
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "VPS identities");
  await activate(accessTabs.getByRole("button", { name: "VPS identities" }));

  await expect(
    page.getByRole("heading", { level: 2, name: "VPS identities" }),
  ).toBeVisible();
  const revocationGrid = page.getByLabel("Client key revocations data grid");
  await expect(revocationGrid.locator(".gridCounts")).not.toContainText(
    "selected",
  );
  await expect(
    revocationGrid.locator('.gridHeaderGroup input[type="checkbox"]'),
  ).toHaveCount(0);
  await expect(revocationGrid).toContainText("Host rebuild");
  await expect(revocationGrid).not.toContainText("fixture");
  const identityGrid = page.getByLabel("VPS identities data grid");
  await expect(
    identityGrid
      .getByRole("button", { name: /Copy current key fingerprint/ })
      .first(),
  ).toBeVisible();
  const inspector = page.locator(".accessInspector");
  await expect(inspector).toBeHidden();
  await identityGrid.getByRole("button", { name: "Register VPS" }).click();
  await expect(
    inspector.getByRole("heading", { name: "Register VPS" }),
  ).toBeVisible();
  await inspector.getByLabel("Agent identity client ID").fill("agent-tokyo-04");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("a".repeat(64));
  await inspector
    .getByLabel("Agent identity display name")
    .fill("edge-tokyo-04");
  await inspector
    .getByLabel("Agent identity tags")
    .fill("country:JP, role:edge");
  await activate(
    inspector.getByRole("button", { name: "Review registration" }),
  );
  await expect(
    page.getByLabel("Confirm VPS identity registration"),
  ).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm VPS identity registration")
      .getByRole("button", { name: "Register VPS" }),
  );
  await expect(inspector.getByText("edge-tokyo-04")).toBeVisible();
  const identityRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { agentIdentities: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.agentIdentities.at(-1);
  });
  expect(identityRequest).toMatchObject({
    client_id: "agent-tokyo-04",
    client_public_key_hex: "a".repeat(64),
    confirmed: true,
    display_name: "edge-tokyo-04",
    replace_existing_key: false,
    tags: ["country:JP", "role:edge"],
  });
  expectPrivilegeAssertion(identityRequest);

  await selectGridRow(page, "VPS identities", "agent-sfo-01");
  await runGridAction(page, "VPS identities", "Revoke selected");
  await expect(
    inspector.getByRole("heading", { name: "Revoke VPS key" }),
  ).toBeVisible();
  await inspector
    .getByLabel("VPS identity revoke reason")
    .fill("lost host rebuild");
  await activate(inspector.getByRole("button", { name: "Revoke current key" }));
  await expect(page.getByLabel("Confirm current key revocation")).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm current key revocation")
      .getByRole("button", { name: "Revoke key" }),
  );
  const revokeRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { clientKeyRevocations: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.clientKeyRevocations.at(-1);
  });
  expect(revokeRequest).toMatchObject({
    confirmed: true,
    reason: "lost host rebuild",
  });
  expectPrivilegeAssertion(revokeRequest);
});

test("shows access posture, MFA risk, identity lifecycle, and gateway readiness", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "access posture is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Access", "Overview");

  const actions = page.getByLabel("Access actions required");
  await expect(actions).toContainText("Policy recommends MFA");
  await expect(actions).toContainText("Recommended");
  await expect(actions).toContainText("Expired bearer sessions");
  await expect(actions).toContainText("2 expired");
  await expect(actions).toContainText("Gateway install defaults");
  await expect(actions).toContainText("Privilege state");
  await expect(actions).toContainText(
    "No saved local vault; enter privilege secret when needed.",
  );

  const responsibilities = page.getByLabel("Access overview responsibilities");
  await expect(responsibilities).toContainText("Operators and active sessions");
  await expect(responsibilities).toContainText("2 operators / 0 active");
  await expect(responsibilities).toContainText(
    "after expiry validation; current session Expired",
  );
  await expect(responsibilities).toContainText("VPS identities");
  await expect(responsibilities).toContainText("Gateway sessions");
  await expect(responsibilities).toContainText("Privilege state");

  await activate(actions.getByRole("button", { name: "Set up MFA" }));
  await expect(
    page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  ).toBeVisible();
  const privilegePanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  });
  await expect(page.getByText("Admin MFA is off")).toBeVisible();
  await expect(page.getByLabel(/super password/i)).toHaveCount(0);
  await expect(page.getByLabel(/super salt/i)).toHaveCount(0);
  await expect(page.getByLabel(/access privilege secret/i)).toBeVisible();
  await expect(page.getByLabel(/access privilege salt/i)).toBeVisible();
  await expect(page.getByLabel("Privilege vault state")).toContainText("Locked");
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "Current browser only",
  );
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "Not active",
  );
  await expect(page.getByText("Keep encrypted in this browser")).toBeVisible();
  await expect(page.getByText("Save encrypted vault")).toHaveCount(0);
  await expect(page.getByText("Deny by default")).toHaveCount(0);
  await expect(page.getByLabel("TOTP enrollment sequence")).toContainText(
    "Password",
  );
  await expect(page.getByLabel("TOTP enrollment sequence")).toContainText(
    "QR/secret",
  );
  await expect(
    page.getByRole("button", { name: "Generate setup" }),
  ).toBeDisabled();
  await page.getByLabel(/access privilege secret/i).fill("local-super-password");
  await page
    .getByLabel(/access privilege salt/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    privilegePanel.getByRole("button", { name: "Unlock", exact: true }),
  );
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "Unlocked",
  );
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "This browser tab",
  );
  await expect(page.getByRole("button", { name: "Lock now" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Clear local session" }),
  ).toHaveCount(0);
  await openConsoleSubpage(page, "Access", "Overview");
  await activate(actions.getByRole("button", { name: "Manage sessions" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Session evidence" }),
  ).toBeVisible();
  await expect(page.getByLabel("Audit session evidence")).toContainText(
    "2 expired bearer sessions",
  );

  await openConsoleSubpage(page, "Access", "VPS identities");
  await expect(page.getByLabel("Access posture overview")).toHaveCount(0);
  await expect(page.getByLabel("Agent identity lifecycle")).toHaveCount(0);
  const identityGrid = page.getByLabel("VPS identities data grid");
  await expect(identityGrid).toContainText("Register VPS");
  await expect(
    identityGrid
      .getByRole("button", { name: /Copy current key fingerprint/ })
      .first(),
  ).toBeVisible();
  const inspector = page.locator(".accessInspector");
  await expect(inspector).toBeHidden();
  await identityGrid.getByRole("button", { name: "Register VPS" }).click();
  await expect(inspector).toContainText("Register VPS");
  await expect(inspector).toContainText("Private key material is shown once");
  await expect(inspector).toContainText("VPS client ID");
  await expect(inspector).toContainText("Noise public key");
  await inspector
    .getByRole("button", { name: "Close VPS identity workflow" })
    .click();
  await expect(inspector).toBeHidden();

  await openConsoleSubpage(page, "Access", "Gateway sessions");
  const emptyState = page.getByLabel("Gateway sessions empty state");
  await expect(emptyState).toContainText(
    "No active gateway sessions. Configure the gateway endpoint and server key.",
  );
  await expect(emptyState).toContainText(
    "Gateway defaults are managed from shared system configuration.",
  );
  await expect(
    emptyState.getByRole("button", { name: "Gateway settings" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Preferences", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText("No panel-side endpoint lookup")).toHaveCount(0);
  await expect(page.getByRole("columnheader", { name: "Gateway" })).toHaveCount(
    0,
  );
  await activate(emptyState.getByRole("button", { name: "Gateway settings" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Suite config" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Suite config" }),
  ).toBeVisible();
});

test("rotates an existing agent key through the access panel", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "key rotation is a desktop admin workflow",
  );

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "VPS identities");
  const accessTabs = page.locator(".accessTabs");
  await activate(accessTabs.getByRole("button", { name: "VPS identities" }));

  await selectGridRow(page, "VPS identities", "agent-sfo-01");
  await runGridAction(page, "VPS identities", "Rotate selected");

  const inspector = page.locator(".accessInspector");
  await expect(
    inspector.getByRole("button", { name: "Review rotation" }),
  ).toBeVisible();

  const displayNameInput = inspector.getByLabel("Agent identity display name");
  const tagsInput = inspector.getByLabel("Agent identity tags");
  await expect(displayNameInput).toBeDisabled();
  await expect(tagsInput).toBeDisabled();

  await inspector.getByLabel("Agent identity client ID").fill("agent-sfo-01");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("b".repeat(64));
  await activate(inspector.getByRole("button", { name: "Review rotation" }));
  await expect(page.getByLabel("Confirm client key rotation")).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm client key rotation")
      .getByRole("button", { name: "Rotate key" }),
  );

  const identityRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { agentIdentities: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.agentIdentities.at(-1);
  });
  expect(identityRequest).toMatchObject({
    client_id: "agent-sfo-01",
    client_public_key_hex: "b".repeat(64),
    confirmed: true,
    display_name: null,
    replace_existing_key: true,
    tags: [],
  });
  expectPrivilegeAssertion(identityRequest);
});

test("mobile VPS identity registration opens full-screen workflow", async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes("mobile"),
    "mobile drawer behavior is specific to the mobile console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Access", "VPS identities");
  const identityGrid = page.getByLabel("VPS identities data grid");
  await identityGrid.getByRole("button", { name: "Register VPS" }).click();

  const workflow = page.locator(".identityWorkflowPanel");
  await expect(workflow).toBeVisible();
  await expect(
    workflow.getByRole("heading", { name: "Register VPS" }),
  ).toBeVisible();
  await expect(
    workflow.getByRole("button", { name: "Close VPS identity workflow" }),
  ).toBeVisible();

  const box = await workflow.boundingBox();
  const viewport = page.viewportSize();
  expect(box?.x).toBeLessThanOrEqual(1);
  expect(box?.y).toBeLessThanOrEqual(1);
  expect(box?.width).toBeGreaterThanOrEqual((viewport?.width ?? 0) - 2);
  expect(box?.height).toBeGreaterThanOrEqual((viewport?.height ?? 0) - 2);
});

test("shows topology network evidence, speed metrics, and probe latency history", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "topology evidence drilldown is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Graph");

  await expect(
    page.getByRole("heading", { name: "Topology graph" }),
  ).toBeVisible();
  await expect(page.getByRole("img", { name: "Topology graph" })).toBeVisible();
  const graphPanel = page.locator(".topologyGraphPanel");
  await expect(
    page.getByText("2 shown / 2 nodes; 1 shown / 1 tunnels"),
  ).toBeVisible();
  await expect(graphPanel).toContainText("Last topology evidence");
  await expect(graphPanel).toContainText("stale");
  await expect(graphPanel.getByLabel("Topology graph legend")).toContainText(
    "Layers",
  );
  await expect(graphPanel.getByLabel("Topology graph legend")).toContainText(
    "OSPF 22 (+8)",
  );
  await expect(graphPanel.getByLabel("Topology graph legend")).toContainText(
    "12.4 ms",
  );
  await expect(graphPanel.getByLabel("Topology graph legend")).toContainText(
    "0.25% loss",
  );
  await expect(graphPanel.getByLabel("Topology graph legend")).toContainText(
    "10.1 Mbps avg",
  );
  await expect(graphPanel.getByText("Why OSPF cost changed")).toBeVisible();
  await expect(graphPanel.getByLabel("Topology minimap")).toHaveCount(0);
  await expect(
    graphPanel.getByRole("button", { name: "Zoom in topology graph" }),
  ).toBeVisible();
  await activate(
    graphPanel.getByRole("button", { name: "Zoom in topology graph" }),
  );
  await expect(graphPanel.getByText("120%")).toBeVisible();
  await activate(
    graphPanel.getByRole("button", { name: "Reset topology graph view" }),
  );
  await expect(graphPanel.getByText("100%")).toBeVisible();
  await expect(
    graphPanel.getByText("Healthy", { exact: true }).first(),
  ).toBeVisible();
  await page.getByLabel("Filter topology graph").fill("fra");
  await expect(
    graphPanel.getByRole("button", { name: /Select core-fra-02/ }),
  ).toBeVisible();
  await page.getByLabel("Topology health filter").selectOption("attention");
  await expect(
    graphPanel.getByText("0 visible tunnels", { exact: true }),
  ).toBeVisible();
  await page.getByLabel("Topology health filter").selectOption("all");
  await page.getByLabel("Filter topology graph").fill("");
  await openConsoleSubpage(page, "Network", "Evidence");
  await expect(
    page.getByRole("heading", { level: 1, name: "Network evidence" }),
  ).toBeVisible();
  const evidence = page.locator(".topologyEvidence");
  const timeline = evidence.getByLabel("Network evidence timeline");
  await expect(timeline.getByText("Evidence timeline")).toBeVisible();
  await expect(
    timeline.getByText("Observation", { exact: true }),
  ).toBeVisible();
  await expect(timeline.getByText("Probe", { exact: true })).toBeVisible();
  await expect(timeline.getByText("Speed test", { exact: true })).toBeVisible();
  await expect(
    timeline.getByText("Status check", { exact: true }),
  ).toBeVisible();
  await expect(
    timeline.getByText("Recommended cost", { exact: true }),
  ).toBeVisible();
  await expect(timeline.getByText("Approval", { exact: true })).toBeVisible();
  await expect(timeline.getByText(/outputs not loaded/)).toBeVisible();
  await expect(
    evidence.getByRole("button", { name: "Load output" }),
  ).toBeVisible();
  await activate(evidence.getByRole("button", { name: "Load output" }));
  for (const label of [
    "Recommendation evidence",
    "Measurement evidence",
    "Status and probe results",
    "Related command jobs",
  ]) {
    await expect(evidence.getByLabel(label)).toBeVisible();
  }
  await expect(
    evidence.getByRole("button", { name: "Compare to previous" }),
  ).toBeVisible();
  await expect(
    evidence.getByRole("button", { name: "Open OSPF" }),
  ).toBeVisible();
  await expect(evidence.getByText("Network probe").first()).toBeVisible();
  await expect(evidence.getByText("1 OSPF update plans")).toBeVisible();
  await expect(evidence.getByText("approval required")).toBeVisible();
  await expect(
    evidence
      .getByText("Apply the reviewed recommendation in Network / OSPF")
      .first(),
  ).toBeVisible();
  await expect(
    evidence.getByText(ospfUpdatePlans[0].recommendation_id),
  ).toHaveCount(0);
  await expect(evidence.getByText("14 -> 22").first()).toBeVisible();
  await expect(evidence.getByText("Confidence Measured").first()).toBeVisible();
  await expect(
    evidence.getByText(/10\.1 Mbps avg - 10% of expected 100 Mbps/).first(),
  ).toBeVisible();
  await expect(evidence.getByText("3 samples")).toBeVisible();
  await expect(
    evidence.getByText("10.1 Mbps avg", { exact: true }),
  ).toBeVisible();
  await expect(
    evidence.getByText("10.9-14.8 ms; 0.25% loss", { exact: true }),
  ).toBeVisible();
  const observationTable = evidence.locator(".observationTable");
  await expect(observationTable.getByText("Network speed test")).toBeVisible();
  await expect(observationTable.getByText("10.1 Mbps")).toBeVisible();
  await expect(observationTable.getByText("12.4 ms")).toBeVisible();
  await expect(observationTable.getByText("0.25% loss")).toBeVisible();
  await expect(
    observationTable.getByText("10.255.0.1", { exact: true }),
  ).toBeVisible();
  await expect(
    observationTable.getByText("Runtime adapter unhealthy"),
  ).toBeVisible();
  await expect(
    observationTable.getByText("Adapter status failed"),
  ).toBeVisible();
  await expect(evidence.getByText("Managed blocks match")).toBeVisible();
  await activate(evidence.getByRole("button", { name: "Open OSPF" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Network OSPF" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Apply cost" })).toBeVisible();
});

test("authors custom adapter tunnel plans from the topology panel", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense topology authoring is covered in the desktop console layout",
  );

  await page.goto("/");
  await page.getByLabel("Search fleet").fill("sfo");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await expect(page.getByText("OSPF cost model")).toBeVisible();
  await expect(
    page.getByText(/Latency\/loss plus a bounded sqrt bandwidth penalty/),
  ).toBeVisible();
  await expect(page.getByText(/speed-test evidence is manual/)).toBeVisible();
  await page.getByRole("button", { name: "Generated config" }).click();
  const generatedConfigReview = page.locator(
    '[aria-label="Generated runtime config review"]',
  );
  await expect(generatedConfigReview).toContainText("Touched files");
  await expect(generatedConfigReview).toContainText("2 files");
  await expect(generatedConfigReview).toContainText("Conflicts");
  await expect(generatedConfigReview).toContainText("None");
  await expect(generatedConfigReview).toContainText("Runtime diff");
  await expect(generatedConfigReview).toContainText("Review-only save");
  const advancedConfig = page.locator(".topologyGeneratedConfigDisclosure");
  await advancedConfig.getByText("Advanced / generated config").click();
  await expect(advancedConfig).toContainText(
    "/etc/network/interfaces.d/vpsman-tunnels",
  );
  await expect(advancedConfig).toContainText("/etc/bird/vpsman-ospf.conf");

  const planGrid = page.getByLabel("Tunnel plans data grid");
  await expect(
    planGrid.getByRole("button", { name: "Plan", exact: true }),
  ).toBeVisible();
  await expect(
    planGrid.getByRole("button", { name: "Desired state", exact: true }),
  ).toBeVisible();
  await expect(
    planGrid.getByRole("button", { name: "Runtime state", exact: true }),
  ).toBeVisible();
  await expect(
    planGrid.getByRole("button", { name: "Health", exact: true }),
  ).toBeVisible();
  await expect(
    planGrid.getByRole("button", { name: "OSPF cost", exact: true }),
  ).toBeVisible();
  await expect(
    planGrid.getByText("Select plan rows for bulk enable, disable, or export."),
  ).toBeVisible();
  await expect(
    planGrid.getByRole("button", { name: "Actions" }),
  ).toBeDisabled();
  const savedPlanRow = planGrid
    .locator(".gridBody [role=row]", { hasText: "sfo-fra-gre" })
    .first();
  await expect(savedPlanRow.getByText("100 Mbps target")).toBeVisible();
  await expect(savedPlanRow.getByText("Runtime sync allowed")).toBeVisible();
  await expect(savedPlanRow.getByText("Both endpoints applied")).toBeVisible();
  await savedPlanRow.getByRole("button", { name: "Disable" }).click();
  await expect(page.getByText("Confirm tunnel plan lifecycle")).toBeVisible();
  await confirmVisiblePrompt(page, "Disable plans");
  await expect(savedPlanRow.getByText("Disabled")).toBeVisible();
  await expect(savedPlanRow.getByText("Runtime sync off")).toBeVisible();
  await savedPlanRow.getByRole("button", { name: "Enable" }).click();
  await expect(page.getByText("Confirm tunnel plan lifecycle")).toBeVisible();
  await confirmVisiblePrompt(page, "Enable plans");
  await expect(savedPlanRow.getByText("Enabled")).toBeVisible();

  const enabledMutations = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { tunnelPlanEnabledMutations: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanEnabledMutations;
  });
  expect(enabledMutations).toMatchObject([
    { enabled: false, plan_id: tunnelPlans[0].id },
    { enabled: true, plan_id: tunnelPlans[0].id },
  ]);

  await page.getByRole("button", { name: "Create tunnel plan" }).click();
  const composer = page.locator(".scheduleComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await composer.scrollIntoViewIfNeeded();
  await expect(composer.getByLabel("OSPF cost preview")).toContainText(
    "OSPF cost",
  );
  await expect(composer.getByLabel("Bandwidth Mbps")).toBeVisible();
  const tunnelWizard = composer.locator('[aria-label="Tunnel plan wizard"]');
  await expect(tunnelWizard).toContainText("1 Endpoints & type");
  await expect(tunnelWizard).toContainText("missing inputs");
  await expect(tunnelWizard).toContainText("2 Addresses & routing");
  await expect(tunnelWizard).toContainText("Allocate or enter CIDRs");
  await expect(tunnelWizard).toContainText("3 Review & create");
  await composer.getByLabel("Name", { exact: true }).fill("external-openvpn");
  await composer.getByLabel("Interface", { exact: true }).fill("ovpn42");
  await composer.getByLabel("Kind").selectOption("openvpn");
  await checkControl(composer.getByLabel("Plan enabled"));
  await chooseVpsBySearch(
    composer,
    "Left VPS",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await chooseVpsBySearch(
    composer,
    "Right VPS",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(
    composer.getByLabel("Left underlay", { exact: true }),
  ).toHaveValue("198.51.100.10");
  await expect(
    composer.getByLabel("Right underlay", { exact: true }),
  ).toHaveValue("203.0.113.20");
  await composer.getByText("Allocation overrides").click();
  await composer
    .getByLabel("IPv4 pool override", { exact: true })
    .fill("10.255.50.0/30");
  await activate(composer.getByRole("button", { name: "Allocate endpoints" }));
  await expect(
    composer.getByLabel("Left IPv4 CIDR", { exact: true }),
  ).toHaveValue("10.255.50.0/31");
  await expect(
    composer.getByLabel("Right IPv4 CIDR", { exact: true }),
  ).toHaveValue("10.255.50.1/31");
  await expect(tunnelWizard).toContainText("No visible overlap");
  await expect(tunnelWizard).toContainText("Save enabled");
  await composer
    .getByLabel("Runtime owner")
    .selectOption("external_managed_adapter");
  await expect(tunnelWizard).toContainText("adapter status argv missing");
  await checkControl(composer.getByLabel("Traffic shaping"));
  await composer.getByLabel("Egress Kbps", { exact: true }).fill("100000");
  await composer.getByLabel("Burst KB", { exact: true }).fill("4096");
  await composer
    .getByLabel("Start argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstart\n{interface}");
  await composer
    .getByLabel("Cleanup argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\ncleanup\n{interface}");
  await composer
    .getByLabel("Status argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstatus\n{interface}");
  await expect(tunnelWizard).toContainText("adapter status argv present");
  await composer
    .getByLabel("Traffic argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nshape\n{interface}");
  await composer.getByText("Network evidence").click();
  await composer
    .getByLabel("Desired interfaces", { exact: true })
    .fill("ovpn42");
  await composer
    .getByLabel("Routes", { exact: true })
    .fill("10.42.0.0/24,dev=ovpn42,metric=42");
  await activate(composer.getByRole("button", { name: "Save plan" }));
  await expect(page.getByText("Confirm tunnel plan save")).toBeVisible();
  await confirmVisiblePrompt(page, "Save plan");
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: { tunnelPlans: unknown[] };
          }
        ).__vpsmanTestRequests;
        return requests.tunnelPlans.length;
      }),
    )
    .toBeGreaterThan(0);

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { tunnelPlans: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.tunnelPlans.at(-1);
  });
  expect(request).toMatchObject({
    enabled: true,
    interface_name: "ovpn42",
    address_pool_cidr: "10.255.50.0/30",
    ipv4_tunnel: {
      left: "10.255.50.0",
      prefix_len: 31,
      right: "10.255.50.1",
    },
    kind: "openvpn",
    latency_primary_family: "ipv4",
    name: "external-openvpn",
    runtime_control: {
      manager: "external_managed_adapter",
      cleanup: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "cleanup",
          "{interface}",
        ],
      },
      startup: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "start",
          "{interface}",
        ],
      },
      status: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "status",
          "{interface}",
        ],
      },
      traffic_limit: {
        burst_kb: 4096,
        egress_kbps: 100000,
      },
      traffic_limit_apply: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "shape",
          "{interface}",
        ],
      },
    },
    runtime_topology: {
      desired_interfaces: ["ovpn42"],
      routes: [
        {
          destination_cidr: "10.42.0.0/24",
          interface_name: "ovpn42",
          metric: 42,
        },
      ],
    },
  });
});

test("promotes saved observed tunnel plans into custom adapters", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense tunnel-plan promotion is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Promotion workflow" }));

  const promotionPanel = page.getByLabel("Tunnel plan promotion workflow");
  const adapterForm = promotionPanel.locator("form", {
    has: page.getByRole("heading", { name: "Custom adapter" }),
  });
  await promotionPanel.scrollIntoViewIfNeeded();
  await expect(
    promotionPanel.getByText("Promotion diff workflow"),
  ).toBeVisible();
  await expect(
    promotionPanel.getByLabel("Topology promotion diff workflow"),
  ).toContainText("Observed source");
  await activate(
    promotionPanel.getByText("Advanced: custom adapter promotion"),
  );
  await adapterForm
    .getByLabel("Observed plan")
    .selectOption("eeeeeeee-ffff-4000-8111-222222222222");
  await adapterForm
    .getByLabel("Name", { exact: true })
    .fill("external-openvpn-managed");
  await adapterForm
    .getByLabel("Status argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstatus\n{interface}");
  await adapterForm.getByText("Lifecycle hooks").click();
  await adapterForm
    .getByLabel("Start argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstart\n{interface}");
  await adapterForm
    .getByLabel("Stop argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstop\n{interface}");
  await adapterForm
    .getByLabel("Cleanup argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\ncleanup\n{interface}");
  await adapterForm.getByText("Traffic shaping").click();
  await adapterForm
    .getByLabel("Traffic argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nshape\n{interface}");
  await checkControl(adapterForm.getByLabel("Enable shaping"));
  await adapterForm.getByLabel("Egress Kbps", { exact: true }).fill("100000");
  await adapterForm.getByLabel("Burst KB", { exact: true }).fill("4096");
  await adapterForm.getByText("Network evidence").click();
  await adapterForm
    .getByLabel("Desired interfaces", { exact: true })
    .fill("ovpn42");
  await activate(
    adapterForm.getByRole("button", { name: "Review custom adapter" }),
  );
  await expect(
    promotionPanel.getByText("Confirm custom adapter"),
  ).toBeVisible();
  await confirmVisiblePrompt(page, "Save custom adapter");

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { tunnelPlanAdapterPromotions: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanAdapterPromotions.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    name: "external-openvpn-managed",
    plan_id: "eeeeeeee-ffff-4000-8111-222222222222",
    runtime_control: {
      manager: "external_managed_adapter",
      cleanup: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "cleanup",
          "{interface}",
        ],
      },
      startup: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "start",
          "{interface}",
        ],
      },
      status: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "status",
          "{interface}",
        ],
      },
      stop: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "stop",
          "{interface}",
        ],
      },
      traffic_limit: {
        burst_kb: 4096,
        egress_kbps: 100000,
      },
      traffic_limit_apply: {
        argv: [
          "/usr/local/libexec/vpsman-openvpn-adapter",
          "shape",
          "{interface}",
        ],
      },
    },
    runtime_topology: {
      desired_interfaces: ["ovpn42"],
    },
  });
});

test("promotes telemetry candidates with explicit activation toggle", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense telemetry promotion is covered inside Network / Tunnel plans",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await activate(page.getByRole("button", { name: "Promotion workflow" }));

  const promotionPanel = page.getByLabel("Tunnel plan promotion workflow");
  const externalForm = promotionPanel.locator("form", {
    has: page.getByRole("heading", { name: "External observe" }),
  });
  await promotionPanel.scrollIntoViewIfNeeded();
  const promotionDiff = promotionPanel.getByLabel(
    "Topology promotion diff workflow",
  );
  await expect(
    promotionPanel.getByText("Promotion diff workflow"),
  ).toBeVisible();
  await expect(
    promotionDiff.getByText("Observed source", { exact: true }),
  ).toBeVisible();
  await expect(
    promotionDiff.getByText("Observed -> saved/proposed", { exact: true }),
  ).toBeVisible();
  await expect(
    promotionDiff.getByText("Review gate", { exact: true }),
  ).toBeVisible();
  await externalForm
    .getByLabel("Observed interface")
    .selectOption("agent-sfo-01:wg-import");
  await chooseVpsBySearch(
    externalForm,
    "External observe peer VPS",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(externalForm.getByLabel("Plan enabled")).not.toBeChecked();
  await externalForm.getByLabel("Self IPv4 CIDR").fill("10.255.60.0/31");
  await externalForm.getByLabel("Peer IPv4 CIDR").fill("10.255.60.1/31");
  await expect(promotionDiff).toContainText("No saved plan match ->");
  await expect(
    promotionDiff.getByText("Ready to review", { exact: true }),
  ).toBeVisible();
  const ospfPreview = externalForm.getByLabel("Promotion OSPF cost preview");
  await expect(ospfPreview).toContainText("30");
  await externalForm.getByLabel("Bandwidth Mbps").fill("10");
  await expect(ospfPreview).toContainText("52");
  await externalForm.getByLabel("Bandwidth Mbps").fill("10000");
  await expect(ospfPreview).toContainText("21");
  await externalForm.getByLabel("Bandwidth Mbps").fill("100");
  await expect(ospfPreview).toContainText("30");

  await activate(
    externalForm.getByRole("button", { name: "Save managed plan" }),
  );
  const prompt = promotionPanel.locator(".confirmationPrompt").last();
  await expect(prompt.getByText("Confirm managed plan")).toBeVisible();
  await expect(prompt.getByText("Deferred", { exact: true })).toBeVisible();
  await confirmVisiblePrompt(page, "Save managed plan");
  await expect(prompt).toBeHidden();

  await expect
    .poll(async () =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: { tunnelPlanTelemetryPromotions: unknown[] };
          }
        ).__vpsmanTestRequests;
        return requests.tunnelPlanTelemetryPromotions.length;
      }),
    )
    .toBe(1);

  let request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { tunnelPlanTelemetryPromotions: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanTelemetryPromotions.at(-1);
  });
  expect(request).toMatchObject({
    client_id: "agent-sfo-01",
    enabled: false,
    interface: "wg-import",
    peer_client_id: "agent-fra-02",
  });

  await checkControl(externalForm.getByLabel("Plan enabled"));
  await activate(
    externalForm.getByRole("button", { name: "Save managed plan" }),
  );
  const enabledPrompt = promotionPanel.locator(".confirmationPrompt").last();
  await expect(enabledPrompt.getByText("Enabled now")).toBeVisible();
  await confirmVisiblePrompt(page, "Save managed plan");

  await expect
    .poll(async () =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: { tunnelPlanTelemetryPromotions: unknown[] };
          }
        ).__vpsmanTestRequests;
        return requests.tunnelPlanTelemetryPromotions.length;
      }),
    )
    .toBe(2);

  request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { tunnelPlanTelemetryPromotions: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanTelemetryPromotions.at(-1);
  });
  expect(request).toMatchObject({
    enabled: true,
    interface: "wg-import",
  });
});

test("shows grouped execution summaries for job output details", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "job detail summary density is covered in the desktop layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "History");
  await activate(page.getByRole("button", { name: "2", exact: true }).first());

  await expect(
    page.getByRole("heading", { name: "Execution summary" }),
  ).toBeVisible();
  await expect(page.getByText(/2 groups across 2 targets/)).toBeVisible();
  await expect(page.getByText("Grouped outcomes")).toBeVisible();
  await expect(page.getByText("Target result details")).toBeVisible();
  const groupedOutcomesGrid = page.getByLabel("Grouped outcomes data grid");
  await expect(groupedOutcomesGrid.locator(".gridCounts")).not.toContainText(
    "selected",
  );
  await expect(
    groupedOutcomesGrid.locator('.gridHeaderGroup input[type="checkbox"]'),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Binary", exact: true }),
  ).toHaveClass(/selected/);

  await activate(page.getByRole("button", { name: "Text", exact: true }));
  await expect(
    page.getByRole("button", { name: "Text", exact: true }),
  ).toHaveClass(/selected/);
  const comparisonRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobOutputComparisons: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.jobOutputComparisons.at(-1);
  });
  expect(comparisonRequest).toMatchObject({ mode: "text" });
});

test("generates local privilege assertions before dispatching a privileged job", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "privileged dispatch flow is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  await expect(
    page.getByRole("heading", { name: "Dispatch command" }),
  ).toBeVisible();
  await unlockPrivilegeFor(page, "Jobs", "Dispatch");
  const topbar = page.locator(".topbar");
  await expect(
    topbar.getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await activate(topbar.getByRole("button", { name: "Lock privilege" }));
  await expect(
    topbar.getByRole("button", { name: "Open privilege unlock" }),
  ).toBeVisible();
  await expect(
    page.locator(".commandComposer").getByLabel("Super password"),
  ).toHaveCount(0);
  await expect(
    page.locator(".commandComposer .privilegeStatus").getByText("Locked", { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".commandComposer").getByRole("button", { name: "Open Privilege Vault" }),
  ).toBeVisible();
  await unlockPrivilegeFor(page, "Jobs", "Dispatch");

  await page.getByLabel("Command argv").fill("/usr/bin/uptime");
  const targetExpression = page.getByLabel("Bulk target selector expression");
  await targetExpression.click();
  await page.keyboard.type("name:s");
  await expect(
    page.getByRole("option", { name: /edge-sfo-01.*Name.*agent-sfo-01/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("name:edge-sfo-01");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("fo01");
  await expect(
    page.getByRole("option", { name: /edge-sfo-01.*ID.*agent-sfo-01/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("id:agent-sfo-01");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("status:on");
  await expect(
    page.getByRole("option", { name: /^status:online$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("status:online");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("vps.status:on");
  await expect(
    page.getByRole("option", { name: /^vps\.status:online$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("vps.status:online");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("role:e");
  await expect(page.getByRole("option", { name: /^role:edge$/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("role:edge");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("*");
  await expect(page.getByRole("option", { name: /^\*$/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("*");
  await targetExpression.fill("");
  await page
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");
  await activate(page.getByRole("button", { name: "Refresh target preview" }));
  await expect(page.getByText("1 resolved targets")).toBeVisible();
  await dispatchWithPrompt(page.locator(".commandComposer"));

  const resultPanel = page.getByLabel("Execution result");
  await expect(resultPanel).toBeVisible();
  await expect(resultPanel.getByText(/completed on 1 VPS/)).toBeVisible();
  await activate(page.getByRole("button", { name: "Open job details" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    argv: ["/usr/bin/uptime"],
    selector_expression: "id:agent-sfo-01",
    command: "shell_argv",
    operation: { argv: ["/usr/bin/uptime"], pty: false, type: "shell" },
    privileged: true,
  });
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
});

test("keeps long search expressions horizontally editable and inspectable", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop expression scrolling covers keyboard and mouse mechanics",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const expression = page.getByRole("searchbox", {
    name: "Bulk target selector expression",
  });
  const longSelector =
    "provider:alpha && country:US && status:online && role:edge && id:agent-sfo-01 || id:agent-fra-02 || id:agent-nyc-03 || " +
    "vps.status:online && vps.provider:alpha && vps.country:US && tag:role:edge && name:edge-sfo-01 || " +
    "id:agent-sfo-01 || id:agent-fra-02 || id:agent-nyc-03";

  await expression.fill(longSelector);
  await expect
    .poll(() =>
      expression.evaluate(
        (element) => element.scrollWidth - element.clientWidth,
      ),
    )
    .toBeGreaterThan(20);
  await expression.press("Home");
  await expect
    .poll(() => expression.evaluate((element) => element.scrollLeft))
    .toBeLessThanOrEqual(2);
  await expression.press("End");
  await expect
    .poll(() => expression.evaluate((element) => element.scrollLeft))
    .toBeGreaterThan(20);

  await page.getByLabel("Command argv").click();
  await expect
    .poll(() =>
      expression.evaluate((element) =>
        element
          .closest(".searchExpressionInput")
          ?.classList.contains("previewing"),
      ),
    )
    .toBe(true);
  await expect(
    expression.locator(".searchExpressionChip").first(),
  ).toBeVisible();
  await expression.evaluate((element) => {
    element.scrollLeft = 0;
  });
  await expression.hover();
  await page.mouse.wheel(0, 500);
  await expect
    .poll(() => expression.evaluate((element) => element.scrollLeft))
    .toBeGreaterThan(20);
});

test("dispatches terminal session control operations with local privilege unlock", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "terminal control dispatch is covered in the desktop job composer",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const composer = page.locator(".commandComposer");
  await unlockPrivilegeFor(page, "Jobs", "Dispatch");
  await activate(composer.getByRole("button", { name: "Terminal" }));
  await composer.getByLabel("Terminal argv").fill("/bin/sh -l");
  await composer.getByLabel("Terminal cwd").fill("/root");
  await composer.getByLabel("Terminal columns").fill("100");
  await composer.getByLabel("Terminal rows").fill("30");
  await composer
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");
  await dispatchWithPrompt(composer);

  await expect(
    page.getByLabel("Execution result").getByText(/completed on 1 VPS/),
  ).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    selector_expression: "id:agent-sfo-01",
    command: "terminal_open",
    operation: {
      argv: ["/bin/sh", "-l"],
      cols: 100,
      cwd: "/root",
      rows: 30,
      type: "terminal_open",
    },
    privileged: true,
  });
  expect(
    (request as { operation: { session_id: string } }).operation.session_id,
  ).toMatch(/[0-9a-f-]{36}/);
  expect(
    (request as { privilege_assertion?: { assertion_hex?: string } })
      .privilege_assertion?.assertion_hex,
  ).toMatch(/^[0-9a-f]+$/);
});

test("previews degraded update targets and sends explicit force override", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "target impact controls are covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  await unlockPrivilegeFor(page, "Jobs", "Dispatch");
  const composer = page.locator(".commandComposer");
  await activate(
    composer.getByLabel("Dispatch operation groups").getByRole("button", {
      name: "Update",
    }),
  );
  await activate(composer.getByRole("button", { name: "Manual update" }));
  await page
    .getByLabel("Agent update artifact URL")
    .fill("https://updates.example/vpsman-agent");
  await page.getByLabel("Agent update SHA-256").fill("a".repeat(64));
  await page
    .locator(".commandComposer")
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-nyc-03");
  await expect(
    page.getByRole("option", { name: /backup-nyc-03.*agent-nyc-03/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await activate(page.getByRole("button", { name: "Refresh target preview" }));

  const impact = page.locator(".commandComposer .targetImpactPreview");
  await expect(impact.getByText("1 target / agent update")).toBeVisible();
  await expect(impact.locator(".targetImpactGroup")).toHaveCount(3);
  await expect(impact.getByText("Needs review")).toBeVisible();
  await expect(impact.getByText("backup-nyc-03")).toBeVisible();

  await checkControl(page.getByLabel("Force unprivileged job best effort"));
  await expect(impact.getByText("Needs review")).toBeVisible();
  await dispatchWithPrompt(page.locator(".commandComposer"));
  await expect(
    page.getByLabel("Execution result").getByText(/unsuccessful on 1 VPS/),
  ).toBeVisible();
  await expect(
    page
      .getByLabel("Failed target reasons")
      .getByText(/stale: agent rejected agent_update command_version 3/),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
        ).__vpsmanTestRequests;
        return requests.jobs.length;
      }),
    )
    .toBeGreaterThan(0);

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    selector_expression: "id:agent-nyc-03",
    command: "agent_update",
    force_unprivileged: true,
    operation: {
      artifact_url: "https://updates.example/vpsman-agent",
      sha256_hex: "a".repeat(64),
      type: "agent_update",
    },
    privileged: true,
  });
});

test("shows audit filters and retention compliance posture", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "audit compliance posture is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Events");

  await expect(
    page.getByRole("heading", { level: 2, name: "Audit log" }),
  ).toBeVisible();
  const auditSummary = page.getByLabel("Audit event summary");
  await expect(auditSummary).toContainText("Visible events");
  await expect(auditSummary).toContainText("Latest visible");
  await expect(auditSummary).toContainText("Related evidence");
  await expect(page.getByLabel("Audit coverage warning")).toContainText(
    "Coverage warning",
  );

  const filters = page.getByLabel("Audit event filters");
  await expect(filters.getByLabel("Audit actor filter")).toBeVisible();
  await expect(filters.getByLabel("Audit action filter")).toBeVisible();
  await expect(filters.getByLabel("Audit resource filter")).toBeVisible();
  await expect(filters.getByLabel("Audit result filter")).toBeVisible();
  await expect(filters.getByLabel("Audit IP filter")).toBeVisible();
  await expect(filters.getByLabel("Audit session filter")).toBeVisible();
  await expect(
    filters.getByLabel("Audit privilege scope filter"),
  ).toBeVisible();
  await expect(filters.getByLabel("Audit from date")).toBeVisible();
  await expect(filters.getByLabel("Audit to date")).toBeVisible();
  await filters.getByLabel("Audit actor filter").fill("console-admin");
  await expect(auditSummary).toContainText("1 active filters");
  await activate(filters.getByRole("button", { name: "Clear" }));
  await expect(filters.getByLabel("Audit actor filter")).toHaveValue("");

  await openConsoleSubpage(page, "Audit", "Retention & export");
  await expect(
    page.getByRole("heading", { level: 2, name: "History retention" }),
  ).toBeVisible();
  const retentionSummary = page.getByLabel("History retention summary");
  await expect(retentionSummary).toContainText("Policy domains");
  await expect(retentionSummary).toContainText("10 enabled / 10");
  await expect(retentionSummary).toContainText("Export enabled");
  await expect(retentionSummary).toContainText("Selected domain");
  await expect(retentionSummary).toContainText("Cleanup review");

  const policies = page.getByLabel("History retention policy table");
  await expect(policies).toContainText("Domain");
  await expect(policies).toContainText("Retention days");
  await expect(policies).toContainText("Metadata only");
  await expect(policies).toContainText("Export enabled");
  await expect(policies).toContainText("Audit logs");
  await expect(policies).toContainText("Job outputs");

  const editor = page.getByLabel("Selected retention domain editor");
  await expect(editor).toContainText("Audit logs");
  await expect(editor).toContainText("Retention days");
  await expect(editor).toContainText("Metadata only");

  const exportScope = page.getByLabel("History retention export scope");
  await expect(exportScope).toContainText("Export scope");
  await expect(exportScope).toContainText("Audit logs");
  await expect(exportScope).toContainText("JSON history bundle");
  await expect(exportScope).toContainText("All retained records");

  const cleanup = page.getByLabel("History retention cleanup workflow");
  await expect(cleanup).toContainText("Evidence retention only");
  await expect(cleanup).toContainText("System / Maintenance");
  await activate(page.getByRole("button", { name: "Preview cleanup" }));
  await expect(retentionSummary).toContainText("0 matched rows / 0 objects");
  await expect(cleanup).toContainText("Would delete 0 metadata rows");

  await activate(page.getByRole("button", { name: "Delete reviewed rows" }));
  const prunePrompt = page.getByLabel("Confirm history prune");
  await expect(prunePrompt).toBeVisible();
  await expect(prunePrompt).toContainText("Reviewed rows");
  await expect(prunePrompt).toContainText("Objects");
  await expect(prunePrompt).toContainText("Effect");
  await expect(prunePrompt).toContainText("Would delete 0 metadata rows");
  await activate(prunePrompt.getByRole("button", { name: "Cancel" }));
});

test("dispatches executable restores with agent-local archive metadata only", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "restore artifact dispatch is covered in the desktop console layout",
  );

  const archivePath =
    "/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.tar";
  const archiveSizeBytes = 512;
  const archiveSha256Hex = "b".repeat(64);
  const destinationRoot = `/var/lib/vpsman/restores/${backupId}/agent-fra-02`;

  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");

  await expect(
    page.getByRole("heading", { name: "Restore operations" }),
  ).toBeVisible();
  const posture = page.getByLabel("Backup posture overview");
  await expect(posture).toContainText("Recent backups");
  await expect(posture).toContainText("0/3");
  await expect(posture).toContainText("Unknown");
  await expect(posture).toContainText("1");
  await expect(posture).toContainText("Unprotected");
  await expect(posture).toContainText("2");
  await expect(posture).toContainText("Artifact storage");
  await expect(posture).toContainText("512 B / 1");
  await expect(posture).toContainText("Restore test");
  await expect(posture).toContainText("Not tested");
  await expect(posture).toContainText("Retention/security");
  await expect(posture).toContainText("API gap");
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Open restore workflow" }));
  const restoreWorkflow = page.getByLabel("Open restore workflow");

  await restoreWorkflow
    .getByLabel("Restore source backup request")
    .selectOption(backupId);
  await chooseVpsBySearch(
    restoreWorkflow,
    "Restore target client",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(restoreWorkflow.getByText(destinationRoot)).toBeVisible();
  await activate(restoreWorkflow.getByRole("button", { name: "Review plan" }));
  await expect(
    restoreWorkflow.getByLabel("Confirm restore plan"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore plan")
      .getByRole("button", { name: "Create restore plan" }),
  );
  await expect(
    page.getByText(/Restore cccccccc planned_metadata_only/),
  ).toBeVisible();
  const restorePlanRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { restorePlans: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.restorePlans.at(-1);
  });
  expect(restorePlanRequest).toMatchObject({
    destination_root: destinationRoot,
    include_config: false,
    paths: ["/etc/hostname"],
    source_backup_request_id: backupId,
    target_client_id: "agent-fra-02",
  });
  expectPrivilegeAssertion(restorePlanRequest);
  const stagedArchive = restoreWorkflow.getByLabel("Staged archive");
  await expect(stagedArchive).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await expect(stagedArchive).toHaveAttribute("title", archivePath);
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("120");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review restore" }),
  );
  await expect(restoreWorkflow.getByLabel("Confirm restore run")).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore run")
      .getByRole("button", { name: "Run restore" }),
  );

  await expect(page.getByText(/Restore job 11111111 running/)).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(JSON.stringify(request)).not.toContain("archive_base64");
  expect(request).toMatchObject({
    argv: [],
    selector_expression: "id:agent-fra-02",
    command: "restore",
    confirmed: true,
    destructive: true,
    operation: {
      archive_path: archivePath,
      archive_sha256_hex: archiveSha256Hex,
      archive_size_bytes: archiveSizeBytes,
      archive_transfer_session_id: "50505050-2222-4333-8444-555555555555",
      destination_root: destinationRoot,
      include_config: false,
      paths: ["/etc/hostname"],
      source_backup_request_id: backupId,
      type: "restore",
    },
    privileged: true,
    max_timeout_secs: 120,
  });
  expectPrivilegeAssertion(request);

  const restoreJobId = "11111111-2222-4333-8444-555555555555";
  const restoreStatusBase64 = Buffer.from(
    JSON.stringify({
      type: "restore",
      rollback_available: true,
      restored_files: [
        {
          archive_path: "/etc/hostname",
          destination_path: `${destinationRoot}/etc/hostname`,
          rollback_path: `${destinationRoot}/etc/.vpsman-restore-hostname.bak`,
          size_bytes: 64,
          sha256_hex: "a".repeat(64),
        },
      ],
    }),
  ).toString("base64");
  await page.evaluate(
    ({ restoreJobId, restoreStatusBase64 }) => {
      const previousFetch = window.fetch.bind(window);
      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        const pathname = new URL(url, window.location.href).pathname;
        if (pathname === `/api/v1/jobs/${restoreJobId}/outputs`) {
          return new Response(
            JSON.stringify({
              items: [
                {
                  client_id: "agent-fra-02",
                  data_base64: restoreStatusBase64,
                  done: true,
                  exit_code: 0,
                  job_id: restoreJobId,
                  seq: 0,
                  stream: "status",
                },
              ],
              limit: 1000,
              next_cursor: null,
              has_more: false,
            }),
            { headers: { "Content-Type": "application/json" }, status: 200 },
          );
        }
        return previousFetch(input, init);
      };
    },
    { restoreJobId, restoreStatusBase64 },
  );
  await expect(
    restoreWorkflow.getByLabel("Restore rollback source job id"),
  ).toHaveValue(restoreJobId);
  await expect(
    restoreWorkflow.getByLabel("Restore rollback target VPS ID"),
  ).toHaveValue("core-fra-02 (ra02)");
  await restoreWorkflow
    .getByLabel("Restore rollback max timeout seconds")
    .fill("45");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review rollback" }),
  );
  await expect(
    restoreWorkflow.getByLabel("Confirm restore rollback"),
  ).toBeVisible();
  await activate(
    restoreWorkflow
      .getByLabel("Confirm restore rollback")
      .getByRole("button", { name: "Rollback restore" }),
  );
  await expect(
    page.getByText(/Restore rollback job 11111111 running/),
  ).toBeVisible();
  const rollbackRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(rollbackRequest)).not.toContain("local-super-password");
  expect(rollbackRequest).toMatchObject({
    argv: [],
    selector_expression: "id:agent-fra-02",
    command: "restore_rollback",
    confirmed: true,
    destructive: true,
    operation: {
      restored_files: [
        {
          archive_path: "/etc/hostname",
          destination_path: `${destinationRoot}/etc/hostname`,
          restored_sha256_hex: "a".repeat(64),
          restored_size_bytes: 64,
          rollback_path: `${destinationRoot}/etc/.vpsman-restore-hostname.bak`,
        },
      ],
      source_restore_job_id: restoreJobId,
      type: "restore_rollback",
    },
    privileged: true,
    max_timeout_secs: 45,
  });
});

test("creates backup artifact transfer package from retained output", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup transfer package controls are covered in the desktop layout",
  );

  const sourceJobId = "99999999-2222-4333-8444-555555555555";

  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Artifacts");

  await activate(page.getByRole("button", { name: "Open artifact workflow" }));
  const artifactWorkflow = page.getByLabel("Open artifact workflow");
  await artifactWorkflow
    .getByLabel("Artifact backup request")
    .selectOption(backupId);
  await artifactWorkflow
    .getByLabel("Backup artifact transfer package source job ID")
    .fill(sourceJobId);
  await activate(
    artifactWorkflow.getByRole("button", { name: "Review transfer package" }),
  );
  await expect(
    artifactWorkflow.getByLabel("Confirm backup artifact transfer package"),
  ).toBeVisible();
  await activate(
    artifactWorkflow
      .getByLabel("Confirm backup artifact transfer package")
      .getByRole("button", { name: "Create transfer package" }),
  );

  await expect(page.getByText(/Artifact dddddddd uploaded/)).toBeVisible();
  const handoffRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { backupArtifactHandoffs: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.backupArtifactHandoffs.at(-1);
  });
  expect(handoffRequest).toMatchObject({
    confirmed: true,
    job_id: sourceJobId,
  });
});

test("dispatches topology network tests and OSPF plan updates with local privilege unlock", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "network test privilege unlock flow is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tests");

  await expect(
    page.getByRole("heading", { name: "Network tests" }),
  ).toBeVisible();
  const networkTestsPanel = page.locator(".fleetPanel", {
    has: page.getByRole("heading", { name: "Network tests" }),
  });
  await expect(networkTestsPanel).toContainText("Required privilege");
  await expect(networkTestsPanel).toContainText("Inspect available");
  await expect(networkTestsPanel).toContainText(
    "100 Mbps, 14 ms target, 0% loss, OSPF 14",
  );
  await expect(networkTestsPanel).toContainText(
    "3s, 16 MiB cap, 100 Mbps cap, TCP 5201, timeout 5000 ms",
  );
  await expect(networkTestsPanel).toContainText(
    "Probe 12.4 ms avg, 0.25% loss",
  );
  await expect(networkTestsPanel).toContainText(
    "Speed 10.1 Mbps avg, 11.8 Mbps max",
  );
  const trendCharts = page.getByLabel("Network test trend charts");
  await expect(trendCharts).toContainText("Trend evidence");
  await expect(trendCharts).toContainText("Latency");
  await expect(trendCharts).toContainText("Packet loss");
  await expect(trendCharts).toContainText("Throughput");
  await expect(trendCharts).toContainText("Single evidence bucket");
  await expect(trendCharts).toContainText(
    "10.1 Mbps avg - 10% of expected 100 Mbps",
  );
  await expect(
    trendCharts.getByRole("button", { name: "Attach evidence" }),
  ).toBeDisabled();

  await page.getByLabel("Network test plan").selectOption(tunnelPlans[0].id);
  await page.getByLabel("Network test endpoint side").selectOption("left");
  await page.getByLabel("Network test max timeout seconds").fill("90");

  await activate(page.getByRole("button", { name: "Inspect status" }));
  await expect(
    page
      .getByLabel("Execution result")
      .last()
      .getByText(/completed on 1 VPS/),
  ).toBeVisible();
  const statusRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(statusRequest)).not.toContain("local-super-password");
  expect(JSON.stringify(statusRequest)).not.toContain("config_backend");
  expect(JSON.stringify(statusRequest)).not.toContain("config_sha256_hex");
  expect(statusRequest).toMatchObject({
    argv: [],
    selector_expression: "id:agent-sfo-01",
    command: "network_status",
    confirmed: false,
    destructive: false,
    operation: {
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_status",
    },
    force_unprivileged: true,
    privilege_assertion: null,
    privileged: false,
    max_timeout_secs: 90,
  });

  await unlockPrivilegeFor(page, "Network", "Tests");
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await expect(networkTestsPanel).toContainText("Probe/speed unlocked");

  await page.getByLabel("Network test plan").selectOption(tunnelPlans[0].id);
  await page.getByLabel("Network test endpoint side").selectOption("left");
  await page.getByLabel("Network test max timeout seconds").fill("90");
  await page.getByLabel("Network probe count").fill("4");
  await page.getByLabel("Network probe interval milliseconds").fill("700");
  await activate(page.getByRole("button", { name: "Run probe" }));
  await expect(
    page
      .getByLabel("Execution result")
      .last()
      .getByText(/completed on 1 VPS/),
  ).toBeVisible();
  const probeRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(probeRequest)).not.toContain("local-super-password");
  expect(probeRequest).toMatchObject({
    argv: [],
    selector_expression: "id:agent-sfo-01",
    command: "network_probe",
    confirmed: false,
    destructive: false,
    operation: {
      count: 4,
      interval_ms: 700,
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_probe",
    },
    privileged: true,
    max_timeout_secs: 90,
  });
  expectPrivilegeAssertion(probeRequest);

  await page.getByLabel("Network speed test duration seconds").fill("5");
  await page.getByLabel("Network speed test max mebibytes").fill("8");
  await page.getByLabel("Network speed test rate limit Kbps").fill("25000");
  await page.getByLabel("Network speed test TCP port").fill("55201");
  await page
    .getByLabel("Network speed test connect timeout milliseconds")
    .fill("2500");
  await expect(networkTestsPanel).toContainText(
    "5s, 8 MiB cap, 25 Mbps cap, TCP 55201, timeout 2500 ms",
  );
  await activate(page.getByRole("button", { name: "Review speed test" }));
  const speedPrompt = page.locator(".confirmationPrompt").last();
  await expect(speedPrompt).toBeVisible();
  await expect(speedPrompt).toContainText("Baseline");
  await expect(speedPrompt).toContainText("Safety cap");
  await expect(speedPrompt).toContainText(
    "5s, 8 MiB cap, 25 Mbps cap, TCP 55201, timeout 2500 ms",
  );
  await expect(speedPrompt).toContainText(
    "network_speed_test unlocked locally",
  );
  await activate(speedPrompt.getByRole("button", { name: "Run speed test" }));
  await expect(
    page
      .getByLabel("Execution result")
      .last()
      .getByText(/completed on 2 VPSs/),
  ).toBeVisible();
  const speedRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(speedRequest)).not.toContain("local-super-password");
  expect(speedRequest).toMatchObject({
    argv: [],
    selector_expression: "id:agent-sfo-01 || id:agent-fra-02",
    command: "network_speed_test",
    confirmed: true,
    destructive: false,
    operation: {
      connect_timeout_ms: 2500,
      duration_secs: 5,
      max_bytes: 8 * 1024 * 1024,
      plan: tunnelPlans[0].plan,
      port: 55201,
      rate_limit_kbps: 25000,
      server_side: "left",
      type: "network_speed_test",
    },
    privileged: true,
    max_timeout_secs: 90,
  });
  expectPrivilegeAssertion(speedRequest);
  await expect(page.getByLabel("Execution result").last()).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Open job details" }).last(),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "OSPF");
  await expect(page.getByRole("heading", { name: "OSPF cost" })).toBeVisible();
  const ospfPanel = page.locator(".fleetPanel", {
    has: page.getByRole("heading", { name: "OSPF cost" }),
  });
  await expect(ospfPanel).toContainText("14 -> 22 (+8)");
  await expect(ospfPanel).toContainText(
    ospfUpdatePlans[0].recommendation_id.slice(0, 8),
  );
  await expect(ospfPanel).toContainText(
    "derived from persisted probe/speed-test trends",
  );
  await expect(ospfPanel).toContainText(
    "12.4 ms avg; 0.25% loss; 10.1 Mbps avg; 11.8 Mbps max",
  );
  await expect(ospfPanel).toContainText("Less preferred by 8");
  await expect(ospfPanel).toContainText(
    "Rollback available after a successful Apply in this panel",
  );
  await expect(
    page.getByRole("button", { name: "Rollback cost" }),
  ).toBeDisabled();
  await expect(ospfPanel).toContainText(
    "After apply, rerun probe/speed tests and verify tunab in Evidence.",
  );
  await page
    .getByLabel("OSPF update plan")
    .selectOption(ospfUpdatePlans[0].plan_id);
  await activate(page.getByRole("button", { name: "Apply cost" }));
  const ospfPrompt = page.locator(".confirmationPrompt").last();
  await expect(ospfPrompt).toBeVisible();
  await expect(ospfPrompt).toContainText("Cost change");
  await expect(ospfPrompt).toContainText("Recommendation ID");
  await expect(ospfPrompt).toContainText("Evidence summary");
  await expect(ospfPrompt).toContainText("Baseline warning");
  await expect(ospfPrompt).toContainText("Traffic impact");
  await expect(ospfPrompt).toContainText("Rollback plan");
  await expect(ospfPrompt).toContainText("Monitor after apply");
  await expect(ospfPrompt).toContainText(
    "approval required; privilege required; reviewed plan only",
  );
  await expect(ospfPrompt).toContainText("network.ospf_cost.apply");
  await activate(ospfPrompt.getByRole("button", { name: "Update cost" }));
  const ospfRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          tunnelPlanOspfCostUpdates: Array<{ plan_id: string; body: unknown }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanOspfCostUpdates.at(-1);
  });
  expect(ospfRequest).toMatchObject({
    plan_id: ospfUpdatePlans[0].plan_id,
    body: {
      confirmed: true,
      mutation_intent: "apply",
      recommendation_id: ospfUpdatePlans[0].recommendation_id,
      current_ospf_cost: ospfUpdatePlans[0].current_ospf_cost,
      recommended_ospf_cost: ospfUpdatePlans[0].recommended_ospf_cost,
    },
  });

  await activate(page.getByRole("button", { name: "Rollback cost" }));
  const rollbackPrompt = page.locator(".confirmationPrompt").last();
  await expect(rollbackPrompt).toContainText("Confirm OSPF rollback");
  await expect(rollbackPrompt).toContainText("Rollback applied recommendation");
  await expect(rollbackPrompt).toContainText("22 -> 14 (-8)");
  await expect(rollbackPrompt).toContainText("network.ospf_cost.rollback");
});
