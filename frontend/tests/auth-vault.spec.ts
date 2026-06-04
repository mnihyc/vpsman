import { expect, test, type Locator } from "@playwright/test";

const accessToken = "a".repeat(64);
const refreshToken = "b".repeat(64);

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

test("stores bearer session only inside encrypted WebCrypto vault", async ({ page }) => {
  await installAuthVaultApiMock(page);
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Operator access" })).toBeVisible();
  await page.getByLabel("Username").fill("vault-admin");
  await page.getByLabel("Password").fill("vault-password-123");
  await page.getByLabel("Session vault key").fill("vault-key-123456");
  await activate(page.getByRole("button", { name: "Submit login" }));

  await page.waitForFunction(() => window.localStorage.getItem("vpsman.authVault") !== null);
  await expect(page.getByRole("heading", { name: "Fleet overview" })).toBeVisible();

  const storage = await readSessionStorage(page);
  expect(storage.access).toBeNull();
  expect(storage.refresh).toBeNull();
  expect(storage.authVault).toContain('"cipher":"AES-GCM"');
  expect(storage.authVault).not.toContain(accessToken);
  expect(storage.authVault).not.toContain(refreshToken);
  expect(storage.authVault).not.toContain("vault-password-123");
  expect(storage.authVault).not.toContain("vault-key-123456");

  await page.reload();
  await expect(page.getByRole("heading", { name: "Operator access" })).toBeVisible();
  await page.getByLabel("Stored session key").fill("vault-key-123456");
  await activate(page.getByRole("button", { name: "Unlock session" }));
  await expect(page.getByRole("heading", { name: "Fleet overview" })).toBeVisible();
});

async function installAuthVaultApiMock(page: import("@playwright/test").Page) {
  await page.route("**/api/v1/auth/login", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: {
        access_token: accessToken,
        expires_in_secs: 900,
        operator: {
          id: "99999999-aaaa-4bbb-8ccc-000000000001",
          role: "admin",
          scopes: ["*"],
          totp_enabled: false,
          username: "vault-admin",
        },
        refresh_expires_in_secs: 1209600,
        refresh_token: refreshToken,
        token_type: "Bearer",
      },
    });
  });
  await page.route("**/api/v1/fleet/summary", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({ contentType: "application/json", json: { error: "missing_bearer_token" }, status: 401 });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: { connected: 1, running_jobs: 0, total: 1, warnings: 0 },
    });
  });
  await page.route("**/api/v1/agents", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({ contentType: "application/json", json: { error: "missing_bearer_token" }, status: 401 });
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
          display_name: "vault-edge-01",
          id: "vault-agent-01",
          status: "connected",
          tags: ["edge"],
        },
      ],
    });
  });
  await page.route("**/api/v1/telemetry/rollups**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({ contentType: "application/json", json: { error: "missing_bearer_token" }, status: 401 });
      return;
    }
    await route.fulfill({ contentType: "application/json", json: [] });
  });
  await page.route("**/api/v1/telemetry/network-rates**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({ contentType: "application/json", json: { error: "missing_bearer_token" }, status: 401 });
      return;
    }
    await route.fulfill({ contentType: "application/json", json: [] });
  });
  await page.route("**/api/v1/telemetry/tunnels**", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({ contentType: "application/json", json: { error: "missing_bearer_token" }, status: 401 });
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
    "agent-update-rollouts",
    "agent-update-releases",
    "process-supervisor/inventory",
    "pools",
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
        await route.fulfill({ contentType: "application/json", json: { error: "missing_bearer_token" }, status: 401 });
        return;
      }
      await route.fulfill({ contentType: "application/json", json: path === "auth/me" ? { id: "99999999-aaaa-4bbb-8ccc-000000000001", role: "admin", scopes: ["*"], totp_enabled: false, username: "vault-admin" } : [] });
    });
  }
}

function isAuthorized(request: import("@playwright/test").Request): boolean {
  return request.headers().authorization === `Bearer ${accessToken}`;
}

async function readSessionStorage(page: import("@playwright/test").Page) {
  return page.evaluate(() => ({
    access: window.localStorage.getItem("vpsman.accessToken"),
    authVault: window.localStorage.getItem("vpsman.authVault") ?? "",
    refresh: window.localStorage.getItem("vpsman.refreshToken"),
  }));
}
