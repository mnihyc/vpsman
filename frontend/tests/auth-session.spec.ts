import { expect, test, type BrowserContext, type Locator, type Page } from "@playwright/test";
import { expectPrivilegeVerifiedForViewport } from "./support/consoleNavigation";
import { readCanonicalAuthSession } from "./support/authSessionStorage";
import { NETWORK_OBSERVATION_FIELDS } from "../src/networkEvidence";

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

test("shares one single-use renewal across two tabs and does not renew on tab switches", async ({ page, context }) => {
  // Headless Chrome does not consistently hide background pages. Drive the
  // browser visibility signal deterministically; all resulting reads and
  // renewal requests still originate from the normal application handlers.
  await context.addInitScript(() => {
    let hidden = false;
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => hidden ? "hidden" : "visible",
    });
    (window as typeof window & { __authTestSetHidden: (value: boolean) => void }).__authTestSetHidden = (value) => {
      if (hidden === value) return;
      hidden = value;
      document.dispatchEvent(new Event("visibilitychange"));
    };
  });
  const setHidden = (tab: Page, hidden: boolean) => tab.evaluate((value) => {
    (window as typeof window & { __authTestSetHidden: (value: boolean) => void }).__authTestSetHidden(value);
  }, hidden);
  const authMock = await installAuthSessionApiMock(context);
  let releaseRefresh!: () => void;
  const refreshGate = new Promise<void>((resolve) => { releaseRefresh = resolve; });
  const refreshTokens: string[] = [];
  await context.route("**/api/v1/auth/refresh", async (route) => {
    const token = route.request().postDataJSON().refresh_token as string;
    refreshTokens.push(token);
    await refreshGate;
    const auth = authMock.rotate(token);
    await route.fulfill({ status: auth ? 200 : 401, json: auth ?? { error: "invalid_refresh_token" } });
  });
  const challengedTabs = new Set<Page>();
  const recoveredTabs = new Set<Page>();
  let homeReads = 0;
  context.on("request", request => {
    if (new URL(request.url()).pathname === "/api/v1/home/snapshot") homeReads++;
  });
  context.on("response", (response) => {
    const request = response.request();
    if (request.method() !== "GET" || !new URL(request.url()).pathname.startsWith("/api/")) return;
    if (response.status() === 401) challengedTabs.add(request.frame().page());
    if (response.status() === 200 && request.headers().authorization === `Bearer ${rotatedAccessToken}`) {
      recoveredTabs.add(request.frame().page());
    }
  });

  await page.goto("/");
  await page.getByLabel("Username").fill("session-admin");
  await page.getByLabel("Password").fill("session-password-123");
  const firstSnapshot = page.waitForResponse("**/api/v1/home/snapshot**");
  await activate(page.getByRole("button", { name: "Sign in" }));
  await firstSnapshot;
  await expectAuthenticatedConsoleShell(page);
  await expect(page.getByTitle("Refresh dashboard telemetry", { exact: true })).toBeEnabled();
  const originalEpoch = await page.evaluate(() => JSON.parse(localStorage.getItem("vpsman.authSession.v1")!).epoch);
  const secondTab = await context.newPage();
  const secondSnapshot = secondTab.waitForResponse("**/api/v1/home/snapshot**");
  await secondTab.goto("/");
  await secondSnapshot;
  await expectAuthenticatedConsoleShell(secondTab);
  await expect(secondTab.getByTitle("Refresh dashboard telemetry", { exact: true })).toBeEnabled();
  const initialHomeReads = homeReads;
  await Promise.all([setHidden(page, true), setHidden(secondTab, true)]);
  authMock.expireAccess();
  await Promise.all([setHidden(page, false), setHidden(secondTab, false)]);
  try {
    await expect.poll(() => challengedTabs.size).toBe(2);
    await expect.poll(() => refreshTokens.length).toBe(1);
  } finally {
    releaseRefresh();
  }
  await expect.poll(() => recoveredTabs.size).toBe(2);
  expect(refreshTokens).toEqual([refreshToken]);
  expect(homeReads).toBe(initialHomeReads);
  for (const tab of [page, secondTab]) {
    await expectAuthenticatedConsoleShell(tab);
    expect(await readSessionStorage(tab)).toEqual({ access: rotatedAccessToken, refresh: rotatedRefreshToken });
    expect(await tab.evaluate(() => JSON.parse(localStorage.getItem("vpsman.authSession.v1")!).epoch)).toBe(originalEpoch);
  }

  // Resume each tab after rotation and wait for its real catch-up GET. Merely
  // observing shared storage would not prove that its transport adopted it.
  for (const tab of [page, secondTab, page]) {
    const other = tab === page ? secondTab : page;
    await Promise.all([setHidden(other, true), setHidden(tab, true)]);
    const resumedRead = tab.waitForResponse((response) =>
      new URL(response.url()).pathname === "/api/v1/fleet/snapshot" &&
      response.status() === 200 &&
      response.request().headers().authorization === `Bearer ${rotatedAccessToken}`,
    );
    await tab.bringToFront();
    await setHidden(tab, false);
    await resumedRead;
    await expectAuthenticatedConsoleShell(tab);
    expect(refreshTokens).toEqual([refreshToken]);
  }
  await secondTab.close();
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

  await expect.poll(() => readSessionStorage(page)).toEqual({
    access: rotatedAccessToken,
    refresh: rotatedRefreshToken,
  });
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
  const authMock = await installAuthSessionApiMock(page);
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
    const auth = authMock.rotate(route.request().postDataJSON().refresh_token);
    await route.fulfill({
      contentType: "application/json",
      json: auth ?? { error: "invalid_refresh_token" },
      status: auth ? 200 : 401,
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

async function signInForRecovery(page: Page) {
  await page.goto("/");
  await page.getByLabel("Username").fill("session-admin");
  await page.getByLabel("Password").fill("session-password-123");
  await activate(page.getByRole("button", { name: "Sign in", exact: true }));
  await expectAuthenticatedConsoleShell(page);
}

async function savePrivilegeGrant(page: Page) {
  await page.evaluate(() => localStorage.setItem("vpsman.privilegeGrant", JSON.stringify({
    version: 1,
    operatorId: "99999999-aaaa-4bbb-8ccc-000000000001",
    material: { superKeyHex: "11".repeat(32) },
  })));
}

async function openRecoveryPeer(context: BrowserContext) {
  const peer = await context.newPage();
  await peer.route("**/__late-auth-peer", route => route.fulfill({
    contentType: "text/html", body: "<!doctype html><title>Authentication peer</title>",
  }));
  await peer.goto("/__late-auth-peer");
  await peer.evaluate(async () => {
    const path = "/src/authSession.ts";
    const auth = await import(path);
    await auth.initializeAuthSession();
  });
  return peer;
}

async function resumeSessionPage(page: Page, trigger: string) {
  if (trigger === "reload") {
    await page.reload();
  } else {
    await page.evaluate(trigger => {
      if (trigger === "visible") {
        Object.defineProperty(document, "hidden", { configurable: true, value: true });
        document.dispatchEvent(new Event("visibilitychange"));
        Object.defineProperty(document, "hidden", { configurable: true, value: false });
        document.dispatchEvent(new Event("visibilitychange"));
      } else if (trigger === "pageshow") {
        window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
      } else {
        window.dispatchEvent(new Event("online"));
      }
    }, trigger);
  }
}

test("restores saved privilege after expired canonical credentials without opening Unlock", async ({ page }, testInfo) => {
  const authMock = await installAuthSessionApiMock(page);
  await signInForRecovery(page);
  const initial = await readCanonicalAuthSession(page);
  await savePrivilegeGrant(page);
  let releaseRefresh!: () => void;
  const verificationChallenged = new Promise<void>(resolve => { releaseRefresh = resolve; });
  let refreshes = 0;
  const assertions: Array<{ nonce_hex: string }> = [];
  await page.route("**/api/v1/auth/privilege/verify", async route => {
    assertions.push(route.request().postDataJSON().privilege_assertion);
    if (route.request().headers().authorization === `Bearer ${accessToken}`) {
      await route.fulfill({ status: 401, json: { error: "invalid_operator_session" } });
      releaseRefresh();
    } else {
      await route.fallback();
    }
  });
  await page.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    await verificationChallenged;
    const auth = authMock.rotate(route.request().postDataJSON().refresh_token);
    await route.fulfill({ status: auth ? 200 : 401, json: auth ?? { error: "invalid_refresh_token" } });
  });
  authMock.expireAccess();
  try {
    await page.reload();
    await expectPrivilegeVerifiedForViewport(page);
    await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(0);
    expect(refreshes).toBe(1);
    expect(assertions).toHaveLength(2);
    expect(assertions[0].nonce_hex).toBeTruthy();
    expect(assertions[1].nonce_hex).not.toBe(assertions[0].nonce_hex);
    expect((await readCanonicalAuthSession(page))?.epoch).toBe(initial?.epoch);
    await page.screenshot({ path: testInfo.outputPath("privilege-restored.png") });
  } finally {
    releaseRefresh();
  }
});

test("retains pending privilege restoration while renewal is unavailable and resumes on reconnect", async ({ page }, testInfo) => {
  const authMock = await installAuthSessionApiMock(page);
  let snapshots = 0;
  page.on("response", response => {
    if (new URL(response.url()).pathname === "/api/v1/home/snapshot" && response.status() === 200) snapshots++;
  });
  await signInForRecovery(page);
  await savePrivilegeGrant(page);
  let available = false;
  let refreshes = 0;
  let verifications = 0;
  await page.route("**/api/v1/auth/privilege/verify", async route => {
    verifications++;
    await route.fallback();
  });
  await page.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    const auth = available ? authMock.rotate(route.request().postDataJSON().refresh_token) : null;
    await route.fulfill({
      status: available ? (auth ? 200 : 401) : 503,
      json: auth ?? { error: available ? "invalid_refresh_token" : "refresh_service_unavailable" },
    });
  });
  authMock.expireAccess();
  await page.reload();
  const notice = page.getByRole("alert").filter({ hasText: "Session refresh unavailable" });
  await expect(notice).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(0);
  expect(await page.evaluate(() => Boolean(localStorage.getItem("vpsman.privilegeGrant")))).toBe(true);
  expect(refreshes).toBe(1);
  available = true;
  await resumeSessionPage(page, "online");
  await expect(notice).toHaveCount(0);
  await expectPrivilegeVerifiedForViewport(page);
  await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(0);
  expect(refreshes).toBe(2);
  expect(verifications).toBeGreaterThanOrEqual(1);
  await expect.poll(() => snapshots).toBe(2);
  await expect(page.locator(".shell")).not.toContainText("Home snapshot: Session renewal");
  await page.screenshot({ path: testInfo.outputPath("privilege-reconnected.png") });
});

for (const trigger of ["visible", "online", "pageshow", "reload"]) {
  for (const revoked of [false, true]) {
    test(`recovers a saved renewal failure on ${trigger} and ${revoked ? "shows Sign in after revocation" : "keeps the same login"}`, async ({ page }) => {
      const authMock = await installAuthSessionApiMock(page);
      await signInForRecovery(page);
      const initial = await readCanonicalAuthSession(page);
      let unavailable = true;
      let refreshes = 0;
      await page.route("**/api/v1/auth/refresh", async route => {
        refreshes++;
        const auth = !unavailable && !revoked
          ? authMock.rotate(route.request().postDataJSON().refresh_token)
          : null;
        await route.fulfill({
          status: unavailable ? 503 : auth ? 200 : 401,
          json: auth ?? { error: unavailable ? "refresh_service_unavailable" : "invalid_refresh_token" },
        });
      });
      authMock.expireAccess();
      await page.reload();
      const notice = page.getByRole("alert").filter({ hasText: "Session refresh unavailable" });
      await expect(notice).toBeVisible();
      expect(refreshes).toBe(1);
      await page.clock.setSystemTime(new Date(Date.now() + 3 * 86_400_000));
      unavailable = false;
      await resumeSessionPage(page, trigger);
      if (revoked) {
        await expectOperatorAccessShell(page);
        expect((await readCanonicalAuthSession(page))?.status).toBe("ended");
      } else {
        await expect(notice).toHaveCount(0);
        await expect.poll(() => readSessionStorage(page)).toEqual({
          access: rotatedAccessToken, refresh: rotatedRefreshToken,
        });
        expect((await readCanonicalAuthSession(page))?.epoch).toBe(initial?.epoch);
        await expectAuthenticatedConsoleShell(page);
      }
      expect(refreshes).toBe(2);
    });
  }
}

test("resolves a renewed read's 401 through refresh and returns the UI to Sign in", async ({ page }) => {
  const authMock = await installAuthSessionApiMock(page);
  await signInForRecovery(page);
  let refreshes = 0;
  let rejectedReads = 0;
  await page.route("**/api/v1/home/snapshot**", async route => {
    rejectedReads++;
    await route.fulfill({ status: 401, json: { error: "invalid_operator_session" } });
  });
  await page.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    const auth = refreshes === 1 ? authMock.rotate(route.request().postDataJSON().refresh_token) : null;
    await route.fulfill({ status: auth ? 200 : 401, json: auth ?? { error: "invalid_refresh_token" } });
  });
  await page.reload();
  await expectOperatorAccessShell(page);
  expect(rejectedReads).toBe(2);
  expect(refreshes).toBe(2);
  expect((await readCanonicalAuthSession(page))?.status).toBe("ended");
});

test("reload reopens an unavailable attempt once while ordinary reads keep sharing its failure", async ({ page }) => {
  const authMock = await installAuthSessionApiMock(page);
  await signInForRecovery(page);
  let refreshes = 0;
  await page.route("**/api/v1/auth/refresh", route => {
    refreshes++;
    return route.fulfill({ status: 503, json: { error: "refresh_service_unavailable" } });
  });
  authMock.expireAccess();
  await page.reload();
  const notice = page.getByRole("alert").filter({ hasText: "Session refresh unavailable" });
  await expect(notice).toBeVisible();
  const firstFailure = (await readCanonicalAuthSession(page))?.attempt?.id;
  expect(refreshes).toBe(1);
  await page.reload();
  await expect.poll(async () => (await readCanonicalAuthSession(page))?.attempt?.id).not.toBe(firstFailure);
  await expect(notice).toBeVisible();
  expect(refreshes).toBe(2);
  const refresh = page.getByTitle("Refresh dashboard telemetry", { exact: true });
  await expect(refresh).toBeEnabled();
  const read = page.waitForResponse(response =>
    response.request().method() === "GET" &&
    new URL(response.url()).pathname.startsWith("/api/v1/") && response.status() === 401);
  await activate(refresh);
  await read;
  await expect(refresh).toBeEnabled();
  expect(refreshes).toBe(2);
});

test("late renewal failure cannot strand saved privilege after another tab already recovered", async ({ page, context }) => {
  const authMock = await installAuthSessionApiMock(context);
  await signInForRecovery(page);
  await savePrivilegeGrant(page);
  const peer = await openRecoveryPeer(context);
  await page.addInitScript(() => {
    const state = window as typeof window & {
      __failureHeld?: boolean;
      __releaseFailure?: () => void;
    };
    const original = navigator.locks.request.bind(navigator.locks);
    let delayed = false;
    navigator.locks.request = ((name: string, ...args: unknown[]) => {
      const result = Reflect.apply(original, navigator.locks, [name, ...args]);
      if (name !== "vpsman.authSession" || delayed) return result;
      delayed = true;
      // Deliver the rejection after the owner has released the lock, as when
      // this renderer resumes after another tab recovered the same login.
      return result.catch(async (error: unknown) => {
        state.__failureHeld = true;
        await new Promise<void>(resolve => { state.__releaseFailure = resolve; });
        throw error;
      });
    }) as LockManager["request"];
  });
  let releaseRefresh!: () => void;
  const verificationRejected = new Promise<void>(resolve => { releaseRefresh = resolve; });
  let available = false;
  let refreshes = 0;
  await context.route("**/api/v1/auth/privilege/verify", async route => {
    if (route.request().headers().authorization === `Bearer ${accessToken}`) {
      await route.fulfill({ status: 401, json: { error: "invalid_operator_session" } });
      releaseRefresh();
    } else {
      await route.fallback();
    }
  });
  await context.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    if (!available) {
      await verificationRejected;
      await route.fulfill({ status: 503, json: { error: "refresh_service_unavailable" } });
    } else {
      const auth = authMock.rotate(route.request().postDataJSON().refresh_token);
      await route.fulfill({ status: auth ? 200 : 401, json: auth ?? { error: "invalid_refresh_token" } });
    }
  });
  try {
    authMock.expireAccess();
    await page.reload();
    await page.waitForFunction(() => (window as typeof window & { __failureHeld?: boolean }).__failureHeld);
    available = true;
    await peer.evaluate(() => window.dispatchEvent(new Event("online")));
    await expect.poll(async () => (await readCanonicalAuthSession(peer))?.revision).toBe(1);
    await page.evaluate(() => (window as typeof window & { __releaseFailure: () => void }).__releaseFailure());
    await expectPrivilegeVerifiedForViewport(page);
    await expect(page.getByRole("dialog", { name: "Unlock privilege" })).toHaveCount(0);
    await expect(page.locator(".shell")).not.toContainText("Home snapshot: Session renewal");
    expect(refreshes).toBe(2);
    expect((await readCanonicalAuthSession(page))?.revision).toBe(1);
  } finally {
    releaseRefresh();
    await page.evaluate(() => (window as typeof window & { __releaseFailure?: () => void }).__releaseFailure?.());
    await peer.close();
  }
});

test("a disconnected refresh preserves the login and resumes on the online event", async ({ page }) => {
  const authMock = await installAuthSessionApiMock(page);
  await signInForRecovery(page);
  const initial = await readCanonicalAuthSession(page);
  let disconnected = true;
  let refreshes = 0;
  await page.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    if (disconnected) await route.abort("internetdisconnected");
    else await route.fallback();
  });
  authMock.expireAccess();
  await page.reload();
  const notice = page.getByRole("alert").filter({ hasText: "Session refresh unavailable" });
  await expect(notice).toBeVisible();
  expect(await readCanonicalAuthSession(page)).toMatchObject({
    epoch: initial?.epoch, status: "active", accessToken, refreshToken,
  });
  expect(refreshes).toBe(1);
  disconnected = false;
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect(notice).toHaveCount(0);
  await expect.poll(async () => (await readCanonicalAuthSession(page))?.revision).toBe(1);
  await expect(page.locator(".shell")).not.toContainText("Home snapshot:");
  expect((await readCanonicalAuthSession(page))?.epoch).toBe(initial?.epoch);
  expect(refreshes).toBe(2);
});

for (const { trigger, lateOutcome, newerOutcome } of [
  { trigger: "manual Retry", lateOutcome: "failure", newerOutcome: "success" },
  { trigger: "manual Retry", lateOutcome: "success", newerOutcome: "failure" },
  { trigger: "manual Retry", lateOutcome: "failure", newerOutcome: "failure" },
  { trigger: "online recovery", lateOutcome: "failure", newerOutcome: "failure" },
] as const) {
  test(`a late ${trigger} ${lateOutcome} cannot overwrite a newer cross-tab ${newerOutcome}`, async ({ page, context }) => {
    const authMock = await installAuthSessionApiMock(context);
    await signInForRecovery(page);
    const peer = await openRecoveryPeer(context);
    await page.bringToFront();
    let available = false;
    let failureStatus = 503;
    let refreshes = 0;
    await context.route("**/api/v1/auth/refresh", async route => {
      refreshes++;
      const auth = available ? authMock.rotate(route.request().postDataJSON().refresh_token) : null;
      await route.fulfill({
        status: available ? (auth ? 200 : 401) : failureStatus,
        json: auth ?? { error: available ? "invalid_refresh_token" : "refresh_service_unavailable" },
      });
    });
    try {
      authMock.expireAccess();
      await page.reload();
      const notice = page.getByRole("alert").filter({ hasText: "Session refresh unavailable" });
      await expect(notice).toBeVisible();
      expect(refreshes).toBe(1);
      await page.evaluate(outcome => {
        const state = window as typeof window & {
          __settlementHeld?: boolean;
          __releaseSettlement?: () => void;
        };
        const original = navigator.locks.request.bind(navigator.locks);
        let delayed = false;
        const delay = async () => {
          state.__settlementHeld = true;
          await new Promise<void>(resolve => { state.__releaseSettlement = resolve; });
        };
        navigator.locks.request = ((name: string, ...args: unknown[]) => {
          const result = Reflect.apply(original, navigator.locks, [name, ...args]);
          if (name !== "vpsman.authSession" || delayed) return result;
          delayed = true;
          return outcome === "success"
            ? result.then(async (value: unknown) => { await delay(); return value; })
            : result.catch(async (error: unknown) => { await delay(); throw error; });
        }) as LockManager["request"];
      }, lateOutcome);
      available = lateOutcome === "success";
      if (trigger === "manual Retry") {
        await notice.getByRole("button", { name: "Retry", exact: true }).click();
      } else {
        await page.evaluate(() => window.dispatchEvent(new Event("online")));
      }
      await page.waitForFunction(() => (window as typeof window & { __settlementHeld?: boolean }).__settlementHeld);
      expect(refreshes).toBe(2);
      if (newerOutcome === "success") {
        available = true;
        await peer.evaluate(() => window.dispatchEvent(new Event("online")));
        await expect.poll(async () => (await readCanonicalAuthSession(peer))?.revision).toBe(1);
      } else {
        available = false;
        failureStatus = 502;
        if (lateOutcome === "success") {
          authMock.expireAccess();
          await peer.evaluate(async () => {
            const authPath = "/src/authSession.ts";
            const apiPath = "/src/api.ts";
            const auth = await import(authPath);
            const api = await import(apiPath);
            await api.apiGet("/api/v1/auth/me", auth.getCurrentAuthSession()).catch(() => undefined);
          });
        } else {
          await peer.evaluate(() => window.dispatchEvent(new Event("online")));
        }
        await expect.poll(async () => (await readCanonicalAuthSession(peer))?.attempt?.error).toContain("502");
        await expect(notice).toBeVisible();
      }
      // Synchronize this renderer before delivering its obsolete callback.
      await page.evaluate(async () => {
        const path = performance.getEntriesByType("resource").map(entry => entry.name)
          .filter(url => new URL(url).pathname === "/src/authSession.ts").at(-1)!;
        const auth = await import(path);
        await auth.readCurrentSessionCredentials(auth.getCurrentAuthSession());
        (window as typeof window & { __releaseSettlement: () => void }).__releaseSettlement();
        await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
      });
      if (newerOutcome === "success") {
        await expect(notice).toHaveCount(0);
        expect((await readCanonicalAuthSession(page))?.attempt?.error).toBeNull();
      } else {
        await expect(notice).toBeVisible();
        await expect(notice).toContainText("502");
        expect((await readCanonicalAuthSession(page))?.attempt?.error).toContain("502");
      }
      expect(refreshes).toBe(3);
      expect((await readCanonicalAuthSession(page))?.revision).toBe(lateOutcome === "success" || newerOutcome === "success" ? 1 : 0);
    } finally {
      await page.evaluate(() => (window as typeof window & { __releaseSettlement?: () => void }).__releaseSettlement?.());
      await peer.close();
    }
  });
}

test("Retry joining an unsettled failure keeps the notice available for another attempt", async ({ page, context }) => {
  const authMock = await installAuthSessionApiMock(context);
  await signInForRecovery(page);
  let available = false;
  let refreshes = 0;
  await context.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    if (available) await route.fallback();
    else await route.fulfill({ status: 503, json: { error: "refresh_service_unavailable" } });
  });
  authMock.expireAccess();
  await page.reload();
  const notice = page.getByRole("alert").filter({ hasText: "Session refresh unavailable" });
  await expect(notice).toBeVisible();
  await page.evaluate(async () => {
    const state = window as typeof window & {
      __failureHeld?: boolean;
      __releaseFailure?: () => void;
      __pendingFailure?: Promise<void>;
    };
    const path = performance.getEntriesByType("resource").map(entry => entry.name)
      .filter(url => new URL(url).pathname === "/src/authSession.ts").at(-1)!;
    const auth = await import(path);
    const original = navigator.locks.request.bind(navigator.locks);
    let delayed = false;
    navigator.locks.request = ((name: string, ...args: unknown[]) => {
      const result = Reflect.apply(original, navigator.locks, [name, ...args]);
      if (name !== "vpsman.authSession" || delayed) return result;
      delayed = true;
      return result.catch(async (error: unknown) => {
        state.__failureHeld = true;
        await new Promise<void>(resolve => { state.__releaseFailure = resolve; });
        throw error;
      });
    }) as LockManager["request"];
    const session = auth.getCurrentAuthSession();
    const credentials = await auth.readCurrentSessionCredentials(session);
    state.__pendingFailure = auth.renewAuthSession(session, credentials.revision, credentials.attempt, true)
      .catch(() => undefined);
  });
  try {
    await page.waitForFunction(() => (window as typeof window & { __failureHeld?: boolean }).__failureHeld);
    expect(refreshes).toBe(2);
    await notice.getByRole("button", { name: "Retry", exact: true }).click();
    await page.evaluate(async () => {
      const state = window as typeof window & { __releaseFailure: () => void; __pendingFailure: Promise<void> };
      state.__releaseFailure();
      await state.__pendingFailure;
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    });
    await expect(notice).toBeVisible();
    await expect(notice).toContainText("503");
    expect(refreshes).toBe(2);
    available = true;
    await notice.getByRole("button", { name: "Retry", exact: true }).click();
    await expect(notice).toHaveCount(0);
    await expect.poll(async () => (await readCanonicalAuthSession(page))?.revision).toBe(1);
    expect(refreshes).toBe(3);
  } finally {
    await page.evaluate(() => (window as typeof window & { __releaseFailure?: () => void }).__releaseFailure?.());
  }
});

async function installAuthSessionApiMock(
  page: Page | BrowserContext,
) {
  let currentAccess: string | null = accessToken;
  let currentRefresh: string | null = refreshToken;
  let rotation = 0;
  const operator = {
    id: "99999999-aaaa-4bbb-8ccc-000000000001",
    preferences,
    role: "admin",
    scopes: ["*"],
    totp_enabled: false,
    username: "session-admin",
  };
  const authResponse = () => ({
    access_token: currentAccess,
    expires_in_secs: 900,
    operator,
    refresh_expires_in_secs: 1209600,
    refresh_token: currentRefresh,
    token_type: "Bearer",
  });
  function isAuthorized(request: import("@playwright/test").Request): boolean {
    return currentAccess !== null && request.headers().authorization === `Bearer ${currentAccess}`;
  }
  function rotate(token: unknown) {
    if (!currentRefresh || token !== currentRefresh) return null;
    // Each accepted rotation consumes the old pair. Deterministic subsequent
    // pairs let tests detect reuse without depending on wall-clock expiry.
    rotation += 1;
    currentAccess = rotation === 1 ? rotatedAccessToken : (rotation * 2).toString(16).padStart(64, "0");
    currentRefresh = rotation === 1 ? rotatedRefreshToken : (rotation * 2 + 1).toString(16).padStart(64, "0");
    return authResponse();
  }
  await page.routeWebSocket("**/ws", () => undefined);
  await page.route("**/api/v1/auth/bootstrap-status", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: { bootstrap_required: false },
    });
  });
  await page.route("**/api/v1/auth/login", async (route) => {
    currentAccess = accessToken;
    currentRefresh = refreshToken;
    await route.fulfill({
      contentType: "application/json",
      json: authResponse(),
    });
  });
  await page.route("**/api/v1/auth/logout", async (route) => {
    if (!isAuthorized(route.request())) {
      await route.fulfill({ status: 401, json: { error: "missing_bearer_token" } });
      return;
    }
    currentAccess = null;
    currentRefresh = null;
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
    const auth = rotate(body.refresh_token);
    if (!auth) {
      await route.fulfill({
        contentType: "application/json",
        json: { error: "invalid_refresh_token" },
        status: 401,
      });
      return;
    }
    await route.fulfill({
      contentType: "application/json",
      json: auth,
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
      response.policy_alerts_truncated = false;
      response.fleet_alert_notifications_truncated = false;
      response.webhook_rule_deliveries_truncated = false;
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
    "operator-auth-events",
    "gateway-sessions",
    "client-key-revocations",
    "key-lifecycle/report",
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
            : path === "key-lifecycle/report"
              ? {
                  clients: [],
                  current_key_revoked_count: 0,
                  direct_identity_client_count: 0,
                  revocation_count: 0,
                  suggested_client_id: "v-1",
                }
            : path === "network/observations" &&
                new URL(route.request().url()).searchParams.get("format") === "compact"
              ? { fields: NETWORK_OBSERVATION_FIELDS, rows: [] }
              : [],
      });
    });
  }
  return { rotate, expireAccess: () => { currentAccess = null; } };
}

async function readSessionStorage(page: import("@playwright/test").Page) {
  const session = await readCanonicalAuthSession(page);
  const storage = await page.evaluate(() => {
    const raw = window.localStorage.getItem("vpsman.authSession.v1");
    return {
      hint: raw ? JSON.parse(raw) : null,
      legacyAccess: window.localStorage.getItem("vpsman.accessToken"),
      legacyRefresh: window.localStorage.getItem("vpsman.refreshToken"),
    };
  });
  expect(storage.legacyAccess).toBeNull();
  expect(storage.legacyRefresh).toBeNull();
  if (storage.hint) {
    expect(storage.hint).not.toHaveProperty("accessToken");
    expect(storage.hint).not.toHaveProperty("refreshToken");
  }
  return {
    access: session?.status === "active" ? session.accessToken : null,
    refresh: session?.status === "active" ? session.refreshToken : null,
  };
}
