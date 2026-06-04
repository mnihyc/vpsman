import { expect, test, type Locator } from "@playwright/test";
import {
  backupId,
  buildEncryptedBackupArtifactFixture,
  installConsoleApiMock,
  ospfUpdatePlans,
  sha256Hex,
  tunnelPlans,
} from "./support/consoleLayoutFixtures";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

async function checkControl(locator: Locator) {
  await locator.evaluate((element) => {
    const input = element as HTMLInputElement;
    if (!input.checked) {
      input.click();
    }
  });
}

test("renders an operational cloud-console fleet workspace", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Fleet overview" })).toBeVisible();
  await expect(page.getByPlaceholder("Search VPS, tag, pool, job")).toBeVisible();
  await expect(page.getByRole("button", { name: /edge-sfo-01/ })).toBeVisible();
  await expect(page.locator(".consoleHeader").getByText("2 connected / 3 total")).toBeVisible();
  await expect(page.getByText("VPS instances")).toBeVisible();
  await expect(page.getByLabel("Fleet alerts")).toBeVisible();
  await expect(page.getByText("Tunnel adapter status failed")).toBeVisible();
  await expect(page.getByText("Agent is not connected")).toBeVisible();

  await activate(page.getByRole("button", { name: /core-fra-02/ }));
  await expect(page.getByRole("heading", { name: "core-fra-02" })).toBeVisible();

  await activate(page.getByRole("tab", { name: "Network" }));
  await expect(page.getByText("BGP/OSPF")).toBeVisible();
  await expect(page.getByText("Client-managed runtime tunnels enabled")).toBeVisible();
  await expect(page.getByText("bgp, bird2, pool:europe")).toBeVisible();
  await expect(page.getByText(/tun0 tun_tap up/)).toBeVisible();
  await expect(page.getByText(/eth0 RX 8.7 Kbps \/ TX 17 Kbps/)).toBeVisible();

  await activate(page.getByRole("button", { name: /backup-nyc-03/ }));
  await activate(page.getByRole("tab", { name: "Network" }));
  await expect(page.getByText("Unprivileged best-effort, root operations may be ineffective")).toBeVisible();
});

test("keeps console layout usable on desktop and mobile widths", async ({ page }, testInfo) => {
  await page.goto("/");

  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);

  await expect(page.locator(".topbar")).toBeVisible();
  await expect(page.locator(".quickStats")).toBeVisible();
  if (testInfo.project.name.includes("desktop")) {
    await expect(page.locator(".sidebar")).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Primary console navigation" })).toBeVisible();
    await expect(page.locator(".navSectionTitle", { hasText: "Operations" })).toBeVisible();
    await expect(page.locator(".navSectionTitle", { hasText: "Network" })).toBeVisible();
    await expect(page.locator(".navSectionTitle", { hasText: "Data & access" })).toBeVisible();
    await expect(page.getByRole("button", { name: /Resource scope\s+All VPS resources/ })).toBeVisible();
    await expect(page.locator(".controlPlanePill", { hasText: "Live control plane" })).toBeVisible();
  } else {
    await expect(page.locator(".sidebar")).toBeHidden();
    await expect(page.locator(".scopeSelector")).toBeHidden();
  }
});

test("manages data-source preset assignments from the pools view", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense preset management is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("navigation", { name: "Primary console navigation" }).getByRole("button", { name: "Pools" }));

  const panel = page.locator(".dataSourcePresetPanel");
  const activeSourcesSearchField = panel.getByRole("combobox", { name: "Active sources search field" });
  const activeSourcesSearch = panel.getByRole("searchbox", { name: "Active sources search" });
  const presetRegistrySearchField = panel.getByRole("combobox", { name: "Preset registry search field" });
  const presetRegistrySearch = panel.getByRole("searchbox", { name: "Preset registry search" });
  await expect(panel.getByRole("heading", { name: "Data-source presets" })).toBeVisible();
  await expect(panel.getByRole("heading", { name: "Active source status" })).toBeVisible();
  await expect(panel.getByLabel("Active sources table controls")).toBeVisible();
  await expect(activeSourcesSearchField).toBeVisible();
  await expect(activeSourcesSearch).toBeVisible();
  await expect(panel.getByText(/\d+ of \d+ sources/)).toBeVisible();
  await expect(panel.getByText(/Page 1 \/ \d+/).first()).toBeVisible();
  await expect(panel.locator(".sourceStatusSection").getByText("shared:vnstat-json")).toBeVisible();
  await expect(panel.locator(".sourceStatusSection").getByText("vnstat", { exact: true })).toBeVisible();
  await expect(panel.locator(".sourceStatusSection").getByText("no server store, 2 artifacts")).toBeVisible();
  await expect(panel.locator(".sourceStatusSection").getByText("no server store, 1 releases, 1 external")).toBeVisible();
  await activeSourcesSearchField.selectOption("Preset");
  await activeSourcesSearch.fill("shared:vnstat-json");
  await expect(panel.locator(".sourceStatusSection").getByText("shared:vnstat-json")).toBeVisible();
  await activeSourcesSearch.fill("");
  await expect(panel.getByLabel("Preset registry table controls")).toBeVisible();
  await expect(presetRegistrySearchField).toBeVisible();
  await expect(presetRegistrySearch).toBeVisible();
  await expect(panel.locator(".historyRow.dataSourcePresetGrid", { hasText: "builtin:interface_counters" })).toBeVisible();
  await expect(panel.locator(".historyRow.dataSourcePresetGrid", { hasText: "shared:vnstat-json" })).toBeVisible();
  await presetRegistrySearchField.selectOption("Domain");
  await presetRegistrySearch.fill("runtime_traffic_accounting_source");
  await expect(panel.locator(".historyRow.dataSourcePresetGrid", { hasText: "builtin:interface_counters" })).toBeVisible();
  await presetRegistrySearch.fill("");
  await panel.getByLabel("Assignment domain").selectOption("runtime_traffic_accounting_source");
  await panel.getByLabel("Preset", { exact: true }).selectOption("11111111-1111-4111-8111-111111111111");
  await checkControl(panel.locator(".presetTargetList label", { hasText: "west" }).locator("input"));
  await activate(panel.getByRole("button", { name: "Assign preset" }));

  const request = await page.evaluate(() => {
    const requests = (window as unknown as {
      __vpsmanTestRequests: { dataSourcePresetAssignments: unknown[] };
    }).__vpsmanTestRequests;
    return requests.dataSourcePresetAssignments.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: false,
    domain: "runtime_traffic_accounting_source",
    pools: ["pool-west"],
    preset_id: "11111111-1111-4111-8111-111111111111",
  });
});

test("creates bound rebuild enrollment tokens from the access panel", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense access administration is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Access" }));

  await expect(page.getByRole("heading", { name: "Enrollment tokens" })).toBeVisible();
  await expect(page.getByText("vpsm12345678")).toBeVisible();
  const inspector = page.locator(".accessInspector");
  await inspector.getByLabel("Enrollment token purpose").selectOption("rebuild_reenrollment");
  await inspector.getByLabel("Enrollment token client id").fill("agent-sfo-01");
  await inspector.getByLabel("Enrollment token ttl").fill("900");
  await inspector.getByLabel("Enrollment default tags").fill("rebuilt,provider:alpha");
  await checkControl(inspector.getByLabel("Confirm rebuild"));
  await activate(inspector.getByRole("button", { name: "Rebuild token" }));

  await expect(inspector.getByText("vpsm_rebuild_token_secret")).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as {
      __vpsmanTestRequests: { enrollmentTokens: unknown[] };
    }).__vpsmanTestRequests;
    return requests.enrollmentTokens.at(-1);
  });
  expect(request).toMatchObject({
    allowed_client_id: "agent-sfo-01",
    confirmed_reenrollment: true,
    default_tags: ["rebuilt", "provider:alpha"],
    preserve_existing_assignments: true,
    purpose: "rebuild_reenrollment",
    ttl_secs: 900,
  });
});

test("shows topology network evidence, speed metrics, and probe latency history", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "topology evidence drilldown is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Topology" }));

  await expect(page.getByRole("heading", { name: "Topology graph" })).toBeVisible();
  await expect(page.getByRole("img", { name: "Topology graph" })).toBeVisible();
  await expect(page.getByText("2 shown / 2 nodes; 1 shown / 1 tunnels")).toBeVisible();
  await expect(page.locator(".topologyGraphPanel").getByText("healthy", { exact: true }).first()).toBeVisible();
  await page.getByLabel("Filter topology graph").fill("fra");
  await expect(page.locator(".topologyGraphPanel").getByText("core-fra-02")).toBeVisible();
  const graphFilter = page.getByRole("group", { name: "Topology health filter" });
  await activate(graphFilter.getByRole("button", { name: "Attention" }));
  await expect(page.locator(".topologyGraphPanel").getByText("0 visible tunnels")).toBeVisible();
  await activate(graphFilter.getByRole("button", { name: "All", exact: true }));
  await page.getByLabel("Filter topology graph").fill("");
  await expect(page.getByRole("heading", { name: "Topology evidence" })).toBeVisible();
  await activate(page.getByRole("button", { name: "Refresh evidence" }));
  await expect(page.getByLabel("Network probe latency history")).toBeVisible();
  const evidence = page.locator(".topologyEvidence");
  await expect(evidence.getByText("1 OSPF update plans")).toBeVisible();
  await expect(evidence.getByText("approval required")).toBeVisible();
  await expect(evidence.getByText("14 -> 22").first()).toBeVisible();
  await expect(evidence.getByText("3 samples")).toBeVisible();
  await expect(evidence.getByText("10.1 Mbps avg", { exact: true })).toBeVisible();
  await expect(evidence.getByText("10.9-14.8 ms; 0.25% loss")).toBeVisible();
  const observationTable = evidence.locator(".observationTable");
  await expect(observationTable.getByText("network_speed_test")).toBeVisible();
  await expect(observationTable.getByText("10.1 Mbps")).toBeVisible();
  await expect(observationTable.getByText("12.4 ms")).toBeVisible();
  await expect(observationTable.getByText("0.25% loss")).toBeVisible();
  await expect(observationTable.getByText("10.255.0.1", { exact: true })).toBeVisible();
  await expect(observationTable.getByText("Runtime adapter unhealthy")).toBeVisible();
  await expect(observationTable.getByText("adapter status failed")).toBeVisible();
  await expect(evidence.getByText("Managed blocks match")).toBeVisible();
});

test("authors external adapter tunnel plans from the topology panel", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense topology authoring is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Topology" }));

  const composer = page.locator(".scheduleComposer", { has: page.getByRole("heading", { name: "Create tunnel plan" }) });
  await composer.scrollIntoViewIfNeeded();
  await composer.getByLabel("Name", { exact: true }).fill("external-openvpn");
  await composer.getByLabel("Interface", { exact: true }).fill("ovpn42");
  await composer.getByLabel("Kind").selectOption("openvpn");
  await composer.getByLabel("Left VPS").selectOption("agent-sfo-01");
  await composer.getByLabel("Right VPS").selectOption("agent-fra-02");
  await composer.getByLabel("Left underlay", { exact: true }).fill("198.51.100.10");
  await composer.getByLabel("Right underlay", { exact: true }).fill("203.0.113.20");
  await composer.getByLabel("Runtime owner").selectOption("external_managed_adapter");
  await composer.getByLabel("Egress Kbps", { exact: true }).fill("100000");
  await composer.getByLabel("Burst KB", { exact: true }).fill("4096");
  await composer.getByLabel("Topology version", { exact: true }).fill("provider-a:42");
  await composer.getByLabel("Start argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nstart\n{interface}");
  await composer.getByLabel("Cleanup argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\ncleanup\n{interface}");
  await composer.getByLabel("Status argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nstatus\n{interface}");
  await composer.getByLabel("Traffic argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nshape\n{interface}");
  await composer.getByLabel("Desired interfaces", { exact: true }).fill("ovpn42");
  await composer.getByLabel("Routes", { exact: true }).fill("10.42.0.0/24,dev=ovpn42,metric=42");
  await activate(composer.getByRole("button", { name: "Save plan" }));

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { tunnelPlans: unknown[] } }).__vpsmanTestRequests;
    return requests.tunnelPlans.at(-1);
  });
  expect(request).toMatchObject({
    interface_name: "ovpn42",
    kind: "openvpn",
    name: "external-openvpn",
    runtime_control: {
      manager: "external_managed_adapter",
      cleanup: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "cleanup", "{interface}"] },
      startup: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "start", "{interface}"] },
      status: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "status", "{interface}"] },
      traffic_limit: {
        burst_kb: 4096,
        egress_kbps: 100000,
      },
      traffic_limit_apply: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "shape", "{interface}"] },
    },
    runtime_topology: {
      desired_interfaces: ["ovpn42"],
      routes: [{ destination_cidr: "10.42.0.0/24", interface_name: "ovpn42", metric: 42 }],
      version: "provider-a:42",
    },
  });
});

test("promotes saved observed tunnel plans into adapter contracts", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "dense topology promotion is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Topology" }));

  const promotionPanel = page.locator(".scheduleComposer", { has: page.getByRole("heading", { name: "Tunnel promotion" }) });
  const adapterForm = promotionPanel.locator("form", { has: page.getByRole("heading", { name: "Adapter contract" }) });
  await promotionPanel.scrollIntoViewIfNeeded();
  await adapterForm.getByLabel("Observed plan").selectOption("eeeeeeee-ffff-4000-8111-222222222222");
  await adapterForm.getByLabel("Name", { exact: true }).fill("external-openvpn-managed");
  await adapterForm.getByLabel("Status argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nstatus\n{interface}");
  await adapterForm.getByLabel("Start argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nstart\n{interface}");
  await adapterForm.getByLabel("Stop argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nstop\n{interface}");
  await adapterForm.getByLabel("Cleanup argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\ncleanup\n{interface}");
  await adapterForm.getByLabel("Traffic argv", { exact: true }).fill("/usr/local/libexec/vpsman-openvpn-adapter\nshape\n{interface}");
  await adapterForm.getByLabel("Egress Kbps", { exact: true }).fill("100000");
  await adapterForm.getByLabel("Burst KB", { exact: true }).fill("4096");
  await adapterForm.getByLabel("Topology version", { exact: true }).fill("adapter:ovpn42");
  await adapterForm.getByLabel("Desired interfaces", { exact: true }).fill("ovpn42");
  await checkControl(adapterForm.getByLabel("Confirmed"));
  await activate(adapterForm.getByRole("button", { name: "Promote adapter" }));

  const request = await page.evaluate(() => {
    const requests = (window as unknown as {
      __vpsmanTestRequests: { tunnelPlanAdapterPromotions: unknown[] };
    }).__vpsmanTestRequests;
    return requests.tunnelPlanAdapterPromotions.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    name: "external-openvpn-managed",
    plan_id: "eeeeeeee-ffff-4000-8111-222222222222",
    runtime_control: {
      manager: "external_managed_adapter",
      cleanup: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "cleanup", "{interface}"] },
      startup: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "start", "{interface}"] },
      status: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "status", "{interface}"] },
      stop: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "stop", "{interface}"] },
      traffic_limit: {
        burst_kb: 4096,
        egress_kbps: 100000,
      },
      traffic_limit_apply: { argv: ["/usr/local/libexec/vpsman-openvpn-adapter", "shape", "{interface}"] },
    },
    runtime_topology: {
      desired_interfaces: ["ovpn42"],
      version: "adapter:ovpn42",
    },
  });
});

test("generates local proof envelopes before dispatching a privileged job", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "privileged dispatch flow is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Jobs" }));

  await expect(page.getByRole("heading", { name: "Dispatch command" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Agent update rollouts" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "File transfer sessions" })).toBeVisible();
  const completedTransferRow = page.locator(".historyRow.fileTransferGrid", { hasText: "/opt/vpsman/app.bin" });
  await expect(completedTransferRow).toBeVisible();
  await expect(completedTransferRow.getByText("1.0 MiB / 1.0 MiB (100%)")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Terminal sessions" })).toBeVisible();
  await expect(page.getByText("/bin/sh -l")).toBeVisible();
  await expect(page.getByText("1 -> 4")).toBeVisible();
  await expect(page.getByText("manual_staging_only")).toBeVisible();
  await expect(page.getByText("staged", { exact: true })).toBeVisible();
  await page.getByLabel("Super password").fill("local-super-password");
  await page.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Use proof" }));
  await expect(page.getByText("Proof unlocked").first()).toBeVisible();

  await page.getByLabel("Command argv").fill("/usr/bin/uptime");
  await checkControl(page.getByLabel("edge-sfo-01"));
  await activate(page.getByRole("button", { name: "Preview" }));
  await expect(page.getByText("1 resolved targets")).toBeVisible();
  await activate(page.getByRole("button", { name: "Dispatch" }));

  await expect(page.getByText(/Job 11111111 accepted; 1 accepted/)).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    argv: ["/usr/bin/uptime"],
    clients: ["agent-sfo-01"],
    command: "/usr/bin/uptime",
    envelope: null,
    operation: { argv: ["/usr/bin/uptime"], pty: false, type: "shell" },
    privileged: true,
  });
  const envelopes = (request as { envelopes: Record<string, { proof: { proof_hex: string } }> }).envelopes;
  expect(envelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);
});

test("dispatches terminal session control operations with local proof", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "terminal control dispatch is covered in the desktop job composer");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Jobs" }));

  const composer = page.locator(".commandComposer");
  await composer.getByLabel("Super password").fill("local-super-password");
  await composer.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(composer.getByRole("button", { name: "Use proof" }));
  await activate(composer.getByRole("button", { name: "Terminal" }));
  await composer.getByLabel("Terminal argv").fill("/bin/sh -l");
  await composer.getByLabel("Terminal cwd").fill("/root");
  await composer.getByLabel("Terminal columns").fill("100");
  await composer.getByLabel("Terminal rows").fill("30");
  await checkControl(composer.getByLabel("edge-sfo-01"));
  await activate(composer.getByRole("button", { name: "Dispatch" }));

  await expect(page.getByText(/Job 11111111 accepted; 1 accepted/)).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> } })
      .__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    clients: ["agent-sfo-01"],
    command: "terminal_session",
    operation: {
      argv: ["/bin/sh", "-l"],
      cols: 100,
      cwd: "/root",
      rows: 30,
      type: "terminal_open",
    },
    privileged: true,
  });
  expect((request as { operation: { session_id: string } }).operation.session_id).toMatch(/[0-9a-f-]{36}/);
  const envelopes = (request as { envelopes: Record<string, { proof: { proof_hex: string } }> }).envelopes;
  expect(envelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);
});

test("previews degraded update targets and sends explicit force override", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "target impact controls are covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Jobs" }));

  await page.getByLabel("Super password").fill("local-super-password");
  await page.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Use proof" }));
  await activate(page.getByRole("button", { name: "Update" }));
  await page.getByLabel("Agent update artifact URL").fill("https://updates.example/vpsman-agent");
  await page.getByLabel("Agent update SHA-256").fill("a".repeat(64));
  await checkControl(page.locator(".commandComposer").getByLabel("backup-nyc-03"));
  await checkControl(page.locator(".commandComposer").getByLabel("Confirmed"));
  await activate(page.getByRole("button", { name: "Preview" }));

  const impact = page.locator(".commandComposer .targetImpactPreview");
  await expect(impact.getByText("1 target / agent update")).toBeVisible();
  await expect(impact.getByText("Would degrade")).toBeVisible();
  await expect(impact.getByText("backup-nyc-03")).toBeVisible();

  await checkControl(page.getByLabel("Force unprivileged job best effort"));
  await expect(impact.getByText("Forced best effort")).toBeVisible();
  await activate(page.getByRole("button", { name: "Dispatch" }));
  await expect(page.getByText(/Job 11111111 accepted; 1 accepted/)).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() => {
        const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
        return requests.jobs.length;
      }),
    )
    .toBeGreaterThan(0);

  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    clients: ["agent-nyc-03"],
    command: "agent_update",
    force_unprivileged: true,
    operation: {
      artifact_url: "https://updates.example/vpsman-agent",
      sha256_hex: "a".repeat(64),
      type: "agent_update",
    },
    privileged: true,
  });
});

test("cancels pending scheduled approval jobs from the console", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "job cancellation controls are covered in the desktop console layout");

  await page.addInitScript(() => {
    const pendingJobId = "55555555-aaaa-4bbb-8ccc-dddddddddddd";
    const previousFetch = window.fetch.bind(window);
    let canceled = false;
    const response = (body: unknown) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        }),
      );
    const job = () => ({
      actor_id: null,
      command_type: "scheduled_shell_argv",
      completed_at: canceled ? "2026-05-31T10:12:00Z" : null,
      created_at: "2026-05-31T10:11:00Z",
      id: pendingJobId,
      payload_hash: "5".repeat(64),
      privileged: true,
      status: canceled ? "canceled" : "approval_required",
      target_count: 1,
    });
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
      if (pathname === "/api/v1/jobs" && method === "GET") {
        return response([job()]);
      }
      if (pathname === `/api/v1/jobs/${pendingJobId}/cancel` && method === "POST") {
        const body = typeof init?.body === "string" ? JSON.parse(init.body) : null;
        const requests = (window as unknown as { __vpsmanTestRequests: { jobCancels?: unknown[] } }).__vpsmanTestRequests;
        requests.jobCancels = [...(requests.jobCancels ?? []), body];
        canceled = true;
        return response({
          canceled: true,
          canceled_targets: 1,
          cancel_requested_targets: 0,
          job_id: pendingJobId,
          status: "canceled",
        });
      }
      return previousFetch(input, init);
    };
  });

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Jobs" }));
  await expect(page.getByText("approval_required")).toBeVisible();
  await activate(page.getByRole("button", { name: "Cancel pending job" }));
  await expect(page.getByText("canceled", { exact: true })).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobCancels: unknown[] } }).__vpsmanTestRequests;
    return requests.jobCancels.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    reason: "Canceled from panel while status was approval_required",
  });
});

test("requests active in-flight job cancellation from the console", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "active job cancellation controls are covered in the desktop console layout");

  await page.addInitScript(() => {
    const activeJobId = "66666666-aaaa-4bbb-8ccc-dddddddddddd";
    const previousFetch = window.fetch.bind(window);
    let cancelRequested = false;
    const response = (body: unknown) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        }),
      );
    const job = () => ({
      actor_id: null,
      command_type: "shell_script",
      completed_at: null,
      created_at: "2026-05-31T10:13:00Z",
      id: activeJobId,
      payload_hash: "6".repeat(64),
      privileged: true,
      status: cancelRequested ? "cancel_requested" : "dispatching",
      target_count: 1,
    });
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
      if (pathname === "/api/v1/jobs" && method === "GET") {
        return response([job()]);
      }
      if (pathname === `/api/v1/jobs/${activeJobId}/cancel` && method === "POST") {
        const body = typeof init?.body === "string" ? JSON.parse(init.body) : null;
        const requests = (window as unknown as { __vpsmanTestRequests: { jobCancels?: unknown[] } }).__vpsmanTestRequests;
        requests.jobCancels = [...(requests.jobCancels ?? []), body];
        cancelRequested = true;
        return response({
          canceled: true,
          canceled_targets: 0,
          cancel_requested_targets: 1,
          job_id: activeJobId,
          status: "cancel_requested",
        });
      }
      return previousFetch(input, init);
    };
  });

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Jobs" }));
  await expect(page.getByText("dispatching")).toBeVisible();
  await activate(page.getByRole("button", { name: "Cancel active job" }));
  await expect(page.getByText("cancel_requested", { exact: true })).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobCancels: unknown[] } }).__vpsmanTestRequests;
    return requests.jobCancels.at(-1);
  });
  expect(request).toMatchObject({
    confirmed: true,
    reason: "Canceled from panel while status was dispatching",
  });
});

test("decrypts backup artifacts locally before dispatching executable restores", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "restore artifact dispatch is covered in the desktop console layout");

  const privateKeyHex = "07".repeat(32);
  const fixture = buildEncryptedBackupArtifactFixture(privateKeyHex, "agent-sfo-01");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Backups" }));

  await expect(page.getByRole("heading", { name: "Backup requests" })).toBeVisible();
  await page.getByLabel("Super password").fill("local-super-password");
  await page.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Use proof" }));
  await expect(page.getByText("Proof unlocked").first()).toBeVisible();

  await page.getByLabel("Restore source backup request").selectOption(backupId);
  await page.getByLabel("Restore target client").selectOption("agent-fra-02");
  await page.getByLabel("Restore destination root").fill("/restore");
  await checkControl(page.getByLabel("Confirmed metadata plan"));
  await activate(page.getByRole("button", { name: "Plan restore" }));
  await expect(page.getByText(/Restore cccccccc planned_metadata_only/)).toBeVisible();
  const restorePlanRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { restorePlans: unknown[] } }).__vpsmanTestRequests;
    return requests.restorePlans.at(-1);
  });
  const expectedPlanOperation = {
    type: "restore",
    source_backup_request_id: backupId,
    paths: ["/etc/hostname"],
    include_config: false,
    destination_root: "/restore",
    archive_base64: null,
    archive_size_bytes: null,
    archive_sha256_hex: null,
  };
  expect(restorePlanRequest).toMatchObject({
    destination_root: "/restore",
    envelope: {
      payload_hash_hex: sha256Hex(new TextEncoder().encode(JSON.stringify(expectedPlanOperation))),
    },
    include_config: false,
    paths: ["/etc/hostname"],
    source_backup_request_id: backupId,
    target_client_id: "agent-fra-02",
  });
  await page.getByLabel("Restore artifact file").setInputFiles({
    buffer: Buffer.from(JSON.stringify(fixture.artifact)),
    mimeType: "application/json",
    name: "backup-artifact.json",
  });
  await page.getByLabel("Backup private key hex").fill(privateKeyHex);
  await page.getByLabel("Restore timeout seconds").fill("120");
  await checkControl(page.getByLabel("Confirmed executable restore"));
  await activate(page.getByRole("button", { name: "Run restore" }));

  await expect(page.getByText(/Restore job 11111111 accepted/)).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain(privateKeyHex);
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    argv: [],
    clients: ["agent-fra-02"],
    command: "restore",
    confirmed: true,
    destructive: true,
    operation: {
      archive_sha256_hex: fixture.archiveSha256Hex,
      archive_size_bytes: fixture.archiveBytes.length,
      destination_root: "/restore",
      include_config: false,
      paths: ["/etc/hostname"],
      source_backup_request_id: backupId,
      type: "restore",
    },
    privileged: true,
    timeout_secs: 120,
  });
  const operation = (request as { operation: { archive_base64: string } }).operation;
  expect(operation.archive_base64).toBe(Buffer.from(fixture.archiveBytes).toString("base64"));
  const envelopes = (request as { envelopes: Record<string, { proof: { proof_hex: string } }> }).envelopes;
  expect(envelopes["agent-fra-02"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);

  const restoreJobId = "11111111-2222-4333-8444-555555555555";
  const restoreStatusBase64 = Buffer.from(
    JSON.stringify({
      type: "restore",
      rollback_available: true,
      restored_files: [
        {
          archive_path: "/etc/hostname",
          destination_path: "/restore/etc/hostname",
          rollback_path: "/restore/etc/.vpsman-restore-hostname.bak",
          size_bytes: 64,
          sha256_hex: "a".repeat(64),
        },
      ],
    }),
  ).toString("base64");
  await page.evaluate(
    ({ restoreJobId, restoreStatusBase64 }) => {
      const previousFetch = window.fetch.bind(window);
      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        const pathname = new URL(url, window.location.href).pathname;
        if (pathname === `/api/v1/jobs/${restoreJobId}/outputs`) {
          return new Response(
            JSON.stringify([
              {
                client_id: "agent-fra-02",
                data_base64: restoreStatusBase64,
                done: true,
                exit_code: 0,
                job_id: restoreJobId,
                seq: 0,
                stream: "status",
              },
            ]),
            { headers: { "Content-Type": "application/json" }, status: 200 },
          );
        }
        return previousFetch(input, init);
      };
    },
    { restoreJobId, restoreStatusBase64 },
  );
  await expect(page.getByLabel("Restore rollback source job id")).toHaveValue(restoreJobId);
  await expect(page.getByLabel("Restore rollback target client")).toHaveValue("agent-fra-02");
  await page.getByLabel("Restore rollback timeout seconds").fill("45");
  await checkControl(page.getByLabel("Confirmed restore rollback"));
  await activate(page.getByRole("button", { name: "Rollback restore" }));
  await expect(page.getByText(/Restore rollback job 11111111 accepted/)).toBeVisible();
  const rollbackRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(rollbackRequest)).not.toContain("local-super-password");
  expect(rollbackRequest).toMatchObject({
    argv: [],
    clients: ["agent-fra-02"],
    command: "restore_rollback",
    confirmed: true,
    destructive: true,
    operation: {
      restored_files: [
        {
          archive_path: "/etc/hostname",
          destination_path: "/restore/etc/hostname",
          restored_sha256_hex: "a".repeat(64),
          restored_size_bytes: 64,
          rollback_path: "/restore/etc/.vpsman-restore-hostname.bak",
        },
      ],
      source_restore_job_id: restoreJobId,
      type: "restore_rollback",
    },
    privileged: true,
    timeout_secs: 45,
  });
});

test("promotes retained backup output into a stored artifact", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "backup handoff controls are covered in the desktop layout");

  const sourceJobId = "99999999-2222-4333-8444-555555555555";

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Backups" }));

  await page.getByLabel("Artifact backup request").selectOption(backupId);
  await page.getByLabel("Backup artifact handoff source job ID").fill(sourceJobId);
  await checkControl(page.getByLabel("Confirmed retained output promotion"));
  await activate(page.getByRole("button", { name: "Promote retained output" }));

  await expect(page.getByText(/Artifact dddddddd uploaded/)).toBeVisible();
  const handoffRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { backupArtifactHandoffs: unknown[] } })
      .__vpsmanTestRequests;
    return requests.backupArtifactHandoffs.at(-1);
  });
  expect(handoffRequest).toMatchObject({
    confirmed: true,
    job_id: sourceJobId,
  });
});

test("dispatches topology network apply, rollback, status, probe, and speed test with local proof", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "network apply proof flow is covered in the desktop console layout");

  await page.goto("/");
  await activate(page.getByRole("button", { name: "Topology" }));

  await expect(page.getByRole("heading", { name: "Network apply" })).toBeVisible();
  await page.getByLabel("Super password").fill("local-super-password");
  await page.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Use proof" }));
  await expect(page.getByText("Proof unlocked").first()).toBeVisible();

  await page.getByLabel("Network apply plan").selectOption(tunnelPlans[0].id);
  await page.getByLabel("Network apply endpoint side").selectOption("left");
  await page.getByLabel("Network apply timeout seconds").fill("90");
  await checkControl(page.getByLabel("Confirm network apply"));
  await activate(page.getByRole("button", { name: "Apply side" }));

  await expect(page.getByText(/Apply job 11111111 accepted/)).toBeVisible();
  const request = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(request)).not.toContain("local-super-password");
  expect(request).toMatchObject({
    argv: [],
    clients: ["agent-sfo-01"],
    command: "network_apply",
    confirmed: true,
    destructive: true,
    operation: {
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_apply",
    },
    privileged: true,
    timeout_secs: 90,
  });
  const operation = (
    request as {
      operation: {
        bird2_sha256_hex: string;
        config_backend: string;
        config_sha256_hex: string;
        ifupdown_sha256_hex: string;
      };
    }
  ).operation;
  expect(operation.ifupdown_sha256_hex).toBe(sha256Hex(new TextEncoder().encode(tunnelPlans[0].plan.ifupdown_snippet)));
  expect(operation.config_backend).toBe("ifupdown");
  expect(operation.config_sha256_hex).toBe(
    sha256Hex(
      new TextEncoder().encode(
        [
          "vpsman-network-backend-file-v1",
          "backend=ifupdown",
          "path=/etc/network/interfaces.d/vpsman-tunnels",
          "kind=ifupdown",
          "contents-sha256-context",
          tunnelPlans[0].plan.ifupdown_snippet,
          "",
        ].join("\n"),
      ),
    ),
  );
  expect(operation.bird2_sha256_hex).toBe(
    sha256Hex(new TextEncoder().encode(tunnelPlans[0].plan.bird2_interface_snippet)),
  );
  const envelopes = (request as { envelopes: Record<string, { payload_hash_hex: string; proof: { proof_hex: string } }> }).envelopes;
  expect(envelopes["agent-sfo-01"].payload_hash_hex).toBe(sha256Hex(new TextEncoder().encode(JSON.stringify(operation))));
  expect(envelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);

  await activate(page.getByRole("button", { name: "Rollback side" }));
  await expect(page.getByText(/Rollback job 11111111 accepted/)).toBeVisible();
  const rollbackRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(rollbackRequest)).not.toContain("local-super-password");
  expect(rollbackRequest).toMatchObject({
    argv: [],
    clients: ["agent-sfo-01"],
    command: "network_rollback",
    confirmed: true,
    destructive: true,
    operation: {
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_rollback",
    },
    privileged: true,
    timeout_secs: 90,
  });
  const rollbackOperation = (rollbackRequest as { operation: { type: string; plan: unknown; side: string } }).operation;
  const rollbackEnvelopes = (
    rollbackRequest as { envelopes: Record<string, { payload_hash_hex: string; proof: { proof_hex: string } }> }
  ).envelopes;
  expect(rollbackEnvelopes["agent-sfo-01"].payload_hash_hex).toBe(
    sha256Hex(new TextEncoder().encode(JSON.stringify(rollbackOperation))),
  );
  expect(rollbackEnvelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);

  await activate(page.getByRole("button", { name: "Inspect side" }));
  await expect(page.getByText(/Status job 11111111 accepted/)).toBeVisible();
  const statusRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(statusRequest)).not.toContain("local-super-password");
  expect(statusRequest).toMatchObject({
    argv: [],
    clients: ["agent-sfo-01"],
    command: "network_status",
    confirmed: false,
    destructive: false,
    operation: {
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_status",
    },
    privileged: true,
    timeout_secs: 90,
  });
  const statusOperation = (statusRequest as { operation: { type: string; plan: unknown; side: string } }).operation;
  const statusEnvelopes = (
    statusRequest as { envelopes: Record<string, { payload_hash_hex: string; proof: { proof_hex: string } }> }
  ).envelopes;
  expect(statusEnvelopes["agent-sfo-01"].payload_hash_hex).toBe(
    sha256Hex(new TextEncoder().encode(JSON.stringify(statusOperation))),
  );
  expect(statusEnvelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);

  await page.getByLabel("Network probe count").fill("4");
  await page.getByLabel("Network probe interval milliseconds").fill("700");
  await activate(page.getByRole("button", { name: "Probe latency" }));
  await expect(page.getByText(/Probe job 11111111 accepted/)).toBeVisible();
  const probeRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(probeRequest)).not.toContain("local-super-password");
  expect(probeRequest).toMatchObject({
    argv: [],
    clients: ["agent-sfo-01"],
    command: "network_probe",
    confirmed: false,
    destructive: false,
    operation: {
      count: 4,
      interval_ms: 700,
      plan: tunnelPlans[0].plan,
      side: "left",
      type: "network_probe",
    },
    privileged: true,
    timeout_secs: 90,
  });
  const probeOperation = (
    probeRequest as { operation: { type: string; plan: unknown; side: string; count: number; interval_ms: number } }
  ).operation;
  const probeEnvelopes = (
    probeRequest as { envelopes: Record<string, { payload_hash_hex: string; proof: { proof_hex: string } }> }
  ).envelopes;
  expect(probeEnvelopes["agent-sfo-01"].payload_hash_hex).toBe(
    sha256Hex(new TextEncoder().encode(JSON.stringify(probeOperation))),
  );
  expect(probeEnvelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);

  await page.getByLabel("Network speed test duration seconds").fill("5");
  await page.getByLabel("Network speed test max mebibytes").fill("8");
  await page.getByLabel("Network speed test rate limit Kbps").fill("25000");
  await page.getByLabel("Network speed test TCP port").fill("55201");
  await page.getByLabel("Network speed test connect timeout milliseconds").fill("2500");
  await activate(page.getByRole("button", { name: "Test speed" }));
  await expect(page.getByText(/Speed test job 11111111 accepted/)).toBeVisible();
  const speedRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  expect(JSON.stringify(speedRequest)).not.toContain("local-super-password");
  expect(speedRequest).toMatchObject({
    argv: [],
    clients: ["agent-sfo-01", "agent-fra-02"],
    command: "network_speed_test",
    confirmed: false,
    destructive: false,
    operation: {
      connect_timeout_ms: 2500,
      duration_secs: 5,
      max_bytes: 8 * 1024 * 1024,
      plan: tunnelPlans[0].plan,
      port: 55201,
      rate_limit_kbps: 25000,
      server_side: "left",
      type: "network_speed_test",
    },
    privileged: true,
    timeout_secs: 90,
  });
  const speedOperation = (
    speedRequest as {
      operation: {
        connect_timeout_ms: number;
        duration_secs: number;
        max_bytes: number;
        plan: unknown;
        port: number;
        rate_limit_kbps: number;
        server_side: string;
        type: string;
      };
    }
  ).operation;
  const speedEnvelopes = (
    speedRequest as { envelopes: Record<string, { payload_hash_hex: string; proof: { proof_hex: string } }> }
  ).envelopes;
  const speedPayloadHash = sha256Hex(new TextEncoder().encode(JSON.stringify(speedOperation)));
  expect(speedEnvelopes["agent-sfo-01"].payload_hash_hex).toBe(speedPayloadHash);
  expect(speedEnvelopes["agent-fra-02"].payload_hash_hex).toBe(speedPayloadHash);
  expect(speedEnvelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);
  expect(speedEnvelopes["agent-fra-02"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);

  await expect(page.getByRole("heading", { name: "OSPF cost apply" })).toBeVisible();
  await page.getByLabel("OSPF proof secret").fill("local-super-password");
  await page.getByLabel("OSPF proof salt").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Use OSPF proof" }));
  await page.getByLabel("OSPF update plan").selectOption(ospfUpdatePlans[0].plan_id);
  await page.getByLabel("OSPF update endpoint side").selectOption("left");
  await page.getByLabel("OSPF update timeout seconds").fill("45");
  await checkControl(page.getByLabel("Confirm OSPF cost update"));
  await activate(page.getByRole("button", { name: "Apply cost" }));
  await expect(page.getByText(/OSPF update job 11111111 accepted/)).toBeVisible();
  const ospfRequest = await page.evaluate(() => {
    const requests = (window as unknown as { __vpsmanTestRequests: { jobs: unknown[] } }).__vpsmanTestRequests;
    return requests.jobs.at(-1);
  });
  const proposedPlan = {
    ...tunnelPlans[0].plan,
    recommended_ospf_cost: ospfUpdatePlans[0].recommended_ospf_cost,
  };
  expect(JSON.stringify(ospfRequest)).not.toContain("local-super-password");
  expect(ospfRequest).toMatchObject({
    argv: [],
    clients: ["agent-sfo-01"],
    command: "network_ospf_cost_update",
    confirmed: true,
    destructive: true,
    operation: {
      current_ospf_cost: ospfUpdatePlans[0].current_ospf_cost,
      plan: proposedPlan,
      recommended_ospf_cost: ospfUpdatePlans[0].recommended_ospf_cost,
      side: "left",
      type: "network_ospf_cost_update",
    },
    privileged: true,
    timeout_secs: 45,
  });
  const ospfOperation = (
    ospfRequest as {
      operation: {
        bird2_sha256_hex: string;
        current_ospf_cost: number;
        plan: unknown;
        recommended_ospf_cost: number;
        side: string;
        type: string;
      };
    }
  ).operation;
  expect(ospfOperation.bird2_sha256_hex).toBe(
    sha256Hex(new TextEncoder().encode(ospfUpdatePlans[0].proposed_left_bird2_interface_snippet)),
  );
  const ospfEnvelopes = (
    ospfRequest as { envelopes: Record<string, { payload_hash_hex: string; proof: { proof_hex: string } }> }
  ).envelopes;
  expect(ospfEnvelopes["agent-sfo-01"].payload_hash_hex).toBe(
    sha256Hex(new TextEncoder().encode(JSON.stringify(ospfOperation))),
  );
  expect(ospfEnvelopes["agent-sfo-01"].proof.proof_hex).toMatch(/^[0-9a-f]+$/);
});
