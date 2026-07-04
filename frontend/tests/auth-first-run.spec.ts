import { expect, test } from "@playwright/test";

async function mockBootstrapStatus(
  page: import("@playwright/test").Page,
  bootstrapRequired: boolean,
) {
  await page.route("**/api/v1/auth/bootstrap-status", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: {
        bootstrap_required: bootstrapRequired,
      },
    });
  });
}

test("first-run auth screen creates the first operator without a bootstrap mode tab", async ({
  page,
}) => {
  await mockBootstrapStatus(page, true);

  await page.goto("/");

  await expect(
    page.getByRole("heading", { name: "Create first operator" }),
  ).toBeVisible();
  await expect(
    page.getByText("This control plane has no operators yet."),
  ).toBeVisible();
  await expect(page.getByLabel("Authentication mode")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Create first operator" }))
    .toBeVisible();
  await expect(page.getByLabel("TOTP code")).toHaveCount(0);
  await expect(page.getByLabel("Session vault key")).toHaveAttribute(
    "placeholder",
    "Optional local key",
  );
});

test("initialized auth screen signs in without exposing bootstrap as a peer mode", async ({
  page,
}) => {
  await mockBootstrapStatus(page, false);

  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await expect(
    page.getByText("Use a registered operator account."),
  ).toBeVisible();
  await expect(page.getByLabel("Authentication mode")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
  await expect(page.getByLabel("TOTP code")).toBeVisible();
  await expect(page.getByLabel("Session vault key")).toHaveAttribute(
    "placeholder",
    "Optional local key",
  );
});
