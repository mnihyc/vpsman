import { expect, type Locator, type Page } from "@playwright/test";

export async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

export async function openConsoleSubpage(page: Page, view: string, subpage: string) {
  const mobilePageSelector = page.getByRole("combobox", { name: "Console page" });
  if (await mobilePageSelector.isVisible()) {
    await mobilePageSelector.selectOption({ label: `${view} / ${subpage}` });
    return;
  }

  const nav = page.getByRole("navigation", { name: "Primary console navigation" });
  await activate(nav.locator("button.navItem").filter({ hasText: view }));
  const subpageGroup = nav.getByLabel(`${view} sections`);
  const subpageButton = subpageGroup.getByRole("button", { name: subpage, exact: true });
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
  await expect(page.getByRole("heading", { name: "Privilege unlock" })).toBeVisible();
  await page.getByLabel("Super password").fill("local-super-password");
  await page.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Unlock privilege" }));
  await expect(topbar.getByRole("button", { name: "Lock privilege" })).toBeVisible();
}
