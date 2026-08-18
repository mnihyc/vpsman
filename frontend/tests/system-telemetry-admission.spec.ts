import { expect, test } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { openConsoleSubpage } from "./support/consoleNavigation";

test("system capacity exposes gateway telemetry admission and its restart-scoped limit", async ({
  page,
}, testInfo) => {
  test.skip(
    testInfo.project.name.includes("mobile"),
    "the telemetry admission projection is identical in the mobile layout",
  );
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "System", "Capacity");

  const posture = page.getByLabel("System capacity posture overview");
  await posture.getByRole("tab", { name: /Gateway/ }).click();

  const gatewaySection = page.locator("section.dashboardSection", {
    has: page.getByRole("heading", {
      name: "Gateway capacity",
      exact: true,
    }),
  });
  const currentMetrics = gatewaySection.locator(".systemMetricTable");
  for (const [label, value] of [
    ["Telemetry admission limit", "8"],
    ["Telemetry posts active", "5"],
    ["Telemetry posts waiting", "2"],
  ]) {
    await expect(
      currentMetrics.locator(".dashboardClientRow", { hasText: label }),
    ).toContainText(value);
  }

  const series = gatewaySection.getByLabel(
    "Gateway capacity system metrics series",
  );
  await expect(series).toContainText("Gateway telemetry admission limit");
  await expect(series).toContainText("Gateway telemetry posts active");
  await expect(series).toContainText("Gateway telemetry posts waiting");

  const configLink = posture.getByRole("button", {
    name: /Gateway telemetry in-flight/,
  });
  await expect(configLink).toContainText(
    "capacity.gateway_telemetry_in_flight",
  );
  await configLink.click();

  const sections = page.getByLabel("Suite config sections");
  await sections.getByRole("button", { name: /Capacity/ }).click();
  const field = page.locator(".systemConfigFieldRow", {
    has: page.getByLabel("Gateway telemetry in-flight"),
  });
  await expect(page.getByLabel("Gateway telemetry in-flight")).toHaveValue("8");
  await expect(field).toContainText("integer, 1 to 512");
  await expect(field).toContainText("Restart required");
});
