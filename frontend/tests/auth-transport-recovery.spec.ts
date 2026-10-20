import { createServer } from "node:http";
import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { readCanonicalAuthSession } from "./support/authSessionStorage";

type ProbeWindow = Window & {
  authProbe: {
    api: typeof import("../src/api");
    auth: typeof import("../src/authSession");
    result?: Promise<{ ok: boolean; value?: unknown; type?: string; message?: string }>;
    errors: Array<string | null>;
    restoreFetch?: () => void;
    headersReceived?: boolean;
    releaseLock?: () => void;
    lockFinished?: Promise<void>;
    releaseStateRead?: () => void;
    stateReadBlocked?: boolean;
  };
};

async function setupTransport(context: BrowserContext, page: Page) {
  const initial = {
    access_token: "transport-access-0", refresh_token: "transport-refresh-0",
    operator: { id: "transport-operator", role: "admin", status: "active", scopes: ["*"] },
  };
  const server = {
    access: initial.access_token, refresh: initial.refresh_token, expired: true,
    refreshes: 0, requests: [] as string[],
  };
  await context.route("**/__auth-transport-probe", route => route.fulfill({
    contentType: "text/html", body: "<!doctype html><title>Authentication transport</title>",
  }));
  await context.route("**/api/v1/probe", route => {
    server.requests.push(route.request().method());
    const accepted = !server.expired && route.request().headers().authorization === `Bearer ${server.access}`;
    return route.fulfill({ status: accepted ? 200 : 401, json: accepted ? { ok: true } : { error: "invalid_operator_session" } });
  });
  await context.route("**/api/v1/auth/refresh", route => {
    server.refreshes++;
    if (route.request().postDataJSON().refresh_token !== server.refresh) {
      return route.fulfill({ status: 401, json: { error: "invalid_refresh_token" } });
    }
    server.access = `transport-access-${server.refreshes}`;
    server.refresh = `transport-refresh-${server.refreshes}`;
    server.expired = false;
    return route.fulfill({ json: { ...initial, access_token: server.access, refresh_token: server.refresh } });
  });
  const open = async (tab: Page, install = false) => {
    await tab.goto("/__auth-transport-probe");
    await tab.evaluate(async ({ initial, install }) => {
      const apiPath = "/src/api.ts";
      const api = await import(apiPath);
      // Use the exact dependency URL, including Vite's revision if present.
      const authUrl = performance.getEntriesByType("resource").map(entry => entry.name)
        .filter(url => new URL(url).pathname === "/src/authSession.ts").at(-1)!;
      const auth = await import(authUrl);
      const state = (window as unknown as ProbeWindow).authProbe = { api, auth, errors: [] };
      auth.subscribeAuthRefreshError((error: string | null) => state.errors.push(error));
      if (install) await auth.installAuthSession(initial);
      else await auth.initializeAuthSession();
    }, { initial, install });
  };
  await open(page, true);
  return { server, open };
}

async function startRead(page: Page, method = "GET", path = "/api/v1/probe") {
  await page.evaluate(({ method, path }) => {
    const state = (window as unknown as ProbeWindow).authProbe;
    const session = state.auth.getCurrentAuthSession();
    state.result = (method === "POST"
      ? state.api.apiPost(path, session, { value: "reviewed operator action" })
      : state.api.apiGet(path, session))
      .then(value => ({ ok: true, value }), error => ({
        ok: false, type: error.constructor.name, message: error.message,
      }));
  }, { method, path });
}

async function finishRead(page: Page) {
  return page.evaluate(() => (window as unknown as ProbeWindow).authProbe.result!);
}

test("a delayed old 401 adopts the newer pair without another rotation", async ({ page, context }) => {
  const { server, open } = await setupTransport(context, page);
  const second = await context.newPage();
  await open(second);
  let release!: () => void;
  let held = false;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await context.route("**/api/v1/probe", async route => {
    if (route.request().frame().page() === page && !held) {
      held = true;
      await gate;
      await route.fulfill({ status: 401, json: { error: "invalid_operator_session" } });
    } else {
      await route.fallback();
    }
  });
  try {
    await startRead(page);
    await expect.poll(() => held).toBe(true);
    await startRead(second);
    expect((await finishRead(second)).ok).toBe(true);
    release();
    expect((await finishRead(page)).ok).toBe(true);
    expect(server.refreshes).toBe(1);
    expect((await readCanonicalAuthSession(page))?.revision).toBe(1);
  } finally {
    release();
    await second.close();
  }
});

test("successful pending writes survive rotation and rejected writes are never replayed", async ({ page, context }) => {
  const { server, open } = await setupTransport(context, page);
  const second = await context.newPage();
  await open(second);
  let writes = 0;
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await context.route("**/api/v1/save", async route => {
    writes++;
    await gate;
    await route.fulfill({ json: { saved: true } });
  });
  try {
    await startRead(page, "POST", "/api/v1/save");
    await expect.poll(() => writes).toBe(1);
    await startRead(second);
    expect((await finishRead(second)).ok).toBe(true);
    release();
    expect(await finishRead(page)).toEqual({ ok: true, value: { saved: true } });
    server.expired = true;
    await startRead(page, "POST");
    expect(await finishRead(page)).toMatchObject({ ok: false, type: "ApiAuthRecoveredError" });
    expect(server.requests.filter(method => method === "POST")).toHaveLength(1);
    expect(writes).toBe(1);
    expect(server.refreshes).toBe(2);
  } finally {
    release();
    await second.close();
  }
});

test("a repeatedly rejected endpoint has bounded recovery without ending a valid login", async ({ page, context }) => {
  const { server } = await setupTransport(context, page);
  const initial = await readCanonicalAuthSession(page);
  let reads = 0;
  await context.route("**/api/v1/probe", route => {
    reads++;
    return route.fulfill({ status: 401, json: { error: "invalid_operator_session" } });
  });
  await startRead(page);
  expect(await finishRead(page)).toMatchObject({ ok: false, type: "ApiResponseError" });
  expect(reads).toBe(2);
  expect(server.refreshes).toBe(2);
  expect(await readCanonicalAuthSession(page)).toMatchObject({ status: "active", epoch: initial?.epoch, revision: 2 });
});

test("resume attempts share failure outcomes across tabs and coalesce successful recovery", async ({ page, context }) => {
  const { server, open } = await setupTransport(context, page);
  const second = await context.newPage();
  await open(second);
  let unavailable = true;
  let attempts = 0;
  await context.route("**/api/v1/auth/refresh", async route => {
    attempts++;
    if (unavailable) await route.fulfill({ status: 503, json: { error: "temporarily_unavailable" } });
    else await route.fallback();
  });
  try {
    for (const tab of [page, second]) {
      await startRead(tab);
      expect(await finishRead(tab)).toMatchObject({ ok: false, type: "AuthRenewalUnavailableError" });
    }
    expect(attempts).toBe(1);
    const firstFailure = await readCanonicalAuthSession(page);
    const notices = await second.evaluate(() => (window as unknown as ProbeWindow).authProbe.errors.length);
    await page.evaluate(() => window.dispatchEvent(new Event("online")));
    await expect.poll(() => attempts).toBe(2);
    await expect.poll(async () => (await readCanonicalAuthSession(page))?.attempt?.id).not.toBe(firstFailure?.attempt?.id);
    // The second tab must observe a new outcome even if its error text repeats.
    await expect.poll(() => second.evaluate(() => (window as unknown as ProbeWindow).authProbe.errors.length)).toBeGreaterThan(notices);
    unavailable = false;
    await Promise.all([page, second].map(tab => tab.evaluate(() => window.dispatchEvent(new Event("online")))));
    await expect.poll(async () => (await readCanonicalAuthSession(page))?.revision).toBe(1);
    await expect.poll(() => second.evaluate(() => (window as unknown as ProbeWindow).authProbe.errors.at(-1))).toBeNull();
    expect(attempts).toBe(3);
    expect(server.refreshes).toBe(1);
    for (const tab of [page, second]) {
      await tab.evaluate(async () => {
        window.dispatchEvent(new Event("online"));
        const auth = (window as unknown as ProbeWindow).authProbe.auth;
        await auth.readCurrentSessionCredentials(auth.getCurrentAuthSession()!);
      });
    }
    expect(attempts).toBe(3);
  } finally {
    await second.close();
  }
});

test("three tabs share repeated failed wake bursts and a later successful recovery", async ({ page, context }) => {
  const { server, open } = await setupTransport(context, page);
  const pages = [page, await context.newPage(), await context.newPage()];
  for (const tab of pages.slice(1)) await open(tab);
  let attempts = 0;
  let available = false;
  let release!: () => void;
  let gate = new Promise<void>(resolve => { release = resolve; });
  await context.route("**/api/v1/auth/refresh", async route => {
    attempts++;
    await gate;
    if (available) await route.fallback();
    else await route.fulfill({ status: 503, json: { error: "temporarily_unavailable" } });
  });
  const wake = async () => {
    await Promise.all(pages.map(tab => tab.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, value: true });
      document.dispatchEvent(new Event("visibilitychange"));
      Object.defineProperty(document, "hidden", { configurable: true, value: false });
      document.dispatchEvent(new Event("visibilitychange"));
      window.dispatchEvent(new Event("online"));
      window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    })));
  };
  try {
    await Promise.all(pages.map(tab => startRead(tab)));
    await expect.poll(() => attempts).toBe(1);
    release();
    for (const tab of pages) {
      expect(await finishRead(tab)).toMatchObject({ ok: false, type: "AuthRenewalUnavailableError" });
    }
    const failed = await readCanonicalAuthSession(page);
    gate = new Promise<void>(resolve => { release = resolve; });
    await wake();
    await expect.poll(() => attempts).toBe(2);
    release();
    await expect.poll(async () => (await readCanonicalAuthSession(page))?.attempt?.id).not.toBe(failed?.attempt?.id);
    await expect.poll(() => page.evaluate(() => navigator.locks.query())).toMatchObject({ held: [], pending: [] });
    expect(attempts).toBe(2);
    available = true;
    gate = new Promise<void>(resolve => { release = resolve; });
    await wake();
    await expect.poll(() => attempts).toBe(3);
    release();
    await expect.poll(async () => (await readCanonicalAuthSession(page))?.revision).toBe(1);
    for (const tab of pages) {
      await startRead(tab);
      expect((await finishRead(tab)).ok).toBe(true);
      expect((await readCanonicalAuthSession(tab))?.epoch).toBe(failed?.epoch);
    }
    await wake();
    for (const tab of pages) {
      await tab.evaluate(async () => {
        const auth = (window as unknown as ProbeWindow).authProbe.auth;
        await auth.readCurrentSessionCredentials(auth.getCurrentAuthSession()!);
      });
    }
    expect(attempts).toBe(3);
    expect(server.refreshes).toBe(1);
  } finally {
    release();
    for (const tab of pages.slice(1)) await tab.close();
  }
});

for (const committed of [false, true]) {
  test(`closing the refresh owner ${committed ? "after" : "before"} server commit releases waiting tabs safely`, async ({ page, context }) => {
    const { server, open } = await setupTransport(context, page);
    const second = await context.newPage();
    await open(second);
    let ownerWaiting = false;
    let requests = 0;
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    await context.route("**/api/v1/auth/refresh", async route => {
      requests++;
      if (!ownerWaiting && route.request().frame().page() === page) {
        ownerWaiting = true;
        if (committed) server.refresh = "consumed-by-closed-owner";
        await gate;
        // This response belongs to the deliberately destroyed renderer.
        await route.abort().catch(() => undefined);
      } else {
        await route.fallback();
      }
    });
    try {
      await startRead(page);
      await expect.poll(() => ownerWaiting).toBe(true);
      await startRead(second);
      await second.waitForFunction(async () => (await navigator.locks.query()).pending
        .some(lock => lock.name === "vpsman.authSession"));
      await page.close();
      const result = await finishRead(second);
      expect(result).toMatchObject(committed
        ? { ok: false, type: "ApiUnauthorizedError" }
        : { ok: true });
      expect(await readCanonicalAuthSession(second)).toMatchObject(committed
        ? { status: "ended", accessToken: "", refreshToken: "" }
        : { status: "active", revision: 1 });
      expect(requests).toBe(2);
      await expect.poll(() => second.evaluate(() => navigator.locks.query())).toMatchObject({ held: [], pending: [] });
    } finally {
      release();
      await second.close();
    }
  });
}

test("an account switch during renewal preserves the new login and revokes only the late old replacement", async ({ page, context }) => {
  const { server, open } = await setupTransport(context, page);
  const second = await context.newPage();
  await open(second);
  const original = await readCanonicalAuthSession(second);
  let entered = false;
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const revoked: Array<string | undefined> = [];
  await context.route("**/api/v1/auth/refresh", async route => {
    entered = true;
    await gate;
    await route.fallback();
  });
  await context.route("**/api/v1/auth/logout", route => {
    revoked.push(route.request().headers().authorization);
    return route.fulfill({ status: 204 });
  });
  await context.route("**/api/v1/probe", route => {
    if (route.request().headers().authorization === "Bearer replacement-access") {
      return route.fulfill({ json: { account: "replacement" } });
    }
    return route.fallback();
  });
  try {
    await startRead(page);
    await expect.poll(() => entered).toBe(true);
    await second.evaluate(async auth => {
      await (window as unknown as ProbeWindow).authProbe.auth.installAuthSession(auth);
    }, {
      access_token: "replacement-access", refresh_token: "replacement-refresh",
      expires_in_secs: 86400, refresh_expires_in_secs: 31536000,
      token_type: "Bearer", session_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      operator: { ...original!.operator!, id: "replacement-operator" },
    });
    const replacement = await readCanonicalAuthSession(second);
    expect(replacement?.epoch).not.toBe(original?.epoch);
    await startRead(second);
    expect(await finishRead(second)).toEqual({ ok: true, value: { account: "replacement" } });
    release();
    expect(await finishRead(page)).toMatchObject({ ok: false, type: "ApiUnauthorizedError" });
    expect(revoked).toEqual([`Bearer ${server.access}`]);
    expect(revoked).not.toContain("Bearer replacement-access");
    expect(await readCanonicalAuthSession(second)).toMatchObject({
      status: "active", epoch: replacement?.epoch,
      accessToken: "replacement-access", refreshToken: "replacement-refresh",
      operator: { id: "replacement-operator" },
    });
  } finally {
    release();
    await second.close();
  }
});

test("publishes a failed attempt before handing renewal ownership to another tab", async ({ page, context }) => {
  const { open } = await setupTransport(context, page);
  const second = await context.newPage();
  await open(second);
  let refreshes = 0;
  let releaseResponse!: () => void;
  const responseGate = new Promise<void>(resolve => { releaseResponse = resolve; });
  await context.route("**/api/v1/auth/refresh", async route => {
    refreshes++;
    await responseGate;
    await route.fulfill({ status: 503, json: { error: "temporarily_unavailable" } });
  });
  try {
    await startRead(page);
    await expect.poll(() => refreshes).toBe(1);
    await page.evaluate(() => {
      const state = (window as unknown as ProbeWindow).authProbe;
      const original = navigator.locks.request.bind(navigator.locks);
      let delayed = false;
      // Delay the failure's state publication, allowing the other renderer to
      // compete for renewal ownership before that outcome becomes visible.
      navigator.locks.request = ((name: string, ...args: unknown[]) => {
        const invoke = () => Reflect.apply(original, navigator.locks, [name, ...args]);
        if (name !== "vpsman.authSession.state" || delayed) return invoke();
        delayed = true;
        state.stateReadBlocked = true;
        return new Promise<void>(resolve => { state.releaseStateRead = resolve; }).then(invoke);
      }) as LockManager["request"];
    });
    releaseResponse();
    await page.waitForFunction(() => (window as unknown as ProbeWindow).authProbe.stateReadBlocked);
    await second.evaluate(() => {
      const state = (window as unknown as ProbeWindow).authProbe;
      state.result = state.auth.renewAuthSession(state.auth.getCurrentAuthSession()!, 0, null)
        .then(() => ({ ok: true }), error => ({ ok: false, type: error.constructor.name }));
    });
    await expect.poll(() => second.evaluate(async () =>
      (await navigator.locks.query()).pending.some(lock => lock.name === "vpsman.authSession"))).toBe(true);
    expect(refreshes).toBe(1);
    await page.evaluate(() => (window as unknown as ProbeWindow).authProbe.releaseStateRead!());
    expect(await finishRead(page)).toMatchObject({ ok: false, type: "AuthRenewalUnavailableError" });
    expect(await finishRead(second)).toMatchObject({ ok: false, type: "AuthRenewalUnavailableError" });
    expect(refreshes).toBe(1);
  } finally {
    releaseResponse();
    await page.evaluate(() => (window as unknown as ProbeWindow).authProbe.releaseStateRead?.());
    await second.close();
  }
});

test("a lock waiter times out without overwriting its owner's result or retaining a stale notice", async ({ page, context }) => {
  const { server, open } = await setupTransport(context, page);
  const second = await context.newPage();
  await open(second);
  await second.clock.install();
  // Playwright's clock applies to every page in the context. Represent a
  // suspended owner with a real held lock whose callback cannot time itself out.
  await page.evaluate(() => {
    const state = (window as unknown as ProbeWindow).authProbe;
    state.lockFinished = navigator.locks.request("vpsman.authSession", () =>
      new Promise<void>(resolve => { state.releaseLock = resolve; }));
  });
  try {
    await page.waitForFunction(() => Boolean((window as unknown as ProbeWindow).authProbe.releaseLock));
    await startRead(second);
    await second.waitForFunction(async () => (await navigator.locks.query()).pending
      .some(lock => lock.name === "vpsman.authSession"));
    await second.clock.runFor(60_001);
    expect(await finishRead(second)).toMatchObject({ ok: false, type: "AuthRenewalUnavailableError" });
    expect((await readCanonicalAuthSession(second))?.attempt).toBeNull();
    expect(await second.evaluate(() => (window as unknown as ProbeWindow).authProbe.errors.at(-1))).toContain("timed out");
    await page.evaluate(async () => {
      const state = (window as unknown as ProbeWindow).authProbe;
      state.releaseLock!();
      await state.lockFinished;
    });
    await startRead(page);
    expect((await finishRead(page)).ok).toBe(true);
    await expect.poll(() => second.evaluate(() => (window as unknown as ProbeWindow).authProbe.errors.at(-1))).toBeNull();
    expect(server.refreshes).toBe(1);
    expect(await second.evaluate(() => navigator.locks.query())).toMatchObject({ held: [], pending: [] });
  } finally {
    await page.evaluate(() => (window as unknown as ProbeWindow).authProbe.releaseLock?.());
    await second.close();
  }
});

for (const scenario of ["headers", "body", "resume", "lost-response"]) {
  test(`aborts a stalled refresh ${scenario} and recovers without overlapping owners`, async ({ page, context }) => {
    const { server } = await setupTransport(context, page);
    // Only this test's cross-port loopback server needs browser network access.
    await context.grantPermissions(["local-network-access"]);
    await page.clock.install();
    let entered = false;
    let disconnected = false;
    const http = createServer((request, response) => {
      response.setHeader("Access-Control-Allow-Origin", "*");
      response.setHeader("Access-Control-Allow-Headers", "content-type");
      if (request.method === "OPTIONS") {
        response.writeHead(204);
        response.end();
        return;
      }
      entered = true;
      response.on("close", () => { disconnected = true; });
      if (scenario === "lost-response") server.refresh = "already-consumed";
      if (scenario !== "headers") {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.write('{"access_token":');
      }
    });
    await new Promise<void>(resolve => http.listen(0, "127.0.0.1", resolve));
    const address = http.address();
    if (!address || typeof address === "string") throw new Error("Expected a TCP test listener");
    try {
      await page.evaluate(url => {
        const state = (window as unknown as ProbeWindow).authProbe;
        const original = window.fetch.bind(window);
        state.restoreFetch = () => { window.fetch = original; };
        window.fetch = async (input, init) => {
          if (String(input) !== "/api/v1/auth/refresh") return original(input, init);
          const response = await original(url, init);
          state.headersReceived = true;
          return response;
        };
      }, `http://127.0.0.1:${address.port}/refresh`);
      await startRead(page);
      await expect.poll(() => entered).toBe(true);
      if (scenario !== "headers") {
        await expect.poll(() => page.evaluate(() => (window as unknown as ProbeWindow).authProbe.headersReceived)).toBe(true);
      }
      if (scenario === "resume") {
        await page.evaluate(() => {
          (window as unknown as ProbeWindow).authProbe.restoreFetch!();
          Object.defineProperty(document, "hidden", { configurable: true, value: true });
          document.dispatchEvent(new Event("visibilitychange"));
        });
        await page.clock.setSystemTime(new Date(Date.now() + 3 * 86_400_000));
        await page.evaluate(() => {
          Object.defineProperty(document, "hidden", { configurable: true, value: false });
          document.dispatchEvent(new Event("visibilitychange"));
        });
      } else {
        await page.clock.runFor(60_001);
      }
      expect(await finishRead(page)).toMatchObject({ ok: false, type: "AuthRenewalUnavailableError" });
      await expect.poll(() => disconnected).toBe(true);
      if (scenario !== "resume") {
        expect((await readCanonicalAuthSession(page))?.attempt?.error).toContain("timed out");
        await page.evaluate(() => {
          (window as unknown as ProbeWindow).authProbe.restoreFetch!();
          window.dispatchEvent(new Event("online"));
        });
      }
      if (scenario === "lost-response") {
        await expect.poll(async () => (await readCanonicalAuthSession(page))?.status).toBe("ended");
      } else {
        await expect.poll(async () => (await readCanonicalAuthSession(page))?.revision).toBe(1);
        expect((await readCanonicalAuthSession(page))?.attempt?.error).toBeNull();
        await startRead(page);
        expect((await finishRead(page)).ok).toBe(true);
      }
      expect(server.refreshes).toBe(1);
      await expect.poll(() => page.evaluate(() => navigator.locks.query())).toMatchObject({ held: [], pending: [] });
    } finally {
      http.closeAllConnections();
      await new Promise<void>(resolve => http.close(() => resolve()));
    }
  });
}
