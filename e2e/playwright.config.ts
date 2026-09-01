import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright E2E config for Kansolo.
 *
 * Approach: Playwright runs against the Vite dev server (http://localhost:1420),
 * NOT against the Tauri webview via tauri-driver. Playwright does not natively
 * speak the WebDriver protocol — tauri-driver exposes a WebDriver bridge, which
 * Playwright cannot connect to. The official Tauri E2E guide uses WebdriverIO,
 * not Playwright. Rather than maintain two test runners, we use Playwright
 * against the Vite dev server, which exercises the same Solid UI / Ark UI
 * components and flows the user sees inside the Tauri webview.
 *
 * Limitations:
 *   - Tauri `invoke()` calls are not available in the browser. Tests must mock
 *     `@tauri-apps/api/core` via Playwright's `page.addInitScript` (see
 *     `e2e/tests/fixtures.ts`).
 *   - Native window behaviors (focus stealing, native menus) are not covered.
 *   - To run against the real Tauri app, use `webdriverio` + `tauri-driver`
 *     separately (out of scope for this step).
 *
 * Run: `deno task test:e2e` (starts the dev server + Playwright).
 * Dev server is expected to already be running on port 1420, OR
 * Playwright will start it via the `webServer` config below.
 */

const PORT = 1420;
const BASE_URL = `http://localhost:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false, // shared dev server + mocked DB state
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: "list",
  timeout: 30_000,
  expect: { timeout: 5_000 },

  use: {
    baseURL: BASE_URL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    // Mock Tauri invoke before each document loads. Individual specs can
    // override by calling page.addInitScript with their own handlers.
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

  // Auto-start the Vite dev server if it isn't already running.
  webServer: {
    command: "deno task dev",
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    stdout: "ignore",
    stderr: "pipe",
  },
});
