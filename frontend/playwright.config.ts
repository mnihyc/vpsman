import { defineConfig } from "@playwright/test";

const port = Number(process.env.VPSMAN_FRONTEND_TEST_PORT ?? 5174);
const baseURL = `http://127.0.0.1:${port}`;
const channel = process.env.VPSMAN_PLAYWRIGHT_CHANNEL ?? "chrome";

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  reporter: [["list"]],
  webServer: {
    command: `npm run dev -- --port ${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    url: baseURL,
  },
  use: {
    baseURL,
    channel,
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
