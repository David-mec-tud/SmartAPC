import { test, expect } from "@playwright/test";
import { seed, stopTestExecution } from "./fixtures";
test.afterEach(async ({ request }) => stopTestExecution(request));

test("mixed dashboard paginates, edits, reorders and persists every panel", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  await page.goto("/operation/trends");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await expect(page.locator(".chart-panel")).toHaveCount(2);
  for (let index = 1; index <= 3; index++) {
    await page.getByRole("button", { name: "Add plot", exact: true }).click();
    await page
      .getByLabel("Plot title", { exact: true })
      .fill(`Process ${index}`);
    await page.getByLabel("Search plot signals", { exact: true }).fill("Y1");
    await page.getByRole("radio", { name: "y1", exact: true }).check();
    await page.getByRole("button", { name: "Apply plot", exact: true }).click();
  }
  await expect(page.getByText("Page 2 of 2", { exact: false })).toBeVisible();
  await expect(page.locator(".chart-panel")).toHaveCount(1);
  await page.getByRole("button", { name: "Edit plot", exact: true }).click();
  await page.getByLabel("Plot title", { exact: true }).fill("Final process");
  await page.getByRole("button", { name: "Apply plot", exact: true }).click();
  await page
    .getByRole("button", { name: "Move Final process earlier", exact: true })
    .click();
  await page.getByLabel("Rows", { exact: true }).fill("4");
  await expect(page.getByLabel("Dashboard layout")).toHaveValue("custom");
  await page.getByLabel("Dashboard layout").selectOption("2x5");
  await expect(page.locator(".chart-panel")).toHaveCount(5);
  await page
    .getByRole("button", { name: "Save dashboard", exact: true })
    .click();
  await expect(page.getByText(/Dashboard changes are unsaved/)).toHaveCount(0);
  await page.reload();
  await expect(page.getByLabel("Rows", { exact: true })).toHaveValue("5");
  await expect(page.locator(".chart-panel")).toHaveCount(5);
  const plots = await (
    await request.get(`/api/projects/${name}/plots/config`)
  ).json();
  expect(
    new Set(plots.dashboard.panels.map((panel: { id: string }) => panel.id))
      .size,
  ).toBe(5);
  expect(plots.dashboard.panels[3].title).toBe("Final process");
  await page.getByLabel("Columns", { exact: true }).fill("1");
  await page.getByLabel("Rows", { exact: true }).fill("1");
  await expect(page.getByText("Page 1 of 5", { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "Remove plot", exact: true }).click();
  await expect(page.getByText("Page 1 of 4", { exact: false })).toBeVisible();
});

test("MPC parameter groups search labels and keys without losing numeric or Boolean drafts", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const other = await seed(request);
  const response = await request.get(`/api/projects/${name}/config`);
  const config = await response.json();
  config.logic.kind = "linear_mpc";
  config.io_definition.cvs[0].band_low_value = -0.5;
  config.io_definition.cvs[0].band_high_value = 0.5;
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
  await page
    .getByRole("button", { name: "Start saved simulation", exact: true })
    .click();
  await page
    .getByRole("link", { name: "Online parameters", exact: true })
    .click();
  await page.getByLabel("Search parameters", { exact: true }).fill("TRACKING");
  await expect(page.getByText("1 of", { exact: false })).toBeVisible();
  await page.getByLabel("mpc.cv.y1.q", { exact: true }).fill("2.5");
  await page.getByLabel("Search parameters", { exact: true }).fill("USE_L1");
  await page
    .getByLabel("mpc.mv.u1.use_l1", { exact: true })
    .selectOption("true");
  await page
    .getByRole("row")
    .filter({ hasText: "mpc.mv.u1.use_l1" })
    .getByRole("button", { name: "Apply online", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/projects/${name}/config`)).json()).logic
          .params["mpc.mv.u1.use_l1"],
    )
    .toBe(true);
  await page
    .getByLabel("Search parameters", { exact: true })
    .fill("mpc.CV.Y1.Q");
  await expect(page.getByLabel("mpc.cv.y1.q", { exact: true })).toHaveValue(
    "2.5",
  );
  await page
    .getByLabel("Search parameters", { exact: true })
    .fill("does not exist");
  await expect(
    page.getByText("No parameters match your search."),
  ).toBeVisible();
  await page.getByLabel("Editing project", { exact: true }).selectOption(other);
  await expect(
    page.getByText(
      "Select the active execution project to edit its online parameters.",
    ),
  ).toBeVisible();
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page.getByLabel("Search parameters", { exact: true }).fill("TRACKING");
  await expect(page.getByLabel("mpc.cv.y1.q", { exact: true })).toHaveValue(
    "1",
  );
});
