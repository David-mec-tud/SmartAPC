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
