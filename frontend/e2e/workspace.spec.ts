import { test, expect, type APIRequestContext } from "@playwright/test";
// Every run creates a separate temporary project on the test backend. No OPC UA
// test writes target any existing controller; engineering execution is simulated.
async function seed(request: APIRequestContext, model = true) {
  const name = `journey_${Date.now()}_${Math.floor(Math.random() * 100000)}`;
  expect(
    (await request.post("/api/projects", { data: { name } })).ok(),
  ).toBeTruthy();
  const response = await request.get(`/api/projects/${name}/config`);
  const config = await response.json();
  Object.assign(config.variables, { mvs: ["u1"], cvs: ["y1"], dvs: [] });
  config.communication.endpoint_url = "mock://browser-journey";
  config.communication.cycle_ms = 100;
  config.io.cycle_ms = 100;
  config.logic.cycle_ms = 100;
  config.io_definition.mvs = [
    {
      name: "u1",
      mv_write: "ns=2;s=U1_WRITE",
      mv_read: "ns=2;s=U1_READ",
      ll: -10,
      hl: 10,
    },
  ];
  config.io_definition.cvs = [
    {
      name: "y1",
      setpoint: "ns=2;s=Y1_SP",
      process_value: "ns=2;s=Y1_PV",
      ll: -100,
      hl: 100,
    },
  ];
  config.logic.p_controller.pairs = [{ cv: "y1", mv: "u1", kp: 1, bias: 0 }];
  if (model)
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
  config.simulation.cv_setpoints = { y1: 2 };
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  return name;
}
test("save and reload optional CV offsets; report stale configuration without losing a draft", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  await page.goto("/configuration/io");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page.getByRole("button", { name: "CVS 1", exact: true }).click();
  await page.getByLabel("y1 Band low offset").fill("-2.5");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    page.getByText("Configuration saved and normalized by Rust."),
  ).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: "CVS 1", exact: true }).click();
  await expect(page.getByLabel("y1 Band low offset")).toHaveValue("-2.5");
  await page.getByLabel("y1 Band high offset").fill("3");
  const saved = await request.get(`/api/projects/${name}/config`);
  const external = await saved.json();
  external.communication.cycle_ms = 50;
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: external,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    page.getByText("saved project changed; reload before saving this draft"),
  ).toBeVisible();
  await expect(page.getByLabel("y1 Band high offset")).toHaveValue("3");
});
test("simulation runs without a browser; typed parameters apply and Plotly renders real history", async ({
  page,
  request,
  browser,
}) => {
  const name = await seed(request);
  await page.goto("/engineering/simulation");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page.getByRole("button", { name: "Start saved simulation" }).click();
  await expect(
    page.getByRole("button", { name: "Stop simulation", exact: true }).first(),
  ).toBeEnabled();
  await page
    .getByRole("link", { name: "Online parameters", exact: true })
    .click();
  await page.getByLabel("p.y1.u1.kp", { exact: true }).fill("2.5");
  await page
    .getByRole("row")
    .filter({ hasText: "p.y1.u1.kp" })
    .getByRole("button", { name: "Apply online" })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .params["p.y1.u1.kp"],
    )
    .toBe(2.5);
  await page.getByRole("link", { name: "Trends", exact: true }).click();
  await expect(page.locator(".js-plotly-plot")).toBeVisible();
  // Live updates must retain operator zoom and legend state.
  await page.evaluate(async () => {
    const plot = document.querySelector(".js-plotly-plot") as HTMLElement;
    await (
      window as unknown as {
        Plotly: {
          relayout: (plot: HTMLElement, update: unknown) => Promise<void>;
        };
      }
    ).Plotly.relayout(plot, {
      "xaxis.range": [0, 1],
      "xaxis.autorange": false,
    });
  });
  const plottedBefore = await page
    .locator(".js-plotly-plot")
    .evaluate(
      (element) =>
        (element as unknown as { data: { x: number[] }[] }).data[0].x.at(-1) ??
        0,
    );
  await expect
    .poll(async () =>
      page
        .locator(".js-plotly-plot")
        .evaluate(
          (element) =>
            (element as unknown as { data: { x: number[] }[] }).data[0].x.at(
              -1,
            ) ?? 0,
        ),
    )
    .toBeGreaterThan(plottedBefore);
  await expect
    .poll(async () =>
      page
        .locator(".js-plotly-plot")
        .evaluate(
          (element) =>
            (element as unknown as { layout: { xaxis: { range: number[] } } })
              .layout.xaxis.range,
        ),
    )
    .toEqual([0, 1]);
  const before = await (await request.get("/api/execution/state")).json();
  await page.close();
  await expect
    .poll(
      async () =>
        (await (await request.get("/api/execution/state")).json()).snapshot
          .metrics.total_cycles,
    )
    .toBeGreaterThan(before.snapshot.metrics.total_cycles);
  const reconnect = await browser.newPage();
  await reconnect.goto("/operation/diagnostics");
  await expect(
    reconnect.getByRole("button", { name: "Stop simulation", exact: true }),
  ).toBeVisible();
  await reconnect
    .getByRole("button", { name: "Stop simulation", exact: true })
    .click();
  await expect
    .poll(async () => (await request.get("/api/execution/state")).json())
    .toBeNull();
});
test("control-test job completes across navigation and deployment requires no optional model", async ({
  page,
  request,
}) => {
  const name = await seed(request, false);
  await page.goto("/engineering/tests");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page
    .getByRole("button", { name: "Run control tests", exact: true })
    .click();
  await page.getByRole("link", { name: "Validate & run", exact: true }).click();
  await expect(page.getByText("Ready to start", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Start saved runtime" }),
  ).toBeEnabled();
  await page.getByRole("link", { name: "Control tests", exact: true }).click();
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Switch to dark theme" }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);
});

test("MPC movement cost stays boolean and external CV edges stay out of editing and numeric optimization", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const response = await request.get(`/api/projects/${name}/config`);
  const config = await response.json();
  config.logic.kind = "linear_mpc";
  config.logic.params = {};
  config.io_definition.cvs[0].band_low_opc = "ns=2;s=EXTERNAL_BAND_LOW";
  config.io_definition.cvs[0].band_low_value = -0.5;
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/engineering/simulation");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page.getByRole("button", { name: "Start saved simulation" }).click();
  await page
    .getByRole("link", { name: "Online parameters", exact: true })
    .click();
  await page
    .getByLabel("mpc.mv.u1.use_l1", { exact: true })
    .selectOption("true");
  await page
    .getByRole("row")
    .filter({ hasText: "mpc.mv.u1.use_l1" })
    .getByRole("button", { name: "Apply online" })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .params["mpc.mv.u1.use_l1"],
    )
    .toBe(true);
  await expect(
    page.getByLabel("mpc.cv.y1.band_low", { exact: true }),
  ).toHaveCount(0);
  const catalog = await (
    await request.get(`/api/projects/${name}/engineering`)
  ).json();
  expect(catalog.online_optimization_parameters).not.toHaveProperty(
    "mpc.mv.u1.use_l1",
  );
  expect(catalog.online_optimization_parameters).not.toHaveProperty(
    "mpc.cv.y1.band_low",
  );
  await page
    .getByRole("button", { name: "Stop simulation", exact: true })
    .click();
});
