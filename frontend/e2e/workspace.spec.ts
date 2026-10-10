import { test, expect } from "@playwright/test";
import { seed, stopTestExecution } from "./fixtures";
test.afterEach(async ({ request }) => stopTestExecution(request));
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

test("custom source drafts follow their project; build, schema, simulation and user logs work through Rust", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const name = await seed(request);
  const other = await seed(request);
  const savedIo = await request.get(`/api/projects/${name}/config`);
  const typedIo = await savedIo.json();
  typedIo.io_definition.custom = [
    {
      name: "lamp",
      signal_type: "bool",
      read: "ns=2;s=LampRead",
      write: "ns=2;s=LampWrite",
    },
  ];
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: typedIo,
        headers: { "If-Match": savedIo.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/configuration/strategy");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  const selectedStrategy = page.getByLabel("Selected strategy");
  const customConfiguration = page.getByRole("heading", {
    name: "Custom Rust strategy",
    exact: true,
  });
  await expect(selectedStrategy).toHaveValue("p_controller");
  await expect(customConfiguration).toHaveCount(0);
  await page.getByLabel("Selected strategy").selectOption("linear_mpc");
  await expect(customConfiguration).toHaveCount(0);
  await selectedStrategy.selectOption("custom_rust");
  await expect(customConfiguration).toBeVisible();
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Generate template", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .custom_rust.strategy_name,
    )
    .not.toBe("");
  await expect(selectedStrategy).toHaveValue("custom_rust");
  const editor = page.getByLabel("Custom Rust source");
  await expect(editor).toContainText("pub fn control");
  await expect(editor).toContainText(
    'out.set_bool("custom.lamp.write", false);',
  );
  const source =
    (await editor.inputValue())
      .replace(
        "pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs)",
        "pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs, gain: f64, enabled: bool)",
      )
      .replace(
        "let _ = ctx;",
        'let _ = enabled;\n    out.log_value("web_custom", "custom.gain", SignalValue::F64(gain));\n    out.log_value("web_custom", "custom.history", SignalValue::F64(ctx.input_history.len() as f64));',
      )
      .replace(
        'out.set_f64("mv.u1.write", 0.0);',
        'out.set_f64("mv.u1.write", gain);',
      ) + "\n// Draft owned by first project\n";
  await editor.fill(source);
  await page
    .getByRole("link", { name: "Variables & I/O", exact: true })
    .click();
  await page
    .getByRole("link", { name: "Control strategy", exact: true })
    .click();
  await expect(editor).toHaveValue(source);
  await page.getByLabel("Editing project", { exact: true }).selectOption(other);
  await expect(editor).toHaveCount(0);
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await expect(editor).toHaveValue(source);
  // Hiding custom configuration must retain its project-owned source draft.
  for (const kind of ["p_controller", "linear_mpc"]) {
    await selectedStrategy.selectOption(kind);
    await expect(customConfiguration).toHaveCount(0);
    await expect(editor).toHaveCount(0);
    await selectedStrategy.selectOption("custom_rust");
    await expect(editor).toHaveValue(source);
  }
  await page.getByRole("button", { name: "Discard draft", exact: true }).click();
  await expect(editor).toHaveValue(source);
  await page.getByRole("button", { name: "Save source", exact: true }).click();
  await expect(
    page.getByText(
      "Source saved. Build and check its contract before running.",
    ),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Build & check contract", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/jobs`)).json())[0]
          .phase,
      { timeout: 90_000 },
    )
    .toBe("succeeded");
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .custom_rust.compatible,
    )
    .toBe(true);
  await expect(
    page.getByText("Build succeeded; contract is compatible", { exact: true }),
  ).toBeVisible();
  await expect(selectedStrategy).toHaveValue("custom_rust");
  await expect(page.getByLabel("gain", { exact: true })).toBeVisible();
  await page.getByLabel("gain", { exact: true }).fill("1.25");
  await page
    .getByRole("combobox", { name: "enabled", exact: true })
    .selectOption("true");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await page.getByRole("link", { name: "Simulation", exact: true }).click();
  await page
    .getByRole("button", { name: "Start saved simulation", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get("/api/execution/state")).json()).snapshot
          .last_outputs["mv.u1.write"]?.value,
    )
    .toBe(1.25);
  await expect.poll(async () =>
    (await (await request.get("/api/execution/state")).json()).snapshot
      .last_outputs["custom.lamp.write"],
  ).toEqual({ kind: "bool", value: false });
  await expect
    .poll(async () =>
      (
        await (
          await request.get(`/api/projects/${name}/logs?mode=simulation`)
        ).json()
      ).some((row: { user_logs?: { message: string }[] }) =>
        row.user_logs?.some((log) => log.message === "web_custom"),
      ),
    )
    .toBe(true);
  await page.getByRole("link", { name: "Logs & events", exact: true }).click();
  await page.getByLabel("Search logs").fill("web_custom");
  await expect(page.locator(".log-list details").first()).toBeVisible();
  await page
    .getByRole("button", { name: "Stop simulation", exact: true })
    .click();
  await expect
    .poll(async () => await (await request.get("/api/execution/state")).json())
    .toBeNull();
  await page
    .getByRole("link", { name: "Control strategy", exact: true })
    .click();
  await page
    .getByRole("button", {
      name: "Regenerate template · back up current source",
      exact: true,
    })
    .click();
  await expect(
    page.getByText(/Previous source and metadata backed up/),
  ).toBeVisible();
  await expect(selectedStrategy).toHaveValue("custom_rust");
  await expect(editor).not.toHaveValue(source);
  await editor.fill(
    (await editor.inputValue()) +
      "\nfn type_error() { let _ = unknown_identifier; }\n",
  );
  await page.getByRole("button", { name: "Save source", exact: true }).click();
  await page
    .getByRole("button", { name: "Build & check contract", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/jobs`)).json())[0]
          .phase,
      { timeout: 90000 },
    )
    .toBe("failed");
  const jobs = await (await request.get(`/api/projects/${name}/jobs`)).json();
  const report = await (
    await request.get(`/api/projects/${name}/jobs/${jobs[0].id}`)
  ).json();
  expect(report.result.output).toContain("unknown_identifier");
  expect(
    (await (await request.get(`/api/projects/${name}/config`)).json()).logic
      .custom_rust.compatible,
  ).toBe(false);
});
