import { defineConfig } from "@playwright/test";

const port = Number(process.env.VPSMAN_FRONTEND_TEST_PORT ?? 5174);
const host = process.env.VPSMAN_FRONTEND_TEST_HOST ?? "localhost";
const baseURL = `http://${host}:${port}`;
const channel = process.env.VPSMAN_PLAYWRIGHT_CHANNEL ?? "chrome";
const executablePath = process.env.VPSMAN_PLAYWRIGHT_EXECUTABLE_PATH;
const launchArgs = ["--disable-enterprise-policy"];
if (process.env.VPSMAN_PLAYWRIGHT_NO_SANDBOX === "1") {
  launchArgs.push("--no-sandbox");
}

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  reporter: [["list"]],
  webServer: {
    command: `npm run dev -- --host ${host} --port ${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    url: baseURL,
  },
  use: {
    baseURL,
    ...(executablePath
      ? { launchOptions: { args: launchArgs, executablePath } }
      : { channel }),
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "desktop-chrome",
      use: { viewport: { width: 1440, height: 900 } },
    },
    {
      name: "mobile-chrome",
      use: { isMobile: true, viewport: { width: 390, height: 844 } },
    },
  ],
});
