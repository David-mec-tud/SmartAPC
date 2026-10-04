import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./e2e",
  workers: 1,
  timeout: 45_000,
  use: {
    baseURL: process.env.SMARTAPC_TEST_URL ?? "http://127.0.0.1:18080",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: process.env.SMARTAPC_TEST_URL
    ? undefined
    : {
        command:
          "cargo run --offline --manifest-path ../Cargo.toml -- serve --port 18080 --project-root /private/tmp/smartapc-playwright-projects",
        url: "http://127.0.0.1:18080/api/projects",
        timeout: 120_000,
        reuseExistingServer: false,
      },
});
