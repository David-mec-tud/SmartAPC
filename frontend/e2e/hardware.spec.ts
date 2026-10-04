import { test, expect } from "@playwright/test";
import { stopTestExecution } from "./fixtures";
test.afterEach(async ({ request }) => stopTestExecution(request));
// Both processes are started by Playwright in isolated local ports. These tests
// address only the disposable virtual plant owned by this suite.
const plantUrl = "http://127.0.0.1:18082";
test("headless virtual plant serves Plotly and applies manual node values", async ({
  page,
  request,
}) => {
  await page.goto(`${plantUrl}/virtual-hardware`);
  await expect(page.getByText("Plant running", { exact: true })).toBeVisible();
  await page.getByLabel("u1_write", { exact: true }).fill("1.5");
  await page.getByLabel("y1_target", { exact: true }).fill("2");
  await page
    .getByRole("button", { name: "Apply manual values", exact: true })
    .click();
  await expect(
    page.getByText("Values queued for the next plant tick."),
  ).toBeVisible();
  await expect
    .poll(
      async () =>
        (await (await request.get(`${plantUrl}/api/virtual-hardware`)).json())
          .latest.sample.u,
    )
    .toBe(1.5);
  await expect
    .poll(
      async () =>
        (await (await request.get(`${plantUrl}/api/virtual-hardware`)).json())
          .latest.sample.y,
    )
    .toBeGreaterThan(0);
  await expect(page.locator(".js-plotly-plot")).toHaveCount(2);
  await page.getByRole("button", { name: "Switch to dark theme" }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  const before = (
    await (await request.get(`${plantUrl}/api/virtual-hardware`)).json()
  ).latest.t_s;
  await page.close();
  await expect
    .poll(
      async () =>
        (await (await request.get(`${plantUrl}/api/virtual-hardware`)).json())
          .latest.t_s,
    )
    .toBeGreaterThan(before);
});

test("real OPC UA read/write/watchdog tests and saved runtime operate against the disposable plant", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const name = `hardware_${Date.now()}`;
  expect(
    (await request.post("/api/projects", { data: { name } })).ok(),
  ).toBeTruthy();
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  config.communication.endpoint_url = "opc.tcp://127.0.0.1:14856/";
  config.communication.cycle_ms = 100;
  config.io.cycle_ms = 200;
  config.logic.cycle_ms = 150;
  config.variables = { mvs: ["u1"], cvs: ["y1"], dvs: [] };
  config.io_definition = {
    mvs: [
      {
        name: "u1",
        mv_write: "ns=2;s=u1_write",
        mv_read: "ns=2;s=u1_read",
        ll: -10,
        hl: 10,
      },
    ],
    cvs: [
      {
        name: "y1",
        setpoint: "ns=2;s=y1_target",
        process_value: "ns=2;s=y1_value",
        ll: -100,
        hl: 100,
      },
    ],
    dvs: [],
  };
  config.logic.p_controller.pairs = [{ cv: "y1", mv: "u1", kp: 1, bias: 0 }];
  config.watchdog = {
    kind: "counter_mirror",
    read_node: "ns=2;s=wd_counter_read",
    write_node: "ns=2;s=wd_counter_mirror",
    stale_timeout_ms: 2000,
  };
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/configuration/io");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page
    .getByRole("button", { name: "Run read test", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/jobs`)).json()).find(
          (job: { kind: string }) => job.kind === "io_read",
        )?.phase,
      { timeout: 20_000 },
    )
    .toBe("succeeded");
  await page.getByLabel("cv.y1.setpoint", { exact: true }).first().fill("3");
  await page
    .getByRole("button", { name: "Write entered test values", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/jobs`)).json()).find(
          (job: { kind: string }) => job.kind === "io_write",
        )?.phase,
      { timeout: 20_000 },
    )
    .toBe("succeeded");
  await expect
    .poll(
      async () =>
        (await (await request.get(`${plantUrl}/api/virtual-hardware`)).json())
          .latest.sample.target,
    )
    .toBe(3);
  await page
    .getByRole("link", { name: "Connection & timing", exact: true })
    .click();
  await page.getByRole("button", { name: /Test saved watchdog/ }).click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/jobs`)).json()).find(
          (job: { kind: string }) => job.kind === "watchdog_test",
        )?.phase,
      { timeout: 20_000 },
    )
    .toBe("succeeded");
  await page.getByRole("link", { name: "Validate & run", exact: true }).click();
  await expect(page.getByText("Ready to start", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "Start saved runtime", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get("/api/execution/state")).json())?.snapshot
          .connection_health,
      { timeout: 20_000 },
    )
    .toBe("healthy");
  await expect
    .poll(
      async () =>
        (await (await request.get("/api/execution/state")).json())?.snapshot
          .metrics.total_cycles,
    )
    .toBeGreaterThan(2);
  await page.getByRole("link", { name: "Trends", exact: true }).click();
  await expect(page.locator(".js-plotly-plot")).toBeVisible();
  await page.getByRole("button", { name: "Stop runtime", exact: true }).click();
  await expect
    .poll(async () => await (await request.get("/api/execution/state")).json())
    .toBeNull();
});
