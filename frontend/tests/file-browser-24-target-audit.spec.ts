import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { activate, openConsoleSubpage, unlockPrivilegeFromTop } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
  await installTwentyFourTargetFileMock(page);
});

test("bulk file operations remain scannable with 24 VPS targets", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "bulk file operations are a dense desktop panel");

  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("vpsman.multiFile.selectorExpression", "provider:alpha && country:US"));
  await openConsoleSubpage(page, "Jobs", "Multi files");
  await expect(page.getByRole("heading", { name: "Multi files" })).toBeVisible();
  await unlockPrivilege(page);

  await activate(page.getByRole("button", { name: "Preview" }));
  await expect(page.getByText("24 VPSs resolved")).toBeVisible();
  await expect(page.getByText("edge-us-00", { exact: true })).toBeVisible();
  await expect(page.locator(".bulkSummaryClients span").filter({ hasText: "edge-us-00" }).first()).toHaveAttribute("title", "a0000000-target-00");

  await page.getByLabel("Bulk file path").fill("/var/log/nginx/");
  await activate(page.getByRole("button", { name: "Run download" }));
  await expect(page.getByText("Confirm multi-file operation")).toBeVisible();
  await expect(page.getByText("Download /var/log/nginx on 24 VPSs")).toBeVisible();
  await activate(page.getByRole("button", { name: "Run bulk action" }));
  const resultPanel = page.getByLabel("Execution result");
  await expect(resultPanel).toBeVisible();
  await expect(resultPanel.locator(".executionResultStats span").filter({ hasText: "pushed" }).filter({ hasText: "23/24" })).toBeVisible();
  await expect(resultPanel.locator(".executionResultStats span").filter({ hasText: "doing" }).filter({ hasText: "0" })).toBeVisible();
  await expect(resultPanel.locator(".executionResultStats span").filter({ hasText: "retrieved" }).filter({ hasText: "22" })).toBeVisible();

  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "22 VPSs" })).toBeVisible();
  await expect(resultPanel.locator(".executionResultStats span").filter({ hasText: "failed" }).filter({ hasText: "1" })).toBeVisible();
  await expect(resultPanel.locator(".executionResultStats span").filter({ hasText: "unavailable" }).filter({ hasText: "1" })).toBeVisible();
  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "1 VPS" }).filter({ hasText: "stale" })).toBeVisible();
  await expect(page.locator(".bulkSummaryList summary").filter({ hasText: "1 VPS" }).filter({ hasText: "offline" })).toBeVisible();
  await expect(resultPanel.getByText("partial success: 22 done, 1 failed, 1 unavailable", { exact: true })).toBeVisible();
  const reasons = resultPanel.getByLabel("Failed target reasons");
  await expect(reasons.getByText("stale: file download command_version mismatch", { exact: true })).toBeVisible();
  await expect(reasons.getByText("edge-us-22", { exact: true })).toBeVisible();
  await expect(page.getByText("Same hierarchy and content")).toBeVisible();
  await expect(page.getByText("Same hash")).toBeVisible();
  await expect(page.getByText("Content preview")).toBeVisible();
  await expect(page.locator(".bulkEvidenceBox").getByText("88888888")).toBeVisible();
  await expect(page.locator(".bulkEvidenceBox").getByText("directory · nginx.tar · application/x-tar")).toBeVisible();
  await expect(page.getByText("edge-us-23", { exact: true }).first()).toBeVisible();
  await expect(page.locator(".bulkSummaryClients span").filter({ hasText: "edge-us-23" }).first()).toHaveAttribute("title", "a0000023-target-23");
  await expect(page.locator(".bulkSummaryClients span").filter({ hasText: "edge-us-23_a0000023" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Download Archive" })).toHaveCount(1);

  const layout = await collectLayoutSignals(page, ".multiFilePanel");
  expect(layout.horizontalOverflowPx).toBeLessThanOrEqual(1);
  expect(layout.clippedControls).toEqual([]);
  expect(layout.overlaps).toEqual([]);
  await page.screenshot({ fullPage: true, path: testInfo.outputPath("bulk-24-summary.png") });
});

test("bulk download summary distinguishes file and hierarchy discrepancies", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "bulk file operations are a dense desktop panel");

  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("vpsman.multiFile.selectorExpression", "provider:alpha && country:US"));
  await openConsoleSubpage(page, "Jobs", "Multi files");
  await unlockPrivilege(page);
  await activate(page.getByRole("button", { name: "Preview" }));

  await page.getByLabel("Bulk file path").fill("/same-tree-diff/");
  await activate(page.getByRole("button", { name: "Run download" }));
  await activate(page.getByRole("button", { name: "Run bulk action" }));
  await expect(page.getByText("File content differs")).toBeVisible();
  await expect(page.getByText("Hierarchy matches; differing files are listed by relative path.")).toBeVisible();
  await expect(page.getByText("sites/app.conf")).toBeVisible();
  await expect(page.getByText("edge-us-21", { exact: true }).first()).toBeVisible();
  await page.screenshot({ fullPage: true, path: testInfo.outputPath("bulk-same-tree-diff.png") });

  await page.getByLabel("Bulk file path").fill("/different-tree/");
  await activate(page.getByRole("button", { name: "Run download" }));
  await activate(page.getByRole("button", { name: "Run bulk action" }));
  await expect(page.getByText("Hierarchy differs")).toBeVisible();
  await expect(page.getByText("Directory tree is not consistent across targets; compare hierarchy before trusting content hashes.")).toBeVisible();
  await expect(page.getByText("File content differs")).toHaveCount(0);
  await page.screenshot({ fullPage: true, path: testInfo.outputPath("bulk-hierarchy-diff.png") });
});

async function installTwentyFourTargetFileMock(page: Page) {
  await page.addInitScript(() => {
    const previousFetch = window.fetch.bind(window);
    const agents = Array.from({ length: 24 }, (_, index) => ({
      capabilities: {
        can_apply_process_limits: true,
        can_attempt_privileged_ops: true,
        can_manage_runtime_tunnels: true,
        effective_uid: 0,
        privilege_mode: "root",
        unprivileged_hint: null,
      },
      display_name: `edge-us-${String(index).padStart(2, "0")}`,
      id: `a${String(index).padStart(7, "0")}-target-${String(index).padStart(2, "0")}`,
      status: index === 22 ? "stale" : index === 23 ? "offline" : "online",
      tags: ["provider:alpha", "country:US", "edge"],
    }));
    const jobOutputs: Record<string, unknown[]> = {};
    const jobTargets: Record<string, unknown[]> = {};
    let counter = 0;
    const jsonResponse = (body: unknown) =>
      Promise.resolve(new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" }, status: 200 }));
    const readJsonBody = async (input: RequestInfo | URL, init?: RequestInit) => {
      if (typeof init?.body === "string") {
        return JSON.parse(init.body);
      }
      if (input instanceof Request) {
        return input.clone().json();
      }
      return null;
    };
    const statusOutputBody = (value: unknown) => btoa(JSON.stringify(value));
    const bytesToBase64 = (text: string) => btoa(text);
    const outputReads: Record<string, number> = {};
    const manifestEntries = (sha256Hex: string) => [
      { kind: "directory", path: "sites" },
      { kind: "file", path: "sites/app.conf", sha256_hex: sha256Hex, size_bytes: 12 },
    ];
    const downloadStatus = (operationPath: string, index: number, data: string) => {
      const variant = operationPath.includes("same-tree-diff") && index >= 12 ? "changed" : "base";
      const hierarchyVariant = operationPath.includes("different-tree") && index >= 12 ? "other-tree" : "base-tree";
      const fileHash = variant === "changed" ? "d".repeat(64) : "c".repeat(64);
      const hierarchyHash = hierarchyVariant === "other-tree" ? "e".repeat(64) : "b".repeat(64);
      const contentHash = hierarchyVariant === "other-tree" ? "f".repeat(64) : variant === "changed" ? "9".repeat(64) : "8".repeat(64);
      return {
        archive: true,
        content_manifest_sha256_hex: contentHash,
        content_type: "application/x-tar",
        directory_count: hierarchyVariant === "other-tree" ? 2 : 1,
        file_count: 1,
        filename: "nginx.tar",
        hierarchy_sha256_hex: hierarchyHash,
        manifest_entries: hierarchyVariant === "other-tree"
          ? [{ kind: "directory", path: "conf" }, { kind: "file", path: "conf/app.conf", sha256_hex: fileHash, size_bytes: 12 }]
          : manifestEntries(fileHash),
        manifest_entry_count: 2,
        manifest_truncated: false,
        path: operationPath,
        sha256_hex: contentHash,
        size_bytes: data.length,
        source_kind: "directory",
        status: "completed",
        total_file_bytes: 12,
        type: "file_download",
      };
    };
    const matchingTargets = (selectorExpression?: string) => {
      const selector = selectorExpression?.toLocaleLowerCase() ?? "";
      if (!selector || selector === "id:*" || selector.includes("provider:alpha") || selector.includes("country:us")) {
        return agents;
      }
      return [];
    };

    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();

      if (pathname === "/api/v1/agents" && method === "GET") {
        return jsonResponse(agents);
      }
      if (pathname === "/api/v1/bulk/resolve" && method === "POST") {
        const body = await readJsonBody(input, init);
        const targets = matchingTargets((body as { selector_expression?: string } | null)?.selector_expression);
        return jsonResponse({
          target_count: targets.length,
          targets,
        });
      }
      if (pathname === "/api/v1/jobs" && method === "POST") {
        const body = await readJsonBody(input, init);
        const operation = (body as { operation?: { type?: string; path?: string } } | null)?.operation;
        if (operation?.type === "file_download") {
          const targets = matchingTargets((body as { selector_expression?: string } | null)?.selector_expression);
          const outputTargets = targets.filter((agent) => agent.status === "online");
          const jobId = `99999999-8888-4777-9666-${String(counter).padStart(12, "0")}`;
          counter += 1;
          jobOutputs[jobId] = outputTargets.flatMap((agent, index) => {
            const data = `tar-for-${agent.id}`;
            return [
              {
                client_id: agent.id,
                created_at: "2026-06-02T10:11:00Z",
                data_base64: bytesToBase64(data),
                done: false,
                exit_code: null,
                job_id: jobId,
                seq: index * 2,
                stream: "stdout",
              },
              {
                client_id: agent.id,
                created_at: "2026-06-02T10:11:00Z",
                data_base64: statusOutputBody(downloadStatus(operation.path ?? "/var/log/nginx/", index, data)),
                done: true,
                exit_code: 0,
                job_id: jobId,
                seq: index * 2 + 1,
                stream: "status",
              },
            ];
          });
          const outputIds = new Set(outputTargets.map((agent) => agent.id));
          jobTargets[jobId] = targets.map((agent) => ({
            client_id: agent.id,
            completed_at: agent.status === "offline" ? null : "2026-06-02T10:11:00Z",
            exit_code: outputIds.has(agent.id) ? 0 : agent.status === "stale" ? 2 : null,
            job_id: jobId,
            message: outputIds.has(agent.id)
              ? "completed"
              : agent.status === "stale"
                ? "stale: file download command_version mismatch"
                : "agent offline",
            started_at: outputIds.has(agent.id) || agent.status === "stale" ? "2026-06-02T10:10:59Z" : null,
            status: outputIds.has(agent.id) ? "completed" : agent.status === "stale" ? "failed" : "dispatch_failed",
          }));
          return jsonResponse({ accepted_targets: targets.filter((agent) => agent.status !== "offline").length, job_id: jobId, status: "accepted" });
        }
      }
      const targetMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/targets$/);
      if (targetMatch && method === "GET" && jobTargets[targetMatch[1]]) {
        return jsonResponse(jobTargets[targetMatch[1]]);
      }
      const outputMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs$/);
      if (outputMatch && method === "GET" && jobOutputs[outputMatch[1]]) {
        const jobId = outputMatch[1];
        const readCount = outputReads[jobId] ?? 0;
        outputReads[jobId] = readCount + 1;
        if (readCount === 0) {
          return jsonResponse(jobOutputs[jobId].slice(0, 10));
        }
        return jsonResponse(jobOutputs[jobId]);
      }
      const bundleMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs\/download-bundle$/);
      if (bundleMatch && method === "GET" && jobOutputs[bundleMatch[1]]) {
        return Promise.resolve(new Response(new TextEncoder().encode("server-side tar bundle"), { headers: { "Content-Type": "application/x-tar" }, status: 200 }));
      }
      return previousFetch(input, init);
    };
  });
}

async function unlockPrivilege(page: Page) {
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Jobs", "Multi files");
}

async function collectLayoutSignals(page: Page, scopeSelector: string) {
  return page.evaluate((selector) => {
    const scope = document.querySelector(selector) ?? document.body;
    const visible = (element: Element) => {
      const rect = element.getBoundingClientRect();
      const style = window.getComputedStyle(element);
      return rect.width > 0 && rect.height > 0 && style.display !== "none" && style.visibility !== "hidden";
    };
    const label = (element: Element) => ({
      className: element instanceof HTMLElement ? String(element.className) : "",
      tagName: element.tagName.toLowerCase(),
      text: (element.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 96),
    });
    const controls = Array.from(scope.querySelectorAll("button, input, select, textarea, summary, .bulkSummaryList details, .searchExpressionInput")).filter(visible);
    const clippedControls = controls
      .filter((element) => {
        if (!(element instanceof HTMLElement)) {
          return false;
        }
        if (element.tagName.toLowerCase() === "input" && element.getAttribute("type") === "file") {
          return false;
        }
        return element.scrollWidth - element.clientWidth > 2 || element.scrollHeight - element.clientHeight > 2;
      })
      .map(label)
      .slice(0, 8);
    const overlaps: Array<Record<string, unknown>> = [];
    for (let index = 0; index < controls.length; index += 1) {
      const leftElement = controls[index];
      const left = leftElement.getBoundingClientRect();
      for (let cursor = index + 1; cursor < controls.length; cursor += 1) {
        const rightElement = controls[cursor];
        if (leftElement.contains(rightElement) || rightElement.contains(leftElement)) {
          continue;
        }
        const right = rightElement.getBoundingClientRect();
        const area =
          Math.max(0, Math.min(left.right, right.right) - Math.max(left.left, right.left)) *
          Math.max(0, Math.min(left.bottom, right.bottom) - Math.max(left.top, right.top));
        if (area > 32) {
          overlaps.push({ area: Math.round(area), left: label(leftElement), right: label(rightElement) });
        }
      }
    }
    return {
      clippedControls,
      horizontalOverflowPx: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      overlaps: overlaps.slice(0, 8),
    };
  }, scopeSelector);
}
