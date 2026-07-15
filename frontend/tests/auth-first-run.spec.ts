import { expect, test } from "@playwright/test";

const removedSessionKeyLabel = ["Session", "vault", "key"].join(" ");

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
  await expect(page.getByLabel(removedSessionKeyLabel)).toHaveCount(0);
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
  await expect(page.getByLabel(removedSessionKeyLabel)).toHaveCount(0);
});

test("bootstrap status failure explains the cause and safe fallback", async ({
  page,
}) => {
  await page.route("**/api/v1/auth/bootstrap-status", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: {
        error: "bootstrap_status_store_unavailable",
        message: "Operator store could not be read",
        recovery: "Restore control-plane storage access, then refresh first-run state.",
        status: 503,
      },
      status: 503,
    });
  });

  await page.goto("/");

  const status = page.getByRole("alert");
  await expect(status).toContainText("Operator store could not be read");
  await expect(status).toContainText(
    "Restore control-plane storage access, then refresh first-run state",
  );
  await expect(status).toContainText(
    "Sign in only if this control plane is already initialized",
  );
});

test("successful bootstrap-status response with malformed JSON is not treated as state", async ({
  page,
}) => {
  await page.route("**/api/v1/auth/bootstrap-status", async (route) => {
    await route.fulfill({
      body: "{",
      contentType: "application/json",
      status: 200,
    });
  });

  await page.goto("/");

  const status = page.getByRole("alert");
  await expect(status).toContainText("returned unreadable JSON");
  await expect(status).toContainText("Current state cannot be inferred");
  await expect(status).toContainText("before repeating any mutation");
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
});

test("sign-in server failure is more than a status code", async ({ page }) => {
  await mockBootstrapStatus(page, false);
  await page.route("**/api/v1/auth/login", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      json: {
        error: "operator_store_unavailable",
        message: "Operator records could not be loaded",
        status: 500,
      },
      status: 500,
    });
  });

  await page.goto("/");
  await page.getByLabel("Username").fill("operator");
  await page.getByLabel("Password").fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Sign in" }).click();

  const status = page.getByRole("alert");
  await expect(status).toContainText("Operator records could not be loaded");
  await expect(status).toContainText("no success is assumed");
  await expect(status).toContainText("inspect API logs and retry");
});
