import { expect, test, type Page } from "@playwright/test";
import { LatestReadConsumer } from "../src/api";
import {
  backupId,
  installConsoleApiMock,
} from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
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

test("Network overview activation has one bounded exact subpage owner", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "request ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await clearTrackedRequests(page);

  await openConsoleSubpage(page, "Network", "Overview");
  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return {
        graph: pathCount(requests, "/api/v1/network/topology-graph") > 0,
        ospfPlans:
          pathCount(requests, "/api/v1/network/ospf-update-plans") > 0,
        tunnelPlans: pathCount(requests, "/api/v1/tunnel-plans") > 0,
      };
    })
    .toEqual({ graph: true, ospfPlans: true, tunnelPlans: true });

  const requests = await trackedRequests(page);
  // StrictMode replays the sole activation effect in the development harness;
  // each exact source still remains within one active and one trailing read.
  for (const path of [
    "/api/v1/network/topology-graph",
    "/api/v1/network/ospf-update-plans",
    "/api/v1/tunnel-plans",
  ]) {
    expect(pathCount(requests, path)).toBeLessThanOrEqual(2);
  }
  expect(
    [
      "/api/v1/network-adapter-definitions",
      "/api/v1/network/observations",
      "/api/v1/network/observation-trends",
      "/api/v1/network/ospf-recommendations",
      "/api/v1/port-forward-rules",
      "/api/v1/runtime-config/apply-state",
    ].map((path) => [path, pathCount(requests, path)]),
  ).toEqual([
    ["/api/v1/network-adapter-definitions", 0],
    ["/api/v1/network/observations", 0],
    ["/api/v1/network/observation-trends", 0],
    ["/api/v1/network/ospf-recommendations", 0],
    ["/api/v1/port-forward-rules", 0],
    ["/api/v1/runtime-config/apply-state", 0],
  ]);
});

test("hidden Network route visits hydrate the final exact owner once", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "visibility is viewport independent",
  );
  await installVisibilityControl(page, false);
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Network", "Overview");
  await clearTrackedRequests(page);

  await setDocumentHidden(page, true);
  await openConsoleSubpage(page, "Network", "Graph");
  await openConsoleSubpage(page, "Network", "Overview");
  await page.waitForTimeout(100);
  let requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/network/topology-graph",
    "/api/v1/network/ospf-update-plans",
    "/api/v1/tunnel-plans",
  ]) {
    expect(pathCount(requests, path), `${path} stayed hidden`).toBe(0);
  }

  await setDocumentHidden(page, false);
  await expect
    .poll(async () => {
      const visibleRequests = await trackedRequests(page);
      return {
        graph: pathCount(visibleRequests, "/api/v1/network/topology-graph") > 0,
        ospfPlans:
          pathCount(visibleRequests, "/api/v1/network/ospf-update-plans") > 0,
        tunnelPlans:
          pathCount(visibleRequests, "/api/v1/tunnel-plans") > 0,
      };
    })
    .toEqual({ graph: true, ospfPlans: true, tunnelPlans: true });
  requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/network/topology-graph",
    "/api/v1/network/ospf-update-plans",
    "/api/v1/tunnel-plans",
  ]) {
    expect(pathCount(requests, path)).toBeLessThanOrEqual(2);
  }
  expect(pathCount(requests, "/api/v1/runtime-config/apply-state")).toBe(0);
});

test("Network manual refresh retries each exact button-owned source", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "request ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);

  await openConsoleSubpage(page, "Network", "Evidence");
  const evidence = page.locator(".topologyEvidence");
  await evidence.getByText("Advanced filters", { exact: true }).click();
  const applyFilters = evidence.getByRole("button", {
    name: "Apply filters",
    exact: true,
  });
  await expect(applyFilters).toBeEnabled();
  await clearTrackedRequests(page);
  await applyFilters.click();
  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return [
        "/api/v1/network/observations",
        "/api/v1/network/observation-trends",
        "/api/v1/network/ospf-recommendations",
        "/api/v1/network/ospf-update-plans",
      ].map((path) => pathCount(requests, path));
    })
    .toEqual([1, 1, 1, 1]);
  let requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/jobs",
    "/api/v1/tunnel-plans",
    "/api/v1/network/topology-graph",
    "/api/v1/network-adapter-definitions",
    "/api/v1/configuration-sources",
    "/api/v1/port-forward-rules",
    "/api/v1/runtime-config/apply-state",
  ]) {
    expect(pathCount(requests, path), `${path} is outside filter apply`).toBe(0);
  }

  const refreshEvidence = evidence
    .getByRole("button", { name: "Refresh evidence", exact: true });
  await expect(refreshEvidence).toBeEnabled();
  await clearTrackedRequests(page);
  await refreshEvidence.click();
  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return [
        "/api/v1/network/observations",
        "/api/v1/network/observation-trends",
        "/api/v1/network/ospf-recommendations",
        "/api/v1/network/ospf-update-plans",
        "/api/v1/jobs",
      ].map((path) => pathCount(requests, path));
    })
    .toEqual([1, 1, 1, 1, 1]);
  requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/tunnel-plans",
    "/api/v1/network/topology-graph",
    "/api/v1/network-adapter-definitions",
    "/api/v1/configuration-sources",
    "/api/v1/port-forward-rules",
    "/api/v1/runtime-config/apply-state",
  ]) {
    expect(pathCount(requests, path), `${path} is outside evidence refresh`).toBe(
      0,
    );
  }

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  const refreshTunnelPlans = page
    .locator(".tunnelPlanRegistry")
    .getByRole("button", { name: "Refresh", exact: true });
  await expect(refreshTunnelPlans).toBeEnabled();
  await clearTrackedRequests(page);
  await refreshTunnelPlans.click();
  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return [
        "/api/v1/tunnel-plans",
        "/api/v1/network/topology-graph",
        "/api/v1/network-adapter-definitions",
        "/api/v1/configuration-sources",
      ].map((path) => pathCount(requests, path));
    })
    .toEqual([1, 1, 1, 1]);
  requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/jobs",
    "/api/v1/network/observations",
    "/api/v1/network/observation-trends",
    "/api/v1/network/ospf-recommendations",
    "/api/v1/network/ospf-update-plans",
    "/api/v1/port-forward-rules",
    "/api/v1/runtime-config/apply-state",
  ]) {
    expect(pathCount(requests, path), `${path} is outside tunnel refresh`).toBe(
      0,
    );
  }
});

test("topology source failures stay with their exact route consumers", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "source ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method === "GET" && url.pathname === "/api/v1/tunnel-plans") {
        return new Response(
          JSON.stringify({
            error: "topology_scope_probe",
            message: "Tunnel source failed",
          }),
          { headers: { "Content-Type": "application/json" }, status: 503 },
        );
      }
      return originalFetch(input, init);
    };
  });

  await openConsoleSubpage(page, "Network", "Tests");
  await expect(page.getByText("Tunnel plans unavailable", { exact: true })).toBeVisible();
  await expect(page.getByText(/Topology Scope Probe: Tunnel source failed/i)).toBeVisible();

  await openConsoleSubpage(page, "Network", "Graph");
  await expect(page.getByText(/Topology Scope Probe: Tunnel source failed/i)).toHaveCount(0);

  await openConsoleSubpage(page, "Network", "OSPF");
  await expect(page.getByText(/Topology Scope Probe: Tunnel source failed/i)).toBeVisible();
});

test("bursty unknown job completions share history ownership and retain every projection classification", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "request ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Network", "Overview");
  await clearTrackedRequests(page);
  await page.evaluate(() => {
    const commandTypes = [
      "runtime_config_sync",
      "network_probe",
      "network_routing_apply",
    ];
    const records = Array.from({ length: 24 }, (_, index) => ({
      actor_id: null,
      command_type: commandTypes[index % commandTypes.length],
      completed_at: new Date(
        Date.parse("2026-06-02T10:00:00Z") + index * 1_000,
      ).toISOString(),
      created_at: new Date(
        Date.parse("2026-06-02T09:59:00Z") + index * 1_000,
      ).toISOString(),
      id: `40000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
      max_timeout_secs: 60,
      payload_hash: String(index).padStart(64, "0"),
      privileged: false,
      source_schedule_id: null,
      status: "completed",
      target_count: 1,
    }));
    const originalFetch = window.fetch.bind(window);
    const state: {
      historyGets: number;
      itemGets: number;
      releaseFirst: (() => void) | null;
    } = { historyGets: 0, itemGets: 0, releaseFirst: null };
    Object.defineProperty(window, "__vpsmanJobEventBurst", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method === "GET" && url.pathname === "/api/v1/jobs") {
        state.historyGets += 1;
        if (state.historyGets === 1) {
          await new Promise<void>((resolve) => {
            state.releaseFirst = resolve;
          });
        }
        return new Response(JSON.stringify(records), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }
      if (method === "GET" && /^\/api\/v1\/jobs\/[^/]+$/.test(url.pathname)) {
        state.itemGets += 1;
      }
      return originalFetch(input, init);
    };
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: EventTarget[];
      }
    ).__vpsmanTestWebSockets.at(-1);
    for (const record of records) {
      socket?.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({
            job_id: record.id,
            status: "completed",
            type: "job_finished",
          }),
        }),
      );
    }
  });

  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobEventBurst: { historyGets: number };
            }
          ).__vpsmanJobEventBurst.historyGets,
      ),
    )
    .toBe(1);
  await page.evaluate(() => {
    const state = (
      window as typeof window & {
        __vpsmanJobEventBurst: { releaseFirst: (() => void) | null };
      }
    ).__vpsmanJobEventBurst;
    state.releaseFirst?.();
    state.releaseFirst = null;
  });

  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return {
        graph: pathCount(requests, "/api/v1/network/topology-graph") > 0,
        ospfPlans:
          pathCount(requests, "/api/v1/network/ospf-update-plans") > 0,
      };
    })
    .toEqual({ graph: true, ospfPlans: true });
  const requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/network/topology-graph",
    "/api/v1/network/ospf-update-plans",
  ]) {
    expect(pathCount(requests, path)).toBeLessThanOrEqual(2);
  }
  for (const path of [
    "/api/v1/runtime-config/apply-state",
    "/api/v1/network/observations",
    "/api/v1/network/observation-trends",
    "/api/v1/network/ospf-recommendations",
  ]) {
    expect(pathCount(requests, path), `${path} is not rendered`).toBe(0);
  }
  const requestState = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __vpsmanJobEventBurst: { historyGets: number; itemGets: number };
        }
      ).__vpsmanJobEventBurst,
  );
  expect(requestState.historyGets).toBeLessThanOrEqual(2);
  expect(requestState.itemGets).toBe(0);
});

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

test("Access exact mutation survives an older aggregate and a rejected source", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "Operators");

  const operatorId = "99999999-aaaa-4bbb-8ccc-000000000002";
  const operatorGrid = page.getByLabel("Operator accounts data grid");
  const operatorRow = operatorGrid
    .locator(".gridBody [role=row]", { hasText: "noc-operator" })
    .first();
  await expect(operatorRow).toContainText(/active/i);

  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    let releaseHeldRead = () => undefined;
    const heldRead = new Promise<void>((resolve) => {
      releaseHeldRead = resolve;
    });
    const state = {
      failNext: false,
      heldStarted: 0,
      release: () => releaseHeldRead(),
    };
    Object.defineProperty(window, "__vpsmanAccessProjectionRace", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method !== "GET" || url.pathname !== "/api/v1/operators") {
        return originalFetch(input, init);
      }
      if (state.failNext) {
        state.failNext = false;
        return new Response(
          JSON.stringify({
            error: "operator_source_unavailable",
            message: "Operator source unavailable",
          }),
          { headers: { "Content-Type": "application/json" }, status: 503 },
        );
      }
      if (state.heldStarted > 0) {
        return originalFetch(input, init);
      }
      // Capture the old projection before waiting. The mutation below changes
      // the fixture while this exact stale response remains in flight.
      const response = await originalFetch(input, init);
      const body = await response.text();
      state.heldStarted += 1;
      await heldRead;
      return new Response(body, {
        headers: response.headers,
        status: response.status,
        statusText: response.statusText,
      });
    };
  });

  await openConsoleSubpage(page, "Fleet", "Instances");
  const returnToAccess = openConsoleSubpage(page, "Access", "Operators");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanAccessProjectionRace: { heldStarted: number };
            }
          ).__vpsmanAccessProjectionRace.heldStarted,
      ),
    )
    .toBe(1);

  await operatorGrid
    .getByLabel(`Select Operator accounts row ${operatorId}`)
    .check();
  await operatorGrid
    .locator(".gridToolbarActions")
    .getByRole("button", { name: "Actions", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Disable", exact: true }).click();
  await activate(
    page
      .getByLabel("Confirm user action")
      .getByRole("button", { name: "Disable user", exact: true }),
  );
  await expect(operatorRow).toContainText(/disabled/i);

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanAccessProjectionRace: { release: () => void };
      }
    ).__vpsmanAccessProjectionRace.release();
  });
  await returnToAccess;
  await expect(operatorRow).toContainText(/disabled/i);

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanAccessProjectionRace: { failNext: boolean };
      }
    ).__vpsmanAccessProjectionRace.failNext = true;
  });
  await openConsoleSubpage(page, "Fleet", "Instances");
  await openConsoleSubpage(page, "Access", "Operators");
  await expect(operatorRow).toContainText(/disabled/i);
  await expect(
    page.getByText(/Operators: .*Operator source unavailable/i),
  ).toBeVisible();
});

test("Access manual refresh follows the canonical visible projection", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Access", "VPS identities");
  const accessPanel = page.locator(".accessMain");
  const refresh = accessPanel.getByRole("button", {
    name: "Refresh",
    exact: true,
  });
  await expect(refresh).toBeEnabled();
  await clearTrackedRequests(page);
  await refresh.click();
  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return {
        keyLifecycle:
          pathCount(requests, "/api/v1/key-lifecycle/report") > 0,
        revocations:
          pathCount(requests, "/api/v1/client-key-revocations") > 0,
      };
    })
    .toEqual({ keyLifecycle: true, revocations: true });
  const requests = await trackedRequests(page);
  for (const path of [
    "/api/v1/operators",
    "/api/v1/operator-sessions",
    "/api/v1/gateway-sessions",
    "/api/v1/terminal-sessions",
  ]) {
    expect(pathCount(requests, path), `${path} is outside VPS identities`).toBe(
      0,
    );
  }
});

test("Backups restore and migration surface their file-transfer owner status", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "source ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    const state: {
      failNext: boolean;
      held: boolean;
      release: (() => void) | null;
    } = { failNext: false, held: false, release: null };
    Object.defineProperty(window, "__vpsmanBackupTransferStatus", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method !== "GET" || url.pathname !== "/api/v1/file-transfers") {
        return originalFetch(input, init);
      }
      if (state.failNext) {
        state.failNext = false;
        return new Response(
          JSON.stringify({
            error: "file_transfer_probe",
            message: "Transfer source failed",
          }),
          { headers: { "Content-Type": "application/json" }, status: 503 },
        );
      }
      if (!state.held) {
        state.held = true;
        await new Promise<void>((resolve) => {
          state.release = resolve;
        });
      }
      return originalFetch(input, init);
    };
  });

  const openingRestore = openConsoleSubpage(page, "Backups", "Restore");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanBackupTransferStatus: { held: boolean };
            }
          ).__vpsmanBackupTransferStatus.held,
      ),
    )
    .toBe(true);
  await openingRestore;
  const backupWorkspace = page.locator(".backupWorkspace");
  const refresh = backupWorkspace.getByRole("button", {
    name: "Refresh",
    exact: true,
  });
  await expect(refresh).toBeDisabled();
  await expect(refresh).toHaveAttribute(
    "title",
    "Restore operations refresh is already in progress",
  );
  await expect(backupWorkspace.getByText("Loading restore sources")).toBeVisible();
  await page.evaluate(() => {
    const state = (
      window as typeof window & {
        __vpsmanBackupTransferStatus: { release: (() => void) | null };
      }
    ).__vpsmanBackupTransferStatus;
    state.release?.();
    state.release = null;
  });
  await expect(refresh).toBeEnabled();

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanBackupTransferStatus: { failNext: boolean };
      }
    ).__vpsmanBackupTransferStatus.failNext = true;
  });
  await openConsoleSubpage(page, "Backups", "Migration");
  await expect(
    page.getByText(/File transfer sessions: .*Transfer source failed/i),
  ).toBeVisible();
  await expect(
    page
      .locator(".backupWorkspace")
      .getByRole("button", { name: "Refresh", exact: true }),
  ).toBeEnabled();
});

test("committed backup prune reconciles request linkage and artifacts once", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, {
    storedAuthSession: true,
    backupPoliciesOverride: [
      {
        cadence_error: null,
        catch_up_limit: 1,
        catch_up_policy: "skip_missed",
        created_at: "2026-01-01T00:00:00Z",
        cron_expr: "0 2 * * *",
        enabled: true,
        failure_count: 0,
        follow_symlinks: false,
        include_config: true,
        keep_last: 7,
        last_error: null,
        last_run_at: null,
        max_failures: 3,
        missing_path_policy: "skip",
        name: "nightly-system",
        next_run_at: "2026-01-02T02:00:00Z",
        next_runs: ["2026-01-02T02:00:00Z"],
        paths: ["/etc"],
        retention_days: 30,
        retry_delay_secs: 300,
        rotation_generation: null,
        schedule_id: "62626262-6161-4717-8abc-defdefdefdef",
        selector_expression: "id:agent-sfo-01",
        target_client_ids: ["agent-sfo-01"],
        timezone: "UTC",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ],
  });
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    const state = { applied: false, artifactGets: 0, requestGets: 0 };
    Object.defineProperty(window, "__vpsmanBackupPruneProjection", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (
        method === "POST" &&
        url.pathname === "/api/v1/backup-policies/prune"
      ) {
        const body = request
          ? ((await request.clone().json()) as {
              confirmed?: boolean;
              dry_run?: boolean;
            })
          : (JSON.parse(String(init?.body ?? "{}")) as {
              confirmed?: boolean;
              dry_run?: boolean;
            });
        const response = await originalFetch(input, init);
        if (body.confirmed && !body.dry_run) state.applied = true;
        return response;
      }
      if (
        state.applied &&
        method === "GET" &&
        url.pathname === "/api/v1/backups"
      ) {
        state.requestGets += 1;
        return new Response(
          JSON.stringify([
            {
              actor_id: null,
              artifact_id: null,
              client_id: "agent-sfo-01",
              command_scope: "client:agent-sfo-01",
              created_at: "2026-05-31T10:00:00Z",
              follow_symlinks: false,
              id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
              include_config: false,
              note: "fixture backup",
              paths: ["/etc/hostname"],
              payload_hash: "a".repeat(64),
              source_job_id: "77777777-aaaa-4bbb-8ccc-dddddddddddd",
              source_schedule_id: null,
              status: "requested_metadata_only",
            },
          ]),
          { headers: { "Content-Type": "application/json" }, status: 200 },
        );
      }
      if (
        state.applied &&
        method === "GET" &&
        url.pathname === "/api/v1/backup-artifacts"
      ) {
        state.artifactGets += 1;
        return new Response("[]", {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }
      return originalFetch(input, init);
    };
  });

  await openConsoleSubpage(page, "Backups", "Policies");
  await activate(page.getByRole("button", { name: "Prune policies" }));
  await page.getByLabel("Dry run").uncheck();
  await activate(page.getByRole("button", { name: "Review prune apply" }));
  await activate(
    page
      .getByLabel("Confirm policy prune apply")
      .getByRole("button", { name: "Apply prune" }),
  );
  await expect(page.getByText("3 pruned", { exact: true })).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanBackupPruneProjection: {
                artifactGets: number;
                requestGets: number;
              };
            }
          ).__vpsmanBackupPruneProjection,
      ),
    )
    .toMatchObject({ artifactGets: 1, requestGets: 1 });

  await openConsoleSubpage(page, "Backups", "Requests");
  const requestGrid = page.getByLabel("Backup request records data grid");
  await expect(requestGrid).toContainText(/requested metadata only/i);
  await expect(requestGrid).toContainText("No package");
});

test("artifact handoff reconciles its changed backup request once without rereading artifacts", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate((requestId) => {
    const originalFetch = window.fetch.bind(window);
    const state = { artifactGets: 0, handoffCommitted: false, requestGets: 0 };
    Object.defineProperty(window, "__vpsmanBackupArtifactProjection", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      const response = await originalFetch(input, init);
      if (
        method === "POST" &&
        /^\/api\/v1\/backups\/[^/]+\/artifact-handoff$/.test(url.pathname)
      ) {
        state.handoffCommitted = true;
        return response;
      }
      if (method === "GET" && url.pathname === "/api/v1/backups") {
        if (state.handoffCommitted) state.requestGets += 1;
        const records = (await response.clone().json()) as Array<
          Record<string, unknown>
        >;
        return new Response(
          JSON.stringify(
            records.map((record) =>
              record.id === requestId
                ? {
                    ...record,
                    artifact_id: state.handoffCommitted
                      ? "dddddddd-eeee-4fff-8000-111111111111"
                      : null,
                    status: state.handoffCommitted
                      ? "artifact_metadata_recorded"
                      : "requested_metadata_only",
                  }
                : record,
            ),
          ),
          {
            headers: response.headers,
            status: response.status,
            statusText: response.statusText,
          },
        );
      }
      if (method === "GET" && url.pathname === "/api/v1/backup-artifacts") {
        if (state.handoffCommitted) state.artifactGets += 1;
        return new Response("[]", {
          headers: response.headers,
          status: response.status,
          statusText: response.statusText,
        });
      }
      return response;
    };
  }, backupId);

  await openConsoleSubpage(page, "Backups", "Artifacts");
  await clearTrackedRequests(page);
  await activate(page.getByRole("button", { name: "Open artifact workflow" }));
  const workflow = page.getByLabel("Open artifact workflow");
  await workflow.getByLabel("Artifact backup request").selectOption(backupId);
  await workflow
    .getByLabel("Backup artifact transfer package source job ID")
    .fill("99999999-2222-4333-8444-555555555555");
  await activate(
    workflow.getByRole("button", { name: "Review transfer package" }),
  );
  await activate(
    workflow
      .getByLabel("Confirm backup artifact transfer package")
      .getByRole("button", { name: "Create transfer package" }),
  );
  await expect(page.getByText(/Artifact dddddddd ready/)).toBeVisible();

  await expect
    .poll(() =>
      page.evaluate(() => {
        const state = (
          window as typeof window & {
            __vpsmanBackupArtifactProjection: {
              artifactGets: number;
              requestGets: number;
            };
          }
        ).__vpsmanBackupArtifactProjection;
        return {
          artifactGets: state.artifactGets,
          requestGets: state.requestGets,
        };
      }),
    )
    .toEqual({ artifactGets: 0, requestGets: 1 });
  await openConsoleSubpage(page, "Backups", "Requests");
  const requestGrid = page.getByLabel("Backup request records data grid");
  await expect(requestGrid).toContainText("verified package available");
  await expect(requestGrid).toContainText("dddddddd");

  const requests = await trackedRequests(page);
  expect(pathCount(requests, "/api/v1/backups")).toBe(1);
  expect(pathCount(requests, "/api/v1/backup-artifacts")).toBe(0);
  expect(fullSnapshotCount(requests)).toBe(0);
});

test("artifact arrival reconciles only the coupled request and artifact projections", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Jobs", "Artifacts");
  await expect(
    page.getByLabel("Job artifact inventory data grid"),
  ).toBeVisible();
  await clearTrackedRequests(page);

  await page.evaluate((requestId) => {
    const socket = (
      window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          artifact_id: "dddddddd-eeee-4fff-8000-111111111111",
          backup_request_id: requestId,
          client_id: "agent-sfo-01",
          type: "backup_artifact_recorded",
        }),
      }),
    );
  }, backupId);

  await expect
    .poll(async () => {
      const requests = await trackedRequests(page);
      return {
        artifacts: pathCount(requests, "/api/v1/backup-artifacts"),
        requests: pathCount(requests, "/api/v1/backups"),
      };
    })
    .toEqual({ artifacts: 1, requests: 1 });
  const requests = await trackedRequests(page);
  expect(pathCount(requests, "/api/v1/backup-policies")).toBe(0);
  expect(pathCount(requests, "/api/v1/restore-plans")).toBe(0);
  expect(pathCount(requests, "/api/v1/migration-links")).toBe(0);
  expect(fullSnapshotCount(requests)).toBe(0);
});

test("resolved occurrences commit their returned projections without a full fleet snapshot", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Fleet", "Alerts");

  const current = page.getByLabel("Current alert episodes data grid");
  const incident = current
    .getByRole("row")
    .filter({ hasText: "Backup request failed" })
    .first();
  await incident.getByRole("checkbox").check();
  await current.getByRole("button", { name: "Actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Resolve incident" }).click();
  const prompt = page.getByLabel("Confirm incident resolution");
  await prompt
    .getByLabel("Incident resolution reason")
    .fill("Replacement backup verified.");
  await clearTrackedRequests(page);
  await activate(prompt.getByRole("button", { name: "Resolve incident" }));

  await expect(current).not.toContainText("Backup request failed");
  await expect(
    page.getByLabel("Alert episode history data grid"),
  ).toContainText("Replacement backup verified.");
  const requests = await trackedRequests(page);
  expect(fullSnapshotCount(requests)).toBe(0);
  expect(
    requests.filter((request) => {
      const url = new URL(request.url, "http://localhost");
      return (
        request.method === "POST" &&
        url.pathname === "/api/v1/fleet-alerts/resolve"
      );
    }),
  ).toHaveLength(1);
});

test("committed notification dispatch keeps its exact delivery projection bounded without a refresh", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Observability", "Alerts");
  await activate(page.getByRole("tab", { name: /Destinations/ }));

  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      const response = await originalFetch(input, init);
      if (
        method !== "POST" ||
        url.pathname !== "/api/v1/fleet-alert-notifications/dispatch"
      ) {
        return response;
      }
      const source = (await response.clone().json()) as Array<
        Record<string, unknown>
      >;
      const template = source[0];
      if (!template) return response;
      const base = Date.parse("2026-08-31T00:00:00Z");
      const deliveries = Array.from({ length: 201 }, (_, index) => ({
        ...template,
        created_at: new Date(base + index * 1_000).toISOString(),
        id: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
        target: `bounded-target-${String(index).padStart(3, "0")}`,
      }));
      return new Response(JSON.stringify(deliveries), {
        headers: response.headers,
        status: response.status,
        statusText: response.statusText,
      });
    };
  });
  await clearTrackedRequests(page);

  await activate(page.getByRole("button", { name: "Queue dispatch" }));
  await activate(
    page
      .getByLabel("Confirm notification queue dispatch")
      .getByRole("button", { name: "Queue dispatch" }),
  );
  await activate(page.getByRole("tab", { name: /Deliveries/ }));

  const grid = page.getByLabel("Notification delivery history data grid");
  await expect(grid).toContainText(
    "Only the newest alert-notification deliveries are loaded",
  );
  await grid
    .getByLabel("Notification delivery history search")
    .fill("bounded-target-200");
  await expect(grid).toContainText("bounded-target-200");
  await grid
    .getByLabel("Notification delivery history search")
    .fill("bounded-target-000");
  await expect(grid).toContainText(
    "No newest alert-notification delivery matches",
  );

  const requests = await trackedRequests(page);
  expect(
    requests.filter((request) => {
      const url = new URL(request.url, "http://localhost");
      return (
        request.method === "GET" &&
        url.pathname === "/api/v1/fleet-alert-notifications"
      );
    }),
  ).toHaveLength(0);
  expect(fullSnapshotCount(requests)).toBe(0);
});

test("Fleet mutation records preserve unrelated tail across both aggregate completion orders", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Observability", "Alerts");

  const policyId = "fbfbfbfb-1111-4111-8111-111111111111";
  const policyName = "edge-resource-policy";
  const tailPolicyName = "Default agent connectivity";
  const grid = page.getByLabel("Policy groups data grid");
  const policyRow = grid
    .locator(".gridBody [role=row]", { hasText: policyName })
    .first();
  const selectedPolicy = grid.getByLabel(
    `Select Policy groups row ${policyId}`,
  );
  const tailPolicyRow = grid
    .locator(".gridBody [role=row]", { hasText: tailPolicyName })
    .first();
  await expect(policyRow.getByText("enabled", { exact: true })).toBeVisible();
  await expect(
    tailPolicyRow.getByText("enabled", { exact: true }),
  ).toBeVisible();

  await page.evaluate(() => {
    type Gate = { promise: Promise<void>; release: () => void };
    const createGate = (): Gate => {
      let release = () => undefined;
      const promise = new Promise<void>((resolve) => {
        release = resolve;
      });
      return { promise, release };
    };
    const originalFetch = window.fetch.bind(window);
    let pendingMutation: Gate | null = null;
    let activeMutation: Gate | null = null;
    let pendingFull: Gate | null = null;
    let activeFull: Gate | null = null;
    const state = {
      mutationCaptured: 0,
      mutationDelivered: 0,
      mutationStarted: 0,
      fullCaptured: 0,
      fullDelivered: 0,
      fullStarted: 0,
      gateNextMutation: () => {
        if (pendingMutation || activeMutation) {
          throw new Error("an alert-policy mutation is already gated");
        }
        pendingMutation = createGate();
      },
      gateNextFull: () => {
        if (pendingFull || activeFull) {
          throw new Error("a full Fleet snapshot is already gated");
        }
        pendingFull = createGate();
      },
      releaseMutation: () => (activeMutation ?? pendingMutation)?.release(),
      releaseFull: () => (activeFull ?? pendingFull)?.release(),
    };
    Object.defineProperty(window, "__vpsmanFleetProjectionRace", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      const mutation =
        method === "POST" &&
        url.pathname === "/api/v1/fleet-alert-policies/bulk-mutate";
      const full =
        method === "GET" &&
        url.pathname === "/api/v1/fleet/snapshot" &&
        url.searchParams.get("mode") === "full";
      if (!mutation && !full) {
        return originalFetch(input, init);
      }

      const gate = mutation ? pendingMutation : pendingFull;
      if (mutation) {
        pendingMutation = null;
        activeMutation = gate;
        state.mutationStarted += 1;
      } else {
        pendingFull = null;
        activeFull = gate;
        state.fullStarted += 1;
      }
      // Capture the fixture response before the gate so each side of the race
      // has a deterministic server state, independent of delivery order.
      const response = await originalFetch(input, init);
      if (mutation) {
        state.mutationCaptured += 1;
      } else {
        state.fullCaptured += 1;
      }
      if (gate) await gate.promise;
      if (mutation) {
        activeMutation = null;
        state.mutationDelivered += 1;
      } else {
        activeFull = null;
        state.fullDelivered += 1;
      }
      return response;
    };
  });

  const raceState = () =>
    page.evaluate(() => {
      const state = (
        window as typeof window & {
          __vpsmanFleetProjectionRace: {
            mutationCaptured: number;
            mutationDelivered: number;
            mutationStarted: number;
            fullCaptured: number;
            fullDelivered: number;
            fullStarted: number;
          };
        }
      ).__vpsmanFleetProjectionRace;
      return {
        mutationCaptured: state.mutationCaptured,
        mutationDelivered: state.mutationDelivered,
        mutationStarted: state.mutationStarted,
        fullCaptured: state.fullCaptured,
        fullDelivered: state.fullDelivered,
        fullStarted: state.fullStarted,
      };
    });
  const controlRace = async (
    action:
      | "gateNextMutation"
      | "gateNextFull"
      | "releaseMutation"
      | "releaseFull",
  ) => {
    await page.evaluate((method) => {
      const controller = (
        window as typeof window & {
          __vpsmanFleetProjectionRace: Record<string, () => void>;
        }
      ).__vpsmanFleetProjectionRace;
      controller[method]?.();
    }, action);
  };
  const dispatchAgentUpdate = async () => {
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
  };
  const runPolicyAction = async (action: "Disable" | "Enable") => {
    if (!(await selectedPolicy.isChecked())) {
      await selectedPolicy.check();
    }
    await grid
      .locator(".gridToolbarActions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await page.getByRole("menuitem", { name: action, exact: true }).click();
  };

  // An aggregate captured before the mutation is stale for this projection.
  // The complete mutation response patches only its policy identity; the
  // unrelated loaded tail remains present.
  await controlRace("gateNextFull");
  await dispatchAgentUpdate();
  await expect.poll(async () => (await raceState()).fullCaptured).toBe(1);
  await runPolicyAction("Disable");
  await expect.poll(async () => (await raceState()).mutationDelivered).toBe(1);
  await expect(policyRow.getByText("disabled", { exact: true })).toBeVisible();
  await controlRace("releaseFull");
  await expect.poll(async () => (await raceState()).fullDelivered).toBe(1);
  await expect(policyRow.getByText("disabled", { exact: true })).toBeVisible();
  await expect(
    tailPolicyRow.getByText("enabled", { exact: true }),
  ).toBeVisible();

  // Restore once, then hold the next mutation response after the fixture has
  // committed it. A later full snapshot may land first; releasing the exact
  // response afterward is idempotent and still cannot replace the tail.
  await runPolicyAction("Enable");
  await expect.poll(async () => (await raceState()).mutationDelivered).toBe(2);
  await expect(policyRow.getByText("enabled", { exact: true })).toBeVisible();
  await controlRace("gateNextMutation");
  await runPolicyAction("Disable");
  await expect.poll(async () => (await raceState()).mutationCaptured).toBe(3);
  expect((await raceState()).mutationDelivered).toBe(2);
  await dispatchAgentUpdate();
  await expect.poll(async () => (await raceState()).fullDelivered).toBe(2);
  await expect(policyRow.getByText("disabled", { exact: true })).toBeVisible();
  await controlRace("releaseMutation");
  await expect.poll(async () => (await raceState()).mutationDelivered).toBe(3);
  await expect(policyRow.getByText("disabled", { exact: true })).toBeVisible();
  await expect(
    tailPolicyRow.getByText("enabled", { exact: true }),
  ).toBeVisible();

  const requests = await trackedRequests(page);
  expect(
    requests.filter((request) => {
      const url = new URL(request.url, "http://localhost");
      return (
        request.method === "GET" &&
        url.pathname === "/api/v1/fleet-alert-policies"
      );
    }),
  ).toHaveLength(0);
});

test("job_finished source refresh overlays an older held Home job page", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "projection ownership is viewport independent",
  );
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

  const exactJobId = "30000000-0000-4000-8000-000000000001";
  await page.evaluate((jobId) => {
    const staleJob = {
      actor_id: null,
      command_type: "shell_argv",
      completed_at: "2026-06-02T10:00:09Z",
      created_at: "2026-06-02T10:00:00Z",
      id: jobId,
      max_timeout_secs: 30,
      payload_hash: "a".repeat(64),
      privileged: false,
      source_schedule_id: null,
      status: "failed",
      target_count: 1,
    };
    const unaffectedFailedJob = {
      ...staleJob,
      created_at: "2026-06-02T09:59:00Z",
      id: "30000000-0000-4000-8000-000000000002",
    };
    const originalJson = Response.prototype.json;
    Response.prototype.json = async function () {
      const payload = await originalJson.call(this);
      if (
        payload &&
        typeof payload === "object" &&
        "generated_at" in payload &&
        "jobs" in payload
      ) {
        const snapshot = payload as {
          jobs?: { data?: unknown[] };
        };
        if (snapshot.jobs?.data) {
          snapshot.jobs.data = [staleJob, unaffectedFailedJob];
        }
      }
      return payload;
    };
    const originalFetch = window.fetch.bind(window);
    const state = { historyGets: 0 };
    Object.defineProperty(window, "__vpsmanJobsProjectionRace", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method === "GET" && url.pathname === "/api/v1/jobs") {
        state.historyGets += 1;
        return new Response(
          JSON.stringify([
            {
              ...staleJob,
              completed_at: "2026-06-02T10:00:10Z",
              status: "completed",
            },
            unaffectedFailedJob,
          ]),
          { headers: { "Content-Type": "application/json" }, status: 200 },
        );
      }
      return originalFetch(input, init);
    };
  }, exactJobId);

  await page.evaluate((jobId) => {
    const socket = (
      window as typeof window & {
        __vpsmanTestWebSockets: EventTarget[];
      }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          job_id: jobId,
          status: "completed",
          type: "job_finished",
        }),
      }),
    );
  }, exactJobId);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobsProjectionRace: { historyGets: number };
            }
          ).__vpsmanJobsProjectionRace.historyGets,
      ),
    )
    .toBe(1);

  await page.evaluate(() => {
    (
      window as typeof window & { __vpsmanReleaseHomeSnapshot: () => void }
    ).__vpsmanReleaseHomeSnapshot();
  });
  await expect(
    page.getByText("1 failed in loaded history", { exact: true }),
  ).toBeVisible();
});

test("rapid Jobs aggregate producers retain one active and one latest trailing read", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "request ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    const gates: Array<() => void> = [];
    const state = {
      active: 0,
      maxActive: 0,
      releaseNext: () => gates.shift()?.(),
      started: 0,
    };
    Object.defineProperty(window, "__vpsmanJobsAggregateOwner", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method !== "GET" || url.pathname !== "/api/v1/jobs") {
        return originalFetch(input, init);
      }
      state.started += 1;
      state.active += 1;
      state.maxActive = Math.max(state.maxActive, state.active);
      await new Promise<void>((resolve) => gates.push(resolve));
      try {
        return await originalFetch(input, init);
      } finally {
        state.active -= 1;
      }
    };
  });

  await openConsoleSubpage(page, "Jobs", "History");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobsAggregateOwner: { started: number };
            }
          ).__vpsmanJobsAggregateOwner.started,
      ),
    )
    .toBe(1);
  await openConsoleSubpage(page, "Config", "Overview");
  await openConsoleSubpage(page, "Automation", "Schedules");
  await openConsoleSubpage(page, "Jobs", "History");
  await page.waitForTimeout(100);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobsAggregateOwner: {
                active: number;
                maxActive: number;
                started: number;
              };
            }
          ).__vpsmanJobsAggregateOwner,
      ),
    )
    .toEqual({ active: 1, maxActive: 1, started: 1 });

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanJobsAggregateOwner: { releaseNext: () => void };
      }
    ).__vpsmanJobsAggregateOwner.releaseNext();
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobsAggregateOwner: {
                active: number;
                maxActive: number;
                started: number;
              };
            }
          ).__vpsmanJobsAggregateOwner,
      ),
    )
    .toEqual({ active: 1, maxActive: 1, started: 2 });
  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanJobsAggregateOwner: { releaseNext: () => void };
      }
    ).__vpsmanJobsAggregateOwner.releaseNext();
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanJobsAggregateOwner: {
                active: number;
                maxActive: number;
                started: number;
              };
            }
          ).__vpsmanJobsAggregateOwner,
      ),
    )
    .toEqual({ active: 0, maxActive: 1, started: 2 });
});

test("monitoring cards keep one request owner across panel remounts", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "request ownership is viewport independent",
  );
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    let releaseFirst: (() => void) | null = null;
    const firstGate = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    const state = {
      active: 0,
      maxActive: 0,
      releaseFirst: () => releaseFirst?.(),
      started: 0,
    };
    Object.defineProperty(window, "__vpsmanMonitoringCardsOwner", {
      configurable: true,
      value: state,
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
      if (url.pathname !== "/api/v1/monitoring/cards") {
        return originalFetch(input, init);
      }
      state.started += 1;
      state.active += 1;
      state.maxActive = Math.max(state.maxActive, state.active);
      try {
        if (state.started === 1) await firstGate;
        return await originalFetch(input, init);
      } finally {
        state.active -= 1;
      }
    };
  });

  await openConsoleSubpage(page, "Fleet", "Monitor");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanMonitoringCardsOwner: { started: number };
            }
          ).__vpsmanMonitoringCardsOwner.started,
      ),
    )
    .toBe(1);

  await openConsoleSubpage(page, "Fleet", "Instances");
  await openConsoleSubpage(page, "Fleet", "Monitor");
  await page.waitForTimeout(100);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanMonitoringCardsOwner: {
                maxActive: number;
                started: number;
              };
            }
          ).__vpsmanMonitoringCardsOwner,
      ),
    )
    .toMatchObject({ maxActive: 1, started: 1 });

  await page.evaluate(() => {
    (
      window as typeof window & {
        __vpsmanMonitoringCardsOwner: { releaseFirst: () => void };
      }
    ).__vpsmanMonitoringCardsOwner.releaseFirst();
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as typeof window & {
              __vpsmanMonitoringCardsOwner: {
                active: number;
                maxActive: number;
                started: number;
              };
            }
          ).__vpsmanMonitoringCardsOwner,
      ),
    )
    .toEqual({ active: 0, maxActive: 1, started: 2 });
  await expect(
    page.getByLabel("VPS monitor cards").locator(".vpsMonitorCard").first(),
  ).toBeVisible();
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

test("a rejected job received while hidden catches up through Jobs history only", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "visibility is viewport independent",
  );
  await installVisibilityControl(page, false);
  await installConsoleApiMock(page, { storedAuthSession: true });
  await page.goto("/");
  await waitForConsoleShell(page);
  await openConsoleSubpage(page, "Jobs", "History");

  const rejectedJobId = "50000000-0000-4000-8000-000000000001";
  await page.evaluate((jobId) => {
    const originalFetch = window.fetch.bind(window);
    const record = {
      actor_id: null,
      command_type: "hidden rejection probe",
      completed_at: "2026-06-02T10:01:00Z",
      created_at: "2026-06-02T10:01:00Z",
      id: jobId,
      max_timeout_secs: 60,
      payload_hash: "f".repeat(64),
      privileged: false,
      source_schedule_id: null,
      status: "rejected",
      target_count: 1,
    };
    const state = { itemGets: 0, listGets: 0 };
    Object.defineProperty(window, "__vpsmanHiddenRejectedJob", {
      configurable: true,
      value: state,
    });
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = new URL(
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url,
        window.location.href,
      );
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      if (method === "GET" && url.pathname === "/api/v1/jobs") {
        state.listGets += 1;
        return new Response(JSON.stringify([record]), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }
      if (method === "GET" && /^\/api\/v1\/jobs\/[^/]+$/.test(url.pathname)) {
        state.itemGets += 1;
      }
      return originalFetch(input, init);
    };
  }, rejectedJobId);

  await setDocumentHidden(page, true);
  await page.evaluate((jobId) => {
    const socket = (
      window as typeof window & { __vpsmanTestWebSockets: EventTarget[] }
    ).__vpsmanTestWebSockets.at(-1);
    socket?.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          job_id: jobId,
          status: "rejected",
          type: "job_rejected",
        }),
      }),
    );
  }, rejectedJobId);
  await page.waitForTimeout(100);
  expect(
    await page.evaluate(
      () =>
        (
          window as typeof window & {
            __vpsmanHiddenRejectedJob: { itemGets: number; listGets: number };
          }
        ).__vpsmanHiddenRejectedJob,
    ),
  ).toEqual({ itemGets: 0, listGets: 0 });

  await setDocumentHidden(page, false);
  await expect(page.getByText("hidden rejection probe")).toBeVisible();
  const requestState = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __vpsmanHiddenRejectedJob: { itemGets: number; listGets: number };
        }
      ).__vpsmanHiddenRejectedJob,
  );
  expect(requestState.listGets).toBeGreaterThan(0);
  expect(requestState.listGets).toBeLessThanOrEqual(2);
  expect(requestState.itemGets).toBe(0);
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
