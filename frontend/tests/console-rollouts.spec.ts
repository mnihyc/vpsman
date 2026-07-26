import { expect, test, type Page } from "@playwright/test";
import { installConsoleApiMock } from "./support/consoleLayoutFixtures";
import {
  activate,
  openConsoleSubpage,
  unlockPrivilegeFromTop,
} from "./support/consoleNavigation";

test("dispatches and operates a durable staged rollout", async ({
  page,
}, testInfo) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Jobs", "Dispatch");
  await unlockPrivilegeFromTop(page);

  const composer = page.locator(".commandComposer");
  await composer.getByLabel("Command argv").fill("/bin/echo staged-rollout");
  await composer
    .getByRole("combobox", { name: "Bulk target selector expression" })
    .fill("id:*");
  await expect(composer.getByText("All 3 scoped VPSs")).toBeVisible();

  await activate(composer.locator("details.dispatchExecutionOptions summary"));
  await composer.getByLabel("Staged rollout").check();
  await composer
    .getByLabel("Rollout canary VPS")
    .selectOption("agent-sfo-01");
  await composer.getByLabel("Batch size").fill("1");
  await composer.getByLabel("Tolerated failures").fill("0");
  await composer.getByLabel("Stage delay (seconds)").fill("30");
  await expect(composer.getByLabel("Pause after canary")).toBeChecked();

  await activate(composer.getByRole("button", { name: "Dispatch", exact: true }));
  const dispatchPrompt = page.getByLabel("Confirm job dispatch");
  await expect(dispatchPrompt).toBeVisible();
  await expect(dispatchPrompt).toContainText("Staged rollout");
  await expect(dispatchPrompt).toContainText("edge-sfo-01");
  await expect(dispatchPrompt).toContainText("1 VPS · 30s delay");
  await expect(dispatchPrompt).toContainText("pause after canary");
  await activate(
    dispatchPrompt.getByRole("button", { name: "Dispatch job", exact: true }),
  );
  await expect(dispatchPrompt).toBeHidden();

  const submitted = await page.evaluate(() => {
    const requests = (
      window as unknown as {
        __vpsmanTestRequests: { jobs: Array<Record<string, unknown>> };
      }
    ).__vpsmanTestRequests.jobs;
    return requests.at(-1);
  });
  expect(submitted).toMatchObject({
    rollout: {
      batch_delay_secs: 30,
      batch_size: 1,
      canary_client_ids: ["agent-sfo-01"],
      max_failures: 0,
      pause_after_canary: true,
    },
    selector_expression: "id:*",
    target_client_ids: ["agent-fra-02", "agent-nyc-03", "agent-sfo-01"],
  });
  await expect(
    composer.getByText(/Staged rollout accepted with 1 canary/),
  ).toBeVisible();
  await activate(
    composer.getByRole("button", { name: "Open staged rollout" }),
  );

  await expect(page).toHaveURL(/rollout_job=11111111-2222-4333-8444-555555555555/);
  const detail = page.locator(".consoleDetailPanel", {
    hasText: "Rollout 11111111",
  });
  await expect(detail).toBeVisible();
  await expect(detail.getByText("Canary review required")).toBeVisible();
  await expect(detail.getByText(/^edge-sfo-01/)).toBeVisible();
  await expect(detail.getByText(/^core-fra-02/)).toBeVisible();
  await expect(detail.getByText(/^backup-nyc-03/)).toBeVisible();

  await activate(detail.getByRole("button", { name: "Resume stage" }));
  const resumePrompt = page.getByLabel("Confirm stage release");
  await expect(resumePrompt).toBeVisible();
  await expect(resumePrompt).toContainText("1 VPS");
  const beforeResume = await rolloutActionRequestCount(page);
  await resumePrompt
    .getByRole("button", { name: "Resume stage", exact: true })
    .dblclick({ delay: 50 });
  await expect.poll(() => rolloutActionRequestCount(page)).toBe(
    beforeResume + 1,
  );
  await expect(resumePrompt).toBeHidden();
  await expect(page.getByLabel("Confirm rollout abort")).toHaveCount(0);
  await expect(detail.getByRole("button", { name: "Pause" })).toBeVisible();

  const beforePause = await rolloutActionRequestCount(page);
  await detail.getByRole("button", { name: "Pause" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await expect.poll(() => rolloutActionRequestCount(page)).toBe(
    beforePause + 1,
  );
  await expect(detail.getByRole("button", { name: "Resume stage" })).toBeVisible();
  await page.reload();
  await expect(page).toHaveURL(/rollout_job=11111111-2222-4333-8444-555555555555/);
  await expect(detail).toBeVisible();
  await expect(detail.getByRole("button", { name: "Resume stage" })).toBeVisible();

  await activate(detail.getByRole("button", { name: "Abort rollout" }));
  const abortPrompt = page.getByLabel("Confirm rollout abort");
  await expect(abortPrompt).toBeVisible();
  await expect(abortPrompt).toContainText("Unreleased targets will be canceled immediately");
  await activate(
    abortPrompt.getByRole("button", { name: "Abort rollout", exact: true }),
  );
  await expect(abortPrompt).toBeHidden();
  await expect(detail.getByText("Aborted", { exact: true }).first()).toBeVisible();
  await expect(detail.getByRole("button", { name: "Resume stage" })).toHaveCount(0);
  await expect(detail.getByRole("button", { name: "Abort rollout" })).toHaveCount(0);

  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
  await page.evaluate(() => window.scrollTo(0, 0));
  await settleScreenshot(page);
  await page.screenshot({
    fullPage: true,
    path: testInfo.outputPath("staged-rollout.png"),
  });
});

test("keeps a rejected stage release beside the reviewed rollout action", async ({
  page,
}) => {
  await installConsoleApiMock(page);
  await page.goto("/");
  await openConsoleSubpage(page, "Automation", "Rollouts");
  const reviewButton = page.getByRole("button", { name: "Review rollout" });
  if (await reviewButton.isVisible()) {
    await activate(reviewButton);
  } else {
    await page
      .getByRole("button", { name: "Actions for rollout 55555555" })
      .click();
    await activate(page.getByRole("menuitem", { name: "Review rollout" }));
  }
  const detail = page.locator(".consoleDetailPanel", {
    hasText: "Rollout 55555555",
  });
  await expect(detail).toBeVisible();
  await page.evaluate(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const request = input instanceof Request ? input : null;
      const url = request?.url ?? String(input);
      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
      const path = new URL(url, location.href).pathname;
      if (
        method === "POST" &&
        path.endsWith("/resume") &&
        path.includes("/api/v1/job-rollouts/")
      ) {
        return new Response(
          JSON.stringify({
            error: "rollout_state_changed",
            message: "The rollout changed after this stage was reviewed.",
            recovery:
              "Reload rollout evidence and review the current stage before resuming.",
          }),
          {
            headers: { "content-type": "application/json" },
            status: 409,
          },
        );
      }
      return originalFetch(input, init);
    };
  });
  await activate(detail.getByRole("button", { name: "Resume stage" }));
  const prompt = page.getByLabel("Confirm stage release");
  await activate(
    prompt.getByRole("button", { name: "Resume stage", exact: true }),
  );

  await expect(prompt).toBeVisible();
  await expect(
    prompt.getByText(/The rollout changed after this stage was reviewed/),
  ).toBeVisible();
  await expect(
    prompt.getByText(/Reload rollout evidence and review the current stage/),
  ).toBeVisible();
  await expect(page.locator(".sectionHeader .actionFeedback")).toHaveCount(0);
  await expect(detail.locator(".rolloutActionFeedback")).toHaveCount(0);

  await activate(prompt.getByRole("button", { name: "Cancel" }));
  await expect(prompt).toBeHidden();
  await expect(detail.locator(".rolloutActionFeedback")).toContainText(
    "The rollout changed after this stage was reviewed",
  );
});

async function settleScreenshot(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1, 1);
  await page.waitForTimeout(300);
}

async function rolloutActionRequestCount(page: Page) {
  return page.evaluate(() =>
    (
      window as unknown as {
        __vpsmanTestRequests: { jobRolloutActions: unknown[] };
      }
    ).__vpsmanTestRequests.jobRolloutActions.length,
  );
}
