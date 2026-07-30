import { expect, type Locator, type Page } from "@playwright/test";
import { defaultSubpages, viewLabel, viewSubpages } from "../../src/constants";
import type { ActiveView } from "../../src/types";

const WORKSPACE_ROUTE_READY_TIMEOUT_MS = 60_000;
const CONSOLE_SHELL_LOAD_ATTEMPTS = 3;

export async function activate(locator: Locator) {
  await expect(locator).toBeVisible({ timeout: 10_000 });
  await expect(locator).toBeEnabled({ timeout: 10_000 });
  await locator.evaluate((element) => (element as HTMLElement).click());
}

export async function waitForConsoleShell(page: Page, timeout = 10_000) {
  if (page.url() === "about:blank") {
    await reloadConsole(page);
  }
  let lastError: unknown;
  let startupRecoveryUsed = false;
  for (let attempt = 0; attempt < CONSOLE_SHELL_LOAD_ATTEMPTS; attempt += 1) {
    try {
      if (
        await recoverTransientStartupFailure(page, !startupRecoveryUsed)
      ) {
        startupRecoveryUsed = true;
        continue;
      }
      if (await isOperatorAccessVisible(page)) {
        if (await loginMockConsoleSession(page)) {
          return;
        }
        throw new Error(
          "Console shell is unavailable because the authenticated session was lost",
        );
      }
      await expect(page.locator(".shell")).toBeVisible({ timeout });
      return;
    } catch (error) {
      lastError = error;
      if (await isOperatorAccessVisible(page)) {
        if (await loginMockConsoleSession(page)) {
          return;
        }
        throw new Error(
          "Console shell is unavailable because the authenticated session was lost",
        );
      }
      if (
        await recoverTransientStartupFailure(page, !startupRecoveryUsed)
      ) {
        startupRecoveryUsed = true;
        continue;
      }
      if (
        attempt + 1 < CONSOLE_SHELL_LOAD_ATTEMPTS &&
        (await isBlankConsoleDocument(page))
      ) {
        await reloadConsole(page);
        continue;
      }
      throw error;
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

async function recoverTransientStartupFailure(
  page: Page,
  allowRecovery: boolean,
) {
  const recovery = page.locator("#boot-recovery[data-state=error]");
  if (!(await recovery.isVisible().catch(() => false))) {
    return false;
  }

  const detail =
    (await recovery.locator("#boot-error").textContent().catch(() => null))
      ?.trim() ?? "";
  const transient =
    /failed to fetch dynamically imported module/i.test(detail) ||
    /error loading dynamically imported module/i.test(detail) ||
    /importing a module script failed/i.test(detail) ||
    /unable to preload (?:css|module)/i.test(detail) ||
    /console startup timed out/i.test(detail) ||
    /^load failed$/i.test(detail);
  if (!transient || !allowRecovery) {
    throw new Error(
      `Console startup failed${detail ? `: ${detail}` : " without technical details"}`,
    );
  }

  await recovery.getByRole("link", { name: "Reload console" }).click();
  await page.waitForLoadState("domcontentloaded").catch(() => undefined);
  return true;
}

async function isOperatorAccessVisible(page: Page) {
  const signInVisible = await page
    .getByRole("heading", { exact: true, name: "Sign in" })
    .isVisible()
    .catch(() => false);
  if (signInVisible) return true;
  return page
    .getByRole("heading", { exact: true, name: "Create first operator" })
    .isVisible()
    .catch(() => false);
}

export async function openConsoleSubpage(
  page: Page,
  view: string,
  subpage: string,
  expectedHeaderTitle?: string,
) {
  const destination = releaseNavigationDestination(view, subpage);
  const viewDisplayLabel = destination.labelView;
  const viewRoute = destination.view;
  const subpageId = destination.subpage;
  const subpageLabel = destination.label;
  const headerTitle = expectedHeaderTitle ?? subpageLabel;
  const mobileValue = `${viewRoute}::${subpageId}`;

  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (attempt > 0) await page.waitForTimeout(750);
    await waitForConsoleShell(page);
    const headerCrumb = page
      .locator(".consoleHeader")
      .getByText(`vpsman / ${viewDisplayLabel} / ${headerTitle}`);
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
      const routeState = await waitForWorkspaceRouteReady(page);
      if (routeState === "ready") {
        return;
      }
      if (routeState === "error") {
        if (await recoverWorkspaceRouteError(page, attempt)) {
          continue;
        }
        throw new Error(
          await workspaceRouteErrorMessage(page, viewDisplayLabel, subpageLabel),
        );
      }
      continue;
    }
    if (await mobilePageSelector.isVisible()) {
      await mobilePageSelector.selectOption({ value: mobileValue });
      await expect(mobilePageSelector).toHaveValue(mobileValue);
      await expect(headerCrumb).toBeVisible({
        timeout: 10_000,
      });
      const routeState = await waitForWorkspaceRouteReady(page);
      if (routeState === "ready") {
        return;
      }
      if (routeState === "error") {
        if (await recoverWorkspaceRouteError(page, attempt)) {
          continue;
        }
        throw new Error(
          await workspaceRouteErrorMessage(page, viewDisplayLabel, subpageLabel),
        );
      }
      continue;
    }

    const nav = page.getByRole("navigation", {
      name: "Primary console navigation",
    });
    await expect(nav).toBeVisible({ timeout: 10_000 });
    await activate(
      nav.getByRole("button", { name: viewDisplayLabel, exact: true }).first(),
    );
    if (
      subpageId !== defaultSubpages[viewRoute as ActiveView] ||
      !(await routeReached())
    ) {
      const subpageGroup = nav.getByLabel(`${viewDisplayLabel} sections`);
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
    const routeState = await waitForWorkspaceRouteReady(page);
    if (routeState === "ready") {
      return;
    }
    if (routeState === "error") {
      if (await recoverWorkspaceRouteError(page, attempt)) {
        continue;
      }
      throw new Error(
        await workspaceRouteErrorMessage(page, viewDisplayLabel, subpageLabel),
      );
    }
  }
  throw new Error(`Workspace route did not become ready: ${viewDisplayLabel} / ${subpageLabel}`);
}

async function recoverWorkspaceRouteError(page: Page, attempt: number) {
  if (attempt >= 2) {
    return false;
  }
  const reloadButton = page
    .locator(".workspaceRouteError")
    .getByRole("button", { name: "Reload console" })
    .first();
  if (await reloadButton.isVisible().catch(() => false)) {
    await reloadButton.click();
  } else {
    await reloadConsole(page);
  }
  await page.waitForLoadState("domcontentloaded").catch(() => undefined);
  return true;
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
  const dialog = page.getByRole("dialog", { name: "Unlock privilege" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel(/super password/i).fill("local-super-password");
  await dialog
    .getByLabel(/(privilege salt|verifier salt hex)/i)
    .fill("00112233445566778899aabbccddeeff");
  await activate(
    dialog
      .getByLabel("Unlock with privilege material")
      .getByRole("button", { name: /Unlock( privilege)?/ }),
  );
  await expect(dialog).toBeHidden();
  await expect(
    topbar.getByRole("button", { name: "Lock privilege" }),
  ).toBeVisible();
}

export async function lockPrivilegeFromTop(page: Page) {
  const topbar = page.locator(".topbar");
  await activate(topbar.getByRole("button", { name: "Lock privilege" }));
  const prompt = page.getByLabel("Confirm privilege lock");
  await expect(prompt).toBeVisible();
  await activate(
    prompt.getByRole("button", { name: "Lock privilege", exact: true }),
  );
  await expect(
    topbar.getByRole("button", { name: "Open privilege unlock" }),
  ).toBeVisible();
}

async function loginMockConsoleSession(page: Page) {
  const mockInstalled = await page
    .evaluate(() => Boolean((window as typeof window & { __vpsmanTestRequests?: unknown }).__vpsmanTestRequests))
    .catch(() => false);
  if (!mockInstalled) {
    return false;
  }
  await page.getByLabel("Username").fill("console-admin");
  await page.getByLabel("Password").fill("local-super-password");
  await activate(page.getByRole("button", { name: "Sign in" }));
  await expect(page.locator(".shell")).toBeVisible({ timeout: 10_000 });
  return true;
}

async function isBlankConsoleDocument(page: Page) {
  return page
    .evaluate(() => {
      const hasConsoleSurface = Boolean(
        document.querySelector(".shell, .authOnlyShell"),
      );
      return !hasConsoleSurface && document.body.innerText.trim().length === 0;
    })
    .catch(() => false);
}

function releaseNavigationDestination(view: string, subpage: string) {
  const subpages = viewSubpages[view as ActiveView] ?? [];
  const match =
    subpages.find((item) => item.id === subpage) ??
    subpages.find((item) => item.label === subpage);
  return {
    label: match?.label ?? subpage,
    labelView: viewLabel(view as ActiveView),
    subpage: match?.id ?? subpage,
    view,
  };
}

async function reloadConsole(page: Page) {
  let lastError: unknown;
  if (page.url() === "about:blank") {
    for (let attempt = 0; attempt < 4; attempt += 1) {
      try {
        await page.goto("/", { waitUntil: "domcontentloaded" });
        return;
      } catch (error) {
        lastError = error;
        if (!isTransientNavigationError(error)) {
          break;
        }
        await page.waitForTimeout(500 * (attempt + 1));
      }
    }
    throw lastError instanceof Error ? lastError : new Error(String(lastError));
  }
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      await page.reload({ waitUntil: "domcontentloaded" });
      return;
    } catch (error) {
      lastError = error;
      if (!isTransientNavigationError(error)) {
        break;
      }
      await page.waitForTimeout(500 * (attempt + 1));
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

function isTransientNavigationError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return (
    message.includes("net::ERR_NETWORK_CHANGED") ||
    message.includes("net::ERR_ABORTED")
  );
}

async function waitForWorkspaceRouteReady(
  page: Page,
  timeout = WORKSPACE_ROUTE_READY_TIMEOUT_MS,
): Promise<"error" | "loading" | "ready"> {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
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
    if ((await loading.count()) === 0 || !(await loading.first().isVisible())) {
      await page.waitForTimeout(500);
      if (
        (await routeError.count()) > 0 &&
        (await routeError.first().isVisible())
      ) {
        return "error";
      }
      if ((await loading.count()) > 0 && (await loading.first().isVisible())) {
        continue;
      }
      return "ready";
    }
    await page.waitForTimeout(250);
  }
  return "loading";
}

async function workspaceRouteErrorMessage(
  page: Page,
  viewLabel: string,
  subpageLabel: string,
) {
  const routeErrorText =
    (await page
      .locator(".workspaceRouteError")
      .first()
      .innerText()
      .catch(() => "")) || "Workspace route recovery panel is visible";
  return `Workspace route failed to render: ${viewLabel} / ${subpageLabel}\n${routeErrorText}`;
}
