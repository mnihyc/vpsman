import { expect, test, type Page } from "@playwright/test";
import { LatestReadConsumer } from "../src/api";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  openConsoleSubpage,
  waitForConsoleShell,
} from "./support/consoleNavigation";

type TrackedRequest = { method: string; url: string };

async function installVisibilityControl(page: Page, initiallyHidden: boolean) {
  await page.addInitScript((hiddenAtStartup) => {
    let hidden = hiddenAtStartup;
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => hidden,
    });
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => (hidden ? "hidden" : "visible"),
    });
    Object.defineProperty(window, "__vpsmanSetDocumentHidden", {
      configurable: true,
      value: (nextHidden: boolean) => {
        if (hidden === nextHidden) return;
        hidden = nextHidden;
        document.dispatchEvent(new Event("visibilitychange"));
      },
    });
  }, initiallyHidden);
}

async function setDocumentHidden(page: Page, hidden: boolean) {
  await page.evaluate((nextHidden) => {
    (
      window as typeof window & {
        __vpsmanSetDocumentHidden: (hidden: boolean) => void;
      }
    ).__vpsmanSetDocumentHidden(nextHidden);
  }, hidden);
}

async function clearTrackedRequests(page: Page) {
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanFetchRequests?: TrackedRequest[];
      }
    ).__vpsmanFetchRequests = [];
  });
}

async function trackedRequests(page: Page): Promise<TrackedRequest[]> {
  return page.evaluate(
    () =>
      (
        window as typeof window & {
          __vpsmanFetchRequests?: TrackedRequest[];
        }
      ).__vpsmanFetchRequests ?? [],
  );
}

function pathCount(requests: TrackedRequest[], pathname: string): number {
  return requests.filter(
    (request) =>
      request.method === "GET" &&
      new URL(request.url, "http://localhost").pathname === pathname,
  ).length;
}

function liveSnapshotCount(requests: TrackedRequest[]): number {
  return requests.filter((request) => {
    const url = new URL(request.url, "http://localhost");
    return (
      request.method === "GET" &&
      url.pathname === "/api/v1/fleet/snapshot" &&
      url.searchParams.get("mode") === "live"
    );
  }).length;
}

function fullSnapshotCount(requests: TrackedRequest[]): number {
  return requests.filter((request) => {
    const url = new URL(request.url, "http://localhost");
    return (
      request.method === "GET" &&
      url.pathname === "/api/v1/fleet/snapshot" &&
      url.searchParams.get("mode") === "full"
    );
  }).length;
}

test("latest desired read has one owner and resolves every coalesced producer", async () => {
  const consumer = new LatestReadConsumer<string>();
  const started: string[] = [];
  let active = 0;
  let maxActive = 0;
  let releaseFirst: (() => void) | undefined;

  const first = consumer.enqueue(async () => {
    started.push("first");
    active += 1;
    maxActive = Math.max(maxActive, active);
    await new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    active -= 1;
    return "first-result";
  });
  const replaced = consumer.enqueue(async () => {
    started.push("replaced");
    return "replaced-result";
  });
  const latest = consumer.enqueue(async () => {
    started.push("latest");
    active += 1;
    maxActive = Math.max(maxActive, active);
    active -= 1;
    return "latest-result";
  });

  expect(started).toEqual(["first"]);
  releaseFirst?.();
  await expect(first).resolves.toBe("first-result");
  await expect(replaced).resolves.toBe("latest-result");
  await expect(latest).resolves.toBe("latest-result");
  expect(started).toEqual(["first", "latest"]);
  expect(maxActive).toBe(1);
});

test("a tab hidden before mount defers its expensive Home bootstrap", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "visibility is viewport independent",
  );
  await installVisibilityControl(page, true);
  await installConsoleApiMock(page, { storedAuthSession: true });

  await page.goto("/");
  await waitForConsoleShell(page);
  await page.waitForTimeout(100);
  const hiddenRequests = await trackedRequests(page);
  for (const path of [
    "/api/v1/home/snapshot",
    "/api/v1/fleet/snapshot",
    "/api/v1/dashboard/overview",
    "/api/v1/monitoring/cards",
  ]) {
    expect(pathCount(hiddenRequests, path), `${path} while hidden`).toBe(0);
  }

  await setDocumentHidden(page, false);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);
  const visibleRequests = await trackedRequests(page);
  expect(pathCount(visibleRequests, "/api/v1/fleet/snapshot")).toBe(0);
  expect(pathCount(visibleRequests, "/api/v1/dashboard/overview")).toBe(0);
  expect(pathCount(visibleRequests, "/api/v1/monitoring/cards")).toBe(0);
});

test("hidden WS bursts, reconnect, and timers produce one visible catch-up per source", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "visibility is viewport independent",
  );
  await page.clock.install({ time: new Date("2026-06-02T10:02:00Z") });
  await installVisibilityControl(page, false);
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);

  await clearTrackedRequests(page);
  await setDocumentHidden(page, true);
  await page.evaluate(() => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: Array<EventTarget & { close?: () => void }>;
      }
    ).__vpsmanTestWebSockets.at(-1);
    for (let index = 0; index < 100; index += 1) {
      socket?.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({ type: "fleet_telemetry_invalidated" }),
        }),
      );
      socket?.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({ type: "agent_updated" }),
        }),
      );
    }
    socket?.close?.();
  });
  await page.clock.runFor(65_000);

  const whileHidden = await trackedRequests(page);
  expect(pathCount(whileHidden, "/api/v1/fleet/snapshot")).toBe(0);
  expect(pathCount(whileHidden, "/api/v1/dashboard/overview")).toBe(0);
  expect(pathCount(whileHidden, "/api/v1/monitoring/cards")).toBe(0);

  await setDocumentHidden(page, false);
  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return {
        cards: pathCount(requests, "/api/v1/monitoring/cards"),
        fleet: pathCount(requests, "/api/v1/fleet/snapshot"),
        overview: pathCount(requests, "/api/v1/dashboard/overview"),
      };
    })
    .toEqual({ cards: 1, fleet: 1, overview: 1 });

  const catchUpRequests = await trackedRequests(page);
  const cardsUrl = catchUpRequests
    .map((request) => new URL(request.url, "http://localhost"))
    .find((url) => url.pathname === "/api/v1/monitoring/cards");
  expect(cardsUrl?.searchParams.get("include_history")).toBe("false");
});

test("hide and show during a held Home bootstrap waits for Home before one catch-up", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "visibility is viewport independent",
  );
  await installVisibilityControl(page, false);
  await installConsoleApiMock(page, {
    holdInitialHomeSnapshot: true,
    storedAuthSession: true,
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);

  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await page.waitForTimeout(100);
  let requests = await trackedRequests(page);
  expect(pathCount(requests, "/api/v1/fleet/snapshot")).toBe(0);
  expect(pathCount(requests, "/api/v1/dashboard/overview")).toBe(0);
  expect(pathCount(requests, "/api/v1/monitoring/cards")).toBe(0);

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanReleaseHomeSnapshot: () => void }
    ).__vpsmanReleaseHomeSnapshot();
  });
  await expect
    .poll(async () => {
      const current = await trackedRequests(page);
      return {
        cards: pathCount(current, "/api/v1/monitoring/cards"),
        fleet: pathCount(current, "/api/v1/fleet/snapshot"),
        overview: pathCount(current, "/api/v1/dashboard/overview"),
      };
    })
    .toEqual({ cards: 1, fleet: 1, overview: 1 });
  requests = await trackedRequests(page);
  expect(pathCount(requests, "/api/v1/fleet/snapshot")).toBe(1);
  expect(pathCount(requests, "/api/v1/dashboard/overview")).toBe(1);
  const cards = requests
    .map((request) => new URL(request.url, "http://localhost"))
    .filter((url) => url.pathname === "/api/v1/monitoring/cards");
  expect(cards).toHaveLength(1);
  expect(cards[0]?.searchParams.get("include_history")).toBe("false");
});

test("an agent event during a held full snapshot schedules one trailing full refresh", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "coalescing is viewport independent",
  );
  await installConsoleApiMock(page, {
    holdInitialFleetSnapshots: true,
    storedAuthSession: true,
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);
  await clearTrackedRequests(page);

  const dispatchAgentUpdates = async (count: number) => {
    await page.evaluate((eventCount) => {
      const socket = (
        window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
      ).__vpsmanTestWebSockets.at(-1);
      for (let index = 0; index < eventCount; index += 1) {
        socket?.dispatchEvent(
          new MessageEvent("message", {
            data: JSON.stringify({ type: "agent_updated" }),
          }),
        );
      }
    }, count);
  };

  await dispatchAgentUpdates(1);
  await expect
    .poll(async () => fullSnapshotCount(await trackedRequests(page)))
    .toBe(1);
  await dispatchAgentUpdates(100);
  await page.waitForTimeout(850);
  expect(fullSnapshotCount(await trackedRequests(page))).toBe(1);
  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanReleaseFleetSnapshots: () => void }
    ).__vpsmanReleaseFleetSnapshots();
  });
  await expect
    .poll(async () => fullSnapshotCount(await trackedRequests(page)))
    .toBe(2);
  await page.waitForTimeout(100);
  expect(fullSnapshotCount(await trackedRequests(page))).toBe(2);
});

test("a WS burst during one live request schedules exactly one trailing refresh", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "coalescing is viewport independent",
  );
  await installConsoleApiMock(page, {
    holdInitialFleetSnapshots: true,
    storedAuthSession: true,
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);
  await clearTrackedRequests(page);

  await page.evaluate(() => {
    const socket = (
      window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ type: "fleet_telemetry_invalidated" }),
      }),
    );
  });
  await expect
    .poll(async () => liveSnapshotCount(await trackedRequests(page)))
    .toBe(1);

  await page.evaluate(() => {
    const socket = (
      window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
    ).__vpsmanTestWebSockets.at(-1);
    for (let index = 0; index < 100; index += 1) {
      socket?.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({ type: "fleet_telemetry_invalidated" }),
        }),
      );
    }
    (
      window as typeof window & { __vpsmanReleaseFleetSnapshots: () => void }
    ).__vpsmanReleaseFleetSnapshots();
  });

  await expect
    .poll(async () => liveSnapshotCount(await trackedRequests(page)))
    .toBe(2);
  await page.waitForTimeout(100);
  expect(liveSnapshotCount(await trackedRequests(page))).toBe(2);
});

test("overview producers share one request owner and retain one trailing latest read", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "coalescing is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);

  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    const releases: Array<() => void> = [];
    let active = 0;
    let calls = 0;
    let maxActive = 0;
    window.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      if (url.pathname !== "/api/v1/dashboard/overview") {
        return originalFetch(input, init);
      }
      calls += 1;
      active += 1;
      maxActive = Math.max(maxActive, active);
      return new Promise<Response>((resolve, reject) => {
        releases.push(() => {
          originalFetch(input, init)
            .finally(() => {
              active -= 1;
            })
            .then(resolve, reject);
        });
      });
    }) as typeof window.fetch;
    Object.assign(window, {
      __vpsmanOverviewConsumerStats: () => ({ active, calls, maxActive }),
      __vpsmanReleaseOverviewConsumer: () => releases.shift()?.(),
    });
  });

  const dispatchAgentUpdates = async (count: number) => {
    await page.evaluate((eventCount) => {
      const socket = (
        window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
      ).__vpsmanTestWebSockets.at(-1);
      for (let index = 0; index < eventCount; index += 1) {
        socket?.dispatchEvent(
          new MessageEvent("message", {
            data: JSON.stringify({ type: "agent_updated" }),
          }),
        );
      }
    }, count);
  };
  const stats = () =>
    page.evaluate(() =>
      (
        window as typeof window & {
          __vpsmanOverviewConsumerStats: () => {
            active: number;
            calls: number;
            maxActive: number;
          };
        }
      ).__vpsmanOverviewConsumerStats(),
    );

  await dispatchAgentUpdates(1);
  await expect.poll(async () => (await stats()).calls).toBe(1);
  await dispatchAgentUpdates(100);
  await page.waitForTimeout(350);
  expect(await stats()).toEqual({ active: 1, calls: 1, maxActive: 1 });

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanReleaseOverviewConsumer: () => void }
    ).__vpsmanReleaseOverviewConsumer();
  });
  await expect.poll(async () => (await stats()).calls).toBe(2);
  expect(await stats()).toEqual({ active: 1, calls: 2, maxActive: 1 });
  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanReleaseOverviewConsumer: () => void }
    ).__vpsmanReleaseOverviewConsumer();
  });
  await expect.poll(async () => (await stats()).active).toBe(0);
});

test("monitor cards warn only after a ten-second projection grace", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "refresh-health semantics are viewport independent",
  );
  await installVisibilityControl(page, false);
  const liveAgent = {
    capabilities: {
      can_apply_process_limits: true,
      can_attempt_privileged_ops: true,
      can_manage_runtime_tunnels: true,
      effective_uid: 0,
      privilege_mode: "root" as const,
      port_forwarding: {
        nft_version: "nftables v1.1.3",
        reason: null,
        status: "supported",
      },
      unprivileged_hint: null,
    },
    display_name: "edge-sfo-01",
    id: "agent-sfo-01",
    last_ip: "198.51.100.10",
    last_seen_at: "2026-06-05T20:35:00Z",
    registration_ip: "198.51.100.9",
    status: "offline",
    tags: ["country:US", "provider:alpha", "role:edge"],
  };
  await installConsoleApiMock(page, {
    agentListOverride: [liveAgent],
    storedAuthSession: true,
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Monitor");
  await page
    .getByLabel("VPS cards density")
    .getByRole("button", { name: "Comfortable" })
    .click();
  const card = page
    .getByLabel("VPS monitor cards")
    .locator(".vpsMonitorCard")
    .first();
  await expect(card).toBeVisible();
  await expect(card.locator(".telemetryEvidence")).not.toHaveClass(/delayed/);

  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    Object.defineProperty(window, "__vpsmanProjectionRefreshes", {
      configurable: true,
      value: 0,
      writable: true,
    });
    Object.defineProperty(window, "__vpsmanFailProjectionRefresh", {
      configurable: true,
      value: false,
      writable: true,
    });
    window.fetch = async (input, init) => {
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const state = window as typeof window & {
        __vpsmanFailProjectionRefresh: boolean;
        __vpsmanProjectionRefreshes: number;
      };
      if (
        url.pathname === "/api/v1/monitoring/cards" &&
        state.__vpsmanFailProjectionRefresh
      ) {
        return new Response("monitoring refresh unavailable", { status: 503 });
      }
      const response = await originalFetch(input, init);
      if (url.pathname !== "/api/v1/monitoring/cards" || !response.ok) {
        return response;
      }
      state.__vpsmanProjectionRefreshes += 1;
      const body = (await response.json()) as {
        items: Array<{
          client: {
            capabilities: { privilege_mode: string };
            display_name: string;
            last_seen_at: string | null;
            stale_reason?: string | null;
            stale_since?: string | null;
            status: string;
            tags: string[];
          };
          projection_pending_since?: string | null;
          projection_checked_at?: string | null;
          network: unknown[];
          resources: {
            latest_observed_at: string;
            updated_at: string;
          } | null;
        }>;
      };
      const refresh = state.__vpsmanProjectionRefreshes;
      for (const item of body.items) {
        // Simulate a held card whose core fields predate the current fleet
        // agent. Only its liveness is card-owned.
        item.client.capabilities.privilege_mode = "unknown";
        item.client.display_name = "held-home-edge";
        item.client.tags = ["country:ZZ", "provider:held"];
        item.client.status = "online";
        item.client.last_seen_at = "2026-06-05T20:45:00Z";
        item.client.stale_reason = null;
        item.client.stale_since = null;
        item.projection_pending_since =
          refresh < 4 ? "2026-06-05T20:35:00Z" : null;
        item.projection_checked_at =
          refresh === 1
            ? "2026-06-05T20:35:09.999Z"
            : refresh === 2
              ? "2026-06-05T20:35:10.001Z"
              : "2026-06-05T20:45:00Z";
        if (refresh <= 2) {
          // First-ever telemetry remains unavailable through the exact grace
          // boundary, then becomes delayed only once the boundary is crossed.
          item.resources = null;
          item.network = [];
        } else if (refresh === 3) {
          // Partial observed evidence does not hide an overdue projection.
          item.resources = null;
        }
        if (item.resources) {
          if (refresh >= 4) {
            // A projected out-of-order envelope need not replace the current
            // resource row; clearing the pending suffix is sufficient.
            item.resources.updated_at = "2026-06-05T20:35:00Z";
          }
        }
      }
      return new Response(JSON.stringify(body), {
        headers: response.headers,
        status: response.status,
        statusText: response.statusText,
      });
    };
  });

  const expectProjectionRefreshes = async (expected: number) => {
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (
              window as typeof window & {
                __vpsmanProjectionRefreshes: number;
              }
            ).__vpsmanProjectionRefreshes,
        ),
      )
      .toBe(expected);
  };

  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expectProjectionRefreshes(1);
  await expect(card.locator(".telemetryEvidence")).not.toHaveClass(/delayed/);
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Telemetry unavailable",
  );
  await expect(card.getByText("edge-sfo-01", { exact: true })).toBeVisible();
  await expect(card).not.toContainText("held-home-edge");
  const identity = card.locator(".vpsMonitorCardMain > small");
  await expect(identity).toContainText("alpha");
  await expect(identity).toContainText("/ US");
  await expect(identity).not.toContainText("held");
  await expect(identity).not.toContainText("/ ZZ");
  await expect(
    card.getByText("Online · Warning", { exact: true }),
  ).toBeVisible();
  const cardContact = await page.evaluate(
    (value) => `Last contact ${new Date(value).toLocaleString()}`,
    "2026-06-05T20:45:00Z",
  );
  expect(
    await card.locator(".vpsMonitorStatus").getAttribute("title"),
  ).toContain(cardContact);

  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expectProjectionRefreshes(2);
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Telemetry delayed",
  );
  await expect(card.locator(".telemetryEvidence")).toHaveClass(/delayed/);
  await expect(card).toHaveClass(/warning/);

  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expectProjectionRefreshes(3);
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Telemetry delayed",
  );
  await expect(card.locator(".telemetryEvidence")).toHaveClass(/delayed/);

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanFailProjectionRefresh: boolean }
    ).__vpsmanFailProjectionRefresh = true;
  });
  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Monitoring refresh failed",
  );
  await expect(card.locator(".telemetryEvidence")).not.toContainText(
    "Telemetry delayed",
  );
  await expect(card.locator(".telemetryEvidence")).toHaveClass(/untrusted/);

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanFailProjectionRefresh: boolean }
    ).__vpsmanFailProjectionRefresh = false;
  });
  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expectProjectionRefreshes(4);
  await expect(card.locator(".telemetryEvidence")).not.toHaveClass(
    /delayed|untrusted/,
  );
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Telemetry current",
  );
  const realtimeSpeed = page
    .getByLabel("VPS cards current totals")
    .getByText("Realtime speed", { exact: true })
    .locator("..");
  const locationTotal = page
    .getByLabel("VPS cards current totals")
    .getByText("Locations", { exact: true })
    .locator("..");
  const trafficTotal = page
    .getByLabel("VPS cards current totals")
    .getByText("Traffic", { exact: true })
    .locator("..");
  const cardRxRate = card
    .getByLabel("Current network activity for edge-sfo-01")
    .getByText("Network RX", { exact: true })
    .locator("..")
    .locator("strong");
  await expect(realtimeSpeed).toContainText("1 fresh");
  await expect(realtimeSpeed).not.toContainText("Monitoring refresh failed");
  await expect(locationTotal.locator("strong")).toHaveText("1");
  await expect(locationTotal.locator("em")).toHaveText("US");
  await expect(trafficTotal.locator("em")).toHaveText("1 configured VPS");
  await expect(cardRxRate).not.toHaveText("-");
  const trafficBytes = await trafficTotal.locator("strong").innerText();
  const lastKnownCardRxRate = await cardRxRate.innerText();

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanFailProjectionRefresh: boolean }
    ).__vpsmanFailProjectionRefresh = true;
  });
  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Monitoring refresh failed",
  );
  await expect(card.locator(".telemetryEvidence")).not.toContainText(
    "Telemetry delayed",
  );
  await expect(card.locator(".telemetryEvidence")).toHaveClass(/untrusted/);
  await expect(card).toHaveClass(/warning/);
  await expect(realtimeSpeed).toContainText("↓ -");
  await expect(realtimeSpeed).toContainText("↑ -");
  await expect(realtimeSpeed).toContainText("Monitoring refresh failed");
  await expect(realtimeSpeed).not.toContainText(/\d+ fresh/);
  await expect(realtimeSpeed).toHaveAttribute(
    "title",
    /Last-known card rates remain visible but are not aggregated as current/,
  );
  await expect(locationTotal.locator("strong")).toHaveText("1");
  await expect(locationTotal.locator("em")).toHaveText("US");
  await expect(trafficTotal.locator("strong")).toHaveText(trafficBytes);
  await expect(trafficTotal.locator("em")).toHaveText("1 configured VPS");
  await expect(cardRxRate).toBeVisible();
  await expect(cardRxRate).toHaveText(lastKnownCardRxRate);

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanFailProjectionRefresh: boolean }
    ).__vpsmanFailProjectionRefresh = false;
  });
  await setDocumentHidden(page, true);
  await setDocumentHidden(page, false);
  await expectProjectionRefreshes(5);
  await expect(card.locator(".telemetryEvidence")).not.toHaveClass(
    /delayed|untrusted/,
  );
  await expect(card.locator(".telemetryEvidence")).toContainText(
    "Telemetry current",
  );
  await expect(realtimeSpeed).toContainText("1 fresh");
  await expect(realtimeSpeed).not.toContainText("Monitoring refresh failed");
  await expect(locationTotal.locator("strong")).toHaveText("1");
  await expect(locationTotal.locator("em")).toHaveText("US");
  await expect(trafficTotal.locator("strong")).toHaveText(trafficBytes);
  await expect(trafficTotal.locator("em")).toHaveText("1 configured VPS");
  await expect(cardRxRate).toHaveText(lastKnownCardRxRate);
});
