import { expect, test, type Locator, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

test.skip(!process.env.VPSMAN_VISUAL_AUDIT, "manual Access operators and Audit sessions screenshots only");
test.setTimeout(90_000);

test.beforeEach(async ({ page }) => {
  await installConsoleApiMock(page);
});

test("captures Access operators and Audit sessions interaction loop", async ({ page }, testInfo) => {
  const outputDir = testInfo.outputPath("access-operators-audit-sessions-visual-audit");
  mkdirSync(outputDir, { recursive: true });
  const manifest: Array<Record<string, unknown>> = [];

  await page.goto("/");
  await unlockPrivilegeFromTop(page);
  await openConsoleSubpage(page, "Access", "Operators");
  await expect(page.getByRole("heading", { name: "Operators", exact: true })).toBeVisible();
  await expect(page.getByText("2 operator records")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-initial");

  await activate(page.getByRole("button", { name: "New" }).first());
  const userEditor = page.getByLabel("Operator user editor");
  await expect(userEditor).toBeVisible();
  await userEditor.getByLabel("Operator username").fill("release-admin");
  await userEditor.getByLabel("Operator password").fill("release-admin-password-123");
  await userEditor.getByLabel("Operator role").selectOption("admin");
  await userEditor.getByLabel("Session refresh TTL days").fill("30");
  await activate(userEditor.getByRole("button", { name: "Create", exact: true }));
  await expect(page.getByLabel("Confirm admin user action")).toBeVisible();
  await expect(page.getByText(/targets or grants admin privileges/)).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-create-admin-confirm");
  await activate(page.getByRole("button", { name: "Create user" }));
  await expect(page.getByText("3 operator records")).toBeVisible();
  await expect(page.getByText("release-admin")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-after-create");

  await selectGridRow(page, "Users", "99999999-aaaa-4bbb-8ccc-000000000001");
  await runGridAction(page, "Users", "Edit selected");
  await expect(page.getByRole("heading", { name: "Edit user" })).toBeVisible();
  await activate(userEditor.getByRole("button", { name: "Disable", exact: true }));
  await expect(page.getByLabel("Confirm admin user action")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-admin-disable-confirm");
  await activate(page.getByRole("button", { name: "Cancel" }));

  await unselectGridRow(page, "Users", "99999999-aaaa-4bbb-8ccc-000000000001");
  await selectGridRow(page, "Users", "99999999-aaaa-4bbb-8ccc-000000000002");
  await runGridAction(page, "Users", "Edit selected");
  await expect(userEditor.getByLabel("Operator username")).toHaveValue("noc-operator");
  await userEditor.getByLabel("Operator password").fill("replacement-password-123");
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-edit-operator");
  await activate(userEditor.getByRole("button", { name: "Reset password", exact: true }));
  await expect(page.getByLabel("Confirm user action")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-password-reset-confirm");
  await activate(page.getByRole("button", { name: "Cancel" }));
  await activate(userEditor.getByRole("button", { name: "Clear TOTP", exact: true }));
  await expect(page.getByLabel("Confirm user action")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "users-clear-totp-confirm");
  await activate(page.getByRole("button", { name: "Cancel" }));

  await openConsoleSubpage(page, "Audit", "Sessions");
  await expect(page.getByRole("heading", { name: "Session evidence", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Authentication history", exact: true })).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "sessions-initial");

  await selectGridRow(page, "Sessions", "88888888-aaaa-4bbb-8ccc-000000000002");
  await runGridAction(page, "Sessions", "Revoke selected");
  await expect(page.getByLabel("Confirm admin session revoke")).toBeVisible();
  await capture(page, page.locator("main.content"), outputDir, manifest, "sessions-admin-revoke-confirm");

  writeFileSync(
    join(outputDir, `manifest-${testInfo.project.name}.json`),
    `${JSON.stringify({ screenshots: manifest }, null, 2)}\n`,
  );
});

async function capture(
  page: Page,
  locator: Locator,
  outputDir: string,
  manifest: Array<Record<string, unknown>>,
  name: string,
) {
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(150);
  const layout = await page.evaluate(() => {
    const viewportWidth = document.documentElement.clientWidth;
    const viewportHeight = document.documentElement.clientHeight;
    const hasHorizontalScroller = (element: Element) => {
      let current: Element | null = element.parentElement;
      while (current) {
        const style = window.getComputedStyle(current);
        const allowsHorizontalScroll =
          style.overflowX === "auto" ||
          style.overflowX === "scroll" ||
          style.overflow === "auto" ||
          style.overflow === "scroll";
        if (allowsHorizontalScroll && current.scrollWidth > current.clientWidth + 1) {
          return true;
        }
        current = current.parentElement;
      }
      return false;
    };
    const overflowCandidates = Array.from(document.querySelectorAll("*"))
      .map((element) => {
        const rect = element.getBoundingClientRect();
        return {
          className: element instanceof HTMLElement ? element.className : "",
          clippedByScroller: hasHorizontalScroller(element),
          right: Math.round(rect.right),
          tagName: element.tagName.toLowerCase(),
          text: (element.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 100),
          width: Math.round(rect.width),
        };
      })
      .filter((entry) => entry.right > viewportWidth + 1)
      .slice(0, 10);
    return {
      horizontalOverflowPx: Math.max(0, document.documentElement.scrollWidth - viewportWidth),
      overflowCandidates,
      uncontainedOverflowCandidates: overflowCandidates.filter((entry) => !entry.clippedByScroller),
      viewportHeight,
      viewportWidth,
    };
  });
  expect(
    layout.uncontainedOverflowCandidates,
    `${name} uncontained horizontal overflow candidates: ${JSON.stringify(layout.overflowCandidates)}`,
  ).toHaveLength(0);
  const path = join(outputDir, `${name}-${page.viewportSize()?.width ?? "viewport"}.png`);
  await locator.screenshot({ path });
  manifest.push({ name, path, ...layout });
}

async function selectGridRow(page: Page, title: string, rowId: string) {
  const grid = page.getByLabel(`${title} data grid`);
  await grid.getByLabel(`Select ${title} row ${rowId}`).check();
}

async function unselectGridRow(page: Page, title: string, rowId: string) {
  const grid = page.getByLabel(`${title} data grid`);
  await grid.getByLabel(`Select ${title} row ${rowId}`).uncheck();
}

async function runGridAction(page: Page, title: string, action: string) {
  const grid = page.getByLabel(`${title} data grid`);
  await grid.getByRole("button", { name: "Action" }).click();
  await page.getByRole("menuitem", { name: action }).click();
}
