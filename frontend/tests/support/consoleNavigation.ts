import { expect, type Locator, type Page } from "@playwright/test";

export async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

export async function openConsoleSubpage(page: Page, view: string, subpage: string) {
  const destination = releaseNavigationDestination(view, subpage);
  const viewLabel = destination.view;
  const subpageLabel = destination.subpage;
  const mobilePageSelector = page.locator(".mobilePageSelector");
  if (await mobilePageSelector.isVisible()) {
    await mobilePageSelector.selectOption({ label: `${viewLabel} / ${subpageLabel}` });
    return;
  }

  const nav = page.getByRole("navigation", { name: "Primary console navigation" });
  await activate(nav.getByRole("button", { name: viewLabel, exact: true }).first());
  const subpageGroup = nav.getByLabel(`${viewLabel} sections`);
  const subpageButton = subpageGroup.getByRole("button", { name: subpageLabel, exact: true });
  if ((await subpageButton.count()) > 0) {
    await activate(subpageButton);
  }
}

export async function unlockPrivilegeFromTop(page: Page) {
  const topbar = page.locator(".topbar");
  if ((await topbar.getByRole("button", { name: "Lock privilege" }).count()) > 0) {
    return;
  }
  await activate(topbar.getByRole("button", { name: "Open privilege unlock" }));
  await expect(page.getByRole("heading", { level: 1, name: "Privilege vault" })).toBeVisible();
  await page.getByLabel(/privilege secret/i).fill("local-super-password");
  await page.getByLabel(/(privilege salt|verifier salt hex)/i).fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Unlock privilege" }));
  await expect(topbar.getByRole("button", { name: "Lock privilege" })).toBeVisible();
}

function releaseNavigationDestination(view: string, subpage: string) {
  return { view, subpage };
}
