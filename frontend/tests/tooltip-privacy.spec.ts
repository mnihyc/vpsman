import { expect, test } from "@playwright/test";
import type { AuditLogRecord, BackupPolicyRecord } from "../src/types";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";


test("does not promote hidden data-grid search metadata into titles", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the shared data-grid tooltip contract is covered on desktop",
  );
  const hiddenSearchSentinel = "grid-hidden-search-secret-8c31";
  const audit: AuditLogRecord = {
    action: "job.dispatch_requested",
    actor_id: "99999999-aaaa-4bbb-8ccc-000000000001",
    command_hash: null,
    created_at: "2026-08-08T08:31:00Z",
    id: "audit-tooltip-privacy-0001",
    metadata: {
      command_type: "shell_argv",
      component: "job-submission-controller",
      hidden_search_only: hiddenSearchSentinel,
      origin_kind: "operator_request",
      result: "requested",
      target_count: 1,
    },
    target: "api:/api/v1/jobs",
  };
  await installConsoleApiMock(page, { auditLogsOverride: [audit] });
  await page.goto("/");
  await openConsoleSubpage(page, "Audit", "Events");

  const grid = page.getByLabel("Audit records data grid");
  await expect(grid.locator(".gridBody [role=row]")).toHaveCount(1);
  await expect
    .poll(async () => grid.locator("[title]").count())
    .toBeGreaterThan(0);
  const titles = await grid
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? ""),
    );
  expect(titles.join("\n")).not.toContain(hiddenSearchSentinel);
});
test("keeps editable Suite Config values out of input titles", async ({
  page,
}) => {
  const credentialUrl =
    "https://tooltip-user:credential-bearing-url-sentinel@example.invalid/control";
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Suite config");

  const sections = page.getByLabel("Suite config sections");
  await sections.getByRole("button", { name: /Gateway/ }).click();
  const endpointField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("API URL"),
  });
  const endpointInput = endpointField.getByLabel("API URL");
  await endpointInput.fill(credentialUrl);

  await expect(endpointInput).toHaveValue(credentialUrl);
  await expect(endpointInput).not.toHaveAttribute(
    "title",
    /credential-bearing-url-sentinel/,
  );
  await expect(
    endpointField.getByRole("button", { name: "Reset current" }),
  ).toHaveAttribute("title", "Reset API URL to the loaded value.");
  await expect(
    endpointField.getByRole("button", { name: "Use default" }),
  ).toHaveAttribute(
    "title",
    "Use the inherited default for API URL; removes the explicit value.",
  );

  const endpointTitles = await endpointField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(endpointTitles).not.toContain("credential-bearing-url-sentinel");
  expect(endpointTitles).toContain("http://api:8080");

  const ordinaryField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Gateway ID"),
  });
  const ordinaryInput = ordinaryField.getByLabel("Gateway ID");
  await expect(ordinaryInput).not.toHaveAttribute("title", /compose-gateway/);

  const websocketCredentialUrl =
    "wss://tooltip-user:websocket-url-sentinel@example.invalid/events";
  await ordinaryInput.fill(websocketCredentialUrl);
  await expect(ordinaryInput).toHaveValue(websocketCredentialUrl);
  await expect(ordinaryInput).not.toHaveAttribute(
    "title",
    /websocket-url-sentinel/,
  );
  const ordinaryTitles = await ordinaryField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(ordinaryTitles).not.toContain("websocket-url-sentinel");
  expect(ordinaryTitles).toContain("compose-gateway");

  await sections.getByRole("button", { name: /API/ }).click();
  const gatewayControlField = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Gateway control URL"),
  });
  const gatewayControlInput = gatewayControlField.getByLabel(
    "Gateway control URL",
  );
  await expect(gatewayControlInput).toHaveValue(
    "unix:/var/lib/vpsman/gateway-control.sock",
  );
  await expect(gatewayControlInput).not.toHaveAttribute(
    "title",
    /unix:\/var\/lib\/vpsman\/gateway-control\.sock/,
  );
  const gatewayControlTitles = await gatewayControlField
    .locator("[title]")
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute("title") ?? "").join("\n"),
    );
  expect(gatewayControlTitles).toContain(
    "unix:/var/lib/vpsman/gateway-control.sock",
  );
});

test("reveals a retained external error only when its visible value is shortened", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the retained-error tooltip boundary is covered in the desktop data grid",
  );
  const externalErrorSentinel =
    "external-backup-error-sentinel-8dd9: upstream returned opaque detail";
  const policy: BackupPolicyRecord = {
    cadence_error: null,
    catch_up_limit: 1,
    catch_up_policy: "skip_missed",
    created_at: "2026-08-08T08:00:00Z",
    cron_expr: "0 2 * * *",
    enabled: true,
    failure_count: 1,
    follow_symlinks: false,
    include_config: true,
    keep_last: 7,
    last_error: externalErrorSentinel,
    last_run_at: "2026-08-08T08:15:00Z",
    max_failures: 3,
    missing_path_policy: "skip",
    name: "retained failure evidence",
    next_run_at: "2026-08-09T02:00:00Z",
    next_runs: ["2026-08-09T02:00:00Z"],
    paths: ["/srv/backup"],
    retention_days: 30,
    retry_delay_secs: 300,
    rotation_generation: null,
    schedule_id: "tooltip-privacy-backup-policy",
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
    timezone: "UTC",
    updated_at: "2026-08-08T08:15:00Z",
  };

  await installConsoleApiMock(page, { backupPoliciesOverride: [policy] });
  await page.goto("/");
  await openConsoleSubpage(page, "Backups", "Policies");

  const grid = page.getByLabel("Backup policy records data grid");
  const visibleError = grid.getByText(externalErrorSentinel, { exact: true });
  await expect(visibleError).toBeVisible();
  const result = visibleError.locator("..");
  await expect(result).not.toHaveAttribute("data-tooltip-sensitive", "true");
  await expect(result).not.toHaveAttribute("data-value-tooltip-skip", "true");
  await expect
    .poll(() =>
      visibleError.evaluate(
        (element) => element.scrollWidth > element.clientWidth + 1,
      ),
    )
    .toBe(true);
  await expect(visibleError).toHaveAttribute("title", externalErrorSentinel);
  await expect
    .poll(async () =>
      page
        .locator("[title]")
        .evaluateAll(
          (elements, sentinel) =>
            elements.some((element) =>
              (element.getAttribute("title") ?? "").includes(sentinel),
            ),
          externalErrorSentinel,
        ),
    )
    .toBe(true);
});
