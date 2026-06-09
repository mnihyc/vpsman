import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("schedule registry lifecycle uses UUID actions from the browser", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "schedule lifecycle editing is covered in the desktop console",
  );

  await page.goto("/");
  await openConsoleSubpage(page, "Schedules", "Schedule registry");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Schedules", "Schedule registry");

  await expect(
    page.getByRole("heading", { level: 1, name: "Schedules" }),
  ).toBeVisible();
  await expect(page.getByText("edge-health-hourly")).toBeVisible();

  await activate(
    page.getByRole("button", { name: "Disable edge-health-hourly" }),
  );
  await expect(page.getByText("Confirm schedule disable")).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Disable" }),
  );
  await expect(
    page.locator(".status").filter({ hasText: "disabled" }),
  ).toBeVisible();

  await activate(
    page.getByRole("button", { name: "Enable edge-health-hourly" }),
  );
  await expect(page.getByText("Confirm schedule enable")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", { name: "Enable" }),
  );
  await expect(
    page.locator(".status").filter({ hasText: "enabled" }).first(),
  ).toBeVisible();

  await activate(page.getByRole("button", { name: "Edit edge-health-hourly" }));
  await expect(
    page.getByRole("heading", { name: "Modify schedule" }),
  ).toBeVisible();
  await page.getByLabel("Schedule name").fill("edge-health-hourly");
  await page.getByLabel("Schedule cron expression").fill("10,40 * * * *");
  await page
    .getByLabel("Schedule target expression")
    .fill("provider:alpha && country:US");
  await expect(page.getByText("1 matching VPSs")).toBeVisible();
  await activate(page.getByRole("button", { name: "Update", exact: true }));
  await expect(page.getByText("Confirm schedule update")).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Update schedule" }),
  );
  await expect(page.getByText("10,40 * * * *")).toBeVisible();

  await activate(
    page.getByRole("button", { name: "Defer edge-health-hourly" }),
  );
  await page.getByLabel("Schedule defer until").fill("2026-06-04T09:30");
  await page
    .getByLabel("Schedule defer reason")
    .fill("Customer maintenance freeze for APAC packet-filter update");
  await activate(
    page
      .locator(".inlineOpsForm")
      .getByRole("button", { name: "Defer", exact: true }),
  );
  await expect(page.getByText("Confirm schedule defer")).toBeVisible();
  await expect(
    page.getByText(
      "Customer maintenance freeze for APAC packet-filter update",
    ),
  ).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", { name: "Defer" }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: {
              scheduleActions: Array<{ method: string; path: string }>;
            };
          }
        ).__vpsmanTestRequests;
        return requests.scheduleActions.length;
      }),
    )
    .toBe(4);

  await activate(
    page.getByRole("button", { name: "Apply edge-health-hourly now" }),
  );
  await expect(page.getByText("Confirm apply now")).toBeVisible();
  await expect(
    page.getByText(
      "Dispatches a normal job from the saved schedule without changing the next scheduled run.",
    ),
  ).toBeVisible();
  await activate(
    page
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Apply now" }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (
          window as unknown as {
            __vpsmanTestRequests: {
              scheduleActions: Array<{ method: string; path: string }>;
            };
          }
        ).__vpsmanTestRequests;
        return requests.scheduleActions.length;
      }),
    )
    .toBe(5);

  await activate(
    page.getByRole("button", { name: "Delete edge-health-hourly" }),
  );
  await expect(page.getByText("Confirm schedule delete")).toBeVisible();
  await activate(
    page.locator(".confirmationPrompt").getByRole("button", { name: "Delete" }),
  );
  await expect(page.getByText("edge-health-hourly")).toHaveCount(0);

  const actions = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: {
          scheduleActions: Array<{
            method: string;
            path: string;
            body?: unknown;
          }>;
        };
      }
    ).__vpsmanTestRequests;
    return requests.scheduleActions;
  });
  expect(actions.map((action) => `${action.method} ${action.path}`)).toEqual([
    "POST /api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef/disable",
    "POST /api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef/enable",
    "PUT /api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef",
    "POST /api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef/defer",
    "POST /api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef/apply-now",
    "DELETE /api/v1/schedules/51515151-6161-4717-8abc-defdefdefdef",
  ]);
  expect(JSON.stringify(actions)).not.toContain("local-super-password");
  expect(JSON.stringify(actions)).not.toContain("envelope");
});

test("expert operator can scan and dispatch across a realistic 24 VPS fleet", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "20+ VPS expert audit is a dense desktop workflow",
  );

  await installTwentyFourVpsExpertMock(page);
  await page.goto("/");

  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { name: "Fleet overview" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "VPS instances" }),
  ).toBeVisible();
  const fleetGrid = page.getByLabel("VPS instance records data grid");
  await expect(
    fleetGrid.getByText("ams-payments-edge-01").first(),
  ).toBeVisible();
  await expect(fleetGrid.getByText("Last seen").first()).toBeVisible();
  await expect(fleetGrid.getByText("never seen").first()).toBeVisible();
  await expect(fleetGrid.getByText("stale").first()).toBeVisible();
  await expect(fleetGrid.locator(".countryFlag").first()).toBeVisible();
  await activate(fleetGrid.getByText("ams-payments-edge-01").first());
  const embeddedDetail = fleetGrid.locator(".gridExpandedRow").first();
  await expect(embeddedDetail.getByText("Last seen")).toBeVisible();
  await expect(embeddedDetail).toContainText("acmecloud");
  await expect(embeddedDetail).toContainText("75% (12 GiB / 16 GiB)");
  await expect(embeddedDetail.getByText("Overview curves")).toBeVisible();
  await expect(embeddedDetail.getByLabel("VPS detail sections")).toBeVisible();
  await expect(embeddedDetail.getByLabel("Fleet inline tag")).toBeVisible();

  const rowChecks = fleetGrid.getByRole("checkbox", {
    name: "Select VPS instance records row",
    exact: true,
  });
  await rowChecks.nth(0).check();
  await rowChecks.nth(1).check();
  const selectionPanel = page.locator(".gridSelectionPanel");
  await expect(selectionPanel.getByText("2 selected VPSs")).toBeVisible();
  await expect(
    selectionPanel.getByRole("button", { name: /Bulk execution/ }),
  ).toBeVisible();
  await expect(
    selectionPanel.getByRole("button", { name: /Multi-file/ }),
  ).toBeVisible();
  await expect(
    selectionPanel.getByLabel("Tag to add to selected VPSs"),
  ).toBeVisible();
  await expect(
    selectionPanel.getByLabel("Selected VPS statistical tables"),
  ).toBeVisible();
  await expect(selectionPanel.getByText("RAM used").first()).toBeVisible();
  await expect(selectionPanel).toContainText("75% (12 GiB / 16 GiB)");

  await openConsoleSubpage(page, "Jobs", "Dispatch");
  await expect(
    page.getByRole("heading", { name: "Dispatch command" }),
  ).toBeVisible();
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Jobs", "Dispatch");

  const composer = page.locator(".commandComposer");
  await composer
    .getByLabel("Saved command template")
    .selectOption("46464646-5656-4789-8abc-defdefdefdef");
  await composer
    .getByLabel("Command argv")
    .fill("/usr/local/bin/check-payment-edge --scope eu-west --json");
  await composer
    .getByLabel("Bulk target selector expression")
    .fill("provider:acmecloud && tag:payments");
  await expect(composer.getByText("24/24").first()).toBeVisible();
  await activate(composer.getByRole("button", { name: "Preview" }));
  await expect(composer.getByText("24 resolved targets")).toBeVisible();

  const impact = composer.locator(".targetImpactPreview");
  await expect(
    impact.getByText("24 targets / standard dispatch"),
  ).toBeVisible();
  await expect(impact.getByText("Stale")).toBeVisible();
  await expect(impact.getByText("Unavailable")).toBeVisible();
  await expect(
    impact
      .locator(".targetChip")
      .filter({ hasText: "ams-payments-edge-01" })
      .first(),
  ).toHaveAttribute("title", "pay-prod-ams-edge-01");
  await expect(impact.getByText(/more/)).toBeVisible();

  await composer.getByLabel("Timeout seconds").fill("120");
  await activate(composer.getByRole("button", { name: "Dispatch" }));
  await expect(composer.getByText("Confirm job dispatch")).toBeVisible();
  await expect(composer.locator(".dispatchActions")).toHaveCount(0);
  await expect(
    composer.getByText("24 resolved (21 online, 1 stale, 2 unavailable)"),
  ).toBeVisible();
  await expect(composer.getByText("Unlocked locally")).toBeVisible();
  await activate(
    composer
      .locator(".confirmationPrompt")
      .getByRole("button", { name: "Dispatch job" }),
  );

  const resultPanel = page.getByLabel("Execution result");
  await expect(resultPanel).toBeVisible();
  await expect(
    resultPanel
      .locator(".executionResultStats span")
      .filter({ hasText: "pushed" })
      .filter({ hasText: "22/24" }),
  ).toBeVisible();
  await expect(
    resultPanel
      .locator(".executionResultStats span")
      .filter({ hasText: "failed" })
      .filter({ hasText: "2" }),
  ).toBeVisible();
  await expect(
    resultPanel
      .locator(".executionResultStats span")
      .filter({ hasText: "unavailable" })
      .filter({ hasText: "2" }),
  ).toBeVisible();
  await expect(
    resultPanel.getByText(/partial success: 20 done, 2 failed, 2 unavailable/),
  ).toBeVisible();
  const failedReasons = resultPanel.getByLabel("Failed target reasons");
  await expect(
    failedReasons.getByText(
      /stale: agent rejected shell_argv command_version 3/,
    ),
  ).toBeVisible();
  await expect(
    failedReasons.getByText(/process_guard: permission denied/),
  ).toBeVisible();

  const jobRequest = await page.evaluate(() => {
    const requests = (
      window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }
    ).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(jobRequest)).not.toContain("local-super-password");
  expect(JSON.stringify(jobRequest)).not.toContain("envelope");
  expect(jobRequest).toMatchObject({
    argv: ["/usr/local/bin/check-payment-edge", "--scope", "eu-west", "--json"],
    command: "shell_argv",
    confirmed: false,
    destructive: false,
    privileged: true,
    selector_expression: "provider:acmecloud && tag:payments",
    timeout_secs: 120,
  });

  await openConsoleSubpage(page, "Access", "VPS keys");
  const inspector = page.locator(".accessInspector");
  await expect(
    page.getByRole("heading", { name: "Gateway agent identities" }),
  ).toBeVisible();
  await inspector
    .getByLabel("Agent identity client ID")
    .fill("agent-sin-payments-25");
  await inspector
    .getByLabel("Agent identity public key hex")
    .fill("b".repeat(64));
  await inspector
    .getByLabel("Agent identity display name")
    .fill("sin-payments-edge-25");
  await inspector
    .getByLabel("Agent identity tags")
    .fill("provider:acmecloud,country:SG,payments,edge,env:prod");
  await activate(
    inspector.getByRole("button", { name: "Import gateway identity" }),
  );
  await activate(
    page
      .getByLabel("Confirm direct gateway identity import")
      .getByRole("button", { name: "Import identity" }),
  );
  await expect(inspector.getByText("sin-payments-edge-25")).toBeVisible();

  const layout = await collectLayoutSignals(page, ".commandComposer");
  expect(layout.horizontalOverflowPx).toBeLessThanOrEqual(1);
  expect(layout.clippedControls).toEqual([]);
  expect(layout.overlaps).toEqual([]);
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("expert-24-vps-dispatch.png"),
  });
});

async function installTwentyFourVpsExpertMock(page: Page) {
  await page.addInitScript(() => {
    const previousFetch = window.fetch.bind(window);
    const regions = [
      ["ams", "NL"],
      ["fra", "DE"],
      ["lhr", "GB"],
      ["iad", "US"],
      ["sfo", "US"],
      ["sin", "SG"],
    ] as const;
    const agents = Array.from({ length: 24 }, (_, index) => {
      const [region, country] = regions[index % regions.length];
      const ordinal = String(index + 1).padStart(2, "0");
      const status =
        index === 7 || index === 8
          ? "offline"
          : index === 9
            ? "stale"
            : "online";
      return {
        capabilities: {
          can_apply_process_limits: true,
          can_attempt_privileged_ops: true,
          can_manage_runtime_tunnels: true,
          effective_uid: 0,
          privilege_mode: "root",
          unprivileged_hint: null,
        },
        display_name: `${region}-payments-edge-${ordinal}`,
        id: `pay-prod-${region}-edge-${ordinal}`,
        internal_build_number: status === "stale" ? 17 : 18,
        last_ip: `10.${20 + (index % 6)}.${Math.floor(index / 6)}.${20 + index}`,
        last_seen_at:
          index === 6
            ? null
            : status === "offline"
              ? "2026-06-07T02:15:00Z"
              : status === "stale"
                ? "2026-06-07T03:22:00Z"
                : `2026-06-07T04:${String(10 + (index % 40)).padStart(2, "0")}:00Z`,
        registration_ip: `198.51.100.${20 + index}`,
        stale_reason:
          status === "stale"
            ? "agent rejected shell_argv command_version 3"
            : null,
        stale_since: status === "stale" ? "2026-06-07T03:22:00Z" : null,
        status,
        tags: [
          "provider:acmecloud",
          `country:${country}`,
          "payments",
          "edge",
          "env:prod",
          `region:${region}`,
          index % 4 === 0 ? "pci" : "standard",
        ],
      };
    });
    const gib = 1024 * 1024 * 1024;
    const telemetryRollups = agents.flatMap((agent, index) =>
      Array.from({ length: 4 }, (_, point) => {
        const bucketMinute = String(20 + point * 5 + (index % 4)).padStart(
          2,
          "0",
        );
        const memoryAvailableGiB = 4 + (index % 5);
        const diskAvailableGiB = 56 - (index % 9) * 2;
        return {
          bucket_secs: 300,
          bucket_start: `2026-06-07T04:${bucketMinute}:00Z`,
          client_id: agent.id,
          cpu_load_1_avg: 0.35 + (index % 6) * 0.18 + point * 0.03,
          cpu_load_1_max: 0.75 + (index % 6) * 0.22 + point * 0.04,
          disk_available_bytes_avg: diskAvailableGiB * gib,
          disk_available_bytes_min: (diskAvailableGiB - 1) * gib,
          disk_total_bytes_max: 80 * gib,
          latest_observed_at: `2026-06-07T04:${bucketMinute}:45Z`,
          memory_available_bytes_avg: memoryAvailableGiB * gib,
          memory_available_bytes_min: Math.max(1, memoryAvailableGiB - 1) * gib,
          memory_total_bytes_max: 16 * gib,
          network_rx_bytes_max: (120 + index * 3 + point) * 1024 * 1024,
          network_tx_bytes_max: (70 + index * 2 + point) * 1024 * 1024,
          sample_count: 12 + point,
          updated_at: `2026-06-07T04:${bucketMinute}:50Z`,
        };
      }),
    );
    const telemetryNetworkRates = agents.flatMap((agent, index) =>
      ["eth0", "wg0"].map((networkInterface, interfaceIndex) => ({
        bucket_secs: 300,
        bucket_start: "2026-06-07T04:40:00Z",
        client_id: agent.id,
        interface: networkInterface,
        rx_bps_avg: 12_000 + index * 750 + interfaceIndex * 4_500,
        rx_bytes_avg: 460_000 + index * 8_000,
        rx_bytes_delta: 3_600_000 + index * 18_000,
        sample_count: 8,
        tx_bps_avg: 9_000 + index * 500 + interfaceIndex * 3_500,
        tx_bytes_avg: 380_000 + index * 7_000,
        tx_bytes_delta: 2_900_000 + index * 14_000,
        updated_at: "2026-06-07T04:45:05Z",
      })),
    );
    const jobTargets: Record<string, unknown[]> = {};
    const readJsonBody = async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ) => {
      if (typeof init?.body === "string") {
        return JSON.parse(init.body);
      }
      if (input instanceof Request) {
        return input.clone().json();
      }
      return null;
    };
    const jsonResponse = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          headers: { "Content-Type": "application/json" },
          status,
        }),
      );
    const valueMatches = (
      value: string,
      pattern: string,
      contains: boolean,
    ) => {
      const normalizedValue = value.toLocaleLowerCase();
      const normalizedPattern = pattern.toLocaleLowerCase();
      return contains
        ? normalizedValue.includes(normalizedPattern)
        : normalizedValue === normalizedPattern;
    };
    const matchesSelector = (
      agent: (typeof agents)[number],
      selector: string,
    ) => {
      const expression = selector.trim().toLocaleLowerCase();
      if (!expression || expression === "id:*") {
        return true;
      }
      return expression.split(/\s*&&\s*/).every((term) => {
        const [kind, value] = term.split(":", 2);
        if (!value) {
          return (
            valueMatches(agent.id, kind, true) ||
            valueMatches(agent.display_name, kind, true)
          );
        }
        if (kind === "id") {
          return valueMatches(agent.id, value, false);
        }
        if (kind === "name") {
          return valueMatches(agent.display_name, value, false);
        }
        if (kind === "provider") {
          return agent.tags.some((tag) =>
            valueMatches(tag, `provider:${value}`, false),
          );
        }
        if (kind === "country") {
          return agent.tags.some((tag) =>
            valueMatches(tag, `country:${value}`, false),
          );
        }
        if (kind === "tag") {
          return agent.tags.some((tag) => valueMatches(tag, value, false));
        }
        if (kind === "status") {
          return valueMatches(agent.status, value, false);
        }
        return agent.tags.some((tag) => valueMatches(tag, term, false));
      });
    };
    const resolveTargets = (body: unknown) => {
      const selector =
        (body as { selector_expression?: string } | null)
          ?.selector_expression ?? "";
      return agents.filter((agent) => matchesSelector(agent, selector));
    };

    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (
        init?.method ?? (input instanceof Request ? input.method : "GET")
      ).toUpperCase();
      if (pathname === "/api/v1/agents" && method === "GET") {
        return jsonResponse(agents);
      }
      if (pathname === "/api/v1/fleet/summary" && method === "GET") {
        return jsonResponse({
          offline: agents.filter((agent) => agent.status === "offline").length,
          online: agents.filter((agent) => agent.status === "online").length,
          running_jobs: 7,
          stale: agents.filter((agent) => agent.status === "stale").length,
          total: agents.length,
          warnings: 3,
        });
      }
      if (pathname === "/api/v1/telemetry/rollups" && method === "GET") {
        return jsonResponse(telemetryRollups);
      }
      if (pathname === "/api/v1/telemetry/network-rates" && method === "GET") {
        return jsonResponse(telemetryNetworkRates);
      }
      if (pathname === "/api/v1/bulk/resolve" && method === "POST") {
        const body = await readJsonBody(input, init);
        const targets = resolveTargets(body);
        return jsonResponse({ target_count: targets.length, targets });
      }
      if (pathname === "/api/v1/jobs" && method === "POST") {
        const requests = (
          window as unknown as { __vpsmanTestRequests?: { jobs: unknown[] } }
        ).__vpsmanTestRequests;
        const body = await readJsonBody(input, init);
        requests?.jobs.push(body);
        const targets = resolveTargets(body);
        const acceptedTargets = targets.filter(
          (agent) => agent.status !== "offline",
        );
        const jobId = "24242424-7777-4888-9999-aaaaaaaaaaaa";
        jobTargets[jobId] = targets.map((agent) => ({
          client_id: agent.id,
          completed_at:
            agent.status === "offline" ? null : "2026-06-07T04:50:10Z",
          exit_code:
            agent.status === "stale"
              ? 2
              : agent.id === "pay-prod-sfo-edge-11"
                ? 13
                : agent.status === "offline"
                  ? null
                  : 0,
          job_id: jobId,
          message:
            agent.status === "stale"
              ? "stale: agent rejected shell_argv command_version 3"
              : agent.id === "pay-prod-sfo-edge-11"
                ? "process_guard: permission denied"
                : agent.status === "offline"
                  ? "agent offline"
                  : "completed",
          started_at:
            agent.status === "offline" ? null : "2026-06-07T04:50:05Z",
          status:
            agent.status === "stale" || agent.id === "pay-prod-sfo-edge-11"
              ? "failed"
              : agent.status === "offline"
                ? "dispatch_failed"
                : "completed",
        }));
        return jsonResponse({
          accepted_targets: acceptedTargets.length,
          job_id: jobId,
          status: "accepted",
        });
      }
      const targetsMatch = pathname.match(
        /^\/api\/v1\/jobs\/([^/]+)\/targets$/,
      );
      if (targetsMatch && method === "GET" && jobTargets[targetsMatch[1]]) {
        return jsonResponse(jobTargets[targetsMatch[1]]);
      }
      const jobMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)$/);
      if (
        jobMatch &&
        method === "GET" &&
        jobMatch[1] === "24242424-7777-4888-9999-aaaaaaaaaaaa"
      ) {
        return jsonResponse({
          actor_id: null,
          command_type: "shell_argv",
          completed_at: "2026-06-07T04:50:10Z",
          created_at: "2026-06-07T04:50:05Z",
          id: jobMatch[1],
          payload_hash: "2".repeat(64),
          privileged: true,
          status: "partially_completed",
          target_count: 24,
        });
      }
      return previousFetch(input, init);
    };
  });
}

async function collectLayoutSignals(page: Page, scopeSelector: string) {
  return page.evaluate((selector) => {
    const scope = document.querySelector(selector) ?? document.body;
    const visible = (element: Element) => {
      const rect = element.getBoundingClientRect();
      const style = window.getComputedStyle(element);
      return (
        rect.width > 0 &&
        rect.height > 0 &&
        style.display !== "none" &&
        style.visibility !== "hidden"
      );
    };
    const label = (element: Element) => ({
      className:
        element instanceof HTMLElement ? String(element.className) : "",
      tagName: element.tagName.toLowerCase(),
      text: (element.textContent ?? "")
        .replace(/\s+/g, " ")
        .trim()
        .slice(0, 96),
    });
    const controls = Array.from(
      scope.querySelectorAll(
        "button, input, select, textarea, summary, .searchExpressionInput",
      ),
    ).filter(visible);
    const rects = controls.map((element) => ({
      element,
      rect: element.getBoundingClientRect(),
    }));
    const scopeRect = scope.getBoundingClientRect();
    const clippedControls = rects
      .filter(
        ({ rect }) =>
          rect.right > scopeRect.right + 1 || rect.left < scopeRect.left - 1,
      )
      .map(({ element }) => label(element));
    const overlaps = [];
    for (let leftIndex = 0; leftIndex < rects.length; leftIndex += 1) {
      for (
        let rightIndex = leftIndex + 1;
        rightIndex < rects.length;
        rightIndex += 1
      ) {
        const left = rects[leftIndex];
        const right = rects[rightIndex];
        const separated =
          left.rect.right <= right.rect.left + 1 ||
          right.rect.right <= left.rect.left + 1 ||
          left.rect.bottom <= right.rect.top + 1 ||
          right.rect.bottom <= left.rect.top + 1;
        if (!separated) {
          overlaps.push({ a: label(left.element), b: label(right.element) });
        }
      }
    }
    return {
      clippedControls,
      horizontalOverflowPx: Math.max(0, scope.scrollWidth - scope.clientWidth),
      overlaps,
    };
  }, scopeSelector);
}
