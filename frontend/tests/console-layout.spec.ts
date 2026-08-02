import { expect, test, type Locator } from "@playwright/test";
import {
  backupId,
  installConsoleApiMock,
  ospfUpdatePlans,
  tunnelPlans,
} from "./support/consoleLayoutFixtures";
import { DEFAULT_UPDATE_VERSION_URL } from "../src/jobDispatchPreset";
import {
  lockPrivilegeFromTop,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
  waitForConsoleShell,
} from "./support/consoleNavigation";
import type { ScheduleRecord, VpsRuleValueRecord } from "../src/types";

const tunnelPortSpeedRules: VpsRuleValueRecord[] = [
  {
    client_id: "agent-sfo-01",
    key: "network.port_speed",
    parsed_display: "1.5 Gbps",
    source_id: null,
    source_kind: "operator",
    state: "ok",
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: "fixture-admin",
    validation_errors: [],
    value_json: { bps: 1_500_000_000, display: "1.5 Gbps" },
    value_raw: "1.5 Gbps",
  },
  {
    client_id: "agent-fra-02",
    key: "network.port_speed",
    parsed_display: "400 Mbps",
    source_id: null,
    source_kind: "operator",
    state: "ok",
    updated_at: "2026-06-02T10:00:00Z",
    updated_by: "fixture-admin",
    validation_errors: [],
    value_json: { bps: 400_000_000, display: "400 Mbps" },
    value_raw: "400 Mbps",
  },
];

const bulkTargetUpdateSchedules: ScheduleRecord[] = [
  {
    cadence_error: null,
    catch_up_limit: 1,
    catch_up_policy: "run_once",
    command_type: "shell_argv",
    created_at: "2026-05-31T09:00:00Z",
    cron_expr: "0 * * * *",
    deferred_until: null,
    deleted_at: null,
    enabled: true,
    failure_count: 0,
    id: "51515151-6161-4717-8abc-defdefdefdef",
    last_error: null,
    last_run_at: "2026-05-31T10:00:00Z",
    max_failures: 3,
    name: "edge-health-hourly",
    next_run_at: "2026-05-31T11:00:00Z",
    next_runs: ["2026-05-31T11:00:00Z"],
    operation: { argv: ["uptime"], pty: false, type: "shell" },
    retry_delay_secs: 300,
    selector_expression: "id:agent-sfo-01 || provider:alpha",
    target_client_ids: ["agent-sfo-01", "agent-fra-02"],
    timezone: "UTC",
    updated_at: "2026-05-31T10:00:00Z",
  },
  {
    cadence_error: null,
    catch_up_limit: 1,
    catch_up_policy: "skip_missed",
    command_type: "shell_argv",
    created_at: "2026-05-31T09:05:00Z",
    cron_expr: "30 * * * *",
    deferred_until: null,
    deleted_at: null,
    enabled: false,
    failure_count: 0,
    id: "52525252-6161-4717-8abc-defdefdefdef",
    last_error: null,
    last_run_at: null,
    max_failures: 3,
    name: "us-capacity-hourly",
    next_run_at: "2026-05-31T11:30:00Z",
    next_runs: ["2026-05-31T11:30:00Z"],
    operation: { argv: ["df", "-h"], pty: false, type: "shell" },
    retry_delay_secs: 300,
    selector_expression: "country:US",
    target_client_ids: ["agent-fra-02"],
    timezone: "UTC",
    updated_at: "2026-05-31T10:00:00Z",
  },
];

test.beforeEach(async ({ page }, testInfo) => {
  const options = testInfo.tags.includes("@delete-request-failure")
    ? {
        agentDeleteRequestFailure: true,
      }
    : testInfo.tags.includes("@delete-cleanup-queue-failure")
      ? {
          agentDeleteFailedClientIds: ["agent-fra-02"],
          agentDeleteSyncJobIds: [],
        }
      : testInfo.tags.includes("@ospf-planned-baseline")
        ? {
            ospfUpdatePlansOverride: ospfUpdatePlans.map((plan) => ({
              ...plan,
              confidence: "no_recent_observations",
              evidence: {
                ...plan.evidence,
                degraded_count: 0,
                healthy_probe_streak: 0,
                latest_observed_at: null,
                sample_count: 0,
              },
              evidence_summary:
                "No recent probe evidence; using the planned cost baseline",
              status: "review_planned_baseline",
            })),
          }
        : undefined;
  await installConsoleApiMock(page, {
    ...options,
    bulkResolveDelayMs: testInfo.tags.includes("@bulk-resolve-delay")
      ? 250
      : undefined,
    bulkResolveFailure: testInfo.tags.includes("@bulk-resolve-failure"),
    configurationSourceApplyFailure: testInfo.tags.includes(
      "@configuration-source-apply-failure",
    ),
    configurationSourceSyncFailure: testInfo.tags.includes(
      "@configuration-source-sync-failure",
    ),
    fleetAlertStateFailure: testInfo.tags.includes(
      "@fleet-alert-state-failure",
    ),
    schedulesOverride: testInfo.tags.includes("@bulk-schedule-targets")
      ? bulkTargetUpdateSchedules
      : undefined,
    portSpeedRulesDelayMs: testInfo.tags.includes("@tunnel-prefill-late")
      ? 400
      : undefined,
    portSpeedRulesOverride: testInfo.tags.some(
      (tag) => tag === "@tunnel-prefill" || tag === "@tunnel-prefill-late",
    )
      ? tunnelPortSpeedRules
      : undefined,
    vpsRulesApplyDelayMs: testInfo.tags.includes("@vps-rules-apply-delay")
      ? 1_000
      : undefined,
  });
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
  const option = root.page().locator(".vpsComboboxMenu").getByRole("option", {
    name: optionName,
  });
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

test("exposes hover titles for truncated grid text and editable values", async ({
  page,
}, testInfo) => {
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Instances");

  const fleetGrid = page.getByLabel("VPS instance records data grid");
  await page.evaluate(() => {
    const input = document.createElement("input");
    input.setAttribute("aria-label", "Agent identity private key");
    input.dataset.testTooltipProbe = "true";
    input.value = "a".repeat(64);
    document.body.append(input);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await expect(
    page.locator('input[data-test-tooltip-probe="true"]'),
  ).not.toHaveAttribute("title", /.+/);
  if (testInfo.project.name.includes("mobile")) {
    const mobilePageSelector = page.getByRole("combobox", {
      name: "Console page",
      exact: true,
    });
    await expect(mobilePageSelector).toHaveAttribute(
      "title",
      /Fleet \/ Instances/,
    );
    await mobilePageSelector.focus();
    await expect(mobilePageSelector).toBeFocused();
    await expect
      .poll(() =>
        page
          .locator(".mobilePageMenu")
          .evaluate((element) => getComputedStyle(element).boxShadow),
      )
      .not.toBe("none");
    const mobileCard = fleetGrid.getByLabel(
      "VPS instance records mobile card agent-sfo-01",
    );
    await expect(mobileCard.locator(".gridMobilePrimary")).toHaveAttribute(
      "title",
      /edge-sfo-01/,
    );
    await expect(
      mobileCard.locator(".gridMobileFieldValue").first(),
    ).toHaveAttribute("title", /.+/);
    return;
  }

  const edgeRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await expect(
    edgeRow.locator(".gridCellContent", { hasText: "edge-sfo-01" }).first(),
  ).toHaveAttribute("title", /edge-sfo-01/);
  const fleetSearch = page.getByRole("combobox", { name: "Search fleet" });
  await fleetSearch.fill("edge-sfo-01");
  await expect(fleetSearch).toHaveAttribute("title", "edge-sfo-01");
});

test("keeps VPS combobox menus above clipped workflow panels", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "overflow escape for dense combobox menus is covered in desktop workflow panels",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");

  const terminalPanel = page.locator(".terminalSessionsPanel");
  const targetPicker = terminalPanel.getByRole("combobox", {
    name: "New terminal target",
  });
  await targetPicker.fill("edge");

  const menu = page.locator(".vpsComboboxMenu");
  await expect(menu).toBeVisible();
  await expect(terminalPanel.locator(".vpsComboboxMenu")).toHaveCount(0);
  const placement = await Promise.all([
    targetPicker.boundingBox(),
    menu.boundingBox(),
  ]);
  expect(placement[0]).not.toBeNull();
  expect(placement[1]).not.toBeNull();
  const inputBox = placement[0]!;
  const menuBox = placement[1]!;
  const verticalGap =
    menuBox.y >= inputBox.y + inputBox.height
      ? menuBox.y - (inputBox.y + inputBox.height)
      : inputBox.y - (menuBox.y + menuBox.height);
  expect(verticalGap).toBeGreaterThanOrEqual(0);
  expect(verticalGap).toBeLessThanOrEqual(6);
  const edgeOption = menu.getByRole("option", { name: /edge-sfo-01/ });
  await expect(edgeOption).toBeVisible();
  await expect(edgeOption).toHaveAttribute(
    "title",
    /edge-sfo-01.*agent-sfo-01/,
  );
});

test("keeps search expression autocomplete above clipped workflow panels", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "overflow escape for dense expression autocomplete is covered in desktop workflow panels",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Sources");

  const panel = page.locator(".configurationSourcesPanel");
  await activate(panel.getByRole("button", { name: "Change configuration" }));
  const drawer = page.getByRole("complementary", {
    name: "Change effective configuration",
  });
  await drawer.getByText("Add targets by selector", { exact: true }).click();
  const targetExpression = drawer.getByRole("combobox", {
    name: "Configuration target selector",
  });
  await targetExpression.fill("provider:");

  const autocomplete = page.locator(".searchExpressionAutocomplete");
  await expect(autocomplete).toBeVisible();
  await expect(drawer.locator(".searchExpressionAutocomplete")).toHaveCount(0);
  const expressionControl = targetExpression.locator(
    "xpath=ancestor::*[contains(concat(' ', normalize-space(@class), ' '), ' searchExpressionInput ')][1]",
  );
  const placement = await Promise.all([
    expressionControl.boundingBox(),
    autocomplete.boundingBox(),
  ]);
  expect(placement[0]).not.toBeNull();
  expect(placement[1]).not.toBeNull();
  const inputBox = placement[0]!;
  const menuBox = placement[1]!;
  const verticalGap =
    menuBox.y >= inputBox.y + inputBox.height
      ? menuBox.y - (inputBox.y + inputBox.height)
      : inputBox.y - (menuBox.y + menuBox.height);
  expect(verticalGap).toBeGreaterThanOrEqual(0);
  expect(verticalGap).toBeLessThanOrEqual(6);
  const providerOption = autocomplete.getByRole("option", {
    name: /^provider:alpha$/,
  });
  await expect(providerOption).toBeVisible();
  await expect(providerOption).toHaveAttribute("title", "provider:alpha");
  await expect(targetExpression).toHaveAttribute("role", "combobox");
  await expect(targetExpression).toHaveAttribute("aria-expanded", "true");
  const options = autocomplete.getByRole("option");
  await expect(options.first()).toHaveAttribute("aria-selected", "true");
  const optionCount = await options.count();
  expect(optionCount).toBeGreaterThan(0);
  await targetExpression.press("ArrowDown");
  const nextOption = options.nth(optionCount > 1 ? 1 : 0);
  await expect(nextOption).toHaveAttribute("aria-selected", "true");
  const selectedSuggestion =
    (await nextOption.locator("span").textContent()) ?? "";
  await targetExpression.press("Enter");
  await expect(targetExpression).toHaveValue(selectedSuggestion);
  await expect(autocomplete).toHaveCount(0);
});

test("renders an operational cloud-console fleet workspace", async ({
  page,
}, testInfo) => {
  await page.goto("/");
  await waitForConsoleShell(page);

  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fleet command home" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home quick actions")).toBeVisible();
  const homePosture = page.getByLabel("Home posture strip");
  await expect(homePosture).toContainText("Live VPS");
  await expect(homePosture).toContainText("0 live, 2 contact unknown");
  await expect(homePosture).not.toContainText("visible online");
  await expect(
    page.getByRole("heading", { name: "Running work" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Recent issues" }),
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
      .filter({ has: page.getByRole("heading", { name: "Recent issues" }) })
      .getByRole("button", { name: /Tunnel adapter status failed/ }),
  ).toBeVisible();
  await expect(page.getByLabel("Home fleet scan")).toBeVisible();
  await expect(page.getByLabel("Home telemetry widgets")).toBeVisible();
  if (testInfo.project.name.includes("mobile")) {
    await openFleetFromDashboard(page);
  } else {
    await activate(
      page.getByRole("button", {
        name: "View all VPS",
      }),
    );
    await expect(
      page.getByRole("heading", { name: "Fleet monitor" }),
    ).toBeVisible();
    await openConsoleSubpage(page, "Fleet", "Instances");
  }

  await expect(
    page.getByRole("heading", { name: "Fleet instances" }),
  ).toBeVisible();
  const fleetInstancesHeader = page.locator(".fleetInstancesHeader");
  await expect(fleetInstancesHeader).toContainText(
    "0 live / 0 access revoked / 2 no contact / 3 total",
  );
  await expect(fleetInstancesHeader).not.toContainText("2 online / 3 total");
  if (testInfo.project.name.includes("desktop")) {
    await expect(
      page.getByRole("combobox", { name: "Search fleet" }),
    ).toBeVisible();
  }
  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const edgeRow = testInfo.project.name.includes("mobile")
    ? fleetGrid.getByLabel("VPS instance records mobile card agent-sfo-01")
    : fleetGrid
        .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
        .first();
  await expect(edgeRow).toBeVisible();
  await expect(edgeRow).toContainText("edge-sfo-01 (fo01)");
  if (testInfo.project.name.includes("mobile")) {
    for (const label of [
      "IP",
      "Last contact",
      "Agent",
      "CPU",
      "Memory",
      "Disk",
      "Alerts",
    ]) {
      await expect(edgeRow).toContainText(label);
    }
    await expect(
      edgeRow.getByRole("button", { name: "Open detail", exact: true }),
    ).toHaveCount(0);
  } else {
    for (const column of [
      "VPS",
      "State",
      "IP",
      "Country",
      "Last contact",
      "Agent",
      "CPU",
      "Memory",
      "Disk",
      "Alerts",
    ]) {
      await expect(
        fleetGrid
          .locator(".gridHeaderCell")
          .filter({ hasText: new RegExp(`^${column}$`) })
          .first(),
      ).toBeVisible();
    }
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
      gateway_endpoints: "primary=gw.example.com:9443=10",
      gateway_server_public_key_hex:
        "1111111111111111111111111111111111111111111111111111111111111111",
      vps_name_display_mode: "name",
    });
    expect(savedPreferences).not.toHaveProperty(
      "tunnel_ipv4_allocation_pool_cidr",
    );
    expect(savedPreferences).not.toHaveProperty(
      "tunnel_ipv6_allocation_pool_cidr",
    );
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
    page
      .locator(".consoleHeader")
      .getByText(
        "0 live / 0 offline / 1 stale / 0 access revoked / 2 no contact / 3 total",
      ),
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

  const coreRecord = testInfo.project.name.includes("mobile")
    ? fleetGrid.getByLabel("VPS instance records mobile card agent-fra-02")
    : fleetGrid
        .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
        .first();
  await selectGridRow(page, "VPS instance records", "agent-fra-02");
  await runGridAction(page, "VPS instance records", "Open detail");
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

  if (testInfo.project.name.includes("mobile")) {
    await coreDetail.getByLabel("VPS detail section").selectOption("Network");
  } else {
    await activate(coreDetail.getByRole("tab", { name: "Network" }));
  }
  await expect(
    coreDetail.getByRole("tabpanel", { name: "Network" }),
  ).toBeVisible();
  await expect(coreDetail).toContainText("Network workflow");
  await expect(
    coreDetail.getByRole("button", { name: "Open network graph" }),
  ).toBeVisible();
  await expect(
    coreDetail.getByRole("button", { name: "Fleet evidence" }),
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
  const preDeletionFleetSnapshot = await page.evaluate(async () => {
    const [summaryResponse, agentsResponse] = await Promise.all([
      fetch("/api/v1/fleet/summary"),
      fetch("/api/v1/agents"),
    ]);
    return {
      agents: await agentsResponse.json(),
      summary: await summaryResponse.json(),
    };
  });
  await page.evaluate(() =>
    localStorage.setItem(
      "vpsman.fileBrowser.state",
      JSON.stringify({
        path: "/var/log",
        showHidden: false,
        targetClientId: "agent-nyc-03",
      }),
    ),
  );
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Fleet", "Instances");

  const fleetGrid = page.getByLabel("VPS instance records data grid");
  const backupRow = fleetGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await backupRow.getByLabel("Select VPS instance records row").check();
  await fleetGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review VPS deletion" }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Open detail", exact: true }),
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
    page
      .locator(".consoleHeader")
      .getByText(
        "0 live / 0 offline / 0 stale / 0 access revoked / 2 no contact / 2 total",
      ),
  ).toBeVisible();
  await expect(
    page.getByText("VPS deleted; tunnel cleanup queued for 1 surviving peer."),
  ).toBeVisible();
  await expect(
    page.locator(".fleetInstancesPanel > .actionFeedbackProgress"),
  ).toContainText("tunnel cleanup queued");

  await page.evaluate((snapshot) => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ type: "fleet_snapshot", ...snapshot }),
      }),
    );
  }, preDeletionFleetSnapshot);
  await expect(
    fleetGrid.locator(".gridBody [role=row]", { hasText: "backup-nyc-03" }),
  ).toHaveCount(0);
  await expect(
    page
      .locator(".consoleHeader")
      .getByText(
        "0 live / 0 offline / 0 stale / 0 access revoked / 2 no contact / 2 total",
      ),
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

  await openConsoleSubpage(page, "Remote Operations", "Files");
  const fileTarget = page.getByRole("combobox", {
    name: "File browser target VPS",
  });
  await expect(fileTarget).not.toHaveValue(/backup-nyc-03|agent-nyc-03/);
  await expect(page.locator(".fileBrowserActionFeedback")).toContainText(
    "agent-nyc-03 is no longer available. Cached file evidence was cleared",
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const stored = JSON.parse(
          localStorage.getItem("vpsman.fileBrowser.state") ?? "{}",
        ) as { targetClientId?: string };
        return stored.targetClientId ?? "";
      }),
    )
    .not.toBe("agent-nyc-03");

  await openConsoleSubpage(page, "Remote Operations", "Processes");
  await activate(
    page.getByRole("group", { name: "Process scope" }).getByRole("button", {
      name: "Managed",
    }),
  );
  await page.evaluate(() => {
    const url = new URL(window.location.href);
    url.searchParams.set("process_mode", "managed");
    url.searchParams.set("process_client", "agent-nyc-03");
    window.history.replaceState(
      window.history.state,
      "",
      `${url.pathname}${url.search}${url.hash}`,
    );
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  const processInventory = page.locator(".fleetPanel", {
    hasText: "Process supervisor inventory",
  });
  await expect(processInventory).toContainText(
    "agent-nyc-03 is no longer available. Process focus and cached rows for that VPS were cleared.",
  );
  await expect(page).not.toHaveURL(/process_client=agent-nyc-03/);
});

test(
  "surfaces exact tunnel cleanup queue failures after VPS deletion",
  { tag: "@delete-cleanup-queue-failure" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "delete feedback behavior is shared after the desktop grid action",
    );

    await page.goto("/");
    await unlockPrivilegeFromTop(page);
    await openConsoleSubpage(page, "Fleet", "Instances");
    await openDeleteVpsReview(page);
    const prompt = page.locator(".fleetInstancesPanel > .confirmationPrompt");
    await activate(prompt.getByRole("button", { name: "Delete VPS" }));

    const warning = page.locator(
      ".fleetInstancesPanel > .actionFeedbackWarning",
    );
    await expect(warning).toContainText("Tunnel cleanup for agent-fra-02");
    await expect(warning).toContainText(
      "Desired state remains saved; inspect API logs and retry",
    );
  },
);

test(
  "keeps an overlay confirmation failure visible after submit",
  { tag: "@delete-request-failure" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "overlay confirmation error persistence is shared across layouts",
    );

    await page.goto("/");
    await openConsoleSubpage(page, "System", "Preferences");
    await page.getByLabel("Review prompt display mode").selectOption("overlay");
    await page.getByRole("button", { name: "Save preferences" }).click();
    await unlockPrivilegeFromTop(page);
    await openConsoleSubpage(page, "Fleet", "Instances");
    await openDeleteVpsReview(page);
    await activate(
      page
        .getByRole("dialog", { name: "Delete VPS from panel" })
        .getByRole("button", { name: "Delete VPS" }),
    );

    await expect(page.locator(".confirmationPromptOverlay")).toHaveCount(0);
    const failure = page.getByRole("alert").filter({
      hasText: "Delete VPS from panel failed",
    });
    await expect(failure).toContainText(
      "Fixture refused the VPS deletion before changing inventory.",
    );
    await expect(
      page
        .getByLabel("VPS instance records data grid")
        .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" }),
    ).toBeVisible();
  },
);

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
  const inlinePrompt = page.getByRole("region", {
    name: "Delete VPS from panel",
  });
  await expect(inlinePrompt).toBeVisible();
  await expect(inlinePrompt).toBeFocused();
  await expect
    .poll(() =>
      inlinePrompt.evaluate((element) => {
        const box = element.getBoundingClientRect();
        return box.top >= 0 && box.bottom <= window.innerHeight;
      }),
    )
    .toBe(true);
  await expect(page.locator(".confirmationPromptOverlay")).toHaveCount(0);
});

test("reviews notification and webhook queue mutations before commit", async ({
  page,
}, testInfo) => {
  testInfo.setTimeout(60_000);
  test.skip(
    testInfo.project.name.includes("mobile"),
    "queue mutation confirmations are covered in the desktop notifications panel",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Observability", "Alerts");
  const notifications = page.locator("main");
  await activate(notifications.getByRole("tab", { name: /Destinations/ }));

  await activate(notifications.getByRole("button", { name: "Queue dispatch" }));
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

  await activate(notifications.getByRole("button", { name: "Deliver queued" }));
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

  await openConsoleSubpage(page, "Observability", "Event webhooks");
  const webhooks = page.locator("main");
  await expect(
    webhooks.getByText("Event webhook rules", { exact: true }).first(),
  ).toBeVisible();
  const webhookRules = webhooks.getByRole("tabpanel", {
    name: /^Rules\b/,
  });
  await activate(
    webhookRules.getByRole("button", { name: "Create rule" }).first(),
  );
  const webhookExpression = webhooks.getByRole("combobox", {
    name: "Webhook expression",
  });
  await webhookExpression.click();
  await webhookExpression.fill("");
  await page.keyboard.type("interval.");
  await expect(
    page.getByRole("option", { name: /^interval\.30sec$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(webhookExpression).toHaveValue("interval.30sec");
  await activate(webhooks.getByLabel("Close detail panel"));

  await activate(webhookRules.getByRole("button", { name: "Send test" }));
  await expect(webhooks.getByLabel("Confirm event webhook test")).toBeVisible();
  await activate(
    webhooks
      .getByLabel("Confirm event webhook test")
      .getByRole("button", { name: "Send test" }),
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

  await activate(webhookRules.getByRole("button", { name: "Retry failed" }));
  await expect(
    webhooks.getByLabel("Confirm failed webhook retry"),
  ).toBeVisible();
  await activate(
    webhooks
      .getByLabel("Confirm failed webhook retry")
      .getByRole("button", { name: "Retry failed" }),
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

  await activate(webhooks.getByRole("tab", { name: "Maintenance" }));
  await activate(webhooks.getByRole("button", { name: "Review rotation" }));
  const reviewCleanup = webhooks.getByRole("button", {
    name: "Review cleanup",
  });
  await expect(reviewCleanup).toBeEnabled();
  await activate(reviewCleanup);
  const cleanupPrompt = page.getByLabel("Delete webhook delivery history");
  await expect(cleanupPrompt).toBeVisible();
  await expect(cleanupPrompt.getByTitle("9".repeat(64))).toBeVisible();
  await activate(
    cleanupPrompt.getByRole("button", {
      name: "Delete retained history",
    }),
  );
  await expect(cleanupPrompt).toBeHidden();
  await expect(reviewCleanup).toBeDisabled();
  await expect(
    webhooks.getByText("not reviewed", { exact: true }),
  ).toBeVisible();
  const rotationRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          webhookDeliveryRotations: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.webhookDeliveryRotations.at(-1);
  });
  expect(rotationRequest).toMatchObject({
    confirmed: true,
    older_than: "2025-12-31T00:00:00.000Z",
    preview_hash: "9".repeat(64),
  });
});

test("clears browser-local console selections without deleting session or privilege records", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "local reset control is covered in the desktop preferences layout",
  );

  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    window.localStorage.setItem(
      "vpsman.privilegeGrant",
      JSON.stringify({
        material: { superKeyHex: "c".repeat(64) },
        operatorId: "99999999-aaaa-4bbb-8ccc-000000000001",
        version: 1,
      }),
    );
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
  await waitForConsoleShell(page);
  await expect(
    page.getByRole("heading", { name: "System preferences" }),
  ).toBeVisible();
  await expect(
    page.locator(".consoleHeader").getByText("vpsman / System / Preferences"),
  ).toBeVisible();

  const storage = await page.evaluate(() => ({
    accessToken: window.localStorage.getItem("vpsman.accessToken"),
    dashboardPreferences: window.localStorage.getItem(
      "vpsman.dashboardPreferences",
    ),
    grid: window.localStorage.getItem("vpsman.grid.example"),
    privilegeGrant: window.localStorage.getItem("vpsman.privilegeGrant"),
    privilegeVault: window.localStorage.getItem("vpsman.privilegeVault"),
    refreshToken: window.localStorage.getItem("vpsman.refreshToken"),
    sidebarSubpanels: window.localStorage.getItem("vpsman.sidebarSubpanels"),
  }));
  const sidebarSubpanels = storage.sidebarSubpanels
    ? JSON.parse(storage.sidebarSubpanels)
    : null;
  const privilegeGrant = storage.privilegeGrant
    ? JSON.parse(storage.privilegeGrant)
    : null;
  expect(storage).toMatchObject({
    accessToken:
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    dashboardPreferences: null,
    grid: null,
    privilegeVault: "preserved-privilege",
    refreshToken:
      "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  });
  expect(sidebarSubpanels).toMatchObject({
    defaultMode: "active",
    state: {},
  });
  expect(privilegeGrant).toMatchObject({
    material: { superKeyHex: "c".repeat(64) },
    operatorId: "99999999-aaaa-4bbb-8ccc-000000000001",
    version: 1,
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
  const fleetAlerts = nav
    .getByLabel("Fleet sections")
    .getByRole("button", { name: "Alerts", exact: true });
  const observabilityAlerts = nav
    .getByLabel("Observability sections")
    .getByRole("button", { name: "Alerts", exact: true });

  await openConsoleSubpage(page, "Observability", "Alerts");
  await expect(observabilityAlerts).toHaveAttribute("aria-current", "page");
  await expect(fleetAlerts).not.toHaveAttribute("aria-current", "page");
  await expect(fleetAlerts).not.toHaveClass(/active/);

  await fleetAlerts.click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Fleet alerts" }),
  ).toBeVisible();
  await expect(fleetAlerts).toHaveAttribute("aria-current", "page");
  await expect(observabilityAlerts).not.toHaveAttribute("aria-current", "page");
  await expect(observabilityAlerts).not.toHaveClass(/active/);
});

test("keeps previously visited sidebar groups expanded", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop sidebar groups are replaced by the mobile page selector",
  );

  await page.goto("/");
  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(nav.getByLabel("Fleet sections")).toBeVisible();

  await openConsoleSubpage(page, "Automation", "Schedules");
  await expect(nav.getByLabel("Automation sections")).toBeVisible();
  await expect(nav.getByLabel("Fleet sections")).toBeVisible();
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
  expect(
    await grid
      .locator('[role="columnheader"]')
      .first()
      .locator(".gridHeaderButton")
      .getAttribute("title"),
  ).toBeNull();
  await grid.getByLabel("VPS instance records search").fill("fra");
  await expect(grid.getByText("1 of 3 instances")).toBeVisible();
  const mobileFleetGrid = testInfo.project.name.includes("mobile");
  const visibleCoreRecord = mobileFleetGrid
    ? grid.getByLabel("VPS instance records mobile card agent-fra-02")
    : grid.locator("[role=row]", { hasText: "core-fra-02" });
  await expect(visibleCoreRecord).toBeVisible();
  await grid.getByLabel("VPS instance records search").fill("");

  const coreRow = mobileFleetGrid
    ? grid.getByLabel("VPS instance records mobile card agent-fra-02")
    : grid.locator(".gridBody [role=row]", { hasText: "core-fra-02" }).first();
  await coreRow.getByLabel("Select VPS instance records row").check();
  await expect(grid.getByText("1 selected", { exact: true })).toBeVisible();
  await grid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Copy client IDs" }),
  ).toBeVisible();
  const actionMenuLayer = await page.evaluate(() => {
    const menu = Array.from(
      document.querySelectorAll<HTMLElement>(".consoleMenu"),
    ).find((element) => element.textContent?.includes("Copy client IDs"));
    const topbar = document.querySelector<HTMLElement>(".topbar");
    return {
      menuZIndex: Number.parseInt(
        window.getComputedStyle(menu as HTMLElement).zIndex,
        10,
      ),
      topbarZIndex: Number.parseInt(
        window.getComputedStyle(topbar as HTMLElement).zIndex,
        10,
      ),
    };
  });
  expect(actionMenuLayer.menuZIndex).toBeGreaterThan(
    actionMenuLayer.topbarZIndex,
  );
  await page.keyboard.press("Escape");

  await grid.getByLabel("VPS instance records columns").click();
  if (!mobileFleetGrid) {
    await expect(
      grid.getByRole("columnheader", { name: /Provider/ }),
    ).toHaveCount(0);
  }
  await page.getByRole("menuitemcheckbox", { name: "Provider" }).click();
  if (!mobileFleetGrid) {
    await expect(
      grid.getByRole("columnheader", { name: /Provider/ }),
    ).toBeVisible();
  } else {
    await expect(coreRow).toContainText("core-fra-02");
  }
  await page.keyboard.press("Escape");

  if (mobileFleetGrid) {
    await runGridAction(page, "VPS instance records", "Open detail");
  } else {
    await coreRow.click({ button: "right" });
    await expect(page.getByText("Row actions")).toBeVisible();
    await page.getByRole("menuitem", { name: "Open detail" }).click();
  }
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

  await selectGridRow(page, "VPS instance records", "agent-sfo-01");
  await runGridAction(page, "VPS instance records", "Open detail");
  await expect(
    page
      .locator(".consoleHeader")
      .getByText("vpsman / Fleet / Instance detail"),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Network" }).click();
  const edgeDetail = page
    .locator(".vpsDetailBlock", { hasText: "Traffic & Rules" })
    .first();
  await expect(
    edgeDetail.getByRole("heading", { name: "Traffic & Rules" }),
  ).toBeVisible();
  await expect(edgeDetail).toContainText("traffic.reset_day");
  await expect(edgeDetail).toContainText("traffic.quota.total");
  await expect(edgeDetail).toContainText("eth0+tx,ens3");
  await expect(edgeDetail).toContainText("Selected traffic");
  await expect(edgeDetail).toContainText("Latest avg RX");
  await expect(edgeDetail).toContainText("Cycle Total");
  const policyDetail = page
    .locator(".vpsDetailBlock", { hasText: "Matched policies" })
    .first();
  await expect(
    policyDetail.getByRole("heading", { name: "Matched policies" }),
  ).toBeVisible();
  await expect(policyDetail).toContainText("Recent policy alerts");
  await expect(policyDetail).toContainText("edge-resource-policy");
  await expect(policyDetail).toContainText("80% total quota");

  await edgeDetail.getByRole("button", { name: "Open Alert Policy" }).click();
  await expect(
    page.getByRole("heading", { name: "Alert policies" }),
  ).toBeVisible();
  await expect(page).toHaveURL(
    /#\/observability\/alert-policy\/fbfbfbfb-1111-4111-8111-111111111111$/,
  );
  await expect(
    page.locator(".consoleDetailPanelHeader strong", {
      hasText: "Alert policy details",
    }),
  ).toBeVisible();
  await expect(page.locator(".consoleDetailPanel").last()).toContainText(
    "edge-resource-policy",
  );
  await page.reload({ waitUntil: "domcontentloaded" });
  await waitForConsoleShell(page);
  await expect(page).toHaveURL(
    /#\/observability\/alert-policy\/fbfbfbfb-1111-4111-8111-111111111111$/,
  );
  await expect(
    page.locator(".consoleDetailPanelHeader strong", {
      hasText: "Alert policy details",
    }),
  ).toBeVisible();
});

test("rehydrates the exact VPS on the canonical Config Rules route", async ({
  page,
}) => {
  await page.goto("/#/config/rules/agent-sfo-01");
  await waitForConsoleShell(page);

  await expect(page).toHaveURL(/#\/config\/rules\/agent-sfo-01$/);
  await expect(page.getByRole("heading", { name: "VPS Rules" })).toBeVisible();
  await expect(page.getByLabel("VPS rules selector expression")).toHaveValue(
    "id:agent-sfo-01",
  );

  await page.reload({ waitUntil: "domcontentloaded" });
  await waitForConsoleShell(page);
  await expect(page).toHaveURL(/#\/config\/rules\/agent-sfo-01$/);
  await expect(page.getByLabel("VPS rules selector expression")).toHaveValue(
    "id:agent-sfo-01",
  );
});

test("clears stale alert-policy detail when a routed policy does not exist", async ({
  page,
}) => {
  await page.goto(
    "/#/observability/alert-policy/fbfbfbfb-1111-4111-8111-111111111111",
  );
  await waitForConsoleShell(page);
  await expect(
    page.locator(".consoleDetailPanelHeader strong", {
      hasText: "Alert policy details",
    }),
  ).toBeVisible();

  await page.evaluate(() => {
    window.location.hash = "#/observability/alert-policy/missing";
  });
  await expect(page).toHaveURL(/#\/observability\/alert-policy\/missing$/);
  await expect(page.getByText("Policy not found: missing")).toBeVisible();
  await expect(
    page.locator(".consoleDetailPanelHeader strong", {
      hasText: "Alert policy details",
    }),
  ).toHaveCount(0);
});

test(
  "supports Config VPS Rules dry-run, confirm, and explicit unset",
  {
    tag: "@vps-rules-apply-delay",
  },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "VPS Rules bulk editor is covered in desktop layout",
    );

    await page.goto("/");
    await openConsoleSubpage(page, "Config", "Rules");

    await expect(
      page.getByRole("heading", { name: "VPS Rules" }),
    ).toBeVisible();
    const grid = page.getByLabel("VPS rule values data grid");
    await expect(grid.getByText("3 of 3 rules")).toBeVisible();
    await expect(
      grid.getByRole("columnheader", { name: /^Actions?$/ }),
    ).toHaveCount(0);
    await expect(grid.getByLabel("VPS rule values columns")).toBeVisible();
    await expect(grid).toContainText("traffic.reset_day");
    await expect(grid).toContainText("traffic.selectors");
    const registryFilters = page.getByLabel("VPS rule registry filters");
    await expect(
      registryFilters.getByLabel("VPS rules selector expression"),
    ).toHaveCount(0);
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
    await expect(
      editor.getByLabel("VPS rules selector expression"),
    ).toBeVisible();
    await expect(page.locator(".vpsRulesMutationScope")).toHaveCSS(
      "border-top-style",
      "solid",
    );
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
      previewBlock.getByText("No changes detected", { exact: true }).first(),
    ).toBeVisible();
    await expect(
      page.getByLabel("VPS rules preview final action"),
    ).toContainText("Apply is disabled");
    await expect(
      page.locator(".confirmationPrompt", {
        hasText: "Confirm VPS rule write",
      }),
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
    const finalAction = page.getByLabel("VPS rules preview final action");
    await expect(finalAction).toContainText("Apply 1 change");
    await expect(finalAction).toContainText("1 effective change");
    await expect(finalAction).toContainText("2 no-ops hidden");
    await expect(
      page.locator(".confirmationPrompt", {
        hasText: "Confirm VPS rule write",
      }),
    ).toHaveCount(0);
    await finalAction.getByRole("button", { name: "Apply 1 change" }).click();
    const applyPrompt = page.locator(".confirmationPrompt", {
      hasText: "Confirm VPS rule write",
    });
    await expect(applyPrompt).toBeVisible();
    await applyPrompt.getByRole("button", { name: "Apply 1 change" }).click();
    await expect(
      page.getByLabel("VPS rules selector expression"),
    ).toBeDisabled();
    await expect(editor.getByLabel("Total quota")).toBeDisabled();
    await expect(
      editor.getByRole("button", { name: "Preview changes", exact: true }),
    ).toBeDisabled();
    await expect(
      page.locator(".vpsRulesActionFeedback.actionFeedbackSuccess"),
    ).toContainText("applied 1 VPS rule changes");
    await expect(previewBlock).toHaveCount(0);
    await expect(finalAction).toHaveCount(0);
    await expect(
      page.getByLabel("VPS rules selector expression"),
    ).toBeEnabled();
    await expect(
      page.locator(".vpsRulesWorkspace > .fleetPolicyStatus"),
    ).toHaveCount(0);

    await editor.getByRole("button", { name: "Unset values" }).click();
    await checkControl(editor.getByLabel("Unset traffic.quota.total"));
    await editor
      .getByRole("button", { name: "Preview changes", exact: true })
      .click();
    const unsetPrompt = page.locator(".confirmationPrompt", {
      hasText: "Confirm VPS rule write",
    });
    await finalAction.getByRole("button", { name: "Apply 1 change" }).click();
    await expect(unsetPrompt).toBeVisible();
    await expect(unsetPrompt.getByTitle("unset")).toBeVisible();
    await page.getByRole("button", { name: "Cancel" }).click();
  },
);

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
  await page
    .locator(".fleetSelectionPanel")
    .getByRole("button", { name: "Check update" })
    .click();

  await expect(
    page
      .locator(".consoleHeader")
      .getByRole("heading", { name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "Bulk target selector expression" }),
  ).toHaveValue("id:agent-fra-02");
  await expect(
    page.getByLabel("Agent update version manifest URL"),
  ).toHaveValue(DEFAULT_UPDATE_VERSION_URL);
  await expect(page.getByLabel("Max timeout seconds")).toHaveValue("300");
  await expect(page.getByText("Version manifest")).toBeVisible();
  await expect(
    page.getByText(/without activating or restarting it/),
  ).toBeVisible();
  await expect(
    page.getByText(/Activation is a separate reviewed action/),
  ).toBeVisible();
  await expect(page.getByLabel(/activate/i)).toHaveCount(0);
  await expect(page.getByLabel(/restart/i)).toHaveCount(0);

  await unlockPrivilegeFromTop(page);
  await expect(
    page.getByText(/without activating or restarting it/),
  ).toBeVisible();
  await page
    .locator("#console-main-content")
    .getByRole("button", { name: "Dispatch", exact: true })
    .click();
  const confirmation = page.getByLabel("Confirm job dispatch");
  await expect(confirmation).toHaveClass(/\bnormal\b/);
  await expect(confirmation).not.toHaveClass(/\bdanger\b/);
  await expect(confirmation).toContainText(
    "Check and stage verified artifact only",
  );
  await expect(confirmation).toContainText("ActivationNo");
  await expect(confirmation).toContainText("Agent restartNo");
  await expect(confirmation).toContainText("Protected operation");
  await confirmation.getByRole("button", { name: "Dispatch job" }).click();

  const updateRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(updateRequest).toMatchObject({
    command: "agent_update_check",
    operation: {
      activate: false,
      restart_agent: false,
      type: "agent_update_check",
      version_url: DEFAULT_UPDATE_VERSION_URL,
    },
  });
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
  await page
    .locator(".fleetSelectionPanel")
    .getByRole("button", { name: "Open dispatch" })
    .click();

  await expect(
    page
      .locator(".consoleHeader")
      .getByRole("heading", { name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "Bulk target selector expression" }),
  ).toHaveValue("id:agent-fra-02");
  await expect(page.getByRole("button", { name: "Argv" })).toHaveClass(
    /selected/,
  );
});

test("keeps fleet alert policy actions selection-scoped", async ({
  page,
}, testInfo) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Observability", "Alerts");

  const grid = page.getByLabel("Policy groups data grid");
  await expect(grid.getByText("1 of 1 policy")).toBeVisible();
  await expect(grid.getByRole("columnheader", { name: "Actions" })).toHaveCount(
    0,
  );
  await expect(page.getByText("Policy detail")).toHaveCount(0);
  const policySearch = grid.getByRole("combobox", {
    name: "Policy groups search",
  });
  await policySearch.click();
  await page.keyboard.type("enabled");
  await expect(page.getByRole("option", { name: /^enabled$/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(policySearch).toHaveValue("enabled");
  await policySearch.fill("");

  const mobilePolicyCard = grid
    .getByLabel(/Policy groups mobile card/)
    .filter({ hasText: "edge-resource-policy" })
    .first();
  const policyRow = testInfo.project.name.includes("mobile")
    ? mobilePolicyCard
    : grid
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
  await expect(page).toHaveURL(
    /#\/observability\/alert-policy\/fbfbfbfb-1111-4111-8111-111111111111$/,
  );
  const belowDetail = page.locator(".consoleDetailPanel");
  await expect(belowDetail).toContainText("edge-resource-policy");
  await expect(belowDetail).toContainText("traffic.cycle.total");
  await expect(belowDetail).toContainText("traffic.quota.total * 0.8");
  await expect(belowDetail).toContainText("Traffic quota threshold reached");
  await page.getByLabel("Close detail panel").click();
  await expect(page.getByText("Alert policy details")).toHaveCount(0);
  await expect(page).toHaveURL(/#\/observability\/alerts$/);

  await page.goBack();
  await expect(page.getByText("Alert policy details")).toBeVisible();
  await page.goForward();
  await expect(page.getByText("Alert policy details")).toHaveCount(0);

  if (!testInfo.project.name.includes("mobile")) {
    await policyRow.getByLabel("Expand Policy groups row").click();
    const inlineDetail = grid.locator(".gridExpandedRow");
    await expect(inlineDetail).toContainText("edge-resource-policy");
    await expect(inlineDetail).toContainText("traffic.cycle.total");
    await policyRow.getByLabel("Collapse Policy groups row").click();
    await expect(inlineDetail).toHaveCount(0);
  }

  await checkControl(policyRow.getByLabel("Select Policy groups row"));
  await grid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();
  const editor = page.locator(".consoleDetailPanel", {
    hasText: "Edit alert policy",
  });
  await expect(editor.getByLabel("Policy VPS selector expression")).toHaveValue(
    "tag:edge",
  );
  await expect(editor.getByLabel("Rule condition expression")).toHaveValue(
    "traffic.cycle.total >= traffic.quota.total * 0.8",
  );
  await editor.getByRole("button", { name: "Preview matches" }).click();
  await expect(editor.getByText("Match preview")).toBeVisible();
  await expect(editor).toContainText("80% total quota");
  await expect(editor).toContainText("edge-sfo-01");
  await editor.getByRole("button", { name: "Update policy" }).click();
  await expect(page.getByText("Confirm alert policy save")).toBeVisible();
  const policyWriteCountBeforeConfirm = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: { fleetAlertPolicies: unknown[] };
      }
    ).__vpsmanTestRequests.fleetAlertPolicies.length;
  });
  await page
    .getByRole("button", { name: "Update alert policy" })
    .evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
  await expect(page.getByText("saved edge-resource-policy")).toBeVisible();
  const policyWriteCountAfterConfirm = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: { fleetAlertPolicies: unknown[] };
      }
    ).__vpsmanTestRequests.fleetAlertPolicies.length;
  });
  expect(policyWriteCountAfterConfirm).toBe(policyWriteCountBeforeConfirm + 1);
  await page.getByLabel("Close detail panel").click();

  if (testInfo.project.name.includes("mobile")) {
    await expect(policyRow.locator(".gridMobileActions")).toHaveCount(0);
  } else {
    await policyRow.click({ button: "right" });
    await expect(page.getByText("Row actions")).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "Details" })).toBeVisible();
    await page.keyboard.press("Escape");
  }
});

test(
  "keeps a failed fleet alert triage inside its reviewed confirmation",
  { tag: "@fleet-alert-state-failure" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "the same triage handler is covered through the desktop grid action",
    );

    await page.goto("/");
    await openConsoleSubpage(page, "Fleet", "Alerts");

    const grid = page.getByLabel("Fleet alerts data grid");
    const alertRow = grid
      .getByRole("row")
      .filter({ hasText: "Tunnel adapter status failed" })
      .first();
    await alertRow.getByRole("checkbox").check();
    await grid.getByRole("button", { name: "Actions", exact: true }).click();
    await activate(
      page.getByRole("menuitem", {
        name: "Acknowledge open",
        exact: true,
      }),
    );

    const prompt = page.getByLabel("Confirm fleet alert triage");
    await activate(prompt.getByRole("button", { name: "Acknowledge" }));
    await expect(prompt).toBeVisible();
    await expect(prompt.locator(".confirmationPromptError")).toContainText(
      "Simulated fleet alert triage failure",
    );
  },
);

test("exposes console route state through URL, browser history, and reload", async ({
  page,
}) => {
  await page.goto("/#/network/tunnel-plans");
  await waitForConsoleShell(page);

  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/network\/tunnel-plans$/);

  await page.evaluate(() => {
    window.location.hash = "#/jobs/dispatch";
  });
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/jobs\/dispatch$/);

  await page.goBack();
  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/network\/tunnel-plans$/);

  await page.goForward();
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/jobs\/dispatch$/);

  await page.goBack();
  await page.reload({ waitUntil: "domcontentloaded" });
  await waitForConsoleShell(page);
  await expect(
    page.getByRole("heading", { level: 1, name: "Tunnel plans" }),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/network\/tunnel-plans$/);
});

test("isolates untouched dispatch defaults between browser history entries", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "vpsman.jobDispatch.selectorExpression",
      "id:agent-sfo-01",
    );
  });
  await page.goto("/#/jobs/dispatch");
  await waitForConsoleShell(page);
  const selector = page.getByLabel("Bulk target selector expression");
  await expect(selector).toHaveValue("id:agent-sfo-01");

  await openConsoleSubpage(page, "Jobs", "History");
  await openConsoleSubpage(page, "Jobs", "Dispatch");
  await selector.fill("id:agent-fra-02");
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.localStorage.getItem("vpsman.jobDispatch.selectorExpression"),
      ),
    )
    .toBe("id:agent-fra-02");

  await page.goBack();
  await page.goBack();
  await expect(selector).toHaveValue("id:agent-sfo-01");

  await page.goForward();
  await page.goForward();
  await expect(selector).toHaveValue("id:agent-fra-02");
});

test("keeps an exact job detail identity through browser history and reload", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the desktop job grid is the canonical job-detail entry point",
  );
  const jobId = "77777777-aaaa-4bbb-8ccc-dddddddddddd";

  await page.goto("/#/jobs/history");
  await waitForConsoleShell(page);
  const grid = page.getByLabel("Job records data grid");
  const row = grid
    .locator(".gridBody [role=row]", { hasText: "network speed test" })
    .first();
  await row.getByRole("checkbox").check();
  await runGridAction(page, "Job records", "Open target detail");

  await expect(page).toHaveURL(new RegExp(`#\\/jobs\\/history\\/${jobId}$`));
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();

  await page.goBack();
  await expect(page).toHaveURL(/#\/jobs\/history$/);
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toHaveCount(0);

  await page.goForward();
  await expect(page).toHaveURL(new RegExp(`#\\/jobs\\/history\\/${jobId}$`));
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();

  await page.reload({ waitUntil: "domcontentloaded" });
  await waitForConsoleShell(page);
  await expect(page).toHaveURL(new RegExp(`#\\/jobs\\/history\\/${jobId}$`));
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();
});

test("centers and focuses the active left subpanel after navigation", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the compact mobile page selector replaces the desktop left subpanel",
  );
  await page.setViewportSize({ height: 520, width: 1440 });
  await page.goto("/#/automation/agent-updates");
  await waitForConsoleShell(page);

  const initialSubpanel = page
    .getByLabel("Automation sections")
    .getByRole("button", { name: "Agent updates", exact: true });
  await expect(initialSubpanel).not.toBeFocused();
  const sidebar = page.locator(".sidebar");
  await sidebar.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await activate(
    page
      .getByLabel("Agent update rollout posture")
      .getByRole("button", { name: "Update jobs" }),
  );
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();

  const activeSubpanel = page
    .getByLabel("Jobs sections")
    .getByRole("button", { name: "History", exact: true });
  await expect(activeSubpanel).toBeFocused();
  await expect
    .poll(async () => {
      const [sidebarBounds, subpanelBounds] = await Promise.all([
        sidebar.boundingBox(),
        activeSubpanel.boundingBox(),
      ]);
      if (!sidebarBounds || !subpanelBounds) {
        return Number.POSITIVE_INFINITY;
      }
      const sidebarCenter = sidebarBounds.y + sidebarBounds.height / 2;
      const subpanelCenter = subpanelBounds.y + subpanelBounds.height / 2;
      return Math.abs(sidebarCenter - subpanelCenter);
    })
    .toBeLessThan(48);

  const jobsNavGroup = page.locator(".navGroup").filter({
    has: page.getByRole("button", { name: "Jobs", exact: true }),
  });
  await jobsNavGroup.getByRole("button", { name: "Collapse subpages" }).click();
  await expect(page.getByLabel("Jobs sections")).toHaveCount(0);
  await page.evaluate(() => {
    window.location.hash = "#/jobs/dispatch";
  });
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
  await expect(
    page
      .getByLabel("Jobs sections")
      .getByRole("button", { name: "Dispatch", exact: true }),
  ).toBeFocused();

  await page.locator(".content").focus();
  await page.goBack();
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(activeSubpanel).toBeFocused();

  await page.locator(".content").focus();
  await page.goBack();
  await expect(
    page.getByRole("heading", { level: 1, name: "Agent updates" }),
  ).toBeVisible();
  await expect(initialSubpanel).toHaveAttribute("aria-current", "page");
  await expect(initialSubpanel).toBeFocused();
  await expect
    .poll(async () => {
      const [sidebarBounds, subpanelBounds] = await Promise.all([
        sidebar.boundingBox(),
        initialSubpanel.boundingBox(),
      ]);
      if (!sidebarBounds || !subpanelBounds) {
        return Number.POSITIVE_INFINITY;
      }
      const sidebarCenter = sidebarBounds.y + sidebarBounds.height / 2;
      const subpanelCenter = subpanelBounds.y + subpanelBounds.height / 2;
      return Math.abs(sidebarCenter - subpanelCenter);
    })
    .toBeLessThan(48);
});

test("reconnects the live console stream after an interrupted socket", async ({
  page,
}) => {
  await page.goto("/#/fleet/instances");
  await waitForConsoleShell(page);
  await expect(page.getByText("Console stream connected")).toBeVisible();

  await page.evaluate(() => {
    const sockets = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<{ close: () => void }>;
      }
    ).__vpsmanTestWebSockets;
    sockets.at(-1)?.close();
  });

  await expect(page.getByText("Console stream reconnecting")).toBeVisible();
  await expect(page.getByText("Console stream connected")).toBeVisible({
    timeout: 5_000,
  });
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

  await openConsoleSubpage(page, "Observability", "Event webhooks");
  await expect(
    page.getByRole("heading", { name: "Event webhook rules" }),
  ).toBeVisible();
  await expect(page.getByLabel("Webhook rules data grid")).toContainText(
    "edge-interval-webhook",
  );
});

test("keeps console layout usable on desktop and mobile widths", async ({
  page,
}, testInfo) => {
  await page.goto("/");
  await waitForConsoleShell(page);

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
    const quickStatLabelOverflow = await page
      .locator(".quickStats .metric span")
      .evaluateAll((labels) =>
        labels
          .filter((label) => label.scrollWidth > label.clientWidth + 1)
          .map((label) => label.textContent?.trim() ?? ""),
      );
    expect(quickStatLabelOverflow).toEqual([]);
    const sidebarBox = await page.locator(".sidebar").boundingBox();
    expect(sidebarBox?.x).toBe(0);
    expect(sidebarBox?.y).toBe(0);
    const appShellScrollState = await page.evaluate(() => {
      const shell = document.querySelector(".shell") as HTMLElement;
      const sidebar = document.querySelector(".sidebar") as HTMLElement;
      const content = document.querySelector(".content") as HTMLElement;
      const topbar = document.querySelector(".topbar") as HTMLElement;
      return {
        contentHeight: content.getBoundingClientRect().height,
        contentOverflowY: getComputedStyle(content).overflowY,
        shellOverflow: getComputedStyle(shell).overflow,
        sidebarHeight: sidebar.getBoundingClientRect().height,
        sidebarOverflowY: getComputedStyle(sidebar).overflowY,
        topbarPosition: getComputedStyle(topbar).position,
        viewportHeight: window.innerHeight,
      };
    });
    expect(appShellScrollState.shellOverflow).toBe("hidden");
    expect(appShellScrollState.sidebarOverflowY).toBe("auto");
    expect(appShellScrollState.contentOverflowY).toBe("auto");
    expect(appShellScrollState.topbarPosition).toBe("sticky");
    expect(
      Math.abs(
        appShellScrollState.sidebarHeight - appShellScrollState.viewportHeight,
      ),
    ).toBeLessThanOrEqual(1);
    expect(
      Math.abs(
        appShellScrollState.contentHeight - appShellScrollState.viewportHeight,
      ),
    ).toBeLessThanOrEqual(1);
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
      page.locator(".controlPlanePill", { hasText: "Live" }),
    ).toBeVisible();
  } else {
    await expect(page.locator(".sidebar")).toBeHidden();
    await expect(page.locator(".topbar")).toHaveCSS("position", "static");
    await expect
      .poll(() =>
        page.evaluate(() =>
          document.documentElement.style.getPropertyValue(
            "--console-sticky-offset",
          ),
        ),
      )
      .toBe("16px");
    await expect(
      page.getByRole("button", { name: /Edit fleet scope: All VPS resources/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Clear fleet scope" }),
    ).toBeDisabled();
    await expect(
      page.locator(".topbarActions > .savedViewControls"),
    ).toBeHidden();
    const mobileSavedViews = page.locator(".mobileSavedViewMenu");
    await expect(mobileSavedViews).toBeVisible();
    const mobileSavedViewSelect = mobileSavedViews.getByRole("combobox", {
      name: "Saved fleet view",
      exact: true,
    });
    await expect(mobileSavedViewSelect).toBeHidden();
    await mobileSavedViews.getByLabel("Open saved fleet views menu").click();
    await expect(mobileSavedViewSelect).toBeVisible();
    const mobilePageMenu = page.locator(".mobilePageMenu");
    await expect(mobilePageMenu).toBeVisible();
    const mobilePageSelector = page.getByRole("combobox", {
      name: "Console page",
      exact: true,
    });
    await expect(mobilePageSelector).toBeVisible();
    await mobilePageSelector.selectOption("Config::sources");
    await expect(
      page.getByRole("heading", { name: "Config", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Configuration sources" }),
    ).toBeVisible();
  }
});

test("keeps control-plane metrics in System pages", async ({ page }) => {
  await page.goto("/");
  await waitForConsoleShell(page);

  const dashboard = page.locator(".dashboardWorkspace");
  await expect(
    page.getByRole("heading", { name: "Home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fleet command home", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Operational Health" }),
  ).toBeVisible();
  await expect(page.getByLabel("Home telemetry widgets")).toBeVisible();
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
  await expect(systemOverview.locator(".systemOverviewKpis")).toHaveCount(0);
  await expect(systemOverview.locator(".systemSubsystemGrid")).toHaveCount(0);
  await expect(systemOverview).not.toContainText("Capacity forecast");
  await expect(systemOverview).not.toContainText("Drilldown coverage");
  await expect(
    page.getByRole("heading", {
      name: "Selected chart - Dispatch queue",
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Selected chart - Dispatch queue thresholds"),
  ).toHaveCount(0);
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
  ).toContainText(/queue is growing/i);
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
  await expect(configSections).toContainText("Network");
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
  await configSections.getByRole("button", { name: /Network/ }).click();
  await expect(page.getByLabel("Tunnel IPv4 allocation pool")).toHaveValue("");
  await page.getByLabel("Tunnel IPv4 allocation pool").fill("10.250.0.0/16");
  await expect(
    page
      .locator(".systemConfigReview")
      .getByText("network.tunnel_ipv4_allocation_pool_cidr")
      .first(),
  ).toBeVisible();
  await expect(
    page.locator(".systemConfigReview").getByText("api.alert_cpu_load_warning"),
  ).toHaveCount(0);
  await expect(
    page
      .locator(".systemConfigReview")
      .getByText("api.alert_cpu_load_critical"),
  ).toHaveCount(0);
  await page.getByLabel("Tunnel IPv6 allocation pool").fill("fd42:250::/64");
  await expect(
    page
      .locator(".systemConfigReview")
      .getByText("network.tunnel_ipv6_allocation_pool_cidr")
      .first(),
  ).toBeVisible();
  await page.getByLabel("Tunnel IPv4 allocation pool").fill("");
  await page.getByLabel("Tunnel IPv6 allocation pool").fill("");
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
    suiteConfigReview.getByText("Unlock privilege").first(),
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
    .getByRole("button", { name: "Unlock privilege" })
    .first()
    .click();
  const privilegeDialog = page.getByRole("dialog", {
    name: "Unlock privilege",
  });
  await expect(privilegeDialog).toBeVisible();
  await privilegeDialog
    .getByLabel(/super password/i)
    .fill("local-super-password");
  await privilegeDialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    privilegeDialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: "Unlock", exact: true }),
  );
  await expect(privilegeDialog).toBeHidden();
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await expect(page.getByLabel("API DB pool")).toHaveValue("40");
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

test("keeps mobile Suite config actions in flow and sections scrollable", async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes("mobile"),
    "mobile Suite config containment is specific to the narrow layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "System", "Suite config");

  const layout = await page.evaluate(() => {
    const actionBar = document.querySelector<HTMLElement>(
      ".systemConfigStickyBar",
    );
    const body = document.querySelector<HTMLElement>(".systemConfigBody");
    const sections = document.querySelector<HTMLElement>(
      ".systemConfigSideNav",
    );
    const buttons = Array.from(
      sections?.querySelectorAll<HTMLButtonElement>("button") ?? [],
    );
    if (!actionBar || !body || !sections || buttons.length === 0) {
      throw new Error("Suite config mobile controls are missing");
    }
    const actionBounds = actionBar.getBoundingClientRect();
    const bodyBounds = body.getBoundingClientRect();
    const sectionStyle = getComputedStyle(sections);
    return {
      actionBarBottom: actionBounds.bottom,
      actionBarPosition: getComputedStyle(actionBar).position,
      bodyTop: bodyBounds.top,
      buttonWidths: buttons.map(
        (button) => button.getBoundingClientRect().width,
      ),
      sectionClientWidth: sections.clientWidth,
      sectionDisplay: sectionStyle.display,
      sectionOverflowX: sectionStyle.overflowX,
      sectionScrollWidth: sections.scrollWidth,
    };
  });

  expect(layout.actionBarPosition).toBe("static");
  expect(layout.actionBarBottom).toBeLessThanOrEqual(layout.bodyTop + 1);
  expect(layout.sectionDisplay).toBe("flex");
  expect(layout.sectionOverflowX).toBe("auto");
  expect(layout.sectionScrollWidth).toBeGreaterThan(layout.sectionClientWidth);
  expect(Math.min(...layout.buttonWidths)).toBeGreaterThanOrEqual(64);

  const sections = page.getByLabel("Suite config sections");
  await sections.evaluate((element) => {
    element.scrollLeft = element.scrollWidth;
  });
  await expect
    .poll(() => sections.evaluate((element) => element.scrollLeft))
    .toBeGreaterThan(0);
  await expect(sections.getByRole("button", { name: /Review/ })).toBeVisible();
});

test("surfaces operator users under Access and session evidence under Audit", async ({
  page,
}, testInfo) => {
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
  await expect(governance).toContainText("recommended rather than enforced");
  await expect(governance).toContainText("Refresh TTL policy");
  await expect(governance).toContainText("1 admin over target");
  await expect(governance).toContainText("Role model");
  await expect(governance).toContainText("3 standard roles");
  await expect(governance).toContainText("Viewer");
  await expect(governance).toContainText("Operator");
  await expect(governance).toContainText("Admin");
  await expect(governance).toContainText("Bearer sessions");
  await expect(governance).toContainText("0 active");
  await expect(governance).toContainText(
    "2 expired bearer sessions excluded from active counts",
  );
  await expect(governance).toContainText("Auth failures");
  await expect(governance).toContainText("2 failures");
  await expect(governance).toContainText(
    "Per-user counts below use the same auth history",
  );
  await expect(governance).toContainText("Policy evidence boundary");
  const operatorGrid = page.getByLabel("Operator accounts data grid");
  if (testInfo.project.name.includes("mobile")) {
    const adminCard = operatorGrid.getByLabel(
      "Operator accounts mobile card 99999999-aaaa-4bbb-8ccc-000000000001",
    );
    await expect(adminCard).toBeVisible();
    await expect(adminCard).toContainText("Last login");
    await expect(
      adminCard.getByRole("button", { name: "Edit", exact: true }),
    ).toHaveCount(0);
    await expect(
      adminCard.getByRole("button", {
        name: /^Open operator Operator accounts row /,
      }),
    ).toHaveCount(0);
    await expect(adminCard.locator(".gridMobileActions")).toHaveCount(0);
  } else {
    await expect(
      operatorGrid.getByRole("row", { name: /Last login/ }).first(),
    ).toBeVisible();
  }
  await selectGridRow(
    page,
    "Operator accounts",
    "99999999-aaaa-4bbb-8ccc-000000000001",
  );
  await operatorGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Edit", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", {
      name: "Revoke sessions",
      exact: true,
    }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await runGridAction(page, "Operator accounts", "Edit");
  const operatorEditor = page.locator(".operatorEditorPanel");
  await expect(page.getByLabel("Operator username")).toHaveValue(
    "console-admin",
  );
  const adminEvidence = page.getByLabel("Operator access evidence");
  await expect(adminEvidence).toContainText("Policy recommends MFA");
  await expect(adminEvidence).toContainText("365d - over admin target");
  await expect(adminEvidence).toContainText("unavailable for this operator");
  const revokeAdminSessions = adminEvidence.getByRole("button", {
    name: "Revoke sessions",
  });
  await expect(revokeAdminSessions).toBeDisabled();
  await expect(revokeAdminSessions).toHaveAttribute(
    "title",
    /No non-current active sessions/,
  );
  await activate(
    operatorEditor.getByRole("button", { name: "Disable", exact: true }),
  );
  await expect(page.getByText("Preparing review")).toBeVisible();
  await expect(page.getByLabel("Confirm admin user action")).toBeVisible();
  await expect(
    page.getByText(/targets or grants admin privileges/),
  ).toBeVisible();
  await page.getByLabel("Session refresh TTL days").fill("31");
  await expect(page.getByLabel("Confirm admin user action")).toBeHidden();
  await activate(
    operatorEditor.getByRole("button", { name: "Disable", exact: true }),
  );
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
  await runGridAction(page, "Operator accounts", "Edit");
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
    operatorEditor.getByRole("button", { name: "Save", exact: true }),
  ).toHaveAttribute("title", /never changes the password/);
  await page.getByLabel("Operator role").selectOption("admin");
  await expect(page.getByText(/Admin role grants require/)).toBeVisible();
  await activate(
    operatorEditor.getByRole("button", { name: "Save", exact: true }),
  );
  const adminGrantPrompt = page.getByLabel("Confirm admin user action");
  await expect(adminGrantPrompt).toBeVisible();
  await expect(adminGrantPrompt).toContainText(
    "targets or grants admin privileges",
  );
  await activate(adminGrantPrompt.getByRole("button", { name: "Cancel" }));
  await page.getByLabel("Operator role").selectOption("operator");
  await page.getByLabel("Operator password").fill("replacement-password-123");
  await activate(
    operatorEditor.getByRole("button", { name: "Save", exact: true }),
  );
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
  ).toContainText(/stale terminal states? hidden from open count/);
  await expect(
    auditSessions.getByLabel("Session evidence summary"),
  ).toContainText("expired bearer sessions");
  await expect(
    auditSessions.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Stale state");
  await expect(
    auditSessions.getByLabel("Terminal session evidence data grid"),
  ).toContainText("Replayable transcript");
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

test("marks saturated operator auth history without changing normal counts", async ({
  page,
}) => {
  await installConsoleApiMock(page, {
    operatorAuthEventsOverride: Array.from({ length: 200 }, (_, index) => ({
      created_at: new Date(Date.UTC(2026, 0, 1, 0, index)).toISOString(),
      id: `auth-failure-${String(index).padStart(3, "0")}`,
      operator_id: "99999999-aaaa-4bbb-8ccc-000000000001",
      reason: "invalid_credentials",
      remote_ip: "127.0.0.1",
      result: "failure",
      session_id: null,
      user_agent: "Playwright",
      username: "console-admin",
    })),
  });
  await page.goto("/");

  await unlockPrivilegeFor(page, "Access", "Operators");
  const governance = page.getByLabel("Operator governance overview");
  await expect(governance).toContainText("Auth failures in loaded history");
  await expect(governance).toContainText("≥200 loaded failures");
  await expect(governance).toContainText(
    "Per-user counts below use the same loaded auth history",
  );
  await selectGridRow(
    page,
    "Operator accounts",
    "99999999-aaaa-4bbb-8ccc-000000000001",
  );
  await runGridAction(page, "Operator accounts", "Edit");
  const selectedOperatorEvidence = page.getByLabel("Operator access evidence");
  await expect(selectedOperatorEvidence).toContainText("Failed logins");
  await expect(selectedOperatorEvidence).toContainText("≥200 loaded");
});

test("revokes selected non-current bearer sessions from Audit", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const frozenNow = Date.parse("2026-01-03T00:00:00Z");
    Date.now = () => frozenNow;
  });
  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Audit", "Sessions");

  const sessionsGrid = page.getByLabel("Operator bearer sessions data grid");
  const currentSessionId = "88888888-aaaa-4bbb-8ccc-000000000001";
  const currentSessionRow = sessionsGrid
    .locator(".gridBody [role=row], .gridMobileCard", {
      hasText: currentSessionId.slice(0, 8),
    })
    .first();
  await currentSessionRow.click();
  await expect(sessionsGrid).toContainText("Current browser");
  await expect(sessionsGrid).toContainText("Yes");
  await selectGridRow(page, "Operator bearer sessions", currentSessionId);
  await sessionsGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(page.getByRole("menuitem", { name: "Revoke" })).toBeDisabled();
  await page.keyboard.press("Escape");
  await unselectGridRow(page, "Operator bearer sessions", currentSessionId);

  const sessionId = "88888888-aaaa-4bbb-8ccc-000000000002";
  await selectGridRow(page, "Operator bearer sessions", sessionId);
  await runGridAction(page, "Operator bearer sessions", "Revoke");
  const confirmation = page.getByLabel("Confirm admin session revoke");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("console-admin");
  await activate(confirmation.getByRole("button", { name: "Revoke session" }));

  await expect(
    page.getByLabel("Operator bearer sessions data grid"),
  ).toContainText("Revoked");
  const revokeRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { operatorActions: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.operatorActions.find(
      (request) => (request as { action?: string }).action === "session-revoke",
    );
  });
  expect(revokeRequest).toMatchObject({
    action: "session-revoke",
    session_id: sessionId,
  });
  expectPrivilegeAssertion((revokeRequest as { body?: unknown }).body);
});

test("keeps legacy invalid schedule cadences visible and blocks only automatic runs", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense schedule and backup policy controls are covered in the desktop console",
  );
  await installConsoleApiMock(page, {
    schedulesOverride: [
      {
        cadence_error: "schedule_cron_no_future_occurrence",
        catch_up_limit: 1,
        catch_up_policy: "skip_missed",
        command_type: "shell_argv",
        created_at: "2026-01-01T00:00:00Z",
        cron_expr: "0 0 31 2 *",
        deferred_until: null,
        deleted_at: null,
        enabled: true,
        failure_count: 0,
        id: "legacy-invalid-schedule",
        last_error: null,
        last_run_at: null,
        max_failures: 3,
        name: "legacy impossible cadence",
        next_run_at: "",
        next_runs: [],
        operation: { argv: ["uptime"], pty: false, type: "shell" },
        retry_delay_secs: 300,
        selector_expression: "id:agent-sfo-01",
        target_client_ids: ["agent-sfo-01"],
        timezone: "UTC",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ],
    backupPoliciesOverride: [
      {
        cadence_error: "schedule_cron_invalid",
        catch_up_limit: 4,
        catch_up_policy: "run_all_limited",
        created_at: "2026-01-01T00:00:00Z",
        cron_expr: "not a cron",
        enabled: true,
        failure_count: 0,
        follow_symlinks: false,
        include_config: true,
        keep_last: 11,
        last_error: null,
        last_run_at: null,
        max_failures: 9,
        missing_path_policy: "skip",
        name: "legacy invalid backup cadence",
        next_run_at: "",
        next_runs: [],
        paths: ["/etc", "/srv/data"],
        retention_days: 45,
        retry_delay_secs: 777,
        rotation_generation: "quarterly-2026",
        schedule_id: "legacy-invalid-backup-policy",
        selector_expression: "id:agent-sfo-01",
        target_client_ids: ["agent-sfo-01"],
        timezone: "UTC",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ],
  });

  await page.goto("/");
  await unlockPrivilegeFor(page, "Automation", "Schedules");
  const scheduleGrid = page.getByLabel("Schedule records data grid");
  await expect(scheduleGrid).toContainText("Invalid cadence");
  await expect(scheduleGrid).toContainText("Invalid cadence — edit required");
  await expect(scheduleGrid).toContainText(
    "Edit required; automatic runs blocked",
  );
  await selectGridRow(page, "Schedule records", "legacy-invalid-schedule");
  await scheduleGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review run now" }),
  ).toBeEnabled();
  await expect(page.getByRole("menuitem", { name: "Edit" })).toBeEnabled();
  await expect(
    page.getByRole("menuitem", { name: "Review disable" }),
  ).toBeEnabled();
  await expect(
    page.getByRole("menuitem", { name: "Review enable" }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");

  await openConsoleSubpage(page, "Backups", "Policies");
  await expect(page.getByLabel("Backup policy summary")).toContainText(
    "0 automatic · 0 paused · 1 invalid cadence",
  );
  const policyGrid = page.getByLabel("Backup policy records data grid");
  await expect(policyGrid).toContainText("Invalid cadence");
  await expect(policyGrid).toContainText("edit required");
  await expect(policyGrid).toContainText("automatic backups blocked");
  await selectGridRow(
    page,
    "Backup policy records",
    "legacy-invalid-backup-policy",
  );
  await runGridAction(page, "Backup policy records", "Edit policy");
  await expect(
    page.getByRole("heading", { name: "Edit backup policy" }),
  ).toBeVisible();
  await expect(page.getByLabel("Backup policy name")).toHaveValue(
    "legacy invalid backup cadence",
  );
  await expect(page.getByLabel("Backup policy selected paths")).toHaveValue(
    "/etc\n/srv/data",
  );
  await page.getByLabel("Backup policy UTC cron expression").fill("30 2 * * *");
  await activate(page.getByRole("button", { name: "Review policy update" }));
  const updatePrompt = page.locator(".confirmationPrompt").last();
  await expect(updatePrompt).toContainText("Confirm backup policy update");
  await expect(updatePrompt).toContainText("/etc, /srv/data");
  await expect(updatePrompt).toContainText(
    "45 days · keep last 11 · rotation quarterly-2026",
  );
  await activate(updatePrompt.getByRole("button", { name: "Save changes" }));
  const backupPolicyUpdate = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          backupPolicyUpdates: Array<{ body: unknown; schedule_id: string }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.backupPolicyUpdates.at(-1);
  });
  expect(backupPolicyUpdate).toMatchObject({
    body: {
      catch_up_limit: 4,
      catch_up_policy: "run_all_limited",
      confirmed: true,
      cron_expr: "30 2 * * *",
      enabled: true,
      follow_symlinks: false,
      include_config: true,
      keep_last: 11,
      max_failures: 9,
      missing_path_policy: "skip",
      name: "legacy invalid backup cadence",
      paths: ["/etc", "/srv/data"],
      privilege_assertion: expect.any(Object),
      retention_days: 45,
      retry_delay_secs: 777,
      rotation_generation: "quarterly-2026",
      selector_expression: "id:agent-sfo-01",
      target_client_ids: ["agent-sfo-01"],
      timezone: "UTC",
    },
    schedule_id: "legacy-invalid-backup-policy",
  });
  await expect(policyGrid).not.toContainText("Invalid cadence");
  await expect(policyGrid).toContainText("30 2 * * * · UTC");
  await expect(policyGrid).toContainText("Automatic");
});

test("shows every saved fixed target in expanded backup policy details", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared expanded-row content is covered in the desktop data grid",
  );
  const fixedTargets = [
    "agent-sfo-01",
    "agent-fra-02",
    "agent-nyc-03",
    "agent-lon-04",
    "agent-sin-05",
  ];
  await installConsoleApiMock(page, {
    backupPoliciesOverride: [
      {
        cadence_error: null,
        catch_up_limit: 1,
        catch_up_policy: "skip_missed",
        created_at: "2026-01-01T00:00:00Z",
        cron_expr: "0 2 * * *",
        enabled: true,
        failure_count: 0,
        follow_symlinks: false,
        include_config: true,
        keep_last: 7,
        last_error: null,
        last_run_at: null,
        max_failures: 3,
        missing_path_policy: "skip",
        name: "worldwide fixed targets",
        next_run_at: "2026-01-02T02:00:00Z",
        next_runs: ["2026-01-02T02:00:00Z"],
        paths: ["/etc"],
        retention_days: 30,
        retry_delay_secs: 300,
        rotation_generation: null,
        schedule_id: "worldwide-fixed-targets",
        selector_expression: "tag:backup",
        target_client_ids: fixedTargets,
        timezone: "UTC",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ],
  });

  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Policies");
  const grid = page.getByLabel("Backup policy records data grid");
  await grid
    .getByLabel("Expand Backup policy records row worldwide-fixed-targets")
    .click();

  const details = grid.locator(".gridExpandedRow");
  await expect(details).toContainText(
    "5 VPSs · agent-sfo-01, agent-fra-02, agent-nyc-03 +2 more",
  );
  await expect(details.locator(".monoValue")).toHaveText(
    `Fixed targets: ${fixedTargets.join(", ")}`,
  );
});

test("keeps malformed schedule operations visible with only repair and removal actions", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense schedule actions are covered in the desktop console",
  );
  await installConsoleApiMock(page, {
    schedulesOverride: [
      {
        cadence_error: null,
        catch_up_limit: 1,
        catch_up_policy: "skip_missed",
        command_type: "invalid_operation",
        created_at: "2026-01-01T00:00:00Z",
        cron_expr: "0 * * * *",
        deferred_until: null,
        deleted_at: null,
        enabled: true,
        failure_count: 0,
        id: "malformed-operation-schedule",
        last_error: "schedule_operation_invalid",
        last_run_at: null,
        max_failures: 3,
        name: "malformed operation",
        next_run_at: "2026-01-01T01:00:00Z",
        next_runs: [],
        operation: null,
        operation_error: "schedule_operation_invalid",
        operation_payload_hash:
          "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        retry_delay_secs: 300,
        selector_expression: "id:agent-sfo-01",
        target_client_ids: ["agent-sfo-01"],
        timezone: "UTC",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ],
  });

  await page.goto("/");
  await unlockPrivilegeFor(page, "Automation", "Schedules");
  const scheduleGrid = page.getByLabel("Schedule records data grid");
  await expect(scheduleGrid).toContainText("Invalid saved operation");
  await expect(scheduleGrid).toContainText(
    "Run, enable, defer, and retarget are blocked",
  );
  await selectGridRow(page, "Schedule records", "malformed-operation-schedule");
  await scheduleGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Review run now" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("menuitem", { name: "Review enable" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("menuitem", { name: "Update targets" }),
  ).toBeDisabled();
  await expect(page.getByRole("menuitem", { name: "Defer" })).toBeDisabled();
  await expect(
    page.getByRole("menuitem", { name: "Review disable" }),
  ).toBeEnabled();
  await expect(
    page.getByRole("menuitem", { name: "Review deletion" }),
  ).toBeEnabled();
  await expect(page.getByRole("menuitem", { name: "Edit" })).toBeEnabled();
  await page.keyboard.press("Escape");
  await scheduleGrid
    .getByRole("row")
    .filter({ hasText: "malformed operation" })
    .first()
    .click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Update targets" }),
  ).toBeDisabled();
  await expect(page.getByRole("menuitem", { name: "Defer" })).toBeDisabled();
  await expect(
    page.getByRole("menuitem", { name: "Review deletion" }),
  ).toBeEnabled();
});

test(
  "updates only a schedule's frozen targets through the table action",
  {
    tag: "@bulk-resolve-delay",
  },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "the shared mobile card action path is covered by the responsive audit",
    );

    await page.goto("/");
    await unlockPrivilegeFor(page, "Automation", "Schedules");
    const grid = page.getByLabel("Schedule records data grid");
    await expect(
      grid.getByRole("columnheader", { name: /^Actions?$/ }),
    ).toHaveCount(0);
    const pageSize = grid.getByLabel("Schedule records page size");
    await expect(pageSize.locator('option[value="1000"]')).toHaveCount(1);
    await pageSize.selectOption("1000");
    await expect(pageSize).toHaveValue("1000");
    const scheduleRow = grid
      .getByRole("row")
      .filter({ hasText: "edge-health-hourly" })
      .first();
    const expand = scheduleRow.getByRole("button", {
      name: /Expand Schedule records row/,
    });
    await expect(expand).toHaveAttribute("aria-expanded", "false");
    await scheduleRow.click();
    await expect(
      grid.getByLabel(
        "Collapse Schedule records row 51515151-6161-4717-8abc-defdefdefdef",
      ),
    ).toHaveAttribute("aria-expanded", "true");
    const expandedDetail = grid.locator(".gridExpandedRow");
    await expect(expandedDetail).toBeVisible();
    await expect(expandedDetail).toContainText("Run only one missed run");
    await expect(expandedDetail).toContainText("edge-sfo-01 (agent-sfo-01)");
    await expect(expandedDetail).toContainText("core-fra-02 (agent-fra-02)");
    await expect
      .poll(() =>
        expandedDetail.evaluate(
          (element) => window.getComputedStyle(element).animationName,
        ),
      )
      .toBe("grid-detail-reveal");
    await expect(
      page.getByRole("heading", { level: 1, name: "Schedules" }),
    ).toBeVisible();
    await selectGridRow(
      page,
      "Schedule records",
      "51515151-6161-4717-8abc-defdefdefdef",
    );
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    const updateTargets = page.getByRole("menuitem", {
      name: "Update targets",
      exact: true,
    });
    await expect(updateTargets).toBeEnabled();
    await expect(updateTargets).toHaveAttribute(
      "title",
      /only fixed target IDs change/i,
    );
    await updateTargets.click();
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await expect(page.getByRole("menuitem", { name: "Edit" })).toBeDisabled();
    await expect(
      page.getByRole("menuitem", { name: "Review deletion" }),
    ).toBeDisabled();
    await expect(
      page.getByRole("menuitem", { name: "Update targets", exact: true }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");

    const prompt = page.getByRole("region", {
      name: "Update schedule targets",
    });
    await expect(prompt).toContainText("2 VPSs");
    await expect(prompt).toContainText("1 VPS");
    await expect(prompt).toContainText("No other schedule setting changes");
    await activate(prompt.getByRole("button", { name: "Update targets" }));

    await expect
      .poll(() =>
        page.evaluate(() => {
          const actions = (
            window as unknown as {
              __vpsmanTestRequests: {
                scheduleActions: Array<{
                  body: Record<string, unknown>;
                  method: string;
                  path: string;
                }>;
              };
            }
          ).__vpsmanTestRequests.scheduleActions;
          return (
            actions.find((action) => action.path.endsWith("/targets")) ?? null
          );
        }),
      )
      .not.toBeNull();
    const targetUpdate = await page.evaluate(() => {
      const actions = (
        window as unknown as {
          __vpsmanTestRequests: {
            scheduleActions: Array<{
              body: Record<string, unknown>;
              method: string;
              path: string;
            }>;
          };
        }
      ).__vpsmanTestRequests.scheduleActions;
      return actions.find((action) => action.path.endsWith("/targets"));
    });
    expect(targetUpdate).toMatchObject({
      method: "POST",
      path: "/api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef/targets",
      body: {
        confirmed: true,
        privilege_assertion: expect.objectContaining({
          assertion_hex: expect.any(String),
        }),
      },
    });
    expect(targetUpdate?.body).not.toHaveProperty("selector_expression");
    expect(targetUpdate?.body).not.toHaveProperty("target_client_ids");
    expect(targetUpdate?.body).not.toHaveProperty("cron_expr");
    expect(targetUpdate?.body).not.toHaveProperty("name");
    expect(targetUpdate?.body).not.toHaveProperty("operation");

    await expect(grid).toContainText("1 fixed VPS");
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await expect(
      page.getByRole("menuitem", { name: "Update targets", exact: true }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");
    await grid
      .getByRole("row")
      .filter({ hasText: "edge-health-hourly" })
      .first()
      .click({ button: "right" });
    await expect(
      page.getByRole("menuitem", { name: "Update targets", exact: true }),
    ).toBeDisabled();
  },
);

test(
  "bulk-updates each selected schedule from its own saved selector",
  { tag: "@bulk-schedule-targets" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "bulk selection uses the shared table toolbar covered on desktop",
    );

    await page.goto("/");
    await unlockPrivilegeFor(page, "Automation", "Schedules");
    const grid = page.getByLabel("Schedule records data grid");
    for (const scheduleId of [
      "51515151-6161-4717-8abc-defdefdefdef",
      "52525252-6161-4717-8abc-defdefdefdef",
    ]) {
      await selectGridRow(page, "Schedule records", scheduleId);
    }
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    const updateTargets = page.getByRole("menuitem", {
      name: "Update targets",
      exact: true,
    });
    await expect(updateTargets).toBeEnabled();
    await expect(updateTargets).toHaveAttribute(
      "title",
      /Update 2 of 2 selected schedules/,
    );
    await updateTargets.click();

    const prompt = page.getByRole("region", {
      name: "Update schedule targets",
    });
    await expect(prompt).toContainText("Selected schedules");
    await expect(prompt).toContainText("Changed snapshots");
    await expect(prompt).toContainText("edge-health-hourly");
    await expect(prompt).toContainText("us-capacity-hourly");
    await expect(prompt).toContainText("Saved fixed target IDs");
    await expect(prompt).toContainText("Added:");
    await expect(prompt).toContainText("Removed:");
    await expect(prompt).toContainText("core-fra-02 (agent-fra-02)");
    await expect(prompt.locator(".configurationReviewList")).toHaveCSS(
      "overflow-y",
      "auto",
    );
    await activate(prompt.getByRole("button", { name: "Update targets" }));

    await expect
      .poll(() =>
        page.evaluate(() => {
          const actions = (
            window as unknown as {
              __vpsmanTestRequests: {
                scheduleActions: Array<{ path: string }>;
              };
            }
          ).__vpsmanTestRequests.scheduleActions;
          return actions.filter((action) => action.path.endsWith("/targets"))
            .length;
        }),
      )
      .toBe(2);
    const updates = await page.evaluate(() => {
      const actions = (
        window as unknown as {
          __vpsmanTestRequests: {
            scheduleActions: Array<{
              body: Record<string, unknown>;
              path: string;
            }>;
          };
        }
      ).__vpsmanTestRequests.scheduleActions;
      return actions.filter((action) => action.path.endsWith("/targets"));
    });
    expect(updates).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          path: "/api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef/targets",
          body: expect.objectContaining({
            confirmed: true,
            privilege_assertion: expect.objectContaining({
              assertion_hex: expect.any(String),
            }),
          }),
        }),
        expect.objectContaining({
          path: "/api/v1/schedules/52525252-6161-4717-8abc-defdefdefdef/targets",
          body: expect.objectContaining({
            confirmed: true,
            privilege_assertion: expect.objectContaining({
              assertion_hex: expect.any(String),
            }),
          }),
        }),
      ]),
    );
    for (const update of updates) {
      expect(update.body).not.toHaveProperty("cron_expr");
      expect(update.body).not.toHaveProperty("name");
      expect(update.body).not.toHaveProperty("operation");
      expect(update.body).not.toHaveProperty("selector_expression");
      expect(update.body).not.toHaveProperty("target_client_ids");
    }
  },
);

test(
  "edit and save re-resolves schedule targets and ignores a late older review",
  { tag: "@bulk-resolve-delay" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "the schedule composer logic is shared across responsive layouts",
    );

    await page.goto("/");
    await unlockPrivilegeFor(page, "Automation", "Schedules");
    await selectGridRow(
      page,
      "Schedule records",
      "51515151-6161-4717-8abc-defdefdefdef",
    );
    await runGridAction(page, "Schedule records", "Edit");

    const selector = page.getByRole("combobox", {
      name: "Schedule target expression",
    });
    const reviewUpdate = page.getByRole("button", {
      name: "Review update",
      exact: true,
    });
    await selector.fill("country:DE");
    await activate(reviewUpdate);
    await selector.fill("country:US");
    await page.waitForTimeout(500);
    await expect(
      page.getByRole("region", { name: "Confirm schedule update" }),
    ).toHaveCount(0);

    await activate(reviewUpdate);
    const prompt = page.getByRole("region", {
      name: "Confirm schedule update",
    });
    await expect(prompt).toContainText("country:US");
    await expect(prompt).toContainText("agent-sfo-01");
    await expect(prompt).toContainText("agent-nyc-03");
    await activate(prompt.getByRole("button", { name: "Update schedule" }));

    await expect
      .poll(() =>
        page.evaluate(() => {
          const actions = (
            window as unknown as {
              __vpsmanTestRequests: {
                scheduleActions: Array<{
                  body: Record<string, unknown>;
                  method: string;
                  path: string;
                }>;
              };
            }
          ).__vpsmanTestRequests.scheduleActions;
          return actions.find((action) => action.method === "PUT") ?? null;
        }),
      )
      .not.toBeNull();
    const saved = await page.evaluate(() => {
      const actions = (
        window as unknown as {
          __vpsmanTestRequests: {
            scheduleActions: Array<{
              body: Record<string, unknown>;
              method: string;
            }>;
          };
        }
      ).__vpsmanTestRequests.scheduleActions;
      return actions.find((action) => action.method === "PUT");
    });
    expect(saved?.body).toMatchObject({
      selector_expression: "country:US",
      target_client_ids: expect.arrayContaining([
        "agent-sfo-01",
        "agent-nyc-03",
      ]),
    });
    expect(saved?.body.target_client_ids).toHaveLength(2);

    const grid = page.getByLabel("Schedule records data grid");
    await expect(grid).toContainText("2 fixed VPSs");
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await expect(
      page.getByRole("menuitem", { name: "Update targets", exact: true }),
    ).toBeDisabled();
  },
);

test(
  "ignores a late failed schedule review after the draft changes",
  { tag: ["@bulk-resolve-delay", "@bulk-resolve-failure"] },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "the schedule composer logic is shared across responsive layouts",
    );

    await page.goto("/");
    await unlockPrivilegeFor(page, "Automation", "Schedules");
    await selectGridRow(
      page,
      "Schedule records",
      "51515151-6161-4717-8abc-defdefdefdef",
    );
    await runGridAction(page, "Schedule records", "Edit");

    const selector = page.getByRole("combobox", {
      name: "Schedule target expression",
    });
    const reviewUpdate = page.getByRole("button", {
      name: "Review update",
      exact: true,
    });
    await selector.fill("country:DE");
    await activate(reviewUpdate);
    await selector.fill("country:US");
    await page.waitForTimeout(500);

    await expect(
      page.getByRole("region", { name: "Confirm schedule update" }),
    ).toHaveCount(0);
    await expect(
      page.getByText("Target inventory could not be read", { exact: true }),
    ).toHaveCount(0);
    await expect(reviewUpdate).toBeEnabled();
  },
);

test("accepts server-valid leap-day, named, and extended cadences without treating the short preview as validation", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense schedule composition is covered in the desktop console layout",
  );
  await page.clock.setFixedTime(new Date("2026-07-27T00:00:00Z"));
  await page.goto("/");
  await unlockPrivilegeFor(page, "Automation", "Schedules");
  await activate(page.getByRole("button", { name: "Expand Create schedule" }));
  await expect(page.getByLabel("Catch-up limit help")).toHaveAttribute(
    "title",
    /Maximum missed runs dispatched/,
  );
  await expect(page.getByLabel("Schedule catch-up limit")).toBeDisabled();
  await page
    .getByLabel("Schedule catch-up policy")
    .selectOption("run_all_limited");
  await expect(page.getByLabel("Schedule catch-up limit")).toBeEnabled();
  await page
    .getByLabel("Schedule job template")
    .selectOption("46464646-5656-4789-8abc-defdefdefdef");
  await page.getByLabel("Schedule target expression").fill("country:US");

  const cronInput = page.getByLabel("Schedule cron expression");
  const reviewSave = page.getByRole("button", {
    name: "Review save",
    exact: true,
  });
  await cronInput.fill("0 9 * * MON-FRI");
  await expect(reviewSave).toBeEnabled();
  await cronInput.fill("0 9 L * *");
  await expect(reviewSave).toBeEnabled();
  await cronInput.fill("0 0 29 2 *");
  await expect(
    page.getByText(
      "No run appears in the short local preview; the server validates this cadence when saved.",
    ),
  ).toBeVisible();
  await expect(reviewSave).toBeEnabled();
  await activate(reviewSave);
  await expect(page.getByText("Confirm schedule")).toBeVisible();
  await expect(page.locator(".confirmationPrompt")).toContainText(
    "Server calculates after save",
  );
});

test("discloses the exact schedule fetch cap", async ({ page }) => {
  await installConsoleApiMock(page, {
    schedulesOverride: Array.from({ length: 1000 }, (_, index) => ({
      cadence_error: null,
      catch_up_limit: 1,
      catch_up_policy: "skip_missed",
      command_type: "shell_argv",
      created_at: "2026-01-01T00:00:00Z",
      cron_expr: "0 * * * *",
      deferred_until: null,
      deleted_at: null,
      enabled: false,
      failure_count: 0,
      id: `schedule-cap-${String(index).padStart(4, "0")}`,
      last_error: null,
      last_run_at: null,
      max_failures: 3,
      name: `bounded schedule ${index}`,
      next_run_at: "",
      next_runs: [],
      operation: { argv: ["uptime"], pty: false, type: "shell" },
      retry_delay_secs: 300,
      selector_expression: "",
      target_client_ids: [],
      timezone: "UTC",
      updated_at: "2026-01-01T00:00:00Z",
    })),
  });

  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Schedules");
  await expect(
    page.getByRole("heading", { level: 2, name: "Schedules" }).locator(".."),
  ).toContainText("≥1000 loaded schedules");
  await expect(page.getByLabel("Schedule records data grid")).toContainText(
    "1000 loaded; more may exist",
  );
});

test("discloses the exact audit fetch cap", async ({ page }) => {
  await installConsoleApiMock(page, {
    auditLogsOverride: Array.from({ length: 1000 }, (_, index) => ({
      action: "fixture.cap",
      actor_id: null,
      command_hash: null,
      created_at: new Date(Date.UTC(2026, 0, 1, 0, 0, index)).toISOString(),
      id: `audit-cap-${String(index).padStart(4, "0")}`,
      metadata: {
        component: "fixture-generator",
        origin_kind: "control_plane",
        result: "generated",
      },
      target: `fixture:${index}`,
    })),
  });
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Events");
  await expect(page.getByLabel("Audit event summary")).toContainText("≥1000");
  await expect(page.getByLabel("Audit event summary")).toContainText(
    "All loaded events; more may exist",
  );
  await expect(page.getByLabel("Audit records data grid")).toContainText(
    "1000 loaded; more may exist",
  );
});

test("packs dense metric rows by label length", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop metric-row packing is the production density target",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "System", "Overview");
  const metricRows = page
    .locator(".dashboardTopClients.systemMetricTable")
    .first();
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

test("reviews one authoritative configuration target union and applies its frozen VPS list", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the dense configuration source registry is covered on desktop",
  );

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Sources");

  const panel = page.locator(".configurationSourcesPanel");
  await expect(
    panel.getByRole("heading", { name: "Configuration sources" }),
  ).toBeVisible();
  await expect(panel.locator(".sectionHeader").first()).toContainText(
    "3 VPSs · 1 setting needs attention · 1 setting unconfigured",
  );
  const effectiveGrid = panel.getByLabel("Effective configuration data grid");
  await expect(
    effectiveGrid.locator(".gridBody .gridRow").filter({
      hasText: "Edge vnStat",
    }),
  ).toContainText("Explicit override");
  await effectiveGrid
    .getByLabel("Effective configuration page size")
    .selectOption("25");
  await expect(effectiveGrid).toContainText("Not Observed");

  await activate(panel.getByRole("button", { name: "Change configuration" }));
  const drawer = page.getByRole("complementary", {
    name: "Change effective configuration",
  });
  await drawer
    .getByLabel("Configuration behavior")
    .selectOption("tunnel_traffic");
  await drawer
    .getByLabel("Configuration preset")
    .selectOption("11111111-1111-4111-8111-111111111111");
  await chooseVpsBySearch(
    drawer,
    "Add configuration target VPS",
    "edge-sfo",
    /edge-sfo-01/,
  );
  await expect(drawer.getByLabel("Configuration target preview")).toContainText(
    "edge-sfo-01",
  );
  await drawer.getByText("Add targets by selector", { exact: true }).click();
  await drawer
    .getByLabel("Configuration target selector")
    .fill("id:agent-sfo-01 || country:DE");
  await activate(drawer.getByRole("button", { name: "Review change" }));

  const prompt = page.locator(".confirmationPrompt").last();
  await expect(prompt).toContainText("Review effective configuration change");
  await expect(prompt).toContainText("edge-sfo-01");
  await expect(prompt).toContainText("agent-sfo-01");
  await expect(prompt).toContainText("core-fra-02");
  await expect(prompt).toContainText("agent-fra-02");
  await expect(prompt.locator(".configurationReviewList")).toHaveCount(3);
  await expect(prompt.locator(".configurationReviewList").first()).toHaveCSS(
    "white-space",
    "normal",
  );
  await expect(prompt).toContainText(
    "Interface traffic counters → Edge vnStat",
  );
  await expect(prompt).toContainText("Unchanged targets");
  await expect(prompt).toContainText(
    "Edge vnStat already selected; included for runtime resync",
  );
  await confirmVisiblePrompt(page, "Save selection");

  const requests = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          configurationSourceOverrides: Array<{
            body: Record<string, unknown>;
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.configurationSourceOverrides;
  });
  expect(requests[0]).toMatchObject({
    pathname: "/api/v1/configuration-source-overrides/preview",
    body: {
      action: "set",
      behavior: "tunnel_traffic",
      preset_id: "11111111-1111-4111-8111-111111111111",
      selector_expression: "id:agent-sfo-01 || country:DE",
      target_client_ids: ["agent-sfo-01"],
    },
  });
  expect(requests.at(-1)).toMatchObject({
    pathname: "/api/v1/configuration-source-overrides/apply",
    body: {
      preview_hash: "8".repeat(64),
      selector_expression: "id:agent-sfo-01 || country:DE",
      target_client_ids: ["agent-fra-02", "agent-sfo-01"],
    },
  });
  expectPrivilegeAssertion(requests.at(-1)?.body);
  await expect(panel).toContainText(
    "Selection saved and runtime sync queued for 2 VPSs",
  );
});

test("rejects a same-preset no-op without inventing a configuration change", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the dense configuration source registry is covered on desktop",
  );

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Sources");

  const panel = page.locator(".configurationSourcesPanel");
  await activate(panel.getByRole("button", { name: "Change configuration" }));
  const unchangedDrawer = page.getByRole("complementary", {
    name: "Change effective configuration",
  });
  await unchangedDrawer
    .getByLabel("Configuration behavior")
    .selectOption("tunnel_traffic");
  await unchangedDrawer
    .getByLabel("Configuration preset")
    .selectOption("11111111-1111-4111-8111-111111111111");
  await chooseVpsBySearch(
    unchangedDrawer,
    "Add configuration target VPS",
    "edge-sfo",
    /edge-sfo-01/,
  );
  await activate(
    unchangedDrawer.getByRole("button", { name: "Review change" }),
  );
  await expect(unchangedDrawer.getByRole("alert")).toContainText(
    "Every reviewed VPS already has this configuration selection; nothing would change.",
  );
  await expect(page.locator(".confirmationPrompt")).toHaveCount(0);
});

test("keeps the primary one-VPS preset and inheritance path direct and explicit", async ({
  page,
}) => {
  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Config", "Sources");

  const panel = page.locator(".configurationSourcesPanel");
  await activate(panel.getByRole("button", { name: "Change configuration" }));
  let drawer = page.getByRole("complementary", {
    name: "Change effective configuration",
  });
  await drawer
    .getByLabel("Configuration behavior")
    .selectOption("tunnel_traffic");
  await drawer
    .getByLabel("Configuration preset")
    .selectOption("11111111-1111-4111-8111-111111111111");

  const emptyReview = drawer.getByRole("button", { name: "Review change" });
  await expect(emptyReview).toBeDisabled();
  await expect(emptyReview).toHaveAttribute(
    "title",
    "Choose at least one VPS directly or with a matching selector.",
  );

  await chooseVpsBySearch(
    drawer,
    "Add configuration target VPS",
    "core-fra",
    /core-fra-02/,
  );
  await activate(
    drawer.getByRole("button", { name: "Inspect current effective config" }),
  );
  await expect(drawer.getByLabel("Effective agent config TOML")).toBeVisible();
  await expect(drawer).toContainText("Current server-rendered configuration");
  await expect(drawer).toContainText(
    /core-fra-02(?: \(ra02\))? · agent-fra-02/,
  );

  await activate(drawer.getByRole("button", { name: "Review change" }));
  let prompt = page.locator(".confirmationPrompt").last();
  await expect(prompt).toContainText("Direct VPS choices only");
  await expect(prompt).toContainText("core-fra-02");
  await expect(prompt).not.toContainText("edge-sfo-01");
  await confirmVisiblePrompt(page, "Save selection");

  await activate(panel.getByRole("button", { name: "Change configuration" }));
  drawer = page.getByRole("complementary", {
    name: "Change effective configuration",
  });
  await drawer
    .getByLabel("Configuration behavior")
    .selectOption("tunnel_traffic");
  await chooseVpsBySearch(
    drawer,
    "Add configuration target VPS",
    "core-fra",
    /core-fra-02/,
  );
  await activate(
    drawer.getByRole("button", {
      name: "Review reset to system default",
    }),
  );
  prompt = page.locator(".confirmationPrompt").last();
  await expect(prompt).toContainText(
    "Edge vnStat → Interface traffic counters",
  );
  await confirmVisiblePrompt(page, "Reset to system default");

  const requests = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          configurationSourceOverrides: Array<{
            body: Record<string, unknown>;
            pathname: string;
          }>;
        };
      }
    ).__vpsmanTestRequests.configurationSourceOverrides;
  });
  expect(requests.at(-1)).toMatchObject({
    pathname: "/api/v1/configuration-source-overrides/apply",
    body: {
      action: "reset",
      behavior: "tunnel_traffic",
      selector_expression: "",
      target_client_ids: ["agent-fra-02"],
    },
  });
});

test("preserves multiline preset drafts and rejects ambiguous environment rows", async ({
  page,
}) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Sources");

  const panel = page.locator(".configurationSourcesPanel");
  await activate(panel.getByRole("tab", { name: "Configuration presets" }));
  await activate(panel.getByRole("button", { name: "New preset" }));
  const drawer = page.getByRole("complementary", {
    name: "New configuration preset",
  });
  await drawer.getByLabel("Preset behavior").selectOption("command_execution");
  await drawer.getByLabel("Preset name").fill("Strict command execution");

  const argv = drawer.getByLabel("Shell command arguments");
  await argv.focus();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("Enter");
  await page.keyboard.type("--noprofile");
  await expect(argv).toHaveValue("/bin/sh\n-lc\n--noprofile");

  const keep = drawer.getByLabel("Environment names to keep");
  await keep.type("TERM");
  await page.keyboard.press("Enter");
  await page.keyboard.type("LANG");
  await expect(keep).toHaveValue("TERM\nLANG");

  const values = drawer.getByLabel("Command environment values");
  await values.type("FOO=one");
  await page.keyboard.press("Enter");
  await page.keyboard.type("FOO=two");
  await expect(values).toHaveValue("FOO=one\nFOO=two");
  await activate(drawer.getByRole("button", { name: "Create preset" }));
  await expect(drawer.locator(".actionFeedbackDanger")).toContainText(
    "Environment name FOO is repeated on line 2",
  );

  await values.fill("FOO=one\nBROKEN");
  await activate(drawer.getByRole("button", { name: "Create preset" }));
  await expect(drawer.locator(".actionFeedbackDanger")).toContainText(
    "Environment values line 2 must use KEY=value",
  );

  await values.fill("FOO=one\nBAR=two");
  await activate(drawer.getByRole("button", { name: "Create preset" }));
  await expect(drawer).toBeHidden();

  const mutation = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          configurationPresetMutations: Array<{
            action: string;
            body: Record<string, unknown>;
          }>;
        };
      }
    ).__vpsmanTestRequests.configurationPresetMutations.at(-1);
  });
  expect(mutation).toMatchObject({
    action: "create",
    body: {
      definition: {
        environment_keep: ["TERM", "LANG"],
        environment_set: { BAR: "two", FOO: "one" },
        shell_script_argv: ["/bin/sh", "-lc", "--noprofile"],
      },
    },
  });
});

test("authors OSPF updater presets only with paired bounded commands", async ({
  page,
}) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Config", "Sources");

  const panel = page.locator(".configurationSourcesPanel");
  await activate(panel.getByRole("tab", { name: "Configuration presets" }));
  await activate(panel.getByRole("button", { name: "New preset" }));
  const drawer = page.getByRole("complementary", {
    name: "New configuration preset",
  });
  await drawer
    .getByLabel("Preset behavior")
    .selectOption("ospf_update_command");
  await drawer.getByLabel("Preset name").fill("FRR edge updater");
  await drawer
    .getByLabel("Read current OSPF cost arguments")
    .fill("/usr/bin/vtysh\n-c\nshow ip ospf interface");

  await activate(drawer.getByRole("button", { name: "Create preset" }));
  await expect(drawer.locator(".actionFeedbackDanger")).toContainText(
    "Update OSPF cost arguments require an executable",
  );

  await drawer
    .getByLabel("Update OSPF cost arguments")
    .fill("/usr/bin/vtysh\n-c\nconfigure terminal");
  await activate(drawer.getByRole("button", { name: "Create preset" }));
  await expect(drawer).toBeHidden();

  const mutation = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          configurationPresetMutations: Array<{
            action: string;
            body: Record<string, unknown>;
          }>;
        };
      }
    ).__vpsmanTestRequests.configurationPresetMutations.at(-1);
  });
  expect(mutation).toMatchObject({
    action: "create",
    body: {
      behavior: "ospf_update_command",
      name: "FRR edge updater",
      definition: {
        contract_version: 1,
        status_command: {
          argv: ["/usr/bin/vtysh", "-c", "show ip ospf interface"],
          max_output_bytes: 16384,
          max_timeout_secs: 5,
        },
        update_command: {
          argv: ["/usr/bin/vtysh", "-c", "configure terminal"],
          max_output_bytes: 16384,
          max_timeout_secs: 5,
        },
      },
    },
  });
});

test(
  "keeps a failed configuration apply diagnosis inside its confirmation",
  { tag: "@configuration-source-apply-failure" },
  async ({ page }) => {
    await page.goto("/");
    await unlockPrivilegeFromTop(page);
    await openConsoleSubpage(page, "Config", "Sources");

    const panel = page.locator(".configurationSourcesPanel");
    await activate(panel.getByRole("button", { name: "Change configuration" }));
    const drawer = page.getByRole("complementary", {
      name: "Change effective configuration",
    });
    await drawer
      .getByLabel("Configuration behavior")
      .selectOption("tunnel_traffic");
    await drawer
      .getByLabel("Configuration preset")
      .selectOption("11111111-1111-4111-8111-111111111111");
    await chooseVpsBySearch(
      drawer,
      "Add configuration target VPS",
      "core-fra",
      /core-fra-02/,
    );
    await activate(drawer.getByRole("button", { name: "Review change" }));
    await confirmVisiblePrompt(page, "Save selection");

    const prompt = page.locator(".confirmationPrompt").last();
    await expect(prompt).toBeVisible();
    await expect(prompt.locator(".confirmationPromptError")).toContainText(
      "Review the current action snapshot",
    );
  },
);

test(
  "uses warning feedback when a saved preset selection does not fully queue",
  { tag: "@configuration-source-sync-failure" },
  async ({ page }) => {
    await page.goto("/");
    await unlockPrivilegeFromTop(page);
    await openConsoleSubpage(page, "Config", "Sources");

    const panel = page.locator(".configurationSourcesPanel");
    await activate(panel.getByRole("button", { name: "Change configuration" }));
    const drawer = page.getByRole("complementary", {
      name: "Change effective configuration",
    });
    await drawer
      .getByLabel("Configuration behavior")
      .selectOption("tunnel_traffic");
    await drawer
      .getByLabel("Configuration preset")
      .selectOption("11111111-1111-4111-8111-111111111111");
    await chooseVpsBySearch(
      drawer,
      "Add configuration target VPS",
      "core-fra",
      /core-fra-02/,
    );
    await activate(drawer.getByRole("button", { name: "Review change" }));
    await confirmVisiblePrompt(page, "Save selection");

    await expect(panel.locator(".actionFeedbackWarning")).toContainText(
      "Selection saved for 1 VPS; runtime sync needs attention on 1",
    );
    await expect(
      panel.locator(".actionFeedbackSuccess", {
        hasText: "runtime sync needs attention",
      }),
    ).toHaveCount(0);
  },
);

test("authors adapter definitions with the exact alternative lifecycle contract", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the dense tunnel-plan adapter registry is covered on desktop",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");

  const registry = page.getByLabel("Network adapter definitions");
  await expect(
    registry.getByRole("heading", { name: "Adapter definitions" }),
  ).toBeVisible();
  await expect(registry).toContainText("SFO routing cost");
  await activate(
    registry.getByRole("button", { name: "Tunnel runtime adapter" }),
  );

  const drawer = page.getByRole("complementary", {
    name: "New tunnel runtime adapter",
  });
  await expect(
    drawer.getByLabel("Start adapter command", { exact: true }),
  ).toHaveValue("");
  await expect(drawer.getByLabel("Status adapter command")).toHaveValue("");
  await drawer.getByLabel("Adapter definition name").fill("Custom lifecycle");
  await drawer
    .getByLabel("Status adapter command")
    .fill("/opt/operator/tunnel-adapter\nstatus");
  await drawer
    .getByLabel("Restart adapter command")
    .fill("/opt/operator/tunnel-adapter\nrestart");
  await drawer
    .getByLabel("Cleanup adapter command")
    .fill("/opt/operator/tunnel-adapter\ncleanup");
  await activate(
    drawer.getByRole("button", { name: "Create adapter definition" }),
  );

  const request = await page.evaluate(() => {
    return (
      window as unknown as {
        __vpsmanTestRequests: {
          networkAdapterMutations: Array<{
            action: string;
            body: Record<string, unknown>;
          }>;
        };
      }
    ).__vpsmanTestRequests.networkAdapterMutations.at(-1);
  });
  expect(request).toMatchObject({
    action: "create",
    body: {
      adapter_kind: "runtime_tunnel",
      name: "Custom lifecycle",
      definition: {
        manager: "external_managed_adapter",
        contract_version: 1,
        status_command: {
          argv: ["/opt/operator/tunnel-adapter", "status"],
        },
        restart_command: {
          argv: ["/opt/operator/tunnel-adapter", "restart"],
        },
        cleanup_command: {
          argv: ["/opt/operator/tunnel-adapter", "cleanup"],
        },
      },
    },
  });
  expect(
    (request?.body.definition as Record<string, unknown>).startup_command,
  ).toBeUndefined();
  expect(
    (request?.body.definition as Record<string, unknown>).stop_command,
  ).toBeUndefined();
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
  await activate(page.getByRole("button", { name: "Register release" }));
  await activate(page.getByRole("button", { name: "Review release" }));
  await expect(
    page.locator(".releaseActionFeedback.actionFeedbackDanger"),
  ).toContainText("Artifact URL must use https://");
  await expect(page.locator(".agentReleasesPanel .inlineError")).toHaveCount(0);
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
  const registry = page.getByLabel("Patch generator registry");
  await expect(
    registry.getByRole("button", { name: "Close patch generator registry" }),
  ).toBeVisible();
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
  const generatorValues = bulk.getByLabel("Patch generator values JSON");
  await expect(generatorValues).toHaveValue(
    /github\.com\/mnihyc\/vpsman\/releases\/latest\/download\/version\.json/,
  );
  const validGeneratorValues = await generatorValues.inputValue();
  await bulk
    .getByRole("combobox", { name: "Bulk patch target expression" })
    .fill("id:agent-sfo-01");
  await expect(
    page.getByRole("option", { name: /edge-sfo-01.*agent-sfo-01/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await generatorValues.fill("{");
  await activate(bulk.getByRole("button", { name: "Preview changes" }));
  await expect(
    page.locator(
      ".configWorkspace > .fleetPanel > .sectionHeader .actionFeedbackDanger",
    ),
  ).toHaveCount(0);
  await expect(
    page.locator(".configActionFeedback.actionFeedbackDanger"),
  ).toBeVisible();
  await generatorValues.fill(validGeneratorValues);
  await activate(bulk.getByRole("button", { name: "Preview changes" }));
  await expect(
    bulk.getByLabel("Rendered bulk runtime config patch TOML"),
  ).toHaveValue(
    /\[update\][\s\S]*unmanaged_enabled = false[\s\S]*version\.json/,
  );
  await expect(bulk.getByText("1 VPS verified")).toBeVisible();
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
  await expect(
    page.locator(".configReviewFeedback.actionFeedbackSuccess"),
  ).toContainText("Patch preview ready");
  await expect(
    page.locator(".singleConfigPatchPane .formHint", {
      hasText: "Patch preview ready",
    }),
  ).toHaveCount(0);
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
  await expect(scheduledRunsGrid).toContainText(/\d+(?:s|m|h|d|w|mo) ago/);
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
    "Enabled schedules with a valid cadence automatically dispatch future jobs",
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
  await expect(
    page.getByText("2 VPSs in local preview; server resolves before save"),
  ).toBeVisible();
  await expect(page.getByLabel("Schedule local VPS preview")).toContainText(
    "edge-sfo-01",
  );
  await expect(page.getByLabel("Schedule local VPS preview")).toContainText(
    "backup-nyc-03",
  );
  await expect(
    page.getByText(/Every 15 minutes\. Times shown in browser timezone\./),
  ).toBeVisible();
  await expect(page.getByText("Every 15 minutes")).toBeVisible();
  await expect(
    page.getByText(
      /2 matching VPSs in local preview; server resolves before save; edge-health-check/,
    ),
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
  const generatedPrivateKeyHex =
    "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
  const generatedPublicKeyHex =
    "fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0";

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
  const accessRevokedRow = identityGrid
    .locator(".gridBody [role=row]", { hasText: "backup-nyc-03" })
    .first();
  await expect(accessRevokedRow).toContainText("Access revoked");
  await selectGridRow(page, "VPS identities", "agent-nyc-03");
  await identityGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  const alreadyRevokedAction = page.getByRole("menuitem", {
    name: "Revoke",
    exact: true,
  });
  await expect(alreadyRevokedAction).toBeDisabled();
  await expect(alreadyRevokedAction).toHaveAttribute(
    "title",
    /already has Access revoked.*assign a new key/i,
  );
  await page.keyboard.press("Escape");
  await unselectGridRow(page, "VPS identities", "agent-nyc-03");
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
  const clientIdInput = inspector.getByLabel("Agent identity client ID");
  await clientIdInput.fill("agent tokyo");
  await expect(clientIdInput).toHaveAttribute("aria-invalid", "true");
  await expect(inspector).toContainText(
    "Client ID may use only letters, numbers, dot, underscore, colon, and hyphen.",
  );
  await expect(
    inspector.getByRole("button", { name: "Review registration" }),
  ).toBeDisabled();
  await clientIdInput.fill("agent-tokyo-04");
  await expect(clientIdInput).toHaveAttribute("aria-invalid", "false");
  await page.evaluate(
    ({ privateKeyHex, publicKeyHex }) => {
      function hexToBase64Url(hex: string): string {
        const bytes = new Uint8Array(hex.length / 2);
        for (let index = 0; index < bytes.length; index += 1) {
          bytes[index] = Number.parseInt(
            hex.slice(index * 2, index * 2 + 2),
            16,
          );
        }
        return btoa(String.fromCharCode(...bytes))
          .replace(/\+/g, "-")
          .replace(/\//g, "_")
          .replace(/=+$/g, "");
      }

      const subtle = window.crypto.subtle;
      const originalGenerateKey = subtle.generateKey.bind(subtle);
      const originalExportKey = subtle.exportKey.bind(subtle);
      const privateKey = {
        __vpsmanKeypairRole: "private",
      } as unknown as CryptoKey;
      const publicKey = {
        __vpsmanKeypairRole: "public",
      } as unknown as CryptoKey;
      Object.defineProperty(subtle, "generateKey", {
        configurable: true,
        value: (async (...args) => {
          const [algorithm] = args;
          const name =
            typeof algorithm === "string" ? algorithm : algorithm.name;
          if (name === "X25519") {
            return { privateKey, publicKey };
          }
          return originalGenerateKey(...args);
        }) as SubtleCrypto["generateKey"],
      });
      Object.defineProperty(subtle, "exportKey", {
        configurable: true,
        value: (async (...args) => {
          const [format, key] = args;
          const role = (key as unknown as { __vpsmanKeypairRole?: string })
            .__vpsmanKeypairRole;
          if (format === "jwk" && role === "private") {
            return {
              crv: "X25519",
              d: hexToBase64Url(privateKeyHex),
              ext: true,
              key_ops: ["deriveBits"],
              kty: "OKP",
              x: hexToBase64Url(publicKeyHex),
            };
          }
          if (format === "jwk" && role === "public") {
            return {
              crv: "X25519",
              ext: true,
              key_ops: [],
              kty: "OKP",
              x: hexToBase64Url(publicKeyHex),
            };
          }
          return originalExportKey(...args);
        }) as SubtleCrypto["exportKey"],
      });
    },
    {
      privateKeyHex: generatedPrivateKeyHex,
      publicKeyHex: generatedPublicKeyHex,
    },
  );
  await activate(inspector.getByRole("button", { name: "Generate keypair" }));
  await expect(
    inspector.getByLabel("Agent identity public key hex"),
  ).toHaveValue(/^[0-9a-f]{64}$/);
  await expect(inspector.getByLabel("Agent identity private key")).toHaveValue(
    /^[0-9a-f]{64}$/,
  );
  await inspector
    .getByLabel("Agent identity display name")
    .fill("edge-tokyo-04");
  await inspector
    .getByLabel("Agent identity tags")
    .fill("country:JP, role:edge");
  await activate(
    inspector.getByRole("button", { name: "Review registration" }),
  );
  const identityConfirmation = page.getByLabel(
    "Confirm VPS identity registration",
  );
  await expect(identityConfirmation).toBeVisible();
  await expect(identityConfirmation).toContainText("edge-tokyo-04");
  await expect(identityConfirmation).toContainText("country:JP, role:edge");
  await activate(
    identityConfirmation.getByRole("button", { name: "Register VPS" }),
  );
  await expect(inspector).toContainText("VPS identity registered.");
  await expect(inspector.getByText("edge-tokyo-04")).toBeVisible();
  const registrationComplete = inspector.locator(
    ".identityRegistrationComplete",
  );
  await expect(registrationComplete).toHaveCSS("margin-top", "10px");
  await expect(registrationComplete.locator("span")).toHaveAttribute(
    "title",
    /^agent-tokyo-04 \/ [0-9a-f]{64}$/,
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
    client_id: "agent-tokyo-04",
    client_public_key_hex: generatedPublicKeyHex,
    confirmed: true,
    display_name: "edge-tokyo-04",
    replace_existing_key: false,
    tags: ["country:JP", "role:edge"],
  });
  expectPrivilegeAssertion(identityRequest);

  const installCommand = inspector.getByLabel("Agent install command");
  await expect(installCommand).toContainText(
    'agent_install_tmp="$(mktemp -d)"',
  );
  await expect(installCommand).toContainText(
    "https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh",
  );
  const installCommandText =
    (await installCommand.locator("pre code").textContent()) ?? "";
  await expect(installCommand).toContainText("VPSMAN_AGENT_RELEASE='latest'");
  await expect(installCommand).toContainText("VPSMAN_INSTALL_MODE='root'");
  await expect(installCommand).toContainText(
    "VPSMAN_AGENT_CLIENT_ID='agent-tokyo-04'",
  );
  await expect(installCommand).toContainText(
    `VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX='${generatedPrivateKeyHex}'`,
  );
  await expect(installCommand).toContainText(
    "VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX='1111111111111111111111111111111111111111111111111111111111111111'",
  );
  await expect(installCommand).toContainText(
    "VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10'",
  );
  const gatewayKey = installCommand.getByLabel("Gateway server public key hex");
  await expect(gatewayKey).toHaveAttribute("aria-invalid", "false");
  await gatewayKey.fill("not-a-key");
  await expect(gatewayKey).toHaveAttribute("aria-invalid", "true");
  const gatewayKeyErrorId = await gatewayKey.getAttribute("aria-errormessage");
  expect(gatewayKeyErrorId).toBeTruthy();
  await expect(page.locator(`#${gatewayKeyErrorId}`)).toContainText(
    "exactly 64 hex characters",
  );
  await expect(
    installCommand.getByRole("button", { name: "Copy command" }),
  ).toBeDisabled();
  await gatewayKey.fill("1".repeat(64));
  const gatewayEndpoints = installCommand.getByLabel("Gateway endpoints");
  await gatewayEndpoints.fill("");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill("primary=gw.example.com:not-a-port=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  const gatewayEndpointsErrorId =
    await gatewayEndpoints.getAttribute("aria-errormessage");
  expect(gatewayEndpointsErrorId).toBeTruthy();
  await expect(page.locator(`#${gatewayEndpointsErrorId}`)).toContainText(
    "numeric port from 1 to 65535",
  );
  await gatewayEndpoints.fill("ipv6=2001:db8::1:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill("scoped=[fe80::1%eth0]:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill("bad=bad_host.example:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill("bad=999.0.0.1:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill("bad=001.2.3.4:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill("bad=[::ffff:001.2.3.4]:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "true");
  await gatewayEndpoints.fill(
    "primary=gw.example.com:9443=10\nbackup=gw-backup.example.com:9443=20",
  );
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "false");
  await gatewayEndpoints.fill("ipv6=[2001:db8::1]:9443=10");
  await expect(gatewayEndpoints).toHaveAttribute("aria-invalid", "false");
  await expect(installCommand).toContainText(
    "VPSMAN_GATEWAY_ENDPOINTS='ipv6=[2001:db8::1]:9443=10'",
  );
  await gatewayEndpoints.fill("primary=gw.example.com:9443=20");
  await activate(installCommand.getByRole("button", { name: "Save defaults" }));
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: { operatorPreferences: unknown[] };
          }
        ).__vpsmanTestRequests;
        return requests.operatorPreferences.length;
      }),
    )
    .toBeGreaterThan(0);
  const savedInstallDefaults = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { operatorPreferences: unknown[] };
      }
    ).__vpsmanTestRequests;
    return requests.operatorPreferences.at(-1);
  });
  expect(savedInstallDefaults).toMatchObject({
    gateway_endpoints: "primary=gw.example.com:9443=20",
    gateway_server_public_key_hex:
      "1111111111111111111111111111111111111111111111111111111111111111",
  });
  await expect(installCommand).toContainText(
    "VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=20'",
  );
  await installCommand.getByLabel("Install mode").selectOption("staged");
  await expect(installCommand).toContainText(
    "VPSMAN_INSTALL_MODE='unprivileged'",
  );
  await expect(installCommand).toContainText("VPSMAN_AGENT_ENABLE_SERVICE='0'");
  await expect(
    inspector.getByRole("heading", { name: "VPS registered" }),
  ).toBeVisible();
  await expect(
    inspector.getByRole("button", { name: "Generate keypair" }),
  ).toHaveCount(0);
  await expect(installCommand).toContainText(
    `VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX='${generatedPrivateKeyHex}'`,
  );
  await activate(
    inspector.getByRole("button", { name: "Register another VPS" }),
  );
  await expect(installCommand).toHaveCount(0);
  await expect(
    inspector.getByRole("button", { name: "Generate keypair" }),
  ).toBeVisible();

  await inspector
    .getByLabel("Agent identity client ID")
    .fill("agent-imported-05");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("2".repeat(64));
  await inspector.getByLabel("Agent identity display name").fill("imported-05");
  await activate(
    inspector.getByRole("button", { name: "Review registration" }),
  );
  await activate(
    page
      .getByLabel("Confirm VPS identity registration")
      .getByRole("button", { name: "Register VPS" }),
  );
  await expect(inspector.getByLabel("Agent install command")).toHaveCount(0);
  await expect(inspector).toContainText(
    "Registration is complete; use the matching private key from your secure source",
  );

  await selectGridRow(page, "VPS identities", "agent-sfo-01");
  await runGridAction(page, "VPS identities", "Revoke");
  await expect(
    inspector.getByRole("heading", { name: "Revoke VPS key" }),
  ).toBeVisible();
  const revokeTarget = inspector.getByRole("combobox", {
    name: "VPS identity revoke VPS ID",
  });
  await revokeTarget.fill("backup-nyc-03");
  await expect(page.getByRole("option", { name: /backup-nyc-03/ })).toHaveCount(
    0,
  );
  await revokeTarget.press("Escape");
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
  await expect(inspector).toContainText("VPS key revoked.");
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
  await expect(actions).toContainText("Configure defaults");
  await expect(actions).not.toContainText("Privilege state");
  const initialSessionScopes = page.getByLabel("Access session scopes");
  await expect(initialSessionScopes).toContainText("Privilege unlock");
  await expect(initialSessionScopes).toContainText("No saved local vault");
  await activate(actions.getByRole("button", { name: "Configure defaults" }));
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Gateway sessions",
    }),
  ).toBeVisible();
  await expect(page.getByLabel("Gateway installer defaults")).toBeVisible();
  await openConsoleSubpage(page, "Access", "Overview");

  const responsibilities = page.getByLabel("Access overview responsibilities");
  await expect(responsibilities).toContainText("Operators");
  await expect(responsibilities).toContainText("2 operators");
  await expect(responsibilities).toContainText(
    "Bearer sessions are listed under Session scopes.",
  );
  await expect(responsibilities).toContainText("VPS identities");
  await expect(responsibilities).not.toContainText("current session Expired");

  const sessionScopes = page.getByLabel("Access session scopes");
  await expect(sessionScopes).toContainText("Console/browser session");
  await expect(sessionScopes).toContainText("Active as console-admin");
  await expect(sessionScopes).toContainText("console stream");
  await expect(sessionScopes).toContainText("API bearer sessions");
  await expect(sessionScopes).toContainText("0 active / 2 expired");
  await expect(sessionScopes).toContainText("current bearer record Expired");
  await expect(sessionScopes).toContainText("Privilege unlock");
  await expect(sessionScopes).toContainText("Terminal sessions");
  await expect(sessionScopes).toContainText("Gateway sessions");
  await expect(sessionScopes).not.toContainText("current session Expired");

  await activate(actions.getByRole("button", { name: "Set up MFA" }));
  await expect(
    page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  ).toBeVisible();
  const privilegePanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  });
  await expect(page.getByText("Admin MFA is off")).toBeVisible();
  await expect(page.getByLabel(/^super password$/i)).toHaveCount(0);
  await expect(page.getByLabel(/super salt/i)).toHaveCount(0);
  await expect(privilegePanel.getByLabel(/access super password/i)).toHaveCount(
    0,
  );
  await expect(privilegePanel.getByLabel(/access privilege salt/i)).toHaveCount(
    0,
  );
  await expect(
    privilegePanel.locator('input[placeholder*="salt" i]'),
  ).toHaveCount(0);
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "Locked",
  );
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "This browser after verification",
  );
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "Not active",
  );
  await expect(page.getByText("Persistent browser unlock")).toBeVisible();
  await expect(page.getByText("Keep encrypted in this browser")).toHaveCount(0);
  await expect(page.getByText("Save encrypted vault")).toHaveCount(0);
  await expect(page.getByText("Deny by default")).toHaveCount(0);
  await expect(page.getByLabel("TOTP enrollment sequence")).toContainText(
    "Password",
  );
  await expect(page.getByLabel("TOTP enrollment sequence")).toContainText(
    "QR / key",
  );
  await expect(
    page.getByTitle("Scan the QR code or enter the setup key"),
  ).toBeVisible();
  await expect(page.getByLabel("TOTP enrollment sequence")).toHaveJSProperty(
    "tagName",
    "FORM",
  );
  await expect(
    page
      .getByLabel("TOTP enrollment sequence")
      .locator('input[autocomplete="username"]'),
  ).toHaveValue("console-admin");
  await expect(
    page.getByRole("button", { name: "Set up TOTP" }),
  ).toBeDisabled();
  await activate(
    privilegePanel.getByRole("button", { name: "Unlock privilege" }),
  );
  const unlockDialog = page.getByRole("dialog", { name: "Unlock privilege" });
  await unlockDialog.getByLabel(/super password/i).fill("local-super-password");
  await unlockDialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    unlockDialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: "Unlock", exact: true }),
  );
  await expect(unlockDialog).toBeHidden();
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "Verified and unlocked",
  );
  await expect(page.getByLabel("Privilege vault state")).toContainText(
    "This browser, including restarts",
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
  await expect(inspector.getByLabel("Agent identity client ID")).toHaveValue(
    "v-1",
  );
  await expect(inspector).toContainText("next numbered VPS ID");
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
    "Installer defaults can be edited above",
  );
  const gatewayDefaults = page.getByLabel("Gateway installer defaults");
  await expect(gatewayDefaults).toBeVisible();
  await expect(
    gatewayDefaults.getByLabel("Gateway server public key hex"),
  ).toBeVisible();
  await expect(gatewayDefaults.getByLabel("Gateway endpoints")).toBeVisible();
  await gatewayDefaults
    .getByLabel("Gateway endpoints")
    .fill("primary=gw.example.com:9443=10,backup=[2001:db8::5]:9443=20");
  const saveDefaults = gatewayDefaults.getByRole("button", {
    name: "Save defaults",
  });
  await expect(saveDefaults).toHaveCSS("white-space", "nowrap");
  await activate(saveDefaults);
  await expect(gatewayDefaults).toContainText(
    "Gateway install defaults saved for this operator.",
  );
  const savedGatewayDefaults = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          operatorPreferences: Array<Record<string, unknown>>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.operatorPreferences.at(-1);
  });
  expect(savedGatewayDefaults).toMatchObject({
    gateway_endpoints:
      "primary=gw.example.com:9443=10\nbackup=[2001:db8::5]:9443=20",
    gateway_server_public_key_hex: "1".repeat(64),
  });
  await expect(
    emptyState.getByRole("button", { name: "Preferences", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText("No panel-side endpoint lookup")).toHaveCount(0);
  await expect(page.getByRole("columnheader", { name: "Gateway" })).toHaveCount(
    0,
  );
  await expect(
    emptyState.getByRole("button", { name: "Gateway settings" }),
  ).toHaveCount(0);
});

test("keeps clipboard failures visible beside the copied identity value", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "identity hash copy behavior is shared with the mobile registry",
  );
  await page.goto("/");
  await openConsoleSubpage(page, "Access", "VPS identities");
  await page.evaluate(() => {
    Object.defineProperty(navigator.clipboard, "writeText", {
      configurable: true,
      value: async () => {
        throw new Error("Clipboard permission denied");
      },
    });
  });

  const copy = page
    .getByRole("button", { name: /Copy current key fingerprint/ })
    .first();
  await copy.click();

  await expect(copy).toContainText("Copy failed");
  await expect(copy).toHaveAttribute("title", /Clipboard permission denied/);
  await expect(copy).toHaveAttribute("title", /Allow clipboard access/);
});

test("keeps access action feedback out of headings and durable labels", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "access feedback placement is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Access", "Privilege vault");

  const accessHeader = page.locator(".accessMain > .sectionHeader").first();
  await expect(accessHeader).toContainText(
    "Local unlock state, request-bound assertions, and vault controls",
  );

  const privilegePanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "Privilege vault" }),
  });
  await expect(
    privilegePanel.locator(".privilegeVaultNotice strong"),
  ).toHaveText("Persistent browser unlock");
  await expect(privilegePanel.locator("form")).toHaveCount(0);
  await expect(
    privilegePanel.locator('input[placeholder*="salt" i]'),
  ).toHaveCount(0);
  await activate(
    privilegePanel.getByRole("button", { name: "Unlock privilege" }),
  );
  const unlockDialog = page.getByRole("dialog", { name: "Unlock privilege" });
  await unlockDialog.getByLabel(/super password/i).fill("local-super-password");
  await unlockDialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("not-hex");
  await activate(
    unlockDialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: "Unlock", exact: true }),
  );
  await expect(unlockDialog.locator(".actionFeedbackDanger")).toContainText(
    "Invalid hex value",
  );
  await activate(
    unlockDialog.getByRole("button", { name: "Close privilege unlock" }),
  );
  await expect(
    privilegePanel.locator(".privilegeVaultNotice strong"),
  ).toHaveText("Persistent browser unlock");

  const totpPanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "TOTP" }),
  });
  await expect(totpPanel.locator(".sectionHeader")).toContainText(
    "admin MFA required",
  );
  await page.getByLabel("TOTP password").fill("short");
  await page.getByLabel("TOTP password").press("Enter");
  await expect(totpPanel.locator(".sectionHeader")).toContainText(
    "admin MFA required",
  );
  await expect(totpPanel.locator(".sectionHeader")).not.toContainText(
    "Password Too Short",
  );
  await expect(totpPanel.locator(".actionFeedbackDanger")).toContainText(
    "Password Too Short (400)",
  );
});

test("renders a local authenticator QR code with a manual fallback", async ({
  page,
}) => {
  await page.goto("/");
  await openConsoleSubpage(page, "Access", "Privilege vault");

  const totpPanel = page.locator(".controlPanel").filter({
    has: page.getByRole("heading", { level: 2, name: "TOTP" }),
  });
  await totpPanel.getByLabel("TOTP password").fill("valid-password-123");
  await activate(totpPanel.getByRole("button", { name: "Set up TOTP" }));

  const enrollment = totpPanel.getByLabel("Authenticator QR code");
  await expect(enrollment).toBeVisible();
  await expect(
    enrollment.getByRole("img", {
      name: "QR code for this vpsman authenticator account",
    }),
  ).toHaveAttribute("src", /^data:image\/svg\+xml,/);
  await expect(enrollment).toContainText("JBSWY3DPEHPK3PXP");
  await expect(enrollment).toContainText("SHA1 · 6 digits · 30-second period");
  await expect(totpPanel).not.toContainText("otpauth://");
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
  await runGridAction(page, "VPS identities", "Rotate");

  const inspector = page.locator(".accessInspector");
  await expect(
    inspector.getByRole("button", { name: "Review rotation" }),
  ).toBeVisible();

  const displayNameInput = inspector.getByLabel("Agent identity display name");
  const tagsInput = inspector.getByLabel("Agent identity tags");
  await expect(displayNameInput).toBeDisabled();
  await expect(tagsInput).toBeDisabled();

  await expect(
    inspector.getByLabel("Agent identity client ID"),
  ).toHaveAttribute("readonly");
  await expect(inspector.getByLabel("Agent identity client ID")).toHaveValue(
    "agent-sfo-01",
  );
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
    page.getByRole("heading", { name: "Topology graph", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("img", { name: "Topology graph" })).toBeVisible();
  const graphPanel = page.locator(".topologyGraphPanel");
  await expect(
    page.getByText("2 of 2 plan endpoints shown; 1 of 1 tunnel shown"),
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
  await expect(graphPanel.getByLabel("Topology graph legend")).toContainText(
    "0 healthy, 0 unknown, 1 attention",
  );
  await page.getByLabel("Filter topology graph").fill("fra");
  await expect(
    graphPanel.getByRole("button", { name: /Select core-fra-02/ }),
  ).toBeVisible();
  await page.getByLabel("Topology health filter").selectOption("attention");
  await expect(
    graphPanel.getByText("1 visible tunnel", { exact: true }),
  ).toBeVisible();
  await page.getByLabel("Topology health filter").selectOption("all");
  await page.getByLabel("Filter topology graph").fill("");
  await openConsoleSubpage(page, "Network", "Evidence");
  await expect(
    page.getByRole("heading", { level: 1, name: "Network evidence" }),
  ).toBeVisible();
  const evidence = page.locator(".topologyEvidence");
  await expect(evidence.getByLabel("Network evidence freshness")).toContainText(
    /Evidence set was observed .* ago\./,
  );
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
    await expect(evidence.getByLabel(label, { exact: true })).toBeVisible();
  }
  await expect(
    evidence.getByRole("button", { name: "Compare to previous" }),
  ).toBeVisible();
  await expect(evidence.getByLabel("Measurement evidence")).toContainText(
    "Stale sample · degraded throughput",
  );
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
  const speedObservationRow = observationTable
    .locator(".historyRow")
    .filter({ hasText: "Network speed test" });
  await expect(observationTable).toContainText(
    "Stale sample · degraded throughput",
  );
  await expect(
    speedObservationRow.getByText("Healthy", { exact: true }),
  ).toHaveCount(0);
  await expect(
    speedObservationRow.getByText("10.1 Mbps", { exact: true }),
  ).toBeVisible();
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
  await expect(evidence.getByLabel("Related command jobs")).toContainText(
    "Stale sample · degraded throughput",
  );
  const commandSignals = evidence
    .getByLabel("Related command jobs")
    .locator(".status");
  for (let index = 0; index < (await commandSignals.count()); index += 1) {
    const signal = commandSignals.nth(index);
    await expect(signal).toHaveAttribute("title", await signal.innerText());
  }
  await expect(evidence).toContainText(
    "Runtime status evidence is available in observations or retained command output.",
  );
  await activate(evidence.getByRole("button", { name: "Open OSPF" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Network OSPF" }),
  ).toBeVisible();
  const ospfTable = page.getByLabel("OSPF updater plans data grid");
  await expect(ospfTable).toContainText("14");
  const ospfPlanRow = ospfTable
    .getByRole("row")
    .filter({ hasText: "sfo-fra-gre" })
    .first();
  await ospfPlanRow.click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Apply cost", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const planGrid = page.getByLabel("Tunnel plans data grid");
  await expect(planGrid).toContainText("22 cost");
  await expect(planGrid).toContainText("Reviewed · Review required");
});

test(
  "authors explicit tunnel plans with endpoint-scoped adapters",
  {
    tag: "@tunnel-prefill",
  },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "dense tunnel authoring is covered in the desktop console layout",
    );

    await page.goto("/");
    await openConsoleSubpage(page, "Network", "Tunnel plans");
    const planGrid = page.getByLabel("Tunnel plans data grid");
    await expect(planGrid).toBeVisible();
    await expect(planGrid).toContainText("Agent iproute2");
    await expect(planGrid).toContainText("External observed");
    await expect(planGrid).toContainText("Tunnel only");

    await planGrid
      .locator(".gridBody [role=row]", { hasText: "sfo-fra-gre" })
      .first()
      .click({ button: "right" });
    await expect(
      page.getByRole("menuitem", { name: "Edit", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("menuitem", { name: "Disable", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");

    await selectGridRow(page, "Tunnel plans", tunnelPlans[0].id);
    await runGridAction(page, "Tunnel plans", "Disable");
    const lifecyclePrompt = page.locator(".confirmationPrompt", {
      hasText: "Confirm tunnel plan disable",
    });
    await expect(lifecyclePrompt).toContainText("2 endpoint configurations");
    await expect(lifecyclePrompt).toContainText(
      "OSPF control stops; existing external daemon costs are not reverted",
    );
    await confirmVisiblePrompt(page, "Disable plans");
    await selectGridRow(page, "Tunnel plans", tunnelPlans[0].id);
    await planGrid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await expect(
      page.getByRole("menuitem", { name: "Enable", exact: true }),
    ).toBeEnabled();
    await page.keyboard.press("Escape");

    await activate(page.getByRole("button", { name: "Create plan" }));
    const composer = page.locator(".tunnelPlanComposer");
    await expect(composer).toBeVisible();
    await activate(composer.getByRole("button", { name: "Review plan" }));
    await expect(composer.locator(".localActionFeedback")).toContainText(
      "Plan name is required",
    );
    const kind = composer.getByLabel("Tunnel kind");
    await expect(kind.locator('option[value="openvpn"]')).toHaveCount(0);
    await activate(composer.getByRole("button", { name: "External observed" }));
    await expect(kind.locator('option[value="openvpn"]')).toHaveCount(1);
    await expect(
      composer.getByText("Agent-managed routes and cleanup"),
    ).toHaveCount(0);

    await composer.getByLabel("Tunnel plan name").fill("external-openvpn-ospf");
    await composer
      .getByLabel("Tunnel interface", { exact: true })
      .fill("ovpn70");
    await kind.selectOption("openvpn");
    await activate(composer.getByRole("button", { name: "Agent iproute2" }));
    await expect(
      composer.getByText(/OpenVPN cannot be agent-managed/),
    ).toBeVisible();
    await expect(kind).toHaveValue("openvpn");
    await expect(
      composer.getByRole("button", { name: "External observed" }),
    ).toHaveAttribute("aria-pressed", "true");
    await chooseVpsBySearch(
      composer,
      "Left tunnel VPS",
      "sfo",
      /edge-sfo-01.*agent-sfo-01/,
    );
    await chooseVpsBySearch(
      composer,
      "Right tunnel VPS",
      "fra",
      /core-fra-02.*agent-fra-02/,
    );
    await expect(
      composer.getByLabel("Left remote underlay destination"),
    ).toHaveValue("203.0.113.20");
    await expect(
      composer.getByLabel("Right remote underlay destination"),
    ).toHaveValue("198.51.100.10");
    await expect(composer.getByLabel("Tunnel bandwidth")).toHaveValue("400");
    await expect(
      composer.getByText(/Planning bandwidth suggested from/),
    ).toContainText("lower value 400 Mbps");
    await composer.getByLabel("Left local underlay source").fill("10.0.0.10");
    await composer.getByLabel("Left tunnel IPv4").fill("10.255.70.0");
    await composer.getByLabel("Right tunnel IPv4").fill("10.255.70.1");
    await composer
      .getByLabel("Tunnel interface", { exact: true })
      .fill("tunab");
    await activate(composer.getByRole("button", { name: "Review plan" }));
    await expect(composer.locator(".localActionFeedback")).toContainText(
      "Another saved plan already uses this interface",
    );
    await composer
      .getByLabel("Tunnel interface", { exact: true })
      .fill("ovpn70");

    await composer.getByText("Enable OSPF cost control").click();
    const resolvedOspfCommands = composer.getByLabel(
      "Resolved endpoint OSPF commands",
    );
    await expect(resolvedOspfCommands).toContainText(
      "FRR OSPF updater · VPS override · Ready",
    );
    await expect(
      composer.getByRole("button", { name: "Manage VPS presets" }),
    ).toBeVisible();
    await composer
      .getByLabel("Left OSPF command override (optional)", { exact: true })
      .selectOption("44444444-4444-4444-8444-444444444444");
    await composer
      .getByLabel("Right OSPF command override (optional)", { exact: true })
      .selectOption("55555555-5555-4555-8555-555555555555");
    await expect(resolvedOspfCommands).toContainText(
      "Per-plan override · SFO routing cost",
    );
    await expect(resolvedOspfCommands).toContainText(
      "Per-plan override · FRA routing cost",
    );
    const bandwidth = composer.getByLabel("Tunnel bandwidth");
    const liveCost = composer
      .getByLabel("Live OSPF cost preview")
      .locator("strong");
    await bandwidth.fill("10.5");
    await activate(composer.getByRole("button", { name: "Review plan" }));
    await expect(composer.locator(".localActionFeedback")).toContainText(
      "Bandwidth must be a whole number",
    );
    await bandwidth.fill("10");
    const lowBandwidthCost = Number(await liveCost.textContent());
    await bandwidth.fill("10000");
    const highBandwidthCost = Number(await liveCost.textContent());
    expect(lowBandwidthCost).toBeGreaterThan(highBandwidthCost);

    await activate(composer.getByRole("button", { name: "Review plan" }));
    const confirmation = page.locator(".confirmationPrompt", {
      hasText: "Confirm tunnel plan creation",
    });
    await expect(confirmation).toContainText("External observed");
    await expect(confirmation).toContainText("Reviewed · planned cost");
    await expect(confirmation).toContainText("OSPF command overrides");
    await expect(confirmation).toContainText(
      "Per-plan override · SFO routing cost",
    );
    await expect(confirmation).toContainText(
      "Per-plan override · FRA routing cost",
    );
    await expect(confirmation).toContainText("OSPF gates");
    await expect(confirmation).toContainText("Save disabled");
    await confirmVisiblePrompt(page, "Save plan");

    const request = await page.evaluate(() => {
      const requests = (
        window as unknown as {
          __vpsmanTestRequests: { tunnelPlans: unknown[] };
        }
      ).__vpsmanTestRequests;
      return requests.tunnelPlans.at(-1);
    });
    expect(request).toMatchObject({
      bandwidth_mbps: 10000,
      confirmed: true,
      enabled: false,
      interface_name: "ovpn70",
      kind: "openvpn",
      left_client_id: "agent-sfo-01",
      left_local_underlay: "10.0.0.10",
      left_remote_underlay: "203.0.113.20",
      name: "external-openvpn-ospf",
      ospf: {
        left_adapter_template_id: "44444444-4444-4444-8444-444444444444",
        mode: "reviewed",
        right_adapter_template_id: "55555555-5555-4555-8555-555555555555",
      },
      right_client_id: "agent-fra-02",
      right_local_underlay: null,
      right_remote_underlay: "198.51.100.10",
      runtime_control: { manager: "external_observed" },
      runtime_topology: {},
    });
    expect(JSON.stringify(request)).not.toMatch(/bird|argv|\/usr\/local/i);
  },
);

test(
  "late tunnel suggestions never overwrite touched fields",
  {
    tag: "@tunnel-prefill-late",
  },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "the delayed authoring race is covered in the desktop form",
    );

    await page.goto("/");
    await openConsoleSubpage(page, "Network", "Tunnel plans");
    await activate(page.getByRole("button", { name: "Create plan" }));
    const composer = page.locator(".tunnelPlanComposer");
    await chooseVpsBySearch(
      composer,
      "Left tunnel VPS",
      "sfo",
      /edge-sfo-01.*agent-sfo-01/,
    );
    await chooseVpsBySearch(
      composer,
      "Right tunnel VPS",
      "fra",
      /core-fra-02.*agent-fra-02/,
    );

    const bandwidth = composer.getByLabel("Tunnel bandwidth");
    await bandwidth.fill("777");
    const leftRemote = composer.getByLabel("Left remote underlay destination");
    await expect(leftRemote).toHaveValue("203.0.113.20");
    await leftRemote.fill("");
    await activate(composer.getByRole("button", { name: "External observed" }));
    await page.waitForTimeout(600);

    await expect(bandwidth).toHaveValue("777");
    await expect(leftRemote).toHaveValue("");
  },
);

test("inspects disabled tunnel cleanup without exposing probe or speed mutations", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "disabled-plan diagnostic controls are covered in the desktop workflow",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await selectGridRow(page, "Tunnel plans", tunnelPlans[1].id);
  await runGridAction(page, "Tunnel plans", "Disable");
  await confirmVisiblePrompt(page, "Disable plans");

  await openConsoleSubpage(page, "Network", "Tests");
  await page.getByLabel("Network test plan").selectOption(tunnelPlans[1].id);
  const panel = page.locator(".fleetPanel", {
    has: page.getByRole("heading", { level: 2, name: "Network tests" }),
  });
  await expect(panel).toContainText("Plan disabled; inspect only");
  await expect(
    page.getByRole("button", { name: "Inspect status" }),
  ).toBeEnabled();
  await expect(page.getByRole("button", { name: "Run probe" })).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Review speed test" }),
  ).toBeDisabled();

  await activate(page.getByRole("button", { name: "Inspect status" }));
  await expect(page.getByLabel("Execution result")).toBeFocused();
  const statusRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(statusRequest).toMatchObject({
    command: "network_status",
    force_unprivileged: true,
    privileged: false,
    operation: {
      plan_id: tunnelPlans[1].id,
      type: "network_status",
    },
  });
});

test(
  "warns but permits reviewed OSPF planned-baseline apply",
  { tag: "@ospf-planned-baseline" },
  async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name.includes("mobile"),
      "reviewed OSPF warning confirmation is covered in the desktop workflow",
    );

    await page.goto("/");
    await openConsoleSubpage(page, "Network", "OSPF");
    const table = page.getByLabel("OSPF updater plans data grid");
    await expect(table).toContainText("Planned baseline");
    const planRow = table
      .getByRole("row")
      .filter({ hasText: "sfo-fra-gre" })
      .first();
    await planRow.click({ button: "right" });
    const applyAction = page.getByRole("menuitem", {
      name: "Apply cost",
      exact: true,
    });
    await expect(applyAction).toBeEnabled();
    await activate(applyAction);
    const prompt = page.locator(".confirmationPrompt.warning", {
      hasText: "Confirm OSPF cost update",
    });
    await expect(prompt).toBeVisible();
    await expect(prompt).toContainText(
      "No recent probe evidence is available, so this applies the operator-declared planned baseline",
    );
    await expect(prompt).toContainText("Review condition");
    await expect(prompt).toContainText("Planned baseline");
  },
);

test("separates runtime reconciliation, failed probes, and operator connectivity assessment", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense tunnel evidence and assessment controls are covered on desktop",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const planGrid = page.getByLabel("Tunnel plans data grid");
  const row = planGrid
    .locator(".gridBody [role=row]", { hasText: "sfo-fra-gre" })
    .first();
  await expect(row).toContainText("L Healthy");
  await expect(row).toContainText("R Healthy");
  await expect(row).toContainText("Partially verified");
  await expect(row).toContainText("Peer probe failed; not proof of disconnect");

  await activate(row);
  const detail = planGrid.locator(".gridExpandedRow");
  const assessment = detail.locator(".tunnelConnectionAssessment");
  await expect(assessment).toContainText(
    "Display-only annotation; runtime and automatic OSPF stay machine-derived",
  );
  await assessment
    .getByLabel("Connectivity assessment for sfo-fra-gre")
    .selectOption("connected");
  const save = assessment.getByRole("button", { name: "Save assessment" });
  await expect(save).toBeDisabled();
  await assessment
    .getByLabel("Connectivity assessment note for sfo-fra-gre")
    .fill("Application traffic verified; ICMP is blocked");
  await activate(save);

  await expect(row).toContainText("Connected");
  await expect(row).toContainText("Operator assessment");
  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: { tunnelPlanConnectionAssessments: unknown[] };
        }
      ).__vpsmanTestRequests.tunnelPlanConnectionAssessments,
  );
  expect(requests).toEqual([
    {
      body: {
        assessment: "connected",
        expected_revision: 3,
        note: "Application traffic verified; ICMP is blocked",
      },
      plan_id: "dddddddd-eeee-4fff-8000-111111111111",
    },
  ]);
});

test("retires an enabled tunnel plan immediately from a frozen revision", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "dense tunnel lifecycle actions are covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const planGrid = page.getByLabel("Tunnel plans data grid");
  await selectGridRow(page, "Tunnel plans", tunnelPlans[0].id);
  await planGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  const deleteButton = page.getByRole("menuitem", {
    name: "Delete",
    exact: true,
  });
  await expect(deleteButton).toBeEnabled();
  await expect(deleteButton).toHaveAttribute(
    "title",
    "Delete sfo-fra-gre and queue runtime removal on both endpoints.",
  );

  await activate(deleteButton);
  const confirmation = page.locator(".confirmationPrompt", {
    hasText: "Confirm tunnel plan deletion",
  });
  await expect(confirmation).toContainText("sfo-fra-gre (r3)");
  await expect(confirmation).toContainText("Queue removal on both endpoints");
  await expect(confirmation).toContainText("Current stateEnabled");
  await expect(confirmation).toContainText("Stop control; keep daemon cost");
  await confirmVisiblePrompt(page, "Delete plan");

  await expect(planGrid).not.toContainText("sfo-fra-gre");
  await expect(page.locator(".topologyPlanActionFeedback")).toContainText(
    "Deleted tunnel plan sfo-fra-gre. Runtime removal queued for 2 endpoints.",
  );
  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: { tunnelPlanDeletes: unknown[] };
        }
      ).__vpsmanTestRequests.tunnelPlanDeletes,
  );
  expect(requests).toEqual([
    {
      expected_revision: 3,
      plan_id: "dddddddd-eeee-4fff-8000-111111111111",
    },
  ]);
});

test("shows telemetry only for explicitly saved tunnel endpoints", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "declared tunnel telemetry is covered in the desktop console layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");
  const row = page
    .getByLabel("VPS instance records data grid")
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  await activate(row.getByLabel("Expand VPS instance records row"));
  const detail = page
    .getByLabel("VPS instance records data grid")
    .locator(".gridExpandedRow", { hasText: "edge-sfo-01" })
    .first();
  await activate(detail.getByRole("tab", { name: "Network" }));
  await expect(detail).toContainText("sfo-fra-gre");
  await expect(detail).toContainText("external-openvpn-observed");
  await expect(detail).not.toContainText("wg-import");

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const planGrid = page.getByLabel("Tunnel plans data grid");
  await expect(planGrid.locator(".gridBody [role=row]")).toHaveCount(2);
  await expect(page.getByText(/promotion workflow/i)).toHaveCount(0);
});

test("keeps each expanded VPS network evidence scoped to that VPS", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "desktop supports multiple simultaneous inline VPS details",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Fleet", "Instances");
  const grid = page.getByLabel("VPS instance records data grid");
  const edgeRow = grid
    .locator(".gridBody [role=row]", { hasText: "edge-sfo-01" })
    .first();
  const coreRow = grid
    .locator(".gridBody [role=row]", { hasText: "core-fra-02" })
    .first();

  await activate(edgeRow.getByLabel("Expand VPS instance records row"));
  await activate(coreRow.getByLabel("Expand VPS instance records row"));

  const edgeDetail = grid
    .locator(".gridExpandedRow", { hasText: "edge-sfo-01" })
    .first();
  const coreDetail = grid
    .locator(".gridExpandedRow", { hasText: "core-fra-02" })
    .first();
  await activate(edgeDetail.getByRole("tab", { name: "Telemetry" }));
  await activate(coreDetail.getByRole("tab", { name: "Telemetry" }));
  const edgeRate = edgeDetail
    .locator(".timeline")
    .filter({ hasText: /^Network rate/ });
  const coreRate = coreDetail
    .locator(".timeline")
    .filter({ hasText: /^Network rate/ });

  await expect(edgeRate).toContainText("RX 19 Mbps / TX 18 Mbps");
  await expect(coreRate).toContainText("RX 4.1 Mbps / TX 3.6 Mbps");
  await expect(edgeRate).not.toContainText("RX 4.1 Mbps");
  await expect(coreRate).not.toContainText("RX 19 Mbps");
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
  const jobGrid = page.getByLabel("Job records data grid");
  const scheduledJobRow = jobGrid
    .locator(".gridBody [role=row]", { hasText: "network speed test" })
    .first();
  await scheduledJobRow.getByRole("checkbox").check();
  await runGridAction(page, "Job records", "Open target detail");

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
  test.slow();
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
  await lockPrivilegeFromTop(page);
  await expect(
    page.locator(".commandComposer").getByLabel("Super password"),
  ).toHaveCount(0);
  await expect(
    page
      .locator(".commandComposer .privilegeStatus")
      .getByText("Locked", { exact: true }),
  ).toBeVisible();
  await expect(
    page
      .locator(".commandComposer")
      .getByRole("button", { name: "Unlock privilege" }),
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
  await expect(targetExpression).toHaveValue("name:edge-sfo-01");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("fo01");
  await expect(
    page.getByRole("option", { name: /edge-sfo-01.*ID.*agent-sfo-01/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toHaveValue("id:agent-sfo-01");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("status:on");
  await expect(
    page.getByRole("option", { name: /^status:online$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toHaveValue("status:online");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("vps.status:on");
  await expect(
    page.getByRole("option", { name: /^vps\.status:online$/ }),
  ).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toHaveValue("vps.status:online");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("role:e");
  await expect(page.getByRole("option", { name: /^role:edge$/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toHaveValue("role:edge");
  await targetExpression.fill("");
  await targetExpression.click();
  await page.keyboard.type("*");
  await expect(page.getByRole("option", { name: /^\*$/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(targetExpression).toHaveValue("*");
  await targetExpression.fill("");
  await page
    .getByLabel("Bulk target selector expression")
    .fill("id:agent-sfo-01");
  await expect(page.getByText("1/3 resolved from selector")).toBeVisible();
  await dispatchWithPrompt(page.locator(".commandComposer"));

  const resultPanel = page.getByLabel("Execution result");
  await expect(resultPanel).toBeVisible();
  await expect(resultPanel.getByText(/completed on 1 VPS/)).toBeVisible();
  const dispatchHistoryState = await page.evaluate(() =>
    JSON.stringify(window.history.state),
  );
  expect(dispatchHistoryState).toContain("__vpsman_history");
  expect(dispatchHistoryState).not.toContain("/usr/bin/uptime");
  expect(dispatchHistoryState).not.toContain("local-super-password");
  await activate(page.getByRole("button", { name: "Open job details" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Job history" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();
  await expect(page).toHaveURL(
    /#\/jobs\/history\/11111111-2222-4333-8444-555555555555$/,
  );
  const targetDetails = page.getByRole("region", {
    name: "Job target details",
  });
  await expect(targetDetails).toBeFocused();
  await expect
    .poll(async () => {
      const [detailBounds, topbarBounds] = await Promise.all([
        targetDetails.boundingBox(),
        topbar.boundingBox(),
      ]);
      if (!detailBounds || !topbarBounds) {
        return false;
      }
      return (
        detailBounds.y >= topbarBounds.y + topbarBounds.height &&
        detailBounds.y < testInfo.project.use.viewport!.height
      );
    })
    .toBe(true);
  const targetDetailHeader = targetDetails.locator(".targetDetailHeader");
  const targetDetailClose = targetDetails.getByRole("button", {
    name: "Close job target details",
  });
  await expect
    .poll(async () => {
      const [headerBounds, closeBounds] = await Promise.all([
        targetDetailHeader.boundingBox(),
        targetDetailClose.boundingBox(),
      ]);
      if (!headerBounds || !closeBounds) {
        return false;
      }
      return (
        closeBounds.x + closeBounds.width <=
          headerBounds.x + headerBounds.width + 1 &&
        closeBounds.x >= headerBounds.x + headerBounds.width / 2 &&
        closeBounds.y < headerBounds.y + headerBounds.height / 2
      );
    })
    .toBe(true);
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

  await page.goBack();
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
  await expect(page.getByLabel("Command argv")).toHaveValue("/usr/bin/uptime");
  await expect(page.getByLabel("Execution result")).toContainText(
    /completed on 1 VPS/,
  );
  await expect(page.locator(".confirmationPrompt")).toHaveCount(0);
  const serializedHistoryState = await page.evaluate(() =>
    JSON.stringify(window.history.state),
  );
  expect(serializedHistoryState).toContain("__vpsman_history");
  expect(serializedHistoryState).not.toContain("/usr/bin/uptime");
  expect(serializedHistoryState).not.toContain("local-super-password");

  await page.goForward();
  await expect(page).toHaveURL(
    /#\/jobs\/history\/11111111-2222-4333-8444-555555555555$/,
  );
  await expect(
    page.getByRole("heading", { name: "Target results" }),
  ).toBeVisible();

  await page.reload({ waitUntil: "domcontentloaded" });
  await waitForConsoleShell(page);
  await page.goBack();
  await expect(
    page.getByRole("heading", { level: 1, name: "Command dispatch" }),
  ).toBeVisible();
  await expect(page.getByLabel("Command argv")).toHaveValue("");
  await expect(page.getByLabel("Execution result")).toHaveCount(0);
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

  const expression = page.getByRole("combobox", {
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
  const expressionInput = page.locator(".searchExpressionInput", {
    has: expression,
  });
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
    expressionInput.locator(".searchExpressionChip").first(),
  ).toBeVisible();
  await expression.evaluate((element) => {
    element.scrollLeft = 0;
  });
  await expressionInput.evaluate((element) =>
    element.scrollIntoView({ block: "center", inline: "nearest" }),
  );
  await expressionInput.hover();
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
    "terminal launch dispatch is covered in the desktop remote operations layout",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Terminal");
  await unlockPrivilegeFor(page, "Remote Operations", "Terminal");

  const terminalComposer = page.getByLabel("New terminal composer");
  await chooseVpsBySearch(
    terminalComposer,
    "New terminal target",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await terminalComposer
    .getByLabel("New terminal working directory")
    .fill("/root");
  await terminalComposer
    .getByLabel("New terminal user policy")
    .selectOption("root");
  await terminalComposer.getByText("Advanced terminal options").click();
  await terminalComposer.getByLabel("New terminal columns").fill("100");
  await terminalComposer.getByLabel("New terminal rows").fill("30");
  await activate(
    terminalComposer.getByRole("button", { name: "Open terminal" }),
  );

  await expect(
    terminalComposer.getByText(/terminal open job submitted/),
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
    target_client_ids: ["agent-sfo-01"],
    command: "terminal_open",
    operation: {
      argv: ["/bin/sh", "-l"],
      cols: 100,
      cwd: "/root",
      rows: 30,
      type: "terminal_open",
      user: "root",
      user_policy: "fail",
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
  await expect(auditSummary).toContainText("Known actors");

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

  const deleteReviewedRows = page.getByRole("button", {
    name: "Delete reviewed rows",
  });
  await expect(deleteReviewedRows).toBeDisabled();
  await expect(deleteReviewedRows).toHaveAttribute(
    "title",
    "No reviewed rows match; deletion is not needed",
  );
  await expect(page.getByLabel("Confirm history prune")).toHaveCount(0);
});

test("dispatches executable restores with agent-local archive metadata only", async ({
  page,
}, testInfo) => {
  test.slow();
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
  await openConsoleSubpage(page, "Backups", "Overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "Backup overview" }),
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
  await expect(posture).toContainText("No retention");

  await openConsoleSubpage(page, "Backups", "Restore");
  await unlockPrivilegeFor(page, "Backups", "Restore");
  await expect(
    page.locator(".topbar").getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
  await activate(page.getByRole("button", { name: "Choose restore artifact" }));
  const restoreWorkflow = page.getByLabel("Choose restore artifact");

  await restoreWorkflow
    .getByLabel("Restore source backup request")
    .selectOption(backupId);
  await chooseVpsBySearch(
    restoreWorkflow,
    "Restore target client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  const stagedArchive = restoreWorkflow.getByLabel("Staged archive");
  await expect(stagedArchive).toHaveValue("");
  await expect(stagedArchive).toContainText("No matching upload");
  await expect(
    restoreWorkflow.getByRole("button", { name: "Download package" }),
  ).toBeVisible();
  await expect(
    restoreWorkflow.getByRole("button", { name: "Open Transfers" }),
  ).toBeVisible();
  await chooseVpsBySearch(
    restoreWorkflow,
    "Restore target client",
    "fra",
    /core-fra-02.*agent-fra-02/,
  );
  await expect(restoreWorkflow.getByText(destinationRoot)).toBeVisible();
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review draft restore" }),
  );
  await expect(restoreWorkflow.getByLabel("Confirm draft restore")).toBeVisible(
    { timeout: 15_000 },
  );
  await activate(
    restoreWorkflow
      .getByLabel("Confirm draft restore")
      .getByRole("button", { name: "Save draft restore" }),
  );
  await expect(page.getByText(/Draft restore cccccccc saved/)).toBeVisible();
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
  await expect(stagedArchive).toHaveValue(
    "agent-fra-02:50505050-2222-4333-8444-555555555555",
  );
  await expect(stagedArchive).toHaveAttribute("title", archivePath);
  const dryRunToggle = restoreWorkflow.getByLabel("Dry-run rehearsal");
  await expect(dryRunToggle).toBeChecked();
  await expect(
    restoreWorkflow.getByRole("button", { name: "Review dry run" }),
  ).not.toHaveClass(/dangerPrimary/);
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review dry run" }),
  );
  const dryRunConfirmation = restoreWorkflow.getByLabel("Confirm restore");
  await expect(dryRunConfirmation).toContainText("Dry run");
  await expect(dryRunConfirmation).toContainText("Simulates");
  await expect(dryRunConfirmation).not.toContainText("Replaces");
  await activate(
    dryRunConfirmation.getByRole("button", { name: "Close confirmation" }),
  );
  await dryRunToggle.setChecked(false);
  await expect(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  ).toHaveClass(/dangerPrimary/);
  await restoreWorkflow.getByLabel("Restore max timeout seconds").fill("120");
  await activate(
    restoreWorkflow.getByRole("button", { name: "Review live restore" }),
  );
  const liveRestoreConfirmation = restoreWorkflow.getByLabel("Confirm restore");
  await expect(liveRestoreConfirmation).toBeVisible();
  await expect(liveRestoreConfirmation).toContainText("Replaces");
  await activate(
    liveRestoreConfirmation.getByRole("button", { name: "Run restore" }),
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

test("restore staging opens Transfers with the selected VPS", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "restore-to-transfer routing is covered in the desktop workflow",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Restore");
  await activate(page.getByRole("button", { name: "Choose restore artifact" }));
  const restoreWorkflow = page.getByLabel("Choose restore artifact");
  await restoreWorkflow
    .getByLabel("Restore source backup request")
    .selectOption(backupId);
  await chooseVpsBySearch(
    restoreWorkflow,
    "Restore target client",
    "sfo",
    /edge-sfo-01.*agent-sfo-01/,
  );
  await activate(
    restoreWorkflow.getByRole("button", { name: "Open Transfers" }),
  );

  await expect(
    page.getByRole("heading", { level: 1, name: "Transfers" }),
  ).toBeVisible();
  await expect(page.getByLabel("Transfer target VPS")).toHaveValue(
    /edge-sfo-01/,
  );
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

  await expect(page.getByText(/Artifact dddddddd ready/)).toBeVisible();
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
    page.getByRole("heading", { level: 1, name: "Network tests" }),
  ).toBeVisible();
  const networkTestsPanel = page.locator(".fleetPanel", {
    has: page.getByRole("heading", { level: 2, name: "Network tests" }),
  });
  await expect(networkTestsPanel).toContainText("Required privilege");
  await expect(networkTestsPanel).toContainText("Inspect available");
  await expect(networkTestsPanel).toContainText(
    "100 Mbps, 14 ms target, 0% loss, OSPF 22",
  );
  await expect(networkTestsPanel).toContainText(
    "3s, 16 MiB cap, 100 Mbps cap, TCP 5201, timeout 5000 ms",
  );
  await expect(networkTestsPanel).toContainText(
    "Probe 12.4 ms avg, 0.25% loss",
  );
  await expect(networkTestsPanel).toContainText(
    "Throughput 10.1 Mbps avg, 11.8 Mbps max",
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
  await expect(trendCharts.getByRole("button")).toHaveCount(0);

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
      plan_id: tunnelPlans[0].id,
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
      plan_id: tunnelPlans[0].id,
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
  await page.getByLabel("Network speed test rate limit Mbps").fill("25");
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
      plan_id: tunnelPlans[0].id,
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
  await expect(
    page.getByRole("heading", { name: "OSPF cost control" }),
  ).toBeVisible();
  const ospfTable = page.getByLabel("OSPF updater plans data grid");
  await expect(ospfTable).toContainText("sfo-fra-gre");
  await expect(ospfTable).toContainText("Reviewed");
  await expect(ospfTable).toContainText("Review required");
  await expect(ospfTable).toContainText("max delta +8");
  await expect(ospfTable).toContainText("5 samples, 0 degraded");
  const ospfPlanRow = ospfTable
    .getByRole("row")
    .filter({ hasText: "sfo-fra-gre" })
    .first();
  await ospfPlanRow.click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Check updater", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Apply cost", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  if ((await ospfPlanRow.getAttribute("aria-expanded")) !== "true") {
    await activate(ospfPlanRow);
  }
  await expect(
    ospfTable.getByRole("button", {
      name: "Close OSPF updater plans row details",
    }),
  ).toBeVisible();
  await expect(ospfTable).toContainText("FRR OSPF updater");
  await expect(ospfTable).toContainText("VPS Configuration preset");
  await expect(ospfTable).toContainText("Plan override");
  await expect(ospfTable).toContainText("FRA routing cost");
  await expect(ospfTable).toContainText("Operator review required");
  await expect(ospfTable).toContainText("14 / 14");
  await expect(ospfTable).toContainText("22 · max delta +8");
  await expect(ospfTable).toContainText("3 consecutive · 2 required");
  await ospfPlanRow.click({ button: "right" });
  await activate(
    page.getByRole("menuitem", { name: "Apply cost", exact: true }),
  );
  const ospfPrompt = page.locator(".confirmationPrompt").last();
  await expect(ospfPrompt).toBeVisible();
  await expect(ospfPrompt).toContainText("Confirm OSPF cost update");
  await expect(ospfPrompt).toContainText("Current costs");
  await expect(ospfPrompt).toContainText("14 / 14");
  await expect(ospfPrompt).toContainText(
    `r${ospfUpdatePlans[0].plan_revision}`,
  );
  await expect(ospfPrompt).toContainText("Desired cost");
  await expect(ospfPrompt).toContainText("Updater snapshots");
  await expect(ospfPrompt).toContainText(ospfUpdatePlans[0].evidence_summary);
  await activate(
    ospfPrompt.getByRole("button", { name: "Apply routing cost" }),
  );
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
      desired_ospf_cost: ospfUpdatePlans[0].recommended_ospf_cost,
      plan_revision: ospfUpdatePlans[0].plan_revision,
      left_adapter_definition_hash:
        ospfUpdatePlans[0].left_adapter_definition_hash,
      left_current_ospf_cost: ospfUpdatePlans[0].left_current_ospf_cost,
      recommendation_id: ospfUpdatePlans[0].recommendation_id,
      right_adapter_definition_hash:
        ospfUpdatePlans[0].right_adapter_definition_hash,
      right_current_ospf_cost: ospfUpdatePlans[0].right_current_ospf_cost,
    },
  });
  expectPrivilegeAssertion((ospfRequest as { body: unknown }).body);
  await expect(ospfTable).toContainText("Jobs in progress");
  await expect(page.getByRole("button", { name: /rollback/i })).toHaveCount(0);
});
