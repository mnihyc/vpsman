import { expect, test } from "@playwright/test";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_LIVE_API_SMOKE, "live API smoke is enabled by scripts/smoke-frontend-live-api.sh");

test("uses the real API proxy for fleet, topology planning, and audit visibility", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Fleet overview" })).toBeVisible();
  await expect(page.getByRole("button", { name: /edge-live-a/ })).toBeVisible();
  await expect(page.locator(".consoleHeader").getByText("2 connected / 2 total")).toBeVisible();

  await openConsoleSubpage(page, "Topology", "Tunnel plans");
  await expect(page.getByRole("heading", { name: "Tunnel plans" })).toBeVisible();
  const composer = page.locator(".scheduleComposer", { has: page.getByRole("heading", { name: "Create tunnel plan" }) });
  await composer.getByLabel("Name", { exact: true }).fill("live-gre-a-b");
  await composer.getByLabel("Interface", { exact: true }).fill("gre42");
  await composer.getByLabel("Kind").selectOption("gre");
  await composer.getByLabel("Bandwidth").selectOption("1000m");
  await composer.getByLabel("Left VPS").selectOption("live-agent-a");
  await composer.getByLabel("Right VPS").selectOption("live-agent-b");
  await composer.getByLabel("Left underlay", { exact: true }).fill("203.0.113.10");
  await composer.getByLabel("Right underlay", { exact: true }).fill("203.0.113.20");
  await composer.getByLabel("Address pool").fill("10.252.0.0/30");
  await composer.getByLabel("Latency ms").fill("18");
  await composer.getByLabel("Preference").fill("1.2");
  await composer.getByRole("button", { name: "Save plan" }).click();

  const planRow = page.getByRole("row", { name: /live-gre-a-b/ });
  await expect(planRow.getByRole("cell").filter({ hasText: "live-gre-a-b" })).toBeVisible();
  await expect(planRow.getByRole("cell", { exact: true, name: "GRE" })).toBeVisible();
  await expect(planRow.getByRole("cell", { exact: true, name: "Agent iproute2" })).toBeVisible();
  await expect(planRow.getByText("planned", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Audit" }).click();
  await expect(page.locator(".consoleHeader").getByRole("heading", { name: "Audit log" })).toBeVisible();
  await expect(page.getByText("network.tunnel_plan_created")).toBeVisible();
});
