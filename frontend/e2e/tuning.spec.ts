import { test, expect } from "@playwright/test";
import { seed, stopTestExecution } from "./fixtures";
test.afterEach(async ({ request }) => stopTestExecution(request));

test("Kalman CSV upload replays measured data, charts results and saves observer values", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  config.logic.kind = "linear_mpc";
  config.logic.params = {};
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/engineering/kalman");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  // The backend determines the observer schema; use its returned numeric key.
  const catalog = await (
    await request.get(`/api/projects/${name}/engineering`)
  ).json();
  const key = Object.keys(catalog.kalman_parameters).find((key) =>
    key.endsWith("initial_p"),
  )!;
  await page.getByLabel(key, { exact: true }).fill("2");
  await page.getByLabel("Kalman CSV").setInputFiles({
    name: "measurements.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(
      "time,y1,u1\n2026-10-04T12:00:00Z,0,0\n2026-10-04T12:00:00.1Z,0.1,1\n2026-10-04T12:00:00.2Z,0.2,1\n",
    ),
  });
  await page.getByRole("button", { name: "Run observer", exact: true }).click();
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  await page.locator(".job summary").first().click();
  await expect(page.locator(".js-plotly-plot").first()).toBeVisible();
  await expect(page.getByLabel(key, { exact: true })).toHaveValue("2");
  await page
    .getByRole("button", {
      name: "Stage observer parameters for save",
      exact: true,
    })
    .click();
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .params[key],
    )
    .toBe(2);
  await page.reload();
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
});

test("offline optimization reports curves and applies results; online manual stop restores originals and unlocks edits", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  Object.assign(config.optimization, {
    scenario_count: 1,
    max_iterations: 2,
    initial_samples: 1,
    convergence_patience: 2,
    plot_interval: 1,
    simulation_duration_s: 1,
  });
  config.optimization.parameters.forEach(
    (row: {
      key: string;
      fixed: boolean;
      start: number;
      min: number;
      max: number;
    }) => {
      row.fixed = row.key !== "p.y1.u1.kp";
      if (!row.fixed) Object.assign(row, { start: 1, min: 0.5, max: 2 });
    },
  );
  config.online_optimization = {
    parameters: [{ key: "p.y1.u1.kp", min: 0.5, max: 2 }],
    cvs: [{ name: "y1", weight: 1, low: -99, high: 99 }],
    delta_mvs: [],
    duration_s: 1,
    max_iterations: 100,
    initial_samples: 2,
    convergence_patience: 100,
  };
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/engineering/optimization");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page
    .getByRole("button", { name: "Start saved optimization", exact: true })
    .click();
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  await page.locator(".job summary").first().click();
  await expect(page.locator(".js-plotly-plot").first()).toBeVisible();
  await page
    .getByRole("button", {
      name: "Apply best parameters to saved project",
      exact: true,
    })
    .click();
  const jobs = await (await request.get(`/api/projects/${name}/jobs`)).json();
  const report = await (
    await request.get(`/api/projects/${name}/jobs/${jobs[0].id}`)
  ).json();
  const optimized = report.result.best_params["p.y1.u1.kp"];
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .params["p.y1.u1.kp"],
    )
    .toBe(optimized);
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
  await expect(page.getByText("running", { exact: true })).toBeVisible();
  await page
    .getByRole("link", { name: "Online parameters", exact: true })
    .click();
  await expect(page.getByLabel("p.y1.u1.kp", { exact: true })).toBeDisabled();
  await page
    .getByRole("link", { name: "Online optimization", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Stop & restore originals", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get("/api/execution/optimization")).json())
          ?.phase,
    )
    .toBe("finished");
  const state = await (await request.get("/api/execution/state")).json();
  const paramsUrl = `/api/execution/parameters?project_name=${name}&mode=simulation&execution_id=${state.snapshot.execution_id}`;
  await expect
    .poll(
      async () =>
        (await (await request.get(paramsUrl)).json()).params["p.y1.u1.kp"],
    )
    .toBe(optimized);
  await page
    .getByRole("link", { name: "Online parameters", exact: true })
    .click();
  await expect(page.getByLabel("p.y1.u1.kp", { exact: true })).toBeEnabled();
  await page
    .getByRole("button", { name: "Stop simulation", exact: true })
    .click();
});
