import type { Page, Route } from "@playwright/test";
import type {
  AgentView,
  MonitoringShareView,
  PingTargetAssignmentView,
  PingTargetDetailView,
  PingTargetView,
} from "../../src/types";

export const screenshotPingTargetId =
  "11111111-aaaa-4111-8111-111111111111";

export async function installMonitoringManagementApiMock(page: Page) {
  const pingTargets = pingTargetFixtures();
  const targetDetails = new Map<string, PingTargetDetailView>([
    [
      screenshotPingTargetId,
      {
        assignments: [
          pingAssignment("agent-sfo-01", "edge-sfo-01", true),
          pingAssignment("agent-fra-02", "core-fra-02", false),
        ],
        target: pingTargets[0],
      },
    ],
  ]);

  await page.route(/\/api\/v1\/ping-targets(?:\/.*)?$/, async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    if (pathname === "/api/v1/ping-targets" && request.method() === "GET") {
      await fulfillJson(route, pingTargets);
      return;
    }
    const detailId = decodeURIComponent(
      pathname.slice("/api/v1/ping-targets/".length),
    );
    const detail = targetDetails.get(detailId);
    if (detail && request.method() === "GET") {
      await fulfillJson(route, detail);
      return;
    }
    await route.fallback();
  });

  await page.route(/\/api\/v1\/monitoring-shares(?:\?.*)?$/, async (route) => {
    if (route.request().method() !== "GET") {
      await route.fallback();
      return;
    }
    const url = new URL(route.request().url());
    const offset = Math.max(0, Number(url.searchParams.get("offset") ?? "0"));
    const limit = Math.max(1, Number(url.searchParams.get("limit") ?? "100"));
    await fulfillJson(route, monitoringShareFixtures().slice(offset, offset + limit));
  });
}

function pingTargetFixtures(): PingTargetView[] {
  return [
    {
      assigned_count: 2,
      created_at: "2026-07-31T08:00:00Z",
      enabled: true,
      generation: 3,
      host: "fra.example.net",
      id: screenshotPingTargetId,
      name: "Frankfurt gateway",
      port: null,
      primary_count: 1,
      probe_kind: "icmp",
      runtime_sync: {
        reason: "Every assigned VPS has confirmed Ping generation 3.",
        state: "applied",
      },
      selector_expression: "provider:alpha || country:DE",
      target_update_available: true,
      updated_at: "2026-07-31T09:00:00Z",
    },
    {
      assigned_count: 1,
      created_at: "2026-07-30T08:00:00Z",
      enabled: true,
      generation: 2,
      host: "status.example.net",
      id: "22222222-bbbb-4222-8222-222222222222",
      name: "Status endpoint",
      port: 443,
      primary_count: 0,
      probe_kind: "tcp",
      runtime_sync: {
        reason: "One assigned VPS has not confirmed Ping generation 2.",
        state: "stale",
      },
      selector_expression: "id:agent-nyc-03",
      target_update_available: false,
      updated_at: "2026-07-31T08:45:00Z",
    },
  ];
}

function pingAssignment(
  clientId: string,
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
    id: clientId,
    last_seen_at: "2026-07-31T09:00:00Z",
    status: "online",
    tags: [],
  };
  return {
    assigned_at: "2026-07-31T08:30:00Z",
    client,
    is_primary: isPrimary,
    target_id: screenshotPingTargetId,
  };
}

function monitoringShareFixtures(): MonitoringShareView[] {
  const visibility = {
    detail_history: true,
    identity_context: false,
    network: true,
    ping: true,
    resources: true,
    traffic: true,
  };
  return [
    {
      created_at: "2026-07-31T07:00:00Z",
      created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
      expires_at: "2099-08-01T07:00:00Z",
      first_visited_at: "2026-07-31T07:10:00Z",
      id: "33333333-cccc-4333-8333-333333333333",
      last_visited_at: "2026-07-31T08:30:00Z",
      name: "Customer status",
      revoked_at: null,
      selector_expression: "provider:alpha",
      status: "active",
      target_count: 2,
      updated_at: "2026-07-31T08:30:00Z",
      visibility,
      visitor_count: 4,
    },
    {
      created_at: "2026-07-20T07:00:00Z",
      created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
      expires_at: "2026-07-21T07:00:00Z",
      first_visited_at: null,
      id: "44444444-dddd-4444-8444-444444444444",
      last_visited_at: null,
      name: "Expired handoff",
      revoked_at: null,
      selector_expression: "id:agent-nyc-03",
      status: "expired",
      target_count: 1,
      updated_at: "2026-07-20T07:00:00Z",
      visibility,
      visitor_count: 0,
    },
    {
      created_at: "2026-07-25T07:00:00Z",
      created_by: "99999999-aaaa-4bbb-8ccc-000000000001",
      expires_at: "2099-08-01T07:00:00Z",
      first_visited_at: "2026-07-25T08:00:00Z",
      id: "55555555-eeee-4555-8555-555555555555",
      last_visited_at: "2026-07-25T08:00:00Z",
      name: "Revoked wall",
      revoked_at: "2026-07-26T07:00:00Z",
      selector_expression: "*",
      status: "revoked",
      target_count: 3,
      updated_at: "2026-07-26T07:00:00Z",
      visibility,
      visitor_count: 1,
    },
  ];
}

async function fulfillJson(route: Route, body: unknown) {
  await route.fulfill({
    body: JSON.stringify(body),
    contentType: "application/json",
    status: 200,
  });
}
