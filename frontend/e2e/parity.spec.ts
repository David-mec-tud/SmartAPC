import { test, expect, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { seed, stopTestExecution } from "./fixtures";
test.afterEach(async ({ request }) => stopTestExecution(request));
async function select(page: Page, name: string, path: string) {
  await page.goto(path);
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
}
async function save(page: Page) {
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    page.getByText("Configuration saved and normalized by Rust."),
  ).toBeVisible();
}

test("TOML drafts, variable references, matrix import errors and saved downloads", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  await select(page, name, "/configuration/settings");
  const toml = await (
    await request.get(`/api/projects/${name}/config/download`)
  ).text();
  await page.getByLabel("Import project TOML").setInputFiles({
    name: "project.toml",
    mimeType: "text/plain",
    buffer: Buffer.from(toml),
  });
  await page
    .getByRole("link", { name: "Variables & I/O", exact: true })
    .click();
  await page.getByLabel("mvs variable 1 name").fill("actuator");
  await save(page);
  const renamed = await (
    await request.get(`/api/projects/${name}/config`)
  ).json();
  expect(renamed.variables.mvs).toEqual(["actuator"]);
  expect(renamed.logic.p_controller.pairs[0].mv).toBe("actuator");
  expect(renamed.model.u_order).toEqual(["actuator"]);
  expect(renamed.simulation.mv_initials).toHaveProperty("actuator");
  await page.getByRole("link", { name: "Plant model", exact: true }).click();
  await page.getByLabel("State dimensions").fill("2");
  await page.getByLabel("Import model CSV", { exact: true }).setInputFiles({
    name: "invalid.csv",
    mimeType: "text/csv",
    buffer: Buffer.from("1,2,3\n"),
  });
  await expect(page.getByRole("alert")).toContainText("A header must match");
  await page.getByLabel("Import model CSV", { exact: true }).setInputFiles({
    name: "model.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(
      "A,x1,x2\nx1,-1,0\nx2,0,-2\nB,actuator\nx1,1\nx2,0\nC,x1,x2\ny1,1,0\nD,actuator\ny1,0\n",
    ),
  });
  await expect(page.getByLabel("A row 2 column 2")).toHaveValue("-2");
  await save(page);
  await page.reload();
  await expect(page.getByLabel("A row 2 column 2")).toHaveValue("-2");
  await page
    .getByRole("link", { name: "Project settings", exact: true })
    .click();
  const pending = page.waitForEvent("download");
  await page
    .getByRole("link", { name: "Download saved TOML", exact: true })
    .click();
  expect(await readFile((await (await pending).path())!, "utf8")).toContain(
    '"actuator"',
  );
});

test("live simulation settings, concurrent applied values and persistent custom plots", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  await select(page, name, "/engineering/simulation");
  await page
    .getByRole("button", { name: "Start saved simulation", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Stop simulation", exact: true }).first(),
  ).toBeEnabled();
  await page.getByLabel("Simulation speed").selectOption("x10");
  await page.getByLabel("A scaling (%) row 1 column 1").fill("90");
  await page.getByLabel("B scaling (%) row 1 column 1").fill("110");
  await page
    .locator("section")
    .filter({
      has: page.getByRole("heading", { name: "cv noise std", exact: true }),
    })
    .getByLabel("y1", { exact: true })
    .fill("0.1");
  await page
    .getByRole("button", {
      name: "Apply simulation settings live",
      exact: true,
    })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json())
          .simulation.b_scale_pct[0][0],
    )
    .toBe(110);
  await page
    .getByRole("link", { name: "Online parameters", exact: true })
    .click();
  const key = "p.y1.u1.kp";
  await page.getByLabel(key, { exact: true }).fill("2.5");
  const state = await (await request.get("/api/execution/state")).json();
  const identity = {
    project_name: name,
    mode: "simulation",
    execution_id: state.snapshot.execution_id,
  };
  expect(
    (
      await request.post("/api/execution/parameters", {
        data: {
          ...identity,
          params: { [key]: 3 },
          expected_params: { [key]: 1 },
        },
      })
    ).ok(),
  ).toBeTruthy();
  await page
    .getByRole("row")
    .filter({ hasText: key })
    .getByRole("button", { name: "Apply online" })
    .click();
  await expect(
    page.getByText(
      "applied parameter changed; refresh before applying this draft",
    ),
  ).toBeVisible();
  await expect(page.getByLabel(key, { exact: true })).toHaveValue("2.5");
  await page
    .getByRole("button", { name: `Reload ${key} applied value`, exact: true })
    .click();
  await expect(page.getByLabel(key, { exact: true })).toHaveValue("3");
  await page.getByRole("link", { name: "Trends", exact: true }).click();
  await expect(page.locator(".js-plotly-plot").first()).toBeVisible();
  await page.getByLabel("Sliding history (s)").fill("30");
  await page.getByLabel("Future horizon (s)").focus();
  await page.getByRole("button", { name: "Custom plots", exact: true }).click();
  await page.getByLabel("Plot title").fill("Actuator and process");
  await page
    .locator(".series-picker label")
    .filter({ hasText: "cv.y1.process_value" })
    .first()
    .getByRole("checkbox")
    .check();
  await page
    .locator(".series-picker label")
    .filter({ hasText: "mv.u1.write" })
    .first()
    .getByRole("checkbox")
    .check();
  await page
    .getByRole("button", { name: "Save custom plot", exact: true })
    .click();
  await expect(
    page.getByText("Actuator and process", { exact: true }).first(),
  ).toBeVisible();
  await page.reload();
  await expect(page.getByLabel("Sliding history (s)")).toHaveValue("30");
  await page.getByRole("button", { name: "Custom plots", exact: true }).click();
  await expect(
    page.getByText("Actuator and process", { exact: true }).first(),
  ).toBeVisible();
});

test("selected sweeps download full reports; deployment copies and downloads commands", async ({
  page,
  request,
  context,
}) => {
  const name = await seed(request, false);
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await select(page, name, "/engineering/tests");
  await page.getByLabel("Sweep p.y1.u1.kp", { exact: true }).check();
  await page.getByLabel("p.y1.u1.kp min", { exact: true }).fill("0.5");
  await page.getByLabel("p.y1.u1.kp max", { exact: true }).fill("2");
  await page.getByLabel("p.y1.u1.kp samples", { exact: true }).fill("3");
  await page
    .getByRole("button", { name: "Run control tests", exact: true })
    .click();
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  if ((await page.locator(".job").first().getAttribute("open")) === null)
    await page.locator(".job summary").first().click();
  const pending = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Download report", exact: true })
    .click();
  const report = JSON.parse(
    await readFile((await (await pending).path())!, "utf8"),
  );
  // The report includes the unchanged baseline plus the three sweep samples.
  expect(report.result.execution.executed_case_count).toBe(4);
  expect(
    (await (await request.get(`/api/projects/${name}/jobs`)).json())[0].result,
  ).toBeNull();
  await page.getByRole("link", { name: "Validate & run", exact: true }).click();
  await expect(
    page.getByText("Ready for shadow mode", { exact: true }),
  ).toBeVisible();
  await page
    .getByText("Start from a terminal instead", { exact: true })
    .click();
  await page
    .getByRole("button", { name: "Copy local run command", exact: true })
    .click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(
    name,
  );
  await page
    .getByText("Windows service command reference", { exact: true })
    .click();
  const commands = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Download commands", exact: true })
    .click();
  expect(await readFile((await (await commands).path())!, "utf8")).toContain(
    "sc.exe",
  );
});

test("offline cancellation unlocks configuration without applying partial results", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  Object.assign(config.optimization, {
    max_iterations: 10000,
    initial_samples: 100,
    simulation_duration_s: 100,
    scenario_count: 4,
  });
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await select(page, name, "/engineering/optimization");
  await page
    .getByRole("button", { name: "Start saved optimization", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Cancel optimization", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Cancel optimization", exact: true })
    .click();
  await expect(page.getByText("cancelled", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Start saved optimization", exact: true }),
  ).toBeEnabled();
  expect(
    (await (await request.get(`/api/projects/${name}/config`)).json()).logic
      .params,
  ).toEqual(config.logic.params);
});

test("online objectives enforce safety and normal completion applies best values", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const response = await request.get(`/api/projects/${name}/config`);
  const config = await response.json();
  config.online_optimization = {
    parameters: [{ key: "p.y1.u1.kp", min: 0.5, max: 2 }],
    cvs: [{ name: "y1", weight: 1, low: -99, high: 99 }],
    delta_mvs: [],
    duration_s: 1,
    max_iterations: 1,
    initial_samples: 1,
    convergence_patience: 1,
  };
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await select(page, name, "/operation/optimization");
  await page.getByLabel("y1 low", { exact: true }).fill("-101");
  await page
    .getByRole("button", { name: "Save optimization objectives", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText(
    /limit|predefined|safety/i,
  );
  expect(
    (await (await request.get(`/api/projects/${name}/config`)).json())
      .online_optimization.cvs[0].low,
  ).toBe(-99);
  await page.getByLabel("y1 low", { exact: true }).fill("-90");
  await page
    .getByRole("button", { name: "Save optimization objectives", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json())
          .online_optimization.cvs[0].low,
    )
    .toBe(-90);
  await page.getByRole("link", { name: "Simulation", exact: true }).click();
  await page
    .getByRole("button", { name: "Start saved simulation", exact: true })
    .click();
  await page
    .getByRole("link", { name: "Online optimization", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Start saved optimization", exact: true })
    .click();
  await expect(page.getByText("finished", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  const optimization = await (
    await request.get("/api/execution/optimization")
  ).json();
  const execution = await (await request.get("/api/execution/state")).json();
  const applied = await (
    await request.get(
      `/api/execution/parameters?project_name=${name}&mode=simulation&execution_id=${execution.snapshot.execution_id}`,
    )
  ).json();
  expect(applied.params["p.y1.u1.kp"]).toBe(
    optimization.best_params["p.y1.u1.kp"],
  );
  await page
    .getByRole("button", { name: "Save best-known parameters", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .params["p.y1.u1.kp"],
    )
    .toBe(optimization.best_params["p.y1.u1.kp"]);
});
