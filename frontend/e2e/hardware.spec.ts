import { test, expect, type APIRequestContext } from "@playwright/test";
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

async function createHardwareProject(request: APIRequestContext) {
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
  return name;
}

test("real OPC UA read/write/watchdog tests and saved runtime operate against the disposable plant", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const name = await createHardwareProject(request);
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
  const readResult = page.getByRole("region", { name: "Read test result" });
  await expect(readResult.getByText("Passed", { exact: true })).toBeVisible();
  await expect(
    readResult.getByRole("row", { name: /cv.y1.setpoint/ }),
  ).toContainText("ns=2;s=y1_target");
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
  await expect(
    page.getByRole("region", { name: "Write test result" }),
  ).toContainText("Successfully wrote 1 test value to hardware.");
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
  await expect(
    page.getByRole("region", { name: "Watchdog test result" }),
  ).toContainText("read and mirrored successfully");
  await page.getByRole("link", { name: "Validate & run", exact: true }).click();
  await expect(
    page.getByText("Ready for shadow mode", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Start in shadow mode", exact: true })
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
  await expect(page.locator(".js-plotly-plot").first()).toBeVisible();
  await page.getByRole("button", { name: "Stop runtime", exact: true }).click();
  await expect
    .poll(async () => await (await request.get("/api/execution/state")).json())
    .toBeNull();
});

test("hardware failures stay beside each test through edits, navigation and retries until that test passes", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const name = await createHardwareProject(request);
  const other = await createHardwareProject(request);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  config.io_definition.mvs[0].mv_read = "ns=2;s=missing_read";
  config.io_definition.mvs[0].mv_write = "ns=2;s=missing_write";
  config.watchdog.read_node = "ns=2;s=missing_counter";
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
  const readResult = page.getByRole("region", { name: "Read test result" });
  await expect(
    readResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await expect(readResult.getByRole("alert")).toContainText("mv.u1.read");
  // The backend read job completes even when individual nodes fail. Its report
  // must drive the visual failure state rather than the job phase alone.
  await expect(
    readResult.getByRole("row", { name: /mv.u1.read/ }),
  ).toContainText("missing_read");

  await page.getByLabel("mv.u1.write", { exact: true }).first().fill("1");
  await page
    .getByRole("button", { name: "Write entered test values", exact: true })
    .click();
  const writeResult = page.getByRole("region", { name: "Write test result" });
  await expect(
    writeResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await expect(writeResult.getByRole("alert")).not.toBeEmpty();
  await page
    .getByRole("link", { name: "Connection & timing", exact: true })
    .click();
  await page.getByRole("button", { name: /Test saved watchdog/ }).click();
  const watchdogResult = page.getByRole("region", {
    name: "Watchdog test result",
  });
  await expect(
    watchdogResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await expect(watchdogResult.getByRole("alert")).not.toBeEmpty();
  await page.reload();
  await expect(
    watchdogResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();

  await page
    .getByLabel("read node", { exact: true })
    .fill("ns=2;s=wd_counter_read");
  await expect(
    watchdogResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /Test saved watchdog/ }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    watchdogResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: /Test saved watchdog/ }).click();
  await expect(
    watchdogResult.getByText("Passed", { exact: true }),
  ).toBeVisible();
  await expect(watchdogResult.getByRole("alert")).toHaveCount(0);

  await page
    .getByRole("link", { name: "Variables & I/O", exact: true })
    .click();
  await expect(
    readResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await expect(
    writeResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await page
    .getByLabel("u1 Read Node ID", { exact: true })
    .fill("ns=2;s=u1_read");
  await page
    .getByLabel("u1 Write Node ID", { exact: true })
    .fill("ns=2;s=u1_write");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    readResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();

  // Hold submission briefly to verify that retrying does not erase the failure.
  let releaseRetry!: () => void;
  const retryGate = new Promise<void>((resolve) => {
    releaseRetry = resolve;
  });
  const jobRoute = `**/api/projects/${name}/jobs`;
  await page.route(jobRoute, async (route) => {
    if (route.request().method() === "POST") await retryGate;
    await route.continue();
  });
  let releaseReport!: () => void;
  const reportGate = new Promise<void>((resolve) => {
    releaseReport = resolve;
  });
  const reportRoute = `**/api/projects/${name}/jobs/*`;
  await page.route(reportRoute, async (route) => {
    await reportGate;
    await route.continue();
  });
  await page
    .getByRole("button", { name: "Run read test", exact: true })
    .click();
  try {
    await expect(readResult).toContainText("Starting read test");
    await expect(
      readResult.getByText("Test failed", { exact: true }),
    ).toBeVisible();
  } finally {
    releaseRetry();
  }
  try {
    await expect
      .poll(async () => {
        const jobs = await (
          await request.get(`/api/projects/${name}/jobs`)
        ).json();
        return jobs.find((job: { kind: string }) => job.kind === "io_read")
          ?.phase;
      })
      .toBe("succeeded");
    // A succeeded summary cannot clear the old failure before per-node results
    // arrive: read jobs also use this phase when one or more nodes fail.
    await expect(
      readResult.getByText("Test failed", { exact: true }),
    ).toBeVisible();
  } finally {
    releaseReport();
  }
  await expect(readResult.getByText("Passed", { exact: true })).toBeVisible();
  await page.unroute(jobRoute);
  await page.unroute(reportRoute);
  await expect(readResult.getByRole("alert")).toHaveCount(0);
  // A successful read must not clear the independent write failure.
  await expect(
    writeResult.getByText("Test failed", { exact: true }),
  ).toBeVisible();
  await page.getByLabel("mv.u1.write", { exact: true }).first().fill("1");
  await page
    .getByRole("button", { name: "Write entered test values", exact: true })
    .click();
  await expect(writeResult.getByText("Passed", { exact: true })).toBeVisible();
  await expect(writeResult.getByRole("alert")).toHaveCount(0);

  await page.getByLabel("Editing project", { exact: true }).selectOption(other);
  await expect(readResult).toContainText("No read test results yet");
  await expect(writeResult).toContainText("No write test results yet");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await expect(readResult.getByText("Passed", { exact: true })).toBeVisible();
});

test("a rejected test request retains its error across navigation until a successful test", async ({
  page,
  request,
}) => {
  const name = await createHardwareProject(request);
  await page.goto("/configuration/communication");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  const jobRoute = `**/api/projects/${name}/jobs`;
  await page.route(jobRoute, async (route) => {
    if (route.request().method() === "POST")
      await route.fulfill({
        status: 409,
        contentType: "application/json",
        body: JSON.stringify({
          error: "Hardware is busy. Stop execution first.",
        }),
      });
    else await route.continue();
  });
  await page.getByRole("button", { name: /Test saved watchdog/ }).click();
  const result = page.getByRole("region", { name: "Watchdog test result" });
  await expect(result.getByText("Test failed", { exact: true })).toBeVisible();
  await expect(result.getByRole("alert")).toContainText("Hardware is busy");
  await page
    .getByRole("link", { name: "Variables & I/O", exact: true })
    .click();
  await page
    .getByRole("link", { name: "Connection & timing", exact: true })
    .click();
  await expect(result.getByRole("alert")).toContainText("Hardware is busy");
  await page.unroute(jobRoute);
  await page.getByRole("button", { name: /Test saved watchdog/ }).click();
  await expect(result.getByText("Passed", { exact: true })).toBeVisible();
  await expect(result.getByRole("alert")).toHaveCount(0);
});

test("deployment handshake tests preserve shadow startup and clear flags on stop", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const name = await createHardwareProject(request);
  await page.goto("/deployment");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page
    .getByLabel("ControllerIsReady Node ID", { exact: true })
    .fill("ns=2;s=ControllerIsReady");
  await page
    .getByLabel("ControllerHasControl Node ID", { exact: true })
    .fill("ns=2;s=ControllerHasControl");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Test saved controller signals", exact: true })
    .click();
  const result = page.getByRole("region", {
    name: "Controller signals test result",
  });
  await expect(result.getByText("Passed", { exact: true })).toBeVisible();
  await expect(
    result.getByRole("row", { name: /ControllerIsReady/ }),
  ).toContainText("false");
  await expect(
    result.getByRole("row", { name: /ControllerHasControl/ }),
  ).toContainText("false");
  await request.post(`${plantUrl}/api/virtual-hardware`, {
    data: { u_write: 0.5, target: 3, hl: 10, ll: -10 },
  });
  const plant = async () =>
    await (await request.get(`${plantUrl}/api/virtual-hardware`)).json();
  await expect.poll(async () => (await plant()).latest.sample.u).toBe(0.5);
  await page
    .getByRole("button", { name: "Start in shadow mode", exact: true })
    .click();
  const execution = async () =>
    await (await request.get("/api/execution/state")).json();
  await expect
    .poll(async () => (await execution())?.snapshot.controller.is_ready)
    .toBe(true);
  expect((await execution()).snapshot.controller.enabled).toBe(false);
  expect((await plant()).latest.sample.u).toBe(0.5);
  expect((await plant()).controller_signals.ControllerHasControl).toBe(false);
  await expect(
    page
      .getByRole("region", { name: "Verify before enabling control" })
      .getByText("Shadow mode", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Turn controller on", exact: true })
    .click();
  await expect
    .poll(async () => (await plant()).controller_signals.ControllerHasControl)
    .toBe(true);
  await page
    .getByRole("button", { name: "Turn controller off", exact: true })
    .click();
  await expect
    .poll(async () => (await plant()).controller_signals.ControllerHasControl)
    .toBe(false);
  expect((await execution()).snapshot.controller.is_ready).toBe(true);
  await page.getByRole("button", { name: "Stop runtime", exact: true }).click();
  await expect
    .poll(async () => (await plant()).controller_signals.ControllerIsReady)
    .toBe(false);
  expect((await plant()).controller_signals.ControllerHasControl).toBe(false);
});

test("external enable is tested read only, disables the UI, and owns runtime on/off", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  const name = await createHardwareProject(request);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  Object.assign(config.deployment, {
    controller_is_ready: "ns=2;s=ControllerIsReady",
    controller_has_control: "ns=2;s=ControllerHasControl",
    external_controller_enable: "ns=2;s=ExternalControllerEnable",
  });
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  const setEnable = (enabled: boolean) =>
    request.post(`${plantUrl}/api/virtual-hardware`, {
      data: {
        u_write: 0,
        target: 2,
        hl: 10,
        ll: -10,
        external_controller_enable: enabled,
      },
    });
  const plant = async () =>
    await (await request.get(`${plantUrl}/api/virtual-hardware`)).json();
  await setEnable(true);
  await expect
    .poll(
      async () => (await plant()).controller_signals.ExternalControllerEnable,
    )
    .toBe(true);
  await page.goto("/deployment");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await expect(
    page.getByText(/Keep this external signal false to remain in shadow mode/),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Test saved controller signals", exact: true })
    .click();
  await expect(
    page
      .getByRole("region", { name: "Controller signals test result" })
      .getByText("Passed", { exact: true }),
  ).toBeVisible();
  expect((await plant()).controller_signals.ExternalControllerEnable).toBe(
    true,
  );
  await setEnable(false);
  await expect
    .poll(
      async () => (await plant()).controller_signals.ExternalControllerEnable,
    )
    .toBe(false);
  await page
    .getByRole("button", { name: "Start in shadow mode", exact: true })
    .click();
  await expect(
    page.getByRole("button", {
      name: "ExternalControllerEnable owns on/off",
      exact: true,
    }),
  ).toBeDisabled();
  const current = await (await request.get("/api/execution/state")).json();
  expect(
    (
      await request.post("/api/execution/command", {
        data: {
          action: "enable_control",
          mode: "runtime",
          project_name: name,
          execution_id: current.snapshot.execution_id,
        },
      })
    ).ok(),
  ).toBeFalsy();
  await setEnable(true);
  await expect
    .poll(async () => (await plant()).controller_signals.ControllerHasControl)
    .toBe(true);
  await setEnable(false);
  await expect
    .poll(async () => (await plant()).controller_signals.ControllerHasControl)
    .toBe(false);
  await page.getByRole("button", { name: "Stop runtime", exact: true }).click();
  await expect
    .poll(async () => (await plant()).controller_signals.ControllerIsReady)
    .toBe(false);
});

test("MPC reads real external band offsets in shadow mode and tracks changed target bands", async ({
  request,
}) => {
  test.setTimeout(90_000);
  const name = await createHardwareProject(request);
  const response = await request.get(`/api/projects/${name}/config`);
  const config = await response.json();
  config.logic.kind = "linear_mpc";
  config.logic.cycle_ms = 100;
  config.io.cycle_ms = 100;
  config.model = {
    kind: "linear_state_space",
    state_count: 1,
    u_order: ["u1"],
    y_order: ["y1"],
    a: [[-1]],
    b: [[1]],
    c: [[1]],
    d: [[0]],
  };
  Object.assign(config.io_definition.cvs[0], {
    band_low_opc: "ns=2;s=y1_band_low",
    band_high_opc: "ns=2;s=y1_band_high",
    band_low_value: -0.5,
    band_high_value: 0.5,
  });
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  expect(
    (
      await request.post(`${plantUrl}/api/virtual-hardware`, {
        data: { u_write: 0, target: 2, band_low: -0.25, band_high: 0.75 },
      })
    ).ok(),
  ).toBeTruthy();
  expect(
    (
      await request.post("/api/runtime/start", {
        data: { project_name: name, mode: "runtime" },
      })
    ).ok(),
  ).toBeTruthy();
  const state = async () =>
    await (await request.get("/api/execution/state")).json();
  await expect
    .poll(async () => (await state())?.snapshot.controller?.is_ready, {
      timeout: 40_000,
    })
    .toBe(true);
  const shadow = await state();
  expect(shadow.snapshot.controller.enabled).toBe(false);
  expect(shadow.snapshot.controller.has_control).toBe(false);
  expect(shadow.snapshot.last_inputs["cv.y1.band_low"].value).toBe(-0.25);
  expect(shadow.snapshot.last_inputs["cv.y1.band_high"].value).toBe(0.75);
  const identity = {
    project_name: name,
    mode: "runtime",
    execution_id: shadow.snapshot.execution_id,
  };
  expect(
    (
      await request.post("/api/execution/command", {
        data: { ...identity, action: "enable_control" },
      })
    ).ok(),
  ).toBeTruthy();
  await expect
    .poll(async () => (await state())?.snapshot.controller?.has_control)
    .toBe(true);
  expect(
    (
      await request.post(`${plantUrl}/api/virtual-hardware`, {
        data: { u_write: 0, target: 3, band_low: -0.5, band_high: 0.5 },
      })
    ).ok(),
  ).toBeTruthy();
  await expect
    .poll(
      async () =>
        (await state())?.snapshot.last_inputs["cv.y1.setpoint"]?.value,
    )
    .toBe(3);
  await expect
    .poll(
      async () => {
        const pv = (await state())?.snapshot.last_inputs["cv.y1.process_value"]
          ?.value;
        return typeof pv === "number" && pv >= 2.45 && pv <= 3.55;
      },
      { timeout: 25_000 },
    )
    .toBe(true);
});
