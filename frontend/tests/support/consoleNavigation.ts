import { expect, type Locator, type Page } from "@playwright/test";
import { viewSubpages } from "../../src/constants";
import type { ActiveView } from "../../src/types";

export async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
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
  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await mobilePageSelector.selectOption({ value: mobileValue });
    await expect(mobilePageSelector).toHaveValue(mobileValue);
    await expect(
      page
        .locator(".consoleHeader")
        .getByText(`vpsman / ${viewLabel} / ${headerTitle}`),
    ).toBeVisible({
      timeout: 10_000,
    });
    return;
  }

  const nav = page.getByRole("navigation", {
    name: "Primary console navigation",
  });
  await activate(
    nav.getByRole("button", { name: viewLabel, exact: true }).first(),
  );
  const subpageGroup = nav.getByLabel(`${viewLabel} sections`);
  const subpageButton = subpageGroup.getByRole("button", {
    name: subpageLabel,
    exact: true,
  });
  if ((await subpageButton.count()) > 0) {
    await activate(subpageButton);
  }
  await expect(
    page
      .locator(".consoleHeader")
      .getByText(`vpsman / ${viewLabel} / ${headerTitle}`),
  ).toBeVisible({
    timeout: 10_000,
  });
}

export async function unlockPrivilegeFromTop(page: Page) {
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
