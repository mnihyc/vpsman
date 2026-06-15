import { expect, test, type Locator } from "@playwright/test";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_LIVE_API_SMOKE, "live API smoke is enabled by scripts/smoke-frontend-live-api.sh");

async function chooseVpsBySearch(
  root: Locator,
  label: string,
  query: string,
  optionName: RegExp,
) {
  await root.getByRole("combobox", { name: label }).fill(query);
  const option = root.getByRole("option", { name: optionName });
  await expect(option).toBeVisible();
  await option.click();
}

test("uses the real API proxy for fleet, topology planning, and audit visibility", async ({ page }) => {
  await page.goto("/");
  if (await page.getByRole("heading", { name: "Operator access" }).isVisible()) {
    await page.getByLabel("Username").fill(process.env.VPSMAN_LIVE_API_USERNAME ?? "frontend-live-admin");
    await page.getByLabel("Password").fill(process.env.VPSMAN_LIVE_API_PASSWORD ?? "frontend-live-password");
    await page.getByRole("button", { name: "Submit login" }).click();
  }

  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(page.getByRole("heading", { name: "Fleet overview" })).toBeVisible();
  await expect(page.getByRole("row", { name: /edge-live-a/ })).toBeVisible();
  await expect(page.locator(".consoleHeader").getByText("2 online / 2 total")).toBeVisible();

  await openConsoleSubpage(page, "Topology", "Tunnel plans");
  await expect(page.getByRole("heading", { name: "Topology management" })).toBeVisible();
  await expect(page.getByText("Tunnel plans").first()).toBeVisible();
  const composer = page.locator(".scheduleComposer", { has: page.getByRole("heading", { name: "Create tunnel plan" }) });
  await composer.getByLabel("Name", { exact: true }).fill("live-gre-a-b");
  await composer.getByLabel("Interface", { exact: true }).fill("gre42");
  await composer.getByLabel("Kind").selectOption("gre");
  await composer.getByLabel("Bandwidth").selectOption("1000m");
  await chooseVpsBySearch(composer, "Left VPS", "live-agent-a", /live-agent-a|edge-live-a/);
  await chooseVpsBySearch(composer, "Right VPS", "live-agent-b", /live-agent-b|edge-live-b/);
  await composer.getByLabel("Left underlay", { exact: true }).fill("203.0.113.10");
  await composer.getByLabel("Right underlay", { exact: true }).fill("203.0.113.20");
  await composer.getByLabel("IPv4 allocation pool").fill("10.252.0.0/30");
  await composer.getByRole("button", { name: "Generate endpoints" }).click();
  await expect(composer.getByLabel("Left IPv4", { exact: true })).toHaveValue("10.252.0.0");
  await expect(composer.getByLabel("Right IPv4", { exact: true })).toHaveValue("10.252.0.1");
  await composer.getByLabel("Latency ms").fill("18");
  await composer.getByLabel("Preference").fill("1.2");
  await composer.getByRole("button", { name: "Save plan" }).click();

  const planRow = page.getByRole("row", { name: /live-gre-a-b/ });
  await expect(planRow).toBeVisible();
  await expect(planRow.getByText("live-gre-a-b", { exact: true })).toBeVisible();
  await expect(planRow.getByText("GRE", { exact: true })).toBeVisible();
  await expect(planRow.getByText("Agent iproute2", { exact: true })).toBeVisible();
  await expect(planRow.getByText("planned", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Audit" }).click();
  await expect(page.locator(".consoleHeader").getByRole("heading", { name: "Audit log" })).toBeVisible();
  await expect(page.getByText("network.tunnel_plan_created")).toBeVisible();
});
