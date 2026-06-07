import { expect, type Locator, type Page } from "@playwright/test";

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

export async function unlockProofFromTop(page: Page) {
  const topbar = page.locator(".topbar");
  if ((await topbar.getByRole("button", { name: "Lock proof" }).count()) > 0) {
    return;
  }
  await activate(topbar.getByRole("button", { name: "Open proof unlock" }));
  await expect(page.getByRole("heading", { name: "Proof unlock" })).toBeVisible();
  await page.getByLabel("Super password").fill("local-super-password");
  await page.getByLabel("Super salt hex").fill("00112233445566778899aabbccddeeff");
  await activate(page.getByRole("button", { name: "Use proof" }));
  await expect(topbar.getByRole("button", { name: "Lock proof" })).toBeVisible();
}
