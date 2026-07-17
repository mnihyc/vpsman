import { expect, test } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

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
}) => {
  await expect(page.getByRole("heading", { name: "Port forwarding" }).first()).toBeVisible();
  await expect(page.getByRole("table", { name: "Port-forward rules" })).toContainText(
    "Public web ingress",
  );
  await page.getByText("Public web ingress", { exact: true }).click();
  const details = page.getByRole("region", { name: "Details for Public web ingress" });
  await expect(details).toBeVisible();
  await expect(details).toContainText("IPv4 forwarding");
  await expect(details).toContainText("nftables v1.1.3");
  await expect(details).toContainText("Control desired");
  await expect(details).toContainText("Agent desired");
  await expect(details).toContainText("Observed table");
  await details.getByRole("button", { name: "Close port-forward details" }).click();

  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await expect(editor).toBeVisible();
  await expect(editor.getByLabel("VPS")).toBeFocused();
  await expect(editor.getByLabel("Enabled")).not.toBeChecked();
  await expect(editor.locator(".portMappingPreview")).toHaveClass(/idle/);
  await expect(editor.locator(".portMappingPreview")).toContainText(
    "Enter incoming and target ports to preview the exact mappings",
  );
  await editor.getByLabel("VPS").selectOption({ index: 1 });
  await editor.getByLabel("Name", { exact: true }).fill("Internal application");
  await editor.getByRole("button", { name: "Both" }).click();
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
    page.locator(".portForwardRegistryFeedback").getByText(
      /Rule created; apply job .* queued/,
    ),
  ).toBeVisible();
  await expect(editor).toBeHidden();
  await expect(page.getByRole("table", { name: "Port-forward rules" })).toContainText(
    "Internal application",
  );

  const requests = await page.evaluate(
    () =>
      (window as unknown as {
        __vpsmanTestRequests: { portForwardRules: unknown[] };
      }).__vpsmanTestRequests.portForwardRules,
  );
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({ action: "create" });
});

test("unsupported agents allow disabled drafts but not enabled apply", async ({ page }) => {
  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await editor.getByLabel("VPS").selectOption("agent-nyc-03");
  await editor.getByLabel("Name", { exact: true }).fill("Future service");
  await editor.getByLabel("Incoming ports").fill("8443");
  await editor.getByLabel("Target ports").fill("443");
  await editor.getByLabel("Target IP or hostname").fill("10.30.0.9");
  await editor.getByLabel("Enabled").check();
  await expect(editor).toContainText(
    "Agent lacks CAP_NET_ADMIN in the host network namespace",
  );
  await expect(editor.getByRole("button", { name: "Create rule" })).toBeDisabled();

  await editor.getByLabel("Enabled").uncheck();
  await editor.getByRole("button", { name: "Create rule" }).click();
  await expect(page.getByText("Rule created")).toBeVisible();
  await expect(page.getByText("Future service", { exact: true })).toBeVisible();
});

test("never-applied disabled drafts explain and complete immediate deletion", async ({
  page,
}, testInfo) => {
  const row = page.getByRole("row", { name: /Staged SSH alternate/ });
  if (testInfo.project.name.startsWith("mobile")) {
    await row
      .getByRole("button", { name: "Expand Staged SSH alternate rule details" })
      .click();
    await page
      .getByRole("region", { name: "Details for Staged SSH alternate" })
      .getByRole("button", { name: "Delete", exact: true })
      .click();
  } else {
    await row.getByTitle("Delete rule").click();
  }

  const confirmation = page.getByLabel("Confirm delete");
  await expect(confirmation).toContainText(
    "This disabled draft has never been applied.",
  );
  await expect(confirmation).toContainText(
    "no agent cleanup or apply job is required",
  );
  await confirmation.getByRole("button", { name: "Delete rule" }).click();

  await expect(
    page.getByRole("row", { name: /Staged SSH alternate/ }),
  ).toHaveCount(0);
  await expect(
    page.locator(".portForwardRegistryFeedback"),
  ).toContainText("no host apply required");
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
  await expect(page.getByLabel("Select Public web ingress")).toBeDisabled();
  await expect(page.getByRole("button", { name: "Refresh", exact: true })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Refresh", exact: true })).toHaveAttribute(
    "title",
    "Reload latest stored desired state and agent evidence; this does not request a live agent inspection",
  );

  await page.getByText("Public web ingress", { exact: true }).click();
  const details = page.getByRole("region", { name: "Details for Public web ingress" });
  await expect(details).toBeVisible();
  if (testInfo.project.name.startsWith("mobile")) {
    await expect(
      details.getByRole("button", { name: "Edit", exact: true }),
    ).toBeDisabled();
  } else {
    await expect(
      page.getByRole("row", { name: /Public web ingress/ }).getByTitle(
        "Operator role and network:write scope required",
      ).first(),
    ).toBeDisabled();
  }
});

test("rule names enforce the API UTF-8 byte limit with an exact reason", async ({ page }) => {
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
  const row = page.getByRole("row", { name: /Public web ingress/ });
  if (testInfo.project.name.startsWith("mobile")) {
    await row
      .getByRole("button", { name: "Expand Public web ingress rule details" })
      .click();
    await page
      .getByRole("region", { name: "Details for Public web ingress" })
      .getByRole("button", { name: "Delete", exact: true })
      .click();
  } else {
    await row.getByTitle("Delete rule").click();
  }
  const confirmation = page.getByLabel("Confirm delete");
  await expect(confirmation).toContainText("Removal pending");
  await confirmation.getByRole("button", { name: "Delete rule" }).click();
  await expect(page.getByRole("row", { name: /Public web ingress/ })).toContainText(
    "removal pending",
  );
});

test("mobile port-forward workflow has no page-level horizontal overflow", async ({
  page,
}, testInfo) => {
  test.skip(!testInfo.project.name.startsWith("mobile"));
  const firstRow = page.getByRole("row", { name: /Public web ingress/ });
  await expect(firstRow.locator(".portForwardDesktopActions")).toBeHidden();
  await expect(firstRow.locator(".portForwardMobileStatus")).toContainText("enabled");
  await expect(firstRow.locator(".portForwardMobileStatus")).toContainText("applied");
  await firstRow
    .getByRole("button", { name: "Expand Public web ingress rule details" })
    .click();
  const details = page.getByRole("region", { name: "Details for Public web ingress" });
  await expect(details.getByLabel("Actions for Public web ingress")).toBeVisible();
  await details.getByRole("button", { name: "Close port-forward details" }).click();
  await page.getByRole("button", { name: "Create rule" }).click();
  const editor = page.locator(".portForwardEditor");
  await expect(editor).toBeVisible();
  const previewText = editor.locator(".portMappingPreview > span");
  await expect(previewText).toHaveAttribute(
    "title",
    "Enter incoming and target ports to preview the exact mappings",
  );
  expect(
    await previewText.evaluate((element) => element.scrollWidth - element.clientWidth),
  ).toBeLessThanOrEqual(1);
  for (const control of [
    editor.getByLabel("VPS"),
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
        textBox.top + textBox.height / 2 - (checkboxBox.top + checkboxBox.height / 2),
      ),
    };
  });
  expect(enabledGeometry?.horizontalGap ?? 0).toBeGreaterThan(0);
  expect(enabledGeometry?.verticalCenterDelta ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(2);
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow).toBeLessThanOrEqual(1);
});

test("bulk actions state their exact eligible subset", async ({ page }) => {
  await page.getByLabel("Select visible port-forward rules").check();
  const actions = page.getByLabel("Selected port-forward actions");

  await expect(actions).toContainText("4 selected");
  await expect(actions.getByRole("button", { name: "Enable 0", exact: true })).toBeDisabled();
  await expect(actions.getByRole("button", { name: "Disable 2", exact: true })).toBeEnabled();
  await expect(actions.getByRole("button", { name: "Reapply 2", exact: true })).toBeEnabled();
  await expect(actions.getByRole("button", { name: "Delete 3", exact: true })).toBeEnabled();
});

test("applied status explains evidence limits", async ({ page }) => {
  const row = page.getByRole("row", { name: /Public web ingress/ });
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
        typeof input === "string" ? input : input instanceof Request ? input.url : input.toString(),
        window.location.origin,
      );
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
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

  const isMobile = testInfo.project.name.startsWith("mobile");
  const row = page.getByRole("row", { name: /Public web ingress/ });
  let feedback = page.locator(".portForwardRegistryFeedback");
  if (isMobile) {
    await row
      .getByRole("button", { name: "Expand Public web ingress rule details" })
      .click();
    const details = page.getByRole("region", {
      name: "Details for Public web ingress",
    });
    await details.getByRole("button", { name: "Disable", exact: true }).click();
    feedback = details.locator(".portForwardDetailFeedback");
  } else {
    await row.getByTitle("Disable rule").click();
  }
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
        typeof input === "string" ? input : input instanceof Request ? input.url : input.toString(),
        window.location.origin,
      );
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
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
