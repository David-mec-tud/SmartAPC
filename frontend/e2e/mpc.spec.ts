import { test, expect } from "@playwright/test";
import { seed, stopTestExecution } from "./fixtures";

test.afterEach(async ({ request }) => stopTestExecution(request));

test("MPC objectives autosave typed modes and priorities, follow renamed variables and drive simulation within bands", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  config.logic.kind = "linear_mpc";
  config.logic.params = {
    "mpc.prediction_horizon": 5,
    "mpc.control_horizon": 2,
  };
  // A direct process makes the saved objectives observable without waiting for lag.
  Object.assign(config.model, { b: [[0]], c: [[0]], d: [[1]] });
  config.io_definition.cvs[0].band_low_value = -0.5;
  config.io_definition.cvs[0].band_high_value = 0.5;
  config.simulation.cv_setpoints = { y1: 0 };
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": saved.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();

  await page.goto("/configuration/strategy");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await expect(page.getByLabel("MV u1 optimisation mode")).toHaveValue(
    "minimum_move",
  );
  await expect(page.getByLabel("CV y1 priority")).toHaveValue("0");
  await page.getByLabel("CV y1 optimisation mode").selectOption("maximize");
  await page.getByLabel("MV u1 optimisation mode").selectOption("minimize");
  await page.getByLabel("MV u1 priority").fill("2");
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .linear_mpc,
    )
    .toEqual({
      mvs: { u1: { mode: "minimize", priority: 2 } },
      cvs: { y1: { mode: "maximize", priority: 0 } },
    });
  await page.reload();
  await expect(page.getByLabel("CV y1 optimisation mode")).toHaveValue(
    "maximize",
  );
  await expect(page.getByLabel("MV u1 priority")).toHaveValue("2");

  await page.getByLabel("MV u1 priority").fill("-1");
  await expect(
    page.getByText(/Strategy changes could not be saved/),
  ).toBeVisible();
  await expect(page.getByLabel("MV u1 priority")).toHaveValue("-1");
  expect(
    (await (await request.get(`/api/projects/${name}/config`)).json()).logic
      .linear_mpc.mvs.u1.priority,
  ).toBe(2);
  await page.getByLabel("MV u1 priority").fill("1");
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .linear_mpc.mvs.u1.priority,
    )
    .toBe(1);

  await page
    .getByRole("link", { name: "Variables & I/O", exact: true })
    .click();
  await page.getByLabel("mvs variable 1 name").fill("actuator");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .linear_mpc.mvs,
    )
    .toEqual({ actuator: { mode: "minimize", priority: 1 } });
  await page
    .getByRole("link", { name: "Control strategy", exact: true })
    .click();
  await expect(page.getByLabel("MV actuator priority")).toHaveValue("1");

  await page.getByRole("link", { name: "Simulation", exact: true }).click();
  await page
    .getByRole("button", { name: "Start saved simulation", exact: true })
    .click();
  await expect
    .poll(async () => {
      const state = await (await request.get("/api/execution/state")).json();
      return state?.snapshot.last_outputs["mv.actuator.write"]?.value;
    })
    .toBeGreaterThan(0.49);
  const state = await (await request.get("/api/execution/state")).json();
  expect(state.snapshot.last_outputs["mv.actuator.write"].value).toBeLessThan(
    0.501,
  );
  await page
    .getByRole("link", { name: "Control strategy", exact: true })
    .click();
  await expect(page.getByLabel("CV y1 optimisation mode")).toBeDisabled();
  await expect(page.getByLabel("MV actuator priority")).toBeDisabled();
});
