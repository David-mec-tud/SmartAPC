import { test, expect } from "@playwright/test";
import { seed, stopTestExecution } from "./fixtures";

test.afterEach(async ({ request }) => stopTestExecution(request));

test("deployment links missing requirements to configuration and refreshes saved readiness", async ({
  page,
  request,
}) => {
  const name = `journey_deployment_${Date.now()}`;
  expect(
    (await request.post("/api/projects", { data: { name } })).ok(),
  ).toBeTruthy();
  await page.goto("/deployment");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  const requirements = page.getByRole("region", {
    name: "Prepare for shadow mode",
  });
  const steps = requirements.getByLabel("Remaining startup steps");
  await expect(steps).toContainText(
    "Define the MV/CV/DV catalog and I/O contract",
  );
  await expect(steps).toContainText("Selected strategy is not compatible");
  await expect(
    steps.getByRole("link", { name: "Resolve →" }).first(),
  ).toHaveAttribute("href", "/configuration/io");
  await expect(
    page.getByRole("button", { name: "Start in shadow mode", exact: true }),
  ).toBeDisabled();

  // Simulate a second client completing setup. Refresh must use Rust's latest
  // saved contract, without depending on local editor navigation or test jobs.
  const configured = await seed(request, false);
  const config = await (
    await request.get(`/api/projects/${configured}/config`)
  ).json();
  const response = await request.get(`/api/projects/${name}/config`);
  config.name = name;
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await requirements
    .getByRole("button", { name: "Refresh validation" })
    .click();
  await expect(
    requirements.getByText("Ready for shadow mode", { exact: true }),
  ).toBeVisible();
  await expect(steps).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Start in shadow mode", exact: true }),
  ).toBeEnabled();
});

test("blank optional status addresses allow shadow startup; drafts and other executions explain their gates", async ({
  page,
  request,
}) => {
  const name = await seed(request, false);
  const other = await seed(request, false);
  await page.goto("/deployment");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  const start = page.getByRole("button", {
    name: "Start in shadow mode",
    exact: true,
  });
  const requirements = page.getByRole("region", {
    name: "Prepare for shadow mode",
  });
  await expect(start).toBeEnabled();
  await expect(
    page.getByLabel("ControllerIsReady Node ID", { exact: true }),
  ).toHaveValue("");
  await expect(
    page.getByLabel("ControllerHasControl Node ID", { exact: true }),
  ).toHaveValue("");
  await expect(
    page.getByText("ControllerIsReady Node ID (optional)", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("ControllerHasControl Node ID (optional)", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Test saved controller signals" }),
  ).toBeDisabled();

  await page
    .getByLabel("ControllerIsReady Node ID", { exact: true })
    .fill("ns=2;s=OptionalReady");
  await expect(start).toBeDisabled();
  await expect(requirements).toContainText(
    "Save configuration in the bottom bar, or discard your draft.",
  );
  await page
    .getByRole("button", { name: "Discard draft", exact: true })
    .click();
  await expect(start).toBeEnabled();
  await start.click();
  await expect
    .poll(async () => {
      const execution = await (
        await request.get("/api/execution/state")
      ).json();
      return execution?.snapshot.metrics.total_cycles ?? 0;
    })
    .toBeGreaterThan(2);
  const execution = await (await request.get("/api/execution/state")).json();
  expect(execution.snapshot.project_name).toBe(name);
  expect(execution.snapshot.controller.enabled).toBe(false);
  expect(execution.snapshot.controller.has_control).toBe(false);
  const verify = page.getByRole("region", {
    name: "Verify before enabling control",
  });
  await expect(
    verify.getByText("ControllerIsReady", { exact: true }),
  ).toBeVisible();
  await expect(
    verify.getByText("ControllerHasControl", { exact: true }),
  ).toBeVisible();
  await expect(start).toBeDisabled();

  await page.getByLabel("Editing project", { exact: true }).selectOption(other);
  await expect(requirements).toContainText(
    `${name} is already executing in runtime mode.`,
  );
  await expect(start).toBeDisabled();
  await page.getByRole("button", { name: "Stop runtime", exact: true }).click();
  await expect(start).toBeEnabled();
});
