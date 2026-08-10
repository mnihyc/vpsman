import {
  expect,
  test,
  type Locator,
  type Page,
  type TestInfo,
} from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

const portForwardRuleIds = {
  publicWeb: "4f000000-0000-4000-8000-000000000001",
  stagedSsh: "4f000000-0000-4000-8000-000000000003",
  retiredDns: "4f000000-0000-4000-8000-000000000004",
} as const;
const portForwardGridStorageKey = "vpsman.network.portForwardRules";

function portForwardGrid(page: Page): Locator {
  return page.getByLabel("Port-forward rules data grid");
}

function portForwardRecord(
  page: Page,
  testInfo: TestInfo,
  id: string,
  name: string,
): Locator {
  const grid = portForwardGrid(page);
  return testInfo.project.name.startsWith("mobile")
    ? grid.getByLabel(`Port-forward rules mobile card ${id}`)
    : grid.locator(".gridBody [role=row]", { hasText: name }).first();
}

async function openPortForwardDetails(
  page: Page,
  testInfo: TestInfo,
  id: string,
  name: string,
): Promise<Locator> {
  const record = portForwardRecord(page, testInfo, id, name);
  await record.click();
  const details = portForwardGrid(page).locator(".gridExpandedRow");
  await expect(details).toBeVisible();
  return details;
}

async function invokePortForwardAction(
  page: Page,
  testInfo: TestInfo,
  record: Locator,
  action: string,
): Promise<void> {
  if (testInfo.project.name.startsWith("mobile")) {
    await record.getByRole("checkbox").check();
    await portForwardGrid(page)
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await page.getByRole("menuitem", { name: action, exact: true }).click();
    return;
  }
  await record.click({ button: "right" });
  await page.getByRole("menuitem", { name: action, exact: true }).click();
}

test.beforeEach(async ({ page }, testInfo) => {
  await installConsoleApiMock(page, {
    operatorRoleOverride: testInfo.title.includes("without network write scope")
      ? "operator"
      : undefined,
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Network", "Port forwards");
});

test("port-forward registry, details, and reviewed create stay revision-bound", async ({
  page,
}, testInfo) => {
  await expect(
    page.getByRole("heading", { name: "Port forwarding" }).first(),
  ).toBeVisible();
  const grid = portForwardGrid(page);
  await expect(grid).toContainText("Public web ingress");
  await expect(
    grid.getByRole("columnheader", { name: /^Actions?$/ }),
  ).toHaveCount(0);
  await expect(grid.getByLabel("Port-forward rules columns")).toBeVisible();
  const publicWebRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  if (testInfo.project.name.startsWith("mobile")) {
    await publicWebRow.getByRole("checkbox").check();
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await expect(
      page.getByRole("menuitem", { name: "Edit", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("menuitem", { name: "Disable", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
  } else {
    await publicWebRow.click({ button: "right" });
    await expect(
      page.getByRole("menuitem", { name: "Edit", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("menuitem", { name: "Disable", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
  }
  const details = await openPortForwardDetails(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await expect(details).toContainText("IPv4 forwarding");
  await expect(details).toContainText("nftables v1.1.3");
  await expect(details).toContainText("Control desired");
  await expect(details).toContainText("Agent desired");
  await expect(details).toContainText("Observed table");
  await expect(details).toContainText("Domain");
  await expect(details).toContainText("app.internal");
  await details
    .getByRole("button", { name: "Close Port-forward rules row details" })
    .click();

  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await expect(editor).toBeVisible();
  await expect(editor.getByLabel("Port-forward rule VPS")).toBeFocused();
  await expect(editor.getByLabel("Enabled")).not.toBeChecked();
  await expect(editor.locator(".portMappingPreview")).toHaveClass(/idle/);
  await expect(editor.locator(".portMappingPreview")).toContainText(
    "Enter incoming and target ports to preview the exact mappings",
  );
  await editor.getByLabel("Port-forward rule VPS").fill("edge-sfo");
  await page
    .getByRole("listbox", { name: "Port-forward rule VPS options" })
    .getByRole("option", { name: /edge-sfo-01/ })
    .click();
  await editor.getByLabel("Name", { exact: true }).fill("Internal application");
  await editor.getByRole("button", { name: "Both" }).click();
  await editor.getByLabel("Incoming ports").fill("not-a-port");
  await editor.getByLabel("Target ports").fill("80");
  const mappingError = editor.locator(".portMappingPreview.invalid span");
  await expect(mappingError).toBeVisible();
  await expect(mappingError).toHaveAttribute(
    "title",
    (await mappingError.textContent())?.trim() ?? "",
  );
  await editor.getByLabel("Incoming ports").fill("8080,10000-10010");
  await editor.getByLabel("Target ports").fill("80,20000-20010");
  await editor.getByLabel("Target IP or hostname").fill("app.internal");
  await editor.getByRole("button", { name: "Resolve" }).click();
  await editor
    .getByRole("group", { name: "Resolved addresses" })
    .getByRole("radio", { name: /10\.20\.0\.21/ })
    .check();
  await editor.getByLabel("Enabled").check();
  await editor.getByRole("button", { name: "Create rule" }).click();

  const confirmation = page.getByLabel("Confirm rule creation");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("BOTH 8080,10000-10010");
  await expect(editor.getByLabel("Name", { exact: true })).toBeDisabled();
  await confirmation.getByRole("button", { name: "Create and apply" }).click();
  await expect(
    page
      .locator(".portForwardRegistryFeedback")
      .getByText(/Rule created; apply job .* queued/),
  ).toBeVisible();
  await expect(editor).toBeHidden();
  await expect(grid).toContainText("Internal application");

  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: { portForwardRules: unknown[] };
        }
      ).__vpsmanTestRequests.portForwardRules,
  );
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({
    action: "create",
    body: {
      target_hostname: "app.internal",
      target_ip: "10.20.0.21",
    },
  });
});

test("stored domains remain optional table fields and can be re-resolved from Edit", async ({
  page,
}, testInfo) => {
  const grid = portForwardGrid(page);
  const publicWebRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );

  await grid.getByLabel("Port-forward rules search").fill("app.internal");
  await expect(publicWebRow).toBeVisible();
  await expect(
    portForwardRecord(
      page,
      testInfo,
      portForwardRuleIds.stagedSsh,
      "Staged SSH alternate",
    ),
  ).toHaveCount(0);
  await grid.getByLabel("Port-forward rules search").fill("");

  if (!testInfo.project.name.startsWith("mobile")) {
    await expect(
      grid.getByRole("button", { name: "Domain", exact: true }),
    ).toHaveCount(0);
  }
  await grid.getByLabel("Port-forward rules columns").click();
  const hiddenDomainField = page.getByRole("menuitemcheckbox", {
    name: /Domain.*hidden/i,
  });
  await expect(hiddenDomainField).toHaveAttribute("aria-checked", "false");
  await hiddenDomainField.click();
  await page.keyboard.press("Escape");

  if (testInfo.project.name.startsWith("mobile")) {
    await expect(publicWebRow).toContainText("Domain");
  } else {
    await expect(
      grid.getByRole("button", { name: "Domain", exact: true }),
    ).toBeVisible();
  }
  await expect(publicWebRow).toContainText("app.internal");

  await invokePortForwardAction(page, testInfo, publicWebRow, "Clone");
  let editor = page.locator(".portForwardEditor");
  await expect(editor.getByLabel("Target IP or hostname")).toHaveValue(
    "app.internal",
  );
  await expect(editor.locator(".portMappingPreview")).toContainText(
    "10.20.0.15",
  );
  await editor
    .getByRole("button", { name: "Close port-forward editor" })
    .click();

  await invokePortForwardAction(page, testInfo, publicWebRow, "Edit");
  editor = page.locator(".portForwardEditor");
  const targetInput = editor.getByLabel("Target IP or hostname");
  await expect(targetInput).toHaveValue("app.internal");
  await expect(editor.locator(".portMappingPreview")).toContainText(
    "10.20.0.15",
  );

  await editor.getByRole("button", { name: "Resolve" }).click();
  await expect(
    editor.getByRole("button", { name: "Save changes" }),
  ).toBeDisabled();
  await editor
    .getByRole("group", { name: "Resolved addresses" })
    .getByRole("radio", { name: /2001:db8:20::21/ })
    .check();
  await editor.getByRole("button", { name: "Save changes" }).click();

  const confirmation = page.getByLabel("Confirm rule update");
  await expect(confirmation).toContainText("app.internal");
  await expect(confirmation).toContainText("2001:db8:20::21");
  await confirmation.getByRole("button", { name: "Save and apply" }).click();

  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: {
            portForwardRules: Array<{
              action: string;
              body: Record<string, unknown>;
            }>;
          };
        }
      ).__vpsmanTestRequests.portForwardRules,
  );
  expect(requests.find((request) => request.action === "update")).toMatchObject(
    {
      body: {
        target_hostname: "app.internal",
        target_ip: "2001:db8:20::21",
      },
    },
  );
});

test("port-forward columns persist flexible desktop sizing without constraining mobile cards", async ({
  page,
}, testInfo) => {
  const mobile = testInfo.project.name.startsWith("mobile");
  let grid = portForwardGrid(page);

  if (mobile) {
    const firstCard = grid.getByLabel(
      `Port-forward rules mobile card ${portForwardRuleIds.publicWeb}`,
    );
    await expect(firstCard).toBeVisible();
    const baseline = await firstCard.evaluate((card) => {
      const primary = card.querySelector<HTMLElement>(".gridMobilePrimary");
      return {
        cardWidth: card.getBoundingClientRect().width,
        primaryWidth: primary?.getBoundingClientRect().width ?? 0,
      };
    });

    await page.evaluate((storageKey) => {
      const raw = window.localStorage.getItem(storageKey);
      const preferences = raw
        ? (JSON.parse(raw) as Record<string, unknown>)
        : {};
      window.localStorage.setItem(
        storageKey,
        JSON.stringify({
          ...preferences,
          columnSizing: {
            ...((preferences.columnSizing as Record<string, number>) ?? {}),
            rule: 72,
          },
        }),
      );
    }, portForwardGridStorageKey);
    await page.reload();
    await waitForConsoleShell(page);
    await openConsoleSubpage(page, "Network", "Port forwards");

    grid = portForwardGrid(page);
    const persistedCard = grid.getByLabel(
      `Port-forward rules mobile card ${portForwardRuleIds.publicWeb}`,
    );
    await expect(persistedCard).toBeVisible();
    const persisted = await persistedCard.evaluate((card) => {
      const primary = card.querySelector<HTMLElement>(".gridMobilePrimary");
      return {
        cardWidth: card.getBoundingClientRect().width,
        primaryWidth: primary?.getBoundingClientRect().width ?? 0,
      };
    });

    expect(persisted.cardWidth).toBeGreaterThan(250);
    expect(persisted.primaryWidth).toBeGreaterThan(100);
    expect(
      Math.abs(persisted.cardWidth - baseline.cardWidth),
    ).toBeLessThanOrEqual(1);
    expect(
      Math.abs(persisted.primaryWidth - baseline.primaryWidth),
    ).toBeLessThanOrEqual(1);
    expect(
      await page.evaluate(
        (storageKey) =>
          (
            JSON.parse(window.localStorage.getItem(storageKey) ?? "{}") as {
              columnSizing?: Record<string, number>;
            }
          ).columnSizing?.rule,
        portForwardGridStorageKey,
      ),
    ).toBe(72);
    expect(
      await page.evaluate(
        () =>
          document.documentElement.scrollWidth -
          document.documentElement.clientWidth,
      ),
    ).toBeLessThanOrEqual(1);
    return;
  }

  const headerCells = grid.locator(".gridHeaderCell");
  const ruleHeader = headerCells.filter({ hasText: "Rule / VPS" }).first();
  await expect(ruleHeader).toBeVisible();
  const ruleHeaderIndex = await ruleHeader.evaluate((header) =>
    Array.from(header.parentElement?.children ?? []).indexOf(header),
  );
  expect(ruleHeaderIndex).toBe(2);

  for (const structuralHeader of [headerCells.nth(0), headerCells.nth(1)]) {
    await expect(structuralHeader.locator(".gridResizeHandle")).toHaveCount(0);
    await expect(structuralHeader.locator(".gridDragHandle")).toHaveCount(0);
    expect((await structuralHeader.boundingBox())?.width ?? 0).toBeCloseTo(
      42,
      0,
    );
  }

  const resizeHandle = ruleHeader.locator(".gridResizeHandle");
  await expect(resizeHandle).toBeVisible();
  const initialHeaderBox = await ruleHeader.boundingBox();
  const handleBox = await resizeHandle.boundingBox();
  expect(initialHeaderBox).not.toBeNull();
  expect(handleBox).not.toBeNull();
  if (!initialHeaderBox || !handleBox) return;

  const handleCenterX = handleBox.x + handleBox.width / 2;
  const handleCenterY = handleBox.y + handleBox.height / 2;
  await page.mouse.move(handleCenterX, handleCenterY);
  await page.mouse.down();
  await page.mouse.move(
    handleCenterX - Math.max(0, initialHeaderBox.width - 72),
    handleCenterY,
    { steps: 8 },
  );
  await page.mouse.up();

  await expect
    .poll(async () => (await ruleHeader.boundingBox())?.width ?? 0)
    .toBeLessThanOrEqual(100);
  const narrowHeaderBox = await ruleHeader.boundingBox();
  expect(narrowHeaderBox).not.toBeNull();
  expect(narrowHeaderBox?.width ?? 0).toBeGreaterThanOrEqual(64);
  expect(narrowHeaderBox?.width ?? 0).toBeLessThan(180);

  const firstDesktopRow = grid
    .locator(".gridBody .gridRecord > [role=row]")
    .first();
  const ruleCell = firstDesktopRow.locator(".gridCell").nth(ruleHeaderIndex);
  const narrowCellBox = await ruleCell.boundingBox();
  expect(narrowCellBox).not.toBeNull();
  expect(
    Math.abs((narrowCellBox?.x ?? 0) - (narrowHeaderBox?.x ?? 0)),
  ).toBeLessThanOrEqual(1);
  expect(
    Math.abs((narrowCellBox?.width ?? 0) - (narrowHeaderBox?.width ?? 0)),
  ).toBeLessThanOrEqual(1);

  await expect
    .poll(() =>
      page.evaluate((storageKey) => {
        const width = (
          JSON.parse(window.localStorage.getItem(storageKey) ?? "{}") as {
            columnSizing?: Record<string, number>;
          }
        ).columnSizing?.rule;
        return typeof width === "number" && width >= 64 && width <= 100;
      }, portForwardGridStorageKey),
    )
    .toBe(true);
  const storedRuleWidth = await page.evaluate(
    (storageKey) =>
      (
        JSON.parse(window.localStorage.getItem(storageKey) ?? "{}") as {
          columnSizing?: Record<string, number>;
        }
      ).columnSizing?.rule ?? 0,
    portForwardGridStorageKey,
  );
  expect(storedRuleWidth).toBeGreaterThanOrEqual(64);

  await page.reload();
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Network", "Port forwards");
  grid = portForwardGrid(page);
  const reloadedRuleHeader = grid
    .locator(".gridHeaderCell", { hasText: "Rule / VPS" })
    .first();
  await expect(reloadedRuleHeader).toBeVisible();
  const reloadedHeaderBox = await reloadedRuleHeader.boundingBox();
  expect(reloadedHeaderBox).not.toBeNull();
  expect(
    Math.abs((reloadedHeaderBox?.width ?? 0) - storedRuleWidth),
  ).toBeLessThanOrEqual(1);
  expect(reloadedHeaderBox?.width ?? 0).toBeLessThan(180);
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
});

test("hostname re-resolution gates review until explicit candidate selection", async ({
  page,
}, testInfo) => {
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    let releaseResolution = () => {};
    const resolutionGate = new Promise<void>((resolve) => {
      releaseResolution = resolve;
    });
    const state = window as unknown as {
      __portForwardGatedResolutionStarted: boolean;
      __releasePortForwardGatedResolution: () => void;
    };
    state.__portForwardGatedResolutionStarted = false;
    state.__releasePortForwardGatedResolution = releaseResolution;

    let gated = false;
    window.fetch = async (input, init) => {
      const requestUrl = input instanceof Request ? input.url : String(input);
      const pathname = new URL(requestUrl, window.location.href).pathname;
      const method = (
        init?.method ?? (input instanceof Request ? input.method : "GET")
      ).toUpperCase();
      if (
        !gated &&
        pathname === "/api/v1/network/resolve-hostname" &&
        method === "POST"
      ) {
        gated = true;
        state.__portForwardGatedResolutionStarted = true;
        await resolutionGate;
      }
      return originalFetch(input, init);
    };
  });

  const publicWebRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await invokePortForwardAction(page, testInfo, publicWebRow, "Edit");

  const editor = page.locator(".portForwardEditor");
  const save = editor.getByRole("button", { name: "Save changes" });
  await expect(save).toBeEnabled();
  await expect(editor.locator(".portMappingPreview")).toContainText(
    "10.20.0.15",
  );

  await editor.getByRole("button", { name: "Resolve" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as {
              __portForwardGatedResolutionStarted: boolean;
            }
          ).__portForwardGatedResolutionStarted,
      ),
    )
    .toBe(true);

  const resolvingSave = editor.getByRole("button", {
    name: "Resolving hostname",
  });
  await expect(resolvingSave).toBeDisabled();
  await expect(resolvingSave).toHaveAttribute("aria-busy", "true");
  await expect(resolvingSave).toHaveAttribute(
    "title",
    "Wait for hostname resolution to finish before reviewing this rule, then select one resolved address",
  );
  await expect(editor.locator(".portMappingPreview")).toContainText(
    "10.20.0.15",
  );
  await editor
    .locator("form")
    .evaluate((form: HTMLFormElement) => form.requestSubmit());
  await expect(page.getByLabel("Confirm rule update")).toHaveCount(0);

  await page.evaluate(() =>
    (
      window as unknown as {
        __releasePortForwardGatedResolution: () => void;
      }
    ).__releasePortForwardGatedResolution(),
  );

  const candidates = editor.getByRole("group", {
    name: "Resolved addresses",
  });
  await expect(candidates).toBeVisible();
  await expect(save).toBeDisabled();
  await expect(save).toHaveAttribute(
    "title",
    "Enter a literal target IP, or resolve and select a hostname result",
  );
  await expect(page.getByLabel("Confirm rule update")).toHaveCount(0);

  await candidates.getByRole("radio", { name: /2001:db8:20::21/ }).check();
  await expect(save).toBeEnabled();
  await save.click();

  const confirmation = page.getByLabel("Confirm rule update");
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("2001:db8:20::21");
  await expect(
    confirmation.getByText("10.20.0.15", { exact: true }),
  ).toHaveCount(0);
  await confirmation.getByRole("button", { name: "Save and apply" }).click();

  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: {
            portForwardRules: Array<{
              action: string;
              body: Record<string, unknown>;
            }>;
          };
        }
      ).__vpsmanTestRequests.portForwardRules,
  );
  expect(requests.filter((request) => request.action === "update")).toEqual([
    expect.objectContaining({
      body: expect.objectContaining({
        target_hostname: "app.internal",
        target_ip: "2001:db8:20::21",
      }),
    }),
  ]);
});

test("replacing a stored domain with a literal IP clears the domain provenance", async ({
  page,
}, testInfo) => {
  const publicWebRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await invokePortForwardAction(page, testInfo, publicWebRow, "Edit");

  const editor = page.locator(".portForwardEditor");
  const targetInput = editor.getByLabel("Target IP or hostname");
  await expect(targetInput).toHaveValue("app.internal");
  await targetInput.fill("192.0.2.44");
  await expect(editor.getByRole("button", { name: "Resolve" })).toHaveCount(0);
  await expect(
    editor.getByRole("group", { name: "Resolved addresses" }),
  ).toHaveCount(0);
  await editor.getByRole("button", { name: "Save changes" }).click();

  const confirmation = page.getByLabel("Confirm rule update");
  await expect(confirmation).toContainText("192.0.2.44");
  await expect(
    confirmation.getByText("app.internal", { exact: true }),
  ).toHaveCount(0);
  await confirmation.getByRole("button", { name: "Save and apply" }).click();
  await expect(editor).toBeHidden();

  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: {
            portForwardRules: Array<{
              action: string;
              body: Record<string, unknown>;
            }>;
          };
        }
      ).__vpsmanTestRequests.portForwardRules,
  );
  expect(requests.filter((request) => request.action === "update")).toEqual([
    expect.objectContaining({
      body: expect.objectContaining({
        target_hostname: null,
        target_ip: "192.0.2.44",
      }),
    }),
  ]);
});

test("a late hostname resolution cannot overwrite a newly opened editor", async ({
  page,
}, testInfo) => {
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    let releaseResolution = () => {};
    const resolutionGate = new Promise<void>((resolve) => {
      releaseResolution = resolve;
    });
    const state = window as unknown as {
      __portForwardResolutionStarted: boolean;
      __portForwardResolutionSettled: boolean;
      __releasePortForwardResolution: () => void;
    };
    state.__portForwardResolutionStarted = false;
    state.__portForwardResolutionSettled = false;
    state.__releasePortForwardResolution = releaseResolution;

    let gated = false;
    window.fetch = async (input, init) => {
      const requestUrl = input instanceof Request ? input.url : String(input);
      const pathname = new URL(requestUrl, window.location.href).pathname;
      const method = (
        init?.method ?? (input instanceof Request ? input.method : "GET")
      ).toUpperCase();
      if (
        !gated &&
        pathname === "/api/v1/network/resolve-hostname" &&
        method === "POST"
      ) {
        gated = true;
        state.__portForwardResolutionStarted = true;
        await resolutionGate;
        const response = await originalFetch(input, init);
        const originalJson = response.json.bind(response);
        response.json = async () => {
          const body = await originalJson();
          window.setTimeout(() => {
            state.__portForwardResolutionSettled = true;
          }, 0);
          return body;
        };
        return response;
      }
      return originalFetch(input, init);
    };
  });

  const publicWebRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await invokePortForwardAction(page, testInfo, publicWebRow, "Edit");
  let editor = page.locator(".portForwardEditor");
  await editor.getByRole("button", { name: "Resolve" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as unknown as { __portForwardResolutionStarted: boolean })
            .__portForwardResolutionStarted,
      ),
    )
    .toBe(true);
  await editor
    .getByRole("button", { name: "Close port-forward editor" })
    .click();
  if (testInfo.project.name.startsWith("mobile")) {
    await publicWebRow.getByRole("checkbox").uncheck();
  }

  const stagedSshRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.stagedSsh,
    "Staged SSH alternate",
  );
  await invokePortForwardAction(page, testInfo, stagedSshRow, "Edit");
  editor = page.locator(".portForwardEditor");
  await expect(editor.getByLabel("Name", { exact: true })).toHaveValue(
    "Staged SSH alternate",
  );
  await expect(editor.getByLabel("Target IP or hostname")).toHaveValue(
    "10.30.0.8",
  );

  await page.evaluate(() =>
    (
      window as unknown as { __releasePortForwardResolution: () => void }
    ).__releasePortForwardResolution(),
  );
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as unknown as { __portForwardResolutionSettled: boolean })
            .__portForwardResolutionSettled,
      ),
    )
    .toBe(true);
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        window.requestAnimationFrame(() =>
          window.requestAnimationFrame(() => resolve()),
        ),
      ),
  );

  await expect(editor.getByLabel("Name", { exact: true })).toHaveValue(
    "Staged SSH alternate",
  );
  await expect(editor.getByLabel("Target IP or hostname")).toHaveValue(
    "10.30.0.8",
  );
  await expect(
    editor.getByRole("group", { name: "Resolved addresses" }),
  ).toHaveCount(0);
});

test("unsupported agents allow disabled drafts but not enabled apply", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await editor.getByLabel("Port-forward rule VPS").fill("backup-nyc");
  await page
    .getByRole("listbox", { name: "Port-forward rule VPS options" })
    .getByRole("option", { name: /backup-nyc-03/ })
    .click();
  await editor.getByLabel("Name", { exact: true }).fill("Future service");
  await editor.getByLabel("Incoming ports").fill("8443");
  await editor.getByLabel("Target ports").fill("443");
  await editor.getByLabel("Target IP or hostname").fill("10.30.0.9");
  await editor.getByLabel("Enabled").check();
  await expect(editor).toContainText(
    "Agent lacks CAP_NET_ADMIN in the host network namespace",
  );
  await expect(
    editor.getByRole("button", { name: "Create rule" }),
  ).toBeDisabled();

  await editor.getByLabel("Enabled").uncheck();
  await editor.getByRole("button", { name: "Create rule" }).click();
  await expect(page.getByText("Rule created")).toBeVisible();
  await expect(page.getByText("Future service", { exact: true })).toBeVisible();
  const requests = await page.evaluate(
    () =>
      (
        window as unknown as {
          __vpsmanTestRequests: { portForwardRules: unknown[] };
        }
      ).__vpsmanTestRequests.portForwardRules,
  );
  expect(requests[0]).toMatchObject({
    action: "create",
    body: { target_hostname: null, target_ip: "10.30.0.9" },
  });
});

test("never-applied disabled drafts explain and complete immediate deletion", async ({
  page,
}, testInfo) => {
  const row = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.stagedSsh,
    "Staged SSH alternate",
  );
  await invokePortForwardAction(page, testInfo, row, "Delete");

  const confirmation = page.getByLabel("Confirm delete");
  await expect(confirmation).toContainText(
    "This disabled draft has never been applied.",
  );
  await expect(confirmation).toContainText(
    "no agent cleanup or apply job is required",
  );
  await confirmation.getByRole("button", { name: "Delete rule" }).click();

  await expect(
    portForwardGrid(page).getByText("Staged SSH alternate", { exact: true }),
  ).toHaveCount(0);
  await expect(page.locator(".portForwardRegistryFeedback")).toContainText(
    "no host apply required",
  );
});

test("operators without network write scope keep read-only inspection", async ({
  page,
}, testInfo) => {
  const create = page.getByRole("button", { name: "Create rule" });
  await expect(create).toBeDisabled();
  await expect(create).toHaveAttribute(
    "title",
    "Operator role and network:write scope required",
  );
  await expect(
    page.getByLabel(
      `Select Port-forward rules row ${portForwardRuleIds.publicWeb}`,
    ),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Refresh", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Refresh", exact: true }),
  ).toHaveAttribute(
    "title",
    "Reload latest stored desired state and agent evidence; this does not request a live agent inspection",
  );

  const row = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  if (testInfo.project.name.startsWith("mobile")) {
    await expect(
      row.getByRole("button", { name: "Edit", exact: true }),
    ).toHaveCount(0);
    await expect(
      portForwardGrid(page).getByRole("button", {
        name: "Actions",
        exact: true,
      }),
    ).toHaveCount(0);
  } else {
    await row.click({ button: "right" });
    await expect(
      page.getByRole("menuitem", { name: "Edit", exact: true }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");
  }
  const details = await openPortForwardDetails(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await expect(details).toContainText("Observed table");
  await expect(
    details.getByRole("button", { name: "Edit", exact: true }),
  ).toHaveCount(0);
});

test("rule names enforce the API UTF-8 byte limit with an exact reason", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await editor.getByLabel("Name", { exact: true }).fill("é".repeat(65));
  await editor.getByLabel("Incoming ports").fill("8443");
  await editor.getByLabel("Target ports").fill("443");
  await editor.getByLabel("Target IP or hostname").fill("10.30.0.9");
  const create = editor.getByRole("button", { name: "Create rule" });
  await expect(create).toBeDisabled();
  await expect(create).toHaveAttribute(
    "title",
    "Rule name must not exceed 128 UTF-8 bytes",
  );
});

test("delete becomes removal pending instead of disappearing without evidence", async ({
  page,
}, testInfo) => {
  const row = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await invokePortForwardAction(page, testInfo, row, "Delete");
  const confirmation = page.getByLabel("Confirm delete");
  await expect(confirmation).toContainText("Removal pending");
  await confirmation.getByRole("button", { name: "Delete rule" }).click();
  await expect(portForwardGrid(page)).toContainText("removal pending");
});

test("removal-pending rules keep ordinary actions hidden and Forget reason-bound", async ({
  page,
}, testInfo) => {
  const row = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.retiredDns,
    "Retired DNS relay",
  );
  if (testInfo.project.name.startsWith("mobile")) {
    await row.getByRole("checkbox").check();
    await expect(
      portForwardGrid(page)
        .locator(".gridToolbarActions")
        .getByRole("button", { name: "Actions", exact: true }),
    ).toBeDisabled();
  } else {
    await row.click({ button: "right" });
    await expect(page.getByRole("menuitem")).toHaveCount(0);
  }

  const details = await openPortForwardDetails(
    page,
    testInfo,
    portForwardRuleIds.retiredDns,
    "Retired DNS relay",
  );
  await expect(details).toContainText(
    "Removal pending until the agent confirms the owned table no longer contains this rule.",
  );
  const forget = details.getByRole("button", { name: "Forget", exact: true });
  await expect(forget).toBeDisabled();
  await details
    .getByPlaceholder("Decommission reason")
    .fill("VPS decommissioned");
  await expect(forget).toBeEnabled();
});

test("mobile port-forward workflow has no page-level horizontal overflow", async ({
  page,
}, testInfo) => {
  test.skip(!testInfo.project.name.startsWith("mobile"));
  const firstRow = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await expect(firstRow).toContainText("enabled");
  await expect(firstRow).toContainText("applied");
  const details = await openPortForwardDetails(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await expect(details).toContainText("Observed table");
  await expect(
    details.getByLabel("Actions for Public web ingress"),
  ).toHaveCount(0);
  await details
    .getByRole("button", { name: "Close Port-forward rules row details" })
    .click();
  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await expect(editor).toBeVisible();
  const previewText = editor.locator(".portMappingPreview > span");
  await expect(previewText).not.toHaveAttribute("title", /\S/);
  expect(
    await previewText.evaluate(
      (element) => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
  for (const control of [
    editor.getByLabel("Port-forward rule VPS"),
    editor.getByLabel("Name", { exact: true }),
    editor.getByLabel("Incoming ports"),
    editor.getByLabel("Target ports"),
    editor.getByLabel("Target IP or hostname"),
  ]) {
    const box = await control.boundingBox();
    expect(box?.height ?? 0).toBeGreaterThanOrEqual(36);
  }
  const enabledLabel = editor.locator(".portForwardEnabled");
  await expect(enabledLabel).toHaveCSS("display", "flex");
  const enabledGeometry = await enabledLabel.evaluate((label) => {
    const checkbox = label.querySelector("input");
    const text = label.querySelector("span");
    if (!checkbox || !text) return null;
    const checkboxBox = checkbox.getBoundingClientRect();
    const textBox = text.getBoundingClientRect();
    return {
      horizontalGap: textBox.left - checkboxBox.right,
      verticalCenterDelta: Math.abs(
        textBox.top +
          textBox.height / 2 -
          (checkboxBox.top + checkboxBox.height / 2),
      ),
    };
  });
  expect(enabledGeometry?.horizontalGap ?? 0).toBeGreaterThan(0);
  expect(
    enabledGeometry?.verticalCenterDelta ?? Number.POSITIVE_INFINITY,
  ).toBeLessThanOrEqual(2);
  const overflow = await page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  );
  expect(overflow).toBeLessThanOrEqual(1);
});

test("bulk actions state their exact eligible subset", async ({ page }) => {
  const grid = portForwardGrid(page);
  await grid
    .getByRole("button", { name: "Select visible Port-forward rules" })
    .click();
  await expect(grid).toContainText("4 selected");
  await grid.getByRole("button", { name: "Actions", exact: true }).click();
  await expect(page.getByRole("menuitem", { name: "Enable" })).toBeDisabled();
  await expect(page.getByRole("menuitem", { name: "Disable" })).toHaveAttribute(
    "title",
    "Review disabling 2 selected enabled rules.",
  );
  await expect(page.getByRole("menuitem", { name: "Reapply" })).toHaveAttribute(
    "title",
    "Review reapplying the complete forwarding table on 2 eligible VPSs.",
  );
  await expect(page.getByRole("menuitem", { name: "Delete" })).toHaveAttribute(
    "title",
    "Review deleting 3 selected active rules.",
  );
});

test("applied status explains evidence limits", async ({ page }, testInfo) => {
  const row = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await expect(
    row.locator(
      '.portForwardStatus.status-applied:visible[title="Owned nftables table matches desired state; target reachability is not tested"]',
    ),
  ).toBeVisible();
});

test("partial dispatch failure explains saved state, target impact, and recovery", async ({
  page,
}, testInfo) => {
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof Request
            ? input.url
            : input.toString(),
        window.location.origin,
      );
      const method = (
        init?.method ?? (input instanceof Request ? input.method : "GET")
      ).toUpperCase();
      const response = await originalFetch(input, init);
      if (
        method === "POST" &&
        url.pathname.endsWith("/disable") &&
        url.pathname.startsWith("/api/v1/port-forward-rules/")
      ) {
        const body = (await response.json()) as Record<string, unknown>;
        return new Response(
          JSON.stringify({
            ...body,
            sync: {
              error:
                "Agent command queue is full. Desired state remains saved; inspect gateway/API capacity and retry Reapply after the queue drains.",
              job_id: null,
              status: "queue_failed",
            },
          }),
          {
            headers: { "Content-Type": "application/json" },
            status: response.status,
          },
        );
      }
      return response;
    };
  });

  const row = portForwardRecord(
    page,
    testInfo,
    portForwardRuleIds.publicWeb,
    "Public web ingress",
  );
  await invokePortForwardAction(page, testInfo, row, "Disable");
  const feedback = page.locator(".portForwardRegistryFeedback");
  await page
    .getByLabel("Confirm disable")
    .getByRole("button", { name: "Disable rule" })
    .click();

  await expect(feedback).toContainText(
    "Rule disabled; desired state saved, but apply was not queued: Agent command queue is full. Desired state remains saved; inspect gateway/API capacity and retry Reapply after the queue drains.",
  );
});

test("transport failure replaces bare browser errors with operator recovery guidance", async ({
  page,
}) => {
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof Request
            ? input.url
            : input.toString(),
        window.location.origin,
      );
      const method = (
        init?.method ?? (input instanceof Request ? input.method : "GET")
      ).toUpperCase();
      if (method === "GET" && url.pathname === "/api/v1/port-forward-rules") {
        throw new TypeError("NetworkError when attempting to fetch resource");
      }
      return originalFetch(input, init);
    };
  });

  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  const feedback = page.locator(".portForwardActionFeedback");
  await expect(feedback).toContainText(
    "The control plane did not return a readable response.",
  );
  await expect(feedback).toContainText(
    "Check API availability, TLS, reverse-proxy routing, and same-origin/CORS configuration before retrying. No success is assumed.",
  );
  await expect(feedback).toContainText(
    "Browser reported: NetworkError when attempting to fetch resource.",
  );
});
