import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./e2e",
  workers: 1,
  timeout: 45_000,
  use: {
    actionTimeout: 10_000,
    baseURL: process.env.SMARTAPC_TEST_URL ?? "http://127.0.0.1:18080",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: process.env.SMARTAPC_TEST_URL
    ? undefined
    : [
        {
          command:
            "cargo run --offline --manifest-path ../Cargo.toml -- serve --port 18080 --project-root /private/tmp/smartapc-playwright-projects",
          url: "http://127.0.0.1:18080/api/projects",
          timeout: 120_000,
          reuseExistingServer: false,
          env: { CARGO_NET_OFFLINE: "true" },
        },
        {
          command:
            "cargo run --offline --manifest-path ../Cargo.toml -- virtual-hardware --port 14856 --web-port 18082 --dt-ms 100 --tau-s 1",
          url: "http://127.0.0.1:18082/api/virtual-hardware",
          timeout: 120_000,
          reuseExistingServer: false,
        },
      ],
});
