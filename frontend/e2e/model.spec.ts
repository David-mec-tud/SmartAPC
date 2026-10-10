import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { seed } from "./fixtures";

test("one model CSV template follows draft signals and imports every matrix atomically", async ({
  page,
  request,
}) => {
  const name = await seed(request, false);
  const saved = await request.get(`/api/projects/${name}/config`);
  const config = await saved.json();
  config.model = null;
  config.variables.mvs.push("valve2");
  config.io_definition.mvs.push({
    name: "valve2",
    mv_read: "ns=2;s=V2_READ",
    mv_write: "ns=2;s=V2_WRITE",
  });
  config.variables.cvs.push("pressure");
  config.io_definition.cvs.push({
    name: "pressure",
    setpoint: "ns=2;s=P_SP",
    process_value: "ns=2;s=P_PV",
  });
  config.variables.dvs.push("feed");
  config.io_definition.dvs.push({ name: "feed", read: "ns=2;s=FEED" });
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
  await page.getByLabel("mvs variable 1 name").fill("actuator");
  await page.getByRole("link", { name: "Plant model", exact: true }).click();
  // The template is available even before creating this optional model.
  const firstDownload = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Download model CSV template" })
    .click();
  expect(
    await readFile((await (await firstDownload).path())!, "utf8"),
  ).toContain("B,actuator,valve2,feed\nx1,b11,b12,b13");
  await page.getByLabel("State dimensions").fill("2");
  const pending = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Download model CSV template" })
    .click();
  const template = await readFile((await (await pending).path())!, "utf8");
  expect(template).toContain("A,x1,x2\nx1,a11,a12\nx2,a21,a22");
  expect(template).toContain(
    "B,actuator,valve2,feed\nx1,b11,b12,b13\nx2,b21,b22,b23",
  );
  expect(template).toContain("C,x1,x2\ny1,c11,c12\npressure,c21,c22");
  expect(template).toContain(
    "D,actuator,valve2,feed\ny1,d11,d12,d13\npressure,d21,d22,d23",
  );
  expect(
    (await (await request.get(`/api/projects/${name}/config`)).json()).model ??
      null,
  ).toBeNull();
  await expect(page.getByLabel("Import A matrix")).toHaveCount(0);
  const upload = async (csv: string) =>
    page.getByLabel("Import model CSV", { exact: true }).setInputFiles({
      name: "model.csv",
      mimeType: "text/csv",
      buffer: Buffer.from(csv),
    });
  await upload(template);
  await expect(page.getByRole("alert")).toContainText(
    "replace all placeholders",
  );
  // Distinct coefficients prove that every matrix/row/column stays in order.
  const filled = template.replace(
    /([abcd])(\d)(\d)/g,
    (_match, matrix: string, row: string, col: string) =>
      String(
        ("abcd".indexOf(matrix) + 1) * 100 + Number(row) * 10 + Number(col),
      ),
  );
  await upload(filled.replace("pressure,421,422,423", "pressure,421,,423"));
  await expect(page.getByRole("alert")).toContainText(
    "D row pressure, column valve2",
  );
  await expect(page.getByLabel("A row 1 column 1")).toHaveValue("0");
  await upload(
    filled.replace("B,actuator,valve2,feed", "B,feed,valve2,actuator"),
  );
  await expect(page.getByRole("alert")).toContainText("B header must match");
  await upload(filled);
  await expect(
    page.getByText(
      "All four matrices imported into the draft. Save configuration to keep them.",
    ),
  ).toBeVisible();
  await expect(page.getByLabel("A row 2 column 2")).toHaveValue("122");
  await expect(page.getByLabel("B row 2 column 3")).toHaveValue("223");
  await expect(page.getByLabel("C row 2 column 2")).toHaveValue("322");
  await expect(page.getByLabel("D row 2 column 3")).toHaveValue("423");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    page.getByText("Configuration saved and normalized by Rust."),
  ).toBeVisible();
  await page.reload();
  await expect(page.getByLabel("D row 2 column 3")).toHaveValue("423");
  const model = (
    await (await request.get(`/api/projects/${name}/config`)).json()
  ).model;
  expect(model.u_order).toEqual(["actuator", "valve2", "feed"]);
  expect(model.y_order).toEqual(["y1", "pressure"]);
  expect(model.a).toEqual([
    [111, 112],
    [121, 122],
  ]);
  expect(model.b).toEqual([
    [211, 212, 213],
    [221, 222, 223],
  ]);
  expect(model.c).toEqual([
    [311, 312],
    [321, 322],
  ]);
  expect(model.d).toEqual([
    [411, 412, 413],
    [421, 422, 423],
  ]);
});

test("a delayed CSV import preserves newer edits and cannot cross projects", async ({
  page,
  request,
}) => {
  const name = await seed(request);
  const other = await seed(request);
  await page.goto("/configuration/model");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  const csv = "A,x1\nx1,-9\nB,u1\nx1,2\nC,x1\ny1,3\nD,u1\ny1,4\n";
  let release!: () => void;
  let arrived!: () => void;
  const hold = () =>
    new Promise<void>((resolve) => {
      release = resolve;
    });
  let gate = hold();
  let started = new Promise<void>((resolve) => {
    arrived = resolve;
  });
  await page.route("**/model/import", async (route) => {
    arrived();
    await gate;
    await route.continue();
  });
  const upload = () =>
    page.getByLabel("Import model CSV", { exact: true }).setInputFiles({
      name: "model.csv",
      mimeType: "text/csv",
      buffer: Buffer.from(csv),
    });
  await upload();
  await started;
  await page.getByLabel("A row 1 column 1").fill("-5");
  release();
  await expect(page.getByRole("alert")).toContainText(
    "Configuration changed during the CSV request",
  );
  await expect(page.getByLabel("A row 1 column 1")).toHaveValue("-5");
  gate = hold();
  started = new Promise<void>((resolve) => {
    arrived = resolve;
  });
  await upload();
  await started;
  await page.getByLabel("Editing project", { exact: true }).selectOption(other);
  const response = page.waitForResponse((response) =>
    response.url().endsWith(`/projects/${name}/model/import`),
  );
  release();
  await response;
  await expect(page.getByLabel("A row 1 column 1")).toHaveValue("-1");
  await expect(
    page.getByText(
      "All four matrices imported into the draft. Save configuration to keep them.",
    ),
  ).toHaveCount(0);
});
