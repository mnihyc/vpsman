import { expect, test, type Page, type Route } from "@playwright/test";
import type {
  AgentView,
  PingTargetAssignmentView,
  PingTargetDetailView,
  PingTargetMutationResponse,
  PingTargetView,
} from "../src/types";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

const driftedTargetId = "11111111-aaaa-4111-8111-111111111111";
const stableTargetId = "22222222-bbbb-4222-8222-222222222222";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
  await installPingTargetApiMock(page);
});

test("Ping targets keep actions in the table header and expose frozen assignment evidence", async ({
  page,
}) => {
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Observability", "Ping targets");

  const grid = page.getByLabel("Ping targets data grid");
  await expect(grid).toBeVisible();
  await expect(
    grid.getByRole("columnheader", { name: /^Actions?$/i }),
  ).toHaveCount(0);
  await expect(grid.getByText("Applied", { exact: true })).toBeVisible();
  await expect(grid.getByText("Stale", { exact: true })).toBeVisible();
  await expect(
    grid.locator(
      '[title="Every assigned VPS has confirmed Ping generation 3."]',
    ).first(),
  ).toBeVisible();
  await expect(
    grid.locator(
      '[title="One assigned VPS has not confirmed Ping generation 2."]',
    ).first(),
  ).toBeVisible();

  const actions = grid.getByRole("button", { name: "Actions", exact: true });
  await expect(actions).toBeDisabled();

  await grid
    .getByLabel(`Select Ping targets row ${stableTargetId}`)
    .check();
  await actions.click();
  await expect(
    page.getByRole("menuitem", { name: "Update targets", exact: true }),
  ).toHaveAttribute("aria-disabled", "true");
  await page.keyboard.press("Escape");

  await grid
    .getByLabel(`Select Ping targets row ${stableTargetId}`)
    .uncheck();
  await grid
    .getByLabel(`Select Ping targets row ${driftedTargetId}`)
    .check();
  await actions.click();
  await expect(
    page.getByRole("menuitem", { name: "Update targets", exact: true }),
  ).toBeEnabled();
  await page.keyboard.press("Escape");

  const mobileCard = grid.getByLabel(
    `Ping targets mobile card ${driftedTargetId}`,
  );
  if ((await mobileCard.count()) > 0) {
    await expect(mobileCard.locator(".gridMobileActions")).toHaveCount(0);
    await mobileCard.click();
  } else {
    await grid
      .getByRole("button", {
        name: `Expand Ping targets row ${driftedTargetId}`,
      })
      .click();
  }

  const assignments = grid.getByLabel(
    "Frankfurt gateway assignments data grid",
  );
  await expect(assignments).toBeVisible();
  await expect(assignments.locator("..")).toContainText(
    "2 assigned · 1 primary · selector provider:alpha || country:DE",
  );
  if ((await mobileCard.count()) > 0) {
    const assignmentCard = assignments.getByLabel(
      "Frankfurt gateway assignments mobile card agent-fra-02",
    );
    await expect(assignmentCard.locator(".gridMobileActions")).toHaveCount(0);
    const assignmentCheckbox = assignmentCard.getByRole("checkbox");
    const assignmentCheckboxBounds = await assignmentCheckbox.boundingBox();
    expect(assignmentCheckboxBounds).not.toBeNull();
    expect(assignmentCheckboxBounds!.width).toBeLessThanOrEqual(20);
    expect(assignmentCheckboxBounds!.height).toBeLessThanOrEqual(20);
  }
  await assignments
    .getByLabel("Select Frankfurt gateway assignments row agent-fra-02")
    .check();
  await assignments
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  const makePrimary = page.getByRole("menuitem", {
    name: "Make primary",
    exact: true,
  });
  await expect(makePrimary).toBeEnabled();
  await makePrimary.click();
  await expect(
    page.getByText(
      "Frankfurt gateway is now the primary Ping target for 1 VPS.",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(assignments.getByText("Primary", { exact: true })).toHaveCount(2);
});

async function installPingTargetApiMock(page: Page) {
  let primaryApplied = false;
  const assignments = (): PingTargetAssignmentView[] => [
    assignmentFixture("agent-sfo-01", "edge-sfo-01", true),
    assignmentFixture("agent-fra-02", "core-fra-02", primaryApplied),
  ];
  const targets = (): PingTargetView[] => [
    targetFixture({
      assignedCount: 2,
      generation: 3,
      id: driftedTargetId,
      name: "Frankfurt gateway",
      primaryCount: primaryApplied ? 2 : 1,
      runtimeReason: "Every assigned VPS has confirmed Ping generation 3.",
      runtimeState: "applied",
      selector: "provider:alpha || country:DE",
      targetUpdateAvailable: true,
    }),
    targetFixture({
      assignedCount: 1,
      generation: 2,
      id: stableTargetId,
      name: "Status endpoint",
      primaryCount: 0,
      runtimeReason: "One assigned VPS has not confirmed Ping generation 2.",
      runtimeState: "stale",
      selector: "id:agent-nyc-03",
      targetUpdateAvailable: false,
    }),
  ];

  await page.route(/\/api\/v1\/monitoring\/cards(?:\?.*)?$/, async (route) => {
    await json(route, {
      items: [],
      limit: 1_000,
      next_offset: null,
      offset: 0,
      total: 0,
    });
  });

  await page.route(/\/api\/v1\/ping-targets(?:\/.*)?$/, async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    if (pathname === "/api/v1/ping-targets" && request.method() === "GET") {
      await json(route, targets());
      return;
    }
    if (
      pathname === `/api/v1/ping-targets/${driftedTargetId}` &&
      request.method() === "GET"
    ) {
      await json(route, driftedDetail(targets()[0], assignments()));
      return;
    }
    if (
      pathname === `/api/v1/ping-targets/${driftedTargetId}/primary` &&
      request.method() === "POST"
    ) {
      primaryApplied = true;
      const response: PingTargetMutationResponse = {
        runtime_sync: [],
        target: driftedDetail(targets()[0], assignments()),
      };
      await json(route, response);
      return;
    }
    await route.fallback();
  });
}

function targetFixture({
  assignedCount,
  generation,
  id,
  name,
  primaryCount,
  runtimeReason,
  runtimeState,
  selector,
  targetUpdateAvailable,
}: {
  assignedCount: number;
  generation: number;
  id: string;
  name: string;
  primaryCount: number;
  runtimeReason: string;
  runtimeState: string;
  selector: string;
  targetUpdateAvailable: boolean;
}): PingTargetView {
  return {
    assigned_count: assignedCount,
    created_at: "2026-07-31T08:00:00Z",
    enabled: true,
    generation,
    host: name === "Frankfurt gateway" ? "fra.example.net" : "status.example.net",
    id,
    name,
    port: null,
    primary_count: primaryCount,
    probe_kind: "icmp",
    runtime_sync: {
      reason: runtimeReason,
      state: runtimeState,
    },
    selector_expression: selector,
    target_update_available: targetUpdateAvailable,
    updated_at: "2026-07-31T09:00:00Z",
  };
}

function assignmentFixture(
  id: string,
  displayName: string,
  isPrimary: boolean,
): PingTargetAssignmentView {
  const client: AgentView = {
    capabilities: {
      can_apply_process_limits: true,
      can_attempt_privileged_ops: true,
      can_manage_runtime_tunnels: true,
      max_job_timeout_secs: 3_600,
      privilege_mode: "root",
    },
    display_name: displayName,
    id,
    status: "online",
    tags: [],
  };
  return {
    assigned_at: "2026-07-31T08:30:00Z",
    client,
    is_primary: isPrimary,
    target_id: driftedTargetId,
  };
}

function driftedDetail(
  target: PingTargetView,
  assignments: PingTargetAssignmentView[],
): PingTargetDetailView {
  return { assignments, target };
}

async function json(route: Route, body: unknown) {
  await route.fulfill({
    body: JSON.stringify(body),
    contentType: "application/json",
    status: 200,
  });
}
