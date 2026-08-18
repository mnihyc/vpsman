import { expect, test, type Locator, type Page } from "@playwright/test";
import { expectPrivilegeVerifiedForViewport } from "./support/consoleNavigation";

const accessToken = "a".repeat(64);
const refreshToken = "b".repeat(64);
const rotatedAccessToken = "c".repeat(64);
const rotatedRefreshToken = "d".repeat(64);
const preferences = {
  byte_unit_display_mode: "decimal",
  bulk_output_compare_mode: "binary",
  dashboard_curve_exclusions: [],
  dashboard_network_top_limit: 8,
  dashboard_resource_top_limit: 8,
  fleet_location_display_mode: "country_only",
  language: "en",
  show_country_flags: true,
  sidebar_subpanel_default: "active",
  timezone: null,
  vps_name_display_mode: "name_id_suffix",
};

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function expectAuthenticatedConsoleShell(
  page: Page,
  expected = { heading: "Home", mobileRoute: "Home::overview" },
) {
  const desktopNav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  const mobileRoute = page.getByRole("combobox", {
    name: "Console page",
    exact: true,
  });
  await expect
    .poll(
      async () =>
        (await desktopNav.isVisible().catch(() => false)) ||
        (await mobileRoute.isVisible().catch(() => false)),
    )
    .toBe(true);
  if (await mobileRoute.isVisible().catch(() => false)) {
    await expect(mobileRoute).toHaveValue(expected.mobileRoute);
  }
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: expected.heading,
      exact: true,
    }),
  ).toBeVisible();
}

async function expectOperatorAccessShell(page: Page) {
  const heading = page.getByRole("heading", { name: "Sign in" });
  const authForm = page.getByLabel("Operator authentication");

  for (let attempt = 0; attempt < 2; attempt += 1) {
    const rendered = await heading
      .waitFor({ state: "visible", timeout: attempt === 0 ? 8000 : 15000 })
      .then(() => true)
      .catch(() => false);
    if (rendered) {
      await expect(authForm).toBeVisible();
      await expect(
        page.getByRole("navigation", { name: "Primary console navigation" }),
      ).toHaveCount(0);
      return;
    }

    const hasMountedRoot = await page
      .locator("#root")
      .evaluate((root) => (root.textContent ?? "").trim().length > 0)
      .catch(() => false);
    if (hasMountedRoot || attempt > 0) {
      break;
    }

    await page.reload({ waitUntil: "domcontentloaded" });
  }

  await expect(heading).toBeVisible({ timeout: 15000 });
  await expect(authForm).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "Primary console navigation" }),
  ).toHaveCount(0);
}

test("keeps ordinary bearer login authenticated across browser reload", async ({
  page,
}) => {
  await installAuthSessionApiMock(page);
  await page.goto("/");

  await expectOperatorAccessShell(page);
  await expect(
    page.getByText("Sign in with an existing operator account."),
  ).toBeVisible();
  await page.getByLabel("Username").fill("session-admin");
  const password = page.getByLabel("Password");
  await password.fill("session-password-123");
  await expect(password).not.toHaveAttribute("title", /session-password-123/);
  expect(
    await page
      .locator("[title]")
      .evaluateAll((elements) =>
        elements.some((element) =>
          (element.getAttribute("title") ?? "").includes(
            "session-password-123",
          ),
        ),
      ),
  ).toBe(false);
  await activate(page.getByRole("button", { name: "Sign in" }));

  await expectAuthenticatedConsoleShell(page);
  const storage = await readSessionStorage(page);
  expect(storage.access).toBe(accessToken);
  expect(storage.refresh).toBe(refreshToken);

  await page.reload();
  await expectAuthenticatedConsoleShell(page);
  await expect(
    page.getByRole("heading", { name: "Sign in", exact: true }),
  ).toHaveCount(0);
});

test("refreshes a stored session before returning to sign in", async ({
  page,
}) => {
  await page.addInitScript(
    ({ access, refresh }) => {
      window.localStorage.setItem("vpsman.accessToken", access);
      window.localStorage.setItem("vpsman.refreshToken", refresh);
    },
    { access: "expired".repeat(10).slice(0, 64), refresh: refreshToken },
  );
  await installAuthSessionApiMock(page);

  await page.goto("/");
  await expectAuthenticatedConsoleShell(page);

  const storage = await readSessionStorage(page);
  expect(storage.access).toBe(rotatedAccessToken);
  expect(storage.refresh).toBe(rotatedRefreshToken);
  await expect(
    page.getByRole("heading", { name: "Sign in", exact: true }),
  ).toHaveCount(0);
});

test("keeps a stored session visible and retryable when refresh is temporarily unavailable", async ({
  page,
}) => {
  const expiredAccessToken = "expired".repeat(10).slice(0, 64);
  let refreshAvailable = false;
  await page.addInitScript(
    ({ access, refresh }) => {
      window.localStorage.setItem("vpsman.accessToken", access);
      window.localStorage.setItem("vpsman.refreshToken", refresh);
    },
    { access: expiredAccessToken, refresh: refreshToken },
  );
  await installAuthSessionApiMock(page);
  await page.unroute("**/api/v1/auth/refresh");
  await page.route("**/api/v1/auth/refresh", async (route) => {
    if (!refreshAvailable) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "refresh_service_unavailable" },
        status: 503,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: {
        access_token: rotatedAccessToken,
        expires_in_secs: 900,
        operator: {
          id: "99999999-aaaa-4bbb-8ccc-000000000001",
          preferences,
          role: "admin",
          scopes: ["*"],
          totp_enabled: false,
          username: "session-admin",
        },
        refresh_expires_in_secs: 1209600,
        refresh_token: rotatedRefreshToken,
        token_type: "Bearer",
      },
    });
  });

  await page.goto("/");
  const refreshNotice = page.getByRole("alert").filter({
    hasText: "Session refresh unavailable",
  });
  await expect(refreshNotice).toBeVisible();
  expect(await readSessionStorage(page)).toEqual({
    access: expiredAccessToken,
    refresh: refreshToken,
  });

  refreshAvailable = true;
  await activate(refreshNotice.getByRole("button", { name: "Retry" }));
  await expectAuthenticatedConsoleShell(page);
  await expect(refreshNotice).toHaveCount(0);
  expect(await readSessionStorage(page)).toEqual({
    access: rotatedAccessToken,
    refresh: rotatedRefreshToken,
  });
});

test("sign out revokes the bearer session and clears privilege before reauthentication", async ({
  page,
}) => {
  await installAuthSessionApiMock(page);
  await page.goto("/");

  await page.getByLabel("Username").fill("session-admin");
  await page.getByLabel("Password").fill("session-password-123");
  await activate(page.getByRole("button", { name: "Sign in" }));
  await expectAuthenticatedConsoleShell(page);

  await activate(
    page
      .locator(".topbar")
      .getByRole("button", { name: "Open privilege unlock" }),
  );
  const unlock = page.getByRole("dialog", { name: "Unlock privilege" });
  await unlock.getByLabel(/super password/i).fill("local-super-password");
  await unlock
    .getByLabel(/privilege salt/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    unlock
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: "Unlock", exact: true }),
  );
  await expectPrivilegeVerifiedForViewport(page);

  await activate(
    page.locator(".topbar").getByRole("button", { name: "Open sessions" }),
  );
  await expect(page).toHaveURL(/#\/audit\/sessions$/);
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Session evidence",
      exact: true,
    }),
  ).toBeVisible();
  expect(await readSessionStorage(page)).toEqual({
    access: accessToken,
    refresh: refreshToken,
  });

  const logoutRequest = page.waitForRequest("**/api/v1/auth/logout");
  await activate(
    page
      .locator(".auditSessionEvidencePanel")
      .getByRole("button", { name: "Sign out", exact: true }),
  );
  const request = await logoutRequest;
  expect(request.method()).toBe("POST");
  expect(request.headers().authorization).toBe(`Bearer ${accessToken}`);
  await expectOperatorAccessShell(page);
  expect(await readSessionStorage(page)).toEqual({
    access: null,
    refresh: null,
  });

  await page.getByLabel("Username").fill("session-admin");
  await page.getByLabel("Password").fill("session-password-123");
  await activate(page.getByRole("button", { name: "Sign in" }));
  await expectAuthenticatedConsoleShell(page, {
    heading: "Session evidence",
    mobileRoute: "Audit::sessions",
  });
  await expect(
    page
      .locator(".topbar")
      .getByRole("button", { name: "Open privilege unlock" }),
  ).toBeVisible();
  await expect(
    page.getByLabel("Privilege verified for this browser"),
  ).toHaveCount(0);
});

test("sign out clears local authentication when server revocation fails", async ({
  page,
}) => {
  await installAuthSessionApiMock(page);
  await page.unroute("**/api/v1/auth/logout");
  await page.route("**/api/v1/auth/logout", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: { error: "operator_store_unavailable" },
      status: 503,
    });
  });
  await page.goto("/");

  await page.getByLabel("Username").fill("session-admin");
  await page.getByLabel("Password").fill("session-password-123");
  await activate(page.getByRole("button", { name: "Sign in" }));
  await expectAuthenticatedConsoleShell(page);
  await activate(
    page.locator(".topbar").getByRole("button", { name: "Open sessions" }),
  );
  await expect(page).toHaveURL(/#\/audit\/sessions$/);
  await activate(
    page
      .locator(".auditSessionEvidencePanel")
      .getByRole("button", { name: "Sign out", exact: true }),
  );

  await expectOperatorAccessShell(page);
  await expect(
    page.getByText(
      /Signed out locally, but the server could not revoke this session.*Audit > Sessions/,
    ),
  ).toBeVisible();
  expect(await readSessionStorage(page)).toEqual({
    access: null,
    refresh: null,
  });
});

async function installAuthSessionApiMock(
  page: import("@playwright/test").Page,
) {
  await page.route("**/api/v1/auth/bootstrap-status", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: { bootstrap_required: false },
    });
  });
  await page.route("**/api/v1/auth/login", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: {
        access_token: accessToken,
        expires_in_secs: 900,
        operator: {
          id: "99999999-aaaa-4bbb-8ccc-000000000001",
          preferences,
          role: "admin",
          scopes: ["*"],
          totp_enabled: false,
          username: "session-admin",
        },
        refresh_expires_in_secs: 1209600,
        refresh_token: refreshToken,
        token_type: "Bearer",
      },
    });
  });
  await page.route("**/api/v1/auth/logout", async (route) => {
    await route.fulfill({ status: 204 });
  });
  await page.route("**/api/v1/auth/privilege/verify", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: { verified: true },
    });
  });
  await page.route("**/api/v1/auth/refresh", async (route) => {
    const body = (await route.request().postDataJSON()) as {
      refresh_token?: string;
    };
    const validRefreshTokens = new Set([refreshToken, rotatedRefreshToken]);
    if (!body.refresh_token || !validRefreshTokens.has(body.refresh_token)) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "invalid_refresh_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: {
        access_token: rotatedAccessToken,
        expires_in_secs: 900,
        operator: {
          id: "99999999-aaaa-4bbb-8ccc-000000000001",
          preferences,
          role: "admin",
          scopes: ["*"],
          totp_enabled: false,
          username: "session-admin",
        },
        refresh_expires_in_secs: 1209600,
        refresh_token: rotatedRefreshToken,
        token_type: "Bearer",
      },
    });
  });
  await page.route("**/api/v1/home/snapshot**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    const available = <T>(data: T) => ({ data, error: null });
    const unavailable = (error: string) => ({ data: null, error });
    await route.fulfill({
      contentType: "application/json",
      json: {
        generated_at: "2026-06-05T20:44:58Z",
        operator: {
          id: "99999999-aaaa-4bbb-8ccc-000000000001",
          preferences,
          role: "admin",
          scopes: ["*"],
          totp_enabled: false,
          username: "session-admin",
        },
        summary: available({
          never: 0,
          offline: 0,
          online: 1,
          revoked: 0,
          running_jobs: 0,
          stale: 0,
          total: 1,
          unknown: 0,
          warnings: 0,
        }),
        agents: available([
          {
            capabilities: {
              can_apply_process_limits: true,
              can_attempt_privileged_ops: true,
              can_manage_runtime_tunnels: true,
              effective_uid: 0,
              privilege_mode: "root",
              unprivileged_hint: null,
            },
            display_name: "session-edge-01",
            id: "session-agent-01",
            status: "online",
            tags: ["edge"],
          },
        ]),
        telemetry_rollups: available([]),
        telemetry_network_rates: available([]),
        fleet_alerts: available([]),
        monitoring_cards: available([]),
        jobs: available([]),
        file_transfers: available([]),
        terminal_sessions: available([]),
        backups: available([]),
        backup_artifacts: available([]),
        audit: available([]),
        schedules: available([]),
        system_dashboard: unavailable("auth_fixture_source_not_projected"),
        dashboard_overview: unavailable("auth_fixture_source_not_projected"),
      },
    });
  });
  await page.route("**/api/v1/fleet/snapshot**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    const mode = new URL(route.request().url()).searchParams.get("mode");
    const available = <T>(data: T) => ({ data, error: null });
    const response: Record<string, unknown> = {
      agents: available([
        {
          capabilities: {
            can_apply_process_limits: true,
            can_attempt_privileged_ops: true,
            can_manage_runtime_tunnels: true,
            effective_uid: 0,
            privilege_mode: "root",
            unprivileged_hint: null,
          },
          display_name: "session-edge-01",
          id: "session-agent-01",
          status: "online",
          tags: ["edge"],
        },
      ]),
      generated_at: "2026-06-05T20:44:58Z",
      mode,
      summary: available({
        never: 0,
        offline: 0,
        online: 1,
        revoked: 0,
        running_jobs: 0,
        stale: 0,
        total: 1,
        unknown: 0,
        warnings: 0,
      }),
      telemetry_network_rates: available([]),
      telemetry_rollups: available([]),
      telemetry_tunnels: available([]),
      telemetry_uptimes: available([]),
    };
    if (mode === "full") {
      for (const key of [
        "fleet_alert_notification_channels",
        "fleet_alert_notifications",
        "fleet_alert_policies",
        "fleet_alert_states",
        "fleet_alerts",
        "current_policy_alerts",
        "policy_alerts",
        "traffic_accounting",
        "vps_rule_values",
        "webhook_rule_deliveries",
        "webhook_rules",
      ]) {
        response[key] = available([]);
      }
      response.current_policy_alerts_truncated = false;
    }
    await route.fulfill({ contentType: "application/json", json: response });
  });
  await page.route("**/api/v1/fleet/summary", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: {
        never: 0,
        online: 1,
        offline: 0,
        revoked: 0,
        stale: 0,
        running_jobs: 0,
        total: 1,
        unknown: 0,
        warnings: 0,
      },
    });
  });
  await page.route("**/api/v1/dashboard/overview**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: {
        available_filters: {
          countries: [],
          group_by_options: [
            { description: "All labels", label: "Labels", value: "labels" },
          ],
          providers: [],
          windows: [{ label: "1 day", seconds: 86400, value: "1d" }],
        },
        drilldowns: [
          {
            label: "Open fleet instances",
            query: null,
            subpage: "instances",
            view: "Fleet",
          },
        ],
        generated_at: "2026-06-05T20:44:58Z",
        group_by: "labels",
        label_clusters: [],
        network: { points: [], rx_bps: 0, top_clients: [], tx_bps: 0 },
        operations: {
          active_alerts: 0,
          backup_completed: 0,
          backup_failed: 0,
          backup_pending: 0,
          critical_alerts: 0,
          degraded_agents: [],
          recent_alerts: [],
          running_jobs: 0,
          stale_agents: 0,
          warning_alerts: 0,
        },
        resources: {
          cpu_load_avg: null,
          cpu_load_max: null,
          disk_free_ratio: null,
          memory_used_ratio: null,
          sampled_clients: 0,
        },
        scope: {
          kind: "all",
          label: "All VPS",
          matched_clients: 1,
          query: null,
          value: null,
        },
        summary: {
          online: 1,
          offline: 0,
          revoked: 0,
          stale: 0,
          running_jobs: 0,
          running_jobs_truncated: false,
          total: 1,
          warnings: 0,
          warnings_truncated: false,
        },
        time_range: {
          end_at: "2026-06-05T20:44:58Z",
          end_unix: 1780692298,
          mode: "window",
          start_at: "2026-06-04T20:44:58Z",
          start_unix: 1780605898,
          window: "1d",
        },
        window: "1d",
      },
    });
  });
  await page.route("**/api/v1/agents", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: [
        {
          capabilities: {
            can_apply_process_limits: true,
            can_attempt_privileged_ops: true,
            can_manage_runtime_tunnels: true,
            effective_uid: 0,
            privilege_mode: "root",
            unprivileged_hint: null,
          },
          display_name: "session-edge-01",
          id: "session-agent-01",
          status: "online",
          tags: ["edge"],
        },
      ],
    });
  });
  await page.route("**/api/v1/telemetry/rollups**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({ contentType: "application/json", json: [] });
  });
  await page.route("**/api/v1/telemetry/network-rates**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({ contentType: "application/json", json: [] });
  });
  await page.route("**/api/v1/telemetry/tunnels**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "missing_bearer_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({ contentType: "application/json", json: [] });
  });
  for (const path of [
    "auth/me",
    "fleet-alerts",
    "fleet-alert-states",
    "fleet-alert-policies",
    "fleet-alert-notification-channels",
    "fleet-alert-notifications",
    "operators",
    "operator-sessions",
    "gateway-sessions",
    "jobs",
    "agent-update-releases",
    "process-supervisor/inventory",
    "tags",
    "schedules",
    "backups",
    "backup-policies",
    "backup-artifacts",
    "restore-plans",
    "migration-links",
    "audit",
    "history/retention-policies",
    "history/export",
    "network/observations",
    "network/observation-trends",
    "network/ospf-recommendations",
    "network/ospf-update-plans",
  ]) {
    await page.route(`**/api/v1/${path}**`, async (route) => {
      if (!isAuthorized(route.request())) {
        await route.fulfill({
          contentType: "application/json",
          json: { error: "missing_bearer_token" },
          status: 401,
        });
        return;
      }
      await route.fulfill({
        contentType: "application/json",
        json:
          path === "auth/me"
            ? {
                id: "99999999-aaaa-4bbb-8ccc-000000000001",
                preferences,
                role: "admin",
                scopes: ["*"],
                totp_enabled: false,
                username: "session-admin",
              }
            : [],
      });
    });
  }
}

function isAuthorized(request: import("@playwright/test").Request): boolean {
  return (
    request.headers().authorization === `Bearer ${accessToken}` ||
    request.headers().authorization === `Bearer ${rotatedAccessToken}`
  );
}

async function readSessionStorage(page: import("@playwright/test").Page) {
  return page.evaluate(() => ({
    access: window.localStorage.getItem("vpsman.accessToken"),
    refresh: window.localStorage.getItem("vpsman.refreshToken"),
  }));
}
