import { expect, test, type Page } from "@playwright/test";
import {
  hostStorageInventory,
  installConsoleApiMock,
} from "./support/consoleLayoutFixtures";
import { activate, openConsoleSubpage } from "./support/consoleNavigation";

test("keeps read-only storage routable and refreshes an explicit mount scope", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Storage");

  const panel = page.locator(".hostStoragePanel");
  await expect(panel.getByText("Choose a VPS")).toBeVisible();
  await panel.getByLabel("Storage inventory VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();

  await expect(page).toHaveURL(/storage_client=agent-sfo-01/);
  const summary = panel.getByLabel("Storage inventory summary");
  await expect(summary.getByText("lsblk JSON", { exact: true })).toBeVisible();
  await expect(summary.getByText("600 GiB", { exact: true })).toBeVisible();
  await expect(summary.getByText("2 measured", { exact: true })).toBeVisible();
  await expect(summary.getByText("1", { exact: true })).toHaveCount(2);

  const deviceGrid = panel.getByLabel("Block devices data grid");
  await expect(deviceGrid.getByText("vda1", { exact: true }).first()).toBeVisible();
  await expect(deviceGrid.getByText("vdb", { exact: true }).first()).toBeVisible();
  await expect(deviceGrid.getByText("72%", { exact: true })).toBeVisible();
  await expect(deviceGrid.getByText("91%", { exact: true })).toBeVisible();
  if (testInfo.project.name.includes("mobile")) {
    await activate(
      deviceGrid.getByRole("button", {
        name: "Show details for Block devices row /dev/vdb",
      }),
    );
  } else {
    await activate(deviceGrid.getByText("vdb", { exact: true }).first());
  }
  await expect(deviceGrid.getByText("Major:minor")).toBeVisible();
  await expect(deviceGrid.getByText("cloud-archive-001")).toBeVisible();
  await expect(
    deviceGrid.getByRole("button", {
      name: "Close Block devices row details",
    }),
  ).toBeVisible();

  await activate(
    panel.getByRole("button", { name: "Mounts", exact: true }),
  );
  await expect(page).toHaveURL(/storage_view=mounts/);
  const mountGrid = panel.getByLabel("Mounted filesystems data grid");
  await expect(
    mountGrid.getByText("/srv/archive", { exact: true }).first(),
  ).toBeVisible();
  if (testInfo.project.name.includes("mobile")) {
    await activate(
      mountGrid.getByRole("button", {
        name: "Show details for Mounted filesystems row 37:/srv/archive",
      }),
    );
  } else {
    await activate(
      mountGrid.getByText("/srv/archive", { exact: true }).first(),
    );
  }
  await expect(mountGrid.getByText("Parent mount ID")).toBeVisible();
  await expect(
    mountGrid.getByText("attr2, inode64, ro").last(),
  ).toBeVisible();

  await panel.getByLabel("Storage inventory VPS").scrollIntoViewIfNeeded();
  await panel.getByRole("checkbox", { name: "System mounts" }).check();
  await expect(page).toHaveURL(/storage_system=1/);
  await expect(
    panel.getByText(/Refresh inventory to apply the changed setting/),
  ).toBeVisible();
  const beforeRefresh = await storageJobRequestCount(page);
  await panel
    .getByRole("button", { name: "Refresh inventory", exact: true })
    .evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
  await expect.poll(() => storageJobRequestCount(page)).toBe(beforeRefresh + 1);
  expect(await lastStorageJobRequest(page)).toMatchObject({
    command: "storage_inventory",
    confirmed: false,
    destructive: false,
    operation: {
      include_pseudo_mounts: true,
      limit: 2048,
      type: "storage_inventory",
    },
    privileged: false,
    selector_expression: "id:agent-sfo-01",
    target_client_ids: ["agent-sfo-01"],
  });
  await expect(
    panel.getByText(/Storage refreshed from edge-sfo-01/),
  ).toBeVisible();
  await expect(
    panel.getByText(/Refresh inventory to apply the changed setting/),
  ).toHaveCount(0);

  await page.reload();
  await expect(page.getByLabel("Storage inventory VPS")).toHaveValue(
    /edge-sfo-01/,
  );
  await expect(page.getByText("System mounts", { exact: true })).toBeVisible();
  await expect(page.getByRole("checkbox", { name: "System mounts" })).toBeChecked();
  await expect(
    page.getByRole("button", { name: "Mounts", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByLabel("Mounted filesystems data grid")).toBeVisible();
  await expect(page.getByLabel("Storage inventory VPS")).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1, 1);
  await page.waitForTimeout(300);
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("host-storage.png"),
  });
});

test("shows legacy lsblk limitations without inventing filesystem usage", async ({
  page,
}) => {
  const legacy = hostStorageInventory("agent-sfo-01");
  legacy.capability = {
    available_columns: [
      "NAME",
      "KNAME",
      "PKNAME",
      "TYPE",
      "SIZE",
      "FSTYPE",
      "LABEL",
      "UUID",
      "MOUNTPOINT",
      "RO",
      "RM",
      "MODEL",
      "SERIAL",
      "MAJ:MIN",
    ],
    can_report_filesystem_usage: false,
    provider: "lsblk_pairs",
    provider_version: "lsblk from util-linux 2.23.2",
    reason:
      "device inventory is supported; this lsblk version does not report FSAVAIL and FSUSE%",
    status: "supported",
  };
  legacy.devices = legacy.devices.map((device) => ({
    ...device,
    filesystem_available_bytes: null,
    filesystem_used_percent: null,
  }));
  await installConsoleApiMock(page, { hostStorageInventoryOverride: legacy });
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Storage");
  await page.getByLabel("Storage inventory VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();

  const panel = page.locator(".hostStoragePanel");
  await expect(panel.getByText("lsblk pairs", { exact: true })).toBeVisible();
  await expect(panel.getByText(/does not report FSAVAIL and FSUSE%/)).toBeVisible();
  await expect(
    panel.getByLabel("Storage inventory summary").getByText("Not reported"),
  ).toBeVisible();
  await expect(
    panel.getByLabel("Block devices data grid").locator(".mutedValue"),
  ).toHaveCount(3);
  await expect(panel.locator(".storageUsageTrack")).toHaveCount(0);
});

test("keeps an unsupported lsblk provider explicit and read-only", async ({
  page,
}) => {
  const unsupported = hostStorageInventory("agent-sfo-01");
  unsupported.capability = {
    available_columns: [],
    can_report_filesystem_usage: false,
    provider: null,
    provider_version: null,
    reason: "lsblk is not installed in a standard executable path or PATH",
    status: "unsupported",
  };
  unsupported.devices = [];
  unsupported.mounts = [];
  await installConsoleApiMock(page, {
    hostStorageInventoryOverride: unsupported,
  });
  await page.goto("/");
  await openConsoleSubpage(page, "Remote Operations", "Storage");
  await page.getByLabel("Storage inventory VPS").fill("edge-sfo-01");
  await page.getByRole("option", { name: /edge-sfo-01/ }).click();

  const panel = page.locator(".hostStoragePanel");
  await expect(panel.getByText(/^Unsupported:/)).toContainText(
    "lsblk is not installed",
  );
  await expect(panel.getByText("Storage inventory unsupported")).toBeVisible();
  await expect(
    panel.getByRole("button", { name: /format|unmount|resize/i }),
  ).toHaveCount(0);
  expect(await storageJobRequestCount(page)).toBe(0);
});

async function lastStorageJobRequest(page: Page) {
  return page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests
      .filter((request) => request.command === "storage_inventory")
      .at(-1);
  });
}

async function storageJobRequestCount(page: Page) {
  return page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.filter(
      (request) => request.command === "storage_inventory",
    ).length;
  });
}
