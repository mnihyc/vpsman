import { expect, test } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import { waitForConsoleShell } from "./support/consoleNavigation";

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
  await page.goto("/#/config/rules");
  await waitForConsoleShell(page, 15_000);
  await expect(
    page.getByLabel("VPS rule values data grid").getByText("4 of 4 rules"),
  ).toBeVisible();
});

test("VPS-rule search details appear only inside the scoped completion", async ({
  page,
}) => {
  const input = page.getByRole("combobox", { name: "Search fleet" });
  await input.fill("v");

  const suggestions = page.getByRole("listbox", {
    name: "Search fleet suggestions",
  });
  const category = suggestions.getByRole("option", {
    name: /VPS rules…/,
  });
  await expect(category).toHaveCount(1);
  await expect(category).toBeVisible();
  await expect(
    suggestions.getByText("Billing price", { exact: true }),
  ).toHaveCount(0);
  await expect(
    suggestions.getByText("Total quota", { exact: true }),
  ).toHaveCount(0);
  await expect(
    suggestions.getByText("Product name", { exact: true }),
  ).toHaveCount(0);

  await category.click();
  await expect(input).toHaveValue("vps.rules:");
  await expect(
    suggestions.getByText("Billing price", { exact: true }),
  ).toBeVisible();
  await expect(
    suggestions.getByText("Total quota", { exact: true }),
  ).toBeVisible();
  await expect(
    suggestions.getByText("Product name", { exact: true }),
  ).toBeVisible();
  await expect(input).toHaveAttribute("aria-invalid", "false");

  await input.evaluate((element) => (element as HTMLInputElement).blur());
  await expect(input).toHaveAttribute("aria-invalid", "true");
  await expect(
    page.getByText("Selector value is empty", { exact: true }),
  ).toBeVisible();
});
