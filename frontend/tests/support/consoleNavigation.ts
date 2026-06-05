import type { Locator, Page } from "@playwright/test";

export async function activate(locator: Locator) {
  await locator.evaluate((element) => (element as HTMLElement).click());
}

export async function openConsoleSubpage(page: Page, view: string, subpage: string) {
  const nav = page.getByRole("navigation", { name: "Primary console navigation" });
  await activate(nav.getByRole("button", { name: view, exact: true }));
  const subpageButton = nav.getByRole("button", { name: subpage, exact: true });
  if ((await subpageButton.count()) > 0) {
    await activate(subpageButton);
  }
}
