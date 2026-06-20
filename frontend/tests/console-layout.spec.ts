import { expect, test, type Locator } from "@playwright/test";
import {
  backupId,
  installConsoleApiMock,
  ospfUpdatePlans,
  sha256Hex,
  tunnelPlans,
} from "./support/consoleLayoutFixtures";
import { DEFAULT_UPDATE_VERSION_URL } from "../src/jobDispatchPreset";
import {
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";
import { renderTunnelEndpointConfig } from "../src/topologyApply";

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
  await grid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: action }).click();
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
  const reviewButton = composer.getByRole("button", { name: "Review dispatch" });
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
  await activate(page.getByRole("button", { name: /Fleet health/ }));
  await activate(page.getByRole("button", { name: "Open fleet instances" }));
  await expect(
    page.getByRole("heading", { name: "Fleet overview" }),
  ).toBeVisible();
}

test("renders an operational cloud-console fleet workspace", async ({
  page,
}, testInfo) => {
  await page.goto("/");

  await expect(
    page.getByRole("heading", { name: "Dashboard", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operational Health" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Resource Usage" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Network", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Grouped Statistics" }),
  ).toBeVisible();
  const resourceUsage = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Resource Usage" }),
  });
  await expect(resourceUsage.getByLabel("Resource usage curve")).toBeVisible();
  await activate(resourceUsage.getByRole("button", { name: "Memory" }));
  await expect(resourceUsage).toContainText(/Memory used/);
  const networkSection = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Network", exact: true }),
  });
  await expect(networkSection.getByLabel("Network speed curve")).toBeVisible();
  await activate(networkSection.getByRole("button", { name: "Traffic" }));
  await expect(
    networkSection.getByLabel("Network traffic curve"),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "All", exact: true }));
  await expect(page.getByText(/All VPS; grouped by Labels/)).toBeVisible();
  await expect(page.getByLabel("Dashboard group by")).toBeVisible();
  await expect(page.getByLabel("Dashboard refresh interval")).toBeVisible();
  await expect(page.getByLabel("Dashboard chart point density")).toBeVisible();
  await expect(page.getByLabel("Dashboard scope kind")).toBeVisible();
  await page.getByLabel("Dashboard refresh interval").selectOption("5");
  await page.getByLabel("Dashboard chart point density").selectOption("dense");
  await page.getByLabel("Dashboard group by").selectOption("countries");
  await page.getByLabel("Dashboard scope kind").selectOption("client");
  await chooseVpsBySearch(
    page.locator(".dashboardControlBar"),
    "Dashboard scope value",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await expect(
    page.getByText(/agent-sfo-01; grouped by Countries/),
  ).toBeVisible();
  const dashboardVpsScope = page.getByRole("combobox", {
    name: "Dashboard scope value",
  });
  await dashboardVpsScope.fill("not-a-real-vps");
  await page.keyboard.press("Tab");
  await expect(dashboardVpsScope).toHaveValue("edge-sfo-01 (fo01)");
  await page.getByLabel("Dashboard scope kind").selectOption("provider");
  await page.getByLabel("Dashboard scope value").selectOption("alpha");
  await expect(
    page.getByText(/provider:alpha; grouped by Countries/),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() =>
        JSON.parse(
          window.localStorage.getItem("vpsman.dashboardPreferences") ?? "{}",
        ),
      ),
    )
    .toMatchObject({
      groupBy: "countries",
      networkView: "traffic",
      pointDensity: "dense",
      refreshIntervalSecs: 5,
      resourceMetric: "memory_used",
      scopeKind: "provider",
      scopeValue: "alpha",
      window: "all",
    });
  await expect(
    page.getByRole("button", { name: /Fleet health/ }),
  ).toBeVisible();
  if (testInfo.project.name.includes("mobile")) {
    await openFleetFromDashboard(page);
  } else {
    await activate(page.getByRole("button", { name: /Network activity/ }));
    await expect(
      page.getByRole("heading", { name: "Network activity" }),
    ).toBeVisible();
    await activate(
      page.getByRole("button", { name: "Inspect topology evidence" }),
    );
    await expect(
      page.getByRole("heading", { name: "Topology evidence" }),
    ).toBeVisible();
    await openConsoleSubpage(page, "Fleet", "Instances");
  }

  await expect(
    page.getByRole("heading", { name: "Fleet overview" }),
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
  await expect(edgeRow).toContainText("US");
  await expect(edgeRow.locator(".countryFlag")).toBeVisible();
  await expect(edgeRow).toContainText("alpha");
  await expect(edgeRow).not.toContainText("agent-sfo-01");
  if (testInfo.project.name.includes("desktop")) {
    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await openConsoleSubpage(page, "System", "Preferences");
    await expect(
      page.getByRole("heading", { name: "System preferences", exact: true }),
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
    page.locator(".consoleHeader").getByText("2 online / 3 total"),
  ).toBeVisible();
  await expect(page.getByText("VPS instances")).toBeVisible();
  await expect(page.getByLabel("Fleet alerts")).toHaveCount(0);
  if (testInfo.project.name.includes("desktop")) {
    await openConsoleSubpage(page, "Fleet", "Alerts");
    await expect(page.getByLabel("Fleet alerts", { exact: true })).toBeVisible();
    await expect(page.getByText("Tunnel adapter status failed")).toBeVisible();
    await expect(page.getByText("Agent is not online")).toBeVisible();
    await openConsoleSubpage(page, "Fleet", "Instances");
  }

  const coreRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();
  await activate(coreRow.getByLabel("Expand VPS instance records row"));
  const coreDetail = fleetGrid
    .locator(".gridExpandedRow", { hasText: "core-fra-02" })
    .first();
  await expect(
    coreDetail.getByRole("heading", { name: "core-fra-02 (ra02)" }),
  ).toBeVisible();
  await expect(
    coreDetail.getByRole("tabpanel").getByText("agent-fra-02"),
  ).toBeVisible();

  await activate(coreDetail.getByRole("tab", { name: "Network" }));
  await expect(coreDetail.getByText("BGP/OSPF")).toBeVisible();
  await expect(
    coreDetail.getByText("Client-managed runtime tunnels enabled"),
  ).toBeVisible();
  await expect(coreDetail.getByText("bgp, bird2")).toBeVisible();
  await expect(coreDetail.getByText(/tunab/).first()).toBeVisible();
  await expect(coreDetail.getByText(/Latency Down \/ 100\.0% loss/)).toBeVisible();
  await expect(coreDetail.getByText(/OSPF Report only 14->80/)).toBeVisible();
  await expect(coreDetail.getByText("eth0").first()).toBeVisible();
  await expect(coreDetail.getByText(/RX 8.7 Kbps \/ TX 17 Kbps/)).toBeVisible();

  const backupNetworkRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await activate(
    backupNetworkRow.getByLabel("Expand VPS instance records row"),
  );
  const backupNetworkDetail = fleetGrid
    .locator(".gridExpandedRow", { hasText: "backup-nyc-03" })
    .first();
  await activate(backupNetworkDetail.getByRole("tab", { name: "Network" }));
  await expect(
    backupNetworkDetail.getByText(
      "Unprivileged best-effort, root operations may be ineffective",
    ),
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
  await activate(backupRow.getByLabel("Expand VPS instance records row"));
  const backupDetail = fleetGrid
    .locator(".gridExpandedRow", { hasText: "backup-nyc-03" })
    .first();
  await expect(
    backupDetail.getByRole("heading", { name: "backup-nyc-03 (yc03)" }),
  ).toBeVisible();
  await expect(backupDetail.getByRole("button", { name: "Review VPS deletion" })).toHaveCount(
    0,
  );

  await backupRow.getByLabel("Select VPS instance records row").check();
  await fleetGrid.getByRole("button", { name: "Action" }).click();
  await expect(page.getByRole("menuitem", { name: "Review VPS deletion" })).toBeVisible();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  const prompt = page.locator(".fleetInstancesPanel > .confirmationPrompt");
  await expect(prompt.getByText("Delete VPS from panel")).toBeVisible();
  await expect(prompt).toContainText("deactivates VPS access immediately");
  await activate(prompt.getByRole("button", { name: "Cancel" }));
  await expect(
    fleetGrid.locator(".gridBody [role=row]", { hasText: "backup-nyc-03" }),
  ).toBeVisible();

  await fleetGrid.getByRole("button", { name: "Action" }).click();
  await expect(page.getByRole("menuitem", { name: "Review VPS deletion" })).toBeVisible();
  await page.getByRole("menuitem", { name: "Review VPS deletion" }).click();
  await activate(prompt.getByRole("button", { name: "Delete VPS" }));
  await expect(
    fleetGrid.locator(".gridBody [role=row]", { hasText: "backup-nyc-03" }),
  ).toHaveCount(0);
  await expect(
    page.locator(".consoleHeader").getByText("2 online / 2 total"),
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

  await activate(notifications.getByRole("button", { name: "Review delivery" }));
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
  await activate(notifications.getByRole("button", { name: "Create rule" }).first());
  const webhookExpression = notifications.getByRole("searchbox", {
    name: "Webhook expression",
  });
  await webhookExpression.click();
  await webhookExpression.fill("");
  await page.keyboard.type("interval.");
  await expect(page.getByRole("option", { name: /^interval\.30sec$/ })).toBeVisible();
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

  await activate(notifications.getByRole("button", { name: "Review delivery" }));
  await expect(notifications.getByLabel("Confirm webhook delivery")).toBeVisible();
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
  await page.getByLabel("Dashboard group by").selectOption("countries");
  await page.getByLabel("Dashboard refresh interval").selectOption("5");
  await page.getByLabel("Dashboard chart point density").selectOption("dense");
  await page.evaluate(() => {
    window.localStorage.setItem("vpsman.authVault", "preserved-auth");
    window.localStorage.setItem("vpsman.privilegeVault", "preserved-privilege");
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
  const reloaded = page.waitForEvent("load");
  await page.getByRole("button", { name: "Clear local selections" }).click();
  await reloaded;
  await expect(
    page.getByRole("heading", { name: "Dashboard", exact: true }),
  ).toBeVisible();
  await expect(page.getByText(/All VPS; grouped by Labels/)).toBeVisible();

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
  await coreRow.getByLabel("Expand VPS instance records row").click();
  const coreDetail = grid
    .locator(".gridExpandedRow", { hasText: "agent-fra-02" })
    .first();
  await expect(coreDetail.getByText("agent-fra-02").first()).toBeVisible();
  await expect(coreDetail.getByText("Root uid 0")).toBeVisible();

  await coreRow.getByLabel("Select VPS instance records row").check();
  await expect(grid.getByText("1 selected", { exact: true })).toBeVisible();
  await grid.getByRole("button", { name: "Action" }).click();
  await expect(
    page.getByRole("menuitem", { name: "Copy client IDs" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");

  await grid.getByLabel("VPS instance records columns").click();
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  await expect(
    grid.getByRole("columnheader", { name: /Provider/ }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");

  await coreRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await page.getByRole("menuitem", { name: "Inspect selected" }).click();
  await expect(
    grid.locator(".gridExpandedRow", { hasText: "agent-fra-02" }),
  ).toBeVisible();

  await coreRow.click();
  await expect(
    page.getByRole("heading", { name: "core-fra-02 (ra02)" }),
  ).toHaveCount(0);
  await coreRow.click();
  await expect(
    page.getByRole("heading", { name: "core-fra-02 (ra02)" }),
  ).toBeVisible();
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
  await grid.getByRole("button", { name: "Action" }).click();
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
  await expect(page.getByLabel("Timeout seconds")).toHaveValue("300");
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
  await grid.getByRole("button", { name: "Action" }).click();
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

  const grid = page.getByLabel("Alert policy rules data grid");
  await expect(grid.getByText("1 of 1 policies")).toBeVisible();
  await expect(
    grid.getByRole("columnheader", { name: "Actions" }),
  ).toHaveCount(0);
  await expect(page.getByText("Policy detail")).toHaveCount(0);
  const policySearch = grid.getByRole("searchbox", {
    name: "Alert policy rules search",
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
  await checkControl(policyRow.getByLabel("Select Alert policy rules row"));
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
  await expect(belowDetail).toContainText("mem warn 0.2");
  await page.getByLabel("Close detail panel").click();
  await expect(page.getByText("Alert policy details")).toHaveCount(0);

  await policyRow.getByLabel("Expand Alert policy rules row").click();
  const inlineDetail = grid.locator(".gridExpandedRow");
  await expect(inlineDetail).toContainText("edge-resource-policy");
  await expect(inlineDetail).toContainText("mem warn 0.2");
  await policyRow.getByLabel("Collapse Alert policy rules row").click();
  await expect(inlineDetail).toHaveCount(0);

  await policyRow.click({ button: "right" });
  await expect(page.getByText("Row actions")).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Details" })).toBeVisible();
  await page.keyboard.press("Escape");
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
    page.getByRole("heading", { name: "Dashboard", exact: true }),
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
      page.locator(".navSectionTitle", { hasText: "Operations" }),
    ).toBeVisible();
    await expect(
      page.locator(".navSectionTitle", { hasText: "Network" }),
    ).toBeVisible();
    await expect(
      page.locator(".navSectionTitle", { hasText: "Data & access" }),
    ).toBeVisible();
    await expect(
      page.locator(".navSectionTitle", { hasText: "System" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /All VPS resources/ }),
    ).toBeVisible();
    await expect(
      page.locator(".controlPlanePill", { hasText: "Live control plane" }),
    ).toBeVisible();
  } else {
    await expect(page.locator(".sidebar")).toBeHidden();
    await expect(page.locator(".scopeSelector")).toBeHidden();
    await page.getByRole("combobox", { name: "Console page" }).selectOption("Config::status");
    await expect(
      page.getByRole("heading", { name: "Config", exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Active source status").first()).toBeVisible();
  }
});

test("keeps control-plane metrics in System pages", async ({ page }) => {
  await page.goto("/");

  const dashboard = page.locator(".dashboardWorkspace");
  await expect(
    page.getByRole("heading", { name: "Dashboard", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operational Health" }),
  ).toBeVisible();
  await expect(dashboard.getByText("DB pool", { exact: true })).toHaveCount(0);
  await expect(
    dashboard.getByText("Gateway events", { exact: true }),
  ).toHaveCount(0);

  await openConsoleSubpage(page, "System", "Dashboard");
  await expect(
    page.getByRole("heading", { name: "System dashboard", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Capacity", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Dispatch Lifecycle", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Gateway Events", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Dispatcher in-flight")).toBeVisible();
  await expect(page.getByText("API DB pool")).toBeVisible();

  await openConsoleSubpage(page, "System", "Config");
  await expect(
    page.getByRole("heading", { name: "System config", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "System Config", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Private API bind")).toBeVisible();
  await page.getByLabel("API DB pool").fill("40");
  await page.getByRole("button", { name: "Validate" }).click();
  await expect(page.getByText(/Validation passed/)).toBeVisible();
  await expect(
    page
      .locator(".systemConfigReview")
      .getByText("capacity.api_db_pool")
      .first(),
  ).toBeVisible();
  await expect(page.getByLabel("Suite config validation and save review")).toBeVisible();
  await expect(page.getByText("Unlock in Access")).toBeVisible();
  await expect(page.getByLabel(/super password/i)).toHaveCount(0);
  await expect(page.getByLabel(/super salt/i)).toHaveCount(0);

  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "System", "Config");
  await page.getByLabel("API DB pool").fill("40");
  await page.getByRole("button", { name: "Validate" }).click();
  await expect(page.getByText(/Validation passed/)).toBeVisible();
  await activate(page.getByRole("button", { name: "Review save", exact: true }).first());
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
  expect((suiteConfigRequest as { toml: string }).toml).toContain("api_db_pool = 40");
});

test("surfaces operator users and sessions under System", async ({ page }) => {
  await page.goto("/");

  await unlockPrivilegeFor(page, "System", "Users");
  await expect(
    page.getByRole("heading", { name: "System users", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Users", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("2 operator records")).toBeVisible();
  await expect(page.getByText("365d")).toBeVisible();
  await selectGridRow(page, "Users", "99999999-aaaa-4bbb-8ccc-000000000001");
  await runGridAction(page, "Users", "Edit selected");
  await expect(page.getByLabel("Operator username")).toHaveValue("console-admin");
  await activate(page.getByRole("button", { name: "Disable" }));
  await expect(page.getByText("Preparing review")).toBeVisible();
  await expect(
    page.getByLabel("Confirm admin user action"),
  ).toBeVisible();
  await expect(
    page.getByText(/targets or grants admin privileges/),
  ).toBeVisible();
  await page.getByLabel("Session refresh TTL days").fill("31");
  await expect(page.getByLabel("Confirm admin user action")).toBeHidden();
  await activate(page.getByRole("button", { name: "Disable" }));
  await expect(
    page.getByLabel("Confirm admin user action"),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Cancel" }));

  await unselectGridRow(page, "Users", "99999999-aaaa-4bbb-8ccc-000000000001");
  await selectGridRow(page, "Users", "99999999-aaaa-4bbb-8ccc-000000000002");
  await runGridAction(page, "Users", "Edit selected");
  await expect(page.getByLabel("Operator username")).toHaveValue("noc-operator");
  await expect(page.getByLabel("Operator password")).toHaveAttribute(
    "title",
    /Save does not read or send this field/,
  );
  await expect(page.getByLabel("Session refresh TTL days")).toHaveAttribute(
    "title",
    /Refresh-token\/session lifetime/,
  );
  await expect(page.getByRole("button", { name: "Save", exact: true })).toHaveAttribute(
    "title",
    /never changes the password/,
  );
  await page.getByLabel("Operator password").fill("replacement-password-123");
  await activate(page.getByRole("button", { name: "Save", exact: true }));
  await expect(page.getByText("Preparing review")).toBeVisible();
  const savePrompt = page.getByLabel("Confirm user action");
  await expect(savePrompt).toBeVisible();
  await expect(savePrompt).not.toContainText("replacement-password-123");
  await activate(savePrompt.getByRole("button", { name: "Save user" }));
  const operatorUpdate = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { operatorActions: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.operatorActions.at(-1);
  });
  expect(JSON.stringify(operatorUpdate)).not.toContain("replacement-password-123");
  expect(operatorUpdate).toMatchObject({
    action: "update",
    body: { confirmed: true },
    operator_id: "99999999-aaaa-4bbb-8ccc-000000000002",
  });
  expectPrivilegeAssertion((operatorUpdate as { body?: unknown }).body);

  await openConsoleSubpage(page, "System", "Sessions");
  await expect(
    page.getByRole("heading", { name: "System sessions", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Sessions", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Authentication history", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("console-admin").first()).toBeVisible();
  await expect(page.getByText("unknown-user")).toBeVisible();
  await expect(page.getByText("invalid_credentials")).toBeVisible();
  await selectGridRow(page, "Sessions", "88888888-aaaa-4bbb-8ccc-000000000002");
  await runGridAction(page, "Sessions", "Revoke selected");
  await expect(page.getByText("Preparing review")).toBeVisible();
  const revokePrompt = page.getByLabel("Confirm admin session revoke");
  await expect(revokePrompt).toBeVisible();
  await activate(revokePrompt.getByRole("button", { name: "Revoke session" }));
  const sessionRevoke = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { operatorActions: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.operatorActions.at(-1);
  });
  expect(sessionRevoke).toMatchObject({
    action: "session-revoke",
    body: { admin_risk_acknowledged: true, confirmed: true },
    session_id: "88888888-aaaa-4bbb-8ccc-000000000002",
  });
  expectPrivilegeAssertion((sessionRevoke as { body?: unknown }).body);
});

test("packs dashboard top VPS cards by label length", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop dashboard card packing is the production density target",
  );

  await page.goto("/");
  const resourceUsage = page.locator(".dashboardSection").filter({
    has: page.getByRole("heading", { name: "Resource Usage" }),
  });
  const topVps = resourceUsage.locator(".dashboardTopClients");
  await expect(topVps.getByText("Top VPS")).toBeVisible();

  const layout = await topVps.evaluate((container) => {
    const rows = Array.from(
      container.querySelectorAll<HTMLElement>(".dashboardClientRow"),
    );
    const labels = [
      "db-a",
      "edge-observability-relay-long-production-name-us-west",
      "cache-02",
    ];
    rows.forEach((row, index) => {
      row.querySelector("strong")!.textContent = labels[index] ?? `vps-${index}`;
      row.querySelector("small")!.textContent =
        index === 1
          ? "peak measurement over internet-facing production adapters"
          : "peak";
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

test("manages data-source preset assignments from the config view", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense preset management is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Status");

  const panel = page.locator(".dataSourcePresetPanel");
  const activeSourcesSearchField = panel.getByRole("combobox", {
    name: "Active sources search field",
  });
  const activeSourcesSearch = panel.getByRole("searchbox", {
    name: "Active sources search",
  });
  await expect(
    panel.getByRole("heading", { name: "Active source status" }),
  ).toBeVisible();
  await expect(panel.getByLabel("Active sources table controls")).toBeVisible();
  await expect(activeSourcesSearchField).toBeVisible();
  await expect(activeSourcesSearch).toBeVisible();
  await expect(panel.getByText(/\d+ of \d+ sources/)).toBeVisible();
  await expect(panel.getByText(/Page 1 \/ \d+/).first()).toBeVisible();
  await expect(
    panel
      .locator(".sourceStatusSection .historyRow")
      .filter({ hasText: "shared:vnstat-json" }),
  ).toBeVisible();
  await expect(
    panel.locator(".sourceStatusSection").getByText("vnstat", { exact: true }),
  ).toBeVisible();
  await expect(
    panel
      .locator(".sourceStatusSection")
      .getByText("no server store, 2 artifacts"),
  ).toBeVisible();
  await expect(
    panel
      .locator(".sourceStatusSection")
      .locator(".historyRow")
      .filter({ hasText: "Update artifact source" })
      .filter({ hasText: "ready" }),
  ).toBeVisible();
  await activeSourcesSearchField.selectOption("Preset");
  await activeSourcesSearch.click();
  await page.keyboard.type("vnstat");
  await expect(
    page.getByRole("option", { name: /^shared:vnstat-json$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(activeSourcesSearch).toContainText("shared:vnstat-json");
  await activeSourcesSearch.fill("");
  await activeSourcesSearch.fill("shared:vnstat-json");
  await expect(
    panel
      .locator(".sourceStatusSection .historyRow")
      .filter({ hasText: "shared:vnstat-json" }),
  ).toBeVisible();
  await activeSourcesSearch.fill("");

  await openConsoleSubpage(page, "Config", "Templates");
  const presetPanel = page.locator(".dataSourcePresetPanel");
  const presetRegistrySearchField = presetPanel.getByRole("combobox", {
    name: "Preset registry search field",
  });
  const presetRegistrySearch = presetPanel.getByRole("searchbox", {
    name: "Preset registry search",
  });
  await expect(
    presetPanel.getByRole("heading", { name: "Data-source presets" }),
  ).toBeVisible();
  await expect(
    presetPanel.getByLabel("Preset registry table controls"),
  ).toBeVisible();
  await expect(presetRegistrySearchField).toBeVisible();
  await expect(presetRegistrySearch).toBeVisible();
  await expect(
    presetPanel.locator(".historyRow.dataSourcePresetGrid", {
      hasText: "builtin:interface_counters",
    }),
  ).toBeVisible();
  await expect(
    presetPanel.locator(".historyRow.dataSourcePresetGrid", {
      hasText: "shared:vnstat-json",
    }),
  ).toBeVisible();
  await presetRegistrySearchField.selectOption("Domain");
  await presetRegistrySearch.fill("runtime_traffic_accounting_source");
  await expect(
    presetPanel.locator(".historyRow.dataSourcePresetGrid", {
      hasText: "builtin:interface_counters",
    }),
  ).toBeVisible();
  await presetRegistrySearch.fill("");
  await presetPanel
    .getByLabel("Assignment domain")
    .selectOption("runtime_traffic_accounting_source");
  await presetPanel
    .getByLabel("Preset", { exact: true })
    .selectOption("11111111-1111-4111-8111-111111111111");
  await presetPanel
    .getByRole("searchbox", {
      name: "Data-source assignment target expression",
    })
    .fill("(provider:alpha && country:US) || id:agent-fra-02");
  await expect(presetPanel.getByText("2/3 matching VPSs")).toBeVisible();
  await activate(presetPanel.getByRole("button", { name: "Review assignment" }));
  await expect(
    presetPanel.getByText("Assign data-source preset"),
  ).toBeVisible();
  await confirmVisiblePrompt(page, "Confirm");

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { dataSourcePresetAssignments: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.dataSourcePresetAssignments.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    domain: "runtime_traffic_accounting_source",
    preset_id: "11111111-1111-4111-8111-111111111111",
    selector_expression: "(provider:alpha && country:US) || id:agent-fra-02",
    target_client_ids: ["agent-fra-02", "agent-sfo-01"],
  });
});

test("renders updater rules and submits explicit config apply modes", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "updater config rule editing is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Rules");

  const rules = page.locator(".configRuleWorkspace");
  await expect(rules.getByText("Autonomous updater enabled")).toBeVisible();
  await expect(rules.getByText("Autonomous updater disabled")).toBeVisible();
  const updaterEnabledRule = rules.getByRole("button", {
    name: /Autonomous updater enabled/,
  });
  await activate(updaterEnabledRule);
  await expect(updaterEnabledRule.locator("em")).toHaveText("predefined");
  await expect(rules.getByLabel("Rule render values JSON")).toHaveValue(
    /github\.com\/mnihyc\/vpsman\/releases\/latest\/download\/version\.json/,
  );
  await expect(
    rules.getByRole("button", { name: "Review deletion" }),
  ).toBeDisabled();
  await activate(rules.getByRole("button", { name: "Render patch" }));
  await expect(rules.getByLabel("Rendered rule patch TOML")).toHaveValue(
    /\[update\][\s\S]*unmanaged_enabled = true[\s\S]*version\.json/,
  );

  await unlockPrivilegeFor(page, "Config", "Bulk apply");
  const bulk = page.locator(".configApplyGrid");
  await bulk
    .getByLabel("Rule template")
    .selectOption({ label: "Autonomous updater disabled" });
  await expect(bulk.getByLabel("Rule values JSON")).toHaveValue(
    /github\.com\/mnihyc\/vpsman\/releases\/latest\/download\/version\.json/,
  );
  await activate(bulk.getByRole("button", { name: "Render patch" }));
  await expect(
    bulk.getByLabel("Bulk rendered incremental config patch"),
  ).toHaveValue(
    /\[update\][\s\S]*unmanaged_enabled = false[\s\S]*version\.json/,
  );
  await bulk
    .getByRole("searchbox", { name: "Bulk config selector expression" })
    .fill("id:agent-sfo-01");
  await expect(page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await activate(bulk.getByRole("button", { name: "Review targets" }));
  await expect(bulk.getByText("1/3")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(bulk.getByRole("button", { name: "Review apply" })).toBeEnabled();
  await activate(bulk.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm bulk config apply")).toBeVisible();
  await confirmVisiblePrompt(page, "Apply config patch");

  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: any[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(request).toMatchObject({
    command: "data_source_config_patch",
    operation: {
      apply_mode: "incremental_patch",
      type: "data_source_config_patch",
    },
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });
  expect((request as { operation: { toml: string } }).operation.toml).toContain(
    "[update]",
  );
  expect((request as { operation: { toml: string } }).operation.toml).toContain(
    "unmanaged_enabled = false",
  );
});

test("uses an exact VPS combobox for single config jobs", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "single config combobox behavior is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Single VPS");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Single VPS");

  const targetPicker = page.getByRole("combobox", {
    name: "Single VPS config target",
  });
  await expect(targetPicker).toHaveValue("");
  await targetPicker.fill("not-a-real-vps");
  await targetPicker.blur();
  await expect(targetPicker).toHaveValue("");
  await chooseVpsBySearch(
    page.locator(".configApplyGrid"),
    "Single VPS config target",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(targetPicker).toHaveValue("core-fra-02 (ra02)");
  await activate(page.getByRole("button", { name: "Read config" }));

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
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });

  const configEditor = page.getByLabel("Single VPS redacted config TOML");
  await expect(configEditor).toHaveValue(/client_id = "agent-fra-02"/);
  await expect(configEditor).toHaveValue(
    /unmanaged_version_url = "https:\/\/github\.com\/mnihyc\/vpsman\/releases\/latest\/download\/version\.json"/,
  );
  await activate(page.getByRole("button", { name: "Review apply" }));
  await expect(page.getByText("Confirm single-VPS config apply")).toBeVisible();
  await confirmVisiblePrompt(page, "Apply config");

  const applyRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: any[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(applyRequest).toMatchObject({
    command: "hot_config",
    operation: {
      apply_mode: "full_override",
      base_config_sha256_hex: "b".repeat(64),
      preserve_redacted: true,
      type: "hot_config",
    },
    selector_expression: "id:agent-fra-02",
    target_client_ids: ["agent-fra-02"],
  });
});

test("creates a cron schedule from a command template with target preview", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense schedule composition is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Schedules", "Schedule registry");
  await unlockPrivilegeFor(page, "Schedules", "Schedule registry");

  await activate(page.getByRole("button", { name: "Expand Create schedule" }));
  await page
    .getByLabel("Schedule command template")
    .selectOption("46464646-5656-4789-8abc-defdefdefdef");
  await page.getByLabel("Schedule cron expression").fill("*/15 * * * *");
  await page.getByLabel("Schedule target expression").fill("country:US");
  await expect(page.getByText("2 VPSs in local preview")).toBeVisible();
  await expect(
    page.getByText(/UTC schedule, displayed in browser timezone/),
  ).toBeVisible();
  await expect(
    page.getByText(/2 matching VPSs in local preview; edge-health-check/),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Review save", exact: true }));
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

test("imports direct gateway identities and revokes current keys from the access panel", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense access administration is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Access", "VPS keys");
  const accessTabs = page.locator(".accessTabs");
  await activate(accessTabs.getByRole("button", { name: "VPS keys" }));
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "VPS keys");
  await activate(accessTabs.getByRole("button", { name: "VPS keys" }));

  await expect(
    page.getByRole("heading", { name: "Gateway agent identities" }),
  ).toBeVisible();
  const inspector = page.locator(".accessInspector");
  await expect(inspector.getByText("Direct identity actions")).toBeVisible();
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
    inspector.getByRole("button", { name: "Import gateway identity" }),
  );
  await expect(
    page.getByLabel("Confirm direct gateway identity import"),
  ).toBeVisible();
  await activate(
    page
      .getByLabel("Confirm direct gateway identity import")
      .getByRole("button", { name: "Import identity" }),
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

  await chooseVpsBySearch(
    inspector,
    "VPS key revoke VPS ID",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await inspector.getByLabel("VPS key revoke reason").fill("lost host rebuild");
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

test("rotates an existing agent key through the access panel", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "key rotation is a desktop admin workflow",
  );

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "VPS keys");
  const accessTabs = page.locator(".accessTabs");
  await activate(accessTabs.getByRole("button", { name: "VPS keys" }));

  const accessSubnav = page.locator(".accessSubnav");
  await activate(accessSubnav.getByRole("button", { name: "Key rotation" }));

  const inspector = page.locator(".accessInspector");
  await expect(inspector.getByRole("button", { name: "Rotate key" })).toBeVisible();

  const displayNameInput = inspector.getByLabel("Agent identity display name");
  const tagsInput = inspector.getByLabel("Agent identity tags");
  await expect(displayNameInput).toBeDisabled();
  await expect(tagsInput).toBeDisabled();

  await inspector.getByLabel("Agent identity client ID").fill("agent-sfo-01");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("b".repeat(64));
  await activate(inspector.getByRole("button", { name: "Rotate key" }));
  await expect(
    page.getByLabel("Confirm client key rotation"),
  ).toBeVisible();
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

test("shows topology network evidence, speed metrics, and probe latency history", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "topology evidence drilldown is covered in the desktop console layout",
  );

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Topology" }));

  await expect(
    page.getByRole("heading", { name: "Topology graph" }),
  ).toBeVisible();
  await expect(page.getByRole("img", { name: "Topology graph" })).toBeVisible();
  await expect(
    page.getByText("2 shown / 2 nodes; 1 shown / 1 tunnels"),
  ).toBeVisible();
  await expect(
    page
      .locator(".topologyGraphPanel")
      .getByText("Healthy", { exact: true })
      .first(),
  ).toBeVisible();
  await page.getByLabel("Filter topology graph").fill("fra");
  await expect(
    page
      .locator(".topologyGraphPanel")
      .getByRole("button", { name: /Select core-fra-02/ }),
  ).toBeVisible();
  const graphFilter = page.getByRole("group", {
    name: "Topology health filter",
  });
  await activate(graphFilter.getByRole("button", { name: "Attention" }));
  await expect(
    page.locator(".topologyGraphPanel").getByText("0 visible tunnels"),
  ).toBeVisible();
  await activate(graphFilter.getByRole("button", { name: "All", exact: true }));
  await page.getByLabel("Filter topology graph").fill("");
  await openConsoleSubpage(page, "Topology", "Evidence");
  await expect(
    page.getByRole("heading", { name: "Topology evidence" }),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Refresh evidence" }));
  const evidence = page.locator(".topologyEvidence");
  await expect(evidence.getByText("Network probe").first()).toBeVisible();
  await expect(evidence.getByText("1 OSPF update plans")).toBeVisible();
  await expect(evidence.getByText("approval required")).toBeVisible();
  await expect(evidence.getByText("14 -> 22").first()).toBeVisible();
  await expect(evidence.getByText("3 samples")).toBeVisible();
  await expect(
    evidence.getByText("10.1 Mbps avg", { exact: true }),
  ).toBeVisible();
  await expect(evidence.getByText("10.9-14.8 ms; 0.25% loss")).toBeVisible();
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
});

test("authors external adapter tunnel plans from the topology panel", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense topology authoring is covered in the desktop console layout",
  );

  await page.goto("/");
  await page.getByLabel("Search fleet").fill("sfo");
  await openConsoleSubpage(page, "Topology", "Tunnel plans");
  await expect(page.getByText("OSPF cost model")).toBeVisible();
  await expect(page.getByText(/Latency\/loss plus bandwidth tier/)).toBeVisible();

  const planGrid = page.getByLabel("Tunnel plans data grid");
  const savedPlanRow = planGrid
    .locator(".gridBody [role=row]", { hasText: "sfo-fra-gre" })
    .first();
  await savedPlanRow.getByLabel("Select Tunnel plans row").check();
  await planGrid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Disable plan" }).click();
  await expect(page.getByText("Confirm tunnel plan lifecycle")).toBeVisible();
  await confirmVisiblePrompt(page, "Disable plans");
  await expect(savedPlanRow.getByText("disabled")).toBeVisible();
  await savedPlanRow.getByLabel("Select Tunnel plans row").check();
  await planGrid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Enable plan" }).click();
  await expect(page.getByText("Confirm tunnel plan lifecycle")).toBeVisible();
  await confirmVisiblePrompt(page, "Enable plans");
  await expect(savedPlanRow.getByText("enabled")).toBeVisible();

  const enabledMutations = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { tunnelPlanEnabledMutations: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.tunnelPlanEnabledMutations;
  });
  expect(enabledMutations).toMatchObject([
    { enabled: false, plan_id: tunnelPlans[0].id },
    { enabled: true, plan_id: tunnelPlans[0].id },
  ]);

  const composer = page.locator(".scheduleComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await composer.scrollIntoViewIfNeeded();
  await composer.getByLabel("Name", { exact: true }).fill("external-openvpn");
  await composer.getByLabel("Interface", { exact: true }).fill("ovpn42");
  await composer.getByLabel("Kind").selectOption("openvpn");
  await chooseVpsBySearch(composer, "Left VPS", "sfo", /edge-sfo-01.*agent-sfo-01/);
  await chooseVpsBySearch(composer, "Right VPS", "fra", /core-fra-02.*agent-fra-02/);
  await composer
    .getByLabel("Left underlay", { exact: true })
    .fill("198.51.100.10");
  await composer
    .getByLabel("Right underlay", { exact: true })
    .fill("203.0.113.20");
  await composer
    .getByLabel("IPv4 allocation pool", { exact: true })
    .fill("10.255.50.0/30");
  await activate(composer.getByRole("button", { name: "Generate endpoints" }));
  await expect(composer.getByLabel("Left IPv4", { exact: true })).toHaveValue(
    "10.255.50.0",
  );
  await expect(composer.getByLabel("Right IPv4", { exact: true })).toHaveValue(
    "10.255.50.1",
  );
  await composer
    .getByLabel("Runtime owner")
    .selectOption("external_managed_adapter");
  await checkControl(composer.getByLabel("Enable traffic shaping"));
  await composer.getByLabel("Egress Kbps", { exact: true }).fill("100000");
  await composer.getByLabel("Burst KB", { exact: true }).fill("4096");
  await composer
    .getByLabel("Topology version", { exact: true })
    .fill("provider-a:42");
  await composer
    .getByLabel("Start argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstart\n{interface}");
  await composer
    .getByLabel("Cleanup argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\ncleanup\n{interface}");
  await composer
    .getByLabel("Status argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstatus\n{interface}");
  await composer
    .getByLabel("Traffic argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nshape\n{interface}");
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
          window as unknown as { __vpsmanTestRequests: { tunnelPlans: unknown[] } }
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
      version: "provider-a:42",
    },
  });
});

test("promotes saved observed tunnel plans into adapter contracts", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense topology promotion is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Topology", "Promotion");

  const promotionPanel = page.locator(".scheduleComposer", {
    has: page.getByRole("heading", { name: "Tunnel promotion" }),
  });
  const adapterForm = promotionPanel.locator("form", {
    has: page.getByRole("heading", { name: "Adapter contract" }),
  });
  await promotionPanel.scrollIntoViewIfNeeded();
  await adapterForm
    .getByLabel("Observed plan")
    .selectOption("eeeeeeee-ffff-4000-8111-222222222222");
  await adapterForm
    .getByLabel("Name", { exact: true })
    .fill("external-openvpn-managed");
  await adapterForm
    .getByLabel("Status argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstatus\n{interface}");
  await adapterForm
    .getByLabel("Start argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstart\n{interface}");
  await adapterForm
    .getByLabel("Stop argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nstop\n{interface}");
  await adapterForm
    .getByLabel("Cleanup argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\ncleanup\n{interface}");
  await adapterForm
    .getByLabel("Traffic argv", { exact: true })
    .fill("/usr/local/libexec/vpsman-openvpn-adapter\nshape\n{interface}");
  await checkControl(adapterForm.getByLabel("Enable traffic shaping"));
  await adapterForm.getByLabel("Egress Kbps", { exact: true }).fill("100000");
  await adapterForm.getByLabel("Burst KB", { exact: true }).fill("4096");
  await adapterForm
    .getByLabel("Topology version", { exact: true })
    .fill("adapter:ovpn42");
  await adapterForm
    .getByLabel("Desired interfaces", { exact: true })
    .fill("ovpn42");
  await activate(adapterForm.getByRole("button", { name: "Review promotion" }));
  await expect(
    promotionPanel.getByText("Promote tunnel adapter"),
  ).toBeVisible();
  await confirmVisiblePrompt(page, "Promote adapter");

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
      version: "adapter:ovpn42",
    },
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
    page.locator(".commandComposer").getByRole("button", { name: "Unlock" }),
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
  await expect(
    page.getByRole("option", { name: /^role:edge$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("role:edge");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("*");
  await expect(
    page.getByRole("option", { name: /^\*$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toContainText("*");
  await targetExpression.fill("");
  await page
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");
  await activate(page.getByRole("button", { name: "Review targets" }));
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
        element.closest(".searchExpressionInput")?.classList.contains("previewing"),
      ),
    )
    .toBe(true);
  await expect(expression.locator(".searchExpressionChip").first()).toBeVisible();
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
  await activate(page.getByRole("button", { name: "Manual update" }));
  await page
    .getByLabel("Agent update artifact URL")
    .fill("https://updates.example/vpsman-agent");
  await page.getByLabel("Agent update SHA-256").fill("a".repeat(64));
  await page
    .locator(".commandComposer")
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-nyc-03");
  await expect(page.getByRole("option", { name: /backup-nyc-03.*agent-nyc-03/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await activate(page.getByRole("button", { name: "Review targets" }));

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

test("dispatches executable restores with agent-local archive metadata only", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "restore artifact dispatch is covered in the desktop console layout",
  );

  const archivePath = "/var/lib/vpsman/restores/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.tar";
  const archiveSizeBytes = 512;
  const archiveSha256Hex = "b".repeat(64);
  const destinationRoot = `/var/lib/vpsman/restores/${backupId}/agent-fra-02`;

  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");

  await expect(
    page.getByRole("heading", { name: "Restore operations" }),
  ).toBeVisible();
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Open restore workflow" }));
  const restoreWorkflow = page.getByLabel("Open restore workflow");

  await restoreWorkflow
    .getByLabel("Restore source backup request")
    .selectOption(backupId);
  await chooseVpsBySearch(restoreWorkflow, "Restore target client", "fra", /core-fra-02.*agent-fra-02/);
  await expect(restoreWorkflow.getByText(destinationRoot)).toBeVisible();
  await activate(restoreWorkflow.getByRole("button", { name: "Review plan" }));
  await expect(restoreWorkflow.getByLabel("Confirm restore plan")).toBeVisible();
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
  await restoreWorkflow.getByLabel("Restore timeout seconds").fill("120");
  await activate(restoreWorkflow.getByRole("button", { name: "Review restore" }));
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
    timeout_secs: 120,
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
    .getByLabel("Restore rollback timeout seconds")
    .fill("45");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review rollback" }),
  );
  await expect(restoreWorkflow.getByLabel("Confirm restore rollback")).toBeVisible();
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
    timeout_secs: 45,
  });
});

test("promotes retained backup output into a stored artifact", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "backup handoff controls are covered in the desktop layout",
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
    .getByLabel("Backup artifact handoff source job ID")
    .fill(sourceJobId);
  await activate(
    artifactWorkflow.getByRole("button", { name: "Review promotion" }),
  );
  await expect(
    artifactWorkflow.getByLabel("Confirm retained output promotion"),
  ).toBeVisible();
  await activate(
    artifactWorkflow
      .getByLabel("Confirm retained output promotion")
      .getByRole("button", { name: "Promote retained output" }),
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

test("dispatches topology network apply, rollback, status, probe, and speed test with local privilege unlock", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "network apply privilege unlock flow is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Topology", "Apply / rollback");

  await expect(
    page.getByRole("heading", { name: "Network apply" }),
  ).toBeVisible();
  await unlockPrivilegeFor(page, "Topology", "Apply / rollback");
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();

  await page.getByLabel("Network apply plan").selectOption(tunnelPlans[0].id);
  await page.getByLabel("Network apply endpoint side").selectOption("left");
  await page.getByLabel("Network apply timeout seconds").fill("90");
  await activate(page.getByRole("button", { name: "Review apply" }));
  await confirmVisiblePrompt(page, "Apply side");

  await expect(
    page
      .getByLabel("Execution result")
      .last()
      .getByText(/completed on 1 VPS/),
  ).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    argv: [],
    selector_expression: "id:agent-sfo-01",
    command: "network_apply",
    confirmed: true,
    destructive: true,
    operation: {
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_apply",
    },
    privileged: true,
    timeout_secs: 90,
  });
  const operation = (
    request as {
      operation: {
        bird2_sha256_hex: string;
        config_backend: string;
        config_sha256_hex: string;
        ifupdown_sha256_hex: string;
      };
    }
  ).operation;
  const endpoint = renderTunnelEndpointConfig(tunnelPlans[0].plan, "left");
  expect(operation.ifupdown_sha256_hex).toBe(
    sha256Hex(new TextEncoder().encode(endpoint.ifupdownSnippet)),
  );
  expect(operation.config_backend).toBe("ifupdown");
  expect(operation.config_sha256_hex).toBe(
    sha256Hex(
      new TextEncoder().encode(
        [
          "vpsman-network-backend-file-v1",
          "backend=ifupdown",
          "path=/etc/network/interfaces.d/vpsman-tunnels",
          "kind=ifupdown",
          "contents-sha256-context",
          endpoint.ifupdownSnippet,
          "",
        ].join("\n"),
      ),
    ),
  );
  expect(operation.bird2_sha256_hex).toBe(
    sha256Hex(new TextEncoder().encode(endpoint.bird2InterfaceSnippet)),
  );
  expectPrivilegeAssertion(request);

  await activate(page.getByRole("button", { name: "Review rollback" }));
  await confirmVisiblePrompt(page, "Rollback side");
  await expect(
    page
      .getByLabel("Execution result")
      .last()
      .getByText(/completed on 1 VPS/),
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
    selector_expression: "id:agent-sfo-01",
    command: "network_rollback",
    confirmed: true,
    destructive: true,
    operation: {
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_rollback",
    },
    privileged: true,
    timeout_secs: 90,
  });
  expectPrivilegeAssertion(rollbackRequest);

  await activate(page.getByRole("button", { name: "Review inspect" }));
  await confirmVisiblePrompt(page, "Inspect side");
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
    privileged: true,
    timeout_secs: 90,
  });
  expectPrivilegeAssertion(statusRequest);

  await page.getByLabel("Network probe count").fill("4");
  await page.getByLabel("Network probe interval milliseconds").fill("700");
  await activate(page.getByRole("button", { name: "Review probe" }));
  await confirmVisiblePrompt(page, "Probe latency");
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
    timeout_secs: 90,
  });
  expectPrivilegeAssertion(probeRequest);

  await page.getByLabel("Network speed test duration seconds").fill("5");
  await page.getByLabel("Network speed test max mebibytes").fill("8");
  await page.getByLabel("Network speed test rate limit Kbps").fill("25000");
  await page.getByLabel("Network speed test TCP port").fill("55201");
  await page
    .getByLabel("Network speed test connect timeout milliseconds")
    .fill("2500");
  await activate(page.getByRole("button", { name: "Review speed test" }));
  await confirmVisiblePrompt(page, "Run speed test");
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
    timeout_secs: 90,
  });
  expectPrivilegeAssertion(speedRequest);

  await openConsoleSubpage(page, "Topology", "OSPF");
  await expect(
    page.getByRole("heading", { name: "OSPF cost apply" }),
  ).toBeVisible();
  await unlockPrivilegeFor(page, "Topology", "OSPF");
  await page
    .getByLabel("OSPF update plan")
    .selectOption(ospfUpdatePlans[0].plan_id);
  await page.getByLabel("OSPF update endpoint side").selectOption("left");
  await page.getByLabel("OSPF update timeout seconds").fill("45");
  await activate(page.getByRole("button", { name: "Review cost apply" }));
  await confirmVisiblePrompt(page, "Apply cost");
  await expect(
    page
      .getByLabel("Execution result")
      .last()
      .getByText(/completed on 1 VPS/),
  ).toBeVisible();
  const ospfRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  const proposedPlan = {
    ...tunnelPlans[0].plan,
    recommended_ospf_cost: ospfUpdatePlans[0].recommended_ospf_cost,
  };
  expect(JSON.stringify(ospfRequest)).not.toContain("local-super-password");
  expect(ospfRequest).toMatchObject({
    argv: [],
    selector_expression: "id:agent-sfo-01",
    command: "network_ospf_cost_update",
    confirmed: true,
    destructive: true,
    operation: {
      current_ospf_cost: ospfUpdatePlans[0].current_ospf_cost,
      plan: proposedPlan,
      recommended_ospf_cost: ospfUpdatePlans[0].recommended_ospf_cost,
      side: "left",
      type: "network_ospf_cost_update",
    },
    privileged: true,
    timeout_secs: 45,
  });
  const ospfOperation = (
    ospfRequest as {
      operation: {
        bird2_sha256_hex: string;
        current_ospf_cost: number;
        plan: unknown;
        recommended_ospf_cost: number;
        side: string;
        type: string;
      };
    }
  ).operation;
  expect(ospfOperation.bird2_sha256_hex).toBe(
    sha256Hex(
      new TextEncoder().encode(
        ospfUpdatePlans[0].proposed_left_bird2_interface_snippet,
      ),
    ),
  );
  expectPrivilegeAssertion(ospfRequest);
  await expect(page.getByLabel("Execution result").last()).toBeVisible();
  await activate(page.getByRole("button", { name: "Open job details" }).last());
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();
});
