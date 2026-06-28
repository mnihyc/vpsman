import { expect, type Locator, type Page } from "@playwright/test";
import { defaultSubpages, viewSubpages } from "../../src/constants";
import type { ActiveView } from "../../src/types";

export async function activate(locator: Locator) {
  await expect(locator).toBeVisible({ timeout: 10_000 });
  await expect(locator).toBeEnabled({ timeout: 10_000 });
  await locator.evaluate((element) => (element as HTMLElement).click());
}

export async function waitForConsoleShell(page: Page, timeout = 10_000) {
  let lastError: unknown;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (attempt > 0) {
      await reloadConsole(page);
    }
    try {
      await expect(page.locator(".shell")).toBeVisible({ timeout });
      const routeError = page.locator(".workspaceRouteError");
      if ((await routeError.count()) > 0 && (await routeError.first().isVisible())) {
        throw new Error("Workspace route recovery panel is visible");
      }
      return;
    } catch (error) {
      lastError = error;
    }
  }
  if (lastError instanceof Error) {
    throw lastError;
  }
  throw new Error(String(lastError));
}

export async function openConsoleSubpage(
  page: Page,
  view: string,
  subpage: string,
  expectedHeaderTitle?: string,
) {
  const destination = releaseNavigationDestination(view, subpage);
  const viewLabel = destination.view;
  const subpageId = destination.subpage;
  const subpageLabel = destination.label;
  const headerTitle = expectedHeaderTitle ?? subpageLabel;
  const mobileValue = `${viewLabel}::${subpageId}`;

  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (attempt > 0) {
      await reloadConsole(page);
    }
    await waitForConsoleShell(page);
    const headerCrumb = page
      .locator(".consoleHeader")
      .getByText(`vpsman / ${viewLabel} / ${headerTitle}`);
    const routeReached = async (timeout = 500) =>
      expect(headerCrumb)
        .toBeVisible({ timeout })
        .then(() => true)
        .catch(() => false);
    const mobilePageMenu = page.locator(".mobilePageMenu");
    const mobilePageSelector = page.locator(".mobilePageSelector");
    if (await mobilePageMenu.isVisible()) {
      if (!(await mobilePageSelector.isVisible())) {
        await mobilePageMenu.getByLabel("Open mobile page navigation").click();
      }
      await mobilePageSelector.selectOption({ value: mobileValue });
      await expect(mobilePageSelector).toHaveValue(mobileValue);
      await expect(headerCrumb).toBeVisible({
        timeout: 10_000,
      });
      if (await waitForWorkspaceRouteReady(page)) {
        return;
      }
      continue;
    }
    if (await mobilePageSelector.isVisible()) {
      await mobilePageSelector.selectOption({ value: mobileValue });
      await expect(mobilePageSelector).toHaveValue(mobileValue);
      await expect(headerCrumb).toBeVisible({
        timeout: 10_000,
      });
      if (await waitForWorkspaceRouteReady(page)) {
        return;
      }
      continue;
    }

    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await expect(nav).toBeVisible({ timeout: 10_000 });
    await activate(
      nav.getByRole("button", { name: viewLabel, exact: true }).first(),
    );
    if (
      subpageId !== defaultSubpages[viewLabel as ActiveView] ||
      !(await routeReached())
    ) {
      const subpageGroup = nav.getByLabel(`${viewLabel} sections`);
      const subpageButton = subpageGroup.getByRole("button", {
        name: subpageLabel,
        exact: true,
      });
      if ((await subpageButton.count()) > 0) {
        try {
          await activate(subpageButton);
        } catch (error) {
          if (!(await routeReached())) {
            throw error;
          }
        }
      }
      await expect(headerCrumb).toBeVisible({
        timeout: 10_000,
      });
    }
    if (await waitForWorkspaceRouteReady(page)) {
      return;
    }
  }
  throw new Error(`Workspace route did not become ready: ${viewLabel} / ${subpageLabel}`);
}

export async function unlockPrivilegeFromTop(page: Page) {
  await waitForConsoleShell(page);
  const topbar = page.locator(".topbar");
  if (
    (await topbar.getByRole("button", { name: "Lock privilege" }).count()) > 0
  ) {
    return;
  }
  await activate(topbar.getByRole("button", { name: "Open privilege unlock" }));
  await expect(
    page.getByRole("heading", { level: 1, name: "Privilege vault" }),
  ).toBeVisible();
  await page.getByLabel(/privilege secret/i).fill("local-super-password");
  await page
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    page
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: /Unlock( privilege)?/ }),
  );
  await expect(
    topbar.getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
}

function releaseNavigationDestination(view: string, subpage: string) {
  const subpages = viewSubpages[view as ActiveView] ?? [];
  const match =
    subpages.find((item) => item.id === subpage) ??
    subpages.find((item) => item.label === subpage);
  return {
    label: match?.label ?? subpage,
    subpage: match?.id ?? subpage,
    view,
  };
}

async function reloadConsole(page: Page) {
  if (page.url() === "about:blank") {
    await page.goto("/");
    return;
  }
  await page.reload({ waitUntil: "domcontentloaded" });
}

async function waitForWorkspaceRouteReady(page: Page): Promise<boolean> {
  return expect
    .poll(
      async () => {
        const routeError = page.locator(".workspaceRouteError");
        if (
          (await routeError.count()) > 0 &&
          (await routeError.first().isVisible())
        ) {
          return "error";
        }
        const loading = page.locator(".emptyState.compactEmpty", {
          hasText: /Loading .* workspace/i,
        });
        if (
          (await loading.count()) > 0 &&
          (await loading.first().isVisible())
        ) {
          return "loading";
        }
        return "ready";
      },
      { timeout: 10_000 },
    )
    .toBe("ready")
    .then(() => true)
    .catch(() => false);
}
