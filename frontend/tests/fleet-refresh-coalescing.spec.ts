import { expect, test, type Page } from "@playwright/test";
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

test("exhausted heavy-read admission retries preserve the last known fleet view", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "retry semantics are viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await expect
    .poll(async () =>
      pathCount(await trackedRequests(page), "/api/v1/home/snapshot"),
    )
    .toBe(1);
  await openConsoleSubpage(page, "Fleet", "Alerts");
  const grid = page.getByLabel("Current alert episodes data grid");
  await expect(grid).toContainText("Tunnel adapter status failed");

  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    Object.defineProperty(window, "__vpsmanHeavyReadFailures", {
      configurable: true,
      value: 0,
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
        __vpsmanHeavyReadFailures: number;
      };
      if (
        url.pathname === "/api/v1/fleet/snapshot" &&
        url.searchParams.get("mode") === "full" &&
        state.__vpsmanHeavyReadFailures < 4
      ) {
        state.__vpsmanHeavyReadFailures += 1;
        return new Response(
          JSON.stringify({
            error: "heavy_read_admission_busy",
            message: "Heavy read admission is busy",
            status: 429,
          }),
          {
            headers: { "content-type": "application/json" },
            status: 429,
          },
        );
      }
      return originalFetch(input, init);
    };
  });
  await page.evaluate(() => {
    const socket = (
      window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ type: "agent_updated" }),
      }),
    );
  });

  await expect
    .poll(
      () =>
        page.evaluate(
          () =>
            (
              window as typeof window & {
                __vpsmanHeavyReadFailures: number;
              }
            ).__vpsmanHeavyReadFailures,
        ),
      { timeout: 8_000 },
    )
    .toBe(4);
  await expect(grid).toContainText("Tunnel adapter status failed");
});
