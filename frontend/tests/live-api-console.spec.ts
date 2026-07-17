import { expect, test, type Locator, type Page } from "@playwright/test";
import { openConsoleSubpage } from "./support/consoleNavigation";

test.skip(
  !process.env.VPSMAN_LIVE_API_SMOKE,
  "live API smoke is enabled by scripts/smoke-frontend-live-api.sh",
);

async function chooseVpsBySearch(
  root: Locator,
  label: string,
  query: string,
  optionName: RegExp,
) {
  await root.getByRole("combobox", { name: label }).fill(query);
  const option = root.page().locator(".vpsComboboxMenu").getByRole("option", {
    name: optionName,
  });
  await expect(option).toBeVisible();
  await option.click();
}

async function ensureAuthenticated(page: Page) {
  const shell = page.locator(".shell");
  const signIn = page.getByRole("heading", {
    exact: true,
    name: "Sign in",
  });

  await expect(shell.or(signIn).first()).toBeVisible({ timeout: 20_000 });
  if (await shell.isVisible()) return;

  await page
    .getByLabel("Username")
    .fill(process.env.VPSMAN_LIVE_API_USERNAME ?? "frontend-live-admin");
  await page
    .getByLabel("Password")
    .fill(process.env.VPSMAN_LIVE_API_PASSWORD ?? "frontend-live-password");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(shell).toBeVisible({ timeout: 30_000 });
}

test("uses the real API proxy for fleet, topology planning, and audit visibility", async ({
  page,
}) => {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await ensureAuthenticated(page);

  await openConsoleSubpage(page, "Fleet", "Instances");
  await expect(
    page.getByRole("heading", { name: "Fleet instances" }),
  ).toBeVisible();
  await expect(page.getByRole("row", { name: /edge-live-a/ })).toBeVisible();
  await expect(
    page.locator(".consoleHeader").getByText("2 live / 0 no contact / 2 total"),
  ).toBeVisible();

  await openConsoleSubpage(page, "Network", "Tunnel plans");
  await expect(
    page.locator(".consoleHeader").getByRole("heading", {
      level: 1,
      name: "Tunnel plans",
    }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Create plan" }).click();
  const composer = page.locator(".tunnelPlanComposer", {
    has: page.getByRole("heading", { name: "Create tunnel plan" }),
  });
  await expect(composer).toBeVisible();
  await composer.getByLabel("Tunnel plan name", { exact: true }).fill("live-gre-a-b");
  await composer.getByLabel("Tunnel interface", { exact: true }).fill("gre42");
  await composer.getByLabel("Tunnel kind", { exact: true }).selectOption("gre");
  await composer.getByLabel("Tunnel bandwidth", { exact: true }).fill("1000");
  await chooseVpsBySearch(
    composer,
    "Left tunnel VPS",
    "live-agent-a",
    /live-agent-a|edge-live-a/,
  );
  await chooseVpsBySearch(
    composer,
    "Right tunnel VPS",
    "live-agent-b",
    /live-agent-b|edge-live-b/,
  );
  await composer
    .getByLabel("Left remote underlay destination")
    .fill("203.0.113.20");
  await composer
    .getByLabel("Right remote underlay destination")
    .fill("203.0.113.10");
  await composer.getByLabel("IPv4 allocation pool").fill("10.252.0.0/30");
  await composer.getByRole("button", { name: "Allocate" }).click();
  await expect(composer.getByLabel("Left tunnel IPv4")).toHaveValue(
    "10.252.0.0",
  );
  await expect(composer.getByLabel("Right tunnel IPv4")).toHaveValue(
    "10.252.0.1",
  );
  await expect(composer.getByLabel("IPv4 tunnel prefix")).toHaveValue("31");
  await composer.getByRole("button", { name: "Review plan" }).click();
  const savePrompt = page.locator(".confirmationPrompt", {
    hasText: "Confirm tunnel plan creation",
  });
  await expect(savePrompt).toBeVisible();
  await savePrompt.getByRole("button", { name: "Save plan", exact: true }).click();
  await expect(savePrompt).toBeHidden();
  await expect(composer).toBeHidden();

  const planRow = page.getByRole("row", { name: /live-gre-a-b/ });
  await expect(planRow).toBeVisible();
  await expect(
    planRow.getByText("live-gre-a-b", { exact: true }),
  ).toBeVisible();
  await expect(planRow).toContainText(/GRE.*gre42/);
  await expect(planRow).toContainText("Agent iproute2");
  await expect(planRow).toContainText("Disabled");
  await expect(planRow).toContainText("Off");
  await expect(planRow).toContainText("Tunnel only");

  await page.getByRole("button", { name: "Audit" }).click();
  await expect(
    page
      .locator(".consoleHeader")
      .getByRole("heading", { name: "Audit events" }),
  ).toBeVisible();
  await expect(page.getByText("Network Tunnel Plan Created")).toBeVisible();
});
